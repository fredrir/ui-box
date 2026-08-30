import type { Backend } from "./backend/index.js";
import { PlaywrightBackend } from "./backend/playwright.js";
import { WebDriverBackend } from "./backend/webdriver.js";
import {
  DEFAULT_ASSERT_TIMEOUT_MS,
  DEFAULT_MAX_TEXT_LENGTH,
  DEFAULT_MAX_TREE_LINES,
  DEFAULT_NAVIGATION_TIMEOUT_MS,
  DEFAULT_READINESS_TIMEOUT_MS,
  DEFAULT_TIMEOUT_MS,
  MAX_SNAP_WIDTH,
  parseViewport,
  planDeterminism,
  resolveSurface,
  sessionTtlMs,
  snapsDir,
} from "./config.js";
import {
  DriverError,
  RPC_INVALID_PARAMS,
  RPC_NOT_READY,
  describeError,
  errorKind,
} from "./errors.js";
import type { ReadinessReport } from "./injected/runtime.js";
import { parseSelector } from "./selector.js";
import { SnapWriter } from "./snapshot.js";
import { type StepPlan, normalizeStep } from "./steps.js";
import { navigableTarget } from "./target.js";
import type {
  ActResult,
  ConsoleEntry,
  NetworkEntry,
  NormalizedStep,
  OpenOptions,
  OpenParams,
  SnapMode,
  SnapResult,
  Surface,
  Viewport,
} from "./types.js";

const READINESS_POLL_MS = 100;
const ASSERT_DETAIL_LIMIT = 400;

interface PageErrorSource {
  freshPageErrors(): string[];
}

export class Session {
  readonly id: string;
  readonly surface: Surface;
  readonly target: string;
  readonly viewport: Viewport;

  private readonly backend: Backend;
  private readonly options: OpenOptions;
  private readonly snaps: SnapWriter;
  private readonly ttlMs: number;
  private readonly onExpire: (session: Session) => void;
  private expiry: NodeJS.Timeout | null = null;
  private pendingConsole: ConsoleEntry[] = [];
  private pendingNetwork: NetworkEntry[] = [];
  private ready = false;
  private closed = false;

  private constructor(
    id: string,
    backend: Backend,
    target: string,
    options: OpenOptions,
    onExpire: (session: Session) => void,
  ) {
    this.id = id;
    this.backend = backend;
    this.surface = backend.surface;
    this.target = target;
    this.viewport = backend.viewport;
    this.options = options;
    this.snaps = new SnapWriter(
      snapsDir(options),
      Math.min(options.maxSnapWidth ?? MAX_SNAP_WIDTH, MAX_SNAP_WIDTH),
    );
    this.ttlMs = sessionTtlMs(options);
    this.onExpire = onExpire;
    this.touch();
  }

  static async open(
    id: string,
    params: OpenParams,
    onExpire: (session: Session) => void,
  ): Promise<Session> {
    if (typeof params.target !== "string" || params.target.trim().length === 0) {
      throw new DriverError("params", "open requires a non-empty target", RPC_INVALID_PARAMS);
    }
    const target = params.target.trim();
    const options = params.options ?? {};
    const surface = resolveSurface(target, params.surface);
    const viewport = parseViewport(params.viewport);
    const plan = planDeterminism(options);

    const backend: Backend =
      surface === "tauri"
        ? await WebDriverBackend.attach(target, viewport, options, plan)
        : await PlaywrightBackend.launch(viewport, options, plan);

    const session = new Session(id, backend, target, options, onExpire);
    try {
      if (surface === "web" || /^https?:/.test(target)) {
        await backend.goto(
          navigableTarget(target),
          options.navigationTimeoutMs ?? DEFAULT_NAVIGATION_TIMEOUT_MS,
        );
      }
      await session.gateOnReadiness();
    } catch (err) {
      const drained = await session.drainQuietly();
      await backend.dispose().catch(() => undefined);
      session.closed = true;
      session.clearExpiry();
      const inner = err instanceof DriverError ? (err.data ?? {}) : {};
      throw new DriverError(
        errorKind(err) === "timeout" ? "not_ready" : errorKind(err),
        describeError(err),
        RPC_NOT_READY,
        {
          ...inner,
          sessionId: id,
          target,
          surface,
          console: drained.console,
          network: drained.network,
        },
      );
    }
    return session;
  }

  openStep(): NormalizedStep {
    return { open: this.target };
  }

