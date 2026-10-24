//! The thing swap: `replace_thing_type` with its doomednum-or-name resolution,
//! its target check and `allow_unknown` escape, the number the binary `Doom`
//! format cannot store, the preview that records no history, the one labelled
//! undo over both maps and the `not_found` that names the map tools.
//!
//! The `{kind: "things", type: n}` query these tests read the swaps back
//! through lives here so the `bulk` tests can reuse it.

use serde_json::{Value, json};

use crate::query::find::add_imp_in_room;
use crate::query::{find, indices, iwad_plus_pwad, one, plain_iwad};
use crate::support::{call, call_err};

/// A `{kind: "things", type: n}` query over the fixture map.
pub(super) fn things_of_type(kind: i64) -> Value {
    json!({ "kind": "things", "where": one("type", "eq", &json!(kind)) })
}

#[tokio::test]
async fn replace_thing_type_resolves_names_and_moves_every_imp() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let report = call(
        &client,
        "replace_thing_type",
        json!({ "from": "imp", "to": "demon" }),
    )
    .await;
    assert_eq!(
        report["from"],
        json!(3001_i64),
        "the exact name Imp resolves"
    );
    assert_eq!(
        report["to"],
        json!(3002_i64),
        "the exact name Demon wins over CyberDemon (16) and the other substrings"
    );
    assert_eq!(report["maps"], json!({"MAP01": 2_i64}), "both imps moved");
    assert_eq!(report["total"], json!(2_i64), "the total is the imp count");
    assert_eq!(report["dry_run"], json!(false), "the swap wrote");
    let demons = find(&client, things_of_type(3002)).await;
    assert_eq!(
        indices(&demons),
        vec![1_u64, 2],
        "every imp reads back as a demon"
    );
    assert_eq!(
        demons["rows"][0]["x"],
        json!(384_i64),
        "the moved thing keeps its x"
    );
    assert_eq!(demons["rows"][0]["angle"], json!(180_i64), "and its facing");
    let start = find(&client, things_of_type(1)).await;
    assert_eq!(
        start["rows"][0],
        json!({
            "index": 0_i64, "type": 1_i64, "name": "Player 1 Start",
            "x": 64_i64, "y": 128_i64, "angle": 0_i64,
        }),
        "the player start's type, position and facing stand unchanged"
    );
    let flagged = find(
        &client,
        json!({
            "kind": "things",
            "where": [
                { "field": "type", "op": "eq", "value": 1_i64 },
                { "field": "flags", "op": "has", "value": "hard" },
            ],
        }),
    )
    .await;
    assert_eq!(
        flagged["count"],
        json!(1_i64),
        "and so do its presence flags"
    );
}

#[tokio::test]
async fn replace_thing_type_refuses_undefined_targets_unless_allowed() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (from_code, from_message, _) = call_err(
        &client,
        "replace_thing_type",
        json!({ "from": "not-a-thing", "to": "demon" }),
    )
    .await;
    assert_eq!(from_code, "invalid_argument", "unknown from name code");
    assert!(
        from_message.contains("from:"),
        "the failing parameter is named: {from_message}"
    );
    let (to_code, to_message, _) = call_err(
        &client,
        "replace_thing_type",
        json!({ "from": "imp", "to": "not-a-thing" }),
    )
    .await;
    assert_eq!(to_code, "invalid_argument", "unknown to name code");
    assert!(
        to_message.contains("to:"),
        "the failing parameter is named: {to_message}"
    );
    let (number_code, number_message, _) = call_err(
        &client,
        "replace_thing_type",
        json!({ "from": "imp", "to": 99999_i64 }),
    )
    .await;
    assert_eq!(number_code, "invalid_argument", "undefined target code");
    assert!(
        number_message.contains("get_thing_types") && number_message.contains("allow_unknown"),
        "the reference and the escape hatch are named: {number_message}"
    );
    let standing = find(&client, things_of_type(3001)).await;
    assert_eq!(
        indices(&standing),
        vec![1_u64],
        "the refusals wrote nothing"
    );
    let report = call(
        &client,
        "replace_thing_type",
        json!({ "from": "imp", "to": 9999_i64, "allow_unknown": true }),
    )
    .await;
    assert_eq!(
        report["to"],
        json!(9999_i64),
        "the written number reports as-is"
    );
    assert_eq!(
        report["total"],
        json!(1_i64),
        "the fixture imp was rewritten"
    );
    let odd = find(&client, things_of_type(9999)).await;
    assert_eq!(indices(&odd), vec![1_u64], "the thing reads back as 9999");
}

