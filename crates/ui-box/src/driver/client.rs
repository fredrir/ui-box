use std::collections::VecDeque;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::DriverSpec;
use crate::config::Viewport;
use crate::error::DriverError;
use crate::note;

pub const METHOD_PREFIX: &str = "driver.";
pub const REQUEST_FIFO: &str = "driver.req";
pub const RESPONSE_FIFO: &str = "driver.res";
pub const DRIVER_LOG: &str = "driver.log";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DriverInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub surfaces: Vec<String>,
    #[serde(default, deserialize_with = "readable_tauri")]
    pub tauri: Option<TauriInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TauriInfo {
    #[serde(default)]
    pub ok: bool,
    #[serde(default, rename = "tauriDriver")]
    pub tauri_driver: Option<String>,
    #[serde(default, rename = "nativeDriver")]
    pub native_driver: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub reason: Option<String>,
}

fn readable_tauri<'de, D>(deserializer: D) -> Result<Option<TauriInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| serde_json::from_value(value).ok()))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActResult {
    #[serde(default = "yes")]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<Value>,
}

impl ActResult {
    pub fn error_text(&self) -> Option<String> {
        let error = self.error.as_ref()?;
        if error.is_null() {
            return None;
        }
        if let Some(text) = error.as_str() {
            return Some(text.to_string());
        }
        let message = error.get("message").and_then(Value::as_str);
        let selector = error.get("selector").and_then(Value::as_str);
        match (message, selector) {
            (Some(message), Some(selector)) => Some(format!("{message} ({selector})")),
            (Some(message), None) => Some(message.to_string()),
            _ => Some(error.to_string()),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SnapResult {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "pngPath")]
    pub png_path: Option<String>,
    #[serde(default, rename = "txtPath")]
    pub txt_path: Option<String>,
    #[serde(default)]
    pub console: Vec<Value>,
    #[serde(default)]
    pub network: Vec<Value>,
}

const STDERR_TAIL_LINES: usize = 20;
const STDERR_DRAIN: Duration = Duration::from_millis(250);

struct Stderr {
    lines: Mutex<VecDeque<String>>,
    drained: AtomicBool,
}

impl Stderr {
    fn new() -> Arc<Stderr> {
        Arc::new(Stderr {
            lines: Mutex::new(VecDeque::new()),
            drained: AtomicBool::new(false),
        })
    }

    fn push(&self, line: String) {
        let Ok(mut lines) = self.lines.lock() else {
            return;
        };
        if lines.len() == STDERR_TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    fn tail(&self) -> Option<String> {
        let deadline = Instant::now() + STDERR_DRAIN;
        while !self.drained.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let lines = self.lines.lock().ok()?;
        (!lines.is_empty()).then(|| {
            lines
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>()
                .join("\n")
        })
    }
}

pub struct Connection {
    name: String,
    writer: Box<dyn Write + Send>,
    responses: Receiver<Value>,
    next_id: u64,
    timeout: Duration,
    pid: Option<u32>,
    log: Option<PathBuf>,
    stderr: Option<Arc<Stderr>>,
    child: Option<Child>,
    prefix: String,
}

impl Connection {
    pub fn spawn(spec: &DriverSpec, timeout: Duration) -> Result<Connection> {
        let mut child = command_for(spec)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot start driver `{}`", spec.display()))?;
        let stdin = child.stdin.take().context("driver has no stdin")?;
        let stdout = child.stdout.take().context("driver has no stdout")?;
        let stderr = Stderr::new();
        match child.stderr.take() {
            Some(pipe) => {
                let name = spec.name.clone();
                let retained = Arc::clone(&stderr);
                std::thread::spawn(move || {
                    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                        note!("[{name}] {line}");
                        retained.push(line);
                    }
                    retained.drained.store(true, Ordering::Release);
                });
            }
            None => stderr.drained.store(true, Ordering::Release),
        }
        let pid = child.id();
        Ok(Connection {
            name: spec.name.clone(),
            writer: Box::new(stdin),
            responses: reader_thread(Box::new(stdout), spec.name.clone()),
            next_id: seed_id(),
            timeout,
            pid: Some(pid),
            log: None,
            stderr: Some(stderr),
            child: Some(child),
            prefix: method_prefix(),
        })
    }

    pub fn spawn_detached(spec: &DriverSpec, dir: &Path, timeout: Duration) -> Result<Connection> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let request = dir.join(REQUEST_FIFO);
        let response = dir.join(RESPONSE_FIFO);
        let log = dir.join(DRIVER_LOG);
        make_fifo(&request)?;
        make_fifo(&response)?;

        let child_stdin = duplex(&request)?;
        let child_stdout = duplex(&response)?;
        let child_stderr =
            File::create(&log).with_context(|| format!("cannot create {}", log.display()))?;

        let child = command_for(spec)?
            .stdin(Stdio::from(child_stdin))
            .stdout(Stdio::from(child_stdout))
            .stderr(Stdio::from(child_stderr))
            .spawn()
            .with_context(|| format!("cannot start driver `{}`", spec.display()))?;
        let pid = child.id();

