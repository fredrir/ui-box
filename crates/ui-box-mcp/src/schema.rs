use std::borrow::Cow;
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{json, Value};

pub const PREPARE: &str = "ui_test_prepare";
pub const OPEN: &str = "ui_open";
pub const ACT: &str = "ui_act";
pub const SNAP: &str = "ui_snap";
pub const EVAL: &str = "ui_eval";
pub const CLOSE: &str = "ui_close";
pub const RECORD: &str = "ui_record";
pub const RUN: &str = "ui_run";
pub const RUNS: &str = "ui_runs";
pub const SHOW: &str = "ui_show";

pub const INSTRUCTIONS: &str = "\
ui-box drives a real UI that runs inside the dlab-ui lab (a NixOS VM with Xvfb, a window
manager and Playwright browsers) and reports back as text. You stay on this machine; the
UI does not.

Normal loop: call ui_test_prepare first, then ui_open to get a session, then ui_act /
ui_snap / ui_eval against that session, then ui_close. Freeze a session you like into a
committed flow file with ui_record, and replay it later with ui_run.

Snapshots are accessibility trees as text, and that is what you should read. Screenshots
are returned only when you ask for them or when something fails, because images cost far
more context than they are usually worth.

Every result separates two kinds of failure and you must not confuse them:
  ui_test_failed   the UI under test failed. ui-box worked. Go debug the application.
  uibox_unusable   ui-box itself could not run. The UI was never exercised, so nothing
                   is known about it. Go fix the tooling. Do NOT report an application
                   bug on the strength of this.
The structured content of every result carries `status` and `failure_domain` saying which.";

fn object(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(JsonObject::new()),
    }
}

fn project_dir() -> Value {
    json!({
        "type": "string",
        "description": "Directory to run ui-box from, which is how uibox.toml, .uibox/runs \
                        and relative flow paths resolve. Defaults to this server's working \
                        directory, which is normally the project root."
    })
}

