use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use super::terminate;
use crate::backend::{self, proxy_hop, shell_quote, which, Backend, Cmd};
use crate::config::{Config, Surface};
use crate::driver::client::TauriInfo;
use crate::driver::{self, Connection, DriverInfo};
use crate::error::backend_failure;
use crate::note;
use crate::output::Summary;
use crate::pipeline;
use crate::session::SessionStore;
use crate::vision::Vision;

const LAB_LOOPBACK: &str = "127.0.0.1";
const PROBE_TIMEOUT: u64 = 5;
const PREFLIGHT_TIMEOUT: Duration = Duration::from_millis(750);

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

    fn unknown(name: &'static str, detail: impl Into<String>) -> Check {
        Check::fail(name, format!("unknown: {}", detail.into()))
    }

    fn advisory(mut self) -> Check {
        self.severity = Severity::Advisory;
        self
    }

    fn severity(self, severity: Severity) -> Check {
        match severity {
            Severity::Advisory => self.advisory(),
            Severity::Blocking => Check {
                severity: Severity::Blocking,
                ..self
            },
        }
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

    let (backend_check, lab) = check_backend(config);
    checks.push(backend_check);
    let lab = lab.as_deref();

    let (dom, info) = check_dom(config);
    checks.push(dom);
    checks.push(check_tauri(config, info.as_ref()));
    checks.push(check_tui().advisory());

    if let Some(check) = check_forward(config, lab) {
        checks.push(check);
    }
    if let Some(check) = check_target(config, lab) {
        checks.push(check.advisory());
    }

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

fn check_backend(config: &Config) -> (Check, Option<Box<dyn Backend>>) {
    let mut inert = config.clone();
    inert.force = false;
    let backend = match backend::select(&inert) {
        Ok(backend) => backend,
        Err(err) => return (Check::fail("backend", err.to_string()), None),
    };
    if backend.is_local() {
        return (
            Check::pass("backend", "local://, nothing to reach"),
            Some(backend),
        );
    }
    let hop = match (config.force, config.backend.host()) {
        (true, Some(host)) => proxy_hop(host)
            .map(|hop| format!(", --force would set DLAB_FORCE=1 on {hop}"))
            .unwrap_or_else(|| ", --force would set DLAB_FORCE=1 locally".to_string()),
        _ => String::new(),
    };
    match backend.require(&Cmd::new("echo").arg("ui-box")) {
        Ok(output) => {
            let detail = format!(
                "{} answered {:?}{hop}",
                backend.url(),
                output.trimmed_stdout()
            );
            (Check::pass("backend", detail), Some(backend))
        }
        Err(err) => {
            let detail = match backend_failure(&err) {
                Some(failure) => failure.verbatim().to_string(),
                None => format!("{err:#}"),
            };
            (Check::fail("backend", detail), None)
        }
    }
}

fn check_dom(config: &Config) -> (Check, Option<DriverInfo>) {
    let spec = match driver::resolve_without_forwards(Surface::Web, config) {
        Ok(spec) => spec,
        Err(err) => return (Check::fail("driver.dom", err.to_string()), None),
    };
    let Some(program) = spec.argv.first() else {
        return (
            Check::fail("driver.dom", format!("{} has no command to run", spec.name)),
            None,
        );
    };
    if which(program).is_none() && !std::path::Path::new(program).is_file() {
        return (
            Check::fail("driver.dom", format!("{program} is not executable")),
            None,
        );
    }
    let mut conn = match Connection::spawn(&spec, config.rpc_timeout) {
        Ok(conn) => conn,
        Err(err) => return (Check::fail("driver.dom", format!("{err:#}")), None),
    };
    let pid = conn.pid().unwrap_or_default();
    let answered = match conn.info() {
        Ok(info) => (
            Check::pass(
                "driver.dom",
                format!("{} {} speaks {:?}", info.name, info.version, info.surfaces),
            ),
            Some(info),
        ),
        Err(err) => (
            Check::fail("driver.dom", format!("{}: {err:#}", spec.display())),
            None,
        ),
    };
    terminate(pid);
    answered
}

fn check_tauri(config: &Config, info: Option<&DriverInfo>) -> Check {
    let severity = match config.surface == Some(Surface::Tauri) {
        true => Severity::Blocking,
        false => Severity::Advisory,
    };
    let check = match info {
        None => Check::unknown(
            "driver.tauri",
            "driver.dom did not answer, so nothing reported on the tauri surface",
        ),
        Some(info) => match &info.tauri {
            None => Check::unknown(
                "driver.tauri",
                format!(
                    "{} {} reports no readable tauri block on driver.info, so whether \
                     tauri-driver resolves on the driver host is unreported, not confirmed",
                    info.name, info.version
                ),
            ),
            Some(tauri) if tauri.ok => Check::pass("driver.tauri", resolved_paths(tauri)),
            Some(tauri) => Check::fail(
                "driver.tauri",
                tauri.reason.clone().unwrap_or_else(|| {
                    format!(
                        "{} reports the tauri surface unusable and gave no reason",
                        info.name
                    )
                }),
            ),
        },
    };
    check.severity(severity)
}

fn resolved_paths(tauri: &TauriInfo) -> String {
    let mut parts = Vec::new();
    if let Some(path) = &tauri.tauri_driver {
        parts.push(format!("tauri-driver {path}"));
    }
    if let Some(path) = &tauri.native_driver {
        parts.push(format!("native driver {path}"));
    }
    if parts.is_empty() {
        parts.push("usable, paths not reported".to_string());
    }
    match tauri.source.as_ref().and_then(describe_source) {
        Some(source) => format!("{} (from {source})", parts.join(", ")),
        None => parts.join(", "),
    }
}

fn describe_source(source: &Value) -> Option<String> {
    match source {
        Value::Null => None,
        Value::String(text) => (!text.trim().is_empty()).then(|| text.clone()),
        Value::Object(fields) => {
            let described: Vec<String> = fields
                .iter()
                .map(|(key, value)| match value.as_str() {
                    Some(text) => format!("{key}={text}"),
                    None => format!("{key}={value}"),
                })
                .collect();
            (!described.is_empty()).then(|| described.join(", "))
        }
        other => Some(other.to_string()),
    }
}

fn check_tui() -> Check {
    Check::pass(
        "driver.tui",
        "not implemented yet, `--surface tui` fails with a clear error",
    )
}

enum Probe {
    Answered(String),
    Refused,
    TimedOut,
    Unresolved,
    Failed(i32, String),
    Unreachable(String),
}

fn probe_url(host: &str, port: u16) -> String {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.contains(':') {
        true => format!("http://[{bare}]:{port}/"),
        false => format!("http://{bare}:{port}/"),
    }
}

fn probe_from_lab(backend: &dyn Backend, host: &str, port: u16) -> Probe {
    let url = probe_url(host, port);
    let script = format!(
        "curl -sS -o /dev/null -m {PROBE_TIMEOUT} -w '%{{http_code}}' {}",
        shell_quote(&url)
    );
    let output = match backend.run(&Cmd::shell(script)) {
        Ok(output) => output,
        Err(err) => return Probe::Unreachable(format!("{err:#}")),
    };
    match output.code {
        0 => Probe::Answered(output.trimmed_stdout().to_string()),
        6 => Probe::Unresolved,
        7 => Probe::Refused,
        28 => Probe::TimedOut,
        code => Probe::Failed(code, first_line(&output.stderr)),
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output")
        .to_string()
}

fn local_listener(host: &str, port: u16) -> Result<(), String> {
    let addrs: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(err) => return Err(format!("{host} does not resolve here: {err}")),
    };
    if addrs.is_empty() {
        return Err(format!("{host} does not resolve here"));
    }
    let mut failure = String::new();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, PREFLIGHT_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(err) => failure = format!("{addr}: {err}"),
        }
    }
    Err(failure)
}

