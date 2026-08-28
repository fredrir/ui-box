use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output};

use crate::sh;

fn local(program: &str, args: &[String]) -> Result<Output> {
    Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("could not run {} on the host", program))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_string()
}

const VALUE_FLAGS: &[&str] = &[
    "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p", "-Q",
    "-R", "-S", "-W", "-w",
];

pub enum Mediator {
    Here,
    Over(String),
}

pub fn mediator(target: &str) -> Mediator {
    if let Ok(forced) = std::env::var("UIBOX_COPY_VIA") {
        let forced = forced.trim();
        if forced.eq_ignore_ascii_case("local") {
            return Mediator::Here;
        }
        if !forced.is_empty() {
            return Mediator::Over(forced.to_string());
        }
    }

    match proxy_command(target).as_deref().and_then(parse_proxy_hop) {
        Some(hop) => Mediator::Over(hop),
        None => Mediator::Here,
    }
}

fn proxy_command(target: &str) -> Option<String> {
    let output = local("ssh", &["-G".to_string(), target.to_string()]).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("proxycommand ").map(str::to_string))
}

fn parse_proxy_hop(proxycommand: &str) -> Option<String> {
    let mut tokens = proxycommand.split_whitespace();
    if tokens.next()? != "ssh" {
        return None;
    }

    let mut skip_next = false;
    for token in tokens {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token.starts_with('-') {
            skip_next = VALUE_FLAGS.contains(&token);
            continue;
        }
        return Some(token.to_string());
    }
    None
}

impl Mediator {
    fn dispatch(&self, argv: &[String]) -> Result<Output> {
        match self {
            Mediator::Here => local(&argv[0], &argv[1..]),
            Mediator::Over(hop) => {
                let script = argv
                    .iter()
                    .map(|token| sh::quote(token))
                    .collect::<Vec<_>>()
                    .join(" ");
                local("ssh", &[hop.clone(), script])
            }
        }
    }

    fn shell(&self, script: &str) -> Result<Output> {
        match self {
            Mediator::Here => local("sh", &["-c".to_string(), script.to_string()]),
            Mediator::Over(hop) => local("ssh", &[hop.clone(), script.to_string()]),
        }
    }
}

pub fn forced(command: &str, force: Option<&str>) -> String {
    match force {
        Some(value) if !value.trim().is_empty() => {
            format!("DLAB_FORCE={} {}", sh::quote(value.trim()), command)
        }
        _ => command.to_string(),
    }
}

pub fn wake(via: &Mediator, lab: &str) -> Result<()> {
    let output = match via {
        Mediator::Here => local("ssh", &[lab.to_string(), "true".to_string()])?,
        Mediator::Over(hop) => {
            let inner = format!("ssh {} true", sh::quote(lab));
            let force = std::env::var("DLAB_FORCE").ok();
            let script = forced(&inner, force.as_deref());
            local("ssh", &[hop.clone(), script])?
        }
    };

    if !output.status.success() {
        bail!("{}", stderr_of(&output));
    }
    Ok(())
}

pub fn path_exists(lab: &str, path: &Path) -> Result<bool> {
    let output = local(
        "ssh",
        &[lab.to_string(), format!("test -e {}", sh::quote_path(path))],
    )?;
    Ok(output.status.success())
}

pub fn nix_copy(via: &Mediator, from: &str, to: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut argv = vec![
        "nix".to_string(),
        "copy".to_string(),
        "--no-check-sigs".to_string(),
        "--from".to_string(),
        format!("ssh://{}", from),
        "--to".to_string(),
        format!("ssh://{}", to),
    ];
    argv.extend(paths.iter().cloned());

    let output = via.dispatch(&argv)?;
    if !output.status.success() {
        bail!("{}", stderr_of(&output));
    }
    Ok(())
}

