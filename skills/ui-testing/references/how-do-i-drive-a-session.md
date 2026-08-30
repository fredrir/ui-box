# Driving a session

The call sequence is in `SKILL.md`. A session lives `UIBOX_SESSION_TTL` seconds
(900) from its last command, keeping cookies, scroll position and typed state, so
each command can be a separate turn. Close when done — an abandoned session holds
a browser open on the lab until the TTL expires.

## Steps

| Step | Positional | YAML |
| --- | --- | --- |
| `open` | `open URL` | `- open: http://host:3000` |
| `click` | `click SELECTOR` | `- click: "role=button[name=Go]"` |
| `type` | `type SELECTOR TEXT` | `- type: { selector: "css=#email", text: "a@b.c" }` |
| `key` | `key Enter` | `- key: Enter` |
| `wait_for` | `wait_for SELECTOR` | `- wait_for: "text=Welcome"` |
| `assert_text` | `assert_text SELECTOR` | `- assert_text: "text=Welcome"` |
| `assert_visible` | `assert_visible SELECTOR` | `- assert_visible: "css=#banner"` |
| `assert_absent` | `assert_absent SELECTOR` | `- assert_absent: "css=#spinner"` |
| `snap` | `snap NAME` | `- snap: { name: after-submit, mode: text }` |

Hyphen and underscore both parse: `wait-for` == `wait_for`.

A value starting with `-` needs `--` first:

    ui-box act "$session" -- type "css=#qty" "-5"

A raw step when the positional form is awkward:

    ui-box act "$session" --yaml '{click: "css=#go"}'

Prefer `wait_for` over sleeping. A fixed delay is slower than it needs to be or
flaky, and usually both on a loaded machine.

## Selectors

Uniform across every driver.

    css=SEL     DOM (web, tauri)
    role=ROLE   DOM (web, tauri)
    text=STR    DOM and TUI
    re=REGEX    TUI, matched against the terminal buffer
    cell=R,C    TUI, absolute cell

Prefer `role=` and `text=`: they describe what a user perceives, they survive
refactors, and they double as an accessibility check — a button you cannot
address by role and name is a button a screen reader cannot address either.
Reach for `css=` only when the element has no accessible identity.

## eval

For questions a snapshot cannot answer — computed style, in-page state.

    ui-box eval "$session" "getComputedStyle(document.querySelector('#bar')).opacity"

One JSON line on stdout, detail on stderr; `-q` drops the stderr half.
Computed style is what CSS resolved to, not what reached the screen.
