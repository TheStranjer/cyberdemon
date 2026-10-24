//! The `things` and `player_start` parameters: named things filling a room
//! with the start landing in its centre, room fill and a transformed pinned
//! point with presence flags round-tripping through the written block, `skills`
//! and `multiplayer_only` riding into the thing flags, and the refusals around
//! them - an ambiguous name listing its candidates, a plan without any player
//! start refused by name, a row naming an unknown room naming its row and room,
//! and a count larger than the room warning `fits_only` and keeping what fit.

use serde_json::json;

use crate::plan::{features_loaded, plan_for, refuse_with, things_of_kind, working_things};
use crate::support::{call, call_err};

#[tokio::test]
async fn named_things_fill_a_room_and_the_start_lands_in_its_centre() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["things"] = json!([{"name": "imp", "room": "a", "count": 4_u64}]);
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["things_placed"],
        json!(5_u64),
        "four imps and the player start"
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP05", "scope": "working"}),
    )
    .await;
    assert_eq!(
        summary["counts"]["things"],
        json!(5_u64),
        "the written block holds every placed row"
    );
    assert_eq!(
        summary["player_starts"],
        json!([1_u64]),
        "the room-form start is a real type 1"
    );
    let report = call(
        &client,
        "validate_map",
        json!({"map": "MAP05", "scope": "working", "codes": ["no_player1_start"]}),
    )
    .await;
    assert_eq!(
        report["counts"]["error"],
        json!(0_u64),
        "no player-start error: {:?}",
        report["issues"]
    );
    let imps = things_of_kind(&working_things(&client, "MAP05").await, 3001_i64);
    assert_eq!(imps.len(), 4, "four imps in room a");
    assert!(
        imps.iter()
            .all(|row| row["x"].as_i64().is_some_and(|x| x > 0_i64)
                && row["y"].as_i64().is_some_and(|y| y > 0_i64)),
        "every imp stands clear of the walls: {imps:?}"
    );
}

#[tokio::test]
async fn a_pinned_named_point_travels_the_units_transform() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let request = json!({
        "map": "MAP05",
        "rooms": [
            {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
        ],
        "connections": [],
        "units": {"scale_num": 4_i64},
        "things": [{"name": "barrel", "x": 10_i64, "y": 10_i64}],
        "player_start": {"room": "a"}
    });
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(result["things_placed"], json!(2_u64), "barrel plus start");
    let barrels = things_of_kind(&working_things(&client, "MAP05").await, 2035_i64);
    let barrel = barrels.first().expect("one barrel");
    assert_eq!(
        (barrel["x"].as_i64(), barrel["y"].as_i64()),
        (Some(40_i64), Some(40_i64)),
        "plan (10, 10) at scale 4 lands on map (40, 40)"
    );
}

#[tokio::test]
async fn skills_and_multiplayer_only_round_trip_into_thing_flags() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["things"] = json!([
        {"type": 2035_u64, "x": 40_i64, "y": 40_i64,
         "skills": ["easy"], "multiplayer_only": true},
        {"type": 2035_u64, "room": "b", "count": 1_u64}
    ]);
    call(&client, "build_map_from_plan", request).await;
    let barrels = things_of_kind(&working_things(&client, "MAP05").await, 2035_i64);
    assert_eq!(barrels.len(), 2, "both barrel rows landed");
    let pinned = barrels.first().expect("two barrels");
    assert_eq!(
        pinned["flags"],
        json!(17_u64),
        "easy (1) + notsingle (16) in the Doom family"
    );
    assert_eq!(
        pinned["flag_names"],
        json!(["easy", "notsingle"]),
        "the names read back from the written block"
    );
    let plain = barrels.get(1).expect("two barrels");
    assert_eq!(
        plain["flag_names"],
        json!(["easy", "medium", "hard"]),
        "unset skills and mode default to present everywhere"
    );
}

#[tokio::test]
async fn an_ambiguous_name_is_refused_listing_the_candidates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["things"] = json!([{"name": "skul", "room": "a"}]);
    let (code, message, _) = call_err(&client, "build_map_from_plan", request).await;
    assert_eq!(code, "invalid_argument", "several things match 'skul'");
    assert!(message.contains("things[0]"), "the row is named: {message}");
    assert!(
        message.contains("Skull on Pole"),
        "the candidates: {message}"
    );
    assert!(
        message.contains("Pile of Skulls"),
        "the candidates: {message}"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
}

#[tokio::test]
async fn a_plan_without_any_player_start_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request
        .as_object_mut()
        .expect("the plan object")
        .remove("player_start");
    let (code, message, _) = call_err(&client, "build_map_from_plan", request.clone()).await;
    assert_eq!(code, "invalid_argument", "type 1 is mandatory");
    assert!(
        message.contains("a map needs a player 1 start"),
        "the documented advice: {message}"
    );
    let mut named = request;
    named["things"] = json!([{"type": 1_u64, "room": "a"}]);
    let result = call(&client, "build_map_from_plan", named).await;
    assert_eq!(
        result["things_placed"],
        json!(1_u64),
        "a thing of type 1 satisfies the check"
    );
}

#[tokio::test]
async fn a_count_larger_than_the_room_warns_fits_only_and_keeps_what_fit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let request = json!({
        "map": "MAP05",
        "rooms": [
            {"id": "closet", "rect": {"x": 0_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}}
        ],
        "connections": [],
        "things": [{"name": "imp", "room": "closet", "count": 4_u64}],
        "player_start": {"room": "closet"}
    });
    let result = call(&client, "build_map_from_plan", request).await;
    let warnings = result["warnings"].as_array().expect("a warning list");
    assert!(
        warnings
            .iter()
            .any(|note| note.as_str() == Some("room 'closet' fits only 1 of 4 things")),
        "the short room is noted: {warnings:?}"
    );
    assert_eq!(
        result["things_placed"],
        json!(2_u64),
        "one imp cleared the walls, plus the centre start"
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP05", "scope": "working"}),
    )
    .await;
    assert_eq!(summary["counts"]["things"], json!(2_u64), "as written");
}

#[tokio::test]
async fn an_unknown_thing_room_names_its_row_and_room() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["things"] = json!([{"name": "imp", "room": "ghost"}]);
    refuse_with(&client, request, "things[0].room").await;
}
