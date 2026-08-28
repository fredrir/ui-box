import { type ChildProcess, spawn } from "node:child_process";
import { DEFAULT_TIMEOUT_MS, type DeterminismPlan, envNumber, envString } from "../config.js";
import { DriverError, RPC_ATTACH_FAILED } from "../errors.js";
import type {
  ReadinessReport,
  RuntimeConfig,
  RuntimeDrain,
  SnapshotConfig,
} from "../injected/runtime.js";
import { uiboxRuntime } from "../injected/runtime.js";
import { isoFrom } from "../recorder.js";
import type { ParsedSelector } from "../selector.js";
import type { DrainedEvents, OpenOptions, Surface, Viewport } from "../types.js";
import { WebDriverClient } from "../webdriver/client.js";
import { keyActions, parseChord } from "../webdriver/keys.js";
import { type Backend, type FillOptions, toRuntimeSpec } from "./index.js";

const RUNTIME_SOURCE = uiboxRuntime.toString();
const DRIVER_BOOT_TIMEOUT_MS = 20_000;
const POLL_INTERVAL_MS = 100;

export class WebDriverBackend implements Backend {
  readonly surface: Surface = "tauri";
  readonly viewport: Viewport;

  private readonly client: WebDriverClient;
  private readonly child: ChildProcess | null;
  private readonly runtimeConfig: RuntimeConfig;
  private markCounter = 0;
  private pendingErrors: string[] = [];

  private constructor(
    client: WebDriverClient,
    child: ChildProcess | null,
    viewport: Viewport,
    runtimeConfig: RuntimeConfig,
  ) {
    this.client = client;
    this.child = child;
    this.viewport = viewport;
    this.runtimeConfig = runtimeConfig;
  }

  static async attach(
    target: string,
    viewport: Viewport,
    options: OpenOptions,
    plan: DeterminismPlan,
  ): Promise<WebDriverBackend> {
    const runtimeConfig: RuntimeConfig = {
      seed: plan.seed,
      fixedTimeMs: plan.fixedTimeMs,
      disableAnimations: plan.disableAnimations,
      probeConsole: true,
      probeNetwork: true,
      captureAll: options.captureConsole === "all",
      maxEvents: 500,
    };

    let child: ChildProcess | null = null;
    let baseUrl = options.webdriverUrl ?? envString("UIBOX_WEBDRIVER_URL") ?? null;

    if (!baseUrl) {
      const port = options.webdriverPort ?? envNumber("UIBOX_WEBDRIVER_PORT") ?? 4444;
      baseUrl = `http://127.0.0.1:${port}`;
      child = spawnTauriDriver(port, options, plan);
    }

    const client = new WebDriverClient(baseUrl);
    await waitForDriver(
      client,
      child,
      baseUrl,
      options.driverBootTimeoutMs ?? DRIVER_BOOT_TIMEOUT_MS,
    );

    if (options.webdriverSessionId) {
      client.adoptSession(options.webdriverSessionId);
    } else {
      await client.newSession(buildCapabilities(target, options, plan)).catch((err) => {
        killChild(child);
        throw new DriverError(
          "attach",
          `webdriver refused to start a session for ${target}: ${(err as Error).message}`,
          RPC_ATTACH_FAILED,
        );
      });
    }

    const backend = new WebDriverBackend(client, child, viewport, runtimeConfig);
    await client
      .setTimeouts({ implicit: 0, pageLoad: 60_000, script: 30_000 })
      .catch(() => undefined);
    await client
      .setWindowRect({ width: viewport.width, height: viewport.height, x: 0, y: 0 })
      .catch(() => undefined);
    await backend.ensureRuntime();
    return backend;
  }

  async goto(url: string, timeoutMs: number): Promise<void> {
    await this.client.setTimeouts({ pageLoad: timeoutMs }).catch(() => undefined);
    await this.client.navigateTo(url);
    await this.ensureRuntime();
  }