  async act(rawStep: unknown): Promise<ActResult> {
    this.touch();
    const started = Date.now();
    let plan: StepPlan | null = null;
    try {
      plan = normalizeStep(rawStep);
      this.assertReady();
      const snap = await this.execute(plan);
      const uncaught = this.freshPageErrors();
      if (uncaught.length > 0) {
        return this.failure(started, plan, {
          kind: "pageerror",
          message: `uncaught exception during step: ${uncaught[0]}`,
          detail: uncaught.slice(0, 5).join("\n"),
        });
      }
      const result: ActResult = {
        ok: true,
        step: plan.step,
        durationMs: Date.now() - started,
        url: await this.backend.currentUrl().catch(() => undefined),
      };
      if (snap) result.snap = snap;
      return result;
    } catch (err) {
      const detail = errorDetail(err);
      return this.failure(started, plan, {
        kind: errorKind(err),
        message: describeError(err),
        ...(plan?.selector ? { selector: plan.selector } : {}),
        ...(detail ? { detail } : {}),
      });
    }
  }

  async snap(mode: SnapMode = "text", name?: string): Promise<SnapResult> {
    this.touch();
    this.assertReady();
    return this.captureSnap(mode, name);
  }

  async evaluate(expr: string): Promise<unknown> {
    this.touch();
    if (typeof expr !== "string" || expr.trim().length === 0) {
      throw new DriverError("params", "eval requires a non-empty expr string", RPC_INVALID_PARAMS);
    }
    this.assertReady();
    return this.backend.evaluate(expr);
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.clearExpiry();
    await this.backend.dispose().catch(() => undefined);
  }

  isClosed(): boolean {
    return this.closed;
  }

  private assertReady(): void {
    if (this.closed) throw new DriverError("session", `session ${this.id} is closed`);
    if (!this.ready)
      throw new DriverError(
        "not_ready",
        `session ${this.id} never reached a rendered page`,
        RPC_NOT_READY,
      );
  }

  private async execute(plan: StepPlan): Promise<SnapResult | undefined> {
    const timeout = plan.timeoutMs ?? this.options.defaultTimeoutMs ?? DEFAULT_TIMEOUT_MS;

    if ("open" in plan.step) {
      await this.backend.goto(
        navigableTarget(plan.step.open),
        plan.timeoutMs ?? this.options.navigationTimeoutMs ?? DEFAULT_NAVIGATION_TIMEOUT_MS,
      );
      this.ready = false;
      await this.gateOnReadiness();
      return undefined;
    }
    if ("click" in plan.step) {
      await this.backend.click(parseSelector(plan.step.click), timeout);
      return undefined;
    }
    if ("type" in plan.step) {
      const body = plan.step.type;
      await this.backend.fill(
        parseSelector(body.selector),
        body.text,
        { clear: body.clear !== false, delayMs: body.delayMs ?? 0 },
        timeout,
      );
      return undefined;
    }
    if ("key" in plan.step) {
      await this.backend.press(plan.step.key);
      return undefined;
    }
    if ("wait_for" in plan.step) {
      await this.backend.waitFor(parseSelector(plan.step.wait_for), timeout);
      return undefined;
    }
    if ("assert_text" in plan.step) {
      await this.assertText(plan);
      return undefined;
    }
    if ("assert_absent" in plan.step) {
      await this.assertAbsent(plan.step.assert_absent, plan.timeoutMs);
      return undefined;
    }
    const body = plan.step.snap;
    return this.captureSnap(body.mode, body.name);
  }

  private async assertText(plan: StepPlan): Promise<void> {
    const body = (plan.step as { assert_text: string | { selector: string; text?: string } })
      .assert_text;
    const raw = typeof body === "string" ? body : body.selector;
    const expected = typeof body === "string" ? undefined : body.text;
    const selector = parseSelector(raw);
    const timeout = plan.timeoutMs ?? DEFAULT_ASSERT_TIMEOUT_MS;

    try {
      await this.backend.waitFor(selector, timeout);
    } catch {
      throw await this.assertionFailure(`no visible element matched "${raw}"`);
    }

    if (expected === undefined) return;
    const texts = await this.backend.textOf(selector, 5);
    const needle = expected.replace(/\s+/g, " ").trim();
    if (texts.some((value) => value.includes(needle))) return;
    throw await this.assertionFailure(
      `"${raw}" did not contain ${JSON.stringify(needle)}; found ${JSON.stringify(texts.slice(0, 3))}`,
    );
  }

  private async assertAbsent(raw: string, timeoutMs: number | null): Promise<void> {
    const selector = parseSelector(raw);
    const timeout = timeoutMs ?? DEFAULT_ASSERT_TIMEOUT_MS;
    const deadline = Date.now() + Math.max(timeout, 0);
    let count = await this.backend.countMatches(selector);
    while (count > 0 && Date.now() < deadline) {
      await delay(READINESS_POLL_MS);
      count = await this.backend.countMatches(selector);
    }
    if (count > 0) {
      throw await this.assertionFailure(
        `"${raw}" is still present after ${timeout}ms; ${count} element(s) matched`,
      );
    }
    const report = await this.backend.readiness();
    if (!report.ready) {
      throw new DriverError(
        "nothing_verified",
        `"${raw}" matched nothing, but the page is not rendered (${report.reason}), so its absence proves nothing`,
        undefined,
        { detail: `readyState=${report.readyState} textLength=${report.textLength}` },
      );
    }
  }

