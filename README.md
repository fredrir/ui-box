# ui-box

ui-box opens a web page and uses it the way a person would. It clicks buttons. It types into boxes.

After each step it describes what is on the screen in words: the headings, the buttons, the text. It takes a screenshot when you ask for one, and when something goes wrong. It compares those screenshots against ones you have approved, and tells you where they differ.

The browser runs on another Linux machine over ssh, or on the machine you are already on. When it runs elsewhere, `localhost` in an address means that machine, not this one, so ui-box can publish a port across for it.

A session can be saved to a file and replayed later.

Coding assistants that speak MCP get ui-box as a set of tools.

## Installing

```
curl -fsSL https://raw.githubusercontent.com/fredrir/ui-box/main/install.sh | sh
```

`UIBOX_VERSION` installs a chosen release instead of the latest. `UIBOX_INSTALL_DIR` puts the binaries somewhere other than `~/.local/bin`.

## Settings

ui-box reads these from the environment or from a `.env` file.

| variable | what it does |
|---|---|
| `UIBOX_BACKEND` | where tests actually run: a machine over ssh, or the local machine |
| `UIBOX_DISPLAY` | screen size and colour depth of the virtual display, default `1280x800x24` |
| `UIBOX_ARTIFACTS` | where results are written, default `.uibox/runs` |
| `UIBOX_GOLDENS` | the store of approved screenshots to compare against |
| `UIBOX_SESSION_TTL` | seconds an idle session stays alive, default `900` |
| `UIBOX_RPC_TIMEOUT` | seconds to wait on the browser before giving up, default `30` |
| `UIBOX_HOME` | where ui-box looks for its global `.env` |
| `UIBOX_COPY_VIA` | a machine to route file transfers through, when the two ends cannot reach each other |
| `UIBOX_FORWARD` | publishes a port on this machine into the lab, so the browser there can reach it |
| `UIBOX_SSH_OPTS` | replaces the default ssh options, rather than adding to them |
