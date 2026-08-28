use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use ui_box_core::{Backend, Cmd, Output};

use crate::sh;

pub trait RunOutcome {
    fn succeeded(&self) -> bool;
    fn line(&self) -> String;
    fn combined(&self) -> String;
}

impl RunOutcome for Output {
    fn succeeded(&self) -> bool {
        self.code == 0
    }

    fn line(&self) -> String {
        self.stdout.trim().to_string()
    }

    fn combined(&self) -> String {
        let mut merged = String::new();
        if !self.stdout.trim().is_empty() {
            merged.push_str(self.stdout.trim_end());
        }
        if !self.stderr.trim().is_empty() {
            if !merged.is_empty() {
                merged.push('\n');
            }
            merged.push_str(self.stderr.trim_end());
        }
        merged
    }
}

pub struct Lab<'a> {
    backend: &'a dyn Backend,
    pub name: String,
}

impl<'a> Lab<'a> {
    pub fn new(backend: &'a dyn Backend, name: &str) -> Self {
        Lab {
            backend,
            name: name.to_string(),
        }
    }

    pub fn run(&self, script: &str) -> Result<Output> {
        self.backend
            .run(&Cmd::shell(script))
            .with_context(|| format!("{} could not be reached", self.name))
    }

    pub fn capture(&self, script: &str) -> Result<String> {
        let output = self.run(script)?;
        if !output.succeeded() {
            bail!("{} failed on {}:\n{}", script, self.name, output.combined());
        }
        Ok(output.line())
    }

    pub fn pull(&self, remote: &Path, local: &Path) -> Result<()> {
        self.backend
            .pull(remote, local)
            .with_context(|| format!("could not fetch {} from {}", remote.display(), self.name))
    }

    pub fn home(&self) -> Result<PathBuf> {
        let home = self.capture("printf %s \"${HOME:-}\"")?;
        if home.is_empty() {
            bail!("{} did not report a home directory", self.name);
        }
        Ok(PathBuf::from(home))
    }

    pub fn project_dir(&self) -> Result<PathBuf> {
        let declared = self.capture("printf %s \"${DLAB_PROJECT:-}\"")?;
        if !declared.is_empty() {
            return Ok(PathBuf::from(declared));
        }

        let home = self.capture("printf %s \"${HOME:-}\"")?;
        if home.is_empty() {
            bail!(
                "{} exposes neither DLAB_PROJECT nor HOME, so the checkout under test cannot be located",
                self.name
            );
        }
        Ok(PathBuf::from(home))
    }

    pub fn exists(&self, path: &Path) -> Result<bool> {
        Ok(self
            .run(&format!("test -e {}", sh::quote_path(path)))?
            .succeeded())
    }
}
