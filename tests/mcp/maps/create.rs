//! `create_map`: the empty `Doom` and `Hexen` blocks with the working
//! listing, summary and active-map adoption behind them, the `player_start`
//! a call can stand in the fresh block with its wrapped facing and refused
//! wide coordinate, the warning and `next_steps` a start-less block carries
//! and the `validate_map` error that follows it, the `UDMF` block declaring
//! its namespace, the odd-name warning with its bad-format, bad-namespace
//! and empty-name refusals and the working duplicate refusal, the `replace`
//! rewrite and its one-step undo.
//!
//! The expected create result is `created_doom_map` here; the loaded
//! fixture, the row reader and the member names come from [`crate::maps`].

use serde_json::{Value, json};

use crate::maps::{DOOM_MEMBERS, iwad_loaded, map_named};
use crate::support::{call, call_err, connect};

/// The whole `create_map` result for a fresh `doom` block.
fn created_doom_map() -> Value {
    json!({
        "map": "MAP05", "format": "doom", "namespace": null, "start": null,
        "warnings": [NO_START], "active_map": "MAP05",
        "next_steps": [NEEDS_START_STEP, VERIFY_STEP],
    })
}

/// The notice a start-less fresh block carries.
const NO_START: &str = "the new map holds no player 1 start, so nothing can spawn: stand \
                        one in a sector (add_thing type 1) once the map has geometry, or \
                        pass player_start to create_map";
/// The first `next_steps` entry of a start-less fresh block.
const NEEDS_START_STEP: &str =
    "stand a player 1 start in a sector (add_thing type 1) once the map has geometry";
/// The verification step every fresh block is handed.
const VERIFY_STEP: &str = "validate_map and check_playability once the map is drawn";

#[tokio::test]
async fn create_map_builds_empty_doom_and_hexen_blocks_and_sets_the_active_map() {
    let client = connect().await;
    let doom = call(&client, "create_map", json!({ "name": "map05" })).await;
    assert_eq!(doom, created_doom_map(), "the documented doom result");
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let row = map_named(&listing, "MAP05");
    assert_eq!(row["format"], json!("doom"), "the vanilla set");
    assert_eq!(row["lumps"], json!(DOOM_MEMBERS), "the ten empty members");
    assert_eq!(
        row["source"],
        json!("working"),
        "it is the working archive's"
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({ "map": "MAP05", "scope": "working" }),
    )
    .await;
    assert_eq!(
        summary["counts"],
        json!({"things": 0_u64, "vertices": 0_u64, "linedefs": 0_u64, "sidedefs": 0_u64, "sectors": 0_u64}),
        "an empty map holds nothing",
    );
    assert_eq!(summary["bounds"], Value::Null, "and has no bounds");
    let hexen = call(
        &client,
        "create_map",
        json!({ "name": "MAP06", "format": "hexen" }),
    )
    .await;
    assert_eq!(hexen["format"], json!("hexen"), "the hexen report");
    assert_eq!(
        hexen["active_map"],
        json!("MAP05"),
        "only an unset active map is adopted",
    );
    let hexen_listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        map_named(&hexen_listing, "MAP06")["format"],
        json!("hexen"),
        "the generated BEHAVIOR classifies it",
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["create_map MAP06"]),
        "the hexen create is one undoable step",
    );
}

