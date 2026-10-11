//! Cyberdemon: an MCP server for reading and editing Doom WADs.

// Used by the binary and integration tests, not by the library itself.
use anyhow as _;
use tokio as _;

pub mod server;

pub use server::Cyberdemon;
