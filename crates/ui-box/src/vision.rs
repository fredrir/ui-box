use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::backend::which;
use crate::config::{find_dir_upwards, Config};

pub const PROGRAM: &str = "uibox-vision";
pub const SOURCE_DIR: &str = "tools/vision";

#[derive(Debug, Clone, Deserialize)]
pub struct DiffResult {
    pub differs: bool,
    #[serde(default)]
    pub pixels: u64,
    #[serde(default)]
    pub ratio: f64,
    #[serde(default)]
    pub size_mismatch: bool,
    #[serde(default)]
    pub golden_size: Option<Vec<u32>>,
    #[serde(default)]
    pub candidate_size: Option<Vec<u32>>,
}

#[derive(Debug, Clone)]
pub struct Vision {
    argv: Vec<String>,
}

impl Vision {
    pub fn locate(config: &Config) -> Option<Vision> {
        if let Some(command) = &config.vision {
            let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
            if !argv.is_empty() {
                return Some(Vision { argv });
            }
        }
        if let Some(path) = which(PROGRAM) {
            return Some(Vision {
                argv: vec![path.display().to_string()],
            });
        }
        for base in source_dirs(config) {
            let script = base.join(PROGRAM);
            if script.is_file() {
                return Some(Vision {
                    argv: vec![script.display().to_string()],
                });
            }
            let bin = base.join("bin").join(PROGRAM);
            if bin.is_file() {
                return Some(Vision {
                    argv: vec![bin.display().to_string()],
                });
            }
            if base.join("pyproject.toml").is_file() && which("uv").is_some() {
                return Some(Vision {
                    argv: vec![
                        "uv".to_string(),
                        "run".to_string(),
                        "--directory".to_string(),
                        base.display().to_string(),
                        PROGRAM.to_string(),
                    ],
                });
            }
        }
        None
    }

    pub fn require(config: &Config) -> Result<Vision> {
        Vision::locate(config).with_context(|| {
            format!(
                "{PROGRAM} is not on PATH; set UIBOX_VISION, or install {SOURCE_DIR} \
                 so `uv run` can reach it"
            )
        })
    }

    pub fn display(&self) -> String {
        self.argv.join(" ")
    }

    fn invoke(&self, args: &[String]) -> Result<Value> {
        let Some((program, leading)) = self.argv.split_first() else {
            bail!("{PROGRAM} was located with no command to run");
        };
        let mut command = Command::new(program);
        command.args(leading).args(args);
        let output = command
            .output()
            .with_context(|| format!("cannot run `{} {}`", self.display(), args.join(" ")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let verb = args.first().cloned().unwrap_or_default();
        let trimmed = stdout.trim();

        if trimmed.is_empty() {
            bail!("{PROGRAM} {verb} printed no json{}", suffix(&stderr));
        }
        let value: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("{PROGRAM} {verb} printed {trimmed}, which is not json"))?;
        if let Some(message) = value.get("error").and_then(Value::as_str) {
            let code = value.get("code").and_then(Value::as_str).unwrap_or("error");
            bail!("{PROGRAM} {verb} failed [{code}]: {message}");
        }
        if !output.status.success() {
            bail!(
                "{PROGRAM} {verb} exited {}{}",
                output.status.code().unwrap_or(-1),
                suffix(&stderr)
            );
        }
        Ok(value)
    }

    pub fn diff(&self, golden: &Path, candidate: &Path, out: &Path) -> Result<DiffResult> {
        let value = self.invoke(&[
            "diff".to_string(),
            "--golden".to_string(),
            golden.display().to_string(),
            "--candidate".to_string(),
            candidate.display().to_string(),
            "--out".to_string(),
            out.display().to_string(),
            "--json".to_string(),
        ])?;
        serde_json::from_value(value)
            .context("uibox-vision diff did not print {differs, pixels, ratio}")
    }

    pub fn golden_get(&self, store: &str, name: &str, out: &Path) -> Result<bool> {
        let value = self.invoke(&[
            "golden".to_string(),
            "get".to_string(),
            "--store".to_string(),
            store.to_string(),
            "--name".to_string(),
            name.to_string(),
            "--out".to_string(),
            out.display().to_string(),
        ])?;
        if !value.get("found").and_then(Value::as_bool).unwrap_or(false) {
            crate::note!("[vision] no golden yet for {name}");
            return Ok(false);
        }
        Ok(out.is_file())
    }

    pub fn golden_approve(
        &self,
        store: &str,
        name: &str,
        png: &Path,
        run: &str,
        sha: &str,
    ) -> Result<Value> {
        self.invoke(&[
            "golden".to_string(),
            "approve".to_string(),
            "--store".to_string(),
            store.to_string(),
            "--name".to_string(),
            name.to_string(),
            "--png".to_string(),
            png.display().to_string(),
            "--run".to_string(),
            run.to_string(),
            "--sha".to_string(),
            sha.to_string(),
        ])
    }

    pub fn report(&self, run_dir: &Path, out: &Path) -> Result<Value> {
        self.invoke(&[
            "report".to_string(),
            "--run-dir".to_string(),
            run_dir.display().to_string(),
            "--out".to_string(),
            out.display().to_string(),
        ])
    }
}

fn source_dirs(config: &Config) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(found) = find_dir_upwards(&cwd, SOURCE_DIR) {
        dirs.push(found);
    }
    let home = config.uibox_home.join(SOURCE_DIR);
    if !dirs.contains(&home) {
        dirs.push(home);
    }
    dirs
}

fn suffix(stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}
