import assert from "node:assert/strict";
import { test } from "node:test";
import { PNG } from "pngjs";
import { callExpression, describeExpression } from "../backend/index.js";
import {
  MAX_UPSCALED_SIDE,
  cropPng,
  pngDimensions,
  samplePixel,
  upscaleFactor,
  upscalePng,
} from "../image.js";

function solid(width: number, height: number, rgb: [number, number, number]): PNG {
  const png = new PNG({ width, height });
  for (let i = 0; i < png.data.length; i += 4) {
    png.data[i] = rgb[0];
    png.data[i + 1] = rgb[1];
    png.data[i + 2] = rgb[2];
    png.data[i + 3] = 255;
  }
  return png;
}

function withBlock(
  width: number,
  height: number,
  block: { x: number; y: number; width: number; height: number },
  rgb: [number, number, number],
): Buffer {
  const png = solid(width, height, [10, 20, 30]);
  for (let y = block.y; y < block.y + block.height; y += 1) {
    for (let x = block.x; x < block.x + block.width; x += 1) {
      const index = (width * y + x) << 2;
      png.data[index] = rgb[0];
      png.data[index + 1] = rgb[1];
      png.data[index + 2] = rgb[2];
    }
  }
  return PNG.sync.write(png);
}

test("cropPng returns exactly the requested region", () => {
  const source = withBlock(100, 80, { x: 40, y: 30, width: 4, height: 10 }, [200, 0, 0]);
  const cropped = cropPng(source, { x: 40, y: 30, width: 4, height: 10 });
  assert.deepEqual(pngDimensions(cropped), { width: 4, height: 10 });
  assert.equal(samplePixel(cropped, 0, 0), "#c80000");
  assert.equal(samplePixel(cropped, 3, 9), "#c80000");
  assert.equal(samplePixel(source, 39, 30), "#0a141e");
});

test("a crop that runs past the edge is clamped, never silently shifted", () => {
  const source = withBlock(100, 80, { x: 96, y: 76, width: 4, height: 4 }, [0, 200, 0]);
  const cropped = cropPng(source, { x: 90, y: 70, width: 40, height: 40 });
  assert.deepEqual(pngDimensions(cropped), { width: 10, height: 10 });
  assert.equal(samplePixel(cropped, 9, 9), "#00c800");
});

test("a negative origin is clamped to the image", () => {
  const source = withBlock(100, 80, { x: 0, y: 0, width: 3, height: 3 }, [0, 0, 200]);
  const cropped = cropPng(source, { x: -20, y: -20, width: 10, height: 10 });
  assert.deepEqual(pngDimensions(cropped), { width: 10, height: 10 });
  assert.equal(samplePixel(cropped, 0, 0), "#0000c8");
});

test("upscalePng multiplies both sides and keeps colours exact", () => {
  const source = withBlock(4, 4, { x: 0, y: 0, width: 2, height: 2 }, [255, 128, 0]);
  const scaled = upscalePng(source, 4);
  assert.deepEqual(pngDimensions(scaled), { width: 16, height: 16 });
  assert.equal(samplePixel(scaled, 0, 0), "#ff8000");
  assert.equal(samplePixel(scaled, 7, 7), "#ff8000");
  assert.equal(samplePixel(scaled, 8, 8), "#0a141e");
});

test("upscalePng is a no-op below a factor of two", () => {
  const source = withBlock(4, 4, { x: 0, y: 0, width: 1, height: 1 }, [1, 2, 3]);
  assert.equal(upscalePng(source, 1), source);
  assert.equal(upscalePng(source, 0), source);
});

test("upscaleFactor lifts the smallest side to the requested minimum", () => {
  assert.equal(upscaleFactor(24, 24, 96), 4);
  assert.equal(upscaleFactor(120, 400, 96), 1);
  assert.equal(upscaleFactor(1, 1, 96), 96);
});

test("upscaleFactor never produces a side beyond the output cap", () => {
  assert.equal(upscaleFactor(2, 64, 96), Math.floor(MAX_UPSCALED_SIDE / 64));
  assert.ok(64 * upscaleFactor(2, 64, 96) <= MAX_UPSCALED_SIDE);
  assert.equal(upscaleFactor(4, 1900, 96), 1);
});

test("callExpression calls a function source and only parenthesises anything else", () => {
  assert.equal(callExpression("() => 7"), "(() => 7)()");
  assert.equal(callExpression("(a, b) => a + b"), "((a, b) => a + b)()");
  assert.equal(callExpression("value => value"), "(value => value)()");
  assert.equal(callExpression("function () { return 7; }"), "(function () { return 7; })()");
  assert.equal(callExpression("async () => 7"), "(async () => 7)()");
});

test("callExpression does not re-invoke an expression that already called itself", () => {
  assert.equal(callExpression("(async () => 7)()"), "((async () => 7)())");
  assert.equal(callExpression("(function () { return 7; })()"), "((function () { return 7; })())");
  assert.equal(callExpression("(a + b)"), "((a + b))");
  assert.equal(callExpression("(1 + 2) * 3"), "((1 + 2) * 3)");
  assert.equal(callExpression("document.title"), "(document.title)");
});

test("describeExpression defers evaluation, awaits, then describes", () => {
  const script = describeExpression("fetch('/x')");
  assert.match(
    script,
    /^Promise\.resolve\(\)\.then\(function \(\) \{ return \(fetch\('\/x'\)\); \}\)/,
  );
  assert.match(script, /window\.__uibox\.describeValue\(value\)/);
  assert.match(script, /threw: true/);
});
