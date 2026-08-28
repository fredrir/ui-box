import { mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { capPngWidth } from "./image.js";

export interface WrittenSnap {
  pngPath?: string;
  txtPath?: string;
}

export class SnapWriter {
  private readonly dir: string | null;
  private readonly maxWidth: number;
  private readonly used = new Set<string>();
  private counter = 0;

  constructor(dir: string | null, maxWidth: number) {
    this.dir = dir ? resolve(dir) : null;
    this.maxWidth = maxWidth;
  }

  get enabled(): boolean {
    return this.dir !== null;
  }

  nextName(explicit?: string): string {
    const base = sanitize(explicit ?? "");
    if (base.length > 0) {
      if (!this.used.has(base)) return base;
      let suffix = 2;
      while (this.used.has(`${base}-${suffix}`)) suffix += 1;
      return `${base}-${suffix}`;
    }
    this.counter += 1;
    return `snap-${String(this.counter).padStart(3, "0")}`;
  }

  async write(
    name: string,
    text: string | undefined,
    png: Buffer | undefined,
  ): Promise<WrittenSnap> {
    this.used.add(name);
    if (!this.dir) return {};
    await mkdir(this.dir, { recursive: true });
    const written: WrittenSnap = {};
    if (text !== undefined) {
      const txtPath = join(this.dir, `${name}.txt`);
      await writeFile(txtPath, text.endsWith("\n") ? text : `${text}\n`, "utf8");
      written.txtPath = txtPath;
    }
    if (png !== undefined) {
      const pngPath = join(this.dir, `${name}.png`);
      await writeFile(pngPath, capPngWidth(png, this.maxWidth));
      written.pngPath = pngPath;
    }
    return written;
  }
}

function sanitize(name: string): string {
  return name
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 120);
}
