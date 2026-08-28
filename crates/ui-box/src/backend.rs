use anyhow::Result;

use crate::config::{BackendSpec, Config};

pub use ui_box_core::{
    parse_proxy_hop, proxy_hop, shell_quote, wake, which, Backend, Cmd, LocalBackend, Output,
    SshBackend,
};

pub fn for_lab(config: &Config, lab: &str) -> Result<Box<dyn Backend>> {
    match config.backend.clone() {
        BackendSpec::Local => Ok(Box::new(LocalBackend::new())),
        BackendSpec::Ssh { user, .. } => {
            let spec = BackendSpec::Ssh {
                user,
                host: lab.to_string(),
            };
            Ok(Box::new(SshBackend::new(spec, config.force)?))
        }
    }
}

pub fn select(config: &Config) -> Result<Box<dyn Backend>> {
    match config.backend.clone() {
        BackendSpec::Local => Ok(Box::new(LocalBackend::new())),
        spec @ BackendSpec::Ssh { .. } => Ok(Box::new(SshBackend::new(spec, config.force)?)),
    }
}
