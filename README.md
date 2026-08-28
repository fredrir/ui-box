# ui-box

Your coding assistant changes something visual and tells you the change works. It has not looked. It cannot look. So you check by hand, every time.

ui-box gives it eyes. It can open a page, click things, type into them, and see what actually happened, then tell you with evidence you can check yourself.

## Where the looking happens

The browser does not run on your laptop. It runs on a separate Linux machine that exists only for this, with a fixed screen size and a fixed set of fonts that never change. Your assistant stays where it already is and drives that machine remotely. Results come back to you.

Three things follow from that, and they are the reasons to want it:

- Your laptop stays clean. No browser installs, no windows opening while you work, nothing stealing focus.
- Screenshots are comparable over time. The fonts and the browser are pinned and identical on every run, so a screenshot that differs from last week's differs because your interface changed, not because something on your machine updated.
- The assistant that wrote the change is the one that checks it, in the same conversation. Nothing is handed off.

That machine is any Linux box you can reach over ssh, provided it has a browser, a virtual display for the browser to draw into, and ui-box's own driver program. There is no provisioning command. The project ships a NixOS module that sets up all three, and that is how the author's own machine is built, but it is one way rather than the required way.

## What comes back

Text, by default: a short description of the headings, buttons and text a person would see on screen. That is cheap, and it is usually enough. Screenshots come when they are asked for, or when something fails.

## Saving a session as a test

When the assistant has poked around and found the sequence that matters, that exploration can be frozen into a file and replayed later. The thing it just checked by hand becomes something checked automatically from then on.

## Installing

```
curl -fsSL https://raw.githubusercontent.com/fredrir/ui-box/main/install.sh | sh
```

Two settings change what that command does. `UIBOX_VERSION` picks a release instead of the latest. `UIBOX_INSTALL_DIR` picks where the binaries land, instead of `~/.local/bin`.

You do not need that Linux machine to start. `UIBOX_BACKEND` also accepts the local one, and then everything runs where you already are, with no setup, as long as you have a browser. What you give up is the clean laptop and the comparable screenshots. Those now depend on whatever fonts and browser version your machine happens to have.

Any assistant that can run a command can use ui-box from there. Assistants that speak MCP (Claude Code, Codex, opencode) get it as a set of tools instead.

## Configuration

Settings are read from the environment or from a `.env` file.

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
| `UIBOX_SSH_OPTS` | extra options passed to ssh |

## What it does not do yet

It drives web pages. Desktop application windows and terminal interfaces are planned, and not finished.
