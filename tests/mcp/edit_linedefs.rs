//! The `edit_linedefs` tool group as seen by a real MCP client:
//! `add_linedef`, `edit_linedef`, `remove_linedef`, `split_linedef` and
//! `join_linedefs` driving one two-room binary `Doom` `MAP01` and the
//! `UDMF`/binary `Hexen` fixture maps through a live session.
//!
//! The helpers every test drives the server through live here: a connected
//! client on each of the two fixtures, the `assemble_map` linedef rows of the
//! working archive and of the merged view, the vertex count, the
//! `get_map_summary` element counts beside the fixture's own, and the
//! `invalid_argument` and `not_found` refusal wrappers. The tests themselves
//! are split by topic into the `add`, `edit` and `remove_split` modules,
//! covering the appends and their duplicate, degenerate, form, reference and
//! range refusals, the flag, side and reference edits and the nothing-to-change
//! refusal, and removal with and without its pruning with the split and join
//! round trip.

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

/// The working archive's linedef rows for `map`, in order, through
/// `assemble_map`.
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

/// The working `linedefs` row at `index` for `map`.
async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
    working_linedefs(client, map)
        .await
        .into_iter()
        .nth(index)
        .expect("the linedef row exists")
}

/// The merged view's linedef rows for `map`, in order (the layers are
/// what an unedited session shows; failed edits must leave it intact).
async fn merged_linedefs(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "include": ["linedefs"]}),
    )
    .await;
    report["linedefs"]
        .as_array()
        .expect("linedefs is an array")
        .clone()
}

/// The merged `linedefs` row at `index` for `map`.
async fn merged_linedef(client: &Client, map: &str, index: usize) -> Value {
    merged_linedefs(client, map)
        .await
        .into_iter()
        .nth(index)
        .expect("the linedef row exists")
}

/// The vertex count of `map` in `scope` (`"working"` or `"merged"`).
async fn vertex_count(client: &Client, map: &str, scope: &str) -> usize {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": scope, "include": ["vertices"]}),
    )
    .await;
    report["vertices"]
        .as_array()
        .expect("vertices is an array")
        .len()
}

/// Requires `tool` to reject `args` with `invalid_argument`.
async fn refuse(client: &Client, tool: &str, args: Value) {
    let note = args.to_string();
    let (code, _, _) = call_err(client, tool, args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
}

/// Requires `tool` to reject `args` with `not_found`.
async fn miss(client: &Client, tool: &str, args: Value) {
    let note = args.to_string();
    let (code, _, _) = call_err(client, tool, args).await;
    assert_eq!(code, "not_found", "not_found code for {tool} {note}");
}

/// The five element counts of `map` in `scope`, through `get_map_summary`.
async fn counts(client: &Client, map: &str, scope: &str) -> Value {
    let summary = call(
        client,
        "get_map_summary",
        json!({"map": map, "scope": scope}),
    )
    .await;
    summary["counts"].clone()
}

/// The fixture map's element counts.
fn fixture_counts() -> Value {
    json!({
        "things": 2_i32, "vertices": 6_i32, "linedefs": 7_i32,
        "sidedefs": 8_i32, "sectors": 2_i32,
    })
}

#[cfg(test)]
#[path = "edit_linedefs/add.rs"]
mod add;
#[cfg(test)]
#[path = "edit_linedefs/edit.rs"]
mod edit;
#[cfg(test)]
#[path = "edit_linedefs/remove_split.rs"]
mod remove_split;
