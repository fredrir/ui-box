import { DriverError, RPC_INVALID_PARAMS } from "./errors.js";
import type { OpenOptions, Surface, Viewport } from "./types.js";

export const DEFAULT_VIEWPORT: Viewport = { width: 1280, height: 800 };
export const DEFAULT_EPOCH_MS = Date.UTC(2024, 0, 1, 0, 0, 0);
export const DEFAULT_TIMEZONE = "UTC";
export const DEFAULT_LOCALE = "en-US";
export const DEFAULT_SEED = 0x5eed1234;
export const MAX_SNAP_WIDTH = 1024;
export const DEFAULT_TIMEOUT_MS = 15_000;
export const DEFAULT_ASSERT_TIMEOUT_MS = 2_000;
export const DEFAULT_NAVIGATION_TIMEOUT_MS = 30_000;
export const DEFAULT_READINESS_TIMEOUT_MS = 15_000;
export const DEFAULT_SESSION_TTL_MS = 900_000;
export const DEFAULT_MAX_TREE_LINES = 1200;
export const DEFAULT_MAX_TEXT_LENGTH = 160;

export interface DeterminismPlan {
  timezone: string;
  locale: string;
  colorScheme: "light" | "dark" | "no-preference";
  reducedMotion: boolean;
  disableAnimations: boolean;
  seed: number;
  fixedTimeMs: number | null;
}

export function parseViewport(input: unknown): Viewport {
  if (input === undefined || input === null) return DEFAULT_VIEWPORT;
  if (typeof input === "object") {
    const candidate = input as Partial<Viewport>;
    const width = Number(candidate.width);
    const height = Number(candidate.height);
    if (Number.isFinite(width) && Number.isFinite(height) && width > 0 && height > 0) {
      return { width: Math.round(width), height: Math.round(height) };
    }
    throw new DriverError(
      "params",
      `invalid viewport object: ${JSON.stringify(input)}`,
      RPC_INVALID_PARAMS,
    );
  }
  if (typeof input === "string") {
    const match = /^\s*(\d+)\s*[x×]\s*(\d+)(?:\s*[x×]\s*\d+)?\s*$/.exec(input);
    if (!match) {
      throw new DriverError(
        "params",
        `invalid viewport "${input}", expected WIDTHxHEIGHT`,
        RPC_INVALID_PARAMS,
      );
    }
    return { width: Number.parseInt(match[1]!, 10), height: Number.parseInt(match[2]!, 10) };
  }
  throw new DriverError("params", `invalid viewport: ${JSON.stringify(input)}`, RPC_INVALID_PARAMS);
}

export function planDeterminism(options: OpenOptions): DeterminismPlan {
  return {
    timezone: options.timezone ?? envString("UIBOX_TIMEZONE") ?? DEFAULT_TIMEZONE,
    locale: options.locale ?? envString("UIBOX_LOCALE") ?? DEFAULT_LOCALE,
    colorScheme: options.colorScheme ?? "light",
    reducedMotion: options.reducedMotion !== false,
    disableAnimations: options.disableAnimations !== false,
    seed: Number.isFinite(options.seed) ? Number(options.seed) : DEFAULT_SEED,
    fixedTimeMs: resolveClock(options.clock),
  };
}

function resolveClock(clock: OpenOptions["clock"]): number | null {
  if (clock === undefined || clock === null || clock === "frozen") return DEFAULT_EPOCH_MS;
  if (clock === "off") return null;
  if (typeof clock === "number") return Number.isFinite(clock) ? clock : DEFAULT_EPOCH_MS;
  const parsed = Date.parse(clock);
  if (Number.isFinite(parsed)) return parsed;
  throw new DriverError(
    "params",
    `invalid clock option "${clock}", expected "frozen", "off" or an ISO timestamp`,
    RPC_INVALID_PARAMS,
  );
}

export function resolveSurface(target: string, requested: unknown): Surface {
  if (requested === "web" || requested === "tauri") return requested;
  if (requested !== undefined && requested !== null) {
    throw new DriverError(
      "params",
      `surface "${String(requested)}" is not served by the dom driver (web, tauri)`,
      RPC_INVALID_PARAMS,
    );
  }
  return target.startsWith("exec:") ? "tauri" : "web";
}

export function envString(name: string): string | undefined {
  const value = process.env[name];
  return value && value.length > 0 ? value : undefined;
}

export function envNumber(name: string): number | undefined {
  const raw = envString(name);
  if (raw === undefined) return undefined;
  const value = Number(raw);
  return Number.isFinite(value) ? value : undefined;
}

export function sessionTtlMs(options: OpenOptions): number {
  if (Number.isFinite(options.sessionTtlMs)) return Number(options.sessionTtlMs);
  const seconds = envNumber("UIBOX_SESSION_TTL");
  if (seconds !== undefined) return seconds * 1000;
  return DEFAULT_SESSION_TTL_MS;
}

export function snapsDir(options: OpenOptions): string | null {
  if (options.snapsDir) return options.snapsDir;
  const runDir = options.runDir ?? envString("UIBOX_RUN_DIR");
  if (runDir) return `${runDir.replace(/\/+$/, "")}/snaps`;
  const artifacts = options.artifactsDir ?? envString("UIBOX_ARTIFACTS");
  if (artifacts) return `${artifacts.replace(/\/+$/, "")}/snaps`;
  return null;
}
