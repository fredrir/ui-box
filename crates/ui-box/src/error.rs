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