fn check_forward(config: &Config, lab: Option<&dyn Backend>) -> Option<Check> {
    let forwards = declared_forwards(config);
    if forwards.is_empty() {
        return None;
    }
    if config.backend.host().is_none() {
        return Some(Check::pass(
            "forward",
            format!(
                "{} declared, and the backend is local://, where the target already \
                 resolves to this machine",
                forwards.len()
            ),
        ));
    }

    let mut ok = true;
    let mut lines = Vec::new();
    for (lab_port, local_host, local_port) in &forwards {
        let here = match local_listener(local_host, *local_port) {
            Ok(()) => format!("{local_host}:{local_port} is listening here"),
            Err(err) => {
                ok = false;
                format!(
                    "nothing is listening on {local_host}:{local_port} here, so lab {lab_port} \
                     would publish a dead port ({err})"
                )
            }
        };
        let there = match lab {
            None => "the backend did not answer, so the lab end was not probed".to_string(),
            Some(lab) => match probe_from_lab(lab, LAB_LOOPBACK, *lab_port) {
                Probe::Refused => {
                    format!("lab {LAB_LOOPBACK}:{lab_port} is free for ssh -R to bind")
                }
                Probe::Answered(status) => format!(
                    "something already answers on lab {LAB_LOOPBACK}:{lab_port} ({status}); a \
                     live ui-box forward, or a listener that will make ssh -R fail to bind"
                ),
                Probe::TimedOut => format!(
                    "lab {LAB_LOOPBACK}:{lab_port} did not answer within {PROBE_TIMEOUT}s, so \
                     whether it is free is unreported"
                ),
                Probe::Unresolved => format!("the lab cannot resolve {LAB_LOOPBACK}"),
                Probe::Failed(code, stderr) => {
                    format!("probing lab {lab_port} exited {code}: {stderr}")
                }
                Probe::Unreachable(err) => format!("probing lab {lab_port} failed: {err}"),
            },
        };
        lines.push(format!(
            "{lab_port} -> {local_host}:{local_port}: {here}; {there}"
        ));
    }

    let detail = lines.join(" | ");
    Some(match ok {
        true => Check::pass("forward", detail),
        false => Check::fail("forward", detail),
    })
}

