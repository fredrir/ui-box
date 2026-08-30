from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
from PIL import Image, ImageDraw

from uibox_vision.cli import main
from uibox_vision.goldens import approve, get
from uibox_vision.imagediff import compare
from uibox_vision.masks import Rect, parse_masks
from uibox_vision.report import build

SIZE = (200, 120)
SCALE = 4


def _shape(path: Path, offset: int = 0) -> None:
    canvas = Image.new("RGB", (SIZE[0] * SCALE, SIZE[1] * SCALE), "white")
    draw = ImageDraw.Draw(canvas)
    draw.polygon(
        [
            (20 * SCALE + offset, 20 * SCALE),
            (170 * SCALE + offset, 40 * SCALE),
            (60 * SCALE + offset, 100 * SCALE),
        ],
        fill="black",
    )
    canvas.resize(SIZE, Image.BOX).save(path)


def _with_block(source: Path, target: Path, box: tuple[int, int, int, int]) -> None:
    with Image.open(source) as image:
        edited = image.convert("RGB")
    draw = ImageDraw.Draw(edited)
    draw.rectangle(box, fill=(220, 20, 60))
    edited.save(target)


@pytest.fixture()
def golden(tmp_path: Path) -> Path:
    path = tmp_path / "golden.png"
    _shape(path)
    return path


def test_identical_images_match(golden: Path, tmp_path: Path) -> None:
    result = compare(golden, golden, tmp_path / "diff.png")
    assert result.pixels == 0
    assert result.ratio == 0.0
    assert result.differs is False
    assert (tmp_path / "diff.png").is_file()


def test_subpixel_shift_is_tolerated(golden: Path, tmp_path: Path) -> None:
    shifted = tmp_path / "shifted.png"
    _shape(shifted, offset=1)
    tolerant = compare(golden, shifted, None)
    strict = compare(golden, shifted, None, antialias=False)
    assert tolerant.differs is False
    assert tolerant.pixels == 0
    assert strict.pixels > 0


def test_real_change_is_reported(golden: Path, tmp_path: Path) -> None:
    candidate = tmp_path / "candidate.png"
    _with_block(golden, candidate, (150, 90, 189, 109))
    out = tmp_path / "diff.png"
    result = compare(golden, candidate, out)
    assert result.differs is True
    assert result.pixels == 40 * 20
    assert 0.0 < result.ratio < 1.0
    with Image.open(out) as image:
        assert image.size == SIZE


def test_mask_hides_dynamic_region(golden: Path, tmp_path: Path) -> None:
    candidate = tmp_path / "candidate.png"
    _with_block(golden, candidate, (150, 90, 189, 109))
    masks = parse_masks(["148,88,45,25"])
    assert masks == [Rect(148, 88, 45, 25)]
    result = compare(golden, candidate, tmp_path / "diff.png", masks=masks)
    assert result.differs is False
    assert result.pixels == 0


def test_mask_from_json_file(golden: Path, tmp_path: Path) -> None:
    spec = tmp_path / "masks.json"
    spec.write_text(json.dumps({"masks": [{"x": 148, "y": 88, "width": 45, "height": 25}]}))
    candidate = tmp_path / "candidate.png"
    _with_block(golden, candidate, (150, 90, 189, 109))
    result = compare(golden, candidate, None, masks=parse_masks([f"@{spec}"]))
    assert result.pixels == 0


def test_size_mismatch(golden: Path, tmp_path: Path) -> None:
    smaller = tmp_path / "small.png"
    with Image.open(golden) as image:
        image.crop((0, 0, 180, 120)).save(smaller)
    result = compare(golden, smaller, tmp_path / "diff.png")
    assert result.size_mismatch is True
    assert result.differs is True
    assert result.pixels == 20 * 120
    assert result.golden_size == SIZE
    assert result.candidate_size == (180, 120)


