from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from .emit import VisionError

REMOTE_SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*://")
SCP_SYNTAX = re.compile(r"^[^/]+@[^/:]+:")
DEFAULT_BRANCH = "main"
FALLBACK_NAME = "uibox-vision"
FALLBACK_EMAIL = "uibox-vision@localhost"


@dataclass(frozen=True)
class Blob:
    data: bytes
    commit: str


@dataclass(frozen=True)
class Written:
    commit: str
    branch: str
    changed: bool


def _timeout() -> float:
    raw = os.environ.get("UIBOX_GIT_TIMEOUT", "120")
    try:
        return float(raw)
    except ValueError:
        return 120.0


def _env() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("GIT_TERMINAL_PROMPT", "0")
    env.setdefault("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
    env["LC_ALL"] = "C"
    return env


def _run(args: Sequence[str], *, check: bool, binary: bool) -> subprocess.CompletedProcess:
    try:
        result = subprocess.run(
            ["git", *args],
            capture_output=True,
            env=_env(),
            timeout=_timeout(),
            text=not binary,
        )
    except FileNotFoundError as exc:
        raise VisionError("git executable not found on PATH", "no-git") from exc
    except subprocess.TimeoutExpired as exc:
        raise VisionError(f"git timed out: git {' '.join(args)}", "git-timeout") from exc
    if check and result.returncode != 0:
        stderr = result.stderr
        if binary:
            stderr = stderr.decode("utf-8", "replace")
        raise VisionError(
            f"git {' '.join(args)} failed: {stderr.strip()}", "git-failed"
        )
    return result


def git(args: Sequence[str], check: bool = True) -> subprocess.CompletedProcess:
    return _run(args, check=check, binary=False)


def git_bytes(args: Sequence[str], check: bool = True) -> subprocess.CompletedProcess:
    return _run(args, check=check, binary=True)


def is_remote(location: str) -> bool:
    return bool(REMOTE_SCHEME.match(location)) or bool(SCP_SYNTAX.match(location))


def _cache_root() -> Path:
    explicit = os.environ.get("UIBOX_VISION_CACHE")
    if explicit:
        return Path(explicit).expanduser()
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        return Path(xdg).expanduser() / "uibox-vision"
    try:
        base = Path.home() / ".cache"
    except RuntimeError:
        base = Path(tempfile.gettempdir()) / "uibox-cache"
    return base / "uibox-vision"


def _config_value(repo: Path, key: str) -> str:
    result = git(["-C", str(repo), "config", "--get", key], check=False)
    return result.stdout.strip() if result.returncode == 0 else ""


def _identity(repo: Path) -> list[str]:
    args: list[str] = []
    if not _config_value(repo, "user.name"):
        args += ["-c", f"user.name={os.environ.get('UIBOX_GIT_NAME', FALLBACK_NAME)}"]
    if not _config_value(repo, "user.email"):
        args += ["-c", f"user.email={os.environ.get('UIBOX_GIT_EMAIL', FALLBACK_EMAIL)}"]
    return args


def _ref_exists(repo: Path, branch: str) -> bool:
    return (
        git(
            ["-C", str(repo), "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}"],
            check=False,
        ).returncode
        == 0
    )


def head_branch(repo: Path) -> str:
    result = git(["-C", str(repo), "symbolic-ref", "--quiet", "--short", "HEAD"], check=False)
    if result.returncode == 0 and result.stdout.strip():
        return result.stdout.strip()
    for candidate in (DEFAULT_BRANCH, "master"):
        if _ref_exists(repo, candidate):
            return candidate
    listing = git(
        ["-C", str(repo), "for-each-ref", "--format=%(refname:short)", "refs/heads"],
        check=False,
    )
    names = listing.stdout.split()
    return names[0] if names else DEFAULT_BRANCH


class GoldenStore:
    def __init__(self, location: str) -> None:
        self.location = location
        self.remote = is_remote(location)
        self.path = None if self.remote else Path(location).expanduser()

    def _local_repo(self) -> Path | None:
        if self.path is None or not self.path.exists():
            return None
        if git(["-C", str(self.path), "rev-parse", "--git-dir"], check=False).returncode != 0:
            raise VisionError(f"golden store is not a git repository: {self.path}", "bad-store")
        return self.path

    def _mirror(self) -> Path:
        digest = hashlib.sha256(self.location.encode()).hexdigest()[:16]
        mirror = _cache_root() / "stores" / f"{digest}.git"
        if (mirror / "HEAD").exists():
            refreshed = git(["-C", str(mirror), "remote", "update", "--prune"], check=False)
            if refreshed.returncode != 0:
                time.sleep(0.5)
                git(["-C", str(mirror), "remote", "update", "--prune"])
            return mirror
        if mirror.exists():
            shutil.rmtree(mirror)
        mirror.parent.mkdir(parents=True, exist_ok=True)
        staging = mirror.with_name(f"{mirror.name}.{os.getpid()}.tmp")
        if staging.exists():
            shutil.rmtree(staging)
        git(["clone", "--mirror", self.location, str(staging)])
        try:
            os.replace(staging, mirror)
        except OSError:
            shutil.rmtree(staging, ignore_errors=True)
        return mirror

    def read(self, path: str) -> Blob | None:
        repo = self._mirror() if self.remote else self._local_repo()
        if repo is None:
            return None
        branch = head_branch(repo)
        commit = git(
            ["-C", str(repo), "rev-parse", "--verify", "--quiet", f"{branch}^{{commit}}"],
            check=False,
        )
        if commit.returncode != 0:
            return None
        blob = git_bytes(
            ["-C", str(repo), "cat-file", "blob", f"{branch}:{path}"], check=False
        )
        if blob.returncode != 0:
            return None
        return Blob(data=blob.stdout, commit=commit.stdout.strip())

    def ensure(self) -> None:
        if self.remote or self.path is None or self.path.exists():
            return
        self.path.parent.mkdir(parents=True, exist_ok=True)
        created = git(["init", "--bare", "-b", DEFAULT_BRANCH, str(self.path)], check=False)
        if created.returncode != 0:
            git(["init", "--bare", str(self.path)])
            git(["-C", str(self.path), "symbolic-ref", "HEAD", f"refs/heads/{DEFAULT_BRANCH}"])

    def write(self, path: str, data: bytes, message: str, attempts: int = 3) -> Written:
        self.ensure()
        with tempfile.TemporaryDirectory(prefix="uibox-golden-") as scratch:
            work = Path(scratch) / "work"
            git(["clone", "--quiet", self.location, str(work)])
            branch = head_branch(work)
            identity = _identity(work)
            target = work / path
            last_error = ""
            for attempt in range(attempts):
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                git(["-C", str(work), "add", "--", path])
                staged = git(["-C", str(work), "diff", "--cached", "--quiet"], check=False)
                if staged.returncode == 0:
                    head = git(["-C", str(work), "rev-parse", "HEAD"])
                    return Written(commit=head.stdout.strip(), branch=branch, changed=False)
                git(["-C", str(work), *identity, "commit", "--quiet", "-m", message])
                head = git(["-C", str(work), "rev-parse", "HEAD"]).stdout.strip()
                push = git(
                    ["-C", str(work), "push", "--quiet", "origin", f"HEAD:refs/heads/{branch}"],
                    check=False,
                )
                if push.returncode == 0:
                    return Written(commit=head, branch=branch, changed=True)
                last_error = push.stderr.strip()
                if attempt == attempts - 1:
                    break
                fetched = git(
                    ["-C", str(work), "fetch", "--quiet", "origin", branch], check=False
                )
                if fetched.returncode != 0:
                    break
                git(["-C", str(work), "reset", "--hard", "--quiet", "FETCH_HEAD"])
            raise VisionError(f"could not push golden to {self.location}: {last_error}", "push-failed")
