use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Surface, Viewport};
use crate::driver::client::process_alive;
use crate::driver::Connection;
use crate::error::SessionError;
use crate::run::now_unix;

pub const RECORD: &str = "session.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub driver_session: String,
    pub driver_name: String,
    pub driver_argv: Vec<String>,
    pub pid: u32,
    pub surface: Surface,
    pub target: String,
    pub viewport: Viewport,
    pub backend: String,
    pub run_dir: PathBuf,
    pub session_dir: PathBuf,
    pub created_unix: u64,
    pub last_used_unix: u64,
    pub ttl_secs: u64,
    pub step_count: usize,
}

impl SessionRecord {
    pub fn idle_secs(&self) -> u64 {
        now_unix().saturating_sub(self.last_used_unix)
    }

    pub fn expires_in(&self) -> u64 {
        self.ttl_secs.saturating_sub(self.idle_secs())
    }

    pub fn expired(&self) -> bool {
        self.idle_secs() > self.ttl_secs
    }

    pub fn ensure_usable(&self) -> Result<()> {
        if self.expired() {
            return Err(SessionError::Expired {
                id: self.id.clone(),
                idle_secs: self.idle_secs(),
                ttl_secs: self.ttl_secs,
                run_dir: self.run_dir.display().to_string(),
            }
            .into());
        }
        if !process_alive(self.pid) {
            return Err(SessionError::DriverGone {
                id: self.id.clone(),
                pid: self.pid,
                run_dir: self.run_dir.display().to_string(),
            }
            .into());
        }
        Ok(())
    }

    pub fn touch(&mut self) {
        self.last_used_unix = now_unix();
    }

    pub fn connect(&self, timeout: Duration) -> Result<Connection> {
        Connection::attach(&self.session_dir, &self.driver_name, self.pid, timeout)
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(config: &Config) -> SessionStore {
        SessionStore {
            root: config.sessions_dir(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn create_dir(&self, id: &str) -> Result<PathBuf> {
        let dir = self.dir(id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create session directory {}", dir.display()))?;
        Ok(dir)
    }

    pub fn save(&self, record: &SessionRecord) -> Result<()> {
        let dir = self.create_dir(&record.id)?;
        let path = dir.join(RECORD);
        let rendered = serde_json::to_string_pretty(record)?;
        std::fs::write(&path, rendered)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<SessionRecord> {
        let path = self.dir(id).join(RECORD);
        if !path.is_file() {
            return Err(SessionError::Unknown {
                id: id.to_string(),
                store: self.root.display().to_string(),
            }
            .into());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let dir = self.dir(id);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("cannot remove {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if let Ok(record) = self.load(&id) {
                out.push(record);
            }
        }
        out.sort_by_key(|record| std::cmp::Reverse(record.created_unix));
        Ok(out)
    }
}
