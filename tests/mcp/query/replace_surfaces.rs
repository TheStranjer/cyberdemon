//! The surface swaps: `replace_texture` and `replace_flat` across the active
//! map, every map or a listed set, with their copy-on-write reports, the
//! face and surface lists, the stray-name and unknown-target refusals, the
//! one-step undo over both blocks and the `dry_run` preview that writes and
//! records nothing.
//!
//! The six `WALL1`-dressed middles of the fixture and the query that reads
//! them back are the two helpers only these tests use.

use serde_json::{Value, json};

use crate::query::find::add_imp_in_room;
use crate::query::{find, indices, iwad_plus_pwad, one, plain_iwad};
use crate::support::{call, call_err};

/// The six `WALL1`-dressed middles of the two-room fixture.
const FIXTURE_WALL1_MIDDLES: [u64; 6] = [0_u64, 1, 4, 5, 6, 7];

/// A `{kind: "sidedefs", middle: NAME}` query over one map.
fn middles_wearing(map: &str, name: &str) -> Value {
    json!({
        "map": map,
        "kind": "sidedefs",
        "where": one("middle", "eq", &json!(name)),
    })
}

#[tokio::test]
async fn replace_texture_swaps_the_fixture_wall_across_one_undoable_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = call(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "STEP1" }),
    )
    .await;
    assert_eq!(
        report["maps"],
        json!({ "MAP01": 6_i64 }),
        "the fixture WALL1 sits on six middles: {report}"
    );
    assert_eq!(report["total"], json!(6_i64), "the sum over the maps");
    assert_eq!(
        report["copied_maps"],
        json!(["MAP01"]),
        "the map came from the IWAD layer"
    );
    assert_eq!(report["dry_run"], json!(false), "the swap wrote");
    assert!(
        report["next_steps"]
            .as_array()
            .is_some_and(|steps| !steps.is_empty()),
        "the report names what to do next: {report}"
    );
    let after = find(&client, middles_wearing("MAP01", "STEP1")).await;
    assert_eq!(
        indices(&after),
        FIXTURE_WALL1_MIDDLES,
        "the six middles wear STEP1 now"
    );
}

#[tokio::test]
async fn replace_texture_lists_faces_and_refuses_stray_names_and_targets() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = call(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "STEP1", "parts": ["upper"] }),
    )
    .await;
    assert_eq!(report["maps"], json!({ "MAP01": 0_i64 }), "no WALL1 upper");
    assert_eq!(report["total"], json!(0_i64), "nothing changed");
    let standing = find(&client, middles_wearing("MAP01", "WALL1")).await;
    assert_eq!(
        indices(&standing),
        FIXTURE_WALL1_MIDDLES,
        "a face-listed miss writes nothing to the other faces"
    );
    let (face_code, face_message, _) = call_err(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "STEP1", "parts": ["roof"] }),
    )
    .await;
    assert_eq!(face_code, "invalid_argument", "stray face code");
    assert!(
        face_message.contains("upper")
            && face_message.contains("middle")
            && face_message.contains("lower"),
        "the message lists the three faces: {face_message}"
    );
    let (gone_code, gone_message, _) = call_err(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "NOSUCH" }),
    )
    .await;
    assert_eq!(gone_code, "not_found", "unknown target code");
    assert!(
        gone_message.contains("list_textures"),
        "the fix is named: {gone_message}"
    );
    let untouched = find(&client, middles_wearing("MAP01", "WALL1")).await;
    assert_eq!(
        indices(&untouched),
        FIXTURE_WALL1_MIDDLES,
        "the refusals wrote nothing"
    );
}

#[tokio::test]
async fn replace_texture_over_all_maps_copies_both_blocks_and_undoes_as_one() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "STEP1", "maps": "all" }),
    )
    .await;
    assert_eq!(
        report["maps"],
        json!({ "MAP01": 6_i64, "MAP02": 6_i64 }),
        "every merged block answered"
    );
    assert_eq!(report["total"], json!(12_i64), "the sum over both maps");
    assert_eq!(
        report["copied_maps"],
        json!(["MAP01", "MAP02"]),
        "both maps came from read-only layers"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["replace_texture WALL1 -> STEP1"]),
        "one labelled step carried both maps"
    );
    assert_eq!(
        undone["remaining_undo"],
        json!(0_i64),
        "nothing else was recorded"
    );
    let back01 = find(&client, middles_wearing("MAP01", "WALL1")).await;
    assert_eq!(
        indices(&back01),
        FIXTURE_WALL1_MIDDLES,
        "MAP01 reads its layer block again after the undo"
    );
    let back02 = find(&client, middles_wearing("MAP02", "WALL1")).await;
    assert_eq!(
        indices(&back02),
        FIXTURE_WALL1_MIDDLES,
        "and so does MAP02: both working blocks came off with the one step"
    );
}

