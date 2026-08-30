import type {
  Browser,
  BrowserContext,
  ConsoleMessage,
  Locator,
  Page,
  Request,
  Response,
} from "playwright-core";
import { chromium, firefox, webkit } from "playwright-core";
import {
  DEFAULT_NAVIGATION_TIMEOUT_MS,
  DEFAULT_TIMEOUT_MS,
  type DeterminismPlan,
  envString,
} from "../config.js";
import { DriverError, RPC_ATTACH_FAILED } from "../errors.js";
import type { ReadinessReport, RuntimeConfig, SnapshotConfig } from "../injected/runtime.js";
import { uiboxRuntime } from "../injected/runtime.js";
import { EventRecorder, nowIso } from "../recorder.js";
import type { ParsedSelector, TextMatch } from "../selector.js";
import type { DrainedEvents, OpenOptions, Surface, Viewport } from "../types.js";
import { type Backend, type FillOptions, toRuntimeSpec } from "./index.js";

const BROWSERS = { chromium, firefox, webkit };

export class PlaywrightBackend implements Backend {
  readonly surface: Surface = "web";
  readonly viewport: Viewport;

  private readonly browser: Browser;
  private readonly context: BrowserContext;
  private readonly page: Page;
  private readonly recorder: EventRecorder;
  private proxyCounter = 0;

  private constructor(
    browser: Browser,
    context: BrowserContext,
    page: Page,
    viewport: Viewport,
    recorder: EventRecorder,
  ) {
    this.browser = browser;
    this.context = context;
    this.page = page;
    this.viewport = viewport;
    this.recorder = recorder;
  }

  static async launch(
    viewport: Viewport,
    options: OpenOptions,
    plan: DeterminismPlan,
  ): Promise<PlaywrightBackend> {
    const name =
      options.browser ?? (envString("UIBOX_BROWSER") as keyof typeof BROWSERS) ?? "chromium";
    const engine = BROWSERS[name];
    if (!engine) {
      throw new DriverError(
        "attach",
        `unknown browser "${name}", expected chromium, firefox or webkit`,
        RPC_ATTACH_FAILED,
      );
    }

    const headless = options.headless ?? envString("UIBOX_HEADLESS") !== "0";
    let browser: Browser;
    try {
      browser = await engine.launch({
        headless,
        channel: options.channel ?? envString("UIBOX_BROWSER_CHANNEL"),
        args: name === "chromium" ? chromiumArgs() : undefined,
      });
    } catch (err) {
      throw new DriverError(
        "attach",
        `failed to launch ${name}: ${(err as Error).message}`,
        RPC_ATTACH_FAILED,
      );
    }

    const context = await browser.newContext({
      viewport,
      deviceScaleFactor: options.deviceScaleFactor ?? 1,
      locale: plan.locale,
      timezoneId: plan.timezone,
      colorScheme: plan.colorScheme,
      reducedMotion: plan.reducedMotion ? "reduce" : "no-preference",
      ignoreHTTPSErrors: options.ignoreHTTPSErrors ?? true,
      userAgent: options.userAgent,
      extraHTTPHeaders: options.extraHTTPHeaders,
      storageState: options.storageState,
    });

    const runtimeConfig: RuntimeConfig = {
      seed: plan.seed,
      fixedTimeMs: plan.fixedTimeMs,
      disableAnimations: plan.disableAnimations,
      probeConsole: false,
      probeNetwork: false,
      captureAll: options.captureConsole === "all",
      maxEvents: 500,
    };
    await context.addInitScript(uiboxRuntime, runtimeConfig);

    context.setDefaultTimeout(options.defaultTimeoutMs ?? DEFAULT_TIMEOUT_MS);
    context.setDefaultNavigationTimeout(
      options.navigationTimeoutMs ?? DEFAULT_NAVIGATION_TIMEOUT_MS,
    );

    const page = await context.newPage();
    const recorder = new EventRecorder();
    attachRecorders(page, recorder, options.captureConsole === "all");

    return new PlaywrightBackend(browser, context, page, viewport, recorder);
  }

  async goto(url: string, timeoutMs: number): Promise<void> {
    const response = await this.page.goto(url, {
      timeout: timeoutMs,
      waitUntil: "domcontentloaded",
    });
    if (response && !response.ok() && response.status() >= 400) {
      this.recorder.network({
        ts: nowIso(),
        method: response.request().method(),
        url: response.url(),
        status: response.status(),
      });
    }
    await this.page.waitForLoadState("load", { timeout: timeoutMs }).catch(() => undefined);
  }

  async click(selector: ParsedSelector, timeoutMs: number): Promise<void> {
    await this.actOnInteractionTarget(selector, timeoutMs, (target) =>
      target.click({ timeout: timeoutMs }),
    );
  }

  async fill(
    selector: ParsedSelector,
    text: string,
    options: FillOptions,
    timeoutMs: number,
  ): Promise<void> {
    const locator = this.locator(selector);
    if (options.clear && options.delayMs <= 0) {
      await locator.fill(text, { timeout: timeoutMs });
      return;
    }
    if (options.clear) await locator.fill("", { timeout: timeoutMs });
    else await locator.focus({ timeout: timeoutMs });
    await locator.pressSequentially(text, { timeout: timeoutMs, delay: options.delayMs });
  }

  async press(key: string): Promise<void> {
    await this.page.keyboard.press(key, { delay: 0 });
  }

  async waitFor(selector: ParsedSelector, timeoutMs: number): Promise<void> {
    await this.actOnInteractionTarget(selector, timeoutMs, (target) =>
      target.first().waitFor({ state: "visible", timeout: timeoutMs }),
    );
  }

