use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::backend::{shell_quote, Backend, Cmd, Output};
use crate::error::BackendFailure;
use crate::spec::BackendSpec;

const SSH_TRANSPORT_FAILURE: i32 = 255;

const VALUE_FLAGS: &str = "bcDEeFIiJLlmOopQRSWw";

#[derive(Debug, Clone)]
pub struct SshBackend {
    spec: BackendSpec,
    target: String,
    force: bool,
    options: Vec<String>,
    hop: Option<String>,
    forced: Arc<Once>,
}

impl SshBackend {
    pub fn new(spec: BackendSpec, force: bool) -> Result<Self> {
        let Some(target) = spec.ssh_target() else {
            bail!("ssh backend requires a host, got {}", spec.url());
        };
        let hop = if force { proxy_hop(&target) } else { None };
        Ok(SshBackend {
            spec,
            target,
            force,
            options: ssh_options(),
            hop,
            forced: Arc::new(Once::new()),
        })
    }

    pub fn hop(&self) -> Option<&str> {
        self.hop.as_deref()
    }

    fn ensure_forced(&self) {
        if !self.force {
            return;
        }
        self.forced.call_once(|| {
            force_start(&self.target, self.hop.as_deref());
        });
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    fn script(&self, cmd: &Cmd) -> String {
        let mut parts = Vec::new();
        if let Some(dir) = &cmd.cwd {
            parts.push(format!("cd {} &&", shell_quote(&dir.display().to_string())));
        }
        for (key, value) in self.environment(cmd) {
            parts.push(format!("{}={}", key, shell_quote(&value)));
        }
        for arg in &cmd.argv {
            parts.push(shell_quote(arg));
        }
        parts.join(" ")
    }

    fn environment(&self, cmd: &Cmd) -> Vec<(String, String)> {
        let mut env = cmd.env.clone();
        if self.force && !env.iter().any(|(key, _)| key == "UIBOX_FORCE") {
            env.push(("UIBOX_FORCE".to_string(), "1".to_string()));
        }
        env
    }

    fn ssh_command(&self) -> Command {
        let mut command = Command::new("ssh");
        command.args(&self.options);
        command.arg(&self.target);
        command
    }

    fn remote_shell(&self) -> String {
        let mut parts = vec!["ssh".to_string()];
        parts.extend(self.options.iter().cloned());
        parts.join(" ")
    }

    fn invoke(&self, mut command: Command, context: String) -> Result<Output> {
        if self.force {
            command.env("UIBOX_FORCE", "1");
        }
        let output = command
            .output()
            .with_context(|| format!("cannot spawn `{}`", context))?;
        let result = Output {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        if result.code == SSH_TRANSPORT_FAILURE {
            return Err(anyhow::Error::new(BackendFailure {
                context,
                code: result.code,
                stderr: result.stderr,
            }));
        }
        Ok(result)
    }
}

impl Backend for SshBackend {
    fn spec(&self) -> BackendSpec {
        self.spec.clone()
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        self.ensure_forced();
        let script = self.script(cmd);
        let mut command = self.ssh_command();
        command.arg(&script);
        self.invoke(command, format!("ssh {} {}", self.target, script))
    }

    fn push(&self, local: &Path, remote: &Path) -> Result<()> {
        self.ensure_forced();
        if !local.exists() {
            bail!("cannot push {}: no such path", local.display());
        }
        if let Some(parent) = remote.parent() {
            self.require(
                &Cmd::new("mkdir")
                    .arg("-p")
                    .arg(parent.display().to_string()),
            )?;
        }
        let source = source_path(local);
        let destination = format!("{}:{}", self.target, remote.display());
        self.transfer(&source, &destination)
    }

    fn pull(&self, remote: &Path, local: &Path) -> Result<()> {
        self.ensure_forced();
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let source = format!("{}:{}", self.target, remote.display());
        self.transfer(&source, &local.display().to_string())
    }
}

impl SshBackend {
    fn transfer(&self, source: &str, destination: &str) -> Result<()> {
        let (command, context) = if which("rsync").is_some() {
            let mut command = Command::new("rsync");
            command
                .arg("-a")
                .arg("--mkpath")
                .arg("-e")
                .arg(self.remote_shell())
                .arg(source)
                .arg(destination);
            (command, format!("rsync -a {source} {destination}"))
        } else {
            let mut command = Command::new("scp");
            command
                .arg("-r")
                .args(&self.options)
                .arg(source)
                .arg(destination);
            (command, format!("scp -r {source} {destination}"))
        };
        let output = self.invoke(command, context.clone())?;
        if output.ok() {
            return Ok(());
        }
        if output.stderr.contains("--mkpath") || output.stderr.contains("unknown option") {
            let mut fallback = Command::new("rsync");
            fallback
                .arg("-a")
                .arg("-e")
                .arg(self.remote_shell())
                .arg(source)
                .arg(destination);
            let output = self.invoke(fallback, context.clone())?;
            if output.ok() {
                return Ok(());
            }
            return Err(anyhow::Error::new(BackendFailure {
                context,
                code: output.code,
                stderr: output.stderr,
            }));
        }
        Err(anyhow::Error::new(BackendFailure {
            context,
            code: output.code,
            stderr: output.stderr,
        }))
    }
}

fn source_path(local: &Path) -> String {
    let rendered = local.display().to_string();
    if local.is_dir() && !rendered.ends_with('/') {
        format!("{rendered}/")
    } else {
        rendered
    }
}

pub fn ssh_options() -> Vec<String> {
    if let Ok(raw) = std::env::var("UIBOX_SSH_OPTS") {
        if !raw.trim().is_empty() {
            return raw.split_whitespace().map(str::to_string).collect();
        }
    }
    vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "LogLevel=ERROR".to_string(),
    ]
}

pub fn wake(lab: &str, force: bool, wait: Duration) -> (String, Option<String>) {
    let log = std::env::temp_dir().join(format!("uibox-wake-{}-{}.log", lab, std::process::id()));
    let hop = if force { proxy_hop(lab) } else { None };
    let mut command = wake_command(lab, force, hop.as_deref());
    command.stdin(Stdio::null()).stdout(Stdio::null());
    match File::create(&log) {
        Ok(file) => {
            command.stderr(Stdio::from(file));
        }
        Err(_) => {
            command.stderr(Stdio::null());
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return ("unavailable".to_string(), Some(err.to_string())),
    };
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                std::fs::remove_file(&log).ok();
                return ("reachable".to_string(), None);
            }
            Ok(Some(_)) => {
                let detail = std::fs::read_to_string(&log)
                    .ok()
                    .map(|text| text.trim_end_matches('\n').to_string());
                std::fs::remove_file(&log).ok();
                return ("refused".to_string(), detail);
            }
            Ok(None) if Instant::now() >= deadline => return ("waking".to_string(), None),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => return ("waking".to_string(), Some(err.to_string())),
        }
    }
}