#[tokio::test]
async fn replace_flat_moves_only_the_listed_surfaces() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = call(
        &client,
        "replace_flat",
        json!({ "from": "FLOOR1", "to": "PNGFLAT", "surfaces": ["floor"] }),
    )
    .await;
    assert_eq!(report["maps"], json!({ "MAP01": 2_i64 }), "the two floors");
    assert_eq!(report["total"], json!(2_i64), "only the floors were listed");
    assert_eq!(
        report["copied_maps"],
        json!(["MAP01"]),
        "the map was copied in"
    );
    let sectors = find(&client, json!({ "kind": "sectors" })).await;
    let rows = sectors["rows"].as_array().expect("two sector rows");
    assert_eq!(rows.len(), 2_usize, "both fixture sectors");
    assert_eq!(
        rows[0]["floor_flat"],
        json!("PNGFLAT"),
        "the hall floor moved"
    );
    assert_eq!(
        rows[0]["ceiling_flat"],
        json!("FLOOR1"),
        "and only the floor: the hall ceiling stands"
    );
    assert_eq!(
        rows[1]["floor_flat"],
        json!("PNGFLAT"),
        "the room floor moved"
    );
    assert_eq!(
        rows[1]["ceiling_flat"],
        json!("FLOOR1"),
        "and only the floor: the room ceiling stands"
    );
    let (stray_code, stray_message, _) = call_err(
        &client,
        "replace_flat",
        json!({ "from": "FLOOR1", "to": "PNGFLAT", "surfaces": ["roof"] }),
    )
    .await;
    assert_eq!(stray_code, "invalid_argument", "stray surface code");
    assert!(
        stray_message.contains("floor") && stray_message.contains("ceiling"),
        "the message lists the two surfaces: {stray_message}"
    );
    let (gone_code, gone_message, _) = call_err(
        &client,
        "replace_flat",
        json!({ "from": "FLOOR1", "to": "NOSUCH" }),
    )
    .await;
    assert_eq!(gone_code, "not_found", "unknown flat code");
    assert!(
        gone_message.contains("list_flats"),
        "the fix is named: {gone_message}"
    );
}

#[tokio::test]
async fn a_dry_run_previews_counts_without_writing_or_recording() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let texture = call(
        &client,
        "replace_texture",
        json!({ "from": "WALL1", "to": "STEP1", "dry_run": true }),
    )
    .await;
    assert_eq!(texture["total"], json!(6_i64), "the swap is previewed");
    assert_eq!(texture["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(
        texture["copied_maps"],
        json!([]),
        "a preview copies nothing"
    );
    let still = find(&client, middles_wearing("MAP01", "WALL1")).await;
    assert_eq!(
        indices(&still),
        FIXTURE_WALL1_MIDDLES,
        "nothing was written"
    );
    let flats = call(
        &client,
        "replace_flat",
        json!({ "from": "FLOOR1", "to": "PNGFLAT", "dry_run": true }),
    )
    .await;
    assert_eq!(flats["total"], json!(4_i64), "two sectors, both surfaces");
    let described = call(&client, "describe_map", json!({})).await;
    assert_eq!(
        described["sectors"][0]["floor_flat"],
        json!("FLOOR1"),
        "describe_map still reads the layer's flats: {described}"
    );
    assert_eq!(
        described["sectors"][0]["ceiling_flat"],
        json!("FLOOR1"),
        "floor and ceiling alike"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add thing 3001"]),
        "undo walks the previous stack, no new entry above it"
    );
    assert_eq!(
        undone["remaining_undo"],
        json!(0_i64),
        "only the imp was there"
    );
}
