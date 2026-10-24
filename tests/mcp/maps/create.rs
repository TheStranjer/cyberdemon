//! `create_map`: the empty `Doom` and `Hexen` blocks with the working
//! listing, summary and active-map adoption behind them, the `UDMF` block
//! declaring its namespace, the odd-name warning with its bad-format,
//! bad-namespace and empty-name refusals and the working duplicate
//! refusal, the `replace` rewrite and its one-step undo.
//!
//! The expected create result is `created_doom_map` here; the loaded
//! fixture, the row reader and the member names come from [`crate::maps`].

use serde_json::{Value, json};

use crate::maps::{DOOM_MEMBERS, iwad_loaded, map_named};
use crate::support::{call, call_err, connect};

/// The whole `create_map` result for a fresh `doom` block.
fn created_doom_map() -> Value {
    json!({
        "map": "MAP05", "format": "doom", "namespace": null,
        "warnings": [], "active_map": "MAP05",
    })
}

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
async fn create_map_warns_about_odd_names_and_refuses_bad_arguments() {
    let client = connect().await;
    let report = call(&client, "create_map", json!({ "name": "SLICK01" })).await;
    assert_eq!(report["map"], json!("SLICK01"), "the odd name is created");
    let warnings = report["warnings"].as_array().expect("warnings is an array");
    assert_eq!(warnings.len(), 1, "one notice: {warnings:?}");
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|text| text.contains("SLICK01") && text.contains("MAPINFO")),
        "the warning names the map and the fix: {warnings:?}",
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
