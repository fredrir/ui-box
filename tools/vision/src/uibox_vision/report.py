from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable

import yaml

from .emit import VisionError

MAX_STEPS = 200
MAX_CONSOLE = 20
MAX_NETWORK = 20
MAX_YAML_REPAIR = 64
ACTION_KEYS = (
    "open",
    "click",
    "type",
    "key",
    "wait_for",
    "assert_text",
    "assert",
    "snap",
    "eval",
    "hover",
    "scroll",
    "fill",
    "select",
    "press",
    "wait",
    "close",
)
ERROR_LEVELS = {"error", "severe", "exception", "critical", "fatal", "assert"}
META_FIELDS = ("project", "lab", "surface", "backend", "git_sha", "started", "ended")


def _clip(value: str, limit: int) -> str:
    text = " ".join(value.split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def _read_json(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {}
    try:
        loaded = json.loads(path.read_text(errors="replace"))
    except json.JSONDecodeError:
        return {}
    return loaded if isinstance(loaded, dict) else {}


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        return []
    entries: list[dict[str, Any]] = []
    for line in path.read_text(errors="replace").splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            loaded = json.loads(stripped)
        except json.JSONDecodeError:
            continue
        if isinstance(loaded, dict):
            entries.append(loaded)
    return entries


def _flatten(documents: Iterable[Any]) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for document in documents:
        if document is None:
            continue
        if isinstance(document, list):
            entries.extend(item for item in document if isinstance(item, dict))
        elif isinstance(document, dict):
            if isinstance(document.get("steps"), list):
                entries.extend(
                    item for item in document["steps"] if isinstance(item, dict)
                )
            else:
                entries.append(document)
    return entries


def _read_steps(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        return []
    lines = path.read_text(errors="replace").splitlines()
    for _ in range(MAX_YAML_REPAIR):
        if not lines:
            return []
        try:
            return _flatten(yaml.safe_load_all("\n".join(lines)))
        except yaml.YAMLError:
            lines.pop()
    return []


def _summarise(value: Any) -> str | None:
    if value is None or isinstance(value, bool):
        return None
    if isinstance(value, str):
        return _clip(value, 160)
    if isinstance(value, dict):
        for key in ("selector", "target", "text", "name", "url", "value", "expr"):
            candidate = value.get(key)
            if isinstance(candidate, (str, int, float)):
                return _clip(str(candidate), 160)
        return _clip(json.dumps(value, separators=(",", ":")), 160)
    return _clip(str(value), 160)


def _action_of(entry: dict[str, Any]) -> tuple[str, Any]:
    inner = entry.get("step")
    if isinstance(inner, dict):
        return _action_of(inner)
    for key in ACTION_KEYS:
        if key in entry:
            return key, entry[key]
    for key in ("action", "kind", "op", "name"):
        value = entry.get(key)
        if isinstance(value, str):
            return value, entry.get("arg", entry.get("value", entry.get("target")))
    return "step", None


def _step_ok(entry: dict[str, Any]) -> bool:
    for key in ("ok", "passed", "success"):
        if key in entry:
            return bool(entry[key])
    status = entry.get("status") or entry.get("result")
    if isinstance(status, str):
        return status.lower() in {"ok", "pass", "passed", "success", "done"}
    return not any(key in entry for key in ("error", "failure", "exception"))


def _step_error(entry: dict[str, Any]) -> str | None:
    for key in ("error", "failure", "exception", "message"):
        value = entry.get(key)
        if isinstance(value, dict):
            value = value.get("message") or value.get("text") or json.dumps(value)
        if isinstance(value, str) and value.strip():
            return _clip(value, 300)
    return None


def _step_ms(entry: dict[str, Any]) -> int | None:
    for key in ("ms", "duration_ms", "elapsed_ms", "took_ms"):
        value = entry.get(key)
        if isinstance(value, (int, float)):
            return int(value)
    return None


def _steps(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for index, entry in enumerate(entries[:MAX_STEPS]):
        action, argument = _action_of(entry)
        item: dict[str, Any] = {"i": index, "action": action}
        summary = _summarise(argument)
        if summary:
            item["arg"] = summary
        ok = _step_ok(entry)
        item["ok"] = ok
        if not ok:
            error = _step_error(entry)
            if error:
                item["error"] = error
        elapsed = _step_ms(entry)
        if elapsed is not None:
            item["ms"] = elapsed
        items.append(item)
    return items


def _console_errors(entries: list[dict[str, Any]]) -> list[str]:
    messages: list[str] = []
    for entry in entries:
        level = ""
        for key in ("level", "type", "kind", "severity"):
            value = entry.get(key)
            if isinstance(value, str):
                level = value.lower()
                break
        if level not in ERROR_LEVELS:
            continue
        text = ""
        for key in ("text", "message", "msg", "args", "value"):
            value = entry.get(key)
            if isinstance(value, str) and value.strip():
                text = value
                break
            if isinstance(value, list) and value:
                text = " ".join(str(item) for item in value)
                break
        location = entry.get("url") or entry.get("source") or entry.get("location")
        if isinstance(location, str) and location:
            line = entry.get("line") or entry.get("lineNumber")
            suffix = f":{line}" if isinstance(line, int) else ""
            text = f"{text} [{_clip(location, 80)}{suffix}]"
        messages.append(_clip(text or json.dumps(entry, separators=(",", ":")), 300))
    return messages


def _network_failures(entries: list[dict[str, Any]]) -> list[str]:
    failures: list[str] = []
    for entry in entries:
        status = entry.get("status", entry.get("statusCode", entry.get("status_code")))
        error = entry.get("error") or entry.get("failure") or entry.get("errorText")
        ok = entry.get("ok")
        bad_status = isinstance(status, int) and status >= 400
        if not bad_status and not error and ok is not False:
            continue
        method = str(entry.get("method", "GET")).upper()
        url = _clip(str(entry.get("url", "")), 160)
        outcome = str(status) if bad_status else _clip(str(error or "failed"), 120)
        failures.append(f"{method} {url} -> {outcome}")
    return failures


def _artifacts(directory: Path, suffixes: tuple[str, ...]) -> dict[str, str]:
    if not directory.is_dir():
        return {}
    found: dict[str, str] = {}
    for path in sorted(directory.iterdir()):
        if path.is_file() and path.suffix in suffixes:
            found[path.stem] = f"{directory.name}/{path.name}"
    return found


def _parse_time(value: Any) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def build(run_dir: Path) -> dict[str, Any]:
    if not run_dir.is_dir():
        raise VisionError(f"run directory not found: {run_dir}", "missing-run")

    meta = _read_json(run_dir / "meta.json")
    entries = _read_steps(run_dir / "steps.yaml")
    items = _steps(entries)
    failed = sum(1 for item in items if not item["ok"])
    total = meta.get("steps_total")
    if not isinstance(total, int):
        total = len(entries)
    declared_failed = meta.get("steps_failed")
    if isinstance(declared_failed, int):
        failed = max(failed, declared_failed)

    console = _console_errors(_read_jsonl(run_dir / "console.jsonl"))
    network = _network_failures(_read_jsonl(run_dir / "network.jsonl"))

    verdict = meta.get("verdict")
    if not isinstance(verdict, str) or not verdict:
        if failed:
            verdict = "fail"
        elif entries or meta:
            verdict = "pass"
        else:
            verdict = "unknown"

    report: dict[str, Any] = {
        "run": str(meta.get("run") or run_dir.name),
        "dir": str(run_dir.resolve()),
        "verdict": verdict,
    }
    for field in META_FIELDS:
        value = meta.get(field)
        if value not in (None, ""):
            report[field] = value

    started = _parse_time(meta.get("started"))
    ended = _parse_time(meta.get("ended"))
    if started and ended:
        report["duration_s"] = round((ended - started).total_seconds(), 3)

    report["steps"] = {"total": total, "failed": failed, "items": items}
    if len(entries) > MAX_STEPS:
        report["steps"]["omitted"] = len(entries) - MAX_STEPS

    if console:
        report["console_errors"] = console[:MAX_CONSOLE]
        if len(console) > MAX_CONSOLE:
            report["console_errors_omitted"] = len(console) - MAX_CONSOLE
    if network:
        report["network_failures"] = network[:MAX_NETWORK]
        if len(network) > MAX_NETWORK:
            report["network_failures_omitted"] = len(network) - MAX_NETWORK

    snaps = _artifacts(run_dir / "snaps", (".png", ".txt"))
    if snaps:
        report["snaps"] = snaps
    diffs = _artifacts(run_dir / "diff", (".png",))
    if diffs:
        report["diffs"] = diffs
    return report