        let writer = duplex(&request)?;
        let reader = duplex(&response)?;
        Ok(Connection {
            name: spec.name.clone(),
            writer: Box::new(writer),
            responses: reader_thread(Box::new(reader), spec.name.clone()),
            next_id: seed_id(),
            timeout,
            pid: Some(pid),
            log: Some(log),
            stderr: None,
            child: Some(child),
            prefix: method_prefix(),
        })
    }

    pub fn attach(dir: &Path, name: &str, pid: u32, timeout: Duration) -> Result<Connection> {
        let request = dir.join(REQUEST_FIFO);
        let response = dir.join(RESPONSE_FIFO);
        if !request.exists() || !response.exists() {
            return Err(DriverError::Exited {
                name: name.to_string(),
                method: "attach".to_string(),
                log: Some(format!("no driver channel in {}", dir.display())),
            }
            .into());
        }
        let writer = duplex(&request)?;
        let reader = duplex(&response)?;
        Ok(Connection {
            name: name.to_string(),
            writer: Box::new(writer),
            responses: reader_thread(Box::new(reader), name.to_string()),
            next_id: seed_id(),
            timeout,
            pid: Some(pid),
            log: Some(dir.join(DRIVER_LOG)),
            stderr: None,
            child: None,
            prefix: method_prefix(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let qualified = format!("{}{method}", self.prefix);
        match self.call_raw(&qualified, params.clone()) {
            Err(err) if !self.prefix.is_empty() && unknown_method(&err) => {
                self.prefix = String::new();
                note!(
                    "[{}] does not answer {qualified}, falling back to {method}",
                    self.name
                );
                self.call_raw(method, params)
            }
            other => other,
        }
    }

    fn call_raw(&mut self, method: &str, params: Value) -> Result<Value> {
        while self.responses.try_recv().is_ok() {}
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|err| self.transport_error(method, err))?;

        loop {
            match self.responses.recv_timeout(self.timeout) {
                Ok(value) => {
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unspecified driver error")
                            .to_string();
                        let data = error.get("data").map(|data| match data.as_str() {
                            Some(text) => text.to_string(),
                            None => data.to_string(),
                        });
                        let code = error
                            .get("code")
                            .and_then(Value::as_i64)
                            .unwrap_or_default();
                        return Err(DriverError::Rpc {
                            name: self.name.clone(),
                            code,
                            message,
                            data,
                        }
                        .into());
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.alive() {
                        return Err(DriverError::Timeout {
                            name: self.name.clone(),
                            method: method.to_string(),
                            timeout_secs: self.timeout.as_secs(),
                        }
                        .into());
                    }
                    return Err(self.exited(method));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(self.exited(method)),
            }
        }
    }

    pub fn info(&mut self) -> Result<DriverInfo> {
        let value = self.call("info", json!({}))?;
        serde_json::from_value(value).context("driver info is not {name, version, surfaces}")
    }

    pub fn open(&mut self, target: &str, viewport: Viewport, options: Value) -> Result<String> {
        let value = self.call(
            "open",
            json!({
                "target": target,
                "viewport": { "width": viewport.width, "height": viewport.height },
                "options": options,
            }),
        )?;
        value
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("driver open did not return a sessionId")
    }

    pub fn act(&mut self, session: &str, step: &Value) -> Result<ActResult> {
        let value = self.call("act", json!({ "sessionId": session, "step": step }))?;
        if value.is_null() {
            return Ok(ActResult::default());
        }
        serde_json::from_value(value).context("driver act is not {ok, error?}")
    }

    pub fn snap(&mut self, session: &str, mode: &str, name: &str) -> Result<SnapResult> {
        let value = self.call(
            "snap",
            json!({ "sessionId": session, "mode": mode, "name": name }),
        )?;
        if value.is_null() {
            return Ok(SnapResult::default());
        }
        serde_json::from_value(value).context("driver snap is not {text?, pngPath?, ...}")
    }

    pub fn eval(&mut self, session: &str, expr: &str) -> Result<Value> {
        let value = self.call("eval", json!({ "sessionId": session, "expr": expr }))?;
        Ok(value.get("value").cloned().unwrap_or(value))
    }

    pub fn close(&mut self, session: &str) -> Result<()> {
        self.call("close", json!({ "sessionId": session }))?;
        Ok(())
    }

    pub fn alive(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            return matches!(child.try_wait(), Ok(None));
        }
        match self.pid {
            Some(pid) => process_alive(pid),
            None => true,
        }
    }

    fn transport_error(&self, method: &str, err: std::io::Error) -> anyhow::Error {
        DriverError::Exited {
            name: self.name.clone(),
            method: method.to_string(),
            log: Some(format!("{err}\n{}", self.log_tail().unwrap_or_default())),
        }
        .into()
    }

    fn exited(&self, method: &str) -> anyhow::Error {
        DriverError::Exited {
            name: self.name.clone(),
            method: method.to_string(),
            log: self.log_tail(),
        }
        .into()
    }

    fn log_tail(&self) -> Option<String> {
        match &self.log {
            Some(path) => file_tail(path),
            None => self.stderr.as_ref()?.tail(),
        }
    }
}

fn file_tail(path: &Path) -> Option<String> {
    let mut contents = String::new();
    File::open(path).ok()?.read_to_string(&mut contents).ok()?;
    let tail: Vec<&str> = contents.lines().rev().take(STDERR_TAIL_LINES).collect();
    if tail.is_empty() {
        return None;
    }
    Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn command_for(spec: &DriverSpec) -> Result<Command> {
    let Some((program, args)) = spec.argv.split_first() else {
        bail!("driver {} has no command to run", spec.name);
    };
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

fn reader_thread(source: Box<dyn Read + Send>, name: String) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(source).lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(value) => {
                    if tx.send(value).is_err() {
                        return;
                    }
                }
                Err(_) => note!("[{name}] {trimmed}"),
            }
        }
    });
    rx
}

