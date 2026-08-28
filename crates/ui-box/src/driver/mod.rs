pub mod client;

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{find_dir_upwards, Config, Surface};
use crate::error::DriverError;

pub use client::{ActResult, Connection, DriverInfo, SnapResult};

pub const DOM_DRIVER_ENTRY: &str = "drivers/dom/dist/main.js";
pub const DOM_DRIVER_SOURCE: &str = "drivers/dom";

#[derive(Debug, Clone)]
pub struct DriverSpec {
    pub name: String,
    pub surface: Surface,
    pub argv: Vec<String>,
    pub entry: Option<PathBuf>,
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
    if let Some(command) = &config.driver_dom {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return Ok(DriverSpec {
                name: "dom".to_string(),
                surface,
                argv,
                entry: None,
            });
        }
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
    })
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
