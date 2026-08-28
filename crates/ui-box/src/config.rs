use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

pub use ui_box_core::BackendSpec;

pub const DEFAULT_ARTIFACTS: &str = ".uibox/runs";
pub const DEFAULT_DISPLAY: &str = "1280x800x24";
pub const DEFAULT_SESSION_TTL: u64 = 900;
pub const DEFAULT_RPC_TIMEOUT: u64 = 30;
pub const PROJECT_FILE: &str = "uibox.toml";
pub const GLOBAL_ENV_FILE: &str = ".env";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum Surface {
    Web,
    Tauri,
    Tui,
}

impl Surface {
    pub fn as_str(&self) -> &'static str {
        match self {
            Surface::Web => "web",
            Surface::Tauri => "tauri",
            Surface::Tui => "tui",
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Surface {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "web" => Ok(Surface::Web),
            "tauri" => Ok(Surface::Tauri),
            "tui" => Ok(Surface::Tui),
            other => bail!("unknown surface {other:?}, expected web, tauri or tui"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn label(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

impl fmt::Display for Viewport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl FromStr for Viewport {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s.trim().split('x');
        let width = parts
            .next()
            .and_then(|v| v.trim().parse().ok())
            .ok_or_else(|| anyhow!("bad viewport {s:?}, expected WIDTHxHEIGHT"))?;
        let height = parts
            .next()
            .and_then(|v| v.trim().parse().ok())
            .ok_or_else(|| anyhow!("bad viewport {s:?}, expected WIDTHxHEIGHT"))?;
        Ok(Viewport { width, height })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Origin {
    pub key: String,
    pub value: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub backend: Option<String>,
    pub display: Option<String>,
    pub artifacts: Option<PathBuf>,
    pub goldens: Option<String>,
    pub session_ttl: Option<u64>,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub backend: BackendSpec,
    pub display: String,
    pub viewport: Viewport,
    pub artifacts: PathBuf,
    pub goldens: Option<String>,
    pub session_ttl: Duration,
    pub rpc_timeout: Duration,
    pub project: Option<String>,
    pub lab: Option<String>,
    pub surface: Option<Surface>,
    pub target: Option<String>,
    pub target_lab: Option<String>,
    pub source: Option<String>,
    pub build: Option<String>,
    pub artifact: Option<String>,
    pub driver_dom_remote: Option<String>,
    pub driver_dom: Option<String>,
    pub vision: Option<String>,
    pub force: bool,
    pub project_root: Option<PathBuf>,
    pub project_file: Option<PathBuf>,
    pub global_env: Option<PathBuf>,
    pub uibox_home: PathBuf,
    pub origins: Vec<Origin>,
}

impl Config {
    pub fn resolve(overrides: &Overrides) -> Result<Config> {
        let cwd = std::env::current_dir().context("cannot read current directory")?;
        Config::resolve_from(overrides, &cwd)
    }

    pub fn resolve_from(overrides: &Overrides, cwd: &Path) -> Result<Config> {
        let uibox_home = uibox_home();
        let project_file = find_upwards(cwd, PROJECT_FILE);
        let project = match &project_file {
            Some(path) => Some((path.clone(), load_toml(path)?)),
            None => None,
        };
        let global_path = uibox_home.join(GLOBAL_ENV_FILE);
        let global = if global_path.is_file() {
            Some((global_path.clone(), load_env_file(&global_path)?))
        } else {
            None
        };

        let mut cli = BTreeMap::new();
        if let Some(value) = &overrides.backend {
            cli.insert("backend".to_string(), value.clone());
        }
        if let Some(value) = &overrides.display {
            cli.insert("display".to_string(), value.clone());
        }
        if let Some(value) = &overrides.artifacts {
            cli.insert("artifacts".to_string(), value.display().to_string());
        }
        if let Some(value) = &overrides.goldens {
            cli.insert("goldens".to_string(), value.clone());
        }
        if let Some(value) = overrides.session_ttl {
            cli.insert("session_ttl".to_string(), value.to_string());
        }

        let layers = Layers {
            cli,
            project,
            global,
        };
        let mut origins = Vec::new();

        let backend_raw = layers.take("backend", &mut origins, || "local://".to_string());
        let backend = BackendSpec::parse(&backend_raw)
            .with_context(|| format!("UIBOX_BACKEND resolved to {backend_raw:?}"))?;

        let display = layers.take("display", &mut origins, || DEFAULT_DISPLAY.to_string());
        let viewport = parse_display(&display)?;

        let artifacts_raw =
            layers.take("artifacts", &mut origins, || DEFAULT_ARTIFACTS.to_string());
        let project_root = project_file
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let artifacts = absolutize(&PathBuf::from(&artifacts_raw), project_root.as_deref(), cwd);

        let goldens = layers.take_opt("goldens", &mut origins);
        let ttl_raw = layers.take("session_ttl", &mut origins, || {
            DEFAULT_SESSION_TTL.to_string()
        });
        let ttl: u64 = ttl_raw.trim().parse().with_context(|| {
            format!("UIBOX_SESSION_TTL resolved to {ttl_raw:?}, expected seconds")
        })?;

        let timeout_raw = layers.take("rpc_timeout", &mut origins, || {
            DEFAULT_RPC_TIMEOUT.to_string()
        });
        let rpc_timeout: u64 = timeout_raw
            .trim()
            .parse()
            .with_context(|| format!("UIBOX_RPC_TIMEOUT resolved to {timeout_raw:?}"))?;

        let surface = match layers.take_opt("surface", &mut origins) {
            Some(raw) => Some(Surface::from_str(&raw)?),
            None => None,
        };

        Ok(Config {
            backend,
            display,
            viewport,
            artifacts,
            goldens,
            session_ttl: Duration::from_secs(ttl),
            rpc_timeout: Duration::from_secs(rpc_timeout),
            project: layers.take_opt("project", &mut origins),
            lab: layers.take_opt("lab", &mut origins),
            surface,
            target: layers.take_opt("target", &mut origins),
            target_lab: layers.take_opt("target_lab", &mut origins),
            source: layers.take_opt("source", &mut origins),
            build: layers.take_opt("build", &mut origins),
            artifact: layers.take_opt("artifact", &mut origins),
            driver_dom_remote: layers.take_opt("driver_dom_remote", &mut origins),
            driver_dom: layers.take_opt("driver_dom", &mut origins),
            vision: layers.take_opt("vision", &mut origins),
            force: overrides.force,
            project_root,
            project_file,
            global_env: global_path.is_file().then_some(global_path),
            uibox_home,
            origins,
        })
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.artifacts.join(".sessions")
    }

    pub fn surface_or(&self, explicit: Option<Surface>) -> Surface {
        explicit.or(self.surface).unwrap_or(Surface::Web)
    }
}

struct Layers {
    cli: BTreeMap<String, String>,
    project: Option<(PathBuf, BTreeMap<String, String>)>,
    global: Option<(PathBuf, BTreeMap<String, String>)>,
}

impl Layers {
    fn lookup(&self, key: &str) -> Option<(String, String)> {
        if let Some(value) = self.cli.get(key) {
            return Some((value.clone(), "cli".to_string()));
        }
        let env_key = format!("UIBOX_{}", key.to_ascii_uppercase());
        if let Ok(value) = std::env::var(&env_key) {
            if !value.trim().is_empty() {
                return Some((value, format!("env:{env_key}")));
            }
        }
        if let Some((path, map)) = &self.project {
            if let Some(value) = map.get(key) {
                return Some((value.clone(), format!("project:{}", path.display())));
            }
        }
        if let Some((path, map)) = &self.global {
            if let Some(value) = map.get(key) {
                return Some((value.clone(), format!("global:{}", path.display())));
            }
        }
        None
    }

    fn take(
        &self,
        key: &str,
        origins: &mut Vec<Origin>,
        fallback: impl FnOnce() -> String,
    ) -> String {
        match self.lookup(key) {
            Some((value, source)) => {
                origins.push(Origin {
                    key: key.to_string(),
                    value: value.clone(),
                    source,
                });
                value
            }
            None => {
                let value = fallback();
                origins.push(Origin {
                    key: key.to_string(),
                    value: value.clone(),
                    source: "default".to_string(),
                });
                value
            }
        }
    }

    fn take_opt(&self, key: &str, origins: &mut Vec<Origin>) -> Option<String> {
        let (value, source) = self.lookup(key)?;
        origins.push(Origin {
            key: key.to_string(),
            value: value.clone(),
            source,
        });
        Some(value)
    }
}

pub fn uibox_home() -> PathBuf {
    if let Ok(explicit) = std::env::var("UIBOX_HOME") {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    home_dir().join("packages").join("ui-box")
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn find_upwards(start: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

pub fn find_dir_upwards(start: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

fn absolutize(path: &Path, project_root: Option<&Path>, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let base = project_root.unwrap_or(cwd);
    base.join(path)
}

fn parse_display(display: &str) -> Result<Viewport> {
    let geometry = match display.rfind('x') {
        Some(_) if display.split('x').count() >= 3 => {
            let mut parts = display.split('x');
            let width = parts.next().unwrap_or_default();
            let height = parts.next().unwrap_or_default();
            format!("{width}x{height}")
        }
        _ => display.to_string(),
    };
    Viewport::from_str(&geometry).with_context(|| format!("UIBOX_DISPLAY resolved to {display:?}"))
}

fn load_toml(path: &Path) -> Result<BTreeMap<String, String>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let parsed: toml::Value = raw
        .parse()
        .with_context(|| format!("cannot parse {}", path.display()))?;
    let mut out = BTreeMap::new();
    let table = match parsed.as_table() {
        Some(table) => table.clone(),
        None => return Ok(out),
    };
    let mut flat = table.clone();
    if let Some(toml::Value::Table(nested)) = table.get("uibox") {
        for (key, value) in nested {
            flat.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in flat {
        let key = normalize_key(&key);
        if let Some(value) = scalar(&value) {
            out.insert(key, value);
        }
    }
    Ok(out)
}

fn scalar(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(v) => Some(v.clone()),
        toml::Value::Integer(v) => Some(v.to_string()),
        toml::Value::Float(v) => Some(v.to_string()),
        toml::Value::Boolean(v) => Some(v.to_string()),
        _ => None,
    }
}

fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        out.insert(normalize_key(key.trim()), value.to_string());
    }
    Ok(out)
}

fn normalize_key(key: &str) -> String {
    let lowered = key.trim().to_ascii_lowercase();
    lowered
        .strip_prefix("uibox_")
        .unwrap_or(&lowered)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_depth_from_display() {
        assert_eq!(
            parse_display("1280x800x24").unwrap(),
            Viewport {
                width: 1280,
                height: 800
            }
        );
        assert_eq!(
            parse_display("1280x800").unwrap(),
            Viewport {
                width: 1280,
                height: 800
            }
        );
    }

    #[test]
    fn normalizes_config_keys() {
        assert_eq!(normalize_key("UIBOX_BACKEND"), "backend");
        assert_eq!(normalize_key("backend"), "backend");
    }
}
