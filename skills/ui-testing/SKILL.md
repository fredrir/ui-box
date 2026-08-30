---
name: ui-testing
description: Drive a real browser, Tauri window or TUI on a graphical lab with the ui-box CLI and check what actually rendered. Use before claiming any render-affecting change works — component, page, route, style sheet, template, layout, Tauri window, TUI screen. Also when asked why a screen is blank, whether a page still loads after a refactor, whether a flow survived a dependency bump, or to freeze a working flow into a committed spec. Covers session mode (open/act/snap), replaying specs, text vs png vs layout snapshots, and reading a run's verdict.
metadata:
  version: "2.0"
---

# UI testing with ui-box

A diff cannot tell you whether a page renders. `ui-box` drives a real browser,
Tauri window or terminal on a graphical lab and reports what came out.

## Rules

1. **Changed something that renders? Look at it before reporting it works.** Type-check, unit test and green build all pass on a blank page.
2. **"X is missing / broken / too subtle" → `open` and `snap` before reading code.** An agent spent minutes in the CSS of a feature that already rendered; the real complaint was that it was too faint.
3. **A blank page is not a pass.** See Verdict.

| Changed | Test |
| --- | --- |
| component, page, route, layout, template; CSS, theme, design token; loader or hook a view depends on; build config, bundler plugin, client-bundle dependency; Tauri command or window; TUI screen | required |
| docs, CI config, tests, server-only code with no client consumer | skip |

Unsure whether a change reaches the UI: run it. Do not open a PR on a build alone.

## Call timeline

