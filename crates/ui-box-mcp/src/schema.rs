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
pub const VERIFY: &str = "ui_verify";
pub const RUNS: &str = "ui_runs";
pub const SHOW: &str = "ui_show";

pub const INSTRUCTIONS: &str = "\
ui-box drives a real UI inside the dlab-ui lab (a NixOS VM with Xvfb and Playwright) and
reports back as text. You stay here; the UI does not.

`localhost` in a target is the LAB's loopback. A dev server you started here is not on
it until a forward publishes it: `forward: [\"3000:5173\"]` on ui_open or ui_run. The
first number is the one in your URL, because the URL resolves inside the lab. A loopback
target with no covering forward is REFUSED -- that refusal is not an application bug.

Loop: ui_test_prepare, ui_open, then ui_act / ui_snap / ui_eval, then ui_close.
ui_record freezes a session into a flow file, ui_run replays it, ui_verify is the gate
the Stop hook runs. project_dir defaults to this server's cwd, which is where uibox.toml,
.uibox/runs and relative flow paths resolve.

Snapshots are accessibility trees as text, and that is what to read. Screenshots come
back only on request or on failure.

Every result carries `status` and `failure_domain`:
  ui_test_failed   the UI failed; ui-box worked. Debug the application.
  uibox_unusable   ui-box could not run. The UI was never exercised, so nothing is known
                   about it. Do NOT report an application bug on the strength of this.";

fn object(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(JsonObject::new()),
    }
}

fn merge(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            base.extend(extra);
            Value::Object(base)
        }
        (base, _) => base,
    }
}

fn project_dir() -> Value {
    json!({ "type": "string" })
}

fn max_chars() -> Value {
    json!({ "type": "integer", "minimum": 200 })
}

fn forward() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Publish a local port into the lab, lab port first: \"3000\", \"3000:5173\", \"3000:HOST:5173\"."
    })
}

fn pipeline() -> Value {
    json!({
        "lab": { "type": "string", "description": "Build lab holding the checkout." },
        "project": { "type": "string" },
        "build": { "type": "string", "description": "Command run in the build lab." },
        "artifact": { "type": "string" },
        "source": { "type": "string", "description": "Synced into the build lab. Default: project root." },
        "lab_checkout": { "type": "boolean", "description": "Build the lab's checkout, not a synced tree." },
        "target_lab": { "type": "string", "description": "Default: the backend host." },
        "no_place": { "type": "boolean", "description": "Skip build and place; replay as-is." },
        "keep_going": { "type": "boolean" },
        "force": { "type": "boolean", "description": "DLAB_FORCE=1 on the ssh backend." },
        "project_dir": project_dir()
    })
}

