import { DriverError, RPC_INVALID_PARAMS } from "./errors.js";

const HOST_PORT = /^[a-zA-Z0-9._-]+:\d+(?:[/?#]|$)/;
const SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;

export function navigableTarget(target: string): string {
  if (target.startsWith("exec:")) {
    throw new DriverError(
      "params",
      `cannot navigate to "${target}"; exec: targets are launched, not navigated`,
      RPC_INVALID_PARAMS,
    );
  }
  if (target.startsWith("tui:")) {
    throw new DriverError(
      "params",
      `target "${target}" belongs to the tui driver`,
      RPC_INVALID_PARAMS,
    );
  }
  if (HOST_PORT.test(target)) return `http://${target}`;
  if (SCHEME.test(target)) return target;
  return `http://${target}`;
}
