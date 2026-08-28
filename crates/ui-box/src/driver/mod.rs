pub mod client;

use std::path::PathBuf;

use anyhow::Result;

use crate::backend::ssh_options;
use crate::config::{find_dir_upwards, Config, Surface};
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
        let argv = remote_argv(&host, &remote_command(config));
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

pub fn remote_argv(host: &str, command: &[String]) -> Vec<String> {
    let mut argv = vec!["ssh".to_string(), "-T".to_string()];
    argv.extend(ssh_options());
    argv.push(host.to_string());
    argv.extend(command.iter().cloned());
    argv
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
        let argv = remote_argv("fredrir@dlab-ui", &words(DOM_DRIVER_REMOTE));
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[1], "-T");
        assert_eq!(argv[argv.len() - 2], "fredrir@dlab-ui");
        assert_eq!(argv[argv.len() - 1], DOM_DRIVER_REMOTE);
        assert!(argv.contains(&"BatchMode=yes".to_string()));
    }
}
