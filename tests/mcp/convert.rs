//! The `convert` tool group as seen by a real MCP client.
//!
//! `convert_map_format` rewriting a binary `Doom` block as `UDMF` and back,
//! the cross-family `Doom` ↔ `Hexen` translations in both directions,
//! `dry_run` changing neither archive nor history and reporting the numbers
//! the real step writes, one `undo` taking a whole conversion back, and every
//! refusal: already-there, unknown map and malformed parameters.
//!
//! The helpers every test drives the server through live here; the tests
//! themselves are split by topic into the `udmf`, `forward` and `reverse`
//! modules.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::support::{call, call_err, connect};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// The hint every conversion carries.
const NODES_HINT: &str = "run build_nodes";

/// The fixture map's element counts, as `get_map_summary` reports them.
const TWO_ROOM_COUNTS: &str = "{\"things\": 2, \"vertices\": 6, \"linedefs\": 7, \
                              \"sidedefs\": 8, \"sectors\": 2}";

/// Nothing dropped and nothing rounded.
const NO_DROPS: &str = "{\"things\": 0, \"linedefs\": 0, \"sidedefs\": 0, \
                        \"sectors\": 0, \"vertices\": 0}";

/// `MAP05`'s counts, as `get_map_summary` reports them.
const LOSSY_COUNTS: &str = "{\"things\": 1, \"vertices\": 4, \"linedefs\": 4, \
                           \"sidedefs\": 1, \"sectors\": 1}";

/// `MAP05`'s unmodelled fields and preserved fractions per kind.
const LOSSY_DROPS: &str = "{\"things\": 3, \"linedefs\": 1, \"sidedefs\": 2, \
                           \"sectors\": 2, \"vertices\": 3}";

/// `MAP05`'s elements whose `extra` held a rounded number, per kind.
const LOSSY_ROUNDED: &str = "{\"things\": 1, \"linedefs\": 0, \"sidedefs\": 1, \
                             \"sectors\": 1, \"vertices\": 3}";

/// The ten vanilla members of a binary `Doom` block, in directory order.
const DOOM_MEMBERS: [&str; 10] = [
    "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT",
    "BLOCKMAP",
];

/// One of the JSON rows above, parsed.
fn row_of(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture rows are JSON")
}

/// A connected client with `path` loaded as its base layer.
async fn loaded(path: &Path) -> Client {
    let client = connect().await;
    call(
        &client,
        "open_iwad",
        json!({ "path": path.display().to_string() }),
    )
    .await;
    client
}

/// One `convert_map_format` call.
async fn convert(client: &Client, args: Value) -> Value {
    call(client, "convert_map_format", args).await
}

/// One `convert_map_format` refusal on `map`, as `(code, message)`.
async fn refusal(client: &Client, map: &str, mut request: Value) -> (String, String) {
    request["map"] = json!(map);
    let (code, message, _) = call_err(client, "convert_map_format", request).await;
    (code, message)
}

/// The `list_maps` row of `name` in `scope`.
async fn map_row(client: &Client, name: &str, scope: &str) -> Value {
    let listed = call(client, "list_maps", json!({ "scope": scope })).await;
    let rows = listed
        .get("maps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no maps array in {listed}"));
    rows.iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("{name} is not listed in the {scope} scope: {listed}"))
}

/// The block's member lump names, from `get_map_lumps`.
async fn lump_names(client: &Client, name: &str, scope: &str) -> Vec<String> {
    let lumps = call(
        client,
        "get_map_lumps",
        json!({ "map": name, "scope": scope }),
    )
    .await;
    let rows = lumps
        .get("lumps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no lumps array in {lumps}"));
    rows.iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// The five element counts of a `get_map_summary` result.
async fn counts(client: &Client, name: &str, scope: &str) -> Value {
    let summary = call(
        client,
        "get_map_summary",
        json!({ "map": name, "scope": scope }),
    )
    .await;
    summary.get("counts").cloned().unwrap_or(Value::Null)
}

/// The undo labels a `get_session_info` result reports, oldest first.
async fn undo_labels(client: &Client) -> Vec<String> {
    let info = call(client, "get_session_info", json!({})).await;
    info.get("undo")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The working scope's map count, which refusals and dry runs leave empty.
async fn working_maps(client: &Client) -> usize {
    let listed = call(client, "list_maps", json!({ "scope": "working" })).await;
    listed
        .get("maps")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// The `assemble_map` linedef rows of one block.
async fn linedef_rows(client: &Client, name: &str, scope: &str) -> Vec<Value> {
    let graph = call(
        client,
        "assemble_map",
        json!({ "map": name, "scope": scope, "include": ["linedefs"] }),
    )
    .await;
    graph
        .get("linedefs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no linedefs in {graph}"))
        .clone()
}

/// The `assemble_map` linedef row of index `index` in one block.
async fn linedef_row(client: &Client, name: &str, scope: &str, index: i32) -> Value {
    let wanted = json!(index);
    linedef_rows(client, name, scope)
        .await
        .into_iter()
        .find(|line| line["index"] == wanted)
        .unwrap_or_else(|| panic!("{name} has no linedef {index}"))
}

/// The first linedef row of `assemble_map` for one block.
async fn first_linedef(client: &Client, name: &str, scope: &str) -> Value {
    let lines = linedef_rows(client, name, scope).await;
    lines
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("{name} has no linedef: {lines:?}"))
}

/// Whether one conversion result warns with a text containing `wanted`.
fn warns(result: &Value, wanted: &str) -> bool {
    result["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings array in {result}"))
        .iter()
        .filter_map(Value::as_str)
        .any(|note| note.contains(wanted))
}

/// Whether the `flag_names` of one `assemble_map` row carry every
/// name in `wanted`.
fn flag_names_contain(row: &Value, wanted: &[&str]) -> bool {
    let names: Vec<&str> = row["flag_names"]
        .as_array()
        .unwrap_or_else(|| panic!("no flag_names in {row}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    wanted.iter().all(|name| names.contains(name))
}

/// The four cross-family report fields of one conversion result, for
/// comparing what a dry run says with what the real step did.
fn cross_family(result: &Value) -> Value {
    json!({
        "translated_specials": result["translated_specials"],
        "untranslated": result["untranslated"],
        "thing_flags_changed": result["thing_flags_changed"],
        "sector_types_changed": result["sector_types_changed"],
    })
}

/// Asserts one refusal message says the map already is in that layout.
fn assert_no_op(message: &str) {
    assert!(
        message.contains("already is a") && message.contains("nothing to convert"),
        "the message says what it is: {message}"
    );
}

#[cfg(test)]
#[path = "convert/forward.rs"]
mod forward;
#[cfg(test)]
#[path = "convert/reverse.rs"]
mod reverse;
#[cfg(test)]
#[path = "convert/udmf.rs"]
mod udmf;
