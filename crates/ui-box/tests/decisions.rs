use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uibox-decide-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("workspace");
    dir
}

fn driver(dir: &Path) -> String {
    let path = dir.join("driver.js");
    std::fs::write(
        &path,
        r#"
const readline = require('readline');
const fs = require('fs');
const path = require('path');
let n = 0;
const dirs = new Map();
const used = new Map();
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
  } else if (verb === 'act') {
    const step = req.params.step || {};
    result = step.click === 'css=#fail'
      ? { ok: false, error: 'no element matches css=#fail' }
      : { ok: true };
  } else if (verb === 'snap') {
    const dir = dirs.get(req.params.sessionId);
    fs.mkdirSync(dir, { recursive: true });
    const base = String(req.params.name || 'snap').replace(/[^A-Za-z0-9._-]/g, '-');
    const seen = used.get(req.params.sessionId) || new Set();
    let name = base, k = 1;
    while (seen.has(name)) { k += 1; name = base + '-' + k; }
    seen.add(name); used.set(req.params.sessionId, seen);
    const out = { name, console: [], network: [] };
    if (req.params.mode !== 'png') {
      const p = path.join(dir, name + '.txt');
      const tree = '- heading "Costs" [level=1]:\n  - text: all \u00b7 11\n';
      fs.writeFileSync(p, tree);
      out.text = tree;
      out.txtPath = p;
    }
    if (req.params.mode !== 'text') {
      const p = path.join(dir, name + '.png');
      fs.writeFileSync(p, Buffer.from('89504e470d0a1a0a', 'hex'));
      out.pngPath = p;
    }
    result = out;
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result }) + '\n');
});
"#,
    )
    .expect("driver fixture");
    format!("node {}", path.display())
}

fn ui_box(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ui-box"))
        .args(args)
        .env("UIBOX_ARTIFACTS", dir.join("runs"))
        .env("UIBOX_BACKEND", "local://")
        .env("UIBOX_HOME", dir)
        .env("UIBOX_DRIVER_DOM", driver(dir))
        .env_remove("UIBOX_GOLDENS")
        .output()
        .expect("ui-box runs")
}

const PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND: u16 = 1;

fn ssh_shim(dir: &Path, sentinel: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let path = bin.join("ssh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 255\n",
            sentinel.display()
        ),
    )
    .expect("ssh shim");
    let mut perms = std::fs::metadata(&path)
        .expect("shim metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("shim mode");
    bin
}

fn ui_box_bare(dir: &Path, args: &[&str], bin: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ui-box"));
    command
        .args(args)
        .current_dir(dir)
        .env("UIBOX_ARTIFACTS", dir.join("runs"))
        .env("UIBOX_HOME", dir)
        .env_remove("UIBOX_BACKEND")
        .env_remove("UIBOX_DRIVER_DOM")
        .env_remove("UIBOX_FORWARD")
        .env_remove("UIBOX_GOLDENS");
    if let Some(bin) = bin {
        let inherited = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{inherited}", bin.display()));
    }
    command.output().expect("ui-box runs")
}

fn summary(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("not json: {stdout}"))
}

fn flow(dir: &Path, body: &str) -> String {
    let path = dir.join("flow.yaml");
    std::fs::write(&path, body).expect("flow");
    path.display().to_string()
}

const FOUR_STEPS: &str = "version: 1\nflow: gate\nsurface: web\ntarget: http://x/\nsteps:\n  \
     - click: \"css=#one\"\n  - click: \"css=#fail\"\n  - click: \"css=#three\"\n  \
     - click: \"css=#four\"\n  - assert_text: \"text=done\"\n";

const NO_ASSERTIONS: &str = "version: 1\nflow: transcript\nsurface: web\ntarget: http://x/\n\
     steps:\n  - click: \"css=#one\"\n  - snap: { name: after }\n";

#[test]
fn a_failing_step_halts_the_flow_before_the_rest_run() {
    if !node_available() {
        return;
    }
    let dir = workspace("halt");
    let path = flow(&dir, FOUR_STEPS);
    let body = summary(&ui_box(&dir, &["run", &path]));
    assert_eq!(body["verdict"], "fail");
    assert_eq!(
        body["steps_total"], 2,
        "the flow must stop at the failing step, not drive the UI past its gate"
    );
    assert_eq!(body["halted_at"], "click css=#fail");
}

#[test]
fn keep_going_runs_the_steps_after_a_failure() {
    if !node_available() {
        return;
    }
    let dir = workspace("keepgoing");
    let path = flow(&dir, FOUR_STEPS);
    let body = summary(&ui_box(&dir, &["run", &path, "--keep-going"]));
    assert_eq!(body["verdict"], "fail");
    assert_eq!(body["steps_total"], 5);
    assert_eq!(body["halted_at"], serde_json::Value::Null);
}

