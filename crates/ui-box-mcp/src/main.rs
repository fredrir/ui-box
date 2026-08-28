mod diagnostics;
mod outcome;
mod schema;
mod server;
mod uibox;

use anyhow::{Context, Result};
use rmcp::transport::stdio;
use rmcp::ServiceExt;

use server::UiBoxServer;

#[tokio::main]
async fn main() -> Result<()> {
    let server = UiBoxServer::new();
    if let Err(reason) = server.uibox_location() {
        eprintln!("ui-box-mcp: {reason}");
    }

    let running = server
        .serve(stdio())
        .await
        .context("cannot serve the Model Context Protocol over stdio")?;
    running
        .waiting()
        .await
        .context("the ui-box MCP service stopped unexpectedly")?;
    Ok(())
}
