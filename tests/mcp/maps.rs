//! The `maps` tool group as seen by a real MCP client: listing the map
//! blocks of every scope with formats, namespaces and node presence,
//! reading one block's raw lump set (with optionally published bytes),
//! the cheap per-map overview, the typed whole-map graph, the active-map
//! selection the map tools' optional `map` parameter defaults to, and the
//! management tools that build, clone, rename and delete whole blocks of
//! the working archive with their undo steps.
//!
//! The helpers every test drives the server through live here: a connected
//! client on the `IWAD` fixture alone, on the map-format fixture and on the
//! `IWAD` with the `PWAD` fixture merged in, the `list_maps` and
//! `get_map_lumps` row readers, the lump
//! `URL` payload reader and the ten vanilla member lump names. The tests
//! themselves are split by topic into the `list`, `summary`, `assemble`,
//! `create` and `copy_rename` modules: the `list_maps` scopes and the
//! `get_map_lumps` raw set, the `get_map_summary` overview beside the
//! `set_active_map` selection, the `assemble_map` graph, the `create_map`
//! blocks and the `copy_map`/`rename_map`/`remove_map` block moves, each
//! read through these helpers.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad, pwad_map_formats};
use crate::support::{call, connect, fetch_asset, path_arg};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// The ten vanilla member lumps of a built binary `Doom` map block.
const DOOM_MEMBERS: [&str; 10] = [
    "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT",
    "BLOCKMAP",
];

/// Load the `IWAD` fixture as layer 0 and stack the `PWAD` fixture as
/// layer 1: two binary `Doom` maps from two archives.
async fn iwad_plus_pwad(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    let extra = pwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    call(&client, "merge_wad", path_arg(&extra)).await;
    client
}

/// Load the map-format fixture (`UDMF` `MAP03`, `Hexen` `MAP04`) as
/// the base layer.
async fn formats_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let extra = pwad_map_formats(dir);
    call(&client, "open_iwad", path_arg(&extra)).await;
    client
}

/// The map row named `wanted` from a `list_maps` report.
fn map_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
    report["maps"]
        .as_array()
        .expect("maps is an array")
        .iter()
        .find(|row| row["name"] == json!(wanted))
        .unwrap_or_else(|| panic!("no map row named {wanted} in {:?}", report["maps"]))
}

/// The lump row named `wanted` from a `get_map_lumps` report.
fn lump_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
    report["lumps"]
        .as_array()
        .expect("lumps is an array")
        .iter()
        .find(|row| row["name"] == json!(wanted))
        .unwrap_or_else(|| panic!("no lump row named {wanted} in {:?}", report["lumps"]))
}

/// The row names of a `get_map_lumps` report, in result order.
fn lump_names(report: &Value) -> Vec<String> {
    report["lumps"]
        .as_array()
        .expect("lumps is an array")
        .iter()
        .map(|row| row["name"].as_str().expect("names are strings").to_owned())
        .collect()
}

/// Load the plain `IWAD` fixture (one two-room `MAP01`) as layer 0.
async fn iwad_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The asset bytes of the lump row `wanted` of a `get_map_lumps urls` report.
async fn lump_bytes(report: &Value, wanted: &str) -> Vec<u8> {
    let url = lump_named(report, wanted)["asset"]["url"]
        .as_str()
        .expect("a published lump");
    fetch_asset(url).await
}

#[cfg(test)]
#[path = "maps/assemble.rs"]
mod assemble;
#[cfg(test)]
#[path = "maps/copy_rename.rs"]
mod copy_rename;
#[cfg(test)]
#[path = "maps/create.rs"]
mod create;
#[cfg(test)]
#[path = "maps/list.rs"]
mod list;
#[cfg(test)]
#[path = "maps/summary.rs"]
mod summary;