#[test]
fn a_step_lands_in_steps_yaml_before_the_session_is_closed() {
    if !node_available() {
        return;
    }
    let dir = workspace("append");
    let opened = summary(&ui_box(&dir, &["open", "http://x/"]));
    let session = opened["session"].as_str().expect("session").to_string();
    let run_dir = PathBuf::from(opened["run_dir"].as_str().expect("run_dir"));

    ui_box(&dir, &["act", &session, "click", "css=#one"]);

    let steps = std::fs::read_to_string(run_dir.join("steps.yaml"))
        .expect("steps.yaml must exist before close, not only after it");
    assert!(steps.contains("css=#one"), "{steps}");

    ui_box(&dir, &["close", &session]);
}

#[test]
fn a_snapshot_defaults_to_text_and_never_to_png() {
    if !node_available() {
        return;
    }
    let dir = workspace("mode");
    let opened = summary(&ui_box(&dir, &["open", "http://x/"]));
    let session = opened["session"].as_str().expect("session").to_string();

    let snapped = summary(&ui_box(&dir, &["snap", &session, "--name", "shot"]));
    assert_eq!(snapped["snap"]["mode"], "text");
    assert!(
        snapped["snap"]["png"].is_null(),
        "png must stay opt-in: {}",
        snapped["snap"]
    );
    assert!(snapped["snap"]["text"].is_string());

    ui_box(&dir, &["close", &session]);
}

