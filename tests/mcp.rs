//! End-to-end tests: a real rmcp client talking to the server.
//!
//! The harness helpers live in `support`, the WAD builders in `fixtures`,
//! the server tests in `server`, the asset-group tests in `assets`, the
//! audio-group tests in `audio`, the session-group tests in `session`, the
//! `save_wad` tests in `save`, the defs-group tests in `defs`, the
//! `lumps`-group tests in `lumps`, the map-edit infrastructure checks in
//! `map_edit`, the maps-group tests in `maps`, the
//! `edit_things`-group tests in `edit_things`, the
//! graphics-group tests in `graphics`,
//! the imports-group tests in `imports`, the
//! inspect-group tests in `inspect`, the
//! music-group tests in `music`, the
//! nodes-group tests in `nodes`, the
//! palette-group tests in `palette`, the
//! render-group tests in `render`, the scripts-group tests in `scripts`,
//! the textures-group tests in `textures`, and the documentation-layout
//! checks in `docs`; every helper is exercised by a test in `server`, and
//! the real result/error contract is checked end to end through a small
//! probe server.

// Linked for other targets, unused by this test crate.
use anyhow as _;
use axum as _;
use crc32fast as _;
use fastrand as _;
use flate2 as _;
use image as _;
use reqwest as _;
use symphonia as _;
use tempfile as _;
use thiserror as _;

// The module files live under `tests/mcp/`, next to this crate root.
// `support` must be a public module of pub items: private+`pub` trips
// `unreachable_pub`, while `pub(crate)` in any test-crate module trips
// `redundant_pub_crate`; the plain `pub` combination is the lint-clean one.
#[cfg(test)]
#[path = "mcp/assets.rs"]
mod assets;
#[cfg(test)]
#[path = "mcp/audio.rs"]
mod audio;
#[cfg(test)]
#[path = "mcp/defs.rs"]
mod defs;
#[cfg(test)]
#[path = "mcp/docs.rs"]
mod docs;
#[cfg(test)]
#[path = "mcp/edit_things.rs"]
mod edit_things;
#[cfg(test)]
#[path = "mcp/fixtures.rs"]
pub mod fixtures;
#[cfg(test)]
#[path = "mcp/graphics.rs"]
mod graphics;
#[cfg(test)]
#[path = "mcp/imports.rs"]
mod imports;
#[cfg(test)]
#[path = "mcp/inspect.rs"]
mod inspect;
#[cfg(test)]
#[path = "mcp/lumps.rs"]
mod lumps;
#[cfg(test)]
#[path = "mcp/map_edit.rs"]
mod map_edit;
#[cfg(test)]
#[path = "mcp/maps.rs"]
mod maps;
#[cfg(test)]
#[path = "mcp/music.rs"]
mod music;
#[cfg(test)]
#[path = "mcp/nodes.rs"]
mod nodes;
#[cfg(test)]
#[path = "mcp/palette.rs"]
mod palette;
#[cfg(test)]
#[path = "mcp/render.rs"]
mod render;
#[cfg(test)]
#[path = "mcp/save.rs"]
mod save;
#[cfg(test)]
#[path = "mcp/scripts.rs"]
mod scripts;
#[cfg(test)]
#[path = "mcp/server.rs"]
mod server;
#[cfg(test)]
#[path = "mcp/session.rs"]
mod session;
#[cfg(test)]
#[path = "mcp/support.rs"]
pub mod support;
#[cfg(test)]
#[path = "mcp/textures.rs"]
mod textures;
