use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Surface, Viewport};
use crate::flow::Step;

pub const META: &str = "meta.json";
pub const STEPS: &str = "steps.yaml";
pub const CONSOLE: &str = "console.jsonl";
pub const NETWORK: &str = "network.jsonl";
pub const REPORT: &str = "report.json";
pub const SNAPS: &str = "snaps";
pub const DIFFS: &str = "diff";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub run: String,
    pub project: Option<String>,
    pub lab: Option<String>,
    pub backend: String,
    pub surface: Surface,
    pub git_sha: Option<String>,
    pub diff_hash: Option<String>,
    pub artifact_hash: Option<String>,
    pub started: String,
    pub ended: Option<String>,
    pub verdict: String,
    pub steps_total: usize,
    pub steps_failed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,
}

impl Meta {
    pub fn new(run: &str, backend: &str, surface: Surface) -> Meta {
        Meta {
            run: run.to_string(),
            project: None,
            lab: None,
            backend: backend.to_string(),
            surface,
            git_sha: git_sha(),
            diff_hash: None,
            artifact_hash: None,
            started: now_iso(),
            ended: None,
            verdict: "open".to_string(),
            steps_total: 0,
            steps_failed: 0,
            flow: None,
            target: None,
            viewport: None,
            source: None,
            remote_path: None,
            cached: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunDir {
    pub id: String,
    pub path: PathBuf,
}

impl RunDir {
    pub fn create(artifacts: &Path) -> Result<RunDir> {
        let id = new_run_id();
        let path = artifacts.join(&id);
        std::fs::create_dir_all(path.join(SNAPS))
            .with_context(|| format!("cannot create run directory {}", path.display()))?;
        std::fs::create_dir_all(path.join(DIFFS))
            .with_context(|| format!("cannot create run directory {}", path.display()))?;
        Ok(RunDir { id, path })
    }

    pub fn open(artifacts: &Path, id: &str) -> Result<RunDir> {
        let path = artifacts.join(id);
        if !path.is_dir() {
            bail!("no run {id} in {}", artifacts.display());
        }
        Ok(RunDir {
            id: id.to_string(),
            path,
        })
    }

    pub fn steps_path(&self) -> PathBuf {
        self.path.join(STEPS)
    }

    pub fn meta_path(&self) -> PathBuf {
        self.path.join(META)
    }

    pub fn report_path(&self) -> PathBuf {
        self.path.join(REPORT)
    }

    pub fn snaps_dir(&self) -> PathBuf {
        self.path.join(SNAPS)
    }

    pub fn diffs_dir(&self) -> PathBuf {
        self.path.join(DIFFS)
    }

    pub fn append_step(&self, step: &Step) -> Result<()> {
        let entry = step.to_yaml_entry()?;
        let path = self.steps_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("cannot append to {}", path.display()))?;
        file.write_all(entry.as_bytes())
            .with_context(|| format!("cannot append to {}", path.display()))?;
        file.sync_data().ok();
        Ok(())
    }

    pub fn read_steps(&self) -> Result<Vec<Step>> {
        let path = self.steps_path();
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_yaml::from_str(&raw).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn append_events(&self, file: &str, entries: &[Value]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.path.join(file);
        let mut handle = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("cannot append to {}", path.display()))?;
        for entry in entries {
            let line = serde_json::to_string(entry)?;
            handle.write_all(line.as_bytes())?;
            handle.write_all(b"\n")?;
        }
        Ok(())
    }

    pub fn write_meta(&self, meta: &Meta) -> Result<()> {
        let path = self.meta_path();
        let rendered = serde_json::to_string_pretty(meta)?;
        std::fs::write(&path, rendered)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    pub fn read_meta(&self) -> Result<Meta> {
        let path = self.meta_path();
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn snapshots(&self) -> Result<Vec<PathBuf>> {
        let dir = self.snaps_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect();
        out.sort();
        Ok(out)
    }
}

pub fn list_runs(artifacts: &Path) -> Result<Vec<RunDir>> {
    if !artifacts.is_dir() {
        return Ok(Vec::new());
    }
    let mut runs: Vec<RunDir> = std::fs::read_dir(artifacts)
        .with_context(|| format!("cannot read {}", artifacts.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some(RunDir {
                id: name,
                path: entry.path(),
            })
        })
        .collect();
    runs.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(runs)
}

pub fn new_run_id() -> String {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{stamp}-{:08x}", hasher.finish() as u32)
}

pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

pub fn git_sha() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_match_the_contract_shape() {
        let id = new_run_id();
        let (stamp, hex) = id.split_once('-').expect("runid has two parts");
        assert_eq!(stamp.len(), 16, "{id}");
        assert!(stamp.ends_with('Z'), "{id}");
        assert_eq!(hex.len(), 8, "{id}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    }
}
