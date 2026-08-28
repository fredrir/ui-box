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
