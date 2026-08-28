use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::diagnostics::{self, Cursor, Events, DEFAULT_MAX_CHARS};
use crate::outcome::{Domain, Report, BLANK_BANNER};
use crate::schema;
use crate::uibox::{Invocation, Landing, UiBox};

const BLOCKING_CHECKS: &[&str] = &["config", "artifacts", "backend", "driver.dom", "sessions"];

#[derive(Default)]
struct Memory {
    run_dirs: HashMap<String, PathBuf>,
    cursors: HashMap<String, Cursor>,
}

pub struct UiBoxServer {
    uibox: UiBox,
    memory: Mutex<Memory>,
}

impl Default for UiBoxServer {
    fn default() -> Self {
        UiBoxServer::new()
    }
}

impl UiBoxServer {
    pub fn new() -> UiBoxServer {
        UiBoxServer {
            uibox: UiBox::discover(),
            memory: Mutex::new(Memory::default()),
        }
    }

    pub fn uibox_location(&self) -> Result<&Path, &str> {
        self.uibox.location()
    }

    fn remember(&self, id: &str, dir: PathBuf) {
        if let Ok(mut memory) = self.memory.lock() {
            memory.run_dirs.insert(id.to_string(), dir);
        }
    }

    fn recall(&self, id: &str) -> Option<PathBuf> {
        self.memory.lock().ok()?.run_dirs.get(id).cloned()
    }

    fn cursor(&self, id: &str) -> Cursor {
        self.memory
            .lock()
            .ok()
            .and_then(|memory| memory.cursors.get(id).copied())
            .unwrap_or_default()
    }

    fn advance(&self, id: &str, cursor: Cursor) {
        if let Ok(mut memory) = self.memory.lock() {
            memory.cursors.insert(id.to_string(), cursor);
        }
    }

    async fn run_dir(&self, id: &str, cwd: Option<&Path>) -> Option<PathBuf> {
        if let Some(dir) = self.recall(id) {
            return Some(dir);
        }
        let shown = self
            .uibox
            .call(
                vec![
                    "show".to_string(),
                    id.to_string(),
                    "--what".to_string(),
                    "meta".to_string(),
                ],
                cwd,
            )
            .await;
        let dir = shown.text("run_dir").map(PathBuf::from)?;
        self.remember(id, dir.clone());
        Some(dir)
    }

    async fn fresh_events(&self, id: &str, cwd: Option<&Path>, from_start: bool) -> Events {
        let Some(dir) = self.run_dir(id, cwd).await else {
            return Events::default();
        };
        let from = if from_start {
            Cursor::default()
        } else {
            self.cursor(id)
        };
        let events = diagnostics::events_since(&dir, from).await;
        self.advance(id, events.cursor);
        events
    }

    async fn capture(&self, request: CaptureRequest<'_>) -> Capture {
        let mut argv = vec![
            "snap".to_string(),
            request.session.to_string(),
            "--mode".to_string(),
            request.mode.to_string(),
        ];
        if let Some(name) = request.name {
            argv.push("--name".to_string());
            argv.push(name.to_string());
        }

        let invocation = self.uibox.call(argv, request.cwd).await;
        let snap = invocation.field("snap").unwrap_or(Value::Null);
        let (text_path, png_path) = diagnostics::snap_paths(&snap);

        let mut text = None;
        let mut truncated = 0;
        if let Some(path) = &text_path {
            if let Some((body, dropped)) = diagnostics::snapshot_text(path, request.max_chars).await
            {
                text = Some(body);
                truncated = dropped;
            }
        }

        let wants_text = request.mode != "png";
        let blank = wants_text
            && text
                .as_deref()
                .map(|body| body.trim().is_empty())
                .unwrap_or(true);

        let mut png = None;
        let mut png_note = None;
        if request.want_image {
            match &png_path {
                Some(path) => match diagnostics::snapshot_png(path).await {
                    Ok(bytes) => png = Some(bytes),
                    Err(reason) => png_note = Some(reason),
                },
                None => png_note = Some("the driver returned no screenshot".to_string()),
            }
        }

        let events = match invocation.text("run") {
            Some(run) => self.fresh_events(&run, request.cwd, false).await,
            None => Events::default(),
        };

        Capture {
            invocation,
            snap,
            text,
            truncated,
            text_path,
            png_path,
            png,
            png_note,
            blank,
            events,
        }
    }

