use std::path::PathBuf;
use std::process::{Command, Output};

fn artifacts() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uibox-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp artifacts");
    dir
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn lying_driver() -> PathBuf {
    let path = artifacts().join("lying-driver.js");
    std::fs::write(
        &path,
        r#"
const readline = require('readline');
let n = 0;
const dirs = new Map();
readline.createInterface({ input: process.stdin }).on('line', (line) => {
  if (!line.trim()) return;
  const req = JSON.parse(line);
  const verb = String(req.method).replace(/^driver\./, '');
  let result = {};
  if (verb === 'info') result = { name: 'dom', version: '0', surfaces: ['web'] };
  else if (verb === 'open') {
    const s = 'd' + (++n);
    dirs.set(s, req.params.options.snapsDir);
    result = { sessionId: s };
  } else if (verb === 'snap') {
    result = { name: 'ghost', txtPath: dirs.get(req.params.sessionId) + '/ghost.txt',
               console: [], network: [] };
  } else if (verb === 'act') result = { ok: true };
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
});
"#,
    )
    .expect("driver fixture");
    path
}

fn ui_box_with(driver: &str, args: &[&str]) -> Output {
    ui_box_env(args, &[("UIBOX_DRIVER_DOM", driver)])
}

fn ui_box(args: &[&str]) -> Output {
    ui_box_env(args, &[])
}

fn ui_box_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ui-box"));
    command
        .args(args)
        .env("UIBOX_ARTIFACTS", artifacts())
        .env("UIBOX_BACKEND", "local://")
        .env("UIBOX_HOME", artifacts())
        .env_remove("UIBOX_GOLDENS")
        .env_remove("UIBOX_SURFACE")
        .env_remove("UIBOX_FORWARD")
        .env_remove("UIBOX_TARGET");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("ui-box runs")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

fn summary(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("not json: {stdout}"))
}

fn check(body: &serde_json::Value, name: &str) -> serde_json::Value {
    body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no check named {name}"))
}

