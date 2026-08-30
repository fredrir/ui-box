# Freezing a flow

A session you never recorded is work you will redo.

    ui-box record "$session" -o tests/ui/checkout.yaml
    ui-box run tests/ui/checkout.yaml

Commit the result. Use `run` for regression checks and in anything automated.
Session ids and run ids are the same string, so `record` takes either — the id
`open` gave you still works after the session is gone.

| `record` | Emits |
| --- | --- |
| `--format uibox` | ui-box spec (default) |
| `--format playwright` | Playwright spec |
| `-o -` | spec on stdout, JSON summary on stderr |

## Spec format

```yaml
version: 1
flow: checkout
surface: web
target: http://host:3000
viewport: 1280x800
steps:
  - open: http://host:3000
  - click: "role=button[name=Submit]"
  - type: { selector: "css=#email", text: "a@b.c" }
  - key: Enter
  - wait_for: "text=Welcome"
  - assert_text: "text=Welcome"
  - snap: { name: after-submit, mode: text }
```

| Field | Values |
| --- | --- |
| `surface` | `web` `tauri` `tui` |
| `target` | URL, `exec:/path/to/binary`, `tui:<name>` |

A flow with no `assert_text` / `assert_visible` / `assert_absent` is rejected: it
would pass against a blank page, which makes it a transcript, not a test.

## verify

    ui-box verify --since <git-ref>

"Have the UI files changed since that ref been covered by a passing run?" A
pre-push hook may run it and reject the push; the fix is to run the UI test, not
to bypass the hook.

It is deliberately quiet — 0 when the tree has not moved and 0 when there are no
flows to run. A 0 from `verify` is not proof that a UI test ever executed.
