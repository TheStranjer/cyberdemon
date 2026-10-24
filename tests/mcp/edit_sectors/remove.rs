//! `remove_sector` and the undo bookkeeping of the sector tools: the binary
//! height, tag and flat caps following the map format, the `conflict` on a
//! sector its sidedefs still frame, the orphan an earlier add left behind
//! going alone, the cascaded room removal with its sidedefs, walls and
//! orphaned corners verified through `get_map_summary` and `assemble_map`,
//! and the add/edit/remove trio rolling back and replaying as one step each.

use serde_json::{Value, json};

use crate::edit_sectors::{
    Client, doom_loaded, formats_loaded, merged_sectors, refuse, working_linedef, working_sectors,
};
use crate::support::{call, call_err};

#[tokio::test]
async fn the_binary_height_and_tag_caps_and_the_flat_cap_follow_the_format() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = formats_loaded(dir.path()).await;
    refuse(
        &client,
        "add_sector",
        json!({"map": "MAP04", "tag": 70_000_i32}),
    )
    .await;
    refuse(
        &client,
        "edit_sector",
        json!({"map": "MAP04", "index": 0_i32, "floor_height": 40_000_i32}),
    )
    .await;
    let udmf = call(
        &client,
        "add_sector",
        json!({"map": "MAP03", "ceiling_height": 40_000_i32, "tag": 70_000_i32,
               "floor_flat": "brick01_metal"}),
    )
    .await;
    assert_eq!(
        udmf["sector"]["ceiling_height"],
        json!(40_000_i32),
        "UDMF stores the wide height"
    );
    assert_eq!(udmf["sector"]["tag"], json!(70_000_i32), "and the wide tag");
    assert_eq!(
        udmf["sector"]["floor_flat"],
        json!("BRICK01_METAL"),
        "and the thirteen-character flat"
    );
}

#[tokio::test]
async fn remove_sector_refuses_used_sectors_and_drops_orphans() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let (code, message, _) = call_err(&client, "remove_sector", json!({"index": 0_i32})).await;
    assert_eq!(code, "conflict", "sidedefs frame sector 0");
    assert!(
        message.contains("sector 0 is used by sidedef(s) 0, 1, 2, 4")
            && message.contains("cascade"),
        "the message names the users and the fix: {message}"
    );
    assert_eq!(
        merged_sectors(&client, "MAP01").await.len(),
        2,
        "the conflict removed nothing"
    );
    call(&client, "add_sector", json!({"light": 64_i32})).await;
    let result = call(&client, "remove_sector", json!({"index": 2_i32})).await;
    assert_eq!(
        result["removed"]["sectors"],
        json!([2_i32]),
        "the added orphan goes"
    );
    assert_eq!(result["removed"]["sidedefs"], json!([]), "no side followed");
    assert_eq!(result["removed"]["linedefs"], json!([]), "and no line");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the sector count is geometry"
    );
    let rows = working_sectors(&client, "MAP01").await;
    assert_eq!(rows.len(), 2, "back to the fixture's two sectors");
}

#[tokio::test]
async fn remove_sector_with_cascade_takes_the_room_with_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "remove_sector",
        json!({"index": 1_i32, "cascade": true}),
    )
    .await;
    assert_eq!(
        result["removed"]["sectors"],
        json!([1_i32]),
        "the room goes"
    );
    assert_eq!(
        result["removed"]["sidedefs"],
        json!([3_i32, 5_i32, 6_i32, 7_i32]),
        "every side of it goes"
    );
    assert_eq!(
        result["removed"]["linedefs"],
        json!([4_i32, 5_i32, 6_i32]),
        "the walls only those sides held go"
    );
    assert_eq!(
        result["removed"]["vertices"],
        json!([2_i32, 3_i32]),
        "and their orphaned corners"
    );
    assert_eq!(
        result["removed"]["one_sided_linedefs"],
        json!([2_i32]),
        "the shared step line closed"
    );
    assert_eq!(
        result["removed"]["flipped_linedefs"],
        json!([]),
        "nothing flipped"
    );
}

#[tokio::test]
async fn the_cascaded_sector_removal_shows_in_summary_and_assembly() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    call(
        &client,
        "remove_sector",
        json!({"index": 1_i32, "cascade": true}),
    )
    .await;
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        summary["counts"]["sectors"],
        json!(1_i32),
        "one sector left"
    );
    assert_eq!(
        summary["counts"]["sidedefs"],
        json!(4_i32),
        "four sides left"
    );
    assert_eq!(
        summary["counts"]["linedefs"],
        json!(4_i32),
        "four lines left"
    );
    assert_eq!(
        summary["counts"]["vertices"],
        json!(4_i32),
        "four vertices left"
    );
    let shared = working_linedef(&client, "MAP01", 2).await;
    assert_eq!(
        shared["back"],
        Value::Null,
        "the step line is one-sided now"
    );
    assert_eq!(
        shared["front"]["sector"],
        json!(0_i32),
        "its surviving side faces the hall"
    );
    assert_eq!(
        shared["flag_names"],
        json!(["blocking"]),
        "two-sided cleared, blocking set"
    );
}

/// Runs the add/edit/remove trio on a fresh sector of the fixture map.
async fn add_edit_remove_trio(client: &Client) {
    call(client, "add_sector", json!({"light": 64_i32})).await;
    call(
        client,
        "edit_sector",
        json!({"index": 2_i32, "floor_height": 8_i32}),
    )
    .await;
    call(client, "remove_sector", json!({"index": 2_i32})).await;
}

#[tokio::test]
async fn sector_edits_undo_and_redo_as_one_step_each() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    add_edit_remove_trio(&client).await;
    let after_remove = working_sectors(&client, "MAP01").await;
    assert_eq!(after_remove.len(), 2, "the trio cancelled out");
    let undo_remove = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo_remove["undone"],
        json!(["remove 1 sectors"]),
        "the removal rolls back alone"
    );
    let restored = working_sectors(&client, "MAP01").await;
    assert_eq!(restored.len(), 3, "the sector is back");
    assert_eq!(
        restored[2]["floor_height"],
        json!(8_i32),
        "with the edited floor"
    );
    let undo_edit = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo_edit["undone"],
        json!(["edit 1 sectors"]),
        "and so is the edit"
    );
    let half_undone = working_sectors(&client, "MAP01").await;
    assert_eq!(
        half_undone[2]["floor_height"],
        json!(0_i32),
        "the edit is gone"
    );
    assert_eq!(
        half_undone[2]["light"],
        json!(64_i32),
        "the add survives it"
    );
    let undo_add = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo_add["undone"],
        json!(["add sector"]),
        "and so is the add, copy included"
    );
    assert_eq!(
        merged_sectors(&client, "MAP01").await.len(),
        2,
        "back to the fixture's two"
    );
    let redo = call(&client, "redo", json!({ "steps": 3_u64 })).await;
    assert_eq!(
        redo["redone"],
        json!(["add sector", "edit 1 sectors", "remove 1 sectors"]),
        "all three steps replay in order"
    );
    let redone = working_sectors(&client, "MAP01").await;
    assert_eq!(
        redone.len(),
        2,
        "the redone removal removed the sector again"
    );
}