fn method_prefix() -> String {
    match std::env::var("UIBOX_RPC_PREFIX") {
        Ok(prefix) => prefix,
        Err(_) => METHOD_PREFIX.to_string(),
    }
}

fn unknown_method(error: &anyhow::Error) -> bool {
    let Some(DriverError::Rpc { code, message, .. }) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<DriverError>())
    else {
        return false;
    };
    let lowered = message.to_ascii_lowercase();
    *code == -32601 || lowered.contains("unknown method") || lowered.contains("method not found")
}

fn seed_id() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.subsec_nanos() as u64 * 1_000 + 1)
        .unwrap_or(1)
}

fn make_fifo(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("cannot replace {}", path.display()))?;
    }
    let raw = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("bad fifo path {}", path.display()))?;
    let status = unsafe { libc::mkfifo(raw.as_ptr(), 0o600) };
    if status != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot create fifo {}", path.display()));
    }
    Ok(())
}

fn duplex(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))
}

pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(raw: &str) -> DriverInfo {
        serde_json::from_str(raw).expect("driver info")
    }

    fn shim(script: &str) -> DriverSpec {
        DriverSpec {
            name: "shim".to_string(),
            surface: crate::config::Surface::Web,
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            entry: None,
            remote: false,
        }
    }

    #[test]
    fn a_driver_that_dies_carries_its_stderr_into_the_error() {
        let spec = shim(
            "echo 'Warning: remote port forwarding failed for listen port 3000' >&2; exit 255",
        );
        let mut conn = Connection::spawn(&spec, Duration::from_secs(5)).expect("spawn");
        let err = conn
            .info()
            .expect_err("a driver that exits cannot answer info");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("remote port forwarding failed for listen port 3000"),
            "forward::classify matches on this text, so discarding it loses the diagnosis:              {rendered}"
        );
    }

    #[test]
    fn a_dead_driver_stays_a_driver_exit_for_classify_to_narrow() {
        let spec = shim("exit 1");
        let mut conn = Connection::spawn(&spec, Duration::from_secs(5)).expect("spawn");
        let err = conn
            .info()
            .expect_err("a driver that exits cannot answer info");
        let kind = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<DriverError>())
            .map(DriverError::kind);
        assert_eq!(kind, Some("driver_exited"));
    }

    #[test]
    fn a_driver_that_predates_the_tauri_block_still_answers() {
        let parsed = info(r#"{"name":"dom","version":"0.1","surfaces":["web"]}"#);
        assert_eq!(parsed.name, "dom");
        assert!(parsed.tauri.is_none());
    }

    #[test]
    fn a_tauri_block_ui_box_cannot_read_never_costs_us_the_rest_of_info() {
        let parsed = info(
            r#"{"name":"dom","version":"0.1","surfaces":["web"],
                "tauri":{"ok":"maybe","tauriDriver":[1,2]}}"#,
        );
        assert_eq!(parsed.surfaces, vec!["web".to_string()]);
        assert!(
            parsed.tauri.is_none(),
            "an unreadable block is unknown, not a verdict"
        );
    }

    #[test]
    fn source_is_carried_verbatim_whatever_shape_the_driver_chose() {
        let parsed = info(
            r#"{"name":"dom","version":"0.1","surfaces":["tauri"],
                "tauri":{"ok":false,"tauriDriver":"tauri-driver","nativeDriver":null,
                         "source":{"tauriDriver":"default","nativeDriver":"unset"},
                         "reason":"tauri-driver is not on PATH on the driver host"}}"#,
        );
        let tauri = parsed.tauri.expect("tauri block");
        assert!(!tauri.ok);
        assert_eq!(tauri.tauri_driver.as_deref(), Some("tauri-driver"));
        assert_eq!(tauri.native_driver, None);
        assert_eq!(
            tauri.source.and_then(|source| source
                .get("tauriDriver")
                .and_then(Value::as_str)
                .map(str::to_string)),
            Some("default".to_string())
        );
        assert_eq!(
            tauri.reason.as_deref(),
            Some("tauri-driver is not on PATH on the driver host")
        );
    }
}
