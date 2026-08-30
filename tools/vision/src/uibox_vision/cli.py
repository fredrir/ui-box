from __future__ import annotations

import argparse
import os
from pathlib import Path
from typing import Any, Sequence

from . import __version__
from .emit import VisionError, emit, emit_error

DEFAULT_STORE = "/var/lib/ui-box-state/ui-box/goldens.git"


def _store(value: str | None) -> str:
    return value or os.environ.get("UIBOX_GOLDENS") or DEFAULT_STORE


def _add_json_flag(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--json", action="store_true", help="emit json on stdout (always on)"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="uibox-vision",
        description="perceptual diffing, golden store and run reporting for ui-box",
    )
    parser.add_argument("--version", action="version", version=__version__)
    commands = parser.add_subparsers(dest="command", required=True)

    diff = commands.add_parser("diff", help="compare a golden png against a candidate")
    diff.add_argument("--golden", required=True, type=Path)
    diff.add_argument("--candidate", required=True, type=Path)
    diff.add_argument("--out", type=Path, help="write a visual diff png here")
    diff.add_argument(
        "--mask",
        action="append",
        default=[],
        metavar="SPEC",
        help="ignore a region: x,y,w,h or @file.json or inline json",
    )
    diff.add_argument(
        "--threshold",
        type=float,
        default=0.1,
        help="per pixel colour tolerance, 0 strict to 1 permissive",
    )
    diff.add_argument(
        "--max-ratio",
        type=float,
        default=0.0,
        help="fraction of differing pixels tolerated before differs is true",
    )
    diff.add_argument(
        "--no-antialias",
        dest="antialias",
        action="store_false",
        help="count subpixel antialiasing noise as a difference",
    )
    diff.add_argument(
        "--exit-code",
        action="store_true",
        help="exit 1 when the images differ",
    )
    _add_json_flag(diff)

    golden = commands.add_parser("golden", help="read and write goldens in the git store")
    actions = golden.add_subparsers(dest="action", required=True)

    get = actions.add_parser("get", help="check a golden out of the store")
    get.add_argument("--store", default=None, help=f"git store, default {DEFAULT_STORE}")
    get.add_argument("--name", required=True, help="project/flow/variant")
    get.add_argument("--out", required=True, type=Path)
    _add_json_flag(get)

    approve = actions.add_parser("approve", help="commit a golden into the store")
    approve.add_argument("--store", default=None, help=f"git store, default {DEFAULT_STORE}")
    approve.add_argument("--name", required=True, help="project/flow/variant")
    approve.add_argument("--png", required=True, type=Path)
    approve.add_argument("--run", required=True, help="run id being approved")
    approve.add_argument("--sha", required=True, help="project git sha being approved")
    _add_json_flag(approve)

    report = commands.add_parser("report", help="summarise a run directory")
    report.add_argument("--run-dir", required=True, type=Path)
    report.add_argument("--out", type=Path, help="default <run-dir>/report.json")
    _add_json_flag(report)

    return parser


def _run_diff(args: argparse.Namespace) -> dict[str, Any]:
    from .imagediff import compare
    from .masks import parse_masks

    result = compare(
        args.golden,
        args.candidate,
        args.out,
        threshold=args.threshold,
        antialias=args.antialias,
        max_ratio=args.max_ratio,
        masks=parse_masks(args.mask),
    )
    payload: dict[str, Any] = {
        "differs": result.differs,
        "pixels": result.pixels,
        "ratio": round(result.ratio, 8),
    }
    if result.size_mismatch:
        payload["size_mismatch"] = True
        payload["golden_size"] = list(result.golden_size)
        payload["candidate_size"] = list(result.candidate_size)
    return payload


def _run_golden(args: argparse.Namespace) -> dict[str, Any]:
    from . import goldens

    location = _store(args.store)
    if args.action == "get":
        return goldens.get(location, args.name, args.out)
    return goldens.approve(location, args.name, args.png, args.run, args.sha)


def _run_report(args: argparse.Namespace) -> dict[str, Any]:
    import json

    from .report import build

    run_dir = args.run_dir
    out = args.out or run_dir / "report.json"
    report = build(run_dir)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, indent=2) + "\n")
    summary = {
        "out": str(out),
        "run": report["run"],
        "verdict": report["verdict"],
        "steps_total": report["steps"]["total"],
        "steps_failed": report["steps"]["failed"],
        "console_errors": len(report.get("console_errors", [])),
        "network_failures": len(report.get("network_failures", [])),
    }
    return summary


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.command == "diff":
            payload = _run_diff(args)
            emit(payload)
            return 1 if args.exit_code and payload["differs"] else 0
        if args.command == "golden":
            emit(_run_golden(args))
            return 0
        payload = _run_report(args)
        emit(payload)
        return 0
    except VisionError as error:
        emit_error(error)
        return 1
    except OSError as error:
        emit_error(VisionError(str(error), "io"))
        return 1