fn tool(name: &'static str, description: &'static str, schema: Value, read_only: bool) -> Tool {
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = Some(read_only);
    annotations.open_world_hint = Some(true);
    Tool::new(
        Cow::Borrowed(name),
        Cow::Borrowed(description),
        object(schema),
    )
    .with_annotations(annotations)
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool(
            PREPARE,
            "Wake the lab that hosts the UI and check that ui-box can actually drive it. \
             Call this once before the first ui_open in a session of work. It returns ready, \
             or the specific reason it is not ready. Checks that only matter for `ui_run` \
             golden comparison (vision, goldens) are reported as advisory and do not block \
             the live loop. Artifact placement itself happens in ui_run, which builds and \
             copies the artifact into the lab as part of a replay.",
            json!({
                "type": "object",
                "properties": {
                    "lab": { "type": "string", "description": "Lab to wake. Defaults to the backend host from UIBOX_BACKEND." },
                    "force": { "type": "boolean", "description": "Set DLAB_FORCE=1 on the ssh backend, forcing a lab that refuses to start." },
                    "wait": { "type": "integer", "minimum": 0, "description": "Seconds to wait for the wake before moving on. Default 2." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            OPEN,
            "Open a live session against a target and keep the driver running. Returns a \
             session id used by ui_act, ui_snap, ui_eval and ui_close, and by default also \
             returns the initial accessibility snapshot so you can see what rendered without \
             a second call. An empty initial snapshot is reported as a failure, not a pass.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "http://host:3000, exec:/path/to/bin or tui:name. Defaults to UIBOX_TARGET or target in uibox.toml." },
                    "surface": { "type": "string", "enum": ["web", "tauri", "tui"], "description": "Defaults to web." },
                    "viewport": { "type": "string", "pattern": "^[0-9]+x[0-9]+$", "description": "WxH, for example 1280x800." },
                    "flow": { "type": "string", "description": "Flow name recorded in meta.json, which ui_record then reuses." },
                    "snapshot": { "type": "boolean", "description": "Take an accessibility snapshot right after opening. Default true." },
                    "max_chars": { "type": "integer", "minimum": 200, "description": "Cap on inlined snapshot text. Default 20000." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            ACT,
            "Send one step, or a list of steps, to a live session. Give either the flat form \
             (action plus its argument) or `steps` for a batch; a batch stops at the first \
             step that fails. When a step fails, an accessibility snapshot, a screenshot and \
             any console errors or failed network requests are collected automatically and \
             returned with the failure, because those are what explain it.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id from ui_open." },
                    "action": {
                        "type": "string",
                        "enum": ["open", "click", "type", "key", "wait_for", "assert_text", "snap"],
                        "description": "The step verb. Ignored when `steps` is given."
                    },
                    "selector": { "type": "string", "description": "Selector for click, type, wait_for and assert_text. Grammar: css=SEL, role=ROLE, text=STR (all surfaces), re=REGEX and cell=R,C (tui only)." },
                    "text": { "type": "string", "description": "Text to type, for action=type." },
                    "target": { "type": "string", "description": "Target for action=open." },
                    "key": { "type": "string", "description": "Key for action=key, for example Enter." },
                    "name": { "type": "string", "description": "Snapshot name for action=snap." },
                    "steps": {
                        "type": "array",
                        "description": "A batch of steps, run in order until one fails. Each element is either the flat form {\"action\":\"click\",\"selector\":\"...\"} or a raw step node {\"click\":\"role=button[name=Submit]\"}.",
                        "items": { "type": "object" }
                    },
                    "snapshot_on_failure": { "type": "boolean", "description": "Collect a snapshot and screenshot when a step fails. Default true." },
                    "max_chars": { "type": "integer", "minimum": 200, "description": "Cap on inlined snapshot text. Default 20000." },
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            SNAP,
            "Snapshot a live session. Returns the accessibility tree as text, which is what \
             you should read, plus any console errors and failed network requests recorded \
             since the last call. Ask for mode png or both, or set include_image, to also get \
             the screenshot back as an image. An empty snapshot is reported as a failure.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id from ui_open." },
                    "name": { "type": "string", "description": "Name for the snapshot file under <run>/snaps/." },
                    "mode": { "type": "string", "enum": ["text", "png", "both"], "description": "Default text." },
                    "include_image": { "type": "boolean", "description": "Return the screenshot as an image block. Promotes mode text to both." },
                    "max_chars": { "type": "integer", "minimum": 200, "description": "Cap on inlined snapshot text. Default 20000." },
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            EVAL,
            "Evaluate an expression inside a live session and return its value. Use this for \
             the things a snapshot cannot tell you, such as application state or a computed \
             style. It does not record a step, so it never appears in a recorded flow.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id from ui_open." },
                    "expr": { "type": "string", "description": "Expression evaluated by the driver for this surface." },
                    "project_dir": project_dir()
                },
                "required": ["session", "expr"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            CLOSE,
            "Close a live session and release its driver. The run directory survives, so the \
             session can still be recorded with ui_record or inspected with ui_show. Returns \
             the run's verdict and how many steps failed.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id from ui_open." },
                    "keep_channel": { "type": "boolean", "description": "Keep the driver channel directory and its log for debugging." },
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RECORD,
            "Freeze a session you explored into a replayable flow file. Every step that landed \
             was already appended as it happened, so this works on a session that is still open \
             as well as on a finished run. Write it into the repository and ui_run replays it.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session id, or the run id they share." },
                    "format": { "type": "string", "enum": ["uibox", "playwright"], "description": "Default uibox, the step format ui_run replays. playwright emits a spec file instead." },
                    "out": { "type": "string", "description": "Where to write it, for example flows/checkout.yaml. Defaults to flow.yaml inside the run directory. Use - to get the file contents back in the result instead." },
                    "flow": { "type": "string", "description": "Flow name to record. Defaults to the name given at ui_open." },
                    "target": { "type": "string", "description": "Override the target recorded in the flow." },
                    "project_dir": project_dir()
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RUN,
            "Replay a saved flow end to end and return its verdict. With an artifact this also \
             builds and places it into the target lab before replaying, which is the full \
             check of a change. On failure it reports the step it halted at together with the \
             last snapshot, console errors and failed network requests.",
            json!({
                "type": "object",
                "properties": {
                    "flow": { "type": "string", "description": "Path to a step-format yaml file, for example flows/checkout.yaml." },
                    "lab": { "type": "string", "description": "Build lab holding the checkout under test." },
                    "project": { "type": "string" },
                    "build": { "type": "string", "description": "Build command to run in the build lab." },
                    "artifact": { "type": "string", "description": "Artifact path to place into the target lab. Without it the flow replays against the target as it stands." },
                    "source": { "type": "string", "description": "Local tree synced into the build lab. Defaults to the project root, which is what you want when testing uncommitted work." },
                    "lab_checkout": { "type": "boolean", "description": "Build from the lab's own checkout instead of syncing a local tree." },
                    "target_lab": { "type": "string", "description": "Lab the artifact is placed into. Defaults to the backend host." },
                    "surface": { "type": "string", "enum": ["web", "tauri", "tui"] },
                    "target": { "type": "string", "description": "Override the flow's target." },
                    "viewport": { "type": "string", "pattern": "^[0-9]+x[0-9]+$" },
                    "no_place": { "type": "boolean", "description": "Skip the pipeline and replay against the target as it stands." },
                    "keep_going": { "type": "boolean", "description": "Keep running after a failing step instead of halting." },
                    "force": { "type": "boolean", "description": "Set DLAB_FORCE=1 on the ssh backend." },
                    "include_image": { "type": "boolean", "description": "Return the last screenshot the flow took as an image block." },
                    "max_chars": { "type": "integer", "minimum": 200, "description": "Cap on inlined snapshot text. Default 20000." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RUNS,
            "List recorded runs, newest first, with each run's verdict and how many steps \
             failed. Use it to find the run id that ui_show or ui_record needs.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "description": "How many runs to list. Default 20." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            SHOW,
            "Show one recorded run: its provenance and verdict, the steps that landed, the \
             golden comparison report, and the snapshot files. Console errors and failed \
             network requests recorded during the run are returned alongside.",
            json!({
                "type": "object",
                "properties": {
                    "run": { "type": "string", "description": "Run id, as listed by ui_runs." },
                    "what": { "type": "string", "enum": ["meta", "steps", "report", "snaps", "all"], "description": "Default meta." },
                    "max_chars": { "type": "integer", "minimum": 200, "description": "Cap on inlined snapshot text. Default 20000." },
                    "project_dir": project_dir()
                },
                "required": ["run"],
                "additionalProperties": false
            }),
            true,
        ),
    ]
}
