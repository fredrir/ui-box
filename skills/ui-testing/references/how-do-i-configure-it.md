# Configuration

Precedence: CLI flag > environment > project `uibox.toml` > global `.env`.
Every config key is also an environment variable named `UIBOX_<KEY>`.

| Env | Default |
| --- | --- |
| `UIBOX_BACKEND` | (unset) — `ssh://user@ui-box-backend`, `local://` |
| `UIBOX_DISPLAY` | `1280x800x24` |
| `UIBOX_ARTIFACTS` | `.uibox/runs` |
| `UIBOX_GOLDENS` | (unset) |
| `UIBOX_SESSION_TTL` | `900` |
| `UIBOX_RPC_TIMEOUT` | `30` |
| `UIBOX_FORWARD` | (unset) |
| `UIBOX_HOME` | where the global `.env` is read from |

Global flags work on every verb and beat the environment:

    --backend  --display  --artifacts  --goldens  --session-ttl  --force  -q/--quiet

## The lab suspends when idle

The first command after a quiet period pays the restore cost.

    ui-box wake

Fire-and-forget: it returns as soon as the lab is reachable or the wake is in
flight, and it never fails a caller. Run it when you begin editing UI files, not
when you are ready to test. Some harnesses fire it automatically on edit; running
it yourself is harmless either way.