  async click(selector: ParsedSelector, timeoutMs: number): Promise<void> {
    const elementId = await this.resolveOne(selector, timeoutMs);
    await this.client.elementClick(elementId);
  }

  async fill(
    selector: ParsedSelector,
    text: string,
    options: FillOptions,
    timeoutMs: number,
  ): Promise<void> {
    const elementId = await this.resolveOne(selector, timeoutMs);
    if (options.clear) await this.client.elementClear(elementId);
    await this.client.elementSendKeys(elementId, text);
  }

  async press(key: string): Promise<void> {
    await this.client.performActions(keyActions(parseChord(key)));
    await this.client.releaseActions();
  }

  async waitFor(selector: ParsedSelector, timeoutMs: number): Promise<void> {
    await this.waitForMatches(selector, timeoutMs);
  }

  async countMatches(selector: ParsedSelector): Promise<number> {
    await this.ensureRuntime();
    const token = this.nextToken();
    const count = (await this.call("mark", [toRuntimeSpec(selector), token])) as number;
    await this.call("clearMarks", [token]);
    return count;
  }

  async textOf(selector: ParsedSelector, limit: number): Promise<string[]> {
    await this.ensureRuntime();
    return (await this.call("textOf", [toRuntimeSpec(selector), limit])) as string[];
  }

