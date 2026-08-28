pub mod backend;
pub mod cli;
pub mod commands;
pub mod config;
pub mod driver;
pub mod error;
pub mod flow;
pub mod output;
pub mod pipeline;
pub mod playwright;
pub mod run;
pub mod session;
pub mod vision;

pub use backend::{Backend, Cmd, LocalBackend, Output, SshBackend};
pub use config::{BackendSpec, Config, Surface, Viewport};
pub use flow::{Flow, SnapMode, Step};

use cli::Cli;

pub fn dispatch(cli: Cli) -> i32 {
    output::set_quiet(cli.global.quiet);
    match commands::execute(&cli) {
        Ok(summary) => {
            summary.emit();
            summary.exit_code()
        }
        Err(err) => {
            report(&err);
            output::EXIT_TOOL
        }
    }
}

fn report(err: &anyhow::Error) {
    let kind = error::kind_of(err);
    match error::backend_failure(err) {
        Some(failure) => {
            eprintln!("{}", failure.verbatim());
            println!(
                "{}",
                output::error_summary(kind, failure.verbatim(), Some(&failure.context))
            );
        }
        None => {
            let message = format!("{err:#}");
            eprintln!("ui-box: {message}");
            println!("{}", output::error_summary(kind, &message, None));
        }
    }
}
