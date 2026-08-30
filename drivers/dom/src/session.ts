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
import { cropPng, pngDimensions, samplePixel, upscaleFactor, upscalePng } from "./image.js";
import type { EvalDescriptor, ReadinessReport, VisibilityReport } from "./injected/runtime.js";
import { parseSelector } from "./selector.js";
import { SnapWriter } from "./snapshot.js";
import { type StepPlan, normalizeStep } from "./steps.js";
import { navigableTarget } from "./target.js";
import type {
  ActResult,
  ClipReport,
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

interface StepOutcome {
  snap?: SnapResult;
  report?: Record<string, unknown>;
}

interface ClipRequest {
  selector?: string;
  padding?: number;
  minSide?: number;
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
  private readonly benignConsole: RegExp[];

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
    this.benignConsole = compilePatterns(options.benignConsole);
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
      const outcome = await this.execute(plan);
      const uncaught = this.freshPageErrors().filter((text) => !this.isBenign(text));
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
      if (outcome.snap) result.snap = outcome.snap;
      if (outcome.report) result.report = outcome.report;
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

  async snap(mode: SnapMode = "text", name?: string, clip: ClipRequest = {}): Promise<SnapResult> {
    this.touch();
    this.assertReady();
    return this.captureSnap(mode, name, clip);
  }

  async evaluate(expr: string): Promise<EvalDescriptor> {
    this.touch();
    if (typeof expr !== "string" || expr.trim().length === 0) {
      throw new DriverError("params", "eval requires a non-empty expr string", RPC_INVALID_PARAMS);
    }
    this.assertReady();
    return this.backend.evalDescribe(expr);
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

  private async execute(plan: StepPlan): Promise<StepOutcome> {
    const timeout = plan.timeoutMs ?? this.options.defaultTimeoutMs ?? DEFAULT_TIMEOUT_MS;

    if ("open" in plan.step) {
      await this.backend.goto(
        navigableTarget(plan.step.open),
        plan.timeoutMs ?? this.options.navigationTimeoutMs ?? DEFAULT_NAVIGATION_TIMEOUT_MS,
      );
      this.ready = false;
      await this.gateOnReadiness();
      return {};
    }
    if ("click" in plan.step) {
      await this.backend.click(parseSelector(plan.step.click), timeout);
      return {};
    }
    if ("type" in plan.step) {
      const body = plan.step.type;
      await this.backend.fill(
        parseSelector(body.selector),
        body.text,
        { clear: body.clear !== false, delayMs: body.delayMs ?? 0 },
        timeout,
      );
      return {};
    }
    if ("key" in plan.step) {
      await this.backend.press(plan.step.key);
      return {};
    }
    if ("wait_for" in plan.step) {
      await this.backend.waitFor(parseSelector(plan.step.wait_for), timeout);
      return {};
    }
    if ("assert_text" in plan.step) {
      await this.assertText(plan);
      return {};
    }
    if ("assert_absent" in plan.step) {
      await this.assertAbsent(plan.step.assert_absent, plan.timeoutMs);
      return {};
    }
    if ("assert_visible" in plan.step) {
      return { report: await this.assertVisible(plan.step.assert_visible, plan.timeoutMs) };
    }
    const body = plan.step.snap;
    return { snap: await this.captureSnap(body.mode, body.name, { selector: body.clip }) };
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

  private async assertVisible(
    raw: string,
    timeoutMs: number | null,
  ): Promise<Record<string, unknown>> {
    const selector = parseSelector(raw);
    const timeout = timeoutMs ?? DEFAULT_ASSERT_TIMEOUT_MS;
    const deadline = Date.now() + Math.max(timeout, 0);
    let report = await this.backend.describeElement(selector);
    while (!report.visible && Date.now() < deadline) {
      await delay(READINESS_POLL_MS);
      report = await this.backend.describeElement(selector);
    }
    const pixel = await this.centrePixel(report);
    const evidence: Record<string, unknown> = {
      visible: report.visible,
      matches: report.matches,
      rect: report.rect,
      hitTest: report.hitTest,
      styles: report.styles,
      ...(pixel ? { pixel } : {}),
    };
    if (report.visible) return evidence;
    throw new DriverError(
      "assertion",
      `"${raw}" is not visible: ${report.reasons.join("; ")}`,
      undefined,
      { detail: JSON.stringify(evidence) },
    );
  }

  private async centrePixel(report: VisibilityReport): Promise<string | null> {
    const rect = report.rect;
    if (!rect || rect.width < 1 || rect.height < 1) return null;
    try {
      const shot = await this.backend.screenshot();
      const dims = pngDimensions(shot);
      const scale = report.viewport.width > 0 ? dims.width / report.viewport.width : 1;
      const x = Math.round((rect.x + rect.width / 2) * scale);
      const y = Math.round((rect.y + rect.height / 2) * scale);
      if (x < 0 || y < 0 || x >= dims.width || y >= dims.height) return null;
      return samplePixel(shot, x, y);
    } catch {
      return null;
    }
  }

  private async clipShot(shot: Buffer, request: ClipRequest): Promise<[Buffer, ClipReport]> {
    const raw = request.selector!;
    const report = await this.backend.describeElement(parseSelector(raw));
    if (report.matches === 0) {
      throw new DriverError(
        "selector",
        `clip selector "${raw}" matched no element; nothing was cropped`,
        RPC_INVALID_PARAMS,
      );
    }
    if (report.matches > 1) {
      throw new DriverError(
        "strictness",
        `clip selector "${raw}" matched ${report.matches} elements; refine it so the crop is unambiguous`,
        RPC_INVALID_PARAMS,
      );
    }
    const rect = report.rect;
    if (!rect || rect.width < 1 || rect.height < 1) {
      throw new DriverError(
        "selector",
        `clip selector "${raw}" resolved to a ${rect?.width ?? 0}x${rect?.height ?? 0} box; there is nothing to crop`,
        RPC_INVALID_PARAMS,
      );
    }

    const dims = pngDimensions(shot);
    const scale = report.viewport.width > 0 ? dims.width / report.viewport.width : 1;
    const padding = Math.max(0, Math.round(request.padding ?? this.options.clipPadding ?? 8));
    const minSide = Math.max(1, Math.round(request.minSide ?? this.options.clipMinSide ?? 96));
    const box = {
      x: Math.round((rect.x - padding) * scale),
      y: Math.round((rect.y - padding) * scale),
      width: Math.round((rect.width + padding * 2) * scale),
      height: Math.round((rect.height + padding * 2) * scale),
    };
    const cropped = cropPng(shot, box);
    const croppedDims = pngDimensions(cropped);
    const upscale = upscaleFactor(croppedDims.width, croppedDims.height, minSide);
    const pixel = samplePixel(
      shot,
      Math.round((rect.x + rect.width / 2) * scale),
      Math.round((rect.y + rect.height / 2) * scale),
    );
    return [
      upscalePng(cropped, upscale),
      { selector: raw, rect: { ...rect }, padding, scale, upscale, pixel },
    ];
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

  private async captureSnap(
    mode: SnapMode,
    requestedName?: string,
    clip: ClipRequest = {},
  ): Promise<SnapResult> {
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
    let png = wantsPng ? await this.backend.screenshot() : undefined;
    let clipReport: ClipReport | undefined;
    if (png && clip.selector) {
      const [cropped, report] = await this.clipShot(png, clip);
      png = cropped;
      clipReport = report;
    }

    const written = await this.snaps.write(name, text, png);
    const drained = await this.drainQuietly();

    const result: SnapResult = {
      name,
      mode,
      console: drained.console,
      network: drained.network,
      url: await this.backend.currentUrl().catch(() => undefined),
    };
    if (clipReport) result.clip = clipReport;
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

  private isBenign(text: string): boolean {
    return this.benignConsole.some((pattern) => pattern.test(text));
  }

  private markBenign(entries: ConsoleEntry[]): ConsoleEntry[] {
    if (this.benignConsole.length === 0) return entries;
    return entries.map((entry) => (this.isBenign(entry.text) ? { ...entry, benign: true } : entry));
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
      return { console: this.markBenign(consoleEntries), network: networkEntries };
    } catch {
      const consoleEntries = this.pendingConsole;
      const networkEntries = this.pendingNetwork;
      this.pendingConsole = [];
      this.pendingNetwork = [];
      return { console: this.markBenign(consoleEntries), network: networkEntries };
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

function compilePatterns(sources: string[] | undefined): RegExp[] {
  if (!Array.isArray(sources)) return [];
  const out: RegExp[] = [];
  for (const source of sources) {
    if (typeof source !== "string" || source.length === 0) continue;
    try {
      out.push(new RegExp(source));
    } catch (err) {
      throw new DriverError(
        "params",
        `invalid benignConsole pattern ${JSON.stringify(source)}: ${(err as Error).message}`,
        RPC_INVALID_PARAMS,
      );
    }
  }
  return out;
}

function errorDetail(err: unknown): string | null {
  if (!(err instanceof DriverError) || !err.data) return null;
  const detail = err.data.detail;
  return typeof detail === "string" && detail.length > 0 ? detail : null;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