    async fn prepare(&self, args: PrepareArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();

        let mut wake = vec!["wake".to_string()];
        if let Some(lab) = &args.lab {
            wake.push("--lab".to_string());
            wake.push(lab.clone());
        }
        if let Some(wait) = args.wait {
            wake.push("--wait".to_string());
            wake.push(wait.to_string());
        }
        if args.force {
            wake.push("--force".to_string());
        }
        let woken = self.uibox.call(wake, cwd).await;

        if let Landing::NotStarted(reason) = &woken.landing {
            let mut report = Report::new("ui-box is not installed where this server can reach it.");
            report
                .fact("command", woken.command_line())
                .failed(Domain::Tooling, reason.clone());
            return report.build();
        }

        let lab = woken
            .text("lab")
            .unwrap_or_else(|| "the backend host".to_string());
        let state = woken.text("state").unwrap_or_else(|| "unknown".to_string());

        let mut doctor = vec!["doctor".to_string()];
        if args.force {
            doctor.push("--force".to_string());
        }
        let checked = self.uibox.call(doctor, cwd).await;

        let mut report = Report::new(format!("lab {lab} is {state}."));
        report
            .fact("lab", lab.clone())
            .fact("lab_state", state)
            .fact("command", checked.command_line());
        if let Some(detail) = woken.text("detail") {
            report.line(format!("wake reported: {detail}"));
        }
        facts_pick(
            &mut report,
            &checked.summary(),
            &["backend", "artifacts", "display", "session_ttl"],
        );

        let Some(checks) = checked
            .field("checks")
            .and_then(|value| value.as_array().cloned())
        else {
            report.absorb(&checked);
            return report.build();
        };

        let mut blocking = Vec::new();
        let mut advisory = Vec::new();
        let mut passing = Vec::new();
        for check in &checks {
            let name = check.get("name").and_then(Value::as_str).unwrap_or("?");
            let ok = check.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let detail = check.get("detail").and_then(Value::as_str).unwrap_or("");
            let line = format!("{name}: {detail}");
            if ok {
                passing.push(line);
            } else if BLOCKING_CHECKS.contains(&name) {
                blocking.push(line);
            } else {
                advisory.push(line);
            }
        }

        report.fact("checks", Value::Array(checks.clone()));
        report.block("ready", passing.join("\n"));
        if !advisory.is_empty() {
            report.block(
                "not available, and not needed for the live loop",
                advisory.join("\n"),
            );
        }

        if blocking.is_empty() {
            report
                .line("Ready. Open a session with ui_open, then drive it with ui_act and ui_snap.");
        } else {
            report.block("blocking", blocking.join("\n"));
            report.failed(
                Domain::Tooling,
                format!(
                    "ui-box cannot drive a UI yet: {}",
                    blocking
                        .iter()
                        .map(|line| line.split(':').next().unwrap_or(line))
                        .collect::<Vec<&str>>()
                        .join(", ")
                ),
            );
            if let Some(stderr) = checked.stderr_verbatim() {
                report.block("ui-box stderr, verbatim", stderr);
            }
        }
        report.build()
    }

    async fn open(&self, args: OpenArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let mut argv = vec!["open".to_string()];
        if let Some(target) = &args.target {
            argv.push(target.clone());
        }
        push_option(&mut argv, "--surface", args.surface.as_deref());
        push_option(&mut argv, "--viewport", args.viewport.as_deref());
        push_option(&mut argv, "--flow", args.flow.as_deref());

        let opened = self.uibox.call(argv, cwd).await;
        let session = opened.text("session");
        if let (Some(session), Some(dir)) = (&session, opened.text("run_dir")) {
            self.remember(session, PathBuf::from(dir));
        }

        let mut report = Report::new(match &session {
            Some(session) => format!(
                "session {session} is open on {} at {}.",
                opened.text("surface").unwrap_or_else(|| "web".to_string()),
                opened
                    .text("target")
                    .unwrap_or_else(|| "the target".to_string())
            ),
            None => "ui-box did not return a session.".to_string(),
        });
        report.absorb(&opened);

        let Some(session) = session else {
            return report.build();
        };
        if report.is_failed() || args.snapshot == Some(false) {
            return report.build();
        }

        let capture = self
            .capture(CaptureRequest {
                session: &session,
                mode: "text",
                name: Some("open"),
                max_chars: args.max_chars.unwrap_or(DEFAULT_MAX_CHARS),
                want_image: false,
                cwd,
            })
            .await;
        merge_capture(&mut report, &capture, "initial snapshot");
        report.build()
    }

