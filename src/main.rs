//! deadlock-mcp: an MCP server over stdio for Deadlock statistics and game knowledge.

mod domain;
mod http;
mod problem;
mod server;
mod sources;
mod tools;

use rmcp::ServiceExt as _;
use rmcp::transport::stdio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    server::DeadlockServer
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
