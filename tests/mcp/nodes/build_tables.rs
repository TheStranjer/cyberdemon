//! The two auxiliary builders: `build_blockmap` writing the grid and
//! `build_reject` writing the visibility matrix, each one read back through
//! its inspection tool, `compress: false` changing only the `BLOCKMAP` lump
//! size, the `zero` and `components` `REJECT` modes, and the refusals of an
//! unknown mode and of a map with no vertices to grid.
//!
//! Every helper here comes from [`crate::nodes`].

use serde_json::json;

use crate::fixtures::pwad_empty_map;
use crate::nodes::plain_iwad;
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn build_blockmap_writes_a_grid_the_inspector_reads_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
    assert_eq!(built["map"], json!("MAP01"), "the fragment");
    assert_eq!(
        built["copied_to_working"],
        json!(true),
        "the map came in to edit"
    );
    assert_eq!(
        built["nodes_invalidated"],
        json!(false),
        "a lump rewrite is not geometry"
    );
    assert_eq!(
        built["origin"]["x"],
        json!(-8_i64),
        "8 below the lowest vertex"
    );
    assert_eq!(built["origin"]["y"], json!(-8_i64), "on both axes");
    assert_eq!(built["columns"], json!(5_u64), "512 units span 5 cells");
    assert_eq!(built["rows"], json!(3_u64), "256 span 3");
    assert_eq!(built["vanilla_safe"], json!(true), "a toy map is tiny");
    let read = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
    assert_eq!(
        read["bytes"], built["bytes"],
        "the merged view shows the build"
    );
    assert_eq!(read["columns"], json!(5_u64), "and agrees on the grid");
    assert_eq!(read["rows"], json!(3_u64));
    assert_eq!(read["blocks"], json!(15_u64), "5x3 cells");
    assert_eq!(
        read["total_entries"],
        json!(21_u64),
        "the two-room wall span"
    );
    assert_eq!(
        read["empty_blocks"],
        json!(2_u64),
        "the two interior corners"
    );
    let (code, _, _) = call_err(&client, "build_blockmap", json!({ "map": "MAP99" })).await;
    assert_eq!(code, "not_found", "no such block to build");
}

#[tokio::test]
async fn blockmap_compression_only_changes_the_lump_size() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let packed = call(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
    let plain = call(
        &client,
        "build_blockmap",
        json!({ "map": "MAP01", "compress": false }),
    )
    .await;
    assert_eq!(
        plain["copied_to_working"],
        json!(false),
        "already in the working copy"
    );
    assert!(
        plain["bytes"].as_u64().expect("a byte count")
            > packed["bytes"].as_u64().expect("a byte count"),
        "uncompressed copies every list: {} vs {}",
        plain["bytes"],
        packed["bytes"],
    );
    let working = call(
        &client,
        "get_blockmap",
        json!({ "map": "MAP01", "scope": "working" }),
    )
    .await;
    assert_eq!(working["bytes"], plain["bytes"], "the last build stands");
    assert_eq!(
        working["total_entries"],
        json!(21_u64),
        "with the same contents"
    );
}

#[tokio::test]
async fn build_reject_writes_matrices_the_inspector_agrees_with() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let zero = call(&client, "build_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(zero["mode"], json!("zero"), "the default mode");
    assert_eq!(zero["bytes"], json!(1_u64), "two sectors need one byte");
    assert_eq!(zero["sectors"], json!(2_u64), "the decoded count");
    assert_eq!(zero["rejected_pairs"], json!(0_u64), "nothing rejected");
    assert_eq!(
        zero["nodes_invalidated"],
        json!(false),
        "reject data is not geometry"
    );
    let read = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(read["bytes"], json!(1_u64), "the written lump");
    assert_eq!(read["expected_bytes"], json!(1_u64), "at its expected size");
    assert_eq!(read["size_ok"], json!(true), "and the inspector agrees");
    assert_eq!(
        read["all_zero"],
        json!(true),
        "the clear matrix came back clear"
    );
    let parts = call(
        &client,
        "build_reject",
        json!({ "map": "MAP01", "mode": "COMPONENTS" }),
    )
    .await;
    assert_eq!(parts["mode"], json!("components"), "the echo");
    assert_eq!(
        parts["rejected_pairs"],
        json!(0_u64),
        "the step line joins both sectors, so nothing is rejected",
    );
    let connected = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(connected["all_zero"], json!(true), "and the lump says so");
}

#[tokio::test]
async fn builders_refuse_an_empty_map_and_an_unknown_mode() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let empty = pwad_empty_map(dir.path());
    call(
        &client,
        "open_iwad",
        json!({ "path": empty.display().to_string() }),
    )
    .await;
    let (code, message, _) = call_err(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
    assert_eq!(code, "invalid_argument", "no vertices to grid");
    assert!(message.contains("vertices"), "the hint: {message}");
    let (mode_code, mode_message, _) = call_err(
        &client,
        "build_reject",
        json!({ "map": "MAP01", "mode": "visibility" }),
    )
    .await;
    assert_eq!(mode_code, "invalid_argument", "not a mode");
    assert!(
        mode_message.contains("zero") && mode_message.contains("components"),
        "the message names both values: {mode_message}"
    );
    let bare = call(&client, "build_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(bare["sectors"], json!(0_u64), "no sectors");
    assert_eq!(bare["bytes"], json!(0_u64), "no bits to write");
    assert_eq!(bare["rejected_pairs"], json!(0_u64), "and nothing rejected");
}