    async fn act(&self, args: ActArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let steps = match args.steps() {
            Ok(steps) => steps,
            Err(reason) => return Report::invalid(reason),
        };

        let mut report = Report::new(String::new());
        let mut landed = Vec::new();
        let mut failure = None;

        for step in &steps {
            let encoded = step.to_string();
            let invocation = self
                .uibox
                .call(
                    vec![
                        "act".to_string(),
                        args.session.clone(),
                        "--yaml".to_string(),
                        encoded.clone(),
                    ],
                    cwd,
                )
                .await;

            let label = invocation.text("step").unwrap_or_else(|| encoded.clone());
            let step_ok = invocation.field("step_ok").and_then(|v| v.as_bool());
            let ran = invocation.landing.passed() && step_ok != Some(false);

            if ran {
                landed.push(format!("ok   {label}"));
                continue;
            }

            let detail = invocation
                .error_message()
                .unwrap_or_else(|| "no reason given".to_string());
            landed.push(format!("fail {label}: {detail}"));
            failure = Some(invocation);
            break;
        }

        report.fact("steps_requested", steps.len());
        report.fact("steps_landed", landed.len());
        report.block("steps", landed.join("\n"));

        let Some(invocation) = failure else {
            report.headline(format!(
                "all {} step(s) landed on session {}.",
                steps.len(),
                args.session
            ));
            let events = self.fresh_events(&args.session, cwd, false).await;
            merge_events(&mut report, &events);
            return report.build();
        };

        report.headline(format!(
            "step {} of {} failed on session {}.",
            landed.len(),
            steps.len(),
            args.session
        ));
        report.absorb(&invocation);
        if args.snapshot_on_failure == Some(false) || !invocation.landing.is_step_failure() {
            return report.build();
        }

        let capture = self
            .capture(CaptureRequest {
                session: &args.session,
                mode: "both",
                name: Some("failed"),
                max_chars: args.max_chars.unwrap_or(DEFAULT_MAX_CHARS),
                want_image: true,
                cwd,
            })
            .await;
        merge_capture(&mut report, &capture, "state when the step failed");
        report.build()
    }

    async fn snap(&self, args: SnapArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let requested = args.mode.as_deref().unwrap_or("text");
        let mode = match (requested, args.include_image) {
            ("text", Some(true)) => "both",
            (other, _) => other,
        };
        let want_image = mode != "text" && args.include_image != Some(false);

        let capture = self
            .capture(CaptureRequest {
                session: &args.session,
                mode,
                name: args.name.as_deref(),
                max_chars: args.max_chars.unwrap_or(DEFAULT_MAX_CHARS),
                want_image,
                cwd,
            })
            .await;

        let mut report = Report::new(format!("snapshot of session {}.", args.session));
        report.absorb(&capture.invocation);
        merge_capture(&mut report, &capture, "accessibility snapshot");
        report.build()
    }

    async fn eval(&self, args: EvalArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let invocation = self
            .uibox
            .call(
                vec!["eval".to_string(), args.session.clone(), args.expr.clone()],
                cwd,
            )
            .await;

        let mut report = Report::new(format!(
            "evaluated {:?} in session {}.",
            args.expr, args.session
        ));
        report.absorb(&invocation);
        if let Some(value) = invocation.field("value") {
            report.block(
                "value",
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            );
        }
        report.build()
    }

