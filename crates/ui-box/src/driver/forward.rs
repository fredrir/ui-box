use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;

use super::carries_own_transport;
use crate::config::{Config, Forward, DEFAULT_FORWARD_HOST};
use crate::driver::client::process_alive;
use crate::error::ForwardError;
use crate::note;
use crate::session::SessionStore;

pub const PREFLIGHT_TIMEOUT: Duration = Duration::from_millis(300);

pub const EXCLUSIVE_OPTIONS: [&str; 3] = [
    "ExitOnForwardFailure=yes",
    "ControlMaster=no",
    "ControlPath=none",
];

const REFUSAL_MARKERS: [&str; 3] = [
    "remote port forwarding failed",
    "bind: address already in use",
    "administratively prohibited",
];

pub fn loopback_target(target: &str) -> Option<(String, u16)> {
    let (scheme, rest) = target.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let (host, port) = split_host_port(authority)?;
    if !is_loopback(&host) {
        return None;
    }
    let port = port.or_else(|| default_port(&scheme.to_ascii_lowercase()))?;
    Some((host, port))
}

pub fn ssh_args(forwards: &[Forward]) -> Vec<String> {
    if forwards.is_empty() {
        return Vec::new();
    }
    let mut args = Vec::new();
    for forward in forwards {
        args.push("-R".to_string());
        args.push(format!(
            "{DEFAULT_FORWARD_HOST}:{}:{}:{}",
            forward.lab_port,
            ssh_host(forward),
            forward.local_port
        ));
    }
    for option in EXCLUSIVE_OPTIONS {
        args.push("-o".to_string());
        args.push(option.to_string());
    }
    args
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEnd {
    Listening,
    Ipv6Only,
    Closed,
}

pub fn probe_local_end(forward: &Forward) -> LocalEnd {
    if listening(forward.connect_host(), forward.local_port) {
        return LocalEnd::Listening;
    }
    if forward.is_default_host() && listening("::1", forward.local_port) {
        return LocalEnd::Ipv6Only;
    }
    LocalEnd::Closed
}

pub fn labels(forwards: &[Forward]) -> String {
    forwards
        .iter()
        .map(Forward::label)
        .collect::<Vec<String>>()
        .join(", ")
}

pub fn guard(config: &Config, target: &str, invocation: &str) -> Result<()> {
    let Some(lab) = config.backend.host().map(str::to_string) else {
        if !config.forward.is_empty() {
            note!(
                "ignoring --forward {}: the backend is local://, where the driver already \
                 reaches this machine's ports",
                labels(&config.forward)
            );
        }
        return Ok(());
    };

    if let Some(command) = verbatim_transport(config) {
        if !config.forward.is_empty() {
            return Err(ForwardError::OwnTransport {
                command,
                specs: labels(&config.forward),
                flags: ssh_args(&config.forward).join(" "),
            }
            .into());
        }
    }

    if let Some((host, port)) = loopback_target(target) {
        if !config
            .forward
            .iter()
            .any(|forward| forward.lab_port == port)
        {
            return Err(ForwardError::Missing {
                target: target.to_string(),
                host,
                port,
                lab,
                command: invocation.to_string(),
            }
            .into());
        }
    }

    for forward in &config.forward {
        preflight(forward)?;
    }
    if !config.forward.is_empty() {
        note!(
            "forwarding {} into {lab} over the driver's own connection",
            labels(&config.forward)
        );
    }
    Ok(())
}

pub fn classify(error: anyhow::Error, config: &Config) -> anyhow::Error {
    if config.forward.is_empty() {
        return error;
    }
    let Some(lab) = config.backend.host() else {
        return error;
    };
    let rendered = format!("{error:#}");
    if !matches_refusal(&rendered) {
        return error;
    }
    if let Some((port, session)) = holding_session(config, &rendered) {
        return ForwardError::HeldBySession {
            session,
            port,
            lab: lab.to_string(),
            log: refusal_lines(&rendered),
        }
        .into();
    }
    ForwardError::Refused {
        ports: refused_ports(&config.forward, &rendered),
        lab: lab.to_string(),
        control_persist: control_persist(lab),
        log: refusal_lines(&rendered),
    }
    .into()
}

fn holding_session(config: &Config, text: &str) -> Option<(u16, String)> {
    let sessions = SessionStore::new(config).list().ok()?;
    for forward in &config.forward {
        if !text.contains(&forward.lab_port.to_string()) {
            continue;
        }
        let prefix = format!("{DEFAULT_FORWARD_HOST}:{}:", forward.lab_port);
        let held = sessions.iter().find(|record| {
            process_alive(record.pid)
                && record
                    .driver_argv
                    .iter()
                    .any(|arg| arg.starts_with(&prefix))
        });
        if let Some(record) = held {
            return Some((forward.lab_port, record.id.clone()));
        }
    }
    None
}

fn control_persist(host: &str) -> Option<String> {
    let output = std::process::Command::new("ssh")
        .arg("-G")
        .arg(host)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix("controlpersist "))?
        .trim();
    match value {
        "no" | "0" => None,
        "yes" => Some("as long as it stays idle".to_string()),
        seconds => seconds.parse::<u64>().ok().map(|held| format!("{held}s")),
    }
}

