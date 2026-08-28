pub mod doctor;
pub mod executor;
pub mod flows;
pub mod live;
pub mod records;

use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::backend::{Backend, Cmd};
use crate::cli::{Cli, Command};
use crate::config::{Config, Surface};
use crate::driver::client::process_alive;
use crate::driver::DriverInfo;
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

pub fn driver_options(config: &Config, surface: Surface, run: &RunDir) -> Value {
    json!({
        "surface": surface.as_str(),
        "display": config.display,
        "force": config.force,
        "runDir": run.path,
        "snapsDir": run.snaps_dir(),
        "artifactsDir": config.artifacts,
    })
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
