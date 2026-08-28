mod builder;
mod cache;
mod host;
mod lab;
mod provenance;
mod scan;
mod sh;
mod ship;
mod sync;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use ui_box_core::Backend;

use crate::cache::{CacheStore, Record};
use crate::lab::Lab;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRequest {
    pub project: String,
    pub lab: String,
    pub target: String,
    pub source: Option<PathBuf>,
    pub build: Option<String>,
    pub artifact: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub git_sha: String,
    pub diff_hash: String,
    pub artifact_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placed {
    pub remote_path: PathBuf,
    pub provenance: Provenance,
    pub cached: bool,
}

enum Tree {
    Local(PathBuf),
    InLab(PathBuf),
}

pub fn place(req: &BuildRequest, backend: &dyn Backend) -> Result<Placed> {
    let target = req.target.trim().to_string();
    if target.is_empty() {
        bail!(
            "BuildRequest.target is empty, so there is no lab to place {} into",
            req.project
        );
    }

    let via = host::mediator(&target);
    let lab = Lab::new(backend, &req.lab);
    let store = CacheStore::open(&req.project)?;

    let tree = match &req.source {
        Some(root) => Tree::Local(root.clone()),
        None => Tree::InLab(lab.project_dir()?),
    };

    let source = match &tree {
        Tree::Local(root) => provenance::local_source(root)?,
        Tree::InLab(dir) => provenance::source(&lab, dir)?,
    };

    if let Some(record) = store.load()? {
        if record.matches(&source, &req.lab, &target) {
            host::wake(&via, &target)?;
            if host::path_exists(&target, &record.remote_path)? {
                return Ok(Placed {
                    remote_path: record.remote_path,
                    provenance: record.provenance,
                    cached: true,
                });
            }
        }
    }

    let build_dir = match &tree {
        Tree::Local(root) => sync::stage(&lab, &req.project, root)?,
        Tree::InLab(dir) => dir.clone(),
    };

    let built = builder::build(&lab, &build_dir, req)?;
    let provenance = Provenance {
        git_sha: source.git_sha,
        diff_hash: source.diff_hash,
        artifact_hash: provenance::artifact_hash(&lab, &built.artifact)?,
    };

    host::wake(&via, &target)?;
    let remote_path = ship::ship(&lab, &target, req, &built, &via)?;

    store.save(&Record {
        provenance: provenance.clone(),
        remote_path: remote_path.clone(),
        lab: req.lab.clone(),
        target,
    })?;

    Ok(Placed {
        remote_path,
        provenance,
        cached: false,
    })
}
