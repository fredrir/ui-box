use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendSpec {
    Local,
    Ssh { user: Option<String>, host: String },
}

impl BackendSpec {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            bail!("empty backend, expected local:// or ssh://[user@]host");
        }
        let (scheme, rest) = match raw.split_once("://") {
            Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
            None => ("ssh".to_string(), raw),
        };
        match scheme.as_str() {
            "local" => Ok(BackendSpec::Local),
            "ssh" => {
                let rest = rest.trim_end_matches('/');
                if rest.is_empty() {
                    bail!("backend {raw:?} has no host, expected ssh://[user@]host");
                }
                let (user, host) = match rest.split_once('@') {
                    Some((user, host)) => (Some(user.to_string()), host.to_string()),
                    None => (None, rest.to_string()),
                };
                if host.is_empty() {
                    bail!("backend {raw:?} has no host, expected ssh://[user@]host");
                }
                Ok(BackendSpec::Ssh { user, host })
            }
            other => bail!("unsupported backend scheme {other:?}, expected local:// or ssh://"),
        }
    }

    pub fn url(&self) -> String {
        match self {
            BackendSpec::Local => "local://".to_string(),
            BackendSpec::Ssh {
                user: Some(user),
                host,
            } => format!("ssh://{user}@{host}"),
            BackendSpec::Ssh { user: None, host } => format!("ssh://{host}"),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            BackendSpec::Local => None,
            BackendSpec::Ssh { host, .. } => Some(host),
        }
    }

    pub fn ssh_target(&self) -> Option<String> {
        match self {
            BackendSpec::Local => None,
            BackendSpec::Ssh {
                user: Some(user),
                host,
            } => Some(format!("{user}@{host}")),
            BackendSpec::Ssh { user: None, host } => Some(host.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backend_urls() {
        assert_eq!(BackendSpec::parse("local://").unwrap(), BackendSpec::Local);
        assert_eq!(
            BackendSpec::parse("ssh://fredrir@ui-box-backend").unwrap(),
            BackendSpec::Ssh {
                user: Some("fredrir".into()),
                host: "ui-box-backend".into()
            }
        );
        assert_eq!(
            BackendSpec::parse("ui-box-backend").unwrap(),
            BackendSpec::Ssh {
                user: None,
                host: "ui-box-backend".into()
            }
        );
        assert!(BackendSpec::parse("ftp://host").is_err());
        assert!(BackendSpec::parse("ssh://").is_err());
    }
}
