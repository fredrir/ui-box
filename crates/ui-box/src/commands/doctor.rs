use anyhow::Result;
use serde_json::{json, Value};

use super::terminate;
use crate::backend::{self, proxy_hop, which, Cmd};
use crate::config::{Config, Surface};
use crate::driver::{self, Connection};
use crate::error::backend_failure;
use crate::note;
use crate::output::Summary;
use crate::pipeline;
use crate::session::SessionStore;
use crate::vision::Vision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Blocking,
    Advisory,
}

impl Severity {
    fn as_str(&self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::Advisory => "advisory",
        }
    }
}

struct Check {
    name: &'static str,
    ok: bool,
    severity: Severity,
    detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Check {
        Check {
            name,
            ok: true,
            severity: Severity::Blocking,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Check {
        Check {
            name,
            ok: false,
            severity: Severity::Blocking,
            detail: detail.into(),
        }
    }

    fn advisory(mut self) -> Check {
        self.severity = Severity::Advisory;
        self
    }

    fn blocks(&self) -> bool {
        !self.ok && self.severity == Severity::Blocking
    }

    fn label(&self) -> &'static str {
        match (self.ok, self.severity) {
            (true, _) => "ok  ",
            (false, Severity::Advisory) => "warn",
            (false, Severity::Blocking) => "fail",
        }
    }

    fn json(&self) -> Value {
        json!({
            "name": self.name,
            "ok": self.ok,
            "severity": self.severity.as_str(),
            "detail": self.detail,
        })
    }
}

fn usable(checks: &[Check]) -> bool {
    !checks.iter().any(Check::blocks)
}

pub fn doctor(config: &Config) -> Result<Summary> {
    let mut checks = Vec::new();

    checks.push(Check::pass(
        "config",
        match (&config.project_file, &config.global_env) {
            (Some(project), Some(global)) => {
                format!("{} then {}", project.display(), global.display())
            }
            (Some(project), None) => format!("{} (no global .env)", project.display()),
            (None, Some(global)) => format!("{} (no uibox.toml)", global.display()),
            (None, None) => "defaults only: no uibox.toml, no global .env".to_string(),
        },
    ));

    checks.push(match std::fs::create_dir_all(&config.artifacts) {
        Ok(()) => Check::pass("artifacts", config.artifacts.display().to_string()),
        Err(err) => Check::fail(
            "artifacts",
            format!("{}: {err}", config.artifacts.display()),
        ),
    });

    checks.push(check_backend(config));
    checks.push(check_driver(config, Surface::Web));
    checks.push(check_tui().advisory());
    checks.push(check_vision(config).advisory());
    checks.push(check_pipeline().advisory());
    checks.push(check_goldens(config).advisory());
    checks.push(check_sessions(config));

    let ok = usable(&checks);
    for check in &checks {
        note!("{} {}: {}", check.label(), check.name, check.detail);
    }

    let body = json!({
        "backend": config.backend.url(),
        "artifacts": config.artifacts,
        "display": config.display,
        "session_ttl": config.session_ttl.as_secs(),
        "config": config
            .origins
            .iter()
            .map(|origin| json!({
                "key": origin.key,
                "value": origin.value,
                "source": origin.source,
            }))
            .collect::<Vec<Value>>(),
        "checks": checks.iter().map(Check::json).collect::<Vec<Value>>(),
        "blocking_failed": checks.iter().filter(|check| check.blocks()).count(),
        "advisory_failed": checks
            .iter()
            .filter(|check| !check.ok && check.severity == Severity::Advisory)
            .count(),
    });

    if ok {
        Ok(Summary::ok(body))
    } else {
        Ok(Summary::unusable(body))
    }
}

fn check_backend(config: &Config) -> Check {
    let mut inert = config.clone();
    inert.force = false;
    let backend = match backend::select(&inert) {
        Ok(backend) => backend,
        Err(err) => return Check::fail("backend", err.to_string()),
    };
    if backend.is_local() {
        return Check::pass("backend", "local://, nothing to reach");
    }
    let hop = match (config.force, config.backend.host()) {
        (true, Some(host)) => proxy_hop(host)
            .map(|hop| format!(", --force would set DLAB_FORCE=1 on {hop}"))
            .unwrap_or_else(|| ", --force would set DLAB_FORCE=1 locally".to_string()),
        _ => String::new(),
    };
    match backend.require(&Cmd::new("echo").arg("ui-box")) {
        Ok(output) => Check::pass(
            "backend",
            format!(
                "{} answered {:?}{hop}",
                backend.url(),
                output.trimmed_stdout()
            ),
        ),
        Err(err) => {
            let detail = match backend_failure(&err) {
                Some(failure) => failure.verbatim().to_string(),
                None => format!("{err:#}"),
            };
            Check::fail("backend", detail)
        }
    }
}

fn check_driver(config: &Config, surface: Surface) -> Check {
    let spec = match driver::resolve(surface, config) {
        Ok(spec) => spec,
        Err(err) => return Check::fail("driver.dom", err.to_string()),
    };
    let Some(program) = spec.argv.first() else {
        return Check::fail("driver.dom", format!("{} has no command to run", spec.name));
    };
    if which(program).is_none() && !std::path::Path::new(program).is_file() {
        return Check::fail("driver.dom", format!("{program} is not executable"));
    }
    let mut conn = match Connection::spawn(&spec, config.rpc_timeout) {
        Ok(conn) => conn,
        Err(err) => return Check::fail("driver.dom", format!("{err:#}")),
    };
    let pid = conn.pid().unwrap_or_default();
    let check = match conn.info() {
        Ok(info) => Check::pass(
            "driver.dom",
            format!("{} {} speaks {:?}", info.name, info.version, info.surfaces),
        ),
        Err(err) => Check::fail("driver.dom", format!("{}: {err:#}", spec.display())),
    };
    terminate(pid);
    check
}

fn check_tui() -> Check {
    Check::pass(
        "driver.tui",
        "not implemented yet, `--surface tui` fails with a clear error",
    )
}

fn check_vision(config: &Config) -> Check {
    match Vision::locate(config) {
        Some(vision) => Check::pass("vision", vision.display()),
        None => Check::fail(
            "vision",
            format!(
                "no {} on PATH, `verify` cannot diff goldens",
                crate::vision::PROGRAM
            ),
        ),
    }
}

fn check_pipeline() -> Check {
    if pipeline::available() {
        Check::pass("pipeline", "ui-box-pipeline linked in")
    } else {
        Check::fail(
            "pipeline",
            "built without the pipeline feature, `run` cannot place artifacts",
        )
    }
}

fn check_goldens(config: &Config) -> Check {
    match &config.goldens {
        Some(store) => Check::pass("goldens", store.clone()),
        None => Check::fail(
            "goldens",
            "UIBOX_GOLDENS is unset, `verify` has nothing to compare",
        ),
    }
}

fn check_sessions(config: &Config) -> Check {
    let store = SessionStore::new(config);
    match store.list() {
        Ok(sessions) => {
            let live = sessions.iter().filter(|record| !record.expired()).count();
            Check::pass(
                "sessions",
                format!(
                    "{} live, {} recorded in {}",
                    live,
                    sessions.len(),
                    store.root().display()
                ),
            )
        }
        Err(err) => Check::fail("sessions", err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_gaps_leave_ui_box_usable() {
        let checks = vec![
            Check::pass("backend", "up"),
            Check::fail("vision", "absent").advisory(),
            Check::fail("goldens", "unset").advisory(),
        ];
        assert!(usable(&checks));
    }

    #[test]
    fn a_blocking_gap_makes_it_unusable() {
        let checks = vec![
            Check::pass("backend", "up"),
            Check::fail("driver.dom", "absent"),
        ];
        assert!(!usable(&checks));
    }

    #[test]
    fn only_verify_specific_checks_are_advisory() {
        let advisory = [check_tui().advisory(), check_pipeline().advisory()];
        assert!(advisory
            .iter()
            .all(|check| check.severity == Severity::Advisory));
        assert!(!Check::fail("artifacts", "read only").ok);
        assert_eq!(
            Check::fail("artifacts", "read only").severity,
            Severity::Blocking
        );
    }
}
