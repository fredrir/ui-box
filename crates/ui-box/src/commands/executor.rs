use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::backend::Backend;
use crate::driver::Connection;
use crate::flow::{SnapMode, SnapStep, Step};
use crate::note;
use crate::run::{RunDir, CONSOLE, NETWORK};

#[derive(Debug, Clone, Default)]
pub struct SnapArtifacts {
    pub name: String,
    pub mode: String,
    pub text_path: Option<PathBuf>,
    pub png_path: Option<PathBuf>,
    pub text_bytes: usize,
    pub console: usize,
    pub network: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StepOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub snap: Option<SnapArtifacts>,
}

pub struct Executor {
    pub conn: Connection,
    pub run: RunDir,
    pub driver_session: String,
    pub default_mode: SnapMode,
    pub total: usize,
    pub failed: usize,
    pub backend: Option<Box<dyn Backend>>,
}

impl Executor {
    pub fn new(conn: Connection, run: RunDir, driver_session: String, start_index: usize) -> Self {
        Executor {
            conn,
            run,
            driver_session,
            default_mode: SnapMode::Text,
            total: start_index,
            failed: 0,
            backend: None,
        }
    }

    pub fn pulling_from(mut self, backend: Box<dyn Backend>) -> Self {
        self.backend = Some(backend);
        self
    }

    fn localise(&self, remote: Option<&String>) -> Result<Option<PathBuf>> {
        let Some(remote) = remote else {
            return Ok(None);
        };
        let remote = PathBuf::from(remote);
        let Some(backend) = &self.backend else {
            if !remote.is_file() {
                return Ok(Some(remote));
            }
            if !remote.starts_with(&self.run.path) {
                note!(
                    "driver wrote {} outside the run directory",
                    remote.display()
                );
            }
            return Ok(Some(remote));
        };
        let name = remote.file_name().with_context(|| {
            format!(
                "driver returned {}, which has no file name",
                remote.display()
            )
        })?;
        let local = self.run.snaps_dir().join(name);
        std::fs::create_dir_all(self.run.snaps_dir())?;
        backend
            .pull(&remote, &local)
            .with_context(|| format!("cannot fetch {} from {}", remote.display(), backend.url()))?;
        if !local.is_file() {
            bail!(
                "fetched {} from {} but nothing arrived at {}",
                remote.display(),
                backend.url(),
                local.display()
            );
        }
        Ok(Some(local))
    }

    pub fn perform(&mut self, step: &Step) -> Result<StepOutcome> {
        let mut recorded = self.resolve(step);
        let result = self.dispatch(&recorded);
        if let (Step::Snap(snap), Ok(outcome)) = (&recorded, &result) {
            if let Some(artifacts) = &outcome.snap {
                if snap.name() != Some(artifacts.name.as_str()) {
                    recorded = Step::Snap(SnapStep::Detail {
                        name: Some(artifacts.name.clone()),
                        mode: snap.mode(),
                    });
                }
            }
        }
        self.total += 1;
        match self.run.append_step(&recorded) {
            Ok(()) => {}
            Err(err) if result.is_ok() => return Err(err),
            Err(err) => note!(
                "cannot record step in {}: {err}",
                self.run.steps_path().display()
            ),
        }
        match result {
            Ok(outcome) => {
                if !outcome.ok {
                    self.failed += 1;
                }
                Ok(outcome)
            }
            Err(err) => {
                self.failed += 1;
                Err(err)
            }
        }
    }

    fn resolve(&self, step: &Step) -> Step {
        match step {
            Step::Snap(snap) => {
                let name = snap
                    .name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("step-{:03}", self.total + 1));
                let mode = snap.mode().unwrap_or(self.default_mode);
                Step::Snap(SnapStep::Detail {
                    name: Some(name),
                    mode: Some(mode),
                })
            }
            other => other.clone(),
        }
    }

    fn dispatch(&mut self, step: &Step) -> Result<StepOutcome> {
        if let Step::Snap(snap) = step {
            let name = snap.name().unwrap_or("snapshot").to_string();
            let mode = snap.mode().unwrap_or(self.default_mode);
            let artifacts = self.snapshot(&name, mode)?;
            return Ok(StepOutcome {
                ok: true,
                error: None,
                snap: Some(artifacts),
            });
        }
        let payload = step.to_json()?;
        let result = self.conn.act(&self.driver_session, &payload)?;
        Ok(StepOutcome {
            ok: result.ok,
            error: result.error_text(),
            snap: None,
        })
    }

    pub fn snapshot(&mut self, name: &str, mode: SnapMode) -> Result<SnapArtifacts> {
        let snap = self.conn.snap(&self.driver_session, mode.as_str(), name)?;
        let written = snap.name.clone().unwrap_or_else(|| name.to_string());
        if written != name {
            note!("driver stored snapshot {name} as {written}");
        }
        let text_path = self.localise(snap.txt_path.as_ref())?;
        let png_path = self.localise(snap.png_path.as_ref())?;
        let artifacts = SnapArtifacts {
            name: written.clone(),
            mode: mode.to_string(),
            text_bytes: text_bytes(snap.text.as_deref(), text_path.as_deref()),
            text_path,
            png_path,
            console: snap.console.len(),
            network: snap.network.len(),
        };
        if mode.wants_text() && artifacts.text_path.is_none() {
            note!("driver wrote no text file for snapshot {written}");
        }
        if mode.wants_png() && artifacts.png_path.is_none() {
            note!("driver wrote no png for snapshot {written}");
        }
        self.run.append_events(CONSOLE, &snap.console)?;
        self.run.append_events(NETWORK, &snap.network)?;
        Ok(artifacts)
    }

    pub fn close(&mut self) {
        if let Err(err) = self.conn.close(&self.driver_session) {
            note!("driver did not confirm close: {err}");
        }
    }
}

pub fn text_bytes(inline: Option<&str>, written: Option<&Path>) -> usize {
    if let Some(text) = inline {
        return text.len();
    }
    written
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() as usize)
        .unwrap_or_default()
}

pub fn snap_json(artifacts: &SnapArtifacts) -> serde_json::Value {
    serde_json::json!({
        "name": artifacts.name,
        "mode": artifacts.mode,
        "text": artifacts.text_path.as_ref().map(|p| p.display().to_string()),
        "png": artifacts.png_path.as_ref().map(|p| p.display().to_string()),
        "text_bytes": artifacts.text_bytes,
        "console": artifacts.console,
        "network": artifacts.network,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_text_measures_the_response() {
        assert_eq!(text_bytes(Some("hello"), None), 5);
        assert_eq!(text_bytes(Some(""), None), 0);
    }

    #[test]
    fn a_driver_that_only_writes_the_file_still_reports_bytes() {
        let path = std::env::temp_dir().join("uibox-text-bytes-probe.txt");
        std::fs::write(&path, "accessibility tree\n").unwrap();
        assert_eq!(text_bytes(None, Some(&path)), 19);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn no_text_anywhere_is_zero() {
        let missing = std::env::temp_dir().join("uibox-text-bytes-absent.txt");
        assert_eq!(text_bytes(None, Some(&missing)), 0);
        assert_eq!(text_bytes(None, None), 0);
    }
}
