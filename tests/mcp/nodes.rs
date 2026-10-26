//! The `nodes` tool group as seen by a real MCP client: the node lumps of a
//! map read back through `get_nodes`, `get_reject` and `get_blockmap` and
//! written by `build_blockmap`, `build_reject`, `build_nodes` and
//! `build_all_nodes`.
//!
//! The helpers every test drives the server through live here: a connected
//! client on the `IWAD` fixture (empty node lumps on `MAP01`), on the nodes
//! fixture (`MAP05` valid, `MAP06` malformed, `MAP07` `XNOD` in `ZNODES`) and
//! on the `IWAD` whose one concave room's build appends one split vertex, the
//! `JSON` asset reader and the name
//! column of one scope's directory. The tests themselves are split by topic
//! into the `read`, `build_tables`, `build_nodes`, `build_gl` and `build_all`
//! modules: the three inspection tools over the `SEGS`/`SSECTORS`/`NODES`
//! tree in its vanilla and extended layouts, the visibility matrix and the
//! spatial index, with their empty-lump tolerance, their `format_error`
//! refusals and the `JSON` assets the `full` detail publishes; the
//! `BLOCKMAP` and `REJECT` builders whose written lumps the inspection tools
//! then read back; `build_nodes` writing the whole tree either as the vanilla
//! run (its split vertices appended to `VERTEXES` and stripped again by the
//! next geometry edit) or as one `ZDoom` extended lump in `NODES`/`ZNODES`
//! with the splits kept inside the lump; the same tool's `glBSP` v2 `GL_*`
//! block (classic run plus miniseg-closed subsectors) attached right after
//! the map block; and `build_all_nodes`, the one-shot
//! tree-plus-reject-plus-blockmap build that writes everything binary maps
//! need (or `ZNODES` alone for `UDMF` ones) in a single undoable step.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, iwad_l_room, pwad_nodes};
use crate::support::{call, connect, fetch_asset, path_arg};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Load the `IWAD` fixture (empty node lumps on `MAP01`) as layer 0.
async fn plain_iwad(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// Load the nodes fixture (`MAP05` valid, `MAP06` malformed, `MAP07`
/// `XNOD` in `ZNODES`) as layer 0.
async fn nodes_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let extra = pwad_nodes(dir);
    call(&client, "open_iwad", path_arg(&extra)).await;
    client
}

/// Load the L-room fixture (a concave one-sector room whose node
/// build appends one split vertex) as layer 0.
async fn l_room_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_l_room(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The `asset.url` of a full-detail report, as a parsed value plus
/// the fetched `JSON` body.
async fn fetch_json(report: &Value) -> Value {
    let url = report["asset"]["url"].as_str().expect("a published url");
    let body = fetch_asset(url).await;
    serde_json::from_slice(&body).expect("the asset is json")
}

/// The name column of one scope's directory.
async fn directory_names(client: &Client, scope: &str) -> Vec<String> {
    let listing = call(
        client,
        "list_lumps",
        json!({ "scope": scope, "limit": 2000_i64 }),
    )
    .await;
    listing["lumps"]
        .as_array()
        .expect("a lump list")
        .iter()
        .map(|row| row["name"].as_str().expect("a name").to_owned())
        .collect()
}

#[cfg(test)]
#[path = "nodes/build_all.rs"]
mod build_all;
#[cfg(test)]
#[path = "nodes/build_gl.rs"]
mod build_gl;
#[cfg(test)]
#[path = "nodes/build_nodes.rs"]
mod build_nodes;
#[cfg(test)]
#[path = "nodes/build_tables.rs"]
mod build_tables;
#[cfg(test)]
#[path = "nodes/read.rs"]
mod read;
