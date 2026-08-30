import { DriverError, RPC_ATTACH_FAILED } from "../errors.js";

export const W3C_ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

interface W3cEnvelope<T> {
  value: T;
}

interface W3cErrorValue {
  error?: string;
  message?: string;
  stacktrace?: string;
}

export class WebDriverError extends DriverError {
  readonly webdriverError: string;

  constructor(webdriverError: string, message: string) {
    super("webdriver", message);
    this.name = "WebDriverError";
    this.webdriverError = webdriverError;
  }
}

export class WebDriverClient {
  private readonly baseUrl: string;
  private readonly requestTimeoutMs: number;
  sessionId: string | null = null;

  constructor(baseUrl: string, requestTimeoutMs = 120_000) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.requestTimeoutMs = requestTimeoutMs;
  }

  async status(): Promise<unknown> {
    return this.send("GET", "/status", undefined, false);
  }

  async newSession(capabilities: Record<string, unknown>): Promise<string> {
    const envelope = (await this.send("POST", "/session", { capabilities }, false, true)) as {
      sessionId?: string;
      value?: { sessionId?: string };
    };
    const id = envelope?.value?.sessionId ?? envelope?.sessionId;
    if (!id)
      throw new DriverError("attach", "webdriver did not return a sessionId", RPC_ATTACH_FAILED);
    this.sessionId = id;
    return id;
  }

  adoptSession(sessionId: string): void {
    this.sessionId = sessionId;
  }

  async deleteSession(): Promise<void> {
    if (!this.sessionId) return;
    await this.send("DELETE", "", undefined, true).catch(() => undefined);
    this.sessionId = null;
  }

  async setTimeouts(timeouts: {
    implicit?: number;
    pageLoad?: number;
    script?: number;
  }): Promise<void> {
    await this.send("POST", "/timeouts", timeouts, true);
  }

  async setWindowRect(rect: {
    width: number;
    height: number;
    x?: number;
    y?: number;
  }): Promise<void> {
    await this.send("POST", "/window/rect", rect, true);
  }

  async navigateTo(url: string): Promise<void> {
    await this.send("POST", "/url", { url }, true);
  }

  async getCurrentUrl(): Promise<string> {
    return (await this.send("GET", "/url", undefined, true)) as string;
  }

  async findElement(using: string, value: string): Promise<string> {
    const result = (await this.send("POST", "/element", { using, value }, true)) as Record<
      string,
      string
    >;
    const id = result?.[W3C_ELEMENT_KEY];
    if (!id) throw new WebDriverError("no such element", `no element for ${using}=${value}`);
    return id;
  }

  async elementClick(elementId: string): Promise<void> {
    await this.send("POST", `/element/${elementId}/click`, {}, true);
  }

  async elementClear(elementId: string): Promise<void> {
    await this.send("POST", `/element/${elementId}/clear`, {}, true);
  }

  async elementSendKeys(elementId: string, text: string): Promise<void> {
    await this.send("POST", `/element/${elementId}/value`, { text, value: Array.from(text) }, true);
  }

  async executeScript(script: string, args: unknown[] = []): Promise<unknown> {
    return this.send("POST", "/execute/sync", { script, args }, true);
  }

  async takeScreenshot(): Promise<Buffer> {
    const base64 = (await this.send("GET", "/screenshot", undefined, true)) as string;
    return Buffer.from(base64, "base64");
  }

  async performActions(actions: unknown[]): Promise<void> {
    await this.send("POST", "/actions", { actions }, true);
  }

  async releaseActions(): Promise<void> {
    await this.send("DELETE", "/actions", undefined, true).catch(() => undefined);
  }

  private async send(
    method: string,
    path: string,
    body: unknown,
    scoped: boolean,
    envelope = false,
  ): Promise<unknown> {
    if (scoped && !this.sessionId) {
      throw new DriverError("webdriver", "no active webdriver session");
    }
    const url = scoped
      ? `${this.baseUrl}/session/${this.sessionId}${path}`
      : `${this.baseUrl}${path}`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.requestTimeoutMs);
    let response: globalThis.Response;
    try {
      response = await fetch(url, {
        method,
        signal: controller.signal,
        headers: body === undefined ? {} : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (err) {
      throw new DriverError("webdriver", `${method} ${url} failed: ${(err as Error).message}`);
    } finally {
      clearTimeout(timer);
    }

    const raw = await response.text();
    let parsed: W3cEnvelope<unknown> | undefined;
    if (raw.length > 0) {
      try {
        parsed = JSON.parse(raw) as W3cEnvelope<unknown>;
      } catch {
        parsed = undefined;
      }
    }

    if (!response.ok) {
      const value = (parsed?.value ?? {}) as W3cErrorValue;
      const kind = value.error ?? `http ${response.status}`;
      const message = value.message ?? raw.slice(0, 500) ?? response.statusText;
      throw new WebDriverError(kind, `${kind}: ${message}`);
    }

    if (envelope) return parsed ?? null;
    return parsed?.value ?? null;
  }
}