fn check_target(config: &Config, lab: Option<&dyn Backend>) -> Option<Check> {
    let target = config.target.as_deref()?;
    let (host, port) = http_endpoint(target)?;
    let Some(lab) = lab else {
        return Some(Check::unknown(
            "target",
            format!("the backend did not answer, so {target} was not probed from the lab"),
        ));
    };
    Some(match probe_from_lab(lab, &host, port) {
        Probe::Answered(status) => Check::pass(
            "target",
            format!("{target} answered {status} from {}", lab.url()),
        ),
        Probe::Refused => Check::fail("target", refused(config, target, &host, port)),
        Probe::TimedOut => Check::fail(
            "target",
            format!(
                "{target} did not answer within {PROBE_TIMEOUT}s from {}",
                lab.url()
            ),
        ),
        Probe::Unresolved => Check::fail("target", format!("{} cannot resolve {host}", lab.url())),
        Probe::Failed(code, stderr) => Check::unknown(
            "target",
            format!(
                "probing {target} from {} exited {code}: {stderr}",
                lab.url()
            ),
        ),
        Probe::Unreachable(err) => Check::unknown(
            "target",
            format!("probing {target} from {} failed: {err}", lab.url()),
        ),
    })
}

fn refused(config: &Config, target: &str, host: &str, port: u16) -> String {
    if !is_loopback(host) {
        return format!("{target} refused the connection from the lab. Nothing answers there.");
    }
    if config.backend.host().is_none() {
        return format!("{target} refused the connection. Nothing is listening on {host}:{port}.");
    }
    if declared_forwards(config)
        .iter()
        .any(|(lab_port, _, _)| *lab_port == port)
    {
        return format!(
            "{target} names the lab's own loopback. Nothing answers there, even though a \
             forward publishes {port} into the lab. Check that this machine is serving it."
        );
    }
    format!(
        "{target} names the lab's own loopback. Nothing answers there. Pass --forward {port} \
         to publish this machine's port {port} into the lab."
    )
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

fn http_endpoint(target: &str) -> Option<(String, u16)> {
    let (scheme, rest) = target.split_once("://")?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if let Some(tail) = authority.strip_prefix('[') {
        let (host, rest) = tail.split_once(']')?;
        let port = match rest.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None => default,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default)),
    }
}

