use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

pub const BIN_ENV: &str = "UIBOX_BIN";
pub const PROGRAM: &str = "ui-box";

pub const EXIT_PASSED: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_UNUSABLE: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    Passed,
    Failed,
    Unusable,
    UnexpectedCode(i32),
    Signalled,
    NotStarted(String),
}

impl Landing {
    pub fn from_code(code: i32) -> Landing {
        match code {
            EXIT_PASSED => Landing::Passed,
            EXIT_FAILED => Landing::Failed,
            EXIT_UNUSABLE => Landing::Unusable,
            other => Landing::UnexpectedCode(other),
        }
    }

    pub fn passed(&self) -> bool {
        matches!(self, Landing::Passed)
    }

    pub fn is_step_failure(&self) -> bool {
        matches!(self, Landing::Failed)
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Landing::Passed => Some(EXIT_PASSED),
            Landing::Failed => Some(EXIT_FAILED),
            Landing::Unusable => Some(EXIT_UNUSABLE),
            Landing::UnexpectedCode(code) => Some(*code),
            Landing::Signalled | Landing::NotStarted(_) => None,
        }
    }

    pub fn tooling_reason(&self) -> Option<String> {
        match self {
            Landing::Passed | Landing::Failed => None,
            Landing::Unusable => Some(format!(
                "`{PROGRAM}` exited 2: it could not run at all (config, backend or driver)."
            )),
            Landing::UnexpectedCode(code) => Some(format!(
                "`{PROGRAM}` exited {code}, which is outside the three-valued contract \
                 (0 passed, 1 the UI failed, 2 ui-box could not run)."
            )),
            Landing::Signalled => Some(format!(
                "`{PROGRAM}` was killed by a signal before it could report a verdict."
            )),
            Landing::NotStarted(reason) => Some(reason.clone()),
        }
    }
}

#[derive(Debug)]
pub struct Invocation {
    pub argv: Vec<String>,
    pub landing: Landing,
    pub stdout: String,
    pub stderr: String,
}

impl Invocation {
    pub fn command_line(&self) -> String {
        self.argv
            .iter()
            .map(|word| {
                if word.is_empty() || word.contains(char::is_whitespace) {
                    format!("{word:?}")
                } else {
                    word.clone()
                }
            })
            .collect::<Vec<String>>()
            .join(" ")
    }

    pub fn summary(&self) -> Value {
        summary_line(&self.stdout)
            .or_else(|| summary_line(&self.stderr))
            .unwrap_or(Value::Null)
    }

    pub fn field(&self, key: &str) -> Option<Value> {
        match self.summary() {
            Value::Object(map) => map.get(key).cloned(),
            _ => None,
        }
    }

    pub fn text(&self, key: &str) -> Option<String> {
        match self.field(key) {
            Some(Value::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn stderr_verbatim(&self) -> Option<String> {
        let prose = self
            .stderr
            .lines()
            .filter(|line| !is_summary(line))
            .collect::<Vec<&str>>()
            .join("\n");
        let trimmed = prose.trim_end().to_string();
        (!trimmed.trim().is_empty()).then_some(trimmed)
    }

    pub fn error_message(&self) -> Option<String> {
        self.text("error")
    }

    pub fn error_kind(&self) -> Option<String> {
        self.text("error_kind")
    }
}

fn parse_summary(line: &str) -> Option<Value> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) if map.contains_key("ok") => Some(Value::Object(map)),
        _ => None,
    }
}

fn is_summary(line: &str) -> bool {
    parse_summary(line).is_some()
}

fn summary_line(stream: &str) -> Option<Value> {
    stream.lines().rev().find_map(parse_summary)
}

pub struct UiBox {
    program: Result<PathBuf, String>,
}

impl UiBox {
    pub fn discover() -> UiBox {
        UiBox {
            program: resolve_program(),
        }
    }

    pub fn location(&self) -> Result<&Path, &str> {
        match &self.program {
            Ok(path) => Ok(path.as_path()),
            Err(reason) => Err(reason.as_str()),
        }
    }

    pub async fn call(&self, args: Vec<String>, cwd: Option<&Path>) -> Invocation {
        let mut argv = vec![PROGRAM.to_string()];
        argv.extend(args.iter().cloned());

        let program = match &self.program {
            Ok(path) => path,
            Err(reason) => {
                return Invocation {
                    argv,
                    landing: Landing::NotStarted(reason.clone()),
                    stdout: String::new(),
                    stderr: String::new(),
                }
            }
        };

        let mut command = tokio::process::Command::new(program);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }

        match command.output().await {
            Ok(output) => Invocation {
                argv,
                landing: match output.status.code() {
                    Some(code) => Landing::from_code(code),
                    None => Landing::Signalled,
                },
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(err) => Invocation {
                argv,
                landing: Landing::NotStarted(format!(
                    "cannot execute {}: {err}",
                    program.display()
                )),
                stdout: String::new(),
                stderr: String::new(),
            },
        }
    }
}

fn resolve_program() -> Result<PathBuf, String> {
    if let Some(configured) = std::env::var_os(BIN_ENV) {
        let path = PathBuf::from(&configured);
        if is_executable(&path) {
            return Ok(path);
        }
        return Err(format!(
            "{BIN_ENV} points at {}, which is not an executable file. \
             Point it at the `{PROGRAM}` binary or unset it to search PATH.",
            path.display()
        ));
    }

    match search_path(PROGRAM) {
        Some(path) => Ok(path),
        None => Err(format!(
            "no `{PROGRAM}` on PATH and {BIN_ENV} is unset. \
             Build it with `cargo build -p ui-box` and set {BIN_ENV} to the binary, \
             or put it on the PATH of the process that launches this MCP server."
        )),
    }
}

fn search_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    if path.as_os_str() == OsStr::new("") {
        return false;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
