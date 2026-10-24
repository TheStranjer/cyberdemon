//! The `bulk_edit` tool: query-selected set/add/flag edits on sectors,
//! linedefs, things and vertices, the never-clamped range refusal, the `max`
//! guard, the geometry rule for vertex moves, thing-type name resolution and
//! the one-step undo over every field on every element.

use serde_json::json;

use crate::query::replace_things::things_of_type;
use crate::query::{edit, find, indices, one, plain_iwad};
use crate::support::{call, call_err};

#[tokio::test]
async fn bulk_edit_shifts_matching_sector_lights_in_one_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = edit(
        &client,
        json!({
            "kind": "sectors",
            "where": one("light", "gt", &json!(170_i64)),
            "add": {"light": -32_i64},
        }),
    )
    .await;
    assert_eq!(report["matched"], json!(1_i64), "only the bright room");
    assert_eq!(report["changed"], json!(1_i64), "its light moved");
    assert_eq!(indices(&report), vec![1_u64], "sector 1 changed");
    assert_eq!(
        report["nodes_invalidated"],
        json!(false),
        "light edits keep the nodes"
    );
    assert_eq!(
        report["copied_to_working"],
        json!(true),
        "the layer map was copied"
    );
    let rows = find(&client, json!({"kind": "sectors"})).await;
    assert_eq!(
        rows["rows"][0]["light"],
        json!(160_i64),
        "sector 0 kept its light"
    );
    assert_eq!(
        rows["rows"][1]["light"],
        json!(160_i64),
        "sector 1 came down to 160"
    );
}

#[tokio::test]
async fn bulk_edit_dry_run_previews_without_writing_or_recording() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let preview = edit(
        &client,
        json!({
            "kind": "sectors",
            "where": one("light", "gt", &json!(170_i64)),
            "add": {"light": -32_i64},
            "dry_run": true,
        }),
    )
    .await;
    assert_eq!(preview["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(preview["changed"], json!(1_i64), "the change is counted");
    assert_eq!(
        preview["preview"][0],
        json!({
            "index": 1_i64,
            "before": {"light": 192_i64},
            "after": {"light": 160_i64},
        }),
        "the preview row: {preview}"
    );
    assert_eq!(
        preview["copied_to_working"],
        json!(false),
        "a preview copies nothing"
    );
    let rows = find(&client, json!({"kind": "sectors"})).await;
    assert_eq!(
        rows["rows"][1]["light"],
        json!(192_i64),
        "the map is unchanged"
    );
    let (code, message, _) = call_err(&client, "undo", json!({})).await;
    assert_eq!(code, "not_found", "the preview recorded no step");
    assert_eq!(message, "nothing to undo", "the stack stayed empty");
}

#[tokio::test]
async fn bulk_edit_never_clamps_and_writes_nothing_on_a_bad_value() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "bulk_edit",
        json!({"map": "MAP01", "kind": "sectors", "add": {"light": -200_i64}}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "out of range refuses");
    assert!(
        message.contains("sector 0") && message.contains("-40"),
        "the refusal names the row and the would-be value: {message}"
    );
    let rows = find(&client, json!({"kind": "sectors"})).await;
    assert_eq!(rows["rows"][0]["light"], json!(160_i64), "sector 0 stands");
    assert_eq!(
        rows["rows"][1]["light"],
        json!(192_i64),
        "and so does sector 1"
    );
}

#[tokio::test]
async fn bulk_edit_max_guard_refuses_before_writing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "bulk_edit",
        json!({"kind": "sectors", "max": 0_i64, "add": {"light": 1_i64}}),
    )
    .await;
    assert_eq!(code, "limit_exceeded", "two matches over max 0 refuse");
    assert!(
        message.contains("max") && message.contains("dry_run") && message.contains("where"),
        "the refusal names max, dry_run and the narrowing: {message}"
    );
    let rows = find(&client, json!({"kind": "sectors"})).await;
    assert_eq!(
        rows["rows"][0]["light"],
        json!(160_i64),
        "the guard wrote nothing"
    );
}

