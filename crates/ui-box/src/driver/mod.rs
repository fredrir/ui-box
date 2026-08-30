pub mod client;
pub mod forward;

use std::path::PathBuf;

use anyhow::Result;

use crate::backend::ssh_options;
use crate::config::{find_dir_upwards, Config, Forward, Surface};
use crate::error::DriverError;
use crate::note;

pub use client::{ActResult, Connection, DriverInfo, SnapResult};

pub const DOM_DRIVER_ENTRY: &str = "drivers/dom/dist/main.js";
pub const DOM_DRIVER_SOURCE: &str = "drivers/dom";
pub const DOM_DRIVER_REMOTE: &str = "ui-box-dom";

#[derive(Debug, Clone)]
pub struct DriverSpec {
    pub name: String,
    pub surface: Surface,
    pub argv: Vec<String>,
    pub entry: Option<PathBuf>,
    pub remote: bool,
}

impl DriverSpec {
    pub fn display(&self) -> String {
        self.argv.join(" ")
    }
}

pub fn resolve(surface: Surface, config: &Config) -> Result<DriverSpec> {
    match surface {
        Surface::Web | Surface::Tauri => resolve_dom(surface, config),
        Surface::Tui => Err(DriverError::UnsupportedSurface {
            surface: surface.to_string(),
        }
        .into()),
    }
}

fn resolve_dom(surface: Surface, config: &Config) -> Result<DriverSpec> {
    let host = config.backend.ssh_target();
    let label = driver_label(config);
    if let Some(command) = &config.driver_dom {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            let carries_transport = carries_own_transport(&argv);
            if host.is_none() || carries_transport {
                return Ok(DriverSpec {
                    name: if carries_transport {
                        label
                    } else {
                        "dom".to_string()
                    },
                    surface,
                    argv,
                    entry: None,
                    remote: carries_transport,
                });
            }
            note!(
                "ignoring UIBOX_DRIVER_DOM={command}: it names a driver on this machine, but \
                 the backend is {}, which is where the display is. Spawning {} there instead; \
                 set UIBOX_DRIVER_DOM_REMOTE to change that command",
                host.clone().unwrap_or_default(),
                remote_command(config).join(" ")
            );
        }
    }
    if let Some(host) = host {
        let argv = remote_argv(&host, &remote_command(config), &config.forward);
        return Ok(DriverSpec {
            name: label,
            surface,
            argv,
            entry: None,
            remote: true,
        });
    }
    let entry = dom_entry(config);
    if !entry.is_file() {
        return Err(DriverError::Missing {
            surface: surface.to_string(),
            path: entry.display().to_string(),
            hint: format!(
                "build the DOM driver in {} or point UIBOX_DRIVER_DOM at a driver command",
                DOM_DRIVER_SOURCE
            ),
        }
        .into());
    }
    Ok(DriverSpec {
        name: "dom".to_string(),
        surface,
        argv: vec!["node".to_string(), entry.display().to_string()],
        entry: Some(entry),
        remote: false,
    })
}

fn remote_command(config: &Config) -> Vec<String> {
    let raw = config
        .driver_dom_remote
        .as_deref()
        .unwrap_or(DOM_DRIVER_REMOTE);
    let argv: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        vec![DOM_DRIVER_REMOTE.to_string()]
    } else {
        argv
    }
}

pub fn driver_label(config: &Config) -> String {
    match config.backend.host() {
        Some(host) => format!("dom@{host}"),
        None => "dom".to_string(),
    }
}

pub fn carries_own_transport(argv: &[String]) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    let base = program.rsplit('/').next().unwrap_or(program);
    base == "ssh"
}

pub fn remote_argv(host: &str, command: &[String], forwards: &[Forward]) -> Vec<String> {
    let mut argv = vec!["ssh".to_string(), "-T".to_string()];
    argv.extend(ssh_options());
    argv.extend(forward::ssh_args(forwards));
    argv.push(host.to_string());
    argv.extend(command.iter().cloned());
    argv
}

pub fn resolve_without_forwards(surface: Surface, config: &Config) -> Result<DriverSpec> {
    if config.forward.is_empty() {
        return resolve(surface, config);
    }
    let mut config = config.clone();
    config.forward = Vec::new();
    resolve(surface, &config)
}

