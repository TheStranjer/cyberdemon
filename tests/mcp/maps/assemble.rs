//! `assemble_map`: the typed whole-map graph of the fixture map with its
//! lines, vertices, sides, sectors and things resolved to each other, the
//! `include` selection, the published asset above the inline limit read
//! back over `HTTP` with its per-kind counts, the inline-limit range
//! refusal, the dangling references reported as problems and the shared
//! map and scope errors.
//!
//! The asset row check is `assert_asset_array` here; the loaded fixtures
//! and the asset fetcher come from [`crate::maps`].

use serde_json::{Value, json};

use crate::fixtures::iwad_broken_map;
use crate::maps::{fetch_asset, iwad_loaded, iwad_plus_pwad};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn assemble_map_resolves_the_lines_of_the_fixture_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let report = call(&client, "assemble_map", json!({})).await;
    assert_eq!(report["map"], json!("MAP01"), "the map echoes");
    assert_eq!(report["problems"], json!([]), "the fixture map is sound");
    let step = &report["linedefs"][2];
    assert_eq!(
        step["front"]["sector"],
        json!(0_i64),
        "linedef 2 faces the hall",
    );
    assert_eq!(
        step["back"]["sector"],
        json!(1_i64),
        "and opens to the back room",
    );
    assert_eq!(
        step["flag_names"],
        json!(["twosided"]),
        "flags named in the Doom family",
    );
    assert_eq!(step["length"], json!(256_u64), "the shared wall measures");
    assert_eq!(
        step["v1"]["x"],
        json!(256_i64),
        "ends resolved to coordinates",
    );
    assert_eq!(step["v2"]["index"], json!(1_i64), "with their indices");
    assert_eq!(
        report["vertices"][1]["linedefs"],
        json!([2_i64, 3_i64, 6_i64]),
        "the shared corner lists its lines",
    );
    assert_eq!(
        report["sidedefs"][3]["side"],
        json!("back"),
        "sidedef 3 is the step line's back side",
    );
    assert_eq!(
        report["sidedefs"][3]["linedef"],
        json!(2_i64),
        "of the step line",
    );
}

#[tokio::test]
async fn assemble_map_resolves_the_sectors_and_things_of_the_fixture_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let report = call(&client, "assemble_map", json!({})).await;
    assert_eq!(
        report["sectors"][0]["neighbors"],
        json!([1_i64]),
        "the hall neighbours only the back room",
    );
    assert_eq!(
        report["sectors"][0]["linedefs"],
        json!([0_i64, 1_i64, 2_i64, 3_i64]),
        "the hall boundary",
    );
    assert_eq!(
        report["sectors"][0]["sidedefs"],
        json!([0_i64, 1_i64, 2_i64, 4_i64]),
        "the hall sides",
    );
    assert_eq!(
        report["sectors"][0]["bbox"],
        json!([0_i64, 0_i64, 256_i64, 256_i64]),
        "the hall box",
    );
    assert_eq!(
        report["things"][1]["sector"],
        json!(1_i64),
        "the imp (thing 1) stands in the back room",
    );
    assert_eq!(
        report["sectors"][1]["things"],
        json!([1_i64]),
        "and the back room lists it",
    );
    assert_eq!(
        report["sectors"][0]["things"],
        json!([0_i64]),
        "the player start stands in the hall",
    );
    assert_eq!(
        report["things"][0]["flag_names"],
        json!(["easy", "medium", "hard"]),
        "thing flags named too",
    );
}

#[tokio::test]
async fn assemble_map_include_returns_only_the_requested_arrays() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let report = call(&client, "assemble_map", json!({ "include": ["things"] })).await;
    assert_eq!(
        report["things"]
            .as_array()
            .expect("things is an array")
            .len(),
        2,
        "both things carry resolved sectors",
    );
    assert!(
        report.get("vertices").is_none(),
        "vertices were not requested"
    );
    assert!(
        report.get("linedefs").is_none(),
        "linedefs were not requested"
    );
    assert!(
        report.get("sidedefs").is_none(),
        "sidedefs were not requested"
    );
    assert!(
        report.get("sectors").is_none(),
        "sectors were not requested"
    );
    assert!(
        report.get("problems").is_some(),
        "problems always ride along"
    );
    let (code, message, _) =
        call_err(&client, "assemble_map", json!({ "include": ["nope"] })).await;
    assert_eq!(code, "invalid_argument", "unknown include name");
    assert!(
        message.contains("vertices") && message.contains("things"),
        "the error lists the valid names: {message}"
    );
}

