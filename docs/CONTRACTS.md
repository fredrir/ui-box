# ui-box interface contracts

Frozen before implementation. Change only by editing this file first.

## 1. Driver protocol

JSON-RPC 2.0, newline-delimited JSON over stdio. The Rust core spawns a driver
process and speaks this. Every driver implements every method.

Method names go on the wire verbatim, namespace included:

    "driver.info"   -> { name, version, surfaces: ["web"|"tauri"|"tui"], tauri? }
    "driver.open"  ({ target, viewport, options })  -> { sessionId }
    "driver.act"   ({ sessionId, step })            -> { ok, error? }
    "driver.snap"  ({ sessionId, mode, name })      -> { text?, pngPath?, console[], network[] }
    "driver.eval"  ({ sessionId, expr })            -> { value }
    "driver.close" ({ sessionId })                  -> {}

`mode` is "text" | "png" | "both" | "layout". Text is the default everywhere.

`layout` is the accessibility tree annotated with bounding rectangles. It exists
because TEXT FLATTENS SPACE: the tree can say an element exists and is reachable,
but not that it is FINDABLE. A blind run confirmed a "clear filters" control that
was present, labelled, reachable and hit-testable -- and 761px away at the far
edge of its card, which is the problem the user actually reported. Every text
signal said ship it.

So the guidance "prefer text, reach for png when the question is visual" is
backwards for any complaint of the form "I cannot find X": you cannot know the
question is visual until you have looked. `layout` is the cheap mode that can
answer it.

`tauri` on `driver.info` is optional and additive:

    tauri: { ok, tauriDriver, nativeDriver, source, reason? }

Only the DRIVER can answer whether the tauri surface is usable, because the
binaries resolve in the driver's own environment on the driver's host, which is
not the environment the core runs in. A core that probed for itself would be
answering a different question and would sometimes be confidently wrong.

A driver that omits the field has not said "no" -- it has said nothing. Per §8 a
consumer must report that as unknown, never as a pass.

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
      - assert_absent: "role=button[name=Clear]"
      - assert_visible: "role=button[name=Clear]"
      - snap: { name: after-submit, mode: text }

`assert_absent` asserts an element is NOT present. Conditional rendering is most
of UI work, and without this a flow can only assert something correlated with the
absence -- which keeps passing after the condition is removed. It is the negative
half of `wait_for` and must distinguish "gone" from "never looked".

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
    UIBOX_FORWARD      3000  |  3000:5173  |  3000:localhost:5173  (repeatable)
    UIBOX_TAURI_DRIVER path to tauri-driver, on the DRIVER's host
    UIBOX_NATIVE_DRIVER path to WebKitWebDriver, on the DRIVER's host

`--force` sets DLAB_FORCE=1 on the ssh backend. When the dlab ssh proxy
refuses to start a lab, its stderr is propagated verbatim, never wrapped.

FORWARDING. `localhost` in a target is the DRIVER HOST's loopback, not the
invoking machine's. A dev server on the Mac is not on it. `--forward` publishes
a local port into the lab over the driver's own ssh connection:

    --forward 3000                 lab 127.0.0.1:3000 -> here 127.0.0.1:3000
    --forward 3000:5173            lab 127.0.0.1:3000 -> here 127.0.0.1:5173
    --forward 3000:localhost:5173  lab 127.0.0.1:3000 -> here localhost:5173

Remote-first, matching `ssh -R`: the first number is the one that appears in
your target URL, because the target resolves inside the lab. Both ends bind
loopback -- the lab shares a NAT bridge with every other guest and with the
host, and a wildcard bind would publish a dev server to all of them.

ui-box DOES NOT INFER a forward. A loopback target on an ssh backend with no
covering forward is REFUSED before any ssh is spawned, with an error naming the
port and the command that would work. Inferring would reconfigure the network as
a side effect of a URL string, could succeed by accident against something
unrelated listening on that port, and would leave the agent reasoning about an
action it never requested. Narrow and loud.

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
staged copy. This is load-bearing, not stylistic, and was probed against a real
lab: a staged subdirectory carries no `.git`, so in-lab provenance would fail
outright with "not a git repository" for every subdirectory source -- and for a
repo-root source it would do something worse, silently hashing the staged copy
and describing the transport rather than the worktree the user edited.

