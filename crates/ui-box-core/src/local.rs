use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::backend::{Backend, Cmd, Output};
use crate::spec::BackendSpec;

#[derive(Debug, Clone, Default)]
pub struct LocalBackend;

impl LocalBackend {
    pub fn new() -> Self {
        LocalBackend
    }
}

impl Backend for LocalBackend {
    fn spec(&self) -> BackendSpec {
        BackendSpec::Local
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        let mut command = Command::new(cmd.program());
        command.args(&cmd.argv[1..]);
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        let output = command
            .output()
            .with_context(|| format!("cannot run `{}` locally", cmd.display()))?;
        Ok(Output {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn push(&self, local: &Path, remote: &Path) -> Result<()> {
        copy_path(local, remote)
    }

    fn pull(&self, remote: &Path, local: &Path) -> Result<()> {
        copy_path(remote, local)
    }
}

pub fn copy_path(src: &Path, dst: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(src).with_context(|| format!("cannot stat {}", src.display()))?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(dst).with_context(|| format!("cannot create {}", dst.display()))?;
        for entry in
            std::fs::read_dir(src).with_context(|| format!("cannot read {}", src.display()))?
        {
            let entry = entry?;
            copy_path(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if dst.exists() {
        std::fs::remove_file(dst).with_context(|| format!("cannot replace {}", dst.display()))?;
    }
    std::fs::copy(src, dst)
        .with_context(|| format!("cannot copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}
