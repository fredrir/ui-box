from __future__ import annotations

import json
import sys
from typing import Any


class VisionError(Exception):
    def __init__(self, message: str, code: str = "error") -> None:
        super().__init__(message)
        self.message = message
        self.code = code


def emit(payload: dict[str, Any]) -> None:
    json.dump(payload, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")
    sys.stdout.flush()


def emit_error(error: VisionError) -> None:
    emit({"error": error.message, "code": error.code})
