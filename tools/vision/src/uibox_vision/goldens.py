from __future__ import annotations

import hashlib
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .emit import VisionError
from .gitstore import GoldenStore

NAME_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*(?:/[A-Za-z0-9][A-Za-z0-9._-]*)+$")
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def golden_path(name: str) -> str:
    if not NAME_PATTERN.match(name):
        raise VisionError(
            f"golden name must look like project/flow/variant, got {name!r}", "bad-name"
        )
    return f"{name}.png"


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _require_png(data: bytes, source: str) -> None:
    if not data.startswith(PNG_MAGIC):
        raise VisionError(f"{source} is not a PNG", "bad-image")


def _one_line(value: str) -> str:
    return " ".join(value.split())


def get(store_location: str, name: str, out: Path) -> dict[str, Any]:
    path = golden_path(name)
    store = GoldenStore(store_location)
    blob = store.read(path)
    if blob is None:
        return {"name": name, "path": path, "found": False}
    _require_png(blob.data, f"golden {name}")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(blob.data)
    return {
        "name": name,
        "path": path,
        "found": True,
        "out": str(out),
        "bytes": len(blob.data),
        "sha256": _digest(blob.data),
        "commit": blob.commit,
    }


def approve(
    store_location: str, name: str, png: Path, run: str, sha: str
) -> dict[str, Any]:
    path = golden_path(name)
    try:
        data = png.read_bytes()
    except OSError as exc:
        raise VisionError(f"cannot read {png}: {exc}", "missing-image") from exc
    _require_png(data, str(png))

    approved = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    message = "\n".join(
        [
            f"golden: {name}",
            "",
            f"Uibox-Name: {name}",
            f"Uibox-Run: {_one_line(run)}",
            f"Uibox-Project-Sha: {_one_line(sha)}",
            f"Uibox-Bytes: {len(data)}",
            f"Uibox-Sha256: {_digest(data)}",
            f"Uibox-Approved: {approved}",
        ]
    )
    written = GoldenStore(store_location).write(path, data, message)
    return {
        "name": name,
        "path": path,
        "commit": written.commit,
        "branch": written.branch,
        "changed": written.changed,
        "bytes": len(data),
        "sha256": _digest(data),
        "store": store_location,
    }
