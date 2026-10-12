//! Stdio entry point. stdout carries the MCP protocol; never print to it.

use anyhow::Result;

// Linked for the library, not used by the binary itself.
use crc32fast as _;
use fastrand as _;
use image as _;
use schemars as _;
use serde as _;
use serde_json as _;
use thiserror as _;

// Dev-dependency, only linked into the `cfg(test)` build of this binary.
#[cfg(test)]
use tempfile as _;

use cyberdemon::Cyberdemon;
use rmcp::{ServiceExt as _, transport::stdio};

#[tokio::main]
async fn main() -> Result<()> {
    let service = Cyberdemon::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
