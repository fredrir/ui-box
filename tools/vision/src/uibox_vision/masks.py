from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence

import numpy as np

from .emit import VisionError


@dataclass(frozen=True)
class Rect:
    x: int
    y: int
    w: int
    h: int


def _as_rect(value: Any) -> Rect:
    if isinstance(value, dict):
        keys = {k.lower(): v for k, v in value.items()}
        if {"x", "y", "w", "h"} <= keys.keys():
            parts = [keys["x"], keys["y"], keys["w"], keys["h"]]
        elif {"x", "y", "width", "height"} <= keys.keys():
            parts = [keys["x"], keys["y"], keys["width"], keys["height"]]
        elif {"left", "top", "right", "bottom"} <= keys.keys():
            left, top = keys["left"], keys["top"]
            parts = [left, top, keys["right"] - left, keys["bottom"] - top]
        else:
            raise VisionError(f"unrecognised mask object: {value!r}", "bad-mask")
    elif isinstance(value, (list, tuple)):
        parts = list(value)
    else:
        raise VisionError(f"unrecognised mask entry: {value!r}", "bad-mask")

    if len(parts) != 4:
        raise VisionError(f"mask needs 4 numbers, got {len(parts)}", "bad-mask")
    try:
        x, y, w, h = (int(round(float(p))) for p in parts)
    except (TypeError, ValueError) as exc:
        raise VisionError(f"mask values must be numbers: {parts!r}", "bad-mask") from exc
    if w <= 0 or h <= 0:
        raise VisionError(f"mask width and height must be positive: {parts!r}", "bad-mask")
    return Rect(max(x, 0), max(y, 0), w, h)


def _from_document(document: Any) -> list[Rect]:
    if isinstance(document, dict):
        for key in ("masks", "regions", "ignore", "rects"):
            if key in document:
                return _from_document(document[key])
        return [_as_rect(document)]
    if isinstance(document, list):
        return [_as_rect(entry) for entry in document]
    raise VisionError("mask file must hold a list of rectangles", "bad-mask")


def parse_masks(specs: Sequence[str]) -> list[Rect]:
    rects: list[Rect] = []
    for spec in specs:
        text = spec.strip()
        if not text:
            continue
        if text.startswith("@"):
            path = Path(text[1:]).expanduser()
            if not path.is_file():
                raise VisionError(f"mask file not found: {path}", "bad-mask")
            text = path.read_text()
        if text.startswith("[") or text.startswith("{"):
            try:
                document = json.loads(text)
            except json.JSONDecodeError as exc:
                raise VisionError(f"mask json is invalid: {exc}", "bad-mask") from exc
            rects.extend(_from_document(document))
            continue
        rects.append(_as_rect(text.replace(":", ",").split(",")))
    return rects


def mask_array(rects: Sequence[Rect], height: int, width: int) -> np.ndarray:
    covered = np.zeros((height, width), dtype=bool)
    for rect in rects:
        y0 = min(rect.y, height)
        x0 = min(rect.x, width)
        y1 = min(rect.y + rect.h, height)
        x1 = min(rect.x + rect.w, width)
        if y1 > y0 and x1 > x0:
            covered[y0:y1, x0:x1] = True
    return covered