Provenance scoping is partial by design. `diff_hash` is scoped to the synced
tree (`git diff HEAD -- .` plus untracked files under `source`), so a change
outside it does not invalidate. `git_sha` stays repo-wide: a commit anywhere
moves HEAD and still over-invalidates. That is wasteful, not wrong -- it errs
toward rebuilding. Scoping the sha would need `git log -1 -- .` and is not
worth the complexity.

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
    ui-box open   <target> [--surface web|tauri|tui] [--viewport WxH] [--forward SPEC]
    ui-box act    <session> <step...>
    ui-box snap   <session> [--mode text|png|both|layout] [--name NAME] [--clip SEL]
    ui-box eval   <session> <expr>
    ui-box close  <session>
    ui-box record <session|runid> [--format uibox|playwright] [-o FILE]
    ui-box run    [flow.yaml] [--lab NAME] [--project NAME] [--force] [--forward SPEC]
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

0, 1 and 2 are the only codes this contract defines. ANY OTHER CODE IS
OFF-CONTRACT and means the binary did not get to report -- a Rust panic exits
101, signal death exits 128+n, and neither is a verdict. A consumer must treat
an undefined code as a tooling failure and must not have a catch-all that reads
non-zero as "the UI failed". A non-zero exit forces the summary onto stdout, so
an exit 1 carrying no parseable JSON line is likewise a broken binary rather
than a result.

### MCP surface

Frozen for the same reason and separately: an agent reaches ui-box through
these names, never through argv, and each consuming project names the server
in its own `.mcp.json`.

    ui_test_prepare  ui_open  ui_act    ui_snap  ui_eval  ui_close
    ui_record        ui_run   ui_verify ui_runs  ui_show

Eleven tools. Every one accepts `project_dir`, which defaults to the server's
working directory and is where `uibox.toml`, `.uibox/runs` and relative flow
paths resolve.

Tool names and parameter names are frozen. Descriptions are NOT: they are prose,
they are rewritten whenever they can be made shorter, and they cost every
session that connects whether it calls the tool or not.

A parameter is its CLI flag without the dashes, same grammar:

    ui_open, ui_run   forward   array of §4 specs, e.g. ["3000:5173"]
    ui_snap           mode      text | png | both | layout
    ui_snap           clip      SEL, crops the png, so mode must produce one
    ui_act            action    every §2 verb, assert_absent and assert_visible included

A verb §2 lists and `action` rejects is §8 aimed at ui-box itself: the schema
advertises a vocabulary smaller than the contract, and the caller cannot tell a
verb that does not exist from one the schema forgot. `steps` is the batch form,
not the escape hatch that gap made it.

Every result carries `status` and `failure_domain`. `failure_domain` separates
`ui_test_failed` from `uibox_unusable` and is the MCP equivalent of the
three-valued exit code above -- a consumer that ignores it reproduces §8 exactly,
by reading "ui-box never ran" as "the UI is fine".

## 8. Absence read as success

The recurring bug in a verification tool is not a wrong answer. It is a
confident answer about nothing. Found three times independently, in three
components:

    a snapshot of a blank page          reads as "the UI rendered"
    `verify` with no flows or no diff   reads as "the UI is correct"
    `wait_for` against an empty page    reads as "the element arrived"

Each is an ABSENCE of evidence presented in the shape of POSITIVE evidence.
The exit code is 0, nothing errored, and nothing was proven.

The rule: a component that can produce "nothing happened" must encode it as
its own outcome, distinct from both success and failure. Not an error -- the
tree genuinely may not have moved, and a tool that cries wolf on a normal path
gets ignored exactly when it matters -- but never silently a pass. The MCP
server's `nothing_verified` (`ok: false`, `isError: false`) is the reference
shape.

Found four times, in two directions -- a blank snapshot and a `verify` skip
read absence as SUCCESS; an unreadable snapshot and an unreadable run directory
read it as a definite ANSWER (a UI failure, and a clean bill of health). The
direction is not the bug. Inferring anything at all from missing evidence is.

