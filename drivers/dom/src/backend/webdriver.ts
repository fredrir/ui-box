import { type ChildProcess, spawn } from "node:child_process";
import { constants, accessSync, statSync } from "node:fs";
import { hostname } from "node:os";
import { delimiter, join } from "node:path";
import { DEFAULT_TIMEOUT_MS, type DeterminismPlan, envNumber, envString } from "../config.js";
import { DriverError, RPC_ATTACH_FAILED } from "../errors.js";
import type {
  EvalDescriptor,
  ReadinessReport,
  RuntimeConfig,
  RuntimeDrain,
  SnapshotConfig,
  VisibilityReport,
} from "../injected/runtime.js";
import { uiboxRuntime } from "../injected/runtime.js";
import { isoFrom } from "../recorder.js";
import type { ParsedSelector } from "../selector.js";
import type { DrainedEvents, OpenOptions, Surface, Viewport } from "../types.js";
import { WebDriverClient } from "../webdriver/client.js";
import { keyActions, parseChord } from "../webdriver/keys.js";
import { type Backend, type FillOptions, callExpression, toRuntimeSpec } from "./index.js";

const RUNTIME_SOURCE = uiboxRuntime.toString();
const DRIVER_BOOT_TIMEOUT_MS = 20_000;
const POLL_INTERVAL_MS = 100;
const STDERR_TAIL_LIMIT = 2000;
const EXIT_DRAIN_MS = 250;
const DEFAULT_TAURI_DRIVER = "tauri-driver";
const DRIVER_LOCALITY = "the driver runs where the display is, not where ui-box was invoked";

export type TauriBinSource = "option" | "env" | "default" | "unset";

export interface TauriBins {
  tauriDriver: string;
  nativeDriver: string | null;
  source: { tauriDriver: TauriBinSource; nativeDriver: TauriBinSource };
}

export interface TauriBinProbe {
  tauriDriver: string;
  nativeDriver: string | null;
  reason: string | null;
}

interface BinKind {
  label: string;
  envName: string;
  optionName: string;
}

interface ResolvedBin {
  value: string | null;
  source: TauriBinSource;
}

interface SpawnFault {
  error: Error | null;
  stderrTail: string;
}

interface SpawnedDriver {
  child: ChildProcess;
  fault: SpawnFault;
}

const TAURI_DRIVER_KIND: BinKind = {
  label: "tauri-driver",
  envName: "UIBOX_TAURI_DRIVER",
  optionName: "options.tauriDriverBin",
};

const NATIVE_DRIVER_KIND: BinKind = {
  label: "native webdriver",
  envName: "UIBOX_NATIVE_DRIVER",
  optionName: "options.nativeDriverBin",
};

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

    let spawned: SpawnedDriver | null = null;
    let baseUrl = options.webdriverUrl ?? envString("UIBOX_WEBDRIVER_URL") ?? null;

    if (!baseUrl) {
      const port = options.webdriverPort ?? envNumber("UIBOX_WEBDRIVER_PORT") ?? 4444;
      baseUrl = `http://127.0.0.1:${port}`;
      spawned = spawnTauriDriver(port, options, plan);
    }
    const child = spawned?.child ?? null;

    const client = new WebDriverClient(baseUrl);
    await waitForDriver(
      client,
      spawned,
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
    return this.client.executeScript(`return ${callExpression(expr)};`, []);
  }

  async evalDescribe(expr: string): Promise<EvalDescriptor> {
    await this.ensureRuntime();
    return (await this.client.executeScript(
      `return window.__uibox["describeValue"](${callExpression(expr)});`,
      [],
    )) as EvalDescriptor;
  }

  async describeElement(selector: ParsedSelector): Promise<VisibilityReport> {
    await this.ensureRuntime();
    return (await this.call("describeElement", [toRuntimeSpec(selector)])) as VisibilityReport;
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

function resolveBin(
  option: string | undefined,
  envName: string,
  fallback: string | null,
): ResolvedBin {
  if (option) return { value: option, source: "option" };
  const fromEnv = envString(envName);
  if (fromEnv) return { value: fromEnv, source: "env" };
  if (fallback === null) return { value: null, source: "unset" };
  return { value: fallback, source: "default" };
}

export function resolveTauriBins(options: OpenOptions): TauriBins {
  const tauri = resolveBin(options.tauriDriverBin, TAURI_DRIVER_KIND.envName, DEFAULT_TAURI_DRIVER);
  const native = resolveBin(options.nativeDriverBin, NATIVE_DRIVER_KIND.envName, null);
  return {
    tauriDriver: tauri.value ?? DEFAULT_TAURI_DRIVER,
    nativeDriver: native.value,
    source: { tauriDriver: tauri.source, nativeDriver: native.source },
  };
}

export function findExecutable(bin: string): string | null {
  if (bin.includes("/")) return isExecutableFile(bin) ? bin : null;
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (dir.length === 0) continue;
    const candidate = join(dir, bin);
    if (isExecutableFile(candidate)) return candidate;
  }
  return null;
}

