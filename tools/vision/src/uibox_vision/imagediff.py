from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

import numpy as np
from PIL import Image, ImageDraw, UnidentifiedImageError

from .emit import VisionError
from .masks import Rect, mask_array

MAX_COLOR_DELTA = 35215.0
DIFF_COLOR = (255, 32, 88)
MASK_COLOR = (120, 150, 210)
MISSING_COLOR = (255, 168, 32)
BASE_FADE = 0.22


@dataclass(frozen=True)
class Comparison:
    differs: bool
    pixels: int
    ratio: float
    width: int
    height: int
    size_mismatch: bool
    golden_size: tuple[int, int]
    candidate_size: tuple[int, int]


def load_rgb(path: Path) -> np.ndarray:
    try:
        with Image.open(path) as image:
            image.load()
            if image.mode in ("RGBA", "LA", "PA") or "transparency" in image.info:
                rgba = image.convert("RGBA")
                canvas = Image.new("RGBA", rgba.size, (255, 255, 255, 255))
                flat = Image.alpha_composite(canvas, rgba).convert("RGB")
            else:
                flat = image.convert("RGB")
            return np.asarray(flat, dtype=np.uint8)
    except FileNotFoundError as exc:
        raise VisionError(f"image not found: {path}", "missing-image") from exc
    except (UnidentifiedImageError, OSError) as exc:
        raise VisionError(f"cannot read image {path}: {exc}", "bad-image") from exc


def _luma(rgb: np.ndarray) -> np.ndarray:
    channels = rgb.astype(np.float32)
    return (
        channels[..., 0] * 0.29889531
        + channels[..., 1] * 0.58662247
        + channels[..., 2] * 0.11448223
    )