    async fn close(&self, args: CloseArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let mut argv = vec!["close".to_string(), args.session.clone()];
        if args.keep_channel {
            argv.push("--keep-channel".to_string());
        }
        let invocation = self.uibox.call(argv, cwd).await;

        let verdict = invocation
            .text("verdict")
            .unwrap_or_else(|| "unknown".to_string());
        let failed = invocation
            .field("steps_failed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total = invocation
            .field("steps_total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mut report = Report::new(match invocation.text("verdict") {
            Some(_) => format!(
                "session {} closed with verdict {verdict}, {failed} of {total} steps failed.",
                args.session
            ),
            None => format!("session {} was not closed.", args.session),
        });
        report.absorb(&invocation);
        if let Some(dir) = invocation.text("run_dir") {
            report.line(format!(
                "The run survives in {dir}. Freeze it into a flow with ui_record, \
                 or read it back with ui_show."
            ));
        }
        if !report.is_failed() && (verdict == "fail" || failed > 0) {
            report.failed(
                Domain::UnderTest,
                format!(
                    "the session closed cleanly, but {failed} of {total} steps failed while it ran"
                ),
            );
        }
        report.build()
    }

    async fn record(&self, args: RecordArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let mut argv = vec!["record".to_string(), args.id.clone()];
        push_option(&mut argv, "--format", args.format.as_deref());
        push_option(&mut argv, "--out", args.out.as_deref());
        push_option(&mut argv, "--flow", args.flow.as_deref());
        push_option(&mut argv, "--target", args.target.as_deref());

        let invocation = self.uibox.call(argv, cwd).await;
        let steps = invocation
            .field("steps")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let out = invocation.text("out").unwrap_or_else(|| "-".to_string());

        let mut report = Report::new(match out.as_str() {
            "-" => format!("recorded {steps} steps from {}.", args.id),
            path => format!("recorded {steps} steps from {} into {path}.", args.id),
        });
        report.absorb(&invocation);
        if out == "-" && !invocation.stdout.trim().is_empty() {
            report.block("flow", &invocation.stdout);
        }
        report.build()
    }

