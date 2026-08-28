use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::builder::Built;
use crate::host::{self, Mediator};
use crate::lab::Lab;
use crate::scan;
use crate::BuildRequest;

pub fn ship(
    lab: &Lab,
    target: &str,
    req: &BuildRequest,
    built: &Built,
    via: &Mediator,
) -> Result<PathBuf> {
    if lab.name == target {
        return Ok(built.artifact.clone());
    }

    if built.from_nix {
        let root = built.root.to_string_lossy().to_string();
        host::nix_copy(via, &lab.name, target, &[root])?;
        return Ok(built.artifact.clone());
    }

    let refs = scan::store_refs(lab, &built.artifact)?;
    host::nix_copy(via, &lab.name, target, &refs)?;

    let name = built
        .artifact
        .file_name()
        .map(|raw| raw.to_string_lossy().to_string())
        .unwrap_or_else(|| req.project.clone());

    let remote_path = host::home(target)?
        .join(".cache/ui-box/artifacts")
        .join(&req.project)
        .join(&name);

    match via {
        Mediator::Over(_) => host::relay(via, &lab.name, &built.artifact, target, &remote_path)?,
        Mediator::Here => stage_here(lab, target, built, &name, &remote_path)?,
    }

    Ok(remote_path)
}

fn stage_here(
    lab: &Lab,
    target: &str,
    built: &Built,
    name: &str,
    remote_path: &Path,
) -> Result<()> {
    let staging = std::env::temp_dir().join(format!("ui-box-place-{}", std::process::id()));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("could not create {}", staging.display()))?;

    let local_path = staging.join(name);
    let outcome = lab
        .pull(&built.artifact, &local_path)
        .and_then(|_| host::send_file(target, &local_path, remote_path));

    let _ = std::fs::remove_dir_all(&staging);
    outcome
}
