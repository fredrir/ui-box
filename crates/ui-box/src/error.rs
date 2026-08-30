use thiserror::Error;

pub use ui_box_core::BackendFailure;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(
        "session {id} expired: idle {idle_secs}s exceeds UIBOX_SESSION_TTL {ttl_secs}s. \
         The UI was never asked. Reopen with `ui-box open` and replay; \
         the steps recorded so far are still in {run_dir}"
    )]
    Expired {
        id: String,
        idle_secs: u64,
        ttl_secs: u64,
        run_dir: String,
    },

    #[error(
        "session {id} has no live driver: process {pid} is gone. \
         The UI was never asked. Reopen with `ui-box open`; \
         the steps recorded so far are still in {run_dir}"
    )]
    DriverGone {
        id: String,
        pid: u32,
        run_dir: String,
    },

    #[error("unknown session {id}: no record in {store}")]
    Unknown { id: String, store: String },
}

impl SessionError {
    pub fn kind(&self) -> &'static str {
        match self {
            SessionError::Expired { .. } => "session_expired",
            SessionError::DriverGone { .. } => "driver_gone",
            SessionError::Unknown { .. } => "unknown_session",
        }
    }
}

#[derive(Debug, Error)]
pub enum FlowError {
    #[error(
        "flow {path} asserts nothing: {steps} steps of navigation and snapshots, and no \
         assert_text or assert_absent among them. It would pass against a blank page, which \
         makes it a transcript rather than a test. Add an assertion, or record a run that \
         snapshots the state you care about and let `ui-box record` derive one"
    )]
    AssertsNothing { path: String, steps: usize },

    #[error(
        "wrote {path}, but it asserts nothing: {snaps} snapshot(s) and no text stable enough \
         to assert on. A flow of clicks and snaps passes against a blank page. Snap a state \
         with a heading or a label in it and record again, or add assert_text by hand"
    )]
    RecordedNothing { path: String, snaps: usize },
}

impl FlowError {
    pub fn kind(&self) -> &'static str {
        match self {
            FlowError::AssertsNothing { .. } | FlowError::RecordedNothing { .. } => {
                "flow_asserts_nothing"
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ForwardError {
    #[error(
        "target {target} points at port {port} on {lab}, because {host} inside a target is \
         {lab}'s loopback and not this machine's. No forward publishes that port, so nothing \
         is listening there. The UI was never asked. Publish it and retry:\n    \
         {command} --forward {port}"
    )]
    Missing {
        target: String,
        host: String,
        port: u16,
        lab: String,
        command: String,
    },

    #[error(
        "--forward {spec} has nothing to publish: nothing is listening on {endpoint} here. \
         The UI was never asked. Start the local server first, or point the forward at the \
         port it really binds"
    )]
    LocalClosed { spec: String, endpoint: String },

    #[error(
        "--forward {spec} dials 127.0.0.1:{local}, where nothing is listening. Something \
         answers on [::1]:{local}, but that is not the address ssh will look at. The local \
         server bound IPv6 only. The UI was never asked. Bind it to 127.0.0.1, or name it \
         with --forward {suggestion}"
    )]
    LocalIpv6Only {
        spec: String,
        local: u16,
        suggestion: String,
    },

    #[error(
        "session {session} already holds port {port} on {lab}, so this run could not bind it. \
         The UI was never asked. Close that session and retry:\n    \
         ui-box close {session}{}",
        detail(.log)
    )]
    HeldBySession {
        session: String,
        port: u16,
        lab: String,
        log: Option<String>,
    },

    #[error(
        "{lab} refused the forward for port {ports} and the driver's ssh exited without \
         opening a session. The UI was never asked. No ui-box session on this machine holds \
         it, which leaves an ssh master still holding the port{}, a process outside ui-box \
         listening on it, or sshd on {lab} refusing forwarding.{}",
        persistence(.control_persist),
        detail(.log)
    )]
    Refused {
        ports: String,
        lab: String,
        control_persist: Option<String>,
        log: Option<String>,
    },

    #[error(
        "UIBOX_DRIVER_DOM={command} carries its own ssh, so ui-box cannot put --forward \
         {specs} on it. The UI was never asked, and dropping the forward silently would \
         have looked like a broken page. Add {flags} to that command yourself, or unset \
         UIBOX_DRIVER_DOM and let ui-box build the connection"
    )]
    OwnTransport {
        command: String,
        specs: String,
        flags: String,
    },
}

impl ForwardError {
    pub fn kind(&self) -> &'static str {
        match self {
            ForwardError::Missing { .. } => "forward_missing",
            ForwardError::LocalClosed { .. } => "forward_unreachable",
            ForwardError::LocalIpv6Only { .. } => "forward_ipv6_only",
            ForwardError::HeldBySession { .. } => "forward_held",
            ForwardError::Refused { .. } => "forward_refused",
            ForwardError::OwnTransport { .. } => "forward_unsupported",
        }
    }
}

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("surface {surface} is not yet supported: no driver implements it")]
    UnsupportedSurface { surface: String },

    #[error("driver {name} reported an error: {message}{}", detail(.data))]
    Rpc {
        name: String,
        code: i64,
        message: String,
        data: Option<String>,
    },

    #[error("driver {name} did not answer {method} within {timeout_secs}s")]
    Timeout {
        name: String,
        method: String,
        timeout_secs: u64,
    },

    #[error("driver {name} exited before answering {method}{}", detail(.log))]
    Exited {
        name: String,
        method: String,
        log: Option<String>,
    },

    #[error("driver for surface {surface} is not installed: {path} is missing. {hint}")]
    Missing {
        surface: String,
        path: String,
        hint: String,
    },
}

impl DriverError {
    pub fn kind(&self) -> &'static str {
        match self {
            DriverError::UnsupportedSurface { .. } => "unsupported_surface",
            DriverError::Rpc { .. } => "driver_error",
            DriverError::Timeout { .. } => "driver_timeout",
            DriverError::Exited { .. } => "driver_exited",
            DriverError::Missing { .. } => "driver_missing",
        }
    }
}

fn persistence(value: &Option<String>) -> String {
    match value {
        Some(seconds) => format!(" for up to {seconds} after the session that made it closed"),
        None => " briefly after the session that made it closed".to_string(),
    }
}

fn detail(value: &Option<String>) -> String {
    match value {
        Some(value) if !value.trim().is_empty() => format!("\n{}", value.trim_end()),
        _ => String::new(),
    }
}

pub fn kind_of(error: &anyhow::Error) -> &'static str {
    for cause in error.chain() {
        if cause.downcast_ref::<BackendFailure>().is_some() {
            return "backend";
        }
        if let Some(session) = cause.downcast_ref::<SessionError>() {
            return session.kind();
        }
        if let Some(flow) = cause.downcast_ref::<FlowError>() {
            return flow.kind();
        }
        if let Some(forward) = cause.downcast_ref::<ForwardError>() {
            return forward.kind();
        }
        if let Some(driver) = cause.downcast_ref::<DriverError>() {
            return driver.kind();
        }
    }
    "error"
}

pub fn backend_failure(error: &anyhow::Error) -> Option<&BackendFailure> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<BackendFailure>())
}
