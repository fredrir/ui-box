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
pub const DEFAULT_FORWARD_HOST: &str = "127.0.0.1";
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub lab_port: u16,
    pub local_host: String,
    pub local_port: u16,
}

impl Forward {
    pub fn label(&self) -> String {
        if self.local_host != DEFAULT_FORWARD_HOST {
            return format!("{}:{}:{}", self.lab_port, self.local_host, self.local_port);
        }
        if self.lab_port == self.local_port {
            return self.lab_port.to_string();
        }
        format!("{}:{}", self.lab_port, self.local_port)
    }

    pub fn connect_host(&self) -> &str {
        self.local_host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
    }

    pub fn is_default_host(&self) -> bool {
        self.local_host == DEFAULT_FORWARD_HOST
    }
}

impl fmt::Display for Forward {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl FromStr for Forward {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let spec = s.trim();
        let (lab_port, local_host, local_port) = match (spec.find(':'), spec.rfind(':')) {
            (Some(first), Some(last)) if first == last => (
                forward_port(&spec[..first], spec)?,
                DEFAULT_FORWARD_HOST.to_string(),
                forward_port(&spec[first + 1..], spec)?,
            ),
            (Some(first), Some(last)) => {
                let host = spec[first + 1..last].trim();
                if host.is_empty() {
                    return Err(bad_forward(spec));
                }
                (
                    forward_port(&spec[..first], spec)?,
                    host.to_string(),
                    forward_port(&spec[last + 1..], spec)?,
                )
            }
            _ => {
                let port = forward_port(spec, spec)?;
                (port, DEFAULT_FORWARD_HOST.to_string(), port)
            }
        };
        Ok(Forward {
            lab_port,
            local_host,
            local_port,
        })
    }
}

fn bad_forward(spec: &str) -> anyhow::Error {
    anyhow!(
        "bad forward {spec:?}, expected REMOTE, REMOTE:LOCAL or REMOTE:HOST:LOCAL \
         with ports 1-65535"
    )
}

fn forward_port(raw: &str, spec: &str) -> Result<u16> {
    match raw.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err(bad_forward(spec)),
        Ok(port) => Ok(port),
    }
}

