pub mod doctor;
pub mod executor;
pub mod flows;
pub mod live;
pub mod records;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::backend::{Backend, Cmd};
use crate::cli::{Cli, Command};
use crate::config::{Config, Surface};
use crate::driver::client::process_alive;
use crate::driver::{DriverInfo, DriverSpec};
use crate::note;
use crate::output::Summary;
use crate::run::RunDir;

pub fn execute(cli: &Cli) -> Result<Summary> {
    let config = Config::resolve(&cli.global.overrides())?;
    match &cli.command {
        Command::Open(args) => live::open(&config, args),
        Command::Act(args) => live::act(&config, args),
        Command::Snap(args) => live::snap(&config, args),
        Command::Eval(args) => live::eval(&config, args),
        Command::Wake(args) => live::wake(&config, args),
        Command::Close(args) => live::close(&config, args),
        Command::Record(args) => records::record(&config, args),
        Command::Run(args) => flows::run(&config, args),
        Command::Verify(args) => flows::verify(&config, args),
        Command::Doctor => doctor::doctor(&config),
        Command::Runs(args) => records::runs(&config, args),
        Command::Show(args) => records::show(&config, args),
    }
}

pub fn probe_backend(backend: &dyn Backend) -> Result<()> {
    if backend.is_local() {
        return Ok(());
    }
    backend.require(&Cmd::new("true"))?;
    Ok(())
}

pub fn ensure_surface(info: &DriverInfo, surface: Surface) -> Result<()> {
    if info.surfaces.is_empty() {
        return Ok(());
    }
    if info
        .surfaces
        .iter()
        .any(|declared| declared.eq_ignore_ascii_case(surface.as_str()))
    {
        return Ok(());
    }
    bail!(
        "driver {} declares surfaces {:?} and cannot drive {surface}",
        info.name,
        info.surfaces
    );
}

pub fn driver_options(config: &Config, surface: Surface, run_dir: &Path) -> Value {
    let mut options = json!({
        "surface": surface.as_str(),
        "display": config.display,
        "force": config.force,
        "runDir": run_dir,
        "snapsDir": run_dir.join("snaps"),
        "artifactsDir": run_dir.parent(),
    });
    if surface == Surface::Tauri {
        if let Some(map) = options.as_object_mut() {
            for (key, value) in tauri_options(config) {
                map.insert(key.to_string(), value);
            }
        }
    }
    options
}

fn tauri_options(config: &Config) -> Vec<(&'static str, Value)> {
    let mut options: Vec<(&'static str, Value)> = Vec::new();
    if let Some(bin) = &config.tauri_driver {
        options.push(("tauriDriverBin", json!(bin)));
    }
    if let Some(bin) = &config.native_driver {
        options.push(("nativeDriverBin", json!(bin)));
    }
    if let Some(port) = config.webdriver_port {
        options.push(("webdriverPort", json!(port)));
    }
    if let Some(port) = config.native_driver_port {
        options.push(("nativeDriverPort", json!(port)));
    }
    if let Some(args) = config.app_args.as_ref().filter(|args| !args.is_empty()) {
        options.push(("appArgs", json!(args)));
    }
    if !config.webdriver_env.is_empty() {
        options.push(("webdriverEnv", json!(config.webdriver_env)));
    }
    if let Some(capabilities) = &config.capabilities {
        options.push(("capabilities", capabilities.clone()));
    }
    options
}

pub fn driver_run_dir(
    spec: &DriverSpec,
    backend: &dyn Backend,
    run: &RunDir,
) -> Result<Option<PathBuf>> {
    if !spec.remote {
        return Ok(None);
    }
    let script = format!(
        "d=\"$HOME/.uibox/runs/{}\"; mkdir -p \"$d/snaps\" \"$d/diff\" && printf %s \"$d\"",
        run.id
    );
    let output = backend.require(&Cmd::shell(script))?;
    let dir = output.trimmed_stdout();
    if dir.is_empty() {
        bail!(
            "{} did not report a run directory for the driver",
            backend.url()
        );
    }
    note!("driver writes to {dir} on {}", backend.url());
    Ok(Some(PathBuf::from(dir)))
}

pub fn terminate(pid: u32) {
    if pid == 0 || !process_alive(pid) {
        return;
    }
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    for _ in 0..40 {
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Overrides;

    const WEB_KEYS: [&str; 6] = [
        "surface",
        "display",
        "force",
        "runDir",
        "snapsDir",
        "artifactsDir",
    ];

    const TAURI_KEYS: [&str; 7] = [
        "tauriDriverBin",
        "nativeDriverBin",
        "webdriverPort",
        "nativeDriverPort",
        "appArgs",
        "webdriverEnv",
        "capabilities",
    ];

    fn bare() -> Config {
        let mut config =
            Config::resolve_from(&Overrides::default(), Path::new("/")).expect("config");
        config.tauri_driver = None;
        config.native_driver = None;
        config.webdriver_port = None;
        config.native_driver_port = None;
        config.app_args = None;
        config.webdriver_env.clear();
        config.capabilities = None;
        config
    }

    #[test]
    fn a_web_open_carries_no_tauri_options() {
        let options = driver_options(&bare(), Surface::Web, Path::new("/tmp/run"));
        let map = options.as_object().expect("object");
        assert_eq!(map.len(), WEB_KEYS.len());
        for key in WEB_KEYS {
            assert!(map.contains_key(key), "{key} went missing");
        }
    }

    #[test]
    fn a_mac_side_tauri_setting_never_leaks_into_a_web_open() {
        let mut config = bare();
        config.tauri_driver = Some("/opt/homebrew/bin/tauri-driver".to_string());
        config.webdriver_port = Some(4444);
        let options = driver_options(&config, Surface::Web, Path::new("/tmp/run"));
        let map = options.as_object().expect("object");
        assert_eq!(map.len(), WEB_KEYS.len());
        for key in TAURI_KEYS {
            assert!(!map.contains_key(key), "{key} leaked into a web open");
        }
    }

    #[test]
    fn an_unconfigured_tauri_open_sends_no_defaults() {
        let options = driver_options(&bare(), Surface::Tauri, Path::new("/tmp/run"));
        let map = options.as_object().expect("object");
        for key in TAURI_KEYS {
            assert!(
                !map.contains_key(key),
                "{key} was sent without anyone configuring it"
            );
        }
    }

    #[test]
    fn a_tauri_open_carries_only_what_was_configured() {
        let mut config = bare();
        config.tauri_driver = Some("/nix/store/aaa/bin/tauri-driver".to_string());
        config.native_driver_port = Some(4445);
        config.webdriver_env.insert(
            "WEBKIT_DISABLE_DMABUF_RENDERER".to_string(),
            "1".to_string(),
        );
        let options = driver_options(&config, Surface::Tauri, Path::new("/tmp/run"));
        assert_eq!(options["tauriDriverBin"], "/nix/store/aaa/bin/tauri-driver");
        assert_eq!(options["nativeDriverPort"], 4445);
        assert_eq!(
            options["webdriverEnv"]["WEBKIT_DISABLE_DMABUF_RENDERER"],
            "1"
        );
        let map = options.as_object().expect("object");
        assert!(!map.contains_key("nativeDriverBin"));
        assert!(!map.contains_key("webdriverPort"));
    }
}
