//! The three inspection tools over a map's node lumps: `get_nodes` for the
//! `SEGS`/`SSECTORS`/`NODES` tree in its vanilla and extended layouts,
//! `get_reject` for the visibility matrix and `get_blockmap` for the spatial
//! index, on the fixture's hand-built `MAP05`, its malformed `MAP06` and its
//! `XNOD`-in-`ZNODES` `MAP07`: the empty-lump tolerance, the `format_error`
//! refusals, the `JSON` assets the `full` detail publishes and reads back
//! over `HTTP`, the shared `map`/`scope`/`detail` parameter checks and the
//! active-map selection driving the default.
//!
//! `assert_extended_read` is `pub(super)` because the `build_nodes` topic
//! reads a fresh extended run with it. Every other helper comes from
//! [`crate::nodes`].

use serde_json::{Value, json};

use crate::nodes::{Client, fetch_json, nodes_loaded, plain_iwad};
use crate::support::{call, call_err};

/// Requires `get_nodes` to read `MAP01`'s fresh extended lump back:
/// the given format, every count of the build report and the lump
/// size it echoes.
pub(super) async fn assert_extended_read(client: &Client, built: &Value, format: &str) {
    let read = call(client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(read["format"], json!(format), "NODES carries the run");
    assert_eq!(read["segs"], built["segs"], "the reader agrees on segs");
    assert_eq!(read["subsectors"], built["subsectors"], "and subsectors");
    assert_eq!(read["nodes"], built["nodes"], "and nodes");
    assert_eq!(read["extra_vertices"], json!(0_u64), "nothing added here");
    assert_eq!(read["bytes"], built["bytes"], "the same lump");
    assert_eq!(read["root"], json!(0_u64), "the only node is the root");
    assert_eq!(read["depth"], json!(1_u64), "one level deep");
}

#[tokio::test]
async fn empty_node_lumps_report_none_rather_than_failing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(report["format"], json!("none"), "the empty block");
    assert_eq!(report["bytes"], json!(0_u64), "no node bytes");
    assert_eq!(report["segs"], json!(0_u64), "counts read zero");
    assert_eq!(report["nodes"], json!(0_u64), "and zero nodes");
    assert_eq!(report["extra_vertices"], json!(0_u64), "no added vertices");
    assert_eq!(report["root"], Value::Null, "no tree to root");
    assert_eq!(report["depth"], json!(0_u64), "and no depth");
    assert_eq!(report["gl"]["present"], json!(false), "no GL run");

    let reject = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(reject["bytes"], json!(0_u64), "no reject bytes");
    assert_eq!(reject["sectors"], json!(2_u64), "the two fixture sectors");
    assert_eq!(reject["expected_bytes"], json!(1_u64), "four bits");
    assert_eq!(reject["all_zero"], json!(true), "nothing rejected");

    let blockmap = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
    assert_eq!(blockmap["bytes"], json!(0_u64), "no blockmap bytes");
    assert_eq!(blockmap["columns"], json!(0_u64), "no grid");
    assert_eq!(blockmap["blocks"], json!(0_u64), "no cells");
    assert_eq!(blockmap["origin"]["x"], json!(0_i64), "zeroed origin");
}

#[tokio::test]
async fn get_nodes_reads_the_hand_built_vanilla_tree_and_gl_run() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let report = call(&client, "get_nodes", json!({ "map": "MAP05" })).await;
    assert_eq!(report["format"], json!("vanilla"), "the record lumps");
    assert_eq!(report["bytes"], json!(28_u64), "one 28-byte node");
    assert_eq!(report["segs"], json!(4_u64), "four segs");
    assert_eq!(report["subsectors"], json!(2_u64), "two subsectors");
    assert_eq!(report["nodes"], json!(1_u64), "one node");
    assert_eq!(report["extra_vertices"], json!(0_u64), "vanilla adds none");
    assert_eq!(report["root"], json!(0_u64), "the only node is the root");
    assert_eq!(report["depth"], json!(1_u64), "one level");
    assert_eq!(report["gl"]["present"], json!(true), "a GL run is attached");
    assert_eq!(report["gl"]["magic"], json!("gNd2"), "the GL_VERT magic");
    assert_eq!(
        report["gl"]["lumps"],
        json!({ "GL_MAP05": 0_u64, "GL_PVS": 2_u64, "GL_SEGS": 4_u64, "GL_VERT": 6_u64 }),
        "the run's sizes",
    );
    assert!(
        report.get("asset").is_none(),
        "summary detail publishes nothing: {report}"
    );
}

