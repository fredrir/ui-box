import type { ReadinessReport, RuntimeSelectorSpec, SnapshotConfig } from "../injected/runtime.js";
import type { ParsedSelector } from "../selector.js";
import type { DrainedEvents, Surface, Viewport } from "../types.js";

export interface FillOptions {
  clear: boolean;
  delayMs: number;
}

export interface Backend {
  readonly surface: Surface;
  readonly viewport: Viewport;
  goto(url: string, timeoutMs: number): Promise<void>;
  click(selector: ParsedSelector, timeoutMs: number): Promise<void>;
  fill(
    selector: ParsedSelector,
    text: string,
    options: FillOptions,
    timeoutMs: number,
  ): Promise<void>;
  press(key: string): Promise<void>;
  waitFor(selector: ParsedSelector, timeoutMs: number): Promise<void>;
  countMatches(selector: ParsedSelector): Promise<number>;
  textOf(selector: ParsedSelector, limit: number): Promise<string[]>;
  evaluate(expr: string): Promise<unknown>;
  snapshotText(config: SnapshotConfig): Promise<string>;
  readiness(): Promise<ReadinessReport>;
  screenshot(): Promise<Buffer>;
  currentUrl(): Promise<string>;
  drain(): Promise<DrainedEvents>;
  dispose(): Promise<void>;
}

export function toRuntimeSpec(selector: ParsedSelector): RuntimeSelectorSpec {
  if (selector.kind === "css") return { kind: "css", value: selector.value };
  if (selector.kind === "text") return { kind: "text", match: { ...selector.match } };
  const options: RuntimeSelectorSpec["options"] = { includeHidden: selector.options.includeHidden };
  if (selector.options.level !== undefined) options.level = selector.options.level;
  if (selector.options.checked !== undefined) options.checked = selector.options.checked;
  if (selector.options.disabled !== undefined) options.disabled = selector.options.disabled;
  if (selector.options.expanded !== undefined) options.expanded = selector.options.expanded;
  if (selector.options.pressed !== undefined) options.pressed = selector.options.pressed;
  if (selector.options.selected !== undefined) options.selected = selector.options.selected;
  const spec: RuntimeSelectorSpec = { kind: "role", role: selector.role, options };
  if (selector.options.name) spec.match = { ...selector.options.name };
  return spec;
}
