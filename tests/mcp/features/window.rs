//! `add_window` raises a blocking glass strip between two touching rooms:
//! its floor and ceiling sit on the sill/lintel trims, every room-facing
//! view line is two-sided *and* blocking with the sill below and the wall
//! above, and a too-small opening or the both/neither wall parameter
//! refuses with the whole edit uncommitted.

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, flag_names, note_mentions, reported_lines, validate_clean,
    working_sector,
};
use crate::support::{call, call_err};

/// Requires `add_window` to reject `args` naming a missing element with
/// the shared `not_found` index refusal mentioning `needle`.
async fn refuse_missing_window(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, hint) = call_err(client, "add_window", args).await;
    assert_eq!(code, "not_found", "index-refusal code for {note}");
    assert!(
        message.contains(needle) && message.contains("does not exist"),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    assert_eq!(
        hint, "describe_map or find_elements returns indices",
        "the shared index hint names the readers"
    );
}

/// Requires `add_window` to reject `args` with `invalid_argument`
/// naming `needle`.
async fn refuse_window(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_window", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

/// Checks one `add_window` result's view-line row: two-sided and
/// blocking, the sill `lower` and the wall `upper` on its room side,
/// and the strip side left open; returns the room's sector.
fn window_view_room(row: &Value) -> Value {
    assert_ne!(row["back"], Value::Null, "the view line is two-sided");
    let names = flag_names(row);
    assert!(
        names.contains(&"twosided"),
        "the glass sees both ways: {names:?}"
    );
    assert!(
        names.contains(&"blocking"),
        "blocking is set so nobody walks through: {names:?}"
    );
    let (strip, room) = if row["front"]["sector"] == json!(2_i32) {
        (&row["front"], &row["back"])
    } else {
        (&row["back"], &row["front"])
    };
    assert_eq!(room["lower"], json!("STEP1"), "the sill under the glass");
    assert_eq!(room["upper"], json!("WALL1"), "the wall over it");
    assert_eq!(
        strip["upper"],
        json!("-"),
        "from inside the strip the line is open"
    );
    assert_eq!(strip["lower"], json!("-"), "above and below");
    room["sector"].clone()
}

#[tokio::test]
async fn a_window_between_touching_rooms_raises_a_blocking_view() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let theme = call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"window_sill": "STEP1", "wall": "WALL1"}}),
    )
    .await;
    assert_eq!(
        theme["roles"]["window_sill"]["texture"],
        json!("STEP1"),
        "the sill override stands for the role"
    );
    let result = call(&client, "add_window", json!({"between": [0_i32, 1_i32]})).await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["window_sector"], json!(2_i32), "a third sector");
    assert_eq!(result["width"], json!(64_i32), "the default width");
    assert_eq!(result["sill"], json!(32_i32), "the default sill");
    assert_eq!(result["lintel"], json!(24_i32), "the default lintel");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the strip is new geometry"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    assert!(
        note_mentions(&result, "void strip"),
        "the strip carve is noted: {}",
        result["notes"]
    );
    let window = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(window["floor_height"], json!(64_i32), "max(0, 32) + 32");
    assert_eq!(
        window["ceiling_height"],
        json!(104_i32),
        "min(128, 128) - 24"
    );
    assert_eq!(window["special"], json!(0_i32), "inert");
    assert_eq!(window["tag"], json!(0_i32), "untagged");
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    assert_eq!(lines.len(), 2, "one view line per room");
    let rooms: Vec<Value> = lines.iter().map(window_view_room).collect();
    assert!(
        rooms.contains(&json!(0_i32)) && rooms.contains(&json!(1_i32)),
        "both rooms see the glass: {rooms:?}"
    );
    validate_clean(&client, "MAP01").await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add window"]),
        "the whole window rolls back together"
    );
}

#[tokio::test]
async fn a_window_with_too_small_an_opening_refuses() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_window(
        &client,
        json!({"between": [0_i32, 1_i32], "sill": 90_i32, "lintel": 24_i32}),
        "at least 16",
    )
    .await;
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "the refused window did not even copy the block into the working archive"
    );
}

#[tokio::test]
async fn the_window_wall_parameter_needs_exactly_one_form() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_window(&client, json!({}), "exactly one").await;
    refuse_window(
        &client,
        json!({"between": [0_i32, 1_i32], "linedef": 2_i32}),
        "exactly one",
    )
    .await;
    refuse_missing_window(
        &client,
        json!({"between": [0_i32, 99_i32]}),
        "sector 99 does not exist",
    )
    .await;
    refuse_missing_window(
        &client,
        json!({"linedef": 999_i32}),
        "linedef 999 does not exist",
    )
    .await;
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "no refused window even copied the block into the working archive"
    );
}