const RELAY_SCRIPT: &str = r#"
set -e
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
rsync -a @FROM@:@ARTIFACT@ "$staging"/@NAME@
ssh @TO@ mkdir -p @PARENT@
rsync -a "$staging"/@NAME@ @TO@:@REMOTE@
"#;

pub fn relay(
    via: &Mediator,
    from: &str,
    artifact: &Path,
    to: &str,
    remote_path: &Path,
) -> Result<()> {
    let parent = remote_path.parent().unwrap_or(Path::new("."));
    let name = remote_path
        .file_name()
        .map(|raw| raw.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact".to_string());

    let script = RELAY_SCRIPT
        .replace("@FROM@", &sh::quote(from))
        .replace("@ARTIFACT@", &sh::quote_path(artifact))
        .replace("@TO@", &sh::quote(to))
        .replace("@PARENT@", &sh::quote_path(parent))
        .replace("@REMOTE@", &sh::quote_path(remote_path))
        .replace("@NAME@", &sh::quote(&name));

    let output = via.shell(&script)?;
    if !output.status.success() {
        bail!("{}", stderr_of(&output));
    }
    Ok(())
}

pub fn send_file(lab: &str, local_path: &Path, remote_path: &Path) -> Result<()> {
    if let Some(parent) = remote_path.parent() {
        let made = local(
            "ssh",
            &[
                lab.to_string(),
                format!("mkdir -p {}", sh::quote_path(parent)),
            ],
        )?;
        if !made.status.success() {
            bail!("{}", stderr_of(&made));
        }
    }

    let output = local(
        "rsync",
        &[
            "-a".to_string(),
            local_path.to_string_lossy().to_string(),
            format!("{}:{}", lab, remote_path.to_string_lossy()),
        ],
    )?;
    if !output.status.success() {
        bail!("{}", stderr_of(&output));
    }
    Ok(())
}

pub fn home(lab: &str) -> Result<std::path::PathBuf> {
    let output = local("ssh", &[lab.to_string(), "printf %s \"$HOME\"".to_string()])?;
    if !output.status.success() {
        bail!("{}", stderr_of(&output));
    }
    let home = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if home.is_empty() {
        bail!("{} did not report a home directory", lab);
    }
    Ok(std::path::PathBuf::from(home))
}

#[cfg(test)]
mod hop_tests {
    use super::*;

    #[test]
    fn finds_the_hop_in_the_remote_workstation_config() {
        let proxycommand = "ssh -T -o LogLevel=ERROR archie /home/fredrir/projects/distro-lab/src/vm/bin/dlab-ssh-proxy %h %p";
        assert_eq!(parse_proxy_hop(proxycommand), Some("archie".to_string()));
    }

    #[test]
    fn finds_no_hop_when_the_proxy_runs_locally() {
        let proxycommand = "/home/fredrir/projects/distro-lab/src/vm/bin/dlab-ssh-proxy %h %p";
        assert_eq!(parse_proxy_hop(proxycommand), None);
    }

    #[test]
    fn does_not_mistake_a_flag_value_for_the_hop() {
        assert_eq!(
            parse_proxy_hop("ssh -o LogLevel=ERROR -p 2222 archie nc %h %p"),
            Some("archie".to_string())
        );
    }

    #[test]
    fn carries_the_force_flag_over_the_hop() {
        assert_eq!(
            forced("ssh dlab-ui true", Some("1")),
            "DLAB_FORCE='1' ssh dlab-ui true"
        );
    }

    #[test]
    fn leaves_the_command_alone_when_force_is_unset() {
        assert_eq!(forced("ssh dlab-ui true", None), "ssh dlab-ui true");
        assert_eq!(forced("ssh dlab-ui true", Some("  ")), "ssh dlab-ui true");
    }

    #[test]
    fn reads_a_hop_carrying_a_user() {
        assert_eq!(
            parse_proxy_hop("ssh -T fredrir@archie proxy %h %p"),
            Some("fredrir@archie".to_string())
        );
    }
}
