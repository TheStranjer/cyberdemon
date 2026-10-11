//! Stdio entry point. stdout carries the MCP protocol; never print to it.

use anyhow::Result;
use cyberdemon::Cyberdemon;
use rmcp::{ServiceExt as _, transport::stdio};

#[tokio::main]
async fn main() -> Result<()> {
    let service = Cyberdemon.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
