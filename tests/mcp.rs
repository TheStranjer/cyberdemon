//! End-to-end tests: a real rmcp client talking to the server.
//!
//! The harness helpers live in `support`, the WAD builders in `fixtures`,
//! the server tests in `server`, the asset-group tests in `assets`, the
//! audio-group tests in `audio`, the batch-group tests in `batch`, the
//! convert-group helpers in `convert` with its `convert/udmf`,
//! `convert/forward` and `convert/reverse` topic tests, the
//! session-group tests in `session`, the
//! `save_wad` tests in `save`, the defs-group shared helpers in `defs` with
//! its `defs/things`, `defs/custom`, `defs/specials` and `defs/sectors` topic
//! tests, the `lumps`-group tests in `lumps`, the map-edit infrastructure
//! checks in `map_edit`, the maps-group tests in `maps`, the capstone low-level
//! element-tool workflow and the spawned-binary asset-server proof in
//! `e2e`, the `images-to-assets`
//! guide-chain checks in `e2e_assets`, the plain-English edit-flow chain
//! checks in `e2e_edit`, the `sketch-to-map` guide-chain
//! checks in `e2e_sketch`, the
//! `edit_geometry`-group tests in `edit_geometry`, the
//! `edit_linedefs`-group shared helpers in `edit_linedefs` with its
//! `edit_linedefs/add`, `edit_linedefs/edit` and
//! `edit_linedefs/remove_split` topic tests, the
//! `edit_sidedefs`-group tests in `edit_sidedefs`, the
//! `edit_sectors`-group shared helpers in `edit_sectors` with its
//! `edit_sectors/edit`, `edit_sectors/remove`,
//! `edit_sectors/create_loop` and `edit_sectors/carve` topic tests, the
//! `edit_things`-group tests in `edit_things`, the
//! `edit_vertices`-group tests in `edit_vertices`, the
//! graphics-group tests in `graphics`,
//! the imports-group tests in `imports`, the
//! inspect-group tests in `inspect`, the features-group tests in
//! `features`, the
//! music-group tests in `music`, the
//! nodes-group tests in `nodes`, the
//! palette-group tests in `palette`, the
//! patches-group shared helpers in `patches` with its `patches/add`,
//! `patches/replace` and `patches/remove` topic tests, the
//! render-group tests in `render`, the scripts-group tests in `scripts`,
//! the plan-group tests in `plan`, the playability-group tests in
//! `playability`, the query-group shared helpers in `query` with its
//! `query/find`, `query/replace_surfaces`, `query/replace_things` and
//! `query/bulk` topic tests, the sketch-group tests in
//! `sketch`, the textures-group shared helpers in
//! `textures` with its `textures/list`, `textures/pnames` and
//! `textures/build` topic tests, the themes-group tests in
//! `themes`, the validate-group tests in `validate`, the usability-pass
//! checks in `usability`, and the
//! documentation-layout and guide-content
//! checks in `docs` (cross-references, `Tools:` lines, line budgets, the
//! `SKILL.md` tool-index, guide-link and tool/schema description
//! consistency checks,
//! the `json plan-example` builds and the `json recipe` `run_batch`
//! checks); every helper is exercised by a test in
//! `server`, and
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
#[path = "mcp/batch.rs"]
mod batch;
#[cfg(test)]
#[path = "mcp/convert.rs"]
mod convert;
#[cfg(test)]
#[path = "mcp/defs.rs"]
mod defs;
#[cfg(test)]
#[path = "mcp/docs.rs"]
mod docs;
#[cfg(test)]
#[path = "mcp/e2e.rs"]
mod e2e;
#[cfg(test)]
#[path = "mcp/e2e_assets.rs"]
mod e2e_assets;
#[cfg(test)]
#[path = "mcp/e2e_edit.rs"]
mod e2e_edit;
#[cfg(test)]
#[path = "mcp/e2e_sketch.rs"]
mod e2e_sketch;
#[cfg(test)]
#[path = "mcp/edit_geometry.rs"]
mod edit_geometry;
#[cfg(test)]
#[path = "mcp/edit_linedefs.rs"]
mod edit_linedefs;
#[cfg(test)]
#[path = "mcp/edit_sectors.rs"]
mod edit_sectors;
#[cfg(test)]
#[path = "mcp/edit_sidedefs.rs"]
mod edit_sidedefs;
#[cfg(test)]
#[path = "mcp/edit_things.rs"]
mod edit_things;
#[cfg(test)]
#[path = "mcp/edit_vertices.rs"]
mod edit_vertices;
#[cfg(test)]
#[path = "mcp/features.rs"]
mod features;
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
#[path = "mcp/patches.rs"]
mod patches;
#[cfg(test)]
#[path = "mcp/plan.rs"]
mod plan;
#[cfg(test)]
#[path = "mcp/playability.rs"]
mod playability;
#[cfg(test)]
#[path = "mcp/query.rs"]
mod query;
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
#[path = "mcp/sketch.rs"]
mod sketch;
#[cfg(test)]
#[path = "mcp/support.rs"]
pub mod support;
#[cfg(test)]
#[path = "mcp/textures.rs"]
mod textures;
#[cfg(test)]
#[path = "mcp/themes.rs"]
mod themes;
#[cfg(test)]
#[path = "mcp/usability.rs"]
mod usability;
#[cfg(test)]
#[path = "mcp/validate.rs"]
mod validate;