    async fn run(&self, args: RunArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let mut argv = vec!["run".to_string()];
        if let Some(flow) = &args.flow {
            argv.push(flow.clone());
        }
        push_option(&mut argv, "--lab", args.lab.as_deref());
        push_option(&mut argv, "--project", args.project.as_deref());
        push_option(&mut argv, "--build", args.build.as_deref());
        push_option(&mut argv, "--artifact", args.artifact.as_deref());
        push_option(&mut argv, "--source", args.source.as_deref());
        push_option(&mut argv, "--target-lab", args.target_lab.as_deref());
        push_option(&mut argv, "--surface", args.surface.as_deref());
        push_option(&mut argv, "--target", args.target.as_deref());
        push_option(&mut argv, "--viewport", args.viewport.as_deref());
        if args.lab_checkout {
            argv.push("--lab-checkout".to_string());
        }
        if args.no_place {
            argv.push("--no-place".to_string());
        }
        if args.keep_going {
            argv.push("--keep-going".to_string());
        }
        if args.force {
            argv.push("--force".to_string());
        }

        let invocation = self.uibox.call(argv, cwd).await;
        let summary = invocation.summary();
        let run = invocation.text("run");
        if let (Some(run), Some(dir)) = (&run, invocation.text("run_dir")) {
            self.remember(run, PathBuf::from(dir));
        }

        let verdict = invocation
            .text("verdict")
            .unwrap_or_else(|| "unknown".to_string());
        let failed = summary
            .get("steps_failed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = summary
            .get("steps_total")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let mut report = Report::new(match invocation.text("verdict") {
            Some(_) => format!(
                "flow {} finished with verdict {verdict}, {failed} of {total} steps failed.",
                invocation.text("flow").unwrap_or_else(|| "?".to_string())
            ),
            None => "the flow did not run, so it reached no verdict.".to_string(),
        });
        report.absorb(&invocation);
        facts_pick(
            &mut report,
            &summary,
            &[
                "run",
                "run_dir",
                "flow",
                "flow_file",
                "surface",
                "target",
                "backend",
                "verdict",
                "steps_total",
                "steps_failed",
                "halted_at",
                "placed",
                "goldens",
            ],
        );

        if let Some(halted) = invocation.text("halted_at") {
            report.line(format!("Halted at: {halted}"));
        }
        if let Some(placed) = summary.get("placed").filter(|v| !v.is_null()) {
            report.line(format!("Artifact placed: {placed}"));
        }

        let snaps = summary.get("snaps").cloned().unwrap_or(Value::Null);
        if let Some(paths) = snaps.as_array().filter(|list| !list.is_empty()) {
            report.block(
                "snapshots",
                paths
                    .iter()
                    .map(|snap| {
                        let name = snap.get("name").and_then(Value::as_str).unwrap_or("?");
                        let text = snap.get("text").and_then(Value::as_str).unwrap_or("-");
                        format!("{name}: {text}")
                    })
                    .collect::<Vec<String>>()
                    .join("\n"),
            );
        }

        let failing = report.is_failed();
        let max_chars = args.max_chars.unwrap_or(DEFAULT_MAX_CHARS);

        if failing {
            if let Some(path) = diagnostics::last_text(&snaps) {
                if let Some((body, dropped)) = diagnostics::snapshot_text(&path, max_chars).await {
                    report.block(
                        &format!("last snapshot ({})", path.display()),
                        truncation_note(&body, dropped),
                    );
                }
            }
        }

        if failing || args.include_image {
            if let Some(path) = diagnostics::last_png(&snaps) {
                match diagnostics::snapshot_png(&path).await {
                    Ok(bytes) => {
                        report.line(format!("Screenshot: {}", path.display()));
                        report.image(bytes);
                    }
                    Err(reason) => {
                        report.line(format!("Screenshot not inlined: {reason}"));
                    }
                }
            }
        }

        if let Some(run) = &run {
            let events = self.fresh_events(run, cwd, true).await;
            merge_events(&mut report, &events);
        }
        report.build()
    }

    async fn runs(&self, args: RunsArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let mut argv = vec!["runs".to_string()];
        if let Some(limit) = args.limit {
            argv.push("--limit".to_string());
            argv.push(limit.to_string());
        }
        let invocation = self.uibox.call(argv, cwd).await;

        let mut report = Report::new(format!(
            "{} run(s) recorded, showing {}.",
            invocation
                .field("total")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            invocation
                .field("shown")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        ));
        report.absorb(&invocation);
        if let Some(list) = invocation.field("runs").and_then(|v| v.as_array().cloned()) {
            report.block(
                "runs",
                list.iter()
                    .map(|entry| {
                        let id = entry.get("run").and_then(Value::as_str).unwrap_or("?");
                        let verdict = entry.get("verdict").and_then(Value::as_str).unwrap_or("?");
                        let flow = entry.get("flow").and_then(Value::as_str).unwrap_or("-");
                        let failed = entry
                            .get("steps_failed")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        let total = entry
                            .get("steps_total")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        format!("{id}  {verdict:<7} {failed}/{total} failed  {flow}")
                    })
                    .collect::<Vec<String>>()
                    .join("\n"),
            );
        }
        report.build()
    }

    async fn show(&self, args: ShowArgs) -> CallToolResult {
        let cwd = args.project_dir.as_deref();
        let what = args.what.as_deref().unwrap_or("meta");
        let invocation = self
            .uibox
            .call(
                vec![
                    "show".to_string(),
                    args.run.clone(),
                    "--what".to_string(),
                    what.to_string(),
                ],
                cwd,
            )
            .await;

        if let Some(dir) = invocation.text("run_dir") {
            self.remember(&args.run, PathBuf::from(dir));
        }

        let summary = invocation.summary();
        let mut report = Report::new(format!("run {}.", args.run));
        report.absorb(&invocation);
        facts_pick(&mut report, &summary, &["run", "run_dir"]);

        let max_chars = args.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
        for key in ["meta", "steps", "report", "snaps"] {
            let Some(value) = summary.get(key).filter(|value| !value.is_null()) else {
                continue;
            };
            let rendered =
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            let total = rendered.chars().count();
            let body: String = rendered.chars().take(max_chars).collect();
            report.block(key, truncation_note(&body, total.saturating_sub(max_chars)));
        }

        let events = self.fresh_events(&args.run, cwd, true).await;
        merge_events(&mut report, &events);
        report.build()
    }

    async fn dispatch(&self, name: &str, arguments: Option<Map<String, Value>>) -> CallToolResult {
        macro_rules! parsed {
            ($handler:ident) => {
                match decode(name, arguments) {
                    Ok(args) => self.$handler(args).await,
                    Err(reason) => Report::invalid(reason),
                }
            };
        }

        match name {
            schema::PREPARE => parsed!(prepare),
            schema::OPEN => parsed!(open),
            schema::ACT => parsed!(act),
            schema::SNAP => parsed!(snap),
            schema::EVAL => parsed!(eval),
            schema::CLOSE => parsed!(close),
            schema::RECORD => parsed!(record),
            schema::RUN => parsed!(run),
            schema::RUNS => parsed!(runs),
            schema::SHOW => parsed!(show),
            other => Report::invalid(format!("no tool named {other:?} on this server")),
        }
    }
}

impl ServerHandler for UiBoxServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ui-box-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(schema::INSTRUCTIONS);
        if let Err(reason) = self.uibox.location() {
            info.instructions = Some(format!(
                "{}\n\nWARNING: {reason}",
                info.instructions.unwrap_or_default()
            ));
        }
        info
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(schema::tools())))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        schema::tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(self
            .dispatch(request.name.as_ref(), request.arguments)
            .await
            .into())
    }
}