fn wake_command(lab: &str, force: bool, hop: Option<&str>) -> Command {
    let mut command = Command::new("ssh");
    command.args(ssh_options());
    match hop {
        Some(hop) => {
            command.arg(hop).arg(forced_probe(lab));
        }
        None => {
            if force {
                command.env("UIBOX_FORCE", "1");
            }
            command.arg(lab).arg("true");
        }
    }
    command
}

fn forced_probe(lab: &str) -> String {
    format!(
        "UIBOX_FORCE=1 ssh -o BatchMode=yes -o LogLevel=ERROR {} true",
        shell_quote(lab)
    )
}

fn force_start(target: &str, hop: Option<&str>) {
    let _ = wake_command(target, true, hop)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn proxy_hop(host: &str) -> Option<String> {
    let output = Command::new("ssh").arg("-G").arg(host).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_proxy_hop(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_proxy_hop(resolved: &str) -> Option<String> {
    let line = resolved
        .lines()
        .map(str::trim_start)
        .find(|line| line.to_ascii_lowercase().starts_with("proxycommand "))?;
    let command = line.split_once(char::is_whitespace)?.1.trim();
    hop_of(command)
}

fn hop_of(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace();
    let program = tokens.next()?;
    if program.rsplit('/').next().unwrap_or(program) != "ssh" {
        return None;
    }
    let mut expecting_value = false;
    for token in tokens {
        if expecting_value {
            expecting_value = false;
            continue;
        }
        match token.strip_prefix('-') {
            Some(flag) => {
                if flag.len() == 1 && VALUE_FLAGS.contains(flag) {
                    expecting_value = true;
                }
            }
            None => return Some(token.to_string()),
        }
    }
    None
}

pub fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(force: bool) -> SshBackend {
        SshBackend::new(
            BackendSpec::Ssh {
                user: Some("fredrir".into()),
                host: "ui-box-backend".into(),
            },
            force,
        )
        .unwrap()
    }

    #[test]
    fn builds_quoted_remote_script() {
        let cmd = Cmd::new("uibox-lab")
            .arg("start")
            .arg("my lab")
            .cwd("/srv/labs");
        assert_eq!(
            backend(false).script(&cmd),
            "cd /srv/labs && uibox-lab start 'my lab'"
        );
    }

    #[test]
    fn force_sets_uibox_force() {
        let script = backend(true).script(&Cmd::new("true"));
        assert_eq!(script, "UIBOX_FORCE=1 true");
    }

    #[test]
    fn force_is_absent_without_the_flag() {
        assert_eq!(backend(false).script(&Cmd::new("true")), "true");
    }

    const RESOLVED: &str = "host ui-box-backend\nuser fredrir\nhostname ui-box-backend\nproxycommand ssh -T -o LogLevel=ERROR archie /home/fredrir/packages/ui-box/backend/bin/ui-box-wake %h %p\n";

    #[test]
    fn finds_the_hop_the_wake_proxy_runs_on() {
        assert_eq!(parse_proxy_hop(RESOLVED).as_deref(), Some("archie"));
    }

    #[test]
    fn treats_a_local_proxy_as_no_hop() {
        let resolved = "proxycommand /home/fredrir/packages/ui-box/backend/bin/ui-box-wake %h %p\n";
        assert_eq!(parse_proxy_hop(resolved), None);
        assert_eq!(parse_proxy_hop("hostname archie\n"), None);
        assert_eq!(parse_proxy_hop("proxycommand none\n"), None);
    }

    #[test]
    fn skips_ssh_flags_that_carry_a_value() {
        let resolved =
            "proxycommand ssh -q -i /tmp/key -oLogLevel=ERROR -p 2222 gateway nc %h %p\n";
        assert_eq!(parse_proxy_hop(resolved).as_deref(), Some("gateway"));
    }

    #[test]
    fn forced_probe_sets_the_variable_where_the_proxy_reads_it() {
        assert_eq!(
            forced_probe("ui-box-backend"),
            "UIBOX_FORCE=1 ssh -o BatchMode=yes -o LogLevel=ERROR ui-box-backend true"
        );
    }
}
