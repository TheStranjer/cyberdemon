//! The `query` tool group as seen by a real MCP client: `find_elements`, the
//! CD-108 bulk swaps and the `bulk_edit` tool.
//!
//! The helpers every test drives the server through live here: a connected
//! client on the two-room `IWAD` fixture and on that with the `PWAD` fixture
//! merged in, the `find_elements` and `bulk_edit` wrappers, the `indices`
//! reader of a report, the one `{field, op, value}` filter builder and the
//! `find_elements` refusal wrapper. The tests themselves are split by topic
//! into the `find`, `replace_surfaces`, `replace_things` and `bulk` modules:
//! `find_elements` over the two-room fixture with its ops and derived fields,
//! its `within` bounds beside an `add_thing`, its compact rows, its paging and
//! its `invalid_argument`/`not_found` refusals; `replace_texture` and
//! `replace_flat` across the active map, every map or a listed set; the
//! doomednum-or-name thing swap `replace_thing_type` with its name resolution,
//! target check and `allow_unknown` escape, all with their copy-on-write
//! reports, previews and one-step undo; and the `bulk_edit` tool with its
//! query-selected set/add/flag edits on sectors, linedefs, things and
//! vertices, the never-clamped range refusal, the `max` guard, the geometry
//! rule for vertex moves, thing-type name resolution and the one-step undo.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad};
use crate::support::{call, call_err, connect};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Load the two-room `IWAD` fixture as layer 0.
async fn plain_iwad(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    client
}

/// Run `find_elements` with these query parts over the fixture map.
async fn find(client: &Client, args: Value) -> Value {
    let mut full = args;
    let Some(map) = full.as_object_mut() else {
        panic!("query arguments are an object");
    };
    map.entry("map").or_insert_with(|| json!("MAP01"));
    call(client, "find_elements", full).await
}

/// The `indices` list of a report, as plain integers.
fn indices(report: &Value) -> Vec<u64> {
    report["indices"]
        .as_array()
        .expect("an index list")
        .iter()
        .map(|index| index.as_u64().expect("unsigned indices"))
        .collect()
}

/// Assert a query fails with `want_code` and a message carrying `want_text`.
async fn refuse(client: &Client, args: Value, want_code: &str, want_text: &str) {
    let described = args.to_string();
    let (code, message, _) = call_err(client, "find_elements", args).await;
    assert_eq!(code, want_code, "error code for {described}: {message}");
    assert!(
        message.contains(want_text),
        "'{want_text}' missing from: {message}"
    );
}

/// One `{field, op, value}` filter as wire JSON.
fn one(field: &str, op: &str, value: &Value) -> Value {
    json!([{ "field": field, "op": op, "value": value }])
}

/// Load the `IWAD` fixture and merge the `PWAD` fixture (a layer-only
/// `MAP02` block, an overridden `FLOOR1` and a `DEHACKED` global) in.
async fn iwad_plus_pwad(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    let extra = pwad(dir);
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    call(
        &client,
        "merge_wad",
        json!({ "path": extra.display().to_string() }),
    )
    .await;
    client
}

/// Run `bulk_edit` with these arguments over the fixture map.
async fn edit(client: &Client, args: Value) -> Value {
    let mut full = args;
    let Some(map) = full.as_object_mut() else {
        panic!("bulk arguments are an object");
    };
    map.entry("map").or_insert_with(|| json!("MAP01"));
    call(client, "bulk_edit", full).await
}

#[cfg(test)]
#[path = "query/bulk.rs"]
mod bulk;
#[cfg(test)]
#[path = "query/find.rs"]
mod find;
#[cfg(test)]
#[path = "query/replace_surfaces.rs"]
mod replace_surfaces;
#[cfg(test)]
#[path = "query/replace_things.rs"]
mod replace_things;
