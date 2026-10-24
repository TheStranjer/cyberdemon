//! `carve_void` as an MCP client calls it: the pillar punched inside the hall
//! with its four one-sided walls, the trench that separates the two rooms and
//! leaves neither of them a neighbour, and the missing or doubled forms, weak,
//! flat and crossing rings and out-of-range coordinates being refused, all as
//! one undo step.
//!
//! The single-wall check comes from the `create_loop` topic beside it; the
//! request builders, row readers and refusal wrappers come from
//! [`crate::edit_sectors`].

use serde_json::{Value, json};

use crate::edit_sectors::create_loop::expect_single_walls;
use crate::edit_sectors::{
    doom_loaded, end, merged_sectors, pt, refuse_all, same_ends, validate_clean, working_linedefs,
    working_sector_row,
};
use crate::support::{call, call_err};

#[tokio::test]
async fn carve_void_punches_a_pillar_in_the_hall() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "carve_void",
        json!({"points": [pt(96, 96), pt(160, 96), pt(160, 160), pt(96, 160)]}),
    )
    .await;
    assert_eq!(result["map"], json!("MAP01"), "the default map");
    assert_eq!(result["sector"], Value::Null, "a carve adds no sector");
    assert_eq!(
        result["linedefs_created"],
        json!([3_i32, 4_i32, 5_i32, 6_i32]),
        "the four walls around the rock"
    );
    assert_eq!(
        result["vertices_created"],
        json!([6_i32, 7_i32, 8_i32, 9_i32]),
        "the pillar corners"
    );
    assert_eq!(
        result["removed_linedefs"],
        json!([]),
        "no existing line died"
    );
    assert_eq!(
        result["removed_sectors_empty"],
        json!([]),
        "the hall survives"
    );
    expect_single_walls(
        &working_linedefs(&client, "MAP01").await,
        result["linedefs_created"].as_array().expect("a list"),
        Some(0_i32),
        "WALL1",
    );
    validate_clean(&client, "MAP01").await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["carve void"]),
        "the carve is one undo step"
    );
}

#[tokio::test]
async fn carve_void_separates_the_two_rooms() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "carve_void",
        json!({"points": [pt(192, 0), pt(320, 0), pt(320, 256), pt(192, 256)]}),
    )
    .await;
    assert_eq!(
        result["removed_linedefs"],
        json!([2_i32]),
        "the shared step line is carved away"
    );
    assert_eq!(
        result["linedefs_created"],
        json!([3_i32, 4_i32]),
        "the trench walls, east of the hall and west of the room"
    );
    assert_eq!(
        result["removed_sectors_empty"],
        json!([]),
        "both rooms survive"
    );
    let lines = working_linedefs(&client, "MAP01").await;
    expect_single_walls(
        &lines,
        result["linedefs_created"].as_array().expect("a list"),
        None,
        "WALL1",
    );
    let trench = lines
        .iter()
        .find(|line| same_ends(line, [end(192, 0), end(192, 256)]))
        .expect("the hall-facing trench wall");
    assert_eq!(trench["front"]["sector"], json!(0_i32), "walls the hall");
    let far = lines
        .iter()
        .find(|line| same_ends(line, [end(320, 0), end(320, 256)]))
        .expect("the room-facing trench wall");
    assert_eq!(far["front"]["sector"], json!(1_i32), "walls the room");
    let hall = working_sector_row(&client, "MAP01", 0).await;
    let room = working_sector_row(&client, "MAP01", 1).await;
    assert!(
        hall["neighbors"].as_array().expect("a list").is_empty(),
        "the hall no longer touches the room: {}",
        hall["neighbors"]
    );
    assert!(
        room["neighbors"].as_array().expect("a list").is_empty(),
        "nor back: {}",
        room["neighbors"]
    );
    validate_clean(&client, "MAP01").await;
}

/// Carve requests the tool must reject with `invalid_argument`: a
/// missing or doubled form, a weak, flat or crossing ring and a
/// coordinate outside the binary `i16` range.
fn invalid_carves() -> Vec<Value> {
    vec![
        json!({}),
        json!({"points": [pt(0, 0), pt(64, 64)]}),
        json!({"points": [pt(0, 0), pt(128, 0), pt(256, 0)]}),
        json!({"points": [pt(0, 0), pt(256, 256), pt(256, 0), pt(0, 256)]}),
        json!({"points": [pt(0, 0), pt(40000, 0), pt(0, 256)]}),
        json!({"points": [pt(0, 0), pt(64, 64)], "vertices": [0, 1, 2]}),
    ]
}

#[tokio::test]
async fn carve_void_refuses_bad_polygons() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    refuse_all(&client, "carve_void", invalid_carves()).await;
    let (code, _, _) = call_err(
        &client,
        "carve_void",
        json!({"vertices": [0_i32, 1_i32, 99_i32]}),
    )
    .await;
    assert_eq!(code, "not_found", "unknown vertex");
    let (map_code, _, _) = call_err(
        &client,
        "carve_void",
        json!({"map": "MAP09", "points": [pt(0, 0), pt(64, 0), pt(0, 64)]}),
    )
    .await;
    assert_eq!(map_code, "not_found", "unknown map");
    let rows = merged_sectors(&client, "MAP01").await;
    assert_eq!(rows.len(), 2, "the refusals carved nothing");
}
