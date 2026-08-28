use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::provenance::Source;
use crate::sh;
use crate::Provenance;

const DEFAULT_ARTIFACTS: &str = ".uibox/runs";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub provenance: Provenance,
    pub remote_path: PathBuf,
    pub lab: String,
    pub target: String,
}

impl Record {
    pub fn matches(&self, source: &Source, lab: &str, target: &str) -> bool {
        self.provenance.git_sha == source.git_sha
            && self.provenance.diff_hash == source.diff_hash
            && self.lab == lab
            && self.target == target
    }
}

pub struct CacheStore {
    path: PathBuf,
}

impl CacheStore {
    pub fn open(project: &str) -> Result<Self> {
        let artifacts = std::env::var("UIBOX_ARTIFACTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ARTIFACTS.to_string());

        let path = Path::new(&artifacts)
            .join(".pipeline")
            .join(format!("{}.json", sh::slug(project)));

        Ok(CacheStore { path })
    }

    pub fn load(&self) -> Result<Option<Record>> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("could not read {}", self.path.display()))
            }
        };
        Ok(serde_json::from_str(&raw).ok())
    }

    pub fn save(&self, record: &Record) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }

        let staged = self.path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(record)?;
        std::fs::write(&staged, encoded)
            .with_context(|| format!("could not write {}", staged.display()))?;
        std::fs::rename(&staged, &self.path)
            .with_context(|| format!("could not update {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            provenance: Provenance {
                git_sha: "abc".to_string(),
                diff_hash: "def".to_string(),
                artifact_hash: "ghi".to_string(),
            },
            remote_path: PathBuf::from("/nix/store/x-app/bin/app"),
            lab: "dlab-archtex".to_string(),
            target: "dlab-ui".to_string(),
        }
    }

    fn source(git_sha: &str, diff_hash: &str) -> Source {
        Source {
            git_sha: git_sha.to_string(),
            diff_hash: diff_hash.to_string(),
        }
    }

    #[test]
    fn accepts_the_same_tree_on_the_same_pair_of_labs() {
        assert!(record().matches(&source("abc", "def"), "dlab-archtex", "dlab-ui"));
    }

    #[test]
    fn rejects_a_dirty_tree_at_the_same_commit() {
        assert!(!record().matches(&source("abc", "changed"), "dlab-archtex", "dlab-ui"));
    }

    #[test]
    fn rejects_a_different_target_lab() {
        assert!(!record().matches(&source("abc", "def"), "dlab-archtex", "dlab-nsql"));
    }
}
