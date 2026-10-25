//! The `lumps` tool group as seen by a real MCP client: directory listing
//! with filters and paging, glob/content search over the merged view, reading
//! one lump through the shared selector, and the editing tools — `add_lump`,
//! the import tools, `replace_lump`, `remove_lump`, `rename_lump` and
//! `reorder_lumps` — whose every change lands in the working archive as one
//! undoable edit, copying a read-only lump out of its layer when there is
//! nothing else to write.
//!
//! The helpers every topic drives the server through live here: a connected
//! client on the `IWAD` with the `PWAD` fixture merged in, the `IWAD`-only
//! session the copy-on-write tests start from, the fixture's lump sizes, the
//! `lumps` row readers, the `{"path": ...}` and `{"source": ...}` argument
//! builders, the working-archive name and undo-label readers, the assertion
//! that a refused edit left nothing behind, the lumps the selection tests seed
//! the working archive with and the fixture map block's round trip through the
//! map model. The tests themselves are split by topic into the `list`,
//! `search`, `get`, `add`, `import`, `replace`, `remove`, `rename` and
//! `reorder` modules: the `list_lumps` scopes, filters and paging; the
//! `search_lumps` criteria; `get_lump` and `extract_lump`; `add_lump` with its
//! placement and twin rules; the two import tools; `replace_lump` with its
//! overrides; and `remove_lump`, `rename_lump` and the `reorder_lumps` run
//! moves.

use std::path::Path;

use cyberdemon::wad::map::read_map;
use cyberdemon::wad::{Wad, analyze};
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad, two_room_map};
use crate::support::{call, connect};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Bytes in one fixture flat.
const FLAT_BYTES: u64 = 4_096;
/// Bytes in the fixture `PLAYPAL` (14 palettes of 768).
const PLAYPAL_BYTES: u64 = 10_752;
/// Bytes in the fixture `COLORMAP` (34 tables of 256).
const COLORMAP_BYTES: u64 = 8_704;
/// Bytes in a `WAD` header.
const HEADER_BYTES: u64 = 12;
/// Bytes of member data in one fixture two-room map block: two things
/// (20), seven linedefs (98), eight sidedefs (240), six vertices (24)
/// and two sectors (52); the generated node lumps are empty.
const MAP_BYTES: u64 = 434;

/// Load the `IWAD` fixture as the base layer and stack the `PWAD`
/// fixture as resource layer 1.
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

/// The row named `wanted` from a report's `lumps` array.
fn row_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
    report["lumps"]
        .as_array()
        .expect("lumps is an array")
        .iter()
        .find(|row| row["name"] == json!(wanted))
        .unwrap_or_else(|| panic!("no row named {wanted} in {:?}", report["lumps"]))
}

/// The row names of a report, in result order.
fn names(report: &Value) -> Vec<String> {
    report["lumps"]
        .as_array()
        .expect("lumps is an array")
        .iter()
        .map(|row| row["name"].as_str().expect("names are strings").to_owned())
        .collect()
}

/// The space-separated uppercase hex of `data`'s first `count` bytes.
fn hex_head(data: &[u8], count: usize) -> String {
    let head = &data[..count.min(data.len())];
    head.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `{"path": ...}` argument object for `file`.
fn path_arg(file: &Path) -> Value {
    json!({ "path": file.display().to_string() })
}

/// The `{"source": ...}` argument object for the absolute `file`.
fn source_arg(file: &Path) -> Value {
    json!({ "source": file.display().to_string() })
}

/// The working archive's lump names, in directory order.
async fn working_names(client: &Client) -> Vec<String> {
    let report = call(client, "list_lumps", json!({ "scope": "working" })).await;
    names(&report)
}

/// `get_session_info`'s undo-label list.
async fn undo_labels(client: &Client) -> Vec<String> {
    let info = call(client, "get_session_info", Value::Null).await;
    info["undo"]
        .as_array()
        .expect("undo is an array")
        .iter()
        .map(|label| label.as_str().expect("labels are strings").to_owned())
        .collect()
}

/// Append ONE, TWO and THREE, in that order, as global lumps.
async fn seed_three(client: &Client) {
    call(client, "add_lump", json!({ "name": "ONE", "text": "a" })).await;
    call(client, "add_lump", json!({ "name": "TWO", "text": "b" })).await;
    call(client, "add_lump", json!({ "name": "THREE", "text": "c" })).await;
}

/// A session where every refused edit left nothing behind: an empty
/// working directory, no history and a clean flag.
async fn assert_untouched(client: &Client) {
    let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listing["total"],
        json!(0_u64),
        "nothing landed in the working archive"
    );
    assert!(
        undo_labels(client).await.is_empty(),
        "and nothing was recorded"
    );
    let info = call(client, "get_session_info", Value::Null).await;
    assert_eq!(info["working"]["dirty"], json!(false), "still clean");
}

/// A session with the `IWAD` fixture as its only layer: the working
/// archive is empty, so every edit there is a copy-on-write.
async fn iwad_only(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// Appends `text` (or nothing for a marker) as a new working lump.
async fn add(client: &Client, lump: &str, body: &str) {
    call(client, "add_lump", json!({ "name": lump, "text": body })).await;
}

/// Appends two global `TWIN` copies, the second forced past the duplicate
/// rule, so occurrence selection has something to choose between.
async fn add_twins(client: &Client) {
    add(client, "TWIN", "first").await;
    call(
        client,
        "add_lump",
        json!({ "name": "TWIN", "text": "second", "allow_duplicate": true }),
    )
    .await;
}

/// The `{ "name": ..., "hex": ... }` arguments for `replace_lump`.
fn replace_hex(name: &str, digits: &str) -> Value {
    json!({ "name": name, "hex": digits })
}

#[test]
fn the_fixture_map_block_round_trips_through_the_map_model() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let base = iwad(dir.path());
    let wad = Wad::read_file(&base).expect("the fixture is a valid WAD");
    let analysis = analyze(&wad.lumps);
    let block = analysis.maps.first().expect("the IWAD holds MAP01");
    let map = read_map(&wad.lumps, block).expect("the fixture block decodes");
    assert_eq!(map, two_room_map(), "the fixture is the two-room map");
    assert_eq!(
        map.bounds(),
        Some((0_i32, 0_i32, 512_i32, 256_i32)),
        "the hall spans 512 by 256"
    );
}

#[cfg(test)]
#[path = "lumps/add.rs"]
mod add;
#[cfg(test)]
#[path = "lumps/get.rs"]
mod get;
#[cfg(test)]
#[path = "lumps/import.rs"]
mod import;
#[cfg(test)]
#[path = "lumps/list.rs"]
mod list;
#[cfg(test)]
#[path = "lumps/remove.rs"]
mod remove;
#[cfg(test)]
#[path = "lumps/rename.rs"]
mod rename;
#[cfg(test)]
#[path = "lumps/reorder.rs"]
mod reorder;
#[cfg(test)]
#[path = "lumps/replace.rs"]
mod replace;
#[cfg(test)]
#[path = "lumps/search.rs"]
mod search;