#[tokio::test]
async fn replace_thing_type_refuses_a_number_the_map_format_cannot_store() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "replace_thing_type",
        json!({ "from": "imp", "to": 99999_i64, "allow_unknown": true }),
    )
    .await;
    assert_eq!(code, "limit_exceeded", "the binary Doom u16 limit");
    assert!(
        message.contains("99999") && message.contains("u16"),
        "the message names the number and the field: {message}"
    );
    let standing = find(&client, things_of_type(3001)).await;
    assert_eq!(indices(&standing), vec![1_u64], "the refusal wrote nothing");
}

#[tokio::test]
async fn replace_thing_type_previews_without_recording_history() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let preview = call(
        &client,
        "replace_thing_type",
        json!({ "from": 3001_i64, "to": 3002_i64, "dry_run": true }),
    )
    .await;
    assert_eq!(preview["maps"], json!({"MAP01": 2_i64}), "counts reported");
    assert_eq!(preview["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(
        preview["copied_maps"],
        json!([]),
        "a preview copies nothing"
    );
    let standing = find(&client, things_of_type(3001)).await;
    assert_eq!(indices(&standing), vec![1_u64, 2], "nothing was written");
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add thing 3001"]),
        "the preview recorded no step of its own"
    );
    assert_eq!(
        undone["remaining_undo"],
        json!(0_i64),
        "the add was the only entry"
    );
}

#[tokio::test]
async fn one_undo_restores_every_thing_type_as_a_labeled_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let report = call(
        &client,
        "replace_thing_type",
        json!({ "from": 3001_i64, "to": 3002_i64 }),
    )
    .await;
    assert_eq!(report["total"], json!(2_i64), "both imps moved");
    assert_eq!(
        report["copied_maps"],
        json!([]),
        "add_thing had copied the map in"
    );
    let demons = find(&client, things_of_type(3002)).await;
    assert_eq!(
        indices(&demons),
        vec![1_u64, 2],
        "and they read back as demons"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["replace_thing_type 3001 -> 3002"]),
        "one labeled step carried both things"
    );
    assert_eq!(
        undone["remaining_undo"],
        json!(1_i64),
        "the add step stands below"
    );
    let back = find(&client, things_of_type(3001)).await;
    assert_eq!(
        indices(&back),
        vec![1_u64, 2],
        "one undo restored every type"
    );
    let gone = find(&client, things_of_type(3002)).await;
    assert_eq!(gone["count"], json!(0_i64), "no demons are left");
}

#[tokio::test]
async fn replace_thing_type_sweeps_all_maps_and_refuses_unknown_ones() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, hint) = call_err(
        &client,
        "replace_thing_type",
        json!({ "from": 3001_i64, "to": 3002_i64, "maps": ["MAP99"] }),
    )
    .await;
    assert_eq!(code, "not_found", "unknown map code");
    assert!(
        message.contains("MAP99"),
        "the message names the map: {message}"
    );
    assert!(
        hint.contains("list_maps") && hint.contains("create_map"),
        "the hint names the map tools: {hint}"
    );
    let report = call(
        &client,
        "replace_thing_type",
        json!({ "from": 3001_i64, "to": 3002_i64, "maps": "all" }),
    )
    .await;
    assert_eq!(
        report["maps"],
        json!({"MAP01": 1_i64, "MAP02": 1_i64}),
        "every merged block answered"
    );
    assert_eq!(report["total"], json!(2_i64), "the sum over both maps");
    assert_eq!(
        report["copied_maps"],
        json!(["MAP01", "MAP02"]),
        "both maps came from read-only layers"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["replace_thing_type 3001 -> 3002"]),
        "one labelled step carried both maps"
    );
    assert_eq!(
        undone["remaining_undo"],
        json!(0_i64),
        "nothing else was recorded"
    );
    let back01 = find(&client, things_of_type(3001)).await;
    assert_eq!(
        indices(&back01),
        vec![1_u64],
        "MAP01's imp is back after the undo"
    );
}
