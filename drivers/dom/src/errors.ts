export const RPC_PARSE_ERROR = -32700;
export const RPC_INVALID_REQUEST = -32600;
export const RPC_METHOD_NOT_FOUND = -32601;
export const RPC_INVALID_PARAMS = -32602;
export const RPC_INTERNAL_ERROR = -32603;

export const RPC_SESSION_NOT_FOUND = -32001;
export const RPC_NOT_READY = -32002;
export const RPC_SELECTOR_INVALID = -32003;
export const RPC_ATTACH_FAILED = -32004;

export class DriverError extends Error {
  readonly code: number;
  readonly kind: string;
  readonly data: Record<string, unknown> | undefined;

  constructor(
    kind: string,
    message: string,
    code = RPC_INTERNAL_ERROR,
    data?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "DriverError";
    this.kind = kind;
    this.code = code;
    this.data = data;
  }
}

export class SelectorError extends DriverError {
  constructor(message: string) {
    super("selector", message, RPC_SELECTOR_INVALID);
    this.name = "SelectorError";
  }
}

export function describeError(err: unknown): string {
  if (err instanceof Error) {
    const first = err.message.split("\n").slice(0, 6).join("\n").trim();
    return first.length > 0 ? first : err.name;
  }
  return String(err);
}

export function errorKind(err: unknown): string {
  if (err instanceof DriverError) return err.kind;
  if (err instanceof Error && /Timeout .* exceeded/i.test(err.message)) return "timeout";
  if (err instanceof Error && err.name === "TimeoutError") return "timeout";
  return "internal";
}
