#!/usr/bin/env node
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { probeTauriBins, resolveTauriBins } from "./backend/webdriver.js";
import { envString } from "./config.js";
import { DriverError, RPC_INVALID_PARAMS, RPC_SESSION_NOT_FOUND } from "./errors.js";
import { RpcServer } from "./rpc.js";
import { Session } from "./session.js";
import type {
  ActParams,
  CloseParams,
  EvalParams,
  OpenParams,
  SnapMode,
  SnapParams,
} from "./types.js";

const DRIVER_NAME = "dom";
const SURFACES = ["web", "tauri"] as const;
const NATIVE_DRIVER_DELEGATED =
  "no native driver override, which is the expected case: tauri-driver resolves WebKitWebDriver itself";
const NO_OVERRIDE_SCOPE =
  "resolved on the driver host from UIBOX_TAURI_DRIVER, UIBOX_NATIVE_DRIVER and PATH, with no per-session overrides";

function redirectConsoleToStderr(): void {
  const write =
    (prefix: string) =>
    (...args: unknown[]) => {
      const text = args
        .map((arg) => (typeof arg === "string" ? arg : safeStringify(arg)))
        .join(" ");
      process.stderr.write(`${prefix}${text}\n`);
    };
  console.log = write("");
  console.info = write("");
  console.debug = write("");
  console.warn = write("[warn] ");
  console.error = write("[error] ");
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function driverVersion(): string {
  try {
    const raw = readFileSync(new URL("../package.json", import.meta.url), "utf8");
    return (JSON.parse(raw) as { version?: string }).version ?? "0.0.0";
  } catch {
    return "0.0.0";
  }
}

class SessionRegistry {
  private readonly sessions = new Map<string, Session>();

  newId(): string {
    return `dom-${randomBytes(6).toString("hex")}`;
  }

  put(session: Session): void {
    this.sessions.set(session.id, session);
  }

  get(sessionId: unknown): Session {
    if (typeof sessionId !== "string" || sessionId.length === 0) {
      throw new DriverError("params", "sessionId is required", RPC_INVALID_PARAMS);
    }
    const session = this.sessions.get(sessionId);
    if (!session || session.isClosed()) {
      throw new DriverError("session", `unknown session ${sessionId}`, RPC_SESSION_NOT_FOUND);
    }
    return session;
  }

  async drop(sessionId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    this.sessions.delete(sessionId);
    if (session) await session.close();
  }

  async closeAll(): Promise<void> {
    const all = Array.from(this.sessions.values());
    this.sessions.clear();
    await Promise.all(all.map((session) => session.close()));
  }
}

async function main(): Promise<void> {
  redirectConsoleToStderr();

  const registry = new SessionRegistry();
  const server = new RpcServer(process.stdin, process.stdout);
  const version = driverVersion();

  server.method("driver.info", async () => ({
    name: DRIVER_NAME,
    version,
    surfaces: SURFACES,
    selectors: ["css", "role", "text"],
    modes: ["text", "png", "both", "layout"],
    tauri: tauriCapability(),
  }));

  server.method("driver.open", async (params) => openSession(registry, params as OpenParams));

  server.method("driver.act", async (params) => actOnSession(registry, params as ActParams));

  server.method("driver.snap", async (params) => snapSession(registry, params as SnapParams));

  server.method("driver.eval", async (params) => evalSession(registry, params as EvalParams));

  server.method("driver.close", async (params) => closeSession(registry, params as CloseParams));

  const shutdown = async (code: number): Promise<never> => {
    await registry.closeAll();
    process.exit(code);
  };
  process.on("SIGINT", () => void shutdown(130));
  process.on("SIGTERM", () => void shutdown(143));

  await server.start();
  await registry.closeAll();
}

function displayFault(): string | null {
  if (process.platform !== "linux") return null;
  if (envString("DISPLAY") || envString("WAYLAND_DISPLAY")) return null;
  return "neither DISPLAY nor WAYLAND_DISPLAY is set, and WebKitWebDriver cannot run without one";
}

function tauriCapability(): Record<string, unknown> {
  const bins = resolveTauriBins({});
  const probe = probeTauriBins(bins);
  const faults = [probe.reason, displayFault()].filter((fault) => fault !== null);
  const notes = bins.source.nativeDriver === "unset" ? [NATIVE_DRIVER_DELEGATED] : [];
  return {
    ok: faults.length === 0,
    tauriDriver: probe.tauriDriver,
    nativeDriver: probe.nativeDriver,
    source: bins.source,
    reason: [...faults, ...notes, NO_OVERRIDE_SCOPE].join("; "),
  };
}

async function openSession(registry: SessionRegistry, params: OpenParams): Promise<unknown> {
  const id = registry.newId();
  const session = await Session.open(id, params, (expired) => {
    void registry.drop(expired.id);
  });
  registry.put(session);
  return {
    sessionId: session.id,
    surface: session.surface,
    target: session.target,
    viewport: session.viewport,
    ready: true,
    step: session.openStep(),
  };
}

async function actOnSession(registry: SessionRegistry, params: ActParams): Promise<unknown> {
  const session = registry.get(params?.sessionId);
  return session.act(params?.step);
}

async function snapSession(registry: SessionRegistry, params: SnapParams): Promise<unknown> {
  const session = registry.get(params?.sessionId);
  const mode = (params?.mode ?? "text") as SnapMode;
  if (mode !== "text" && mode !== "png" && mode !== "both" && mode !== "layout") {
    throw new DriverError("params", `invalid snap mode "${String(mode)}"`, RPC_INVALID_PARAMS);
  }
  return session.snap(mode, params?.name, {
    selector: typeof params?.clip === "string" ? params.clip : undefined,
    padding: params?.clipPadding,
    minSide: params?.clipMinSide,
  });
}

async function evalSession(registry: SessionRegistry, params: EvalParams): Promise<unknown> {
  const session = registry.get(params?.sessionId);
  const described = await session.evaluate(params?.expr);
  return {
    value: described.serializable && described.json !== null ? JSON.parse(described.json) : null,
    kind: described.kind,
    serializable: described.serializable,
    ...(described.detail ? { detail: described.detail } : {}),
  };
}

async function closeSession(registry: SessionRegistry, params: CloseParams): Promise<unknown> {
  if (typeof params?.sessionId !== "string") {
    throw new DriverError("params", "close requires sessionId", RPC_INVALID_PARAMS);
  }
  await registry.drop(params.sessionId);
  return {};
}

main().catch((err) => {
  process.stderr.write(`uibox-driver-dom fatal: ${(err as Error).stack ?? String(err)}\n`);
  process.exit(1);
});