#[tokio::test]
async fn get_nodes_full_publishes_the_tree_as_a_json_asset() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let report = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP05", "detail": "full" }),
    )
    .await;
    assert_eq!(
        report["asset"]["filename"],
        json!("MAP05_nodes.json"),
        "the asset name",
    );
    assert_eq!(
        report["asset"]["mime"],
        json!("application/json"),
        "the media type",
    );
    let body = fetch_json(&report).await;
    assert_eq!(body["format"], json!("vanilla"), "the layout travels");
    assert_eq!(body["segs"].as_array().expect("segs").len(), 4, "four segs");
    assert_eq!(
        body["segs"][3]["angle"],
        json!(-0x4000_i64),
        "record fields keep their signed values",
    );
    assert_eq!(
        body["subsectors"],
        json!([
            { "seg_count": 2_u64, "first_seg": 0_u64 },
            { "seg_count": 2_u64, "first_seg": 2_u64 },
        ]),
        "the subsector runs",
    );
    assert_eq!(
        body["nodes"][0]["left_child"],
        json!({ "subsector": 0_u64 }),
        "tagged children",
    );
    assert_eq!(body["extra_vertices"], json!([]), "none in vanilla");
}

#[tokio::test]
async fn extended_xnod_nodes_parse_with_their_added_vertices() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let report = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP07", "detail": "full" }),
    )
    .await;
    assert_eq!(report["format"], json!("xnod"), "the ZNODES magic");
    assert_eq!(report["segs"], json!(2_u64), "two extended segs");
    assert_eq!(report["subsectors"], json!(2_u64), "two subsectors");
    assert_eq!(report["nodes"], json!(1_u64), "one node");
    assert_eq!(report["extra_vertices"], json!(1_u64), "one added vertex");
    assert_eq!(report["root"], json!(0_u64), "the last node is the root");
    assert_eq!(report["depth"], json!(1_u64), "one level");
    let body = fetch_json(&report).await;
    assert_eq!(body["format"], json!("xnod"), "the asset layout");
    assert_eq!(
        body["extra_vertices"],
        json!([{ "x": 0x1_0000_i64, "y": -0x4_0000_i64 }]),
        "the raw 16.16 pair",
    );
    assert_eq!(body["segs"][0]["v1"], json!(6_u64), "segs reach the vertex");
}

#[tokio::test]
async fn get_reject_measures_the_matrix_and_full_publishes_rows() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let report = call(&client, "get_reject", json!({ "map": "MAP05" })).await;
    assert_eq!(report["bytes"], json!(1_u64), "one byte stored");
    assert_eq!(
        report["expected_bytes"],
        json!(1_u64),
        "two sectors need one"
    );
    assert_eq!(report["size_ok"], json!(true), "the sizes agree");
    assert_eq!(report["all_zero"], json!(false), "two bits are set");
    assert_eq!(report["rejected_pairs"], json!(2_u64), "bit 1 and bit 3");
    let full = call(
        &client,
        "get_reject",
        json!({ "map": "MAP05", "detail": "full" }),
    )
    .await;
    assert_eq!(
        full["asset"]["filename"],
        json!("MAP05_reject.json"),
        "the asset name",
    );
    let body = fetch_json(&full).await;
    assert_eq!(body["sectors"], json!(2_u64), "the matrix width");
    assert_eq!(body["rows"], json!(["01", "01"]), "one row per sector");
    let wrong = call(&client, "get_reject", json!({ "map": "MAP06" })).await;
    assert_eq!(wrong["bytes"], json!(4_u64), "four stored bytes");
    assert_eq!(wrong["expected_bytes"], json!(1_u64), "one was right");
    assert_eq!(wrong["size_ok"], json!(false), "a wrong size is data");
    assert_eq!(wrong["all_zero"], json!(true), "and its bits are clear");
}