/// Requires the fetched graph asset to carry `key` as an array of
/// exactly `len` rows.
fn assert_asset_array(graph: &Value, key: &str, len: usize) {
    let rows = graph[key].as_array().expect("the asset carries the array");
    assert_eq!(rows.len(), len, "the {key} array holds every row");
}

#[tokio::test]
async fn assemble_map_publishes_an_asset_above_the_inline_limit() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let report = call(&client, "assemble_map", json!({ "inline_limit": 3_u64 })).await;
    assert_eq!(report["map"], json!("MAP01"), "the map still echoes");
    assert!(report.get("linedefs").is_none(), "nothing inlines");
    assert_eq!(
        report["counts"],
        json!({
            "vertices": 6_u64, "linedefs": 7_u64, "sidedefs": 8_u64,
            "sectors": 2_u64, "things": 2_u64, "problems": 0_u64,
        }),
        "every kind counted",
    );
    assert_eq!(
        report["asset"]["filename"],
        json!("MAP01_graph.json"),
        "the documented name",
    );
    assert_eq!(
        report["asset"]["mime"],
        json!("application/json"),
        "the documented media type",
    );
    let url = report["asset"]["url"].as_str().expect("an asset url");
    let body = fetch_asset(url).await;
    let graph: Value = serde_json::from_slice(&body).expect("the asset is JSON");
    assert_eq!(graph["map"], json!("MAP01"), "the body names the map");
    assert_asset_array(&graph, "vertices", 6);
    assert_asset_array(&graph, "linedefs", 7);
    assert_asset_array(&graph, "sidedefs", 8);
    assert_asset_array(&graph, "sectors", 2);
    assert_asset_array(&graph, "things", 2);
    assert_eq!(graph["problems"], json!([]), "and the problem list");
}

#[tokio::test]
async fn assemble_map_refuses_inline_limits_outside_the_range() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let (zero_code, _, _) =
        call_err(&client, "assemble_map", json!({ "inline_limit": 0_u64 })).await;
    assert_eq!(zero_code, "invalid_argument", "zero inlines nothing");
    let (big_code, big_message, _) = call_err(
        &client,
        "assemble_map",
        json!({ "inline_limit": 20001_u64 }),
    )
    .await;
    assert_eq!(big_code, "invalid_argument", "above the maximum");
    assert!(
        big_message.contains("1..=20000"),
        "the message names the range: {big_message}"
    );
}

#[tokio::test]
async fn assemble_map_reports_dangling_references_as_problems() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad_broken_map(dir.path());
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    let report = call(&client, "assemble_map", json!({})).await;
    let problems = report["problems"].as_array().expect("problems is an array");
    assert!(!problems.is_empty(), "the break is reported, not fatal");
    assert!(
        problems.iter().any(|problem| problem
            .as_str()
            .is_some_and(|text| text.contains("sector 99"))),
        "the dangling sector is named: {problems:?}"
    );
    assert_eq!(
        report["sidedefs"][4]["sector"],
        Value::Null,
        "the row nulls it"
    );
    assert_eq!(
        report["linedefs"][3]["front"]["sector"],
        Value::Null,
        "and through the referencing line",
    );
    assert_eq!(
        report["sectors"][0]["sidedefs"],
        json!([0_i64, 1_i64, 2_i64]),
        "the hall lost its dangling side",
    );
    assert_eq!(
        report["things"][0]["sector"],
        json!(0_i64),
        "the rest of the graph still resolves",
    );
}

#[tokio::test]
async fn assemble_map_shares_the_group_map_and_scope_errors() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, _) = call_err(&client, "assemble_map", json!({ "map": "MAP99" })).await;
    assert_eq!(code, "not_found", "no such map");
    assert!(
        message.contains("MAP99"),
        "the message names the map: {message}"
    );
    let (default_code, _, _) = call_err(&client, "assemble_map", json!({})).await;
    assert_eq!(default_code, "invalid_argument", "two maps, no default");
    let (scope_code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({ "map": "MAP01", "scope": "nope" }),
    )
    .await;
    assert_eq!(scope_code, "invalid_argument", "unknown scope");
}
