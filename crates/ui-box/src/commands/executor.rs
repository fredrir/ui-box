use std::path::PathBuf;

use anyhow::Result;

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
        }
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
        let artifacts = SnapArtifacts {
            name: written.clone(),
            mode: mode.to_string(),
            text_path: snap.txt_path.as_ref().map(PathBuf::from),
            png_path: snap.png_path.as_ref().map(PathBuf::from),
            text_bytes: snap.text.as_ref().map(String::len).unwrap_or_default(),
            console: snap.console.len(),
            network: snap.network.len(),
        };
        if mode.wants_text() && artifacts.text_path.is_none() {
            note!("driver wrote no text file for snapshot {written}");
        }
        if mode.wants_png() && artifacts.png_path.is_none() {
            note!("driver wrote no png for snapshot {written}");
        }
        for path in [&artifacts.text_path, &artifacts.png_path]
            .into_iter()
            .flatten()
        {
            if !path.starts_with(&self.run.path) {
                note!("driver wrote {} outside the run directory", path.display());
            }
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