def _yiq(rgb: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    channels = rgb.astype(np.float32)
    red, green, blue = channels[..., 0], channels[..., 1], channels[..., 2]
    y = red * 0.29889531 + green * 0.58662247 + blue * 0.11448223
    i = red * 0.59597799 - green * 0.27417610 - blue * 0.32180189
    q = red * 0.21147017 - green * 0.52261711 + blue * 0.31114694
    return y, i, q


def _color_delta(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    ay, ai, aq = _yiq(a)
    by, bi, bq = _yiq(b)
    dy, di, dq = ay - by, ai - bi, aq - bq
    return 0.5053 * dy * dy + 0.299 * di * di + 0.1957 * dq * dq


def _window_extremes(plane: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    height, width = plane.shape
    padded = np.pad(plane, 1, mode="edge")
    views = [
        padded[dy : dy + height, dx : dx + width]
        for dy in range(3)
        for dx in range(3)
    ]
    stacked = np.stack(views)
    return stacked.min(axis=0), stacked.max(axis=0)


def _antialias_mask(golden: np.ndarray, candidate: np.ndarray) -> np.ndarray:
    gold_luma = _luma(golden)
    cand_luma = _luma(candidate)
    gold_min, gold_max = _window_extremes(gold_luma)
    cand_min, cand_max = _window_extremes(cand_luma)
    margin = 1.0
    bracketed = (
        (gold_luma >= cand_min - margin)
        & (gold_luma <= cand_max + margin)
        & (cand_luma >= gold_min - margin)
        & (cand_luma <= gold_max + margin)
    )
    blended = (
        (gold_luma > gold_min + margin) & (gold_luma < gold_max - margin)
    ) | ((cand_luma > cand_min + margin) & (cand_luma < cand_max - margin))
    return bracketed & blended


def _pad_to(image: np.ndarray, height: int, width: int) -> np.ndarray:
    if image.shape[0] == height and image.shape[1] == width:
        return image
    padded = np.full((height, width, 3), 255, dtype=np.uint8)
    padded[: image.shape[0], : image.shape[1]] = image
    return padded


def _changed_regions(changed: np.ndarray, block: int, limit: int) -> list[Rect]:
    height, width = changed.shape
    rows = (height + block - 1) // block
    cols = (width + block - 1) // block
    padded = np.zeros((rows * block, cols * block), dtype=bool)
    padded[:height, :width] = changed
    coarse = padded.reshape(rows, block, cols, block).any(axis=(1, 3))
    seen = np.zeros_like(coarse)
    rects: list[Rect] = []
    for start_y, start_x in zip(*np.nonzero(coarse)):
        if seen[start_y, start_x]:
            continue
        seen[start_y, start_x] = True
        stack = [(int(start_y), int(start_x))]
        cells: list[tuple[int, int]] = []
        while stack:
            cell_y, cell_x = stack.pop()
            cells.append((cell_y, cell_x))
            for next_y in range(max(cell_y - 1, 0), min(cell_y + 2, rows)):
                for next_x in range(max(cell_x - 1, 0), min(cell_x + 2, cols)):
                    if coarse[next_y, next_x] and not seen[next_y, next_x]:
                        seen[next_y, next_x] = True
                        stack.append((next_y, next_x))
        top = min(cell[0] for cell in cells) * block
        bottom = min((max(cell[0] for cell in cells) + 1) * block, height)
        left = min(cell[1] for cell in cells) * block
        right = min((max(cell[1] for cell in cells) + 1) * block, width)
        window = changed[top:bottom, left:right]
        ys, xs = np.nonzero(window)
        rects.append(
            Rect(
                left + int(xs.min()),
                top + int(ys.min()),
                int(xs.max() - xs.min()) + 1,
                int(ys.max() - ys.min()) + 1,
            )
        )
    rects.sort(key=lambda rect: rect.w * rect.h, reverse=True)
    return rects[:limit]


def _render(
    golden: np.ndarray,
    changed: np.ndarray,
    ignored: np.ndarray,
    missing: np.ndarray,
    rects: Sequence[Rect],
    out_path: Path,
) -> None:
    faded = 255.0 - (255.0 - _luma(golden)) * BASE_FADE
    canvas = np.repeat(faded[:, :, None], 3, axis=2).astype(np.uint8)
    canvas[ignored] = MASK_COLOR
    canvas[missing] = MISSING_COLOR
    canvas[changed] = DIFF_COLOR
    image = Image.fromarray(canvas, mode="RGB")
    draw = ImageDraw.Draw(image)
    for rect in rects:
        draw.rectangle(
            (
                max(rect.x - 2, 0),
                max(rect.y - 2, 0),
                min(rect.x + rect.w + 1, image.width - 1),
                min(rect.y + rect.h + 1, image.height - 1),
            ),
            outline=DIFF_COLOR,
            width=1,
        )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    image.save(out_path, format="PNG")


def compare(
    golden_path: Path,
    candidate_path: Path,
    out_path: Path | None,
    *,
    threshold: float = 0.1,
    antialias: bool = True,
    max_ratio: float = 0.0,
    masks: Sequence[Rect] = (),
    block: int = 16,
    max_regions: int = 64,
) -> Comparison:
    golden = load_rgb(golden_path)
    candidate = load_rgb(candidate_path)
    golden_size = (int(golden.shape[1]), int(golden.shape[0]))
    candidate_size = (int(candidate.shape[1]), int(candidate.shape[0]))

    height = max(golden.shape[0], candidate.shape[0])
    width = max(golden.shape[1], candidate.shape[1])
    size_mismatch = golden_size != candidate_size

    golden_full = _pad_to(golden, height, width)
    candidate_full = _pad_to(candidate, height, width)

    shared = np.zeros((height, width), dtype=bool)
    shared[
        : min(golden.shape[0], candidate.shape[0]),
        : min(golden.shape[1], candidate.shape[1]),
    ] = True

    ignored = mask_array(masks, height, width)
    limit = MAX_COLOR_DELTA * threshold * threshold
    over_threshold = _color_delta(golden_full, candidate_full) > limit
    if antialias:
        over_threshold &= ~_antialias_mask(golden_full, candidate_full)

    missing = ~shared & ~ignored
    changed = (over_threshold & shared & ~ignored) | missing
    considered = int(height * width - np.count_nonzero(ignored))
    pixels = int(np.count_nonzero(changed))
    ratio = pixels / considered if considered else 0.0

    if out_path is not None:
        rects = _changed_regions(changed, block, max_regions) if pixels else []
        _render(golden_full, changed, ignored, missing, rects, out_path)

    return Comparison(
        differs=ratio > max_ratio,
        pixels=pixels,
        ratio=ratio,
        width=width,
        height=height,
        size_mismatch=size_mismatch,
        golden_size=golden_size,
        candidate_size=candidate_size,
    )
