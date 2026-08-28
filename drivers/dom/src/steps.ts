import { DriverError, RPC_INVALID_PARAMS } from "./errors.js";
import type { NormalizedStep, SnapMode, StepKind } from "./types.js";

export interface StepPlan {
  kind: StepKind;
  step: NormalizedStep;
  timeoutMs: number | null;
  selector: string | null;
}

const KEY_ALIASES: Record<string, StepKind> = {
  open: "open",
  goto: "open",
  navigate: "open",
  click: "click",
  type: "type",
  fill: "type",
  key: "key",
  press: "key",
  wait_for: "wait_for",
  waitFor: "wait_for",
  assert_text: "assert_text",
  assertText: "assert_text",
  snap: "snap",
  screenshot: "snap",
};

export function normalizeStep(input: unknown): StepPlan {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new DriverError(
      "step",
      `step must be a single-key object such as { click: "role=button[name=Submit]" }, got ${JSON.stringify(input)}`,
      RPC_INVALID_PARAMS,
    );
  }

  const record = input as Record<string, unknown>;
  const timeoutMs = readTimeout(record);

  if (typeof record.action === "string") {
    const kind = KEY_ALIASES[record.action];
    if (!kind) throw unknownStep(record.action);
    return build(kind, actionBody(kind, record), timeoutMs);
  }

  const keys = Object.keys(record).filter((key) => !isMetaKey(key));
  if (keys.length !== 1) {
    throw new DriverError(
      "step",
      `step must have exactly one action key, got [${keys.join(", ")}]`,
      RPC_INVALID_PARAMS,
    );
  }
  const key = keys[0]!;
  const kind = KEY_ALIASES[key];
  if (!kind) throw unknownStep(key);
  return build(kind, record[key], timeoutMs);
}

function isMetaKey(key: string): boolean {
  return (
    key === "timeout_ms" ||
    key === "timeoutMs" ||
    key === "timeout" ||
    key === "name" ||
    key === "action"
  );
}

function readTimeout(record: Record<string, unknown>): number | null {
  const raw = record.timeout_ms ?? record.timeoutMs ?? record.timeout;
  if (raw === undefined || raw === null) return null;
  const value = Number(raw);
  if (!Number.isFinite(value) || value < 0) {
    throw new DriverError(
      "step",
      `invalid step timeout ${JSON.stringify(raw)}`,
      RPC_INVALID_PARAMS,
    );
  }
  return value;
}

function actionBody(kind: StepKind, record: Record<string, unknown>): unknown {
  switch (kind) {
    case "open":
      return record.url ?? record.target ?? record.value;
    case "click":
    case "wait_for":
      return record.selector ?? record.value;
    case "type":
      return {
        selector: record.selector,
        text: record.text,
        clear: record.clear,
        delayMs: record.delayMs,
      };
    case "key":
      return record.key ?? record.value;
    case "assert_text":
      return record.text !== undefined && record.selector !== undefined
        ? { selector: record.selector, text: record.text }
        : (record.selector ?? record.value ?? record.text);
    case "snap":
      return { name: record.name, mode: record.mode };
    default:
      return record.value;
  }
}

function build(kind: StepKind, body: unknown, timeoutMs: number | null): StepPlan {
  switch (kind) {
    case "open": {
      const url = asString(body, "open");
      return { kind, step: { open: url }, timeoutMs, selector: null };
    }
    case "click": {
      const selector = asSelectorString(body, "click");
      return { kind, step: { click: selector }, timeoutMs, selector };
    }
    case "wait_for": {
      const selector = asSelectorString(body, "wait_for");
      return { kind, step: { wait_for: selector }, timeoutMs, selector };
    }
    case "key": {
      const key = asString(body, "key");
      return { kind, step: { key }, timeoutMs, selector: null };
    }
    case "type": {
      if (body === null || typeof body !== "object") {
        throw new DriverError("step", "type expects { selector, text }", RPC_INVALID_PARAMS);
      }
      const record = body as Record<string, unknown>;
      const selector = asSelectorString(record.selector, "type.selector");
      if (typeof record.text !== "string") {
        throw new DriverError("step", "type.text must be a string", RPC_INVALID_PARAMS);
      }
      const step: NormalizedStep = { type: { selector, text: record.text } };
      if (record.clear === false) step.type.clear = false;
      if (Number.isFinite(record.delayMs)) step.type.delayMs = Number(record.delayMs);
      return { kind, step, timeoutMs, selector };
    }
    case "assert_text": {
      if (typeof body === "string") {
        const selector = asSelectorString(body, "assert_text");
        return { kind, step: { assert_text: selector }, timeoutMs, selector };
      }
      if (body === null || typeof body !== "object") {
        throw new DriverError(
          "step",
          "assert_text expects a selector or { selector, text }",
          RPC_INVALID_PARAMS,
        );
      }
      const record = body as Record<string, unknown>;
      const selector = asSelectorString(record.selector, "assert_text.selector");
      const step: NormalizedStep =
        typeof record.text === "string"
          ? { assert_text: { selector, text: record.text } }
          : { assert_text: selector };
      return { kind, step, timeoutMs, selector };
    }
    case "snap": {
      const record = (body === null || typeof body !== "object" ? {} : body) as Record<
        string,
        unknown
      >;
      const name =
        typeof body === "string" ? body : typeof record.name === "string" ? record.name : "";
      const mode = normalizeMode(record.mode);
      return { kind, step: { snap: { name, mode } }, timeoutMs, selector: null };
    }
    default:
      throw unknownStep(kind);
  }
}

function normalizeMode(raw: unknown): SnapMode {
  if (raw === undefined || raw === null) return "text";
  if (raw === "text" || raw === "png" || raw === "both") return raw;
  throw new DriverError(
    "step",
    `invalid snap mode ${JSON.stringify(raw)}, expected text, png or both`,
    RPC_INVALID_PARAMS,
  );
}

function asString(body: unknown, label: string): string {
  if (typeof body !== "string" || body.trim().length === 0) {
    throw new DriverError(
      "step",
      `${label} expects a non-empty string, got ${JSON.stringify(body)}`,
      RPC_INVALID_PARAMS,
    );
  }
  return body.trim();
}

function asSelectorString(body: unknown, label: string): string {
  return asString(body, label);
}

function unknownStep(key: string): DriverError {
  return new DriverError(
    "step",
    `unknown step "${key}"; supported: open, click, type, key, wait_for, assert_text, snap`,
    RPC_INVALID_PARAMS,
  );
}
