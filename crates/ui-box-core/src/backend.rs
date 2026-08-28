use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::error::BackendFailure;
use crate::spec::BackendSpec;

#[derive(Debug, Clone, Default)]
pub struct Cmd {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Cmd {
    pub fn new(program: impl Into<String>) -> Self {
        Cmd {
            argv: vec![program.into()],
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn shell(script: impl Into<String>) -> Self {
        Cmd::new("sh").arg("-c").arg(script)
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn program(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or_default()
    }

    pub fn display(&self) -> String {
        self.argv.join(" ")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    pub fn trimmed_stdout(&self) -> &str {
        self.stdout.trim()
    }
}

pub trait Backend: Send + Sync {
    fn spec(&self) -> BackendSpec;

    fn run(&self, cmd: &Cmd) -> Result<Output>;

    fn push(&self, local: &Path, remote: &Path) -> Result<()>;

    fn pull(&self, remote: &Path, local: &Path) -> Result<()>;

    fn url(&self) -> String {
        self.spec().url()
    }

    fn is_local(&self) -> bool {
        matches!(self.spec(), BackendSpec::Local)
    }

    fn require(&self, cmd: &Cmd) -> Result<Output> {
        let output = self.run(cmd)?;
        if output.ok() {
            return Ok(output);
        }
        Err(anyhow::Error::new(BackendFailure {
            context: cmd.display(),
            code: output.code,
            stderr: output.stderr,
        }))
    }
}

pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@' | ',')
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}