  async evaluate(expr: string): Promise<unknown> {
    const trimmed = expr.trim();
    const isFunction = /^(async\s+)?(function\b|\(|[A-Za-z_$][\w$]*\s*=>)/.test(trimmed);
    const script = isFunction ? `return (${trimmed})();` : `return (${trimmed});`;
    return this.client.executeScript(script, []);
  }

  async snapshotText(config: SnapshotConfig): Promise<string> {
    await this.ensureRuntime();
    return (await this.call("snapshot", [config])) as string;
  }

  async readiness(): Promise<ReadinessReport> {
    await this.ensureRuntime();
    return (await this.call("readiness", [])) as ReadinessReport;
  }

  async screenshot(): Promise<Buffer> {
    return this.client.takeScreenshot();
  }

  async currentUrl(): Promise<string> {
    return this.client.getCurrentUrl();
  }

  async drain(): Promise<DrainedEvents> {
    await this.ensureRuntime();
    const drained = (await this.call("drain", [])) as RuntimeDrain;
    for (const entry of drained.console) {
      if (entry.type === "pageerror") this.pendingErrors.push(entry.text);
    }
    return {
      console: drained.console.map((entry) => ({
        ts: isoFrom(entry.ts),
        type: entry.type,
        text: entry.text,
        location: entry.location,
      })),
      network: drained.network.map((entry) => ({
        ts: isoFrom(entry.ts),
        method: entry.method,
        url: entry.url,
        status: entry.status,
        failure: entry.failure,
      })),
    };
  }

  freshPageErrors(): string[] {
    const fresh = this.pendingErrors;
    this.pendingErrors = [];
    return fresh;
  }

  async dispose(): Promise<void> {
    await this.client.deleteSession().catch(() => undefined);
    killChild(this.child);
  }

  private nextToken(): string {
    this.markCounter += 1;
    return `uibox-${this.markCounter}`;
  }

  private async call(method: string, args: unknown[]): Promise<unknown> {
    const script = `return window.__uibox["${method}"].apply(null, arguments);`;
    return this.client.executeScript(script, args);
  }

  private async ensureRuntime(): Promise<void> {
    const present = await this.client.executeScript("return Boolean(window.__uibox);", []);
    if (present === true) return;
    await this.client.executeScript(`(${RUNTIME_SOURCE})(arguments[0]);`, [this.runtimeConfig]);
  }

  private async waitForMatches(selector: ParsedSelector, timeoutMs: number): Promise<string> {
    const deadline = Date.now() + Math.max(timeoutMs, 0);
    const spec = toRuntimeSpec(selector);
    let lastCount = 0;
    for (;;) {
      await this.ensureRuntime();
      const token = this.nextToken();
      lastCount = (await this.call("mark", [spec, token])) as number;
      if (lastCount === 1) return token;
      if (lastCount > 1) {
        await this.call("clearMarks", [token]);
        throw new DriverError(
          "strictness",
          `selector "${selector.raw}" matched ${lastCount} elements; refine it to match exactly one`,
        );
      }
      await this.call("clearMarks", [token]);
      if (Date.now() >= deadline) {
        throw new DriverError(
          "timeout",
          `timed out after ${timeoutMs}ms waiting for "${selector.raw}"`,
        );
      }
      await delay(POLL_INTERVAL_MS);
    }
  }

  private async resolveOne(selector: ParsedSelector, timeoutMs: number): Promise<string> {
    const token = await this.waitForMatches(selector, timeoutMs);
    try {
      return await this.client.findElement("css selector", `[data-uibox-hit="${token}"]`);
    } finally {
      await this.call("clearMarks", [token]).catch(() => undefined);
    }
  }
}

function buildCapabilities(
  target: string,
  options: OpenOptions,
  plan: DeterminismPlan,
): Record<string, unknown> {
  const application = target.startsWith("exec:") ? target.slice("exec:".length) : target;
  const tauriOptions: Record<string, unknown> = { application };
  if (options.appArgs && options.appArgs.length > 0) tauriOptions.args = options.appArgs;
  const env: Record<string, string> = {
    TZ: plan.timezone,
    LC_ALL: plan.locale.replace("-", "_"),
    LANG: `${plan.locale.replace("-", "_")}.UTF-8`,
    GTK_A11Y: "none",
    ...(options.webdriverEnv ?? {}),
  };
  tauriOptions.env = env;

  const alwaysMatch: Record<string, unknown> = {
    browserName: "wry",
    "tauri:options": tauriOptions,
    ...(options.capabilities ?? {}),
  };
  return { alwaysMatch, firstMatch: [{}] };
}

function spawnTauriDriver(port: number, options: OpenOptions, plan: DeterminismPlan): ChildProcess {
  const bin = options.tauriDriverBin ?? envString("UIBOX_TAURI_DRIVER") ?? "tauri-driver";
  const native = options.nativeDriverBin ?? envString("UIBOX_NATIVE_DRIVER");
  const args = ["--port", String(port)];
  if (native) args.push("--native-driver", native);

  const child = spawn(bin, args, {
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      TZ: plan.timezone,
      ...(options.webdriverEnv ?? {}),
    },
  });
  child.stdout?.on("data", (chunk: Buffer) => process.stderr.write(`[tauri-driver] ${chunk}`));
  child.stderr?.on("data", (chunk: Buffer) => process.stderr.write(`[tauri-driver] ${chunk}`));
  child.on("error", (err) => process.stderr.write(`[tauri-driver] spawn failed: ${err.message}\n`));
  return child;
}

async function waitForDriver(
  client: WebDriverClient,
  child: ChildProcess | null,
  baseUrl: string,
  bootTimeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + bootTimeoutMs;
  for (;;) {
    if (child && child.exitCode !== null) {
      throw new DriverError(
        "attach",
        `webdriver process exited with code ${child.exitCode} before accepting connections`,
        RPC_ATTACH_FAILED,
      );
    }
    try {
      await client.status();
      return;
    } catch (err) {
      if (Date.now() >= deadline) {
        killChild(child);
        throw new DriverError(
          "attach",
          `no webdriver responded at ${baseUrl} within ${bootTimeoutMs}ms: ${(err as Error).message}`,
          RPC_ATTACH_FAILED,
        );
      }
      await delay(POLL_INTERVAL_MS);
    }
  }
}

function killChild(child: ChildProcess | null): void {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
  timer.unref();
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