function isExecutableFile(path: string): boolean {
  try {
    if (!statSync(path).isFile()) return false;
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function probeTauriBins(bins: TauriBins): TauriBinProbe {
  const tauriPath = findExecutable(bins.tauriDriver);
  const nativePath = bins.nativeDriver === null ? null : findExecutable(bins.nativeDriver);
  const reason =
    tauriPath === null
      ? unresolvable(TAURI_DRIVER_KIND, bins.tauriDriver, bins.source.tauriDriver)
      : bins.nativeDriver !== null && nativePath === null
        ? unresolvable(NATIVE_DRIVER_KIND, bins.nativeDriver, bins.source.nativeDriver)
        : null;
  return {
    tauriDriver: tauriPath ?? bins.tauriDriver,
    nativeDriver: nativePath ?? bins.nativeDriver,
    reason,
  };
}

function unresolvable(kind: BinKind, bin: string, source: TauriBinSource): string {
  const problem = bin.includes("/") ? "is not an executable file" : "is not on PATH";
  return `${kind.label} "${bin}" (${binOrigin(kind, source)}) ${problem} on ${hostLabel()}; ${DRIVER_LOCALITY}`;
}

function binOrigin(kind: BinKind, source: TauriBinSource): string {
  if (source === "option") return `from ${kind.optionName}`;
  if (source === "env") return `from ${kind.envName}`;
  return `the default, no ${kind.envName} or ${kind.optionName} set`;
}

function hostLabel(): string {
  return `the driver host ${hostname()}`;
}

function spawnTauriDriver(
  port: number,
  options: OpenOptions,
  plan: DeterminismPlan,
): SpawnedDriver {
  const bins = resolveTauriBins(options);
  const probe = probeTauriBins(bins);
  if (probe.reason !== null) {
    throw new DriverError("attach", probe.reason, RPC_ATTACH_FAILED);
  }

  const args = ["--port", String(port)];
  if (probe.nativeDriver) args.push("--native-driver", probe.nativeDriver);
  if (Number.isFinite(options.nativeDriverPort)) {
    args.push("--native-port", String(options.nativeDriverPort));
  }

  const fault: SpawnFault = { error: null, stderrTail: "" };
  const child = spawn(probe.tauriDriver, args, {
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      TZ: plan.timezone,
      ...(options.webdriverEnv ?? {}),
    },
  });
  child.stdout?.on("data", (chunk: Buffer) => relay(fault, chunk));
  child.stderr?.on("data", (chunk: Buffer) => relay(fault, chunk));
  child.on("error", (err) => {
    fault.error = err;
    process.stderr.write(`[tauri-driver] spawn failed: ${err.message}\n`);
  });
  return { child, fault };
}

function relay(fault: SpawnFault, chunk: Buffer): void {
  process.stderr.write(`[tauri-driver] ${chunk}`);
  fault.stderrTail = `${fault.stderrTail}${chunk.toString()}`.slice(-STDERR_TAIL_LIMIT);
}

function saidOnStderr(fault: SpawnFault | null): string {
  const text = fault?.stderrTail.trim() ?? "";
  return text.length > 0 ? `; tauri-driver said: ${text}` : "";
}

function settle(child: ChildProcess): Promise<void> {
  const stream = child.stderr;
  if (!stream || stream.readableEnded) return Promise.resolve();
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(), EXIT_DRAIN_MS);
    timer.unref();
    stream.once("end", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function waitForDriver(
  client: WebDriverClient,
  spawned: SpawnedDriver | null,
  baseUrl: string,
  bootTimeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + bootTimeoutMs;
  const child = spawned?.child ?? null;
  const fault = spawned?.fault ?? null;
  for (;;) {
    if (fault?.error) {
      throw new DriverError(
        "attach",
        `tauri-driver could not be started: ${fault.error.message}${saidOnStderr(fault)}`,
        RPC_ATTACH_FAILED,
      );
    }
    if (child && child.exitCode !== null) {
      await settle(child);
      throw new DriverError(
        "attach",
        `webdriver process exited with code ${child.exitCode} before accepting connections${saidOnStderr(fault)}`,
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
          `no webdriver responded at ${baseUrl} within ${bootTimeoutMs}ms: ${(err as Error).message}${saidOnStderr(fault)}`,
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
