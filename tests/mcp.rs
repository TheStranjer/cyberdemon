//! End-to-end tests: a real rmcp client talking to the server.
//!
//! The harness helpers live in `support`, the server tests in `server`, and
//! the documentation-layout checks in `docs`; every helper is exercised by a
//! test in `server`, and the real result/error contract is checked end to end
//! through a small probe server.

// Linked for other targets, unused by this test crate.
use anyhow as _;
use crc32fast as _;
use fastrand as _;
use image as _;
use tempfile as _;
use thiserror as _;

// The module files live under `tests/mcp/`, next to this crate root.
// `support` must be a public module of pub items: private+`pub` trips
// `unreachable_pub`, while `pub(crate)` in any test-crate module trips
// `redundant_pub_crate`; the plain `pub` combination is the lint-clean one.
#[cfg(test)]
#[path = "mcp/docs.rs"]
mod docs;
#[cfg(test)]
#[path = "mcp/server.rs"]
mod server;
#[cfg(test)]
#[path = "mcp/support.rs"]
pub mod support;
