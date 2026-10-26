//! The `plan` tool group as seen by a real MCP client: `build_map_from_plan`
//! compiling a two-room plan with a `door` and an `opening` into one written
//! block whose sector and geometry counts match the reported bookkeeping, the
//! `units` scale-and-mirror and `grid` snap carrying an image-space plan into
//! map units before compiling, the `things` and `player_start` parameters
//! standing rows into the map by name, the `finalize` parameter building the
//! whole node run into that block inside the one undo step, the `features`
//! rows wiring lifts, teleporters, lights, secrets, switches and exits, the
//! wall a panel mounts on, and the one six-room plan that builds a whole
//! playable map.
//!
//! The helpers every topic drives the server through, the shared plan
//! builders and the assembled-graph row readers live here; the tests
//! themselves are split by topic into the `basics`, `units`, `things`,
//! `finalize`, `features`, `panels` and `six_room` modules.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::iwad_features;
use crate::support::{call, call_err, connect, path_arg};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Open the features `IWAD` (textured two-room `MAP01`, the door
/// wardrobe and `FLAT10`) as the base layer.
async fn features_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_features(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The two-room plan: `a` west of `b`, sharing the line `x = 128`,
/// joined by a door low on the wall and an opening high on it, with
/// the player start the tool now demands standing in `a`.
fn plan_for(map: &str) -> Value {
    json!({
        "map": map,
        "rooms": [
            {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "b", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
        ],
        "connections": [
            {"id": "d1", "from": "a", "to": "b", "kind": "door", "at": [128_i64, 32_i64]},
            {"from": "a", "to": "b", "kind": "opening", "at": [128_i64, 96_i64]}
        ],
        "player_start": {"room": "a"}
    })
}

/// The working sector row at `index`.
async fn working_sector(client: &Client, map: &str, index: usize) -> Value {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["sectors"]}),
    )
    .await;
    report["sectors"]
        .as_array()
        .expect("sectors is an array")
        .get(index)
        .cloned()
        .expect("the sector row exists")
}

/// The length of a `"sectors"` array as a `u64`.
fn sector_len(list: &Value) -> u64 {
    u64::try_from(list.as_array().expect("a sector list").len()).expect("small")
}

/// How many sectors the result assigns across rooms and connectors.
fn reported_sector_count(result: &Value) -> u64 {
    let rooms = result["rooms"]
        .as_object()
        .expect("rooms is an object")
        .values()
        .map(sector_len);
    let bands = result["connections"]
        .as_array()
        .expect("connections is an array")
        .iter()
        .map(|row| &row["sectors"])
        .map(sector_len);
    rooms.chain(bands).sum()
}

/// Requires `build_map_from_plan` to reject `request` with
/// `invalid_argument` naming `needle`.
async fn refuse_with(client: &Client, request: Value, needle: &str) {
    let note = request.to_string();
    let (code, message, _) = call_err(client, "build_map_from_plan", request).await;
    assert_eq!(code, "invalid_argument", "refusal for {note}");
    assert!(
        message.contains(needle),
        "the refusal must mention {needle:?}: {message}"
    );
}

/// The working linedef and sidedef rows of the assembled graph.
async fn working_lines(client: &Client, map: &str) -> Vec<Value> {
    let graph = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["linedefs", "sidedefs"]}),
    )
    .await;
    graph["linedefs"]
        .as_array()
        .expect("linedefs is an array")
        .clone()
}

/// The row of linedef `index` in an assembled `lines` list.
fn line_row(lines: &[Value], index: u64) -> &Value {
    lines
        .iter()
        .find(|row| row["index"] == json!(index))
        .unwrap_or_else(|| panic!("line {index} is in the block"))
}

/// The first linedef index a `features` row reports.
fn first_line(row: &Value) -> u64 {
    row["lines"][0].as_u64().expect("one reported line")
}

/// The `lines` a reported feature row names, as indices.
fn row_line_indices(row: &Value) -> Vec<u64> {
    row["lines"]
        .as_array()
        .expect("a line list")
        .iter()
        .map(|slot| slot.as_u64().expect("an index"))
        .collect()
}

/// Assert every linedef indexed in `indices` carries `special` and
/// line tag `tag`.
fn assert_line_actions(lines: &[Value], indices: &[u64], special: i64, tag: i64) {
    for index in indices {
        let line = line_row(lines, *index);
        assert_eq!(
            line["special"],
            json!(special),
            "the special of line {index}"
        );
        assert_eq!(line["tag"], json!(tag), "the tag of line {index}");
    }
}

/// Assert every row of `lines` whose index is in `indices` carries
/// the `W1` walk-over level end.
fn assert_walk_lines(lines: &[Value], indices: &[u64]) {
    let crossed: Vec<&Value> = lines
        .iter()
        .filter(|line| indices.contains(&line["index"].as_u64().expect("an index")))
        .collect();
    assert_eq!(
        crossed.len(),
        indices.len(),
        "every reported line is in the block"
    );
    for line in crossed {
        let index = line["index"].as_u64().expect("an index");
        assert_eq!(
            line["special"],
            json!(52_i64),
            "the W1 exit on line {index}"
        );
    }
}

/// The `things` rows of the working map's assembled graph.
async fn working_things(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["things"]}),
    )
    .await;
    report["things"]
        .as_array()
        .expect("things is an array")
        .clone()
}

/// The thing rows of type `kind`, by type number.
fn things_of_kind(rows: &[Value], kind: i64) -> Vec<Value> {
    rows.iter()
        .filter(|row| row["type"].as_i64() == Some(kind))
        .cloned()
        .collect()
}

/// The one sector the result reports for room `room`.
fn room_sector(result: &Value, room: &str) -> u64 {
    result["rooms"][room]
        .as_array()
        .expect("a room sector list")
        .first()
        .expect("plan rooms own one sector")
        .as_u64()
        .expect("an index")
}

/// Assert the row at `index` is a `kind` row naming `room` (when
/// `room` is `Some`).
fn expect_row<'rows>(
    rows: &'rows [Value],
    index: usize,
    kind: &str,
    room: Option<&str>,
) -> &'rows Value {
    let row = rows
        .get(index)
        .unwrap_or_else(|| panic!("row {index} must be a {kind}"));
    assert_eq!(row["kind"], json!(kind), "row {index}'s class");
    assert_eq!(
        row["room"],
        room.map_or(json!(null), |name| json!(name)),
        "row {index} stands in its room"
    );
    row
}

#[cfg(test)]
#[path = "plan/basics.rs"]
mod basics;
#[cfg(test)]
#[path = "plan/features.rs"]
mod features;
#[cfg(test)]
#[path = "plan/finalize.rs"]
mod finalize;
#[cfg(test)]
#[path = "plan/panels.rs"]
mod panels;
#[cfg(test)]
#[path = "plan/six_room.rs"]
mod six_room;
#[cfg(test)]
#[path = "plan/things.rs"]
mod things;
#[cfg(test)]
#[path = "plan/units.rs"]
mod units;
