//! The `finalize` parameter: the whole node run built into the written block
//! inside the one undo step - the classic run plus `REJECT` and `BLOCKMAP` for
//! binary maps, the extended `ZNODES` lump alone for `UDMF` ones - with the
//! result's `nodes` summary read back through `get_nodes` and the errors-only
//! `validation` pass, the null pair when `finalize` is not asked for, and the
//! documented worked example of `skill/reference/plan.md` building finalized
//! and validating clean.

use serde_json::{Value, json};

use crate::plan::{features_loaded, plan_for};
use crate::support::call;

/// Requires the finalized build summary to report `variant` with
/// positive seg, subsector and node counts.
fn expect_real_tree(nodes: &Value, variant: &str) {
    assert_eq!(nodes["variant"], json!(variant), "the finalized layout");
    for field in ["segs", "subsectors", "nodes"] {
        assert!(
            nodes[field].as_u64().is_some_and(|count| count > 0),
            "{field} is positive: {nodes}",
        );
    }
}

/// Requires the read-back `get_nodes` report to agree with the
/// finalized build summary, counts and all.
fn expect_summary_matches_block(read: &Value, nodes: &Value) {
    for field in ["segs", "subsectors", "nodes"] {
        assert_eq!(
            read[field], nodes[field],
            "the summary counts match the written block",
        );
    }
}

/// Requires none of `absent` to name a lump of the listed block.
fn expect_no_lumps(lumps: &[Value], absent: [&str; 4]) {
    for name in absent {
        assert!(
            !lumps.iter().any(|lump| lump == name),
            "{name} stays out of a finalized UDMF block: {lumps:?}",
        );
    }
}

#[tokio::test]
async fn a_finalized_build_ships_the_whole_node_run_and_clean_validation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["finalize"] = json!(true);
    let result = call(&client, "build_map_from_plan", request).await;
    expect_real_tree(&result["nodes"], "classic");
    assert_eq!(
        result["validation"]["errors"],
        json!([]),
        "the finalized map validates clean: {:?}",
        result["validation"],
    );
    assert_eq!(
        result["validation"]["counts"]["error"],
        json!(0_u64),
        "the exact count agrees with the empty list",
    );
    let read = call(
        &client,
        "get_nodes",
        json!({"map": "MAP05", "scope": "working"}),
    )
    .await;
    assert_eq!(read["format"], json!("vanilla"), "the run reads back");
    expect_summary_matches_block(&read, &result["nodes"]);
    let listed = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(
        listed["maps"][0]["has_nodes"],
        json!(true),
        "the block lists with node data",
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["build_map_from_plan MAP05"]),
        "the finalize build stays inside the one undo step",
    );
    let after = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(
        after["maps"],
        json!([]),
        "undo took the nodes with the block"
    );
}

#[tokio::test]
async fn a_finalized_udmf_build_ships_znodes_and_no_binary_lumps() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["format"] = json!("udmf");
    request["finalize"] = json!(true);
    let result = call(&client, "build_map_from_plan", request).await;
    expect_real_tree(&result["nodes"], "extended");
    assert_eq!(
        result["nodes"]["format"],
        json!("ZNOD"),
        "compressed by the default",
    );
    assert_eq!(
        result["validation"]["errors"],
        json!([]),
        "no REJECT/BLOCKMAP complaints on a UDMF build: {:?}",
        result["validation"],
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    let lumps = maps["maps"][0]["lumps"]
        .as_array()
        .expect("the row lists lumps");
    assert!(
        lumps.contains(&json!("ZNODES")),
        "the extended lump is in the block: {lumps:?}",
    );
    expect_no_lumps(lumps, ["REJECT", "BLOCKMAP", "SEGS", "SSECTORS"]);
    let read = call(
        &client,
        "get_nodes",
        json!({"map": "MAP05", "scope": "working"}),
    )
    .await;
    assert_eq!(read["format"], json!("znod"), "the lump reads back");
    expect_summary_matches_block(&read, &result["nodes"]);
}

#[tokio::test]
async fn finalize_false_ships_the_block_without_node_data() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["finalize"] = json!(false);
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(result["nodes"], json!(null), "false is the unset default");
    assert_eq!(result["validation"], json!(null), "and validates nothing");
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(
        maps["maps"][0]["has_nodes"],
        json!(false),
        "the block ships bare",
    );
}