def test_cli_diff_emits_contract_shape(golden: Path, tmp_path: Path, capsys) -> None:
    candidate = tmp_path / "candidate.png"
    _with_block(golden, candidate, (150, 90, 189, 109))
    code = main(
        [
            "diff",
            "--golden",
            str(golden),
            "--candidate",
            str(candidate),
            "--out",
            str(tmp_path / "d.png"),
            "--json",
        ]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    assert set(payload) == {"differs", "pixels", "ratio"}
    assert payload["differs"] is True
    assert isinstance(payload["pixels"], int)


def test_cli_diff_missing_image_is_json_error(tmp_path: Path, capsys) -> None:
    code = main(
        [
            "diff",
            "--golden",
            str(tmp_path / "nope.png"),
            "--candidate",
            str(tmp_path / "nope.png"),
        ]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 1
    assert payload["code"] == "missing-image"


def _bare_store(tmp_path: Path) -> str:
    store = tmp_path / "goldens.git"
    subprocess.run(["git", "init", "--bare", "-b", "main", str(store)], check=True, capture_output=True)
    return str(store)


def test_golden_round_trip(golden: Path, tmp_path: Path) -> None:
    store = _bare_store(tmp_path)
    name = "shop/checkout/desktop"

    missing = get(store, name, tmp_path / "out.png")
    assert missing["found"] is False

    written = approve(store, name, golden, "20260828T101500Z-deadbeef", "abc123")
    assert written["changed"] is True
    assert written["path"] == "shop/checkout/desktop.png"

    fetched = get(store, name, tmp_path / "out.png")
    assert fetched["found"] is True
    assert fetched["commit"] == written["commit"]
    assert (tmp_path / "out.png").read_bytes() == golden.read_bytes()

    log = subprocess.run(
        ["git", "-C", store, "log", "-1", "--format=%B"], check=True, capture_output=True, text=True
    ).stdout
    assert "Uibox-Run: 20260828T101500Z-deadbeef" in log
    assert "Uibox-Project-Sha: abc123" in log

    again = approve(store, name, golden, "run-2", "abc123")
    assert again["changed"] is False
    assert again["commit"] == written["commit"]

    updated = tmp_path / "updated.png"
    _with_block(golden, updated, (10, 10, 40, 40))
    third = approve(store, name, updated, "run-3", "def456")
    assert third["changed"] is True
    assert third["commit"] != written["commit"]

    count = subprocess.run(
        ["git", "-C", store, "rev-list", "--count", "main"], check=True, capture_output=True, text=True
    ).stdout.strip()
    assert count == "2"


def test_golden_store_is_created_on_first_approve(golden: Path, tmp_path: Path) -> None:
    store = str(tmp_path / "fresh" / "goldens.git")
    written = approve(store, "shop/cart/mobile", golden, "run-1", "sha-1")
    assert written["changed"] is True
    assert get(store, "shop/cart/mobile", tmp_path / "back.png")["found"] is True


def test_golden_name_is_validated(golden: Path, tmp_path: Path) -> None:
    store = _bare_store(tmp_path)
    from uibox_vision.emit import VisionError

    with pytest.raises(VisionError):
        approve(store, "../escape/x", golden, "run", "sha")
    with pytest.raises(VisionError):
        approve(store, "flat", golden, "run", "sha")


def _run_dir(tmp_path: Path) -> Path:
    run = tmp_path / "20260828T101500Z-deadbeef"
    (run / "snaps").mkdir(parents=True)
    (run / "diff").mkdir()
    (run / "meta.json").write_text(
        json.dumps(
            {
                "project": "shop",
                "lab": "ui-box-backend",
                "backend": "ssh://fredrir@ui-box-backend",
                "surface": "web",
                "git_sha": "abc123",
                "diff_hash": "0" * 8,
                "artifact_hash": "1" * 8,
                "started": "2026-08-28T10:15:00Z",
                "ended": "2026-08-28T10:15:12Z",
                "verdict": "fail",
                "steps_total": 3,
                "steps_failed": 1,
            }
        )
    )
    (run / "steps.yaml").write_text(
        "- open: http://host:3000\n"
        "  ok: true\n"
        "  ms: 120\n"
        "- click: \"role=button[name=Submit]\"\n"
        "  ok: false\n"
        "  error: timeout waiting for selector\n"
        "- snap: {name: after-submit, mode: text}\n"
        "  ok: true\n"
    )
    (run / "console.jsonl").write_text(
        json.dumps({"ts": 1, "level": "error", "text": "TypeError: x is not a function", "url": "http://host/app.js", "line": 42})
        + "\n"
        + json.dumps({"ts": 2, "level": "log", "text": "hello"})
        + "\n"
    )
    (run / "network.jsonl").write_text(
        json.dumps({"method": "get", "url": "http://host/api/cart", "status": 500})
        + "\n"
        + json.dumps({"method": "GET", "url": "http://host/ok", "status": 200})
        + "\n"
        + json.dumps({"method": "POST", "url": "http://host/api/x", "error": "ECONNREFUSED"})
        + "\n"
    )
    (run / "snaps" / "after-submit.txt").write_text("Welcome\n")
    (run / "diff" / "after-submit.png").write_bytes(b"\x89PNG\r\n\x1a\n")
    return run


def test_report_shape(tmp_path: Path) -> None:
    run = _run_dir(tmp_path)
    report = build(run)
    assert report["verdict"] == "fail"
    assert report["project"] == "shop"
    assert report["duration_s"] == 12.0
    assert report["steps"]["total"] == 3
    assert report["steps"]["failed"] == 1
    assert report["steps"]["items"][1] == {
        "i": 1,
        "action": "click",
        "arg": "role=button[name=Submit]",
        "ok": False,
        "error": "timeout waiting for selector",
    }
    assert report["steps"]["items"][2]["action"] == "snap"
    assert len(report["console_errors"]) == 1
    assert "app.js" in report["console_errors"][0]
    assert report["network_failures"] == [
        "GET http://host/api/cart -> 500",
        "POST http://host/api/x -> ECONNREFUSED",
    ]
    assert report["snaps"] == {"after-submit": "snaps/after-submit.txt"}
    assert report["diffs"] == {"after-submit": "diff/after-submit.png"}


def test_report_tolerates_torn_steps_file(tmp_path: Path) -> None:
    run = _run_dir(tmp_path)
    with (run / "steps.yaml").open("a") as handle:
        handle.write("- click: \"css=#pay\n")
    report = build(run)
    assert len(report["steps"]["items"]) == 3


def test_report_cli_writes_file(tmp_path: Path, capsys) -> None:
    run = _run_dir(tmp_path)
    code = main(["report", "--run-dir", str(run)])
    summary = json.loads(capsys.readouterr().out)
    assert code == 0
    assert summary["verdict"] == "fail"
    assert summary["console_errors"] == 1
    written = json.loads((run / "report.json").read_text())
    assert written["run"] == run.name
    assert written["dir"] == str(run.resolve())


def test_push_retries_after_rejection(golden: Path, tmp_path: Path) -> None:
    store = _bare_store(tmp_path)
    first = approve(store, "shop/checkout/desktop", golden, "run-1", "sha-1")

    hook = Path(store) / "hooks" / "update"
    hook.write_text('#!/bin/sh\nrm -f "$0"\necho "busy" >&2\nexit 1\n')
    hook.chmod(0o755)

    updated = tmp_path / "updated.png"
    _with_block(golden, updated, (10, 10, 40, 40))
    second = approve(store, "shop/checkout/desktop", updated, "run-2", "sha-2")

    assert second["changed"] is True
    assert second["commit"] != first["commit"]
    assert not hook.exists()
    fetched = get(store, "shop/checkout/desktop", tmp_path / "back.png")
    assert (tmp_path / "back.png").read_bytes() == updated.read_bytes()
    assert fetched["commit"] == second["commit"]