#[test]
fn a_loopback_target_on_a_lab_is_refused_before_any_ssh_is_spawned() {
    let dir = workspace("forward-missing");
    let sentinel = dir.join("ssh-was-spawned");
    let bin = ssh_shim(&dir, &sentinel);
    let output = ui_box_bare(
        &dir,
        &["open", "http://localhost:3000", "--backend", "ssh://x@y"],
        Some(&bin),
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "ui-box could not run, which is exit 2, never the UI failing"
    );
    let body = summary(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_kind"], "forward_missing");
    let message = body["error"].as_str().expect("error text");
    assert!(message.contains("3000"), "{message}");
    assert!(message.contains("--forward 3000"), "{message}");
    assert!(
        !sentinel.exists(),
        "the refusal must precede probe_backend, or it costs a connection to say no"
    );
}

#[test]
fn a_forward_whose_local_end_is_closed_is_refused_before_any_ssh_is_spawned() {
    let dir = workspace("forward-closed");
    let sentinel = dir.join("ssh-was-spawned");
    let bin = ssh_shim(&dir, &sentinel);
    let port = PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND;

    let output = ui_box_bare(
        &dir,
        &[
            "open",
            "http://localhost:3000",
            "--backend",
            "ssh://x@y",
            "--forward",
            &format!("3000:{port}"),
        ],
        Some(&bin),
    );

    assert_eq!(output.status.code(), Some(2));
    let body = summary(&output);
    assert!(
        body["error_kind"]
            .as_str()
            .expect("kind")
            .starts_with("forward_"),
        "{body}"
    );
    assert!(body["error"]
        .as_str()
        .expect("error")
        .contains(&port.to_string()));
    assert!(
        !sentinel.exists(),
        "nothing local answers, so nothing is worth connecting for"
    );
}

fn opened_forwards(dir: &Path, driver: &str, env: Option<&str>, args: &[&str]) -> Vec<String> {
    let mut argv = vec!["open", "http://example.invalid/"];
    argv.extend_from_slice(args);
    let mut command = Command::new(env!("CARGO_BIN_EXE_ui-box"));
    command
        .args(&argv)
        .current_dir(dir)
        .env("UIBOX_ARTIFACTS", dir.join("runs"))
        .env("UIBOX_BACKEND", "local://")
        .env("UIBOX_HOME", dir)
        .env("UIBOX_DRIVER_DOM", driver)
        .env_remove("UIBOX_FORWARD")
        .env_remove("UIBOX_GOLDENS");
    if let Some(value) = env {
        command.env("UIBOX_FORWARD", value);
    }
    let output = command.output().expect("ui-box runs");
    let body = summary(&output);
    let session = body["session"].as_str().expect("session").to_string();
    let forwards = body["forward"]
        .as_array()
        .expect("forward is a list")
        .iter()
        .map(|value| value.as_str().expect("label").to_string())
        .collect();
    ui_box(dir, &["close", &session]);
    forwards
}

#[test]
fn a_forward_resolves_cli_over_environment_over_project_file() {
    if !node_available() {
        return;
    }
    let dir = workspace("forward-layers");
    std::fs::write(dir.join("uibox.toml"), "forward = \"4000\"\n").expect("uibox.toml");
    let driver = driver(&dir);

    assert_eq!(opened_forwards(&dir, &driver, None, &[]), vec!["4000"]);
    assert_eq!(
        opened_forwards(&dir, &driver, Some("3000"), &[]),
        vec!["3000"]
    );
    assert_eq!(
        opened_forwards(&dir, &driver, Some("3000"), &["--forward", "5000"]),
        vec!["5000"]
    );
    assert_eq!(
        opened_forwards(
            &dir,
            &driver,
            Some("3000"),
            &["--forward", "5000", "--forward", "6000"]
        ),
        vec!["5000", "6000"]
    );
}

#[test]
fn a_flow_that_asserts_nothing_is_refused_before_the_driver_starts() {
    let dir = workspace("asserts-nothing");
    let path = flow(&dir, NO_ASSERTIONS);
    let output = ui_box(&dir, &["run", &path]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "a transcript is not a failing test, it is an input ui-box cannot verify with"
    );
    let body = summary(&output);
    assert_eq!(body["error_kind"], "flow_asserts_nothing");
    let message = body["error"].as_str().expect("error");
    assert!(message.contains("blank page"), "{message}");
    assert!(
        !dir.join("runs").exists(),
        "the refusal must precede the run directory, or it costs a driver to say no"
    );
}

#[test]
fn a_recorded_flow_asserts_what_the_run_observed() {
    if !node_available() {
        return;
    }
    let dir = workspace("record-asserts");
    let opened = summary(&ui_box(&dir, &["open", "http://x/"]));
    let session = opened["session"].as_str().expect("session").to_string();
    ui_box(&dir, &["act", &session, "click", "css=#one"]);
    ui_box(&dir, &["snap", &session, "--name", "after"]);
    ui_box(&dir, &["close", &session]);

    let recorded = summary(&ui_box(&dir, &["record", &session]));
    assert!(
        recorded["assertions"].as_u64().unwrap_or(0) >= 1,
        "a recorded flow that asserts nothing is green forever: {recorded}"
    );
    assert!(recorded["derived"].as_u64().unwrap_or(0) >= 1);

    let written = std::fs::read_to_string(recorded["out"].as_str().expect("out")).expect("flow");
    assert!(written.contains("assert_text"), "{written}");
    assert!(written.contains("Costs"), "{written}");
}

#[test]
fn a_recording_with_nothing_to_assert_is_not_offered_as_a_test() {
    if !node_available() {
        return;
    }
    let dir = workspace("record-nothing");
    let opened = summary(&ui_box(&dir, &["open", "http://x/"]));
    let session = opened["session"].as_str().expect("session").to_string();
    ui_box(&dir, &["act", &session, "click", "css=#one"]);
    ui_box(&dir, &["close", &session]);

    let output = ui_box(&dir, &["record", &session]);
    assert_eq!(output.status.code(), Some(2));
    let body = summary(&output);
    assert_eq!(body["error_kind"], "flow_asserts_nothing");

    let run_dir = PathBuf::from(opened["run_dir"].as_str().expect("run_dir"));
    assert!(
        run_dir.join("flow.yaml").is_file(),
        "the transcript is still written, so a long session is not lost to the refusal"
    );
}

#[test]
fn a_replayed_flow_carries_its_derived_assertion() {
    if !node_available() {
        return;
    }
    let dir = workspace("record-replay");
    let opened = summary(&ui_box(&dir, &["open", "http://x/"]));
    let session = opened["session"].as_str().expect("session").to_string();
    ui_box(&dir, &["snap", &session, "--name", "after"]);
    ui_box(&dir, &["close", &session]);

    let recorded = summary(&ui_box(&dir, &["record", &session]));
    let written = recorded["out"].as_str().expect("out").to_string();
    let replayed = summary(&ui_box(&dir, &["run", &written]));
    assert_eq!(replayed["verdict"], "pass", "{replayed}");
}

#[test]
fn a_verify_that_replayed_nothing_is_not_reported_as_a_pass() {
    let dir = workspace("nothing-verified");
    let empty = dir.join("no-flows");
    std::fs::create_dir_all(&empty).expect("flows dir");
    let output = ui_box(&dir, &["verify", "--flows", &empty.display().to_string()]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "nothing failed, so a pre-push gate must not go red on a project with no flows yet"
    );
    let body = summary(&output);
    assert_eq!(
        body["ok"], false,
        "exit 0 on its own reads as verified, which is the bug"
    );
    assert_eq!(body["status"], "nothing_verified");
    assert_eq!(body["skipped"], true);
    assert_eq!(body["flows"], 0);
}