fn tool(name: &'static str, description: &'static str, schema: Value, read_only: bool) -> Tool {
    let tool = Tool::new(
        Cow::Borrowed(name),
        Cow::Borrowed(description),
        object(schema),
    );
    match read_only {
        true => tool.with_annotations(ToolAnnotations::new().read_only(true)),
        false => tool,
    }
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool(
            PREPARE,
            "Wake the lab and check ui-box can drive it. Call once before the first ui_open. \
             Advisory failures (golden store) do not block.",
            json!({
                "type": "object",
                "properties": {
                    "lab": { "type": "string", "description": "Defaults to UIBOX_BACKEND." },
                    "force": { "type": "boolean", "description": "Forces a lab that refuses to start (DLAB_FORCE=1)." },
                    "wait": { "type": "integer", "minimum": 0, "description": "Seconds. Default 2." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            OPEN,
            "Open a live session. Returns its id and the initial accessibility snapshot. \
             An empty snapshot is a failure, never a pass.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "http://host:3000, exec:/path/to/bin or tui:name. Default from UIBOX_TARGET or uibox.toml." },
                    "surface": { "type": "string", "enum": ["web", "tauri", "tui"], "description": "Default web." },
                    "viewport": { "type": "string", "pattern": "^[0-9]+x[0-9]+$" },
                    "flow": { "type": "string", "description": "Flow name ui_record reuses." },
                    "forward": forward(),
                    "snapshot": { "type": "boolean" },
                    "max_chars": max_chars(),
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            ACT,
            "One step from the flat fields, or `steps` for a batch that halts at the first \
             failure.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "action": {
                        "type": "string",
                        "enum": ["open", "click", "type", "key", "wait_for", "assert_text", "assert_absent", "assert_visible", "snap"],
                        "description": "By verb: click/wait_for/assert_* -> selector; type -> selector+text; key -> key; open -> target; snap -> name. Ignored with `steps`."
                    },
                    "selector": { "type": "string", "description": "css=SEL, role=ROLE, text=STR; tui also re=REGEX and cell=R,C." },
                    "text": { "type": "string" },
                    "target": { "type": "string" },
                    "key": { "type": "string" },
                    "name": { "type": "string" },
                    "steps": {
                        "type": "array",
                        "description": "Flat [{\"action\":\"click\",\"selector\":\"...\"}], or raw one-verb nodes [{\"click\":\"role=button[name=Submit]\"}].",
                        "items": { "type": "object" }
                    },
                    "snapshot_on_failure": { "type": "boolean" },
                    "max_chars": max_chars(),
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            SNAP,
            "Accessibility tree as text, with console errors and failed requests since the \
             last call. An empty snapshot is a failure, never a pass.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "name": { "type": "string" },
                    "mode": { "type": "string", "enum": ["text", "png", "both", "layout"], "description": "Default text. layout is text plus bounding boxes." },
                    "clip": { "type": "string", "description": "Crop the png to an element, e.g. css=#chart. Needs mode png or both." },
                    "include_image": { "type": "boolean", "description": "Inline the screenshot; promotes mode text to both." },
                    "max_chars": max_chars(),
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            EVAL,
            "Evaluate an expression in a live session, for what the accessibility tree cannot \
             show. Records no step, so it never enters a flow.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "expr": { "type": "string", "description": "JavaScript on web and tauri." },
                    "project_dir": project_dir()
                },
                "required": ["session", "expr"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            CLOSE,
            "Close a live session. The run survives for ui_record and ui_show.",
            json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "keep_channel": { "type": "boolean", "description": "Keeps the driver channel dir and its log." },
                    "project_dir": project_dir()
                },
                "required": ["session"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RECORD,
            "Freeze a session into a flow file. Steps were appended as they landed, so an \
             open session records as well as a finished run.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session id or run id." },
                    "format": { "type": "string", "enum": ["uibox", "playwright"], "description": "Default uibox, what ui_run replays; playwright emits a spec file." },
                    "out": { "type": "string", "description": "flows/checkout.yaml. Default: flow.yaml in the run dir. `-` returns contents." },
                    "flow": { "type": "string", "description": "Defaults to the ui_open name." },
                    "target": { "type": "string", "description": "Override the recorded target." },
                    "project_dir": project_dir()
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RUN,
            "Replay a saved flow and return its verdict. With `artifact` it first builds and \
             places it into the target lab: the full check of a change.",
            json!({
                "type": "object",
                "properties": merge(pipeline(), json!({
                    "flow": { "type": "string", "description": "e.g. flows/checkout.yaml." },
                    "forward": forward(),
                    "surface": { "type": "string", "enum": ["web", "tauri", "tui"] },
                    "target": { "type": "string", "description": "Override the flow's target." },
                    "viewport": { "type": "string", "pattern": "^[0-9]+x[0-9]+$" },
                    "include_image": { "type": "boolean" },
                    "max_chars": max_chars()
                })),
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            VERIFY,
            "Replay every committed flow and diff its screenshots against the approved \
             goldens.\n\
             READ THE STATUS, NOT THE EXIT: with no flow files, or a `since` the tree has not \
             moved past, verify succeeds having run nothing -- status `nothing_verified`, which \
             proves nothing. Only `passed` means flows ran and matched.",
            json!({
                "type": "object",
                "properties": merge(pipeline(), json!({
                    "since": { "type": "string", "description": "Git ref. Skip unless the tree moved past it; a skip is nothing_verified, not a pass." },
                    "flows": { "type": "string", "description": "Default flows/." },
                    "update_goldens": { "type": "boolean", "description": "Approve every candidate as the new golden. REWRITES the store and passes by definition -- only after reading the diffs." },
                    "golden_prefix": { "type": "string", "description": "Default project/flow." }
                })),
                "additionalProperties": false
            }),
            false,
        ),
        tool(
            RUNS,
            "Recorded runs, newest first, with verdict and failed step count.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "description": "Default 20." },
                    "project_dir": project_dir()
                },
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            SHOW,
            "One recorded run: provenance, verdict, steps, golden report, snapshots, console \
             and network errors.",
            json!({
                "type": "object",
                "properties": {
                    "run": { "type": "string" },
                    "what": { "type": "string", "enum": ["meta", "steps", "report", "snaps", "all"], "description": "Default meta." },
                    "max_chars": max_chars(),
                    "project_dir": project_dir()
                },
                "required": ["run"],
                "additionalProperties": false
            }),
            true,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROZEN: &[(&str, &[&str])] = &[
        (PREPARE, &["force", "lab", "project_dir", "wait"]),
        (
            OPEN,
            &[
                "flow",
                "forward",
                "max_chars",
                "project_dir",
                "snapshot",
                "surface",
                "target",
                "viewport",
            ],
        ),
        (
            ACT,
            &[
                "action",
                "key",
                "max_chars",
                "name",
                "project_dir",
                "selector",
                "session",
                "snapshot_on_failure",
                "steps",
                "target",
                "text",
            ],
        ),
        (
            SNAP,
            &[
                "clip",
                "include_image",
                "max_chars",
                "mode",
                "name",
                "project_dir",
                "session",
            ],
        ),
        (EVAL, &["expr", "project_dir", "session"]),
        (CLOSE, &["keep_channel", "project_dir", "session"]),
        (
            RECORD,
            &["flow", "format", "id", "out", "project_dir", "target"],
        ),
        (
            RUN,
            &[
                "artifact",
                "build",
                "flow",
                "force",
                "forward",
                "include_image",
                "keep_going",
                "lab",
                "lab_checkout",
                "max_chars",
                "no_place",
                "project",
                "project_dir",
                "source",
                "surface",
                "target",
                "target_lab",
                "viewport",
            ],
        ),
        (
            VERIFY,
            &[
                "artifact",
                "build",
                "flows",
                "force",
                "golden_prefix",
                "keep_going",
                "lab",
                "lab_checkout",
                "no_place",
                "project",
                "project_dir",
                "since",
                "source",
                "target_lab",
                "update_goldens",
            ],
        ),
        (RUNS, &["limit", "project_dir"]),
        (SHOW, &["max_chars", "project_dir", "run", "what"]),
    ];

    #[test]
    fn the_frozen_tool_and_parameter_names_are_still_what_contracts_says() {
        let tools = tools();
        assert_eq!(tools.len(), FROZEN.len());
        for (tool, (name, parameters)) in tools.iter().zip(FROZEN) {
            assert_eq!(&tool.name, name);
            let properties = tool.input_schema["properties"]
                .as_object()
                .expect("properties is an object");
            let found: Vec<&str> = properties.keys().map(String::as_str).collect();
            assert_eq!(&found, parameters, "{name} parameters moved");
        }
    }
}