#[tokio::test]
async fn bulk_edit_moves_vertices_only_by_add_and_invalidates_nodes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "bulk_edit",
        json!({"kind": "vertices", "set": {"x": 0_i64}}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "vertices take no set");
    assert!(
        message.contains("add"),
        "the refusal points at add: {message}"
    );
    let report = edit(&client, json!({"kind": "vertices", "add": {"x": 16_i64}})).await;
    assert_eq!(report["matched"], json!(6_i64), "all six fixture vertices");
    assert_eq!(report["changed"], json!(6_i64), "all six slid");
    assert_eq!(
        report["nodes_invalidated"],
        json!(true),
        "moving referenced vertices is geometry"
    );
    let moved = find(
        &client,
        json!({"kind": "vertices", "where": one("x", "eq", &json!(16_i64))}),
    )
    .await;
    assert_eq!(indices(&moved), vec![0_u64, 5], "the west wall slid east");
}

#[tokio::test]
async fn bulk_edit_sets_linedef_flags_by_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = edit(
        &client,
        json!({"kind": "linedefs", "flags_set": ["blockmonsters"]}),
    )
    .await;
    assert_eq!(
        report["matched"],
        json!(7_i64),
        "every fixture line matched"
    );
    assert_eq!(report["changed"], json!(7_i64), "every line gained the bit");
    let gated = find(
        &client,
        json!({"kind": "linedefs", "where": one("flags", "has", &json!("blockmonsters"))}),
    )
    .await;
    assert_eq!(
        indices(&gated),
        (0..7).collect::<Vec<u64>>(),
        "the bits read back through the query"
    );
    let (code, message, _) = call_err(
        &client,
        "bulk_edit",
        json!({
            "kind": "linedefs",
            "where": one("flags", "has", &json!("playeruse")),
            "flags_set": ["mapped"],
        }),
    )
    .await;
    assert_eq!(
        code, "invalid_argument",
        "a Hexen flag name is not for this map"
    );
    assert!(
        message.contains("playeruse") && message.contains("doom family"),
        "the refusal names the flag and the map's family: {message}"
    );
}

#[tokio::test]
async fn bulk_edit_resolves_thing_type_names_and_refuses_the_stray() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let swapped = edit(
        &client,
        json!({
            "kind": "things",
            "where": one("name", "glob", &json!("imp*")),
            "set": {"type": "demon"},
        }),
    )
    .await;
    assert_eq!(swapped["matched"], json!(1_i64), "the fixture imp matched");
    assert_eq!(swapped["changed"], json!(1_i64), "its type moved");
    assert_eq!(
        swapped["nodes_invalidated"],
        json!(false),
        "thing edits keep the nodes"
    );
    let demons = find(&client, things_of_type(3002)).await;
    assert_eq!(
        indices(&demons),
        vec![1_u64],
        "the imp reads back as a demon"
    );
    let (stray_code, stray_message, _) = call_err(
        &client,
        "bulk_edit",
        json!({"kind": "things", "set": {"type": "platypus"}}),
    )
    .await;
    assert_eq!(stray_code, "invalid_argument", "no thing is named platypus");
    assert!(
        stray_message.contains("get_thing_types"),
        "the reference is named: {stray_message}"
    );
}

#[tokio::test]
async fn one_undo_restores_every_bulk_edit_field_on_every_element() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = edit(
        &client,
        json!({
            "kind": "sectors",
            "set": {"light": 96_i64},
            "add": {"tag": 7_i64, "floor": 8_i64},
        }),
    )
    .await;
    assert_eq!(report["matched"], json!(2_i64), "both sectors matched");
    assert_eq!(report["changed"], json!(2_i64), "both moved on every field");
    let rows = find(&client, json!({"kind": "sectors"})).await;
    assert_eq!(rows["rows"][0]["light"], json!(96_i64), "sector 0 darkened");
    assert_eq!(
        rows["rows"][1]["tag"],
        json!(8_i64),
        "sector 1's tag rode the add"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["bulk_edit sectors (2)"]),
        "one labeled step carried every field on both sectors"
    );
    let back = find(&client, json!({"kind": "sectors"})).await;
    let restored = back["rows"].as_array().expect("two sector rows");
    let hall = restored.first().expect("sector 0's row");
    assert_eq!(hall["light"], json!(160_i64), "sector 0's light is back");
    assert_eq!(hall["tag"], json!(0_i64), "and its tag");
    assert_eq!(hall["floor"], json!(0_i64), "and its floor");
    let room = restored.get(1).expect("sector 1's row");
    assert_eq!(room["light"], json!(192_i64), "sector 1's light is back");
    assert_eq!(room["tag"], json!(1_i64), "and its tag");
    assert_eq!(room["floor"], json!(32_i64), "and its floor");
}