fn declared_forwards(config: &Config) -> Vec<(u16, String, u16)> {
    config
        .forward
        .iter()
        .map(|forward| {
            (
                forward.lab_port,
                forward.connect_host().to_string(),
                forward.local_port,
            )
        })
        .collect()
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

    fn info_with(tauri: Option<TauriInfo>) -> DriverInfo {
        DriverInfo {
            name: "dom".to_string(),
            version: "0.1".to_string(),
            surfaces: vec!["web".to_string(), "tauri".to_string()],
            tauri,
        }
    }

    fn config_for(surface: Option<Surface>) -> Config {
        let mut config =
            Config::resolve_from(&Default::default(), std::path::Path::new("/")).expect("config");
        config.surface = surface;
        config
    }

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

    #[test]
    fn a_driver_too_old_to_answer_is_not_a_tauri_pass() {
        let check = check_tauri(&config_for(None), Some(&info_with(None)));
        assert!(!check.ok);
        assert!(check.detail.starts_with("unknown:"), "{}", check.detail);
        assert_eq!(check.severity, Severity::Advisory);
    }

    #[test]
    fn an_unanswered_driver_is_not_a_tauri_pass() {
        let check = check_tauri(&config_for(None), None);
        assert!(!check.ok);
        assert!(check.detail.starts_with("unknown:"), "{}", check.detail);
    }

    #[test]
    fn a_driver_that_says_no_is_quoted_verbatim() {
        let check = check_tauri(
            &config_for(None),
            Some(&info_with(Some(TauriInfo {
                ok: false,
                tauri_driver: None,
                native_driver: None,
                source: None,
                reason: Some("tauri-driver not on PATH".to_string()),
            }))),
        );
        assert!(!check.ok);
        assert_eq!(check.detail, "tauri-driver not on PATH");
    }

    #[test]
    fn declaring_the_tauri_surface_makes_its_driver_blocking() {
        let missing = check_tauri(&config_for(Some(Surface::Tauri)), Some(&info_with(None)));
        assert_eq!(missing.severity, Severity::Blocking);
        assert!(missing.blocks());
        let web = check_tauri(&config_for(Some(Surface::Web)), Some(&info_with(None)));
        assert_eq!(web.severity, Severity::Advisory);
        assert!(!web.blocks());
    }

    #[test]
    fn a_usable_tauri_surface_reports_the_paths_the_driver_resolved() {
        let check = check_tauri(
            &config_for(None),
            Some(&info_with(Some(TauriInfo {
                ok: true,
                tauri_driver: Some("/nix/store/aaa/bin/tauri-driver".to_string()),
                native_driver: Some("/nix/store/bbb/bin/WebKitWebDriver".to_string()),
                source: Some(json!({"tauriDriver": "PATH", "nativeDriver": "PATH"})),
                reason: None,
            }))),
        );
        assert!(check.ok);
        assert!(check.detail.contains("/nix/store/aaa/bin/tauri-driver"));
        assert!(check.detail.contains("/nix/store/bbb/bin/WebKitWebDriver"));
        assert!(check.detail.contains("PATH"));
    }

    #[test]
    fn a_target_is_split_into_a_host_and_a_port() {
        assert_eq!(
            http_endpoint("http://localhost:3000/app"),
            Some(("localhost".to_string(), 3000))
        );
        assert_eq!(
            http_endpoint("https://example.test"),
            Some(("example.test".to_string(), 443))
        );
        assert_eq!(
            http_endpoint("http://[::1]:5173"),
            Some(("::1".to_string(), 5173))
        );
        assert_eq!(http_endpoint("exec:/nix/store/abc/bin/app"), None);
        assert_eq!(http_endpoint("tui:nsql"), None);
    }

    #[test]
    fn a_loopback_target_on_a_lab_names_the_forward_that_would_reach_it() {
        let mut config = config_for(None);
        config.backend = crate::config::BackendSpec::parse("ssh://fredrir@dlab-ui").unwrap();
        let detail = refused(&config, "http://localhost:3000", "localhost", 3000);
        assert!(detail.contains("the lab's own loopback"), "{detail}");
        assert!(detail.contains("--forward 3000"), "{detail}");
    }

    #[test]
    fn a_routable_target_is_not_blamed_on_a_missing_forward() {
        let mut config = config_for(None);
        config.backend = crate::config::BackendSpec::parse("ssh://fredrir@dlab-ui").unwrap();
        let detail = refused(&config, "http://example.test:80", "example.test", 80);
        assert!(!detail.contains("--forward"), "{detail}");
    }

    #[test]
    fn an_ipv6_host_is_bracketed_before_it_reaches_curl() {
        assert_eq!(probe_url("::1", 5173), "http://[::1]:5173/");
        assert_eq!(probe_url("[::1]", 5173), "http://[::1]:5173/");
        assert_eq!(probe_url("127.0.0.1", 3000), "http://127.0.0.1:3000/");
        assert_eq!(probe_url("localhost", 3000), "http://localhost:3000/");
    }

    #[test]
    fn loopback_is_recognised_in_every_spelling() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("127.1.2.3"));
        assert!(is_loopback("::1"));
        assert!(!is_loopback("dlab-ui"));
        assert!(!is_loopback("10.0.0.1"));
    }
}
