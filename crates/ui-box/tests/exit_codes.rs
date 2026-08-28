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
    Command::new(env!("CARGO_BIN_EXE_ui-box"))
        .args(args)
        .env("UIBOX_ARTIFACTS", artifacts())
        .env("UIBOX_BACKEND", "local://")
        .env("UIBOX_HOME", artifacts())
        .env("UIBOX_DRIVER_DOM", driver)
        .env_remove("UIBOX_GOLDENS")
        .output()
        .expect("ui-box runs")
}

fn ui_box(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ui-box"))
        .args(args)
        .env("UIBOX_ARTIFACTS", artifacts())
        .env("UIBOX_BACKEND", "local://")
        .env("UIBOX_HOME", artifacts())
        .env_remove("UIBOX_GOLDENS")
        .output()
        .expect("ui-box runs")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

fn summary(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("not json: {stdout}"))
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
    let checks = body["checks"].as_array().expect("checks");
    let severity = |name: &str| {
        checks
            .iter()
            .find(|check| check["name"] == name)
            .map(|check| check["severity"].as_str().unwrap_or_default().to_string())
            .unwrap_or_else(|| panic!("no check named {name}"))
    };
    assert_eq!(severity("config"), "blocking");
    assert_eq!(severity("artifacts"), "blocking");
    assert_eq!(severity("backend"), "blocking");
    assert_eq!(severity("driver.dom"), "blocking");
    assert_eq!(severity("sessions"), "blocking");
    assert_eq!(severity("goldens"), "advisory");
    assert_eq!(severity("vision"), "advisory");
    assert_eq!(severity("pipeline"), "advisory");

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
    assert_eq!(code(&opened), 0, "{}", String::from_utf8_lossy(&opened.stderr));
    let session = summary(&opened)["session"].as_str().expect("session").to_string();

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

    ui_box_with(&driver, &["close", &session]).status.code();
}
