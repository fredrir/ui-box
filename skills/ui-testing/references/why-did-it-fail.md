# Why did it fail

    ui-box runs           recent runs
    ui-box show <runid>   verdict and what failed

1. `ui-box show <runid>` → which step failed.
2. `steps.yaml` → what that step was. Appended as each step lands, not written at
   the end, so it is trustworthy even if the run died mid-flight.
3. `snaps/` → the last text snapshot before the failure. Usually enough.
4. `console.jsonl` → the exception, if there was one.

## Run directory

`UIBOX_ARTIFACTS`, default `.uibox/runs`.

    <artifacts>/<runid>/
      meta.json       provenance + verdict
      steps.yaml      appended as each step lands
      console.jsonl   browser console output
      network.jsonl   requests and status codes
      snaps/<name>.{png,txt}
      diff/<name>.png
      report.json

| `meta.json` | Carries |
| --- | --- |
| `verdict` `steps_total` `steps_failed` | the outcome |
| `git_sha` `diff_hash` `artifact_hash` | exactly which code was under test |

## Exit 2 is ui-box, not the app

    ui-box doctor

Exits 2 when the setup is unhealthy; its JSON names the failed check. The lab
would not start, the driver is missing, the config is wrong — the app may be
perfectly fine. Reporting "the UI is broken" on a 2 sends everyone hunting a bug
that does not exist. Fix the setup and test again.

## The lab refuses to start

The error is passed through verbatim. Read it rather than retrying — it usually
says exactly what is wrong, and `run --force` exists for the case where it is
asking you to override.
