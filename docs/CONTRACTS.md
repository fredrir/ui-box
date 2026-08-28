# ui-box interface contracts

Frozen before implementation. Change only by editing this file first.

## 1. Driver protocol

JSON-RPC 2.0, newline-delimited JSON over stdio. The Rust core spawns a driver
process and speaks this. Every driver implements every method.

Method names go on the wire verbatim, namespace included:

    "driver.info"   -> { name, version, surfaces: ["web"|"tauri"|"tui"] }
    "driver.open"  ({ target, viewport, options })  -> { sessionId }
    "driver.act"   ({ sessionId, step })            -> { ok, error? }
    "driver.snap"  ({ sessionId, mode, name })      -> { text?, pngPath?, console[], network[] }
    "driver.eval"  ({ sessionId, expr })            -> { value }
    "driver.close" ({ sessionId })                  -> {}

`mode` is "text" | "png" | "both". Text is the default everywhere.

DRIVER LOCALITY. The driver runs WHERE THE DISPLAY IS, which for the primary
workflow is dlab-ui, not the invoking Mac. When the backend is `ssh://`, the
core spawns the driver through it -- `ssh <host> ui-box-dom`, JSON-RPC over that
stdio. The transport is verified working. Two things follow and neither is
optional:

  1. The `runDir` passed at open must be a path valid ON THE DRIVER'S HOST. A
     Mac run directory handed to a Linux driver fails with
     `EACCES: mkdir '/private'`. The lab-side run dir is
     `~/.uibox/runs/<runid>` on the driver host.
  2. Artifacts the driver writes land on the driver's host, so the core must
     `Backend::pull` them into the local run directory after each snap. The
     agent reads them on the Mac; that is the entire point of the run directory.

With `local://` the driver is spawned directly and neither applies.

`name` on `driver.snap` is the caller's label for the artifact. The DRIVER
writes the files, to `snaps/<name>.png` and `snaps/<name>.txt` in the run
directory it was given at open, and returns the paths it wrote. The core does
not write a second copy: only the driver holds the pixels, and two writers
produced two files under different names for one logical snapshot.

The driver sanitises `name` to `[A-Za-z0-9._-]` and DEDUPLICATES it: a repeated
name becomes `<name>-2`, never an overwrite. The caller must therefore use the
`name`, `pngPath` and `txtPath` the driver RETURNS, and must never reconstruct a
path from what it sent. Overwriting would silently discard the earlier snapshot
and compare a golden against whichever call happened to run last.

Deduplication is the right default for an exploratory session, where an agent
snaps repeatedly and wants every frame. It is NOT acceptable in a replayed flow,
where a golden must map to exactly one artifact. So the core REJECTS a flow file
containing duplicate snap names before replaying it, as the authoring bug it is.
The driver stays lossless; the flow validator is where it becomes an error.

`viewport` on the wire is an object, `{ "width": 1280, "height": 800 }`. The
`1280x800` string form belongs to the human-authored step file only; the core
parses it before the call.

`step` is passed verbatim as the step-format node from §2, e.g.
`{"click":"role=button[name=Submit]"}`. The driver therefore owns both the step
vocabulary and the selector grammar; the core does not interpret either.

## 2. Step format

    version: 1
    flow: checkout
    surface: web              # web | tauri | tui
    target: http://host:3000  # or exec:/nix/store/.../bin/app, or tui:nsql
    viewport: 1280x800
    steps:
      - open: http://host:3000
      - click: "role=button[name=Submit]"
      - type: { selector: "css=#email", text: "a@b.c" }
      - key: Enter
      - wait_for: "text=Welcome"
      - assert_text: "text=Welcome"
      - snap: { name: after-submit, mode: text }

Selector grammar, uniform across drivers:

    css=SEL     DOM   (web, tauri)
    role=ROLE   DOM   (web, tauri)
    text=STR    DOM + TUI
    re=REGEX    TUI   (match against the terminal buffer)
    cell=R,C    TUI   (absolute cell)

## 3. Run directory

    <artifacts>/<runid>/
      meta.json       provenance + verdict
      steps.yaml      appended as each act lands, never only at close
      console.jsonl
      network.jsonl
      snaps/<name>.{png,txt}
      diff/<name>.png
      report.json

runid = UTC compact timestamp + "-" + 8 hex chars.

meta.json carries: project, lab, backend, surface, git_sha, diff_hash,
artifact_hash, started, ended, verdict, steps_total, steps_failed.

## 4. Configuration

Precedence: CLI flag > environment > project uibox.toml > global .env.

    UIBOX_BACKEND      ssh://fredrir@dlab-ui | local://
    UIBOX_DISPLAY      1280x800x24
    UIBOX_ARTIFACTS    .uibox/runs
    UIBOX_GOLDENS      /var/lib/dlab-state/ui-box/goldens.git
    UIBOX_SESSION_TTL  900

`--force` sets DLAB_FORCE=1 on the ssh backend. When the dlab ssh proxy
refuses to start a lab, its stderr is propagated verbatim, never wrapped.

## 5. Crate layout and pipeline API

    ui-box-core       Backend trait, Cmd, Output, shared types
       ^        ^
       |     ui-box-pipeline
       |        ^
    ui-box (CLI) ----+

