export type Surface = "web" | "tauri";

export type SnapMode = "text" | "png" | "both" | "layout";

export interface Viewport {
  width: number;
  height: number;
}

export interface ConsoleEntry {
  ts: string;
  type: "error" | "warning" | "log" | "pageerror";
  text: string;
  location?: string;
  benign?: boolean;
}

export interface NetworkEntry {
  ts: string;
  method: string;
  url: string;
  status?: number;
  failure?: string;
  resourceType?: string;
}

export interface DrainedEvents {
  console: ConsoleEntry[];
  network: NetworkEntry[];
}

export interface ClockConfig {
  mode: "frozen" | "off";
  epochMs: number;
}

export interface OpenOptions {
  runDir?: string;
  artifactsDir?: string;
  snapsDir?: string;
  browser?: "chromium" | "firefox" | "webkit";
  channel?: string;
  headless?: boolean;
  timezone?: string;
  locale?: string;
  clock?: "frozen" | "off" | string | number;
  seed?: number;
  reducedMotion?: boolean;
  disableAnimations?: boolean;
  deviceScaleFactor?: number;
  ignoreHTTPSErrors?: boolean;
  colorScheme?: "light" | "dark" | "no-preference";
  userAgent?: string;
  extraHTTPHeaders?: Record<string, string>;
  storageState?: string;
  webdriverUrl?: string;
  webdriverPort?: number;
  webdriverSessionId?: string;
  webdriverEnv?: Record<string, string>;
  tauriDriverBin?: string;
  nativeDriverBin?: string;
  nativeDriverPort?: number;
  appArgs?: string[];
  capabilities?: Record<string, unknown>;
  driverBootTimeoutMs?: number;
  defaultTimeoutMs?: number;
  readinessTimeoutMs?: number;
  navigationTimeoutMs?: number;
  sessionTtlMs?: number;
  maxSnapWidth?: number;
  maxTreeLines?: number;
  requireReady?: boolean;
  captureConsole?: "errors" | "all";
  clipPadding?: number;
  clipMinSide?: number;
  benignConsole?: string[];
}

export interface OpenParams {
  target: string;
  surface?: Surface;
  viewport?: string | Viewport;
  options?: OpenOptions;
}

export interface TypeStepBody {
  selector: string;
  text: string;
  clear?: boolean;
  delayMs?: number;
}

export interface SnapStepBody {
  name: string;
  mode: SnapMode;
  clip?: string;
}

export interface ClipReport {
  selector: string;
  rect: { x: number; y: number; width: number; height: number };
  padding: number;
  scale: number;
  upscale: number;
  pixel?: string;
}

export interface AssertTextStepBody {
  selector: string;
  text?: string;
}

export type NormalizedStep =
  | { open: string }
  | { click: string }
  | { type: TypeStepBody }
  | { key: string }
  | { wait_for: string }
  | { assert_text: string | AssertTextStepBody }
  | { assert_absent: string }
  | { assert_visible: string }
  | { snap: SnapStepBody };

export type StepKind =
  | "open"
  | "click"
  | "type"
  | "key"
  | "wait_for"
  | "assert_text"
  | "assert_absent"
  | "assert_visible"
  | "snap";

export interface ActParams {
  sessionId: string;
  step: unknown;
}

export interface ActError {
  kind: string;
  message: string;
  selector?: string;
  detail?: string;
  console?: ConsoleEntry[];
}

export interface ActResult {
  ok: boolean;
  error?: ActError;
  step?: NormalizedStep;
  durationMs: number;
  url?: string;
  snap?: SnapResult;
  report?: Record<string, unknown>;
}

export interface SnapParams {
  sessionId: string;
  mode?: SnapMode;
  name?: string;
  clip?: string;
  clipPadding?: number;
  clipMinSide?: number;
  benignConsole?: string[];
}

export interface SnapResult {
  name: string;
  mode: SnapMode;
  text?: string;
  pngPath?: string;
  txtPath?: string;
  console: ConsoleEntry[];
  network: NetworkEntry[];
  url?: string;
  clip?: ClipReport;
}

export interface EvalParams {
  sessionId: string;
  expr: string;
}

export interface CloseParams {
  sessionId: string;
}
