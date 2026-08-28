use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

use crate::lab::{Lab, RunOutcome};
use crate::sh;
use crate::BuildRequest;

pub struct Built {
    pub root: PathBuf,
    pub artifact: PathBuf,
    pub from_nix: bool,
}

pub fn build(lab: &Lab, project_dir: &Path, req: &BuildRequest) -> Result<Built> {
    let (root, from_nix) = match &req.build {
        Some(command) => (native(lab, project_dir, command)?, false),
        None => (nix(lab, project_dir, req)?, true),
    };

    let artifact = if req.artifact.is_absolute() {
        req.artifact.clone()
    } else {
        root.join(&req.artifact)
    };

    if !lab.exists(&artifact)? {
        bail!(
            "the build succeeded but produced no {} on {}",
            artifact.display(),
            lab.name
        );
    }

    Ok(Built {
        root,
        artifact,
        from_nix,
    })
}

fn native(lab: &Lab, project_dir: &Path, command: &str) -> Result<PathBuf> {
    let run = lab.run(&sh::in_dir(project_dir, command))?;
    if !run.succeeded() {
        return Err(anyhow!("{}", run.combined()));
    }
    Ok(project_dir.to_path_buf())
}

fn nix(lab: &Lab, project_dir: &Path, req: &BuildRequest) -> Result<PathBuf> {
    let attr = std::env::var("UIBOX_NIX_ATTR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| req.project.clone());

    let mut run = lab.run(&nix_build(project_dir, &format!("path:.#{}", attr)))?;
    if !run.succeeded() && run.stderr.contains("does not provide attribute") {
        run = lab.run(&nix_build(project_dir, "path:."))?;
    }
    if !run.succeeded() {
        return Err(anyhow!("{}", run.combined()));
    }

    run.stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("/nix/store/"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow!(
                "nix build printed no output path for {}:\n{}",
                req.project,
                run.combined()
            )
        })
}

fn nix_build(project_dir: &Path, installable: &str) -> String {
    sh::in_dir(
        project_dir,
        &format!(
            "nix build --no-link --no-write-lock-file --print-out-paths {}",
            sh::quote(installable)
        ),
    )
}