pub fn dom_entry(config: &Config) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(dir) = find_dir_upwards(&cwd, DOM_DRIVER_SOURCE) {
        if let Some(root) = dir.parent().and_then(|p| p.parent()) {
            let candidate = root.join(DOM_DRIVER_ENTRY);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    config.uibox_home.join(DOM_DRIVER_ENTRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(raw: &str) -> Vec<String> {
        raw.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_local_driver_path_does_not_override_a_remote_display() {
        assert!(!carries_own_transport(&words(
            "/nix/store/abc-ui-box-dom/bin/ui-box-dom"
        )));
        assert!(!carries_own_transport(&words(
            "node drivers/dom/dist/main.js"
        )));
    }

    #[test]
    fn an_explicit_ssh_command_is_its_own_transport() {
        assert!(carries_own_transport(&words("ssh dlab-ui ui-box-dom")));
        assert!(carries_own_transport(&words(
            "/usr/bin/ssh dlab-ui ui-box-dom"
        )));
    }

    fn config_for(backend: &str) -> crate::config::Config {
        crate::config::Config::resolve_from(
            &crate::config::Overrides {
                backend: Some(backend.to_string()),
                ..Default::default()
            },
            std::path::Path::new("/"),
        )
        .expect("config")
    }

    #[test]
    fn an_ssh_backend_spawns_the_driver_on_the_lab_not_here() {
        let mut config = config_for("ssh://fredrir@dlab-ui");
        config.driver_dom = Some("/nix/store/abc-ui-box-dom/bin/ui-box-dom".to_string());
        let spec = resolve(Surface::Web, &config).expect("spec");
        assert!(
            spec.remote,
            "the display is on the lab, so the driver must be too"
        );
        assert_eq!(spec.argv[0], "ssh");
        assert!(
            spec.argv.contains(&"fredrir@dlab-ui".to_string()),
            "{:?}",
            spec.argv
        );
        assert_eq!(spec.argv.last().unwrap(), DOM_DRIVER_REMOTE);
    }

    #[test]
    fn an_explicit_ssh_command_is_spawned_verbatim() {
        let mut config = config_for("ssh://fredrir@dlab-ui");
        config.driver_dom = Some("ssh dlab-ui ui-box-dom".to_string());
        let spec = resolve(Surface::Web, &config).expect("spec");
        assert!(spec.remote);
        assert_eq!(spec.argv, vec!["ssh", "dlab-ui", "ui-box-dom"]);
    }

    #[test]
    fn a_local_backend_spawns_the_driver_here() {
        let mut config = config_for("local://");
        config.driver_dom = Some("node drivers/dom/dist/main.js".to_string());
        let spec = resolve(Surface::Web, &config).expect("spec");
        assert!(!spec.remote);
        assert_eq!(spec.argv, vec!["node", "drivers/dom/dist/main.js"]);
    }

    #[test]
    fn a_remote_driver_is_labelled_with_its_host() {
        let mut config = crate::config::Config::resolve_from(
            &crate::config::Overrides {
                backend: Some("ssh://fredrir@dlab-ui".to_string()),
                ..Default::default()
            },
            std::path::Path::new("/"),
        )
        .unwrap();
        assert_eq!(driver_label(&config), "dom@dlab-ui");
        config.backend = crate::config::BackendSpec::Local;
        assert_eq!(driver_label(&config), "dom");
    }

    #[test]
    fn the_remote_driver_runs_where_the_display_is() {
        let argv = remote_argv("fredrir@dlab-ui", &words(DOM_DRIVER_REMOTE), &[]);
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[1], "-T");
        assert_eq!(argv[argv.len() - 2], "fredrir@dlab-ui");
        assert_eq!(argv[argv.len() - 1], DOM_DRIVER_REMOTE);
        assert!(argv.contains(&"BatchMode=yes".to_string()));
        for absent in [
            "-R",
            "ExitOnForwardFailure=yes",
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            assert!(
                !argv.contains(&absent.to_string()),
                "an undeclared forward must cost nothing: {argv:?}"
            );
        }
    }

    #[test]
    fn a_declared_forward_rides_the_drivers_own_connection() {
        let forwards = crate::config::parse_forwards("3000:5173").expect("forwards");
        let argv = remote_argv("fredrir@dlab-ui", &words(DOM_DRIVER_REMOTE), &forwards);
        let flags = argv.join(" ");
        assert!(
            flags.contains("-R 127.0.0.1:3000:127.0.0.1:5173"),
            "{flags}"
        );
        for option in forward::EXCLUSIVE_OPTIONS {
            assert!(argv.contains(&option.to_string()), "{flags}");
        }
        assert_eq!(argv[argv.len() - 2], "fredrir@dlab-ui");
        assert_eq!(argv[argv.len() - 1], DOM_DRIVER_REMOTE);
    }

    #[test]
    fn a_forward_never_reaches_the_short_lived_ssh_options() {
        let options = ssh_options().join(" ");
        assert!(!options.contains("-R"), "{options}");
        assert!(!options.contains("ControlMaster"), "{options}");
    }
}
