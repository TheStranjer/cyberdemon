//! Cyberdemon: an MCP server for reading and editing Doom WADs.

// `alloc` paths satisfy `std_instead_of_alloc` for shared pointers.
extern crate alloc;

// Linked for the binary and the integration tests, not used by the library.
// (`schemars`, `serde` and `serde_json` are used directly since `gfx::ops`.)
use anyhow as _;

// Dev-dependency, only linked into the `cfg(test)` builds of this crate.
#[cfg(test)]
use tempfile as _;

pub mod assets;
pub mod audio;
pub mod defs;
pub mod error;
pub mod fixed;
pub mod gfx;
pub mod music;
pub mod server;
pub mod session;
pub mod state;
pub mod tools;
pub mod wad;

pub use server::Cyberdemon;