  private async assertionFailure(message: string): Promise<DriverError> {
    const detail = await this.pageTextExcerpt();
    return new DriverError("assertion", message, undefined, detail ? { detail } : undefined);
  }

  private async pageTextExcerpt(): Promise<string> {
    try {
      const text = await this.backend.evaluate("document.body ? document.body.innerText : ''");
      return String(text ?? "")
        .replace(/\s+/g, " ")
        .trim()
        .slice(0, ASSERT_DETAIL_LIMIT);
    } catch {
      return "";
    }
  }

  private async captureSnap(mode: SnapMode, requestedName?: string): Promise<SnapResult> {
    const wantsText = mode === "text" || mode === "both" || mode === "layout";
    const wantsPng = mode === "png" || mode === "both";
    if (wantsPng && !this.snaps.enabled) {
      throw new DriverError(
        "params",
        `snap mode "${mode}" writes a png but no run directory is configured; pass options.runDir on open or set UIBOX_RUN_DIR`,
        RPC_INVALID_PARAMS,
      );
    }
    const name = this.snaps.nextName(requestedName);

    const text = wantsText
      ? await this.backend.snapshotText({
          maxLines: this.options.maxTreeLines ?? DEFAULT_MAX_TREE_LINES,
          maxTextLength: DEFAULT_MAX_TEXT_LENGTH,
          includeHidden: false,
          layout: mode === "layout",
        })
      : undefined;
    const png = wantsPng ? await this.backend.screenshot() : undefined;

    const written = await this.snaps.write(name, text, png);
    const drained = await this.drainQuietly();

    const result: SnapResult = {
      name,
      mode,
      console: drained.console,
      network: drained.network,
      url: await this.backend.currentUrl().catch(() => undefined),
    };
    if (text !== undefined) result.text = text;
    if (written.txtPath) result.txtPath = written.txtPath;
    if (written.pngPath) result.pngPath = written.pngPath;
    return result;
  }

  private async gateOnReadiness(): Promise<ReadinessReport> {
    const timeout = this.options.readinessTimeoutMs ?? DEFAULT_READINESS_TIMEOUT_MS;
    const deadline = Date.now() + timeout;
    let report: ReadinessReport | null = null;
    for (;;) {
      report = await this.backend.readiness().catch((err) => {
        if (Date.now() >= deadline) throw err;
        return null;
      });
      if (report?.ready) {
        this.ready = true;
        this.freshPageErrors();
        return report;
      }
      if (Date.now() >= deadline) break;
      await delay(READINESS_POLL_MS);
    }
    const reason = report?.reason ?? "readiness could not be evaluated";
    throw new DriverError(
      "not_ready",
      `page did not render within ${timeout}ms: ${reason}`,
      RPC_NOT_READY,
      { readiness: report },
    );
  }

  private freshPageErrors(): string[] {
    const source = this.backend as unknown as Partial<PageErrorSource>;
    if (typeof source.freshPageErrors !== "function") return [];
    return source.freshPageErrors();
  }

  private failure(started: number, plan: StepPlan | null, error: ActResult["error"]): ActResult {
    const result: ActResult = { ok: false, durationMs: Date.now() - started };
    if (error) result.error = error;
    if (plan) result.step = plan.step;
    return result;
  }

  private async drainQuietly(): Promise<{ console: ConsoleEntry[]; network: NetworkEntry[] }> {
    try {
      const drained = await this.backend.drain();
      const consoleEntries = this.pendingConsole.concat(drained.console);
      const networkEntries = this.pendingNetwork.concat(drained.network);
      this.pendingConsole = [];
      this.pendingNetwork = [];
      return { console: consoleEntries, network: networkEntries };
    } catch {
      const consoleEntries = this.pendingConsole;
      const networkEntries = this.pendingNetwork;
      this.pendingConsole = [];
      this.pendingNetwork = [];
      return { console: consoleEntries, network: networkEntries };
    }
  }

  private touch(): void {
    this.clearExpiry();
    if (this.ttlMs <= 0) return;
    this.expiry = setTimeout(() => this.onExpire(this), this.ttlMs);
    this.expiry.unref();
  }

  private clearExpiry(): void {
    if (this.expiry) {
      clearTimeout(this.expiry);
      this.expiry = null;
    }
  }
}

function errorDetail(err: unknown): string | null {
  if (!(err instanceof DriverError) || !err.data) return null;
  const detail = err.data.detail;
  return typeof detail === "string" && detail.length > 0 ? detail : null;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