fn preflight(forward: &Forward) -> Result<()> {
    match probe_local_end(forward) {
        LocalEnd::Listening => Ok(()),
        LocalEnd::Ipv6Only => Err(ForwardError::LocalIpv6Only {
            spec: forward.label(),
            local: forward.local_port,
            suggestion: format!("{}:[::1]:{}", forward.lab_port, forward.local_port),
        }
        .into()),
        LocalEnd::Closed => Err(ForwardError::LocalClosed {
            spec: forward.label(),
            endpoint: endpoint(forward.connect_host(), forward.local_port),
        }
        .into()),
    }
}

fn listening(host: &str, port: u16) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, PREFLIGHT_TIMEOUT).is_ok())
}

fn verbatim_transport(config: &Config) -> Option<String> {
    let command = config.driver_dom.as_ref()?;
    let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    (!argv.is_empty() && carries_own_transport(&argv)).then(|| command.clone())
}

fn matches_refusal(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    REFUSAL_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn refusal_lines(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().filter(|line| matches_refusal(line)).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn refused_ports(forwards: &[Forward], text: &str) -> String {
    let named: Vec<String> = forwards
        .iter()
        .filter(|forward| text.contains(&forward.lab_port.to_string()))
        .map(|forward| forward.lab_port.to_string())
        .collect();
    let ports = if named.is_empty() {
        forwards
            .iter()
            .map(|forward| forward.lab_port.to_string())
            .collect()
    } else {
        named
    };
    ports.join(", ")
}

fn ssh_host(forward: &Forward) -> String {
    let host = forward.connect_host();
    match IpAddr::from_str(host) {
        Ok(IpAddr::V6(_)) => format!("[{host}]"),
        _ => host.to_string(),
    }
}

fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn split_host_port(authority: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(port.parse().ok()?),
            None => None,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), Some(port.parse().ok()?))),
        None => Some((authority.to_string(), None)),
    }
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match IpAddr::from_str(host) {
        Ok(addr) => addr.is_loopback() || addr.is_unspecified(),
        Err(_) => false,
    }
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND: u16 = 1;

    fn config_for(backend: &str, forward: &str) -> Config {
        crate::config::Config::resolve_from(
            &crate::config::Overrides {
                backend: Some(backend.to_string()),
                forward: forward
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<String>>(),
                ..Default::default()
            },
            std::path::Path::new("/"),
        )
        .expect("config")
    }

    #[test]
    fn a_loopback_target_names_its_port() {
        assert_eq!(
            loopback_target("http://localhost:3000"),
            Some(("localhost".to_string(), 3000))
        );
        assert_eq!(
            loopback_target("http://127.0.0.1:5173/app?x=1"),
            Some(("127.0.0.1".to_string(), 5173))
        );
        assert_eq!(
            loopback_target("http://127.9.9.9:8080"),
            Some(("127.9.9.9".to_string(), 8080))
        );
        assert_eq!(
            loopback_target("http://[::1]:3000"),
            Some(("::1".to_string(), 3000))
        );
        assert_eq!(
            loopback_target("http://0.0.0.0:3000"),
            Some(("0.0.0.0".to_string(), 3000))
        );
        assert_eq!(
            loopback_target("https://localhost"),
            Some(("localhost".to_string(), 443))
        );
    }

    #[test]
    fn a_target_that_is_not_loopback_is_not_forwarded() {
        for target in [
            "http://ui-box-backend:3000",
            "http://10.0.0.4:3000",
            "exec:/nix/store/abc-app/bin/app",
            "tui:nsql",
            "",
        ] {
            assert_eq!(loopback_target(target), None, "{target:?}");
        }
    }

    #[test]
    fn a_loopback_target_without_a_forward_is_refused_by_port() {
        let config = config_for("ssh://fredrir@ui-box-backend", "");
        let err = guard(
            &config,
            "http://localhost:3000",
            "ui-box open http://localhost:3000",
        )
        .expect_err("a loopback target on an ssh backend must be refused");
        assert_eq!(crate::error::kind_of(&err), "forward_missing");
        let message = format!("{err:#}");
        assert!(message.contains("3000"), "{message}");
        assert!(message.contains("The UI was never asked"), "{message}");
        assert!(message.contains("--forward 3000"), "{message}");
    }

    #[test]
    fn a_covering_forward_lets_a_loopback_target_through() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("addr").port();
        let config = config_for("ssh://fredrir@ui-box-backend", &format!("3000:{port}"));
        guard(&config, "http://localhost:3000", "ui-box open").expect("declared forward covers it");
    }

    #[test]
    fn a_forward_whose_local_end_is_closed_is_refused() {
        let port = PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND;
        let config = config_for("ssh://fredrir@ui-box-backend", &format!("3000:{port}"));
        let err = guard(&config, "http://localhost:3000", "ui-box open")
            .expect_err("nothing is listening on the local end");
        let message = format!("{err:#}");
        assert!(message.contains(&port.to_string()), "{message}");
        assert!(
            matches!(
                crate::error::kind_of(&err),
                "forward_unreachable" | "forward_ipv6_only"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_default_host_forward_is_probed_at_the_address_it_names() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("addr").port();
        let open = crate::config::parse_forwards(&format!("3000:{port}")).expect("forwards");
        assert_eq!(probe_local_end(&open[0]), LocalEnd::Listening);

        let closed =
            crate::config::parse_forwards(&format!("3000:{PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND}"))
                .expect("forwards");
        assert_eq!(probe_local_end(&closed[0]), LocalEnd::Closed);
    }

    #[test]
    fn an_ipv6_only_listener_is_not_a_listening_default_host_forward() {
        let Ok(listener) = TcpListener::bind("[::1]:0") else {
            return;
        };
        let port = listener.local_addr().expect("addr").port();
        let named = crate::config::parse_forwards(&format!("3000:{port}")).expect("forwards");
        assert_eq!(
            probe_local_end(&named[0]),
            LocalEnd::Ipv6Only,
            "the spec names 127.0.0.1, which is the address ssh -R will connect to"
        );

        let explicit =
            crate::config::parse_forwards(&format!("3000:[::1]:{port}")).expect("forwards");
        assert_eq!(probe_local_end(&explicit[0]), LocalEnd::Listening);
    }

    #[test]
    fn a_forward_on_a_local_backend_is_ignored_not_refused() {
        let config = config_for("local://", "3000");
        guard(&config, "http://localhost:3000", "ui-box open")
            .expect("local:// already reaches this machine's ports");
    }

    #[test]
    fn a_verbatim_ssh_driver_cannot_carry_a_forward() {
        let mut config = config_for("ssh://fredrir@ui-box-backend", "3000");
        config.driver_dom = Some("ssh ui-box-backend ui-box-dom".to_string());
        let err = guard(&config, "http://localhost:3000", "ui-box open")
            .expect_err("dropping the forward silently is the bug this removes");
        assert_eq!(crate::error::kind_of(&err), "forward_unsupported");
        assert!(format!("{err:#}").contains("-R 127.0.0.1:3000:127.0.0.1:3000"));
    }

    #[test]
    fn both_ends_of_a_forward_bind_loopback() {
        let forwards = crate::config::parse_forwards("3000:5173").expect("forwards");
        let args = ssh_args(&forwards);
        assert_eq!(args[0], "-R");
        assert_eq!(args[1], "127.0.0.1:3000:127.0.0.1:5173");
        assert_eq!(
            ssh_args(&crate::config::parse_forwards("3000:[::1]:5173").expect("forwards"))[1],
            "127.0.0.1:3000:[::1]:5173"
        );
        assert!(ssh_args(&[]).is_empty());
    }

    #[test]
    fn a_refused_remote_bind_is_not_reported_as_a_broken_ui() {
        let config = config_for("ssh://fredrir@ui-box-backend", "3000");
        let raw = anyhow::anyhow!(
            "driver dom@ui-box-backend exited before answering info\n\
             Warning: remote port forwarding failed for listen port 3000"
        );
        let classified = classify(raw, &config);
        assert_eq!(crate::error::kind_of(&classified), "forward_refused");
        let message = format!("{classified:#}");
        assert!(message.contains("3000"), "{message}");
        assert!(message.contains("ui-box-backend"), "{message}");
    }

    fn live_session_holding(dir: &std::path::Path, port: u16) -> (Config, String) {
        let config = crate::config::Config::resolve_from(
            &crate::config::Overrides {
                backend: Some("ssh://fredrir@ui-box-backend".to_string()),
                forward: vec![port.to_string()],
                artifacts: Some(dir.to_path_buf()),
                ..Default::default()
            },
            std::path::Path::new("/"),
        )
        .expect("config");

        let id = "20260830T000000Z-abcdef01".to_string();
        let record = crate::session::SessionRecord {
            id: id.clone(),
            driver_session: "d1".to_string(),
            driver_name: "dom@ui-box-backend".to_string(),
            driver_argv: super::super::remote_argv(
                "fredrir@ui-box-backend",
                &["ui-box-dom".to_string()],
                &config.forward,
            ),
            pid: std::process::id(),
            surface: crate::config::Surface::Web,
            target: "http://localhost:3000".to_string(),
            viewport: config.viewport,
            backend: config.backend.url(),
            run_dir: dir.to_path_buf(),
            remote_run_dir: None,
            session_dir: dir.join("session"),
            created_unix: 0,
            last_used_unix: 0,
            ttl_secs: 900,
            step_count: 0,
        };
        SessionStore::new(&config).save(&record).expect("session");
        (config, id)
    }

    #[test]
    fn a_port_held_by_a_live_session_names_that_session() {
        let dir = std::env::temp_dir().join(format!("uibox-holder-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let (config, id) = live_session_holding(&dir, 3000);

        let raw = anyhow::anyhow!(
            "driver dom@ui-box-backend exited before answering info\n\
             Warning: remote port forwarding failed for listen port 3000"
        );
        let classified = classify(raw, &config);
        assert_eq!(crate::error::kind_of(&classified), "forward_held");
        let message = format!("{classified:#}");
        assert!(message.contains(&id), "{message}");
        assert!(message.contains(&format!("ui-box close {id}")), "{message}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_port_no_session_holds_still_lists_what_else_could_hold_it() {
        let dir = std::env::temp_dir().join(format!("uibox-noholder-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("dir");
        let (config, _) = live_session_holding(&dir, 4000);

        let raw = anyhow::anyhow!(
            "driver dom@ui-box-backend exited before answering info\n\
             Warning: remote port forwarding failed for listen port 4000"
        );
        std::fs::remove_dir_all(config.sessions_dir()).ok();
        let classified = classify(raw, &config);
        assert_eq!(crate::error::kind_of(&classified), "forward_refused");
        assert!(format!("{classified:#}").contains("4000"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unrelated_driver_failure_stays_what_it_was() {
        let config = config_for("ssh://fredrir@ui-box-backend", "3000");
        let raw = anyhow::anyhow!("driver dom@ui-box-backend exited before answering info");
        assert_eq!(crate::error::kind_of(&classify(raw, &config)), "error");
    }
}
