use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;

use super::carries_own_transport;
use crate::config::{Config, Forward, DEFAULT_FORWARD_HOST};
use crate::error::ForwardError;
use crate::note;

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
    ForwardError::Refused {
        ports: refused_ports(&config.forward, &rendered),
        lab: lab.to_string(),
        log: refusal_lines(&rendered),
    }
    .into()
}

fn preflight(forward: &Forward) -> Result<()> {
    let host = forward.connect_host();
    if listening(host, forward.local_port) {
        return Ok(());
    }
    if forward.is_default_host() && listening("::1", forward.local_port) {
        return Err(ForwardError::LocalIpv6Only {
            spec: forward.label(),
            local: forward.local_port,
            suggestion: format!("{}:[::1]:{}", forward.lab_port, forward.local_port),
        }
        .into());
    }
    Err(ForwardError::LocalClosed {
        spec: forward.label(),
        endpoint: endpoint(host, forward.local_port),
    }
    .into())
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
            "http://dlab-ui:3000",
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
        let config = config_for("ssh://fredrir@dlab-ui", "");
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
        let config = config_for("ssh://fredrir@dlab-ui", &format!("3000:{port}"));
        guard(&config, "http://localhost:3000", "ui-box open").expect("declared forward covers it");
    }

    #[test]
    fn a_forward_whose_local_end_is_closed_is_refused() {
        let port = PORT_NO_UNPRIVILEGED_PROCESS_CAN_BIND;
        let config = config_for("ssh://fredrir@dlab-ui", &format!("3000:{port}"));
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
    fn a_forward_on_a_local_backend_is_ignored_not_refused() {
        let config = config_for("local://", "3000");
        guard(&config, "http://localhost:3000", "ui-box open")
            .expect("local:// already reaches this machine's ports");
    }

    #[test]
    fn a_verbatim_ssh_driver_cannot_carry_a_forward() {
        let mut config = config_for("ssh://fredrir@dlab-ui", "3000");
        config.driver_dom = Some("ssh dlab-ui ui-box-dom".to_string());
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
        let config = config_for("ssh://fredrir@dlab-ui", "3000");
        let raw = anyhow::anyhow!(
            "driver dom@dlab-ui exited before answering info\n\
             Warning: remote port forwarding failed for listen port 3000"
        );
        let classified = classify(raw, &config);
        assert_eq!(crate::error::kind_of(&classified), "forward_refused");
        let message = format!("{classified:#}");
        assert!(message.contains("3000"), "{message}");
        assert!(message.contains("dlab-ui"), "{message}");
    }

    #[test]
    fn an_unrelated_driver_failure_stays_what_it_was() {
        let config = config_for("ssh://fredrir@dlab-ui", "3000");
        let raw = anyhow::anyhow!("driver dom@dlab-ui exited before answering info");
        assert_eq!(crate::error::kind_of(&classify(raw, &config)), "error");
    }
}