  async countMatches(selector: ParsedSelector): Promise<number> {
    return this.locator(selector).count();
  }

  async textOf(selector: ParsedSelector, limit: number): Promise<string[]> {
    const locator = this.locator(selector);
    const total = Math.min(await locator.count(), limit);
    const out: string[] = [];
    for (let i = 0; i < total; i += 1) {
      out.push((await locator.nth(i).innerText()).replace(/\s+/g, " ").trim());
    }
    return out;
  }

  async evaluate(expr: string): Promise<unknown> {
    return this.page.evaluate(expr);
  }

  async snapshotText(config: SnapshotConfig): Promise<string> {
    await this.ensureRuntime();
    return this.page.evaluate((snapConfig) => window.__uibox!.snapshot(snapConfig), config);
  }

  async readiness(): Promise<ReadinessReport> {
    await this.ensureRuntime();
    return this.page.evaluate(() => window.__uibox!.readiness());
  }

  async screenshot(): Promise<Buffer> {
    return this.page.screenshot({
      type: "png",
      animations: "disabled",
      caret: "hide",
      scale: "css",
    });
  }

  async currentUrl(): Promise<string> {
    return this.page.url();
  }

  async drain(): Promise<DrainedEvents> {
    return this.recorder.drain();
  }

  freshPageErrors(): string[] {
    return this.recorder.freshPageErrors();
  }

  async dispose(): Promise<void> {
    await this.context.close().catch(() => undefined);
    await this.browser.close().catch(() => undefined);
  }

  private async actOnInteractionTarget(
    selector: ParsedSelector,
    timeoutMs: number,
    run: (target: Locator) => Promise<void>,
  ): Promise<void> {
    const locator = this.locator(selector);
    const token = this.nextProxyToken();
    const proxied = await locator
      .evaluate((el, mark) => window.__uibox!.labelProxy(el, mark), token, { timeout: timeoutMs })
      .catch(() => false);
    if (!proxied) return run(locator);
    try {
      await run(this.page.locator(`[data-uibox-hit="${token}"]`));
    } finally {
      await this.page
        .evaluate((mark) => window.__uibox!.clearMarks(mark), token)
        .catch(() => undefined);
    }
  }

  private nextProxyToken(): string {
    this.proxyCounter += 1;
    return `uibox-proxy-${this.proxyCounter}`;
  }

  private async ensureRuntime(): Promise<void> {
    const present = await this.page.evaluate(() => Boolean(window.__uibox));
    if (present) return;
    throw new DriverError(
      "runtime",
      "uibox page runtime is missing; the page may have navigated to a restricted origin",
    );
  }

  private locator(selector: ParsedSelector): Locator {
    if (selector.kind === "css") return this.page.locator(selector.value);
    if (selector.kind === "text") {
      return this.page.getByText(toMatcher(selector.match), { exact: selector.match.exact });
    }
    const options: Record<string, unknown> = { includeHidden: selector.options.includeHidden };
    if (selector.options.name) {
      options.name = toMatcher(selector.options.name);
      options.exact = selector.options.name.exact;
    }
    if (selector.options.level !== undefined) options.level = selector.options.level;
    if (selector.options.checked !== undefined) options.checked = selector.options.checked;
    if (selector.options.disabled !== undefined) options.disabled = selector.options.disabled;
    if (selector.options.expanded !== undefined) options.expanded = selector.options.expanded;
    if (selector.options.pressed !== undefined) options.pressed = selector.options.pressed;
    if (selector.options.selected !== undefined) options.selected = selector.options.selected;
    return this.page.getByRole(selector.role as never, options as never);
  }
}

function toMatcher(match: TextMatch): string | RegExp {
  return match.regex ? new RegExp(match.source, match.flags) : match.source;
}

function chromiumArgs(): string[] {
  return [
    "--disable-lcd-text",
    "--font-render-hinting=none",
    "--disable-skia-runtime-opts",
    "--force-color-profile=srgb",
    "--hide-scrollbars",
    "--disable-background-timer-throttling",
    "--disable-features=PaintHolding,BackForwardCache",
  ];
}

function attachRecorders(page: Page, recorder: EventRecorder, captureAll: boolean): void {
  page.on("console", (message: ConsoleMessage) => {
    const type = message.type();
    if (!captureAll && type !== "error" && type !== "warning") return;
    const location = message.location();
    recorder.console({
      ts: nowIso(),
      type: type === "error" ? "error" : type === "warning" ? "warning" : "log",
      text: message.text().slice(0, 4000),
      location: location.url
        ? `${location.url}:${location.lineNumber}:${location.columnNumber}`
        : undefined,
    });
  });

  page.on("pageerror", (error: Error) => {
    recorder.console({
      ts: nowIso(),
      type: "pageerror",
      text: `${error.name}: ${error.message}`.slice(0, 4000),
    });
  });

  page.on("requestfailed", (request: Request) => {
    recorder.network({
      ts: nowIso(),
      method: request.method(),
      url: request.url(),
      failure: request.failure()?.errorText ?? "request failed",
      resourceType: request.resourceType(),
    });
  });

  page.on("response", (response: Response) => {
    if (response.status() < 400) return;
    recorder.network({
      ts: nowIso(),
      method: response.request().method(),
      url: response.url(),
      status: response.status(),
      resourceType: response.request().resourceType(),
    });
  });

  page.on("crash", () => {
    recorder.console({ ts: nowIso(), type: "pageerror", text: "page crashed" });
  });
}
