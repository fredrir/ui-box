import type { Readable, Writable } from "node:stream";
import {
  RPC_INTERNAL_ERROR,
  RPC_INVALID_REQUEST,
  RPC_METHOD_NOT_FOUND,
  RPC_PARSE_ERROR,
} from "./errors.js";

export interface RpcRequest {
  jsonrpc?: string;
  id?: string | number | null;
  method?: string;
  params?: unknown;
}

export interface RpcErrorShape {
  code: number;
  message: string;
  data?: unknown;
}

export type RpcHandler = (params: unknown) => Promise<unknown>;

const MAX_LINE_BYTES = 64 * 1024 * 1024;

export class RpcServer {
  private readonly handlers = new Map<string, RpcHandler>();
  private readonly input: Readable;
  private readonly output: Writable;
  private buffer = "";
  private inflight = 0;
  private closed = false;
  private onIdleDrained: (() => void) | undefined;

  constructor(input: Readable, output: Writable) {
    this.input = input;
    this.output = output;
  }

  method(name: string, handler: RpcHandler): void {
    this.handlers.set(name, handler);
  }

  start(): Promise<void> {
    return new Promise((resolve) => {
      this.input.setEncoding("utf8");
      this.input.on("data", (chunk: string) => this.feed(chunk));
      this.input.on("end", () => {
        this.closed = true;
        if (this.inflight === 0) resolve();
        else this.onIdleDrained = resolve;
      });
      this.input.on("close", () => {
        this.closed = true;
        if (this.inflight === 0) resolve();
        else this.onIdleDrained = resolve;
      });
    });
  }

  private feed(chunk: string): void {
    this.buffer += chunk;
    if (this.buffer.length > MAX_LINE_BYTES) {
      this.buffer = "";
      this.emit({
        jsonrpc: "2.0",
        id: null,
        error: { code: RPC_PARSE_ERROR, message: "input line exceeded limit" },
      });
      return;
    }
    let index = this.buffer.indexOf("\n");
    while (index !== -1) {
      const line = this.buffer.slice(0, index);
      this.buffer = this.buffer.slice(index + 1);
      const trimmed = line.trim();
      if (trimmed.length > 0) void this.dispatch(trimmed);
      index = this.buffer.indexOf("\n");
    }
  }

  private async dispatch(line: string): Promise<void> {
    let request: RpcRequest;
    try {
      const parsed: unknown = JSON.parse(line);
      if (Array.isArray(parsed)) {
        this.emit({
          jsonrpc: "2.0",
          id: null,
          error: { code: RPC_INVALID_REQUEST, message: "batch requests are not supported" },
        });
        return;
      }
      if (parsed === null || typeof parsed !== "object") {
        this.emit({
          jsonrpc: "2.0",
          id: null,
          error: { code: RPC_INVALID_REQUEST, message: "request must be an object" },
        });
        return;
      }
      request = parsed as RpcRequest;
    } catch (err) {
      this.emit({
        jsonrpc: "2.0",
        id: null,
        error: { code: RPC_PARSE_ERROR, message: `invalid JSON: ${(err as Error).message}` },
      });
      return;
    }

    const id = request.id === undefined ? null : request.id;
    const isNotification = request.id === undefined;

    if (typeof request.method !== "string") {
      if (!isNotification) {
        this.emit({
          jsonrpc: "2.0",
          id,
          error: { code: RPC_INVALID_REQUEST, message: "missing method" },
        });
      }
      return;
    }

    const handler = this.handlers.get(request.method);
    if (!handler) {
      if (!isNotification) {
        this.emit({
          jsonrpc: "2.0",
          id,
          error: { code: RPC_METHOD_NOT_FOUND, message: `unknown method: ${request.method}` },
        });
      }
      return;
    }

    this.inflight += 1;
    try {
      const result = await handler(request.params ?? {});
      if (!isNotification) this.emit({ jsonrpc: "2.0", id, result: result ?? {} });
    } catch (err) {
      if (!isNotification) this.emit({ jsonrpc: "2.0", id, error: toRpcError(err) });
    } finally {
      this.inflight -= 1;
      if (this.closed && this.inflight === 0 && this.onIdleDrained) {
        const done = this.onIdleDrained;
        this.onIdleDrained = undefined;
        done();
      }
    }
  }

  private emit(message: Record<string, unknown>): void {
    this.output.write(`${JSON.stringify(message)}\n`);
  }
}

function toRpcError(err: unknown): RpcErrorShape {
  const candidate = err as { code?: unknown; message?: unknown; data?: unknown };
  const code = typeof candidate?.code === "number" ? candidate.code : RPC_INTERNAL_ERROR;
  const message = err instanceof Error ? err.message : String(err);
  const shape: RpcErrorShape = { code, message };
  if (candidate?.data !== undefined) shape.data = candidate.data;
  return shape;
}
