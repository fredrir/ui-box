pub mod backend;
pub mod error;
pub mod local;
pub mod spec;
pub mod ssh;

pub use backend::{shell_quote, Backend, Cmd, Output};
pub use error::BackendFailure;
pub use local::LocalBackend;
pub use spec::BackendSpec;
pub use ssh::{parse_proxy_hop, proxy_hop, wake, which, SshBackend};
