//! The `edit_sectors` tool group as seen by a real MCP client:
//! `add_sector` with its defaults and custom fields, the bad-flat,
//! over-bright-light and height-order refusals, `edit_sector` over single
//! indices and lists with deltas (a property edit that keeps the nodes),
//! the absolute/delta conflict and the empty request, `remove_sector` with
//! the `conflict` on referenced sectors and the cascaded room removal
//! verified through `get_map_summary` and `assemble_map`,
//! `create_sector_from_loop` growing a room next to room 0, attached
//! mid-wall (T-junction split) and as a platform inside it, with
//! `keep_interior` and the `none` outer mode, `carve_void` punching a
//! pillar and separating the rooms, and the undo of a whole call.
//!
//! The helpers every test drives the server through live here: a connected
//! client on each of the two fixtures, the `assemble_map` sector and linedef
//! rows of the working archive and of the merged view, the `{x, y}` loop
//! point and the square-loop request builder, the `end` and `same_ends`
//! linedef-span test the welds and trenches look their walls up with, the
//! `invalid_argument` refusal wrappers and the `validate_map` pass that lets
//! only the fixture noise through. The tests themselves are split by topic
//! into the `edit`, `remove`, `create_loop` and `carve` modules: the adds and
//! property edits with their refusals and format caps, the removals with
//! their cascade and one-step undo, the loop drawings with their report and
//! wall checks, and the void carvings.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad_map_formats};
use crate::support::{call, call_err, connect};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Open the `DOOM II` fixture (one two-room binary `Doom` `MAP01`) as
/// the base layer.
async fn doom_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// Open the map-format fixture (`UDMF` `MAP03`, binary `Hexen` `MAP04`)
/// as the base layer.
async fn formats_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let extra = pwad_map_formats(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": extra.display().to_string()}),
    )
    .await;
    client
}

/// The working archive's sector rows for `map`, in order, through
/// `assemble_map`.
async fn working_sectors(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["sectors"]}),
    )
    .await;
    report["sectors"]
        .as_array()
        .expect("sectors is an array")
        .clone()
}

/// The working `linedefs` row at `index` for `map`.
async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["linedefs"]}),
    )
    .await;
    report["linedefs"]
        .as_array()
        .expect("linedefs is an array")
        .get(index)
        .cloned()
        .expect("the linedef row exists")
}

/// The working linedefs for `map`, in order.
async fn working_linedefs(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["linedefs"]}),
    )
    .await;
    report["linedefs"]
        .as_array()
        .expect("linedefs is an array")
        .clone()
}

/// The merged view's sector rows for `map`, in order (the layers are
/// what an unedited session shows; failed edits must leave it intact).
async fn merged_sectors(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "merged", "include": ["sectors"]}),
    )
    .await;
    report["sectors"]
        .as_array()
        .expect("sectors is an array")
        .clone()
}

/// Requires `tool` to reject `args` with `invalid_argument`.
async fn refuse(client: &Client, tool: &str, args: Value) {
    let note = args.to_string();
    let (code, _, _) = call_err(client, tool, args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
}

/// Requires `tool` to refuse every one of `requests`.
async fn refuse_all(client: &Client, tool: &str, requests: Vec<Value>) {
    for args in requests {
        refuse(client, tool, args).await;
    }
}

/// The working `sectors` row at `index` for `map`, through
/// `assemble_map`.
async fn working_sector_row(client: &Client, map: &str, index: usize) -> Value {
    working_sectors(client, map)
        .await
        .into_iter()
        .nth(index)
        .unwrap_or_else(|| panic!("the sector row {index} exists"))
}

/// A `{x, y}` loop-corner parameter.
fn pt(x: i32, y: i32) -> Value {
    json!({"x": x, "y": y})
}

/// A loop request over the square `(64, 64)` to `(192, 192)` with
/// `tail` spliced in as further JSON fields.
fn square(tail: &str) -> Value {
    format!(
        r#"{{"points": [{{"x": 64, "y": 64}}, {{"x": 192, "y": 64}},
            {{"x": 192, "y": 192}}, {{"x": 64, "y": 192}}]{tail}}}"#
    )
    .parse()
    .expect("test requests are valid JSON")
}

/// A grid point parameter for [`same_ends`].
fn end(x: i32, y: i32) -> (i32, i32) {
    (x, y)
}

/// Whether `line` spans exactly the two grid points `ends`.
fn same_ends(line: &Value, ends: [(i32, i32); 2]) -> bool {
    let at = |key: &str| -> Option<(i32, i32)> {
        let point = line.get(key)?;
        Some((
            i32::try_from(point["x"].as_i64()?).ok()?,
            i32::try_from(point["y"].as_i64()?).ok()?,
        ))
    };
    match (at("v1"), at("v2")) {
        (Some(first), Some(second)) => {
            (first == ends[0] && second == ends[1]) || (first == ends[1] && second == ends[0])
        }
        _ => false,
    }
}

/// Requires `validate_map` to report no unexpected error- or
/// warning-severity findings for the working `map`. The geometry and
/// health findings the drawing itself is allowed to leave behind are
/// filtered out: the minimal fixture `IWAD` misses some of the names the
/// theme dressing and the fixture maps pick, and every draw invalidates
/// the node lumps.
async fn validate_clean(client: &Client, map: &str) {
    const EXPECTED: [&str; 3] = ["missing_texture", "missing_flat", "nodes_missing"];
    let report = call(
        client,
        "validate_map",
        json!({"map": map, "scope": "working"}),
    )
    .await;
    let unexpected: Vec<String> = report["issues"]
        .as_array()
        .expect("an issue list")
        .iter()
        .filter(|issue| {
            let severity = issue["severity"].as_str().expect("a severity");
            let code = issue["code"].as_str().expect("a code");
            (severity == "error" || severity == "warning") && !EXPECTED.contains(&code)
        })
        .map(|issue| issue["message"].as_str().expect("a message").to_owned())
        .collect();
    assert!(
        unexpected.is_empty(),
        "no findings beyond the expected fixture noise: {unexpected:#?}"
    );
}

#[cfg(test)]
#[path = "edit_sectors/carve.rs"]
mod carve;
#[cfg(test)]
#[path = "edit_sectors/create_loop.rs"]
mod create_loop;
#[cfg(test)]
#[path = "edit_sectors/edit.rs"]
mod edit;
#[cfg(test)]
#[path = "edit_sectors/remove.rs"]
mod remove;
#[cfg(test)]
#[path = "edit_sectors/theme.rs"]
mod theme;