/// The complete worked example of `skill/reference/plan.md`: a
/// sketch-pixel three-room plan with a door, a stair connector,
/// named things and `finalize`.
fn worked_example() -> Value {
    json!({
        "map": "MAP07",
        "units": {"scale_num": 4_i64, "flip_y": true},
        "rooms": [
            {"id": "court",  "rect": {"x": 0_i64,  "y": 0_i64, "w": 60_i64, "h": 48_i64}},
            {"id": "hall",   "rect": {"x": 60_i64, "y": 0_i64, "w": 48_i64, "h": 48_i64},
             "light": 192},
            {"id": "crypt",  "rect": {"x": 60_i64, "y": 48_i64, "w": 48_i64, "h": 36_i64},
             "floor": 64, "ceiling": 224, "light": 96}
        ],
        "connections": [
            {"id": "gate", "from": "court", "to": "hall", "kind": "door",
             "at": [60_i64, 24_i64], "width": 48_i64, "style": "locked_red"},
            {"from": "hall", "to": "crypt", "kind": "stairs",
             "at": [84_i64, 48_i64], "width": 64_i64}
        ],
        "things": [
            {"name": "imp", "room": "hall", "count": 3_u64},
            {"name": "barrel", "x": 16_i64, "y": 40_i64}
        ],
        "player_start": {"room": "court", "angle": 90_i64},
        "finalize": true
    })
}

#[tokio::test]
async fn the_documented_worked_example_builds_finalized_and_validates_clean() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "build_map_from_plan", worked_example()).await;
    assert_eq!(
        result["rooms"],
        json!({"court": [0_u64], "hall": [1_u64], "crypt": [2_u64]}),
        "the three rooms keep their plan order",
    );
    assert_eq!(
        result["connections"],
        json!([
            {"id": "gate", "kind": "door", "sectors": [3_u64]},
            {"id": "hall->crypt", "kind": "stairs", "sectors": [4_u64, 5_u64, 6_u64, 7_u64]}
        ]),
        "one door band, four stair steps up to the crypt's floor",
    );
    assert_eq!(
        result["bounds"],
        json!({"x1": 0_i64, "y1": -336_i64, "x2": 432_i64, "y2": 0_i64}),
        "the 108x84 pixel plan at scale 4 with mirrored y",
    );
    assert_eq!(result["things_placed"], json!(5_u64), "imps, barrel, start");
    assert_eq!(
        result["warnings"],
        json!([
            "red door at gate but no red key in the plan",
            "the map has no exit"
        ]),
        "the playability checks answer for the example's gaps"
    );
    expect_worked_example_features_and_finalization(&result);
    let read = call(
        &client,
        "get_nodes",
        json!({"map": "MAP07", "scope": "working"}),
    )
    .await;
    assert_eq!(
        read["format"],
        json!("vanilla"),
        "the block carries the run"
    );
}

/// The worked example's wired gate row and its finalized `nodes`
/// summary and errors-only `validation` pass, exactly as
/// `skill/reference/plan.md` documents them.
fn expect_worked_example_features_and_finalization(result: &Value) {
    assert_eq!(
        result["features"],
        json!([
            {"kind": "door", "room": null, "sectors": [3_u64], "lines": [5_u64, 17_u64],
             "tag": 1_u64,
             "line_ends": [
                 {"index": 5_u64, "x1": 232_i64, "y1": -72_i64, "x2": 232_i64, "y2": -120_i64},
                 {"index": 17_u64, "x1": 248_i64, "y1": -120_i64, "x2": 248_i64, "y2": -72_i64}
             ]}
        ]),
        "the wired gate reports its band, lines with their endpoints, and tag",
    );
    assert_eq!(
        result["nodes"],
        json!({
            "variant": "classic",
            "segs": 61_u64,
            "subsectors": 15_u64,
            "nodes": 14_u64,
            "split_vertices": 5_u64,
            "warnings": []
        }),
        "the documented classic build summary",
    );
    assert_eq!(
        result["validation"],
        json!({"errors": [], "counts": {"error": 0_u64, "warning": 4_u64, "info": 6_u64}}),
        "no errors, and the full counts beside the empty list",
    );
}
