import type { ConsoleEntry, DrainedEvents, NetworkEntry } from "./types.js";

const MAX_BUFFERED = 1000;

export class EventRecorder {
  private consoleEntries: ConsoleEntry[] = [];
  private networkEntries: NetworkEntry[] = [];
  private pageErrors: string[] = [];
  private consumedPageErrors = 0;

  console(entry: ConsoleEntry): void {
    if (entry.type === "pageerror") this.pageErrors.push(entry.text);
    if (this.consoleEntries.length >= MAX_BUFFERED) this.consoleEntries.shift();
    this.consoleEntries.push(entry);
  }

  network(entry: NetworkEntry): void {
    if (this.networkEntries.length >= MAX_BUFFERED) this.networkEntries.shift();
    this.networkEntries.push(entry);
  }

  drain(): DrainedEvents {
    const drained: DrainedEvents = { console: this.consoleEntries, network: this.networkEntries };
    this.consoleEntries = [];
    this.networkEntries = [];
    return drained;
  }

  freshPageErrors(): string[] {
    const fresh = this.pageErrors.slice(this.consumedPageErrors);
    this.consumedPageErrors = this.pageErrors.length;
    return fresh;
  }
}

export function nowIso(): string {
  return new Date().toISOString();
}

export function isoFrom(ms: number): string {
  const value = new Date(ms);
  return Number.isFinite(value.getTime()) ? value.toISOString() : nowIso();
}