#[tokio::test]
async fn create_map_writes_a_udmf_block_declaring_its_namespace() {
    let client = connect().await;
    let udmf = call(
        &client,
        "create_map",
        json!({ "name": "MAP07", "format": "udmf", "namespace": "GZDoom" }),
    )
    .await;
    assert_eq!(udmf["namespace"], json!("gzdoom"), "the lowered namespace");
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let row = map_named(&listing, "MAP07");
    assert_eq!(row["format"], json!("udmf"), "the TEXTMAP block");
    assert_eq!(row["udmf_namespace"], json!("gzdoom"), "which decodes");
    assert_eq!(
        row["lumps"],
        json!(["TEXTMAP", "ENDMAP"]),
        "and carries only its namespace so far",
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(udmf["map"], json!("MAP07"), "the create echoes the map");
    assert_eq!(summary["format"], json!("udmf"), "the empty UDMF map reads");
    assert_eq!(
        summary["counts"]["things"],
        json!(0_u64),
        "with no elements at all",
    );
}

#[tokio::test]
async fn create_map_stands_an_asked_player_start_in_the_new_block() {
    let client = connect().await;
    let report = call(
        &client,
        "create_map",
        json!({
            "name": "MAP05",
            "player_start": {"x": -64_i64, "y": 128_i64, "angle": 370_i64},
        }),
    )
    .await;
    assert_eq!(
        report["start"],
        json!({"thing": 0_u64, "x": -64_i64, "y": 128_i64, "angle": 10_i64}),
        "the point travels and the facing wraps: {report}",
    );
    assert_eq!(
        report["warnings"],
        json!([]),
        "a loadable name with a start deserves no notice",
    );
    assert_eq!(
        report["next_steps"],
        json!([
            "draw the rooms around the start (create_sector_from_loop) or build a plan",
            VERIFY_STEP,
        ]),
        "the steps skip the start ask once it stands",
    );
    let summary = call(&client, "get_map_summary", json!({ "map": "MAP05" })).await;
    assert_eq!(
        summary["counts"]["things"],
        json!(1_u64),
        "THINGS holds the one record",
    );
    let found = call(
        &client,
        "find_elements",
        json!({
            "map": "MAP05",
            "kind": "things",
            "where": [{"field": "type", "op": "eq", "value": 1_i64}],
        }),
    )
    .await;
    assert_eq!(found["count"], json!(1_u64), "and it is a type 1 start");
    assert_eq!(found["indices"], json!([0_u64]), "as thing 0");
}

#[tokio::test]
async fn create_map_refuses_a_start_a_binary_layout_cannot_store() {
    let client = connect().await;
    let (code, message, _) = call_err(
        &client,
        "create_map",
        json!({"name": "MAP05", "player_start": {"x": 40_000_i64, "y": 0_i64}}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "a binary THINGS x column is i16");
    assert!(
        message.contains('x'),
        "the message names the field: {message}"
    );
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert!(
        listing["maps"].as_array().expect("array").is_empty(),
        "the refusal wrote nothing: {listing}",
    );
    let udmf = call(
        &client,
        "create_map",
        json!({
            "name": "MAP06",
            "format": "udmf",
            "player_start": {"x": 40_000_i64, "y": -40_000_i64},
        }),
    )
    .await;
    assert_eq!(
        udmf["start"],
        json!({"thing": 0_u64, "x": 40_000_i64, "y": -40_000_i64, "angle": 0_i64}),
        "UDMF stores full integers",
    );
}

#[tokio::test]
async fn a_fresh_block_asks_for_its_start_and_add_thing_answers() {
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP05" })).await;
    let before = call(
        &client,
        "validate_map",
        json!({ "map": "MAP05", "codes": ["no_player1_start"] }),
    )
    .await;
    assert_eq!(before["counts"]["error"], json!(1_u64), "nobody spawns yet");
    let issue = before["issues"]
        .as_array()
        .expect("an issue list")
        .first()
        .expect("the start row")
        .clone();
    assert_eq!(issue["code"], json!("no_player1_start"), "the named check");
    assert_eq!(issue["severity"], json!("error"), "a broken map");
    assert!(
        issue["message"]
            .as_str()
            .expect("text")
            .contains("no player 1 start")
            && issue["message"]
                .as_str()
                .expect("text")
                .contains("add thing type 1"),
        "the message says what is missing and what to add: {issue}",
    );
    call(
        &client,
        "add_thing",
        json!({ "map": "MAP05", "type": 1_i64, "x": 0_i64, "y": 0_i64 }),
    )
    .await;
    let after = call(
        &client,
        "validate_map",
        json!({ "map": "MAP05", "codes": ["no_player1_start"] }),
    )
    .await;
    assert_eq!(
        after["counts"]["error"],
        json!(0_u64),
        "the start settles the error: {after}",
    );
    assert_eq!(after["issues"], json!([]), "and reports nothing else");
}

#[tokio::test]
async fn create_map_warns_about_odd_names_and_refuses_bad_arguments() {
    let client = connect().await;
    let report = call(&client, "create_map", json!({ "name": "SLICK01" })).await;
    assert_eq!(report["map"], json!("SLICK01"), "the odd name is created");
    let warnings = report["warnings"].as_array().expect("warnings is an array");
    assert_eq!(
        warnings.len(),
        2,
        "the odd name and the missing start: {warnings:?}"
    );
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|text| text.contains("SLICK01") && text.contains("MAPINFO")),
        "the warning names the map and the fix: {warnings:?}",
    );
    assert_eq!(
        warnings[1],
        json!(NO_START),
        "and the block says nobody can spawn in it yet",
    );
    let (format_code, format_message, _) = call_err(
        &client,
        "create_map",
        json!({ "name": "MAP08", "format": "zdoom" }),
    )
    .await;
    assert_eq!(format_code, "invalid_argument", "not a map format");
    assert!(
        format_message.contains("doom") && format_message.contains("udmf"),
        "the message lists the formats: {format_message}",
    );
    let (ns_code, _, _) = call_err(
        &client,
        "create_map",
        json!({ "name": "MAP08", "namespace": "zdoom" }),
    )
    .await;
    assert_eq!(ns_code, "invalid_argument", "namespace on a binary map");
    let (name_code, _, _) = call_err(&client, "create_map", json!({ "name": "" })).await;
    assert_eq!(name_code, "invalid_argument", "an empty name");
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let maps = listing["maps"].as_array().expect("maps is an array");
    assert_eq!(maps.len(), 1, "only the warned creation landed");
    assert_eq!(
        maps.first().expect("one row")["name"],
        json!("SLICK01"),
        "by name"
    );
}

#[tokio::test]
async fn create_map_refuses_working_duplicates_replaces_and_overrides_layers() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    assert_eq!(
        call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["source"],
        json!("working"),
        "the empty block overrides the IWAD's",
    );
    let (code, message, _) = call_err(&client, "create_map", json!({ "name": "MAP01" })).await;
    assert_eq!(code, "already_exists", "the working override clashes");
    assert!(
        message.contains("replace"),
        "the message names the way out: {message}",
    );
    call(
        &client,
        "add_thing",
        json!({ "map": "MAP01", "type": 1_i64, "x": 8_i64, "y": 8_i64 }),
    )
    .await;
    assert_eq!(
        call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
        json!(1_u64),
        "the block has a thing before the replace",
    );
    let replaced = call(
        &client,
        "create_map",
        json!({ "name": "MAP01", "replace": true }),
    )
    .await;
    assert_eq!(replaced["active_map"], json!("MAP01"), "the replace lands");
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        listing["maps"].as_array().expect("array").len(),
        1,
        "the old block was discarded, not trailed",
    );
    assert_eq!(
        call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
        json!(0_u64),
        "the replaced block is empty again",
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["create_map MAP01"]),
        "the replace is one undoable step",
    );
    assert_eq!(
        call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
        json!(1_u64),
        "undo brings the thing back",
    );
}
