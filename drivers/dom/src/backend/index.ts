import type {
  EvalDescriptor,
  ReadinessReport,
  RuntimeSelectorSpec,
  SnapshotConfig,
  VisibilityReport,
} from "../injected/runtime.js";
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
  evalDescribe(expr: string): Promise<EvalDescriptor>;
  describeElement(selector: ParsedSelector): Promise<VisibilityReport>;
  snapshotText(config: SnapshotConfig): Promise<string>;
  readiness(): Promise<ReadinessReport>;
  screenshot(): Promise<Buffer>;
  currentUrl(): Promise<string>;
  drain(): Promise<DrainedEvents>;
  dispose(): Promise<void>;
}

export function callExpression(expr: string): string {
  const trimmed = expr.trim();
  return isCallableSource(trimmed) ? `(${trimmed})()` : `(${trimmed})`;
}

function isCallableSource(source: string): boolean {
  const body = /^async\s/.test(source) ? source.replace(/^async\s+/, "") : source;
  if (/^function\b/.test(body)) return true;
  if (/^[A-Za-z_$][\w$]*\s*=>/.test(body)) return true;
  if (!body.startsWith("(")) return false;
  const close = matchingParen(body);
  return close !== -1 && /^\s*=>/.test(body.slice(close + 1));
}

function matchingParen(source: string): number {
  let depth = 0;
  for (let i = 0; i < source.length; i += 1) {
    if (source[i] === "(") depth += 1;
    else if (source[i] === ")") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

const EVAL_ERROR_DESCRIPTOR =
  'function (err) { return { kind: "error", serializable: false, json: null, threw: true, detail: String((err && err.message) || err).slice(0, 400) }; }';

export function describeExpression(expr: string): string {
  return `Promise.resolve().then(function () { return ${callExpression(expr)}; }).then(function (value) { return window.__uibox.describeValue(value); }, ${EVAL_ERROR_DESCRIPTOR})`;
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