The resolution rule, which has held in every case: PREFER FACTS THAT CAME OVER
THE WIRE to your own reading of the world. A value ui-box reported survives a
transport problem; a file read does not. `text_bytes` guards the snapshot path,
`readable` guards diagnostics, and `localise()` guarantees that a reported path
is a file that exists. Where neither is available, say you do not know rather
than picking a side.

THE EXIT CODE IS ALSO A READING OF THE WORLD, and is only trustworthy when the
contract defining it was honoured. §7 guarantees exactly one JSON line with an
`ok` field on stdout, so an exit 1 with no parseable summary is not a verdict --
it is a binary that crashed. Reported as a verdict it becomes the worst version
of this bug: a broken tool announced to the agent as a broken application.
For the same reason, never restate an exit code in prose ("UI TEST FAILED
(exit 1)") -- a blank snapshot and a close with failed steps both exit 0.
Precise-looking text that is sometimes false is worse than text that does not
claim the detail.

A recorded flow with no assertions is this bug in its purest form. `record`
turns an exploration into a file, and a file of `click` and `snap` steps and
nothing else PASSES AGAINST A BLANK PAGE -- it is a transcript wearing the label
of a test, and it is green forever. Found by a blind run whose author noticed and
hand-wrote the assertions back in; the next author will not.

So `record` must emit assertions derived from what it observed, and a flow that
asserts nothing must not silently pass. The feature whose whole purpose is to
create regression tests is the last place this class should survive.

When adding any check, ask what its answer is when there is nothing to check.
If that answer is indistinguishable from success -- or from any definite
answer -- it is this bug again.

And ask the companion question: WHAT IS ITS ANSWER WHEN EVERYTHING IS HEALTHY?
If the check still fires, it is measuring its own construction rather than the
system. `check_target` probed the lab for a loopback target whose tunnel exists
only while a session is open, so a perfectly healthy setup always read as
refused. Advisory severity kept that from blocking, but a check that fires on
the healthy path is worse than no check: it teaches the reader the line is
noise, and the next time it fires for a real reason nobody reads it.

Both questions catch the same thing from opposite ends -- a confident answer
about nothing.

## 9. Load-bearing guarantees

Named because an invariant nobody has written down is one nobody can protect.
Every entry here is something a consumer's CORRECTNESS rests on, not merely its
convenience. Breaking one does not produce a compile error -- it produces a
tool that blames the user's application for something that is not its fault.

Changing any of these means changing its consumers in the same commit.

  1. `text_bytes` is the byte count of accessibility text HOWEVER THE DRIVER
     CHOSE TO DELIVER IT -- inline, or measured from the file after it has been
     pulled local. §1 leaves inlining optional, so deriving it only from the
     inline field would make blank-page detection depend on a driver's choice.

  2. A reported artifact path IMPLIES A FILE THAT EXISTS. `localise()` checks
     both a driver naming a file it never wrote and a pull that reports success
     without producing one. This is what lets a consumer discriminate "the file
     is empty" from "nothing was delivered".

  3. stdout ALWAYS carries exactly one JSON line with an `ok` field, and a
     non-zero exit forces it regardless of what the call site asked for. This
     is what lets exit 1 with no parseable line be read as a broken binary
     rather than as a UI failure -- a rule that INVERTS a verdict, so it is
     only safe while a failed summary cannot legitimately be absent.

  4. The exit code means what §7 says. 0, 1, 2 and nothing else; any other code
     is the binary failing to report, never a verdict.

All four fail the same way when broken, which is the property worth protecting
rather than any individual field.

A heuristic for which invariants belong on this list, and which panics or
unchecked accesses need hardening: IF CONVINCING YOURSELF IT HOLDS REQUIRED
SURVEYING SEVERAL CALL SITES, A FUTURE READER WILL NOT DO THAT SURVEY. A guard
on the adjacent line survives editing because whoever edits it sees it. An
invariant established in another function, across several construction sites,
does not -- and neither does one verified once by hand and reported in a
message. Both are true for reasons nobody will encounter again.