#[tokio::test]
async fn get_blockmap_measures_the_grid_and_full_publishes_lists() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let report = call(&client, "get_blockmap", json!({ "map": "MAP05" })).await;
    assert_eq!(report["bytes"], json!(22_u64), "the lump size");
    assert_eq!(report["origin"]["x"], json!(-128_i64), "the origin");
    assert_eq!(report["origin"]["y"], json!(-256_i64), "on both axes");
    assert_eq!(report["columns"], json!(1_u64), "one column");
    assert_eq!(report["rows"], json!(2_u64), "two rows");
    assert_eq!(report["blocks"], json!(2_u64), "two cells");
    assert_eq!(report["total_entries"], json!(1_u64), "one reference");
    assert_eq!(
        report["max_lines_per_block"],
        json!(1_u64),
        "the wider cell"
    );
    assert_eq!(report["empty_blocks"], json!(1_u64), "beside an empty one");
    let full = call(
        &client,
        "get_blockmap",
        json!({ "map": "MAP05", "detail": "full" }),
    )
    .await;
    assert_eq!(
        full["asset"]["filename"],
        json!("MAP05_blockmap.json"),
        "the asset name",
    );
    let body = fetch_json(&full).await;
    assert_eq!(body["origin_x"], json!(-128_i64), "the header");
    assert_eq!(
        body["blocks"],
        json!([[3_u64], []]),
        "the lists in row order"
    );
}

#[tokio::test]
async fn malformed_node_lumps_fail_with_format_error() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let (nodes_code, nodes_message, _) =
        call_err(&client, "get_nodes", json!({ "map": "MAP06" })).await;
    assert_eq!(nodes_code, "format_error", "a broken record size");
    assert!(
        nodes_message.contains("NODES"),
        "the lump is named: {nodes_message}"
    );
    let (map_code, map_message, _) =
        call_err(&client, "get_blockmap", json!({ "map": "MAP06" })).await;
    assert_eq!(map_code, "format_error", "an offset leaves the lump");
    assert!(
        map_message.contains("BLOCKMAP"),
        "the lump is named: {map_message}"
    );
    assert_eq!(
        call(&client, "get_reject", json!({ "map": "MAP06" })).await["size_ok"],
        json!(false),
        "the wrong reject size stays data, not an error",
    );
}

#[tokio::test]
async fn the_nodes_tools_validate_their_shared_parameters() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let (missing_code, _, _) = call_err(&client, "get_nodes", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "no such block");
    let (scope_code, _, _) = call_err(&client, "get_reject", json!({ "scope": "nope" })).await;
    assert_eq!(scope_code, "invalid_argument", "not a scope");
    let (detail_code, detail_message, _) = call_err(
        &client,
        "get_blockmap",
        json!({ "map": "MAP05", "detail": "everything" }),
    )
    .await;
    assert_eq!(detail_code, "invalid_argument", "not a detail level");
    assert!(
        detail_message.contains("summary") && detail_message.contains("full"),
        "the message names both values: {detail_message}"
    );
    let (layer_code, _, _) = call_err(
        &client,
        "get_nodes",
        json!({ "map": "MAP05", "scope": "layer:9" }),
    )
    .await;
    assert_eq!(
        layer_code, "not_found",
        "an unloaded layer reports not_found"
    );
}

#[tokio::test]
async fn the_active_map_selection_drives_the_default() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    call(&client, "set_active_map", json!({ "map": "MAP05" })).await;
    let report = call(&client, "get_nodes", json!({})).await;
    assert_eq!(report["map"], json!("MAP05"), "the active map");
    assert_eq!(report["format"], json!("vanilla"), "and its tree");
    call(&client, "set_active_map", json!({ "map": "MAP07" })).await;
    let reject = call(&client, "get_reject", json!({})).await;
    assert_eq!(reject["map"], json!("MAP07"), "the selection moved");
}