struct CaptureRequest<'a> {
    session: &'a str,
    mode: &'a str,
    name: Option<&'a str>,
    max_chars: usize,
    want_image: bool,
    cwd: Option<&'a Path>,
}

struct Capture {
    invocation: Invocation,
    snap: Value,
    text: Option<String>,
    truncated: usize,
    text_path: Option<PathBuf>,
    png_path: Option<PathBuf>,
    png: Option<Vec<u8>>,
    png_note: Option<String>,
    blank: bool,
    events: Events,
}

fn merge_capture(report: &mut Report, capture: &Capture, title: &str) {
    if let Landing::NotStarted(reason) = &capture.invocation.landing {
        report.failed(Domain::Tooling, reason.clone());
        return;
    }
    if !capture.invocation.landing.passed() && !report.is_failed() {
        report.absorb(&capture.invocation);
    }

    report.fact("snap", capture.snap.clone());

    match &capture.text {
        Some(body) if !body.trim().is_empty() => {
            let heading = match &capture.text_path {
                Some(path) => format!("{title} ({})", path.display()),
                None => title.to_string(),
            };
            report.block(&heading, truncation_note(body, capture.truncated));
        }
        _ if capture.blank => {
            report.line(BLANK_BANNER);
            if let Some(path) = &capture.text_path {
                report.line(format!("The empty snapshot is at {}.", path.display()));
            }
            report.failed(
                Domain::UnderTest,
                "the accessibility snapshot is empty, so the page rendered nothing",
            );
        }
        _ => {}
    }

    if let Some(png) = &capture.png {
        if let Some(path) = &capture.png_path {
            report.line(format!("Screenshot: {}", path.display()));
        }
        report.image(png.clone());
    } else if let Some(note) = &capture.png_note {
        report.line(format!("Screenshot not inlined: {note}"));
    } else if let Some(path) = &capture.png_path {
        report.line(format!("Screenshot: {}", path.display()));
    }

    merge_events(report, &capture.events);
}

fn merge_events(report: &mut Report, events: &Events) {
    for (title, body) in events.render() {
        report.block(&title, body);
    }
    if !events.console.is_empty() || !events.network.is_empty() {
        report.fact("console_errors", events.console.len());
        report.fact("network_failures", events.network.len());
    }
}

fn truncation_note(body: &str, dropped: usize) -> String {
    if dropped == 0 {
        return body.to_string();
    }
    format!("{body}\n... {dropped} more characters, read the file for the rest")
}

fn push_option(argv: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        argv.push(flag.to_string());
        argv.push(value.to_string());
    }
}

fn facts_pick(report: &mut Report, summary: &Value, keys: &[&str]) {
    for key in keys {
        if let Some(value) = summary.get(*key) {
            report.fact(key, value.clone());
        }
    }
}

