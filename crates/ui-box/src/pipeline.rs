use std::path::PathBuf;

use anyhow::Result;

use crate::backend::Backend;

#[derive(Debug, Clone)]
pub struct Request {
    pub project: String,
    pub lab: String,
    pub target: String,
    pub source: Option<PathBuf>,
    pub build: Option<String>,
    pub artifact: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Placement {
    pub remote_path: PathBuf,
    pub source: Option<PathBuf>,
    pub git_sha: Option<String>,
    pub diff_hash: Option<String>,
    pub artifact_hash: Option<String>,
    pub cached: bool,
}

#[cfg(feature = "pipeline")]
pub fn place(request: &Request, backend: &dyn Backend) -> Result<Placement> {
    let build_request = ui_box_pipeline::BuildRequest {
        project: request.project.clone(),
        lab: request.lab.clone(),
        target: request.target.clone(),
        source: request.source.clone(),
        build: request.build.clone(),
        artifact: request.artifact.clone(),
    };
    let placed = ui_box_pipeline::place(&build_request, backend)?;
    Ok(Placement {
        remote_path: placed.remote_path,
        source: request.source.clone(),
        git_sha: Some(placed.provenance.git_sha),
        diff_hash: Some(placed.provenance.diff_hash),
        artifact_hash: Some(placed.provenance.artifact_hash),
        cached: placed.cached,
    })
}

#[cfg(not(feature = "pipeline"))]
pub fn place(_request: &Request, _backend: &dyn Backend) -> Result<Placement> {
    anyhow::bail!(
        "this ui-box was built without the pipeline feature, so it cannot place an artifact. \
         crates/ui-box-pipeline was unavailable at build time; rebuild with --features pipeline, \
         or replay against a target that is already up with `ui-box run --no-place`"
    )
}

pub fn available() -> bool {
    cfg!(feature = "pipeline")
}