One compact JSON line on stdout, human detail on stderr. `-q` for stdout alone.

    ui-box wake                                    # lab suspends when idle
    ui-box doctor                                  # exit 2 here is setup, not app
    <your dev server> &                            # runs here, not on the lab

    ui-box open http://localhost:3000              # no --forward
    {"error":"target http://localhost:3000 points at port 3000 on ui-box-backend, because
     localhost inside a target is ui-box-backend's loopback and not this machine's...",
     "error_kind":"forward_missing","ok":false}

    session=$(ui-box open http://localhost:3000 --forward 3000)
    {"backend":"ssh://fredrir@ui-box-backend","driver":{"name":"dom","version":"0.1.0"},
     "expires_in":900,"forward":["3000"],"ok":true,"run":"20260830T080007Z-d8664cb7",
     "run_dir":"...","session":"20260830T080007Z-d8664cb7","surface":"web",
     "target":"http://localhost:3000","viewport":"1280x800"}

    ui-box snap "$session" --mode both --name patchquest-home
    {"ok":true,"session":"...","snap":{"console":1,"mode":"both","name":"patchquest-home",
     "network":0,"png":"...png","text":"...txt","text_bytes":2973},"steps_total":1}
    # text confirmed a feature that did not solve the problem; only the png showed why

    ui-box act "$session" click "css=#go"
    {"error":null,"ok":true,"session":"...","step":"click css=#go","step_ok":true,
     "steps_total":2,"verb":"click"}
    # role=checkbox[name=X] read off the snapshot failed after 15s on a hidden Mantine
    # input: target what receives the click, not what the tree names

    ui-box act "$session" assert_text "text=Order confirmed"

    ui-box eval "$session" "getComputedStyle(document.querySelector('#bar')).opacity"
    {"eval":"...","expires_in":900,"ok":true,"serializable":true,"session":"...",
     "status":"passed","value":"...","value_kind":"string"}
    # a value the driver cannot serialise returns "status":"nothing_verified"

    ui-box close "$session"
    {"driver_closed":true,"ok":true,"run":"...","run_dir":"...","session":"...",
     "steps_failed":0,"steps_total":3,"verdict":"pass"}

    ui-box record "$session" -o tests/ui/home.yaml
    {"assertions":2,"flow":"home","format":"uibox","ok":true,"out":"tests/ui/home.yaml",
     "run":"...","steps":3}

    ui-box run tests/ui/home.yaml                  # replay, later, in CI
    {"flow":"home","ok":true,"run":"...","status":"pass","steps_failed":0,
     "steps_total":3,"verdict":"pass"}

Sessions persist across commands and across your turns. Every command after
`open` takes the session id first; `record` also takes the run id, after the
session is gone.

## Commands

    ui-box doctor
    ui-box wake   [--lab NAME] [--wait SECONDS]
    ui-box open   <target> [--surface web|tauri|tui] [--viewport WxH] [--forward SPEC]
    ui-box act    <session> <step...>
    ui-box snap   <session> [--mode text|png|both|layout] [--name NAME] [--clip SELECTOR]
    ui-box eval   <session> <expr>
    ui-box close  <session>
    ui-box record <session|runid> [--format uibox|playwright] [-o FILE]
    ui-box run    [flow.yaml] [--lab NAME] [--project NAME] [--force] [--forward SPEC]
    ui-box verify --since <git-ref>
    ui-box runs
    ui-box show   <runid>

Steps: `open` `click` `type` `key` `wait_for` `assert_text` `assert_visible` `assert_absent` `snap`.
Frozen contract — a rejected command is a CLI bug, not a cue to invent a flag.
`ui-box <command> --help` for argument detail the list does not pin down.

## `localhost` is the lab's localhost

The browser runs in the lab, on the lab's loopback. Your dev server is not there.

    ui-box open http://localhost:3000 --forward 3000

| `--forward` | lab | here |
| --- | --- | --- |
| `3000` | 127.0.0.1:3000 | 127.0.0.1:3000 |
| `3000:5173` | 127.0.0.1:3000 | 127.0.0.1:5173 |
| `3000:localhost:5173` | 127.0.0.1:3000 | localhost:5173 |

First number is the one in your URL — the URL resolves inside the lab; repeat the
flag per port. A loopback target with no covering forward is refused before the
browser loads. That refusal is an unpublished port, not a broken app.

## Snapshots

| Question | `--mode` |
| --- | --- |
| is it there, what does it say | `text` (default; `both` adds a png) |
| where is it, is it findable | `layout` — text plus bounding rectangles |
| what does it look like — overlap, spacing, color | `png`, costs orders of magnitude more; `--clip SELECTOR` for one element |

| Text cannot see | Because |
| --- | --- |
| position | the tree flattens space |
| `aria-hidden` decoration | correct decoration has no accessible identity |
| what got painted | computed style is not paint |

Start "I cannot find X" and "X is too subtle" in `layout`: you cannot know the
question is visual until you look. A control can be present, labelled and
reachable, and 761px from where the user is looking. After a png, go back to text.

## Verdict

Failed regardless of exit code:

- snapshot empty, whitespace, or a bare document shell
- only an empty mount point (`<div id="root">` with nothing in it)
- nothing you changed appears in the snapshot
- `console.jsonl` has an uncaught exception, unhandled rejection, or framework error boundary trip
- `network.jsonl` shows the page's own JS or CSS returning 404 or 5xx

Absence of noise is not a pass. Assert positively, and report what you saw —
"empty root div, `TypeError: Cannot read properties of undefined` in console".

    ui-box act "$session" assert_text "text=Order confirmed"

A failed step is an answer. Do not loosen the assertion until it passes.

## Exit codes

    0       passed
    1       failed — something rendered wrong
    2       ui-box could not run: config, backend, driver
    other   off-contract (panic 101, signal 128+n) — tooling, never a verdict

**Never report 2 as a broken UI.** The app may be fine; run `ui-box doctor`.

## References

| Question | Read |
| --- | --- |
| how do I drive a session? | `references/how-do-i-drive-a-session.md` |
| how do I freeze a flow into a spec? | `references/how-do-i-freeze-a-flow.md` |
| why did it fail, where are the artifacts? | `references/why-did-it-fail.md` |
| how do I point it at another lab, port or display? | `references/how-do-i-configure-it.md` |
