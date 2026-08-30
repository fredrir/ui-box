import { PNG } from "pngjs";

export function capPngWidth(buffer: Buffer, maxWidth: number): Buffer {
  if (maxWidth <= 0) return buffer;
  const source = PNG.sync.read(buffer);
  if (source.width <= maxWidth) return buffer;

  const targetWidth = maxWidth;
  const targetHeight = Math.max(1, Math.round((source.height * maxWidth) / source.width));
  const target = new PNG({ width: targetWidth, height: targetHeight });

  const xRatio = source.width / targetWidth;
  const yRatio = source.height / targetHeight;

  for (let y = 0; y < targetHeight; y += 1) {
    const yStart = Math.floor(y * yRatio);
    const yEnd = Math.min(source.height, Math.max(yStart + 1, Math.ceil((y + 1) * yRatio)));
    for (let x = 0; x < targetWidth; x += 1) {
      const xStart = Math.floor(x * xRatio);
      const xEnd = Math.min(source.width, Math.max(xStart + 1, Math.ceil((x + 1) * xRatio)));

      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      let samples = 0;
      for (let sy = yStart; sy < yEnd; sy += 1) {
        for (let sx = xStart; sx < xEnd; sx += 1) {
          const index = (source.width * sy + sx) << 2;
          r += source.data[index]!;
          g += source.data[index + 1]!;
          b += source.data[index + 2]!;
          a += source.data[index + 3]!;
          samples += 1;
        }
      }

      const out = (targetWidth * y + x) << 2;
      target.data[out] = Math.round(r / samples);
      target.data[out + 1] = Math.round(g / samples);
      target.data[out + 2] = Math.round(b / samples);
      target.data[out + 3] = Math.round(a / samples);
    }
  }

  return PNG.sync.write(target, { deflateLevel: 9, filterType: 0 });
}

export function pngDimensions(buffer: Buffer): { width: number; height: number } {
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function cropPng(buffer: Buffer, rect: PixelRect): Buffer {
  const source = PNG.sync.read(buffer);
  const x = clampInt(rect.x, 0, source.width - 1);
  const y = clampInt(rect.y, 0, source.height - 1);
  const width = clampInt(rect.width, 1, source.width - x);
  const height = clampInt(rect.height, 1, source.height - y);
  const target = new PNG({ width, height });
  PNG.bitblt(source, target, x, y, width, height, 0, 0);
  return PNG.sync.write(target, { deflateLevel: 9, filterType: 0 });
}

export function upscalePng(buffer: Buffer, factor: number): Buffer {
  const scale = Math.floor(factor);
  if (scale <= 1) return buffer;
  const source = PNG.sync.read(buffer);
  const target = new PNG({ width: source.width * scale, height: source.height * scale });
  for (let y = 0; y < target.height; y += 1) {
    const sy = Math.floor(y / scale);
    for (let x = 0; x < target.width; x += 1) {
      const sx = Math.floor(x / scale);
      const from = (source.width * sy + sx) << 2;
      const to = (target.width * y + x) << 2;
      target.data[to] = source.data[from]!;
      target.data[to + 1] = source.data[from + 1]!;
      target.data[to + 2] = source.data[from + 2]!;
      target.data[to + 3] = source.data[from + 3]!;
    }
  }
  return PNG.sync.write(target, { deflateLevel: 9, filterType: 0 });
}

export const MAX_UPSCALED_SIDE = 2048;

export function upscaleFactor(width: number, height: number, minSide: number): number {
  const smallest = Math.max(1, Math.min(width, height));
  if (smallest >= minSide) return 1;
  const wanted = Math.ceil(minSide / smallest);
  const largest = Math.max(1, Math.max(width, height));
  const allowed = Math.max(1, Math.floor(MAX_UPSCALED_SIDE / largest));
  return Math.max(1, Math.min(wanted, allowed));
}

export function samplePixel(buffer: Buffer, x: number, y: number): string {
  const png = PNG.sync.read(buffer);
  const px = clampInt(x, 0, png.width - 1);
  const py = clampInt(y, 0, png.height - 1);
  const index = (png.width * py + px) << 2;
  return `#${hex(png.data[index]!)}${hex(png.data[index + 1]!)}${hex(png.data[index + 2]!)}`;
}

function hex(value: number): string {
  return value.toString(16).padStart(2, "0");
}

function clampInt(value: number, min: number, max: number): number {
  const rounded = Math.round(Number.isFinite(value) ? value : min);
  return Math.min(Math.max(rounded, min), Math.max(min, max));
}