fn severity(body: &serde_json::Value, name: &str) -> String {
    check(body, name)["severity"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn an_unknown_session_is_a_tool_failure_not_a_ui_failure() {
    let output = ui_box(&["act", "no-such-session", "click", "css=#go"]);
    assert_eq!(
        code(&output),
        2,
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(summary(&output)["error_kind"], "unknown_session");
    assert_eq!(summary(&output)["ok"], false);
}

#[test]
fn an_unsupported_surface_is_a_tool_failure() {
    let output = ui_box(&["open", "tui:nsql", "--surface", "tui"]);
    assert_eq!(code(&output), 2);
    assert_eq!(summary(&output)["error_kind"], "unsupported_surface");
}

#[test]
fn a_missing_run_is_a_tool_failure() {
    assert_eq!(code(&ui_box(&["show", "20000101T000000Z-deadbeef"])), 2);
    assert_eq!(code(&ui_box(&["record", "20000101T000000Z-deadbeef"])), 2);
}

#[test]
fn a_malformed_step_is_a_tool_failure() {
    let output = ui_box(&["act", "no-such-session", "swipe", "left"]);
    assert_eq!(code(&output), 2);
}

#[test]
fn doctor_exits_two_only_when_a_blocking_check_fails() {
    let output = ui_box(&["doctor"]);
    let body = summary(&output);
    assert_eq!(severity(&body, "config"), "blocking");
    assert_eq!(severity(&body, "artifacts"), "blocking");
    assert_eq!(severity(&body, "backend"), "blocking");
    assert_eq!(severity(&body, "driver.dom"), "blocking");
    assert_eq!(severity(&body, "sessions"), "blocking");
    assert_eq!(severity(&body, "goldens"), "advisory");
    assert_eq!(severity(&body, "vision"), "advisory");
    assert_eq!(severity(&body, "pipeline"), "advisory");
    assert_eq!(severity(&body, "driver.tauri"), "advisory");

    let blocking = body["blocking_failed"].as_u64().expect("blocking_failed");
    let expected = if blocking > 0 { 2 } else { 0 };
    assert_eq!(
        code(&output),
        expected,
        "exit must follow blocking_failed, not any failure"
    );
}

#[test]
fn a_missing_golden_store_alone_never_makes_ui_box_unusable() {
    let output = ui_box(&["doctor"]);
    let body = summary(&output);
    assert!(
        body["advisory_failed"].as_u64().expect("advisory_failed") >= 1,
        "UIBOX_GOLDENS is removed in this harness, so at least one advisory check must fail"
    );
    if body["blocking_failed"].as_u64() == Some(0) {
        assert_eq!(code(&output), 0);
        assert_eq!(body["ok"], true);
    }
}

#[test]
fn wake_never_fails_its_caller() {
    assert_eq!(code(&ui_box(&["wake"])), 0);
    assert_eq!(
        code(&ui_box(&[
            "wake",
            "--lab",
            "no-such-host-anywhere",
            "--wait",
            "1"
        ])),
        0
    );
}

#[test]
fn listing_runs_succeeds_even_with_no_runs() {
    let output = ui_box(&["runs"]);
    assert_eq!(code(&output), 0);
    assert_eq!(summary(&output)["ok"], true);
}

#[test]
fn every_command_prints_one_json_line_on_stdout() {
    for args in [vec!["runs"], vec!["wake"], vec!["show", "nope"]] {
        let output = ui_box(&args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.trim().lines().count(),
            1,
            "{args:?} printed {stdout}"
        );
        assert!(
            summary(&output).get("ok").is_some(),
            "{args:?} has no ok field"
        );
    }
}

#[test]
fn a_path_the_driver_never_wrote_is_a_tool_failure_not_a_blank_page() {
    if !node_available() {
        return;
    }
    let driver = format!("node {}", lying_driver().display());
    let opened = ui_box_with(&driver, &["open", "http://127.0.0.1:1/"]);
    assert_eq!(
        code(&opened),
        0,
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    let session = summary(&opened)["session"]
        .as_str()
        .expect("session")
        .to_string();

    let snapped = ui_box_with(&driver, &["snap", &session, "--name", "ghost"]);
    let body = summary(&snapped);
    assert_eq!(
        code(&snapped),
        2,
        "a phantom artifact must never be reportable as a UI result: {}",
        snapped.status
    );
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("ghost.txt"), "{error}");
    assert!(error.contains("no such file exists"), "{error}");

    let _ = ui_box_with(&driver, &["close", &session]);
}

#[test]
fn the_tauri_driver_blocks_only_for_a_project_that_declares_that_surface() {
    let web = summary(&ui_box(&["doctor"]));
    assert_eq!(
        severity(&web, "driver.tauri"),
        "advisory",
        "a web-only box must not fail doctor over a surface it never drives"
    );

    let declared = summary(&ui_box_env(&["doctor"], &[("UIBOX_SURFACE", "tauri")]));
    assert_eq!(
        severity(&declared, "driver.tauri"),
        "blocking",
        "declaring surface = tauri means nothing ui-box will be asked to do can run without it"
    );
}

#[test]
fn a_driver_that_cannot_report_on_tauri_is_never_a_tauri_pass() {
    if !node_available() {
        return;
    }
    let driver = format!("node {}", lying_driver().display());
    let body = summary(&ui_box_with(&driver, &["doctor"]));
    let tauri = check(&body, "driver.tauri");
    assert_eq!(
        tauri["ok"], false,
        "a driver too old to answer has not said yes"
    );
    let detail = tauri["detail"].as_str().unwrap_or_default();
    assert!(detail.starts_with("unknown:"), "{detail}");
}

#[test]
fn doctor_makes_no_forward_claim_when_no_forward_was_declared() {
    let body = summary(&ui_box(&["doctor"]));
    let named = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .any(|check| check["name"] == "forward");
    assert!(
        !named,
        "with nothing to check, doctor must stay silent rather than report a pass"
    );
}