fn decode<T: DeserializeOwned>(
    name: &str,
    arguments: Option<Map<String, Value>>,
) -> Result<T, String> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|err| format!("{name} was called with arguments it cannot use: {err}"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PrepareArgs {
    lab: Option<String>,
    force: bool,
    wait: Option<u64>,
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OpenArgs {
    target: Option<String>,
    surface: Option<String>,
    viewport: Option<String>,
    flow: Option<String>,
    snapshot: Option<bool>,
    max_chars: Option<usize>,
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ActArgs {
    session: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    steps: Option<Vec<Value>>,
    #[serde(default)]
    snapshot_on_failure: Option<bool>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

impl ActArgs {
    fn steps(&self) -> Result<Vec<Value>, String> {
        match &self.steps {
            Some(list) if !list.is_empty() => list.iter().map(normalise_step).collect(),
            _ => Ok(vec![self.flat_step()?]),
        }
    }

    fn flat_step(&self) -> Result<Value, String> {
        let action = self.action.as_deref().ok_or_else(|| {
            "ui_act needs either `action` with its argument, or a `steps` array".to_string()
        })?;
        build_step(
            action,
            self.selector.as_deref(),
            self.text.as_deref(),
            self.target.as_deref(),
            self.key.as_deref(),
            self.name.as_deref(),
        )
    }
}

#[derive(Debug, Deserialize)]
struct SnapArgs {
    session: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    include_image: Option<bool>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct EvalArgs {
    session: String,
    expr: String,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CloseArgs {
    session: String,
    #[serde(default)]
    keep_channel: bool,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RecordArgs {
    id: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    out: Option<String>,
    #[serde(default)]
    flow: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RunArgs {
    flow: Option<String>,
    lab: Option<String>,
    project: Option<String>,
    build: Option<String>,
    artifact: Option<String>,
    source: Option<String>,
    lab_checkout: bool,
    target_lab: Option<String>,
    surface: Option<String>,
    target: Option<String>,
    viewport: Option<String>,
    no_place: bool,
    keep_going: bool,
    force: bool,
    include_image: bool,
    max_chars: Option<usize>,
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RunsArgs {
    limit: Option<usize>,
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ShowArgs {
    run: String,
    #[serde(default)]
    what: Option<String>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    project_dir: Option<PathBuf>,
}

fn normalise_step(step: &Value) -> Result<Value, String> {
    let Some(map) = step.as_object() else {
        return Err(format!("a step must be an object, got {step}"));
    };
    if let Some(action) = map.get("action").and_then(Value::as_str) {
        return build_step(
            action,
            map.get("selector").and_then(Value::as_str),
            map.get("text").and_then(Value::as_str),
            map.get("target").and_then(Value::as_str),
            map.get("key").and_then(Value::as_str),
            map.get("name").and_then(Value::as_str),
        );
    }
    match map.len() {
        1 => Ok(step.clone()),
        _ => Err(format!(
            "a raw step carries exactly one verb, and {step} carries {}",
            map.len()
        )),
    }
}

fn build_step(
    action: &str,
    selector: Option<&str>,
    text: Option<&str>,
    target: Option<&str>,
    key: Option<&str>,
    name: Option<&str>,
) -> Result<Value, String> {
    let need = |value: Option<&str>, what: &str| -> Result<String, String> {
        value
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("action {action:?} needs {what}"))
    };

    let step = match action {
        "open" => Value::from(Map::from_iter([(
            "open".to_string(),
            Value::from(need(target.or(selector), "a `target`")?),
        )])),
        "click" => Value::from(Map::from_iter([(
            "click".to_string(),
            Value::from(need(selector, "a `selector`")?),
        )])),
        "type" => Value::from(Map::from_iter([(
            "type".to_string(),
            Value::from(Map::from_iter([
                (
                    "selector".to_string(),
                    Value::from(need(selector, "a `selector`")?),
                ),
                (
                    "text".to_string(),
                    Value::from(text.unwrap_or_default().to_string()),
                ),
            ])),
        )])),
        "key" => Value::from(Map::from_iter([(
            "key".to_string(),
            Value::from(need(key.or(text), "a `key`")?),
        )])),
        "wait_for" => Value::from(Map::from_iter([(
            "wait_for".to_string(),
            Value::from(need(selector, "a `selector`")?),
        )])),
        "assert_text" => Value::from(Map::from_iter([(
            "assert_text".to_string(),
            Value::from(need(selector, "a `selector`")?),
        )])),
        "snap" => {
            let mut detail = Map::new();
            if let Some(name) = name {
                detail.insert("name".to_string(), Value::from(name.to_string()));
            }
            Value::from(Map::from_iter([("snap".to_string(), Value::from(detail))]))
        }
        other => {
            return Err(format!(
                "unknown action {other:?}, expected one of \
                 open, click, type, key, wait_for, assert_text, snap"
            ))
        }
    };
    Ok(step)
}