pub fn parse_forwards(raw: &str) -> Result<Vec<Forward>> {
    let mut out: Vec<Forward> = Vec::new();
    for token in raw.split([',', ' ', '\t', '\n', '\r']) {
        if token.trim().is_empty() {
            continue;
        }
        let forward = Forward::from_str(token)?;
        if out.contains(&forward) {
            continue;
        }
        if let Some(held) = out.iter().find(|held| held.lab_port == forward.lab_port) {
            bail!(
                "forwards {} and {} both bind lab port {}, which one connection cannot hold twice",
                held.label(),
                forward.label(),
                forward.lab_port
            );
        }
        out.push(forward);
    }
    Ok(out)
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
    pub forward: Vec<String>,
    pub app_args: Vec<String>,
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
    pub forward: Vec<Forward>,
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
    pub tauri_driver: Option<String>,
    pub native_driver: Option<String>,
    pub webdriver_port: Option<u16>,
    pub native_driver_port: Option<u16>,
    pub webdriver_env: BTreeMap<String, String>,
    pub app_args: Option<Vec<String>>,
    pub capabilities: Option<serde_json::Value>,
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
        if !overrides.forward.is_empty() {
            cli.insert("forward".to_string(), overrides.forward.join(","));
        }
        if !overrides.app_args.is_empty() {
            let encoded = serde_json::to_string(&overrides.app_args)
                .context("cannot encode --app-arg values")?;
            cli.insert("app_args".to_string(), encoded);
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

        let forward = match layers.take_opt("forward", &mut origins) {
            Some(raw) => parse_forwards(&raw)
                .with_context(|| format!("UIBOX_FORWARD resolved to {raw:?}"))?,
            None => Vec::new(),
        };

        let webdriver_port = layers.take_port("webdriver_port", &mut origins)?;
        let native_driver_port = layers.take_port("native_driver_port", &mut origins)?;
        let webdriver_env = match layers.take_opt("webdriver_env", &mut origins) {
            Some(raw) => parse_env_pairs(&raw)
                .with_context(|| format!("UIBOX_WEBDRIVER_ENV resolved to {raw:?}"))?,
            None => BTreeMap::new(),
        };
        let app_args = match layers.take_opt("app_args", &mut origins) {
            Some(raw) => Some(
                parse_string_array(&raw)
                    .with_context(|| format!("UIBOX_APP_ARGS resolved to {raw:?}"))?,
            ),
            None => None,
        };
        let capabilities = match layers.take_opt("capabilities", &mut origins) {
            Some(raw) => Some(
                serde_json::from_str(&raw)
                    .with_context(|| format!("UIBOX_CAPABILITIES resolved to {raw:?}"))?,
            ),
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
            forward,
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
            tauri_driver: layers.take_opt("tauri_driver", &mut origins),
            native_driver: layers.take_opt("native_driver", &mut origins),
            webdriver_port,
            native_driver_port,
            webdriver_env,
            app_args,
            capabilities,
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

    fn take_port(&self, key: &str, origins: &mut Vec<Origin>) -> Result<Option<u16>> {
        let Some(raw) = self.take_opt(key, origins) else {
            return Ok(None);
        };
        let port: u16 = raw.trim().parse().with_context(|| {
            format!(
                "UIBOX_{} resolved to {raw:?}, expected a port 1-65535",
                key.to_ascii_uppercase()
            )
        })?;
        Ok(Some(port))
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

pub fn parse_string_array(raw: &str) -> Result<Vec<String>> {
    let expected = "expected a JSON array of strings, e.g. [\"--open\",\"/a path with spaces\"]";
    let parsed: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|_| anyhow!("{expected}"))?;
    let items = parsed.as_array().ok_or_else(|| anyhow!("{expected}"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("{item} is not a string, {expected}"))
        })
        .collect()
}

pub fn parse_env_pairs(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some((key, value)) = token.split_once('=') else {
            bail!("bad env pair {token:?}, expected KEY=VALUE separated by commas");
        };
        out.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok(out)
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

    fn forward(spec: &str) -> Forward {
        Forward::from_str(spec).expect(spec)
    }

    #[test]
    fn a_forward_is_remote_first() {
        assert_eq!(
            forward("3000"),
            Forward {
                lab_port: 3000,
                local_host: DEFAULT_FORWARD_HOST.to_string(),
                local_port: 3000
            }
        );
        assert_eq!(
            forward("3000:5173"),
            Forward {
                lab_port: 3000,
                local_host: DEFAULT_FORWARD_HOST.to_string(),
                local_port: 5173
            }
        );
        assert_eq!(
            forward("3000:h:5173"),
            Forward {
                lab_port: 3000,
                local_host: "h".to_string(),
                local_port: 5173
            }
        );
    }

    #[test]
    fn a_forward_defaults_the_local_end_to_ipv4() {
        assert_eq!(forward("3000").local_host, "127.0.0.1");
        assert_eq!(forward("3000:5173").local_host, "127.0.0.1");
        assert_eq!(forward("3000:[::1]:5173").connect_host(), "::1");
        assert_eq!(forward("3000:::1:5173").connect_host(), "::1");
    }

    #[test]
    fn a_forward_label_round_trips() {
        for spec in ["3000", "3000:5173", "3000:h:5173"] {
            assert_eq!(forward(spec).label(), spec);
            assert_eq!(forward(&forward(spec).label()), forward(spec));
        }
    }

    #[test]
    fn a_forward_that_is_not_a_port_is_refused() {
        for spec in ["abc", "0", "70000", "3000:0", "3000:abc", "3000::5173", ""] {
            assert!(Forward::from_str(spec).is_err(), "{spec:?} parsed");
        }
    }

    #[test]
    fn an_app_argument_containing_a_space_survives_verbatim() {
        assert_eq!(
            parse_string_array(r#"["--open","/a path with spaces.tex"]"#).expect("args"),
            vec!["--open", "/a path with spaces.tex"]
        );
        assert!(parse_string_array("[]").expect("empty").is_empty());
    }

    #[test]
    fn app_arguments_that_are_not_a_json_string_array_are_refused() {
        for raw in ["--open /a path", "[1,2]", "{}", "[\"a\"", ""] {
            assert!(parse_string_array(raw).is_err(), "{raw:?} parsed");
        }
    }

    #[test]
    fn forwards_split_on_commas_and_spaces() {
        let parsed = parse_forwards("3000, 5173:4000  8080").expect("forwards");
        let labels: Vec<String> = parsed.iter().map(Forward::label).collect();
        assert_eq!(labels, vec!["3000", "5173:4000", "8080"]);
        assert!(parse_forwards("").expect("empty").is_empty());
    }

    #[test]
    fn one_lab_port_cannot_carry_two_forwards() {
        assert!(parse_forwards("3000:5173,3000:5174").is_err());
        assert_eq!(parse_forwards("3000,3000").expect("dedupe").len(), 1);
    }
}
