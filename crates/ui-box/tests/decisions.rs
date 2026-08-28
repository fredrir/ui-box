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
      fs.writeFileSync(p, 'tree\n');
      out.text = 'tree\n';
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
     - click: \"css=#four\"\n";

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
    assert_eq!(body["steps_total"], 4);
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