Dependencies point one way only. `Backend` is a foundational capability, not a
pipeline concept, so it lives in `ui-box-core` and both consumers depend on it.

    pub struct BuildRequest { project: String, lab: String, target: String, source: Option<PathBuf>, build: Option<String>, artifact: PathBuf }
    pub struct Provenance   { git_sha: String, diff_hash: String, artifact_hash: String }
    pub struct Placed       { remote_path: PathBuf, provenance: Provenance, cached: bool }

    pub fn place(req: &BuildRequest, backend: &dyn ui_box_core::Backend) -> Result<Placed>

`place` is idempotent: identical Provenance to the last run skips build and copy
and returns cached = true.

`source` is a path on the INVOKING host (normally the Mac) holding the working
tree under test. The primary workflow is a local checkout on the Mac, an agent
that never leaves it, and a Linux artifact that cannot be produced there -- so
`place()` syncs the tree into the build lab before building:

    macie worktree  --rsync-->  build lab  --nix copy-->  dlab-ui

The sync destination is a STAGING path, `~/.uibox/src/<project>/` in the build
lab. It must never be the lab's own checkout: distro-lab already clones each
repo to `$DLAB_PROJECT`, and writing over it would destroy work that lives only
there. When `source` is `None`, the lab's own `$DLAB_PROJECT` checkout is the
build tree and no sync happens.

The rsync excludes gitignored files (`--filter=":- .gitignore"`). That is not
only for speed: it is what keeps `target/` and `node_modules/` out of the copy,
which is the same cost that makes `path:.` expensive.

Provenance is computed from the LOCAL tree before the sync, never from the
staged copy. The local worktree is the thing under test, and a hash taken after
an rsync would describe the transport rather than the source.

The `&dyn Backend` handed to `place()` is the BUILD environment -- the one
holding `req.lab`'s checkout, where the build command runs. It is NOT the
dlab-ui backend. `req.target` names the lab the artifact is placed into, and
`UIBOX_BACKEND` from §4 refers to that target, not to the build lab. `ui-box`
constructs the build backend for `req.lab` and passes it in; the pipeline never
needs a second `Backend` object, because the copy is expressed as lab names run
on the mediating host.

Shell that runs on the INVOKING host must be POSIX, or verified against BSD
userland. macie is the origin host for the primary workflow and ships BSD
tools: openrsync rejects `--protect-args`, and `date -d`, `readlink -f`,
`stat -c` and `sed -i` all differ from GNU. Shell that runs via `Backend::run`
executes in a lab, where GNU userland is guaranteed and none of this applies.
The local provenance script currently survives only because Apple happens to
ship `sha256sum` and a `sort` supporting `-z` -- that is luck, not design, so
anything added to it needs checking on a stock `PATH`.

`Backend` carries `run`, `push`, `pull`, plus backend introspection (`spec`,
`url`, `is_local`) and `require`, which is where the verbatim-stderr guarantee
lives. The invariant is not the method count -- it is that `Config` and
`select(&Config)` stay in `ui-box` and never reach `ui-box-core` or the
pipeline. The pipeline calls only `run` and `pull` and imports nothing from
`spec`, which is what makes the layering real rather than asserted.

Artifact transfer is HOST-MEDIATED, not lab-to-lab. A build lab has no ssh key
and no dlab ssh config for any other lab -- base.nix authorises only personal
keys, and the waking ProxyCommand is symlinked into the host's ~/.ssh/config.d
only. So a lab cannot reach, let alone wake, another lab. The copy therefore
runs on the host that already reaches both:

    nix copy --no-check-sigs --from ssh://<build-lab> --to ssh://dlab-ui <paths>

Prefer archie as that host when it is the ssh hop anyway; it shares a LAN with
both labs, where the Mac would stream every byte twice over the slower link.
This is the only node that can wake either lab, so it is also the only node the
sequencing can work from.

## 6. Vision tool CLI

Invoked by the Rust core as a subprocess. JSON on stdout.

    uibox-vision diff --golden A.png --candidate B.png --out D.png --json
      -> { differs: bool, pixels: int, ratio: float }
    uibox-vision golden get     --store GIT --name project/flow/variant --out F.png
    uibox-vision golden approve --store GIT --name project/flow/variant --png F.png --run ID --sha SHA
    uibox-vision report --run-dir DIR --out report.json

## 7. CLI surface

Frozen so the skill and the hooks are not written against a guess.

    ui-box doctor
    ui-box wake   [--lab NAME]
    ui-box open   <target> [--surface web|tauri|tui] [--viewport WxH]
    ui-box act    <session> <step...>
    ui-box snap   <session> [--mode text|png|both] [--name NAME]
    ui-box eval   <session> <expr>
    ui-box close  <session>
    ui-box record <session|runid> [--format uibox|playwright] [-o FILE]
    ui-box run    [flow.yaml] [--lab NAME] [--project NAME] [--force]
    ui-box verify --since <git-ref>
    ui-box runs
    ui-box show   <runid>

Exit codes are three-valued and load-bearing, because the gating hooks branch
on them:

    0   the thing under test passed
    1   the thing under test failed
    2   ui-box itself could not run (config, backend, driver)

A hook must never read 2 as a test failure. `wake` is fire-and-forget: it
returns 0 as soon as the lab is reachable or the wake is in flight, and it
never fails a caller.
