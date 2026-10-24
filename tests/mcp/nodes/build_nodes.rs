//! `build_nodes` writing the whole tree either as the classic run (its split
//! vertices appended to `VERTEXES` by the build and stripped again by the
//! next geometry edit, with `undo` bringing them back) or as one `ZDoom`
//! extended lump in `NODES`/`ZNODES` with the splits kept inside the lump and
//! `compress` choosing between the `ZNOD` and `XNOD` magics, beside the
//! unknown-variant, unknown-map and empty-geometry refusals and the rule that
//! `UDMF` maps only take extended nodes.
//!
//! The fresh extended lump is confirmed with [`crate::nodes::read`]'s
//! `assert_extended_read`; every other helper comes from [`crate::nodes`].

use serde_json::json;

use crate::fixtures::pwad_empty_map;
use crate::nodes::read::assert_extended_read;
use crate::nodes::{fetch_json, l_room_loaded, nodes_loaded, plain_iwad};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn build_nodes_writes_vanilla_lumps_the_readers_confirm() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(&client, "build_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(built["variant"], json!("classic"), "the default variant");
    assert_eq!(
        built["segs"],
        json!(8_u64),
        "seven fronts and the shared back"
    );
    assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
    assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
    assert_eq!(built["split_vertices"], json!(0_u64), "flush corners");
    assert_eq!(built["warnings"], json!([]), "each leaf bounds one sector");
    assert_eq!(built["map"], json!("MAP01"), "the fragment");
    assert_eq!(
        built["nodes_invalidated"],
        json!(false),
        "a build is a product, not a geometry change"
    );
    assert_eq!(
        built["copied_to_working"],
        json!(true),
        "the map came in to edit"
    );
    let read = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(read["format"], json!("vanilla"), "the record lumps");
    assert_eq!(read["segs"], built["segs"], "the reader agrees on segs");
    assert_eq!(read["subsectors"], built["subsectors"], "and subsectors");
    assert_eq!(read["nodes"], built["nodes"], "and nodes");
    assert_eq!(read["root"], json!(0_u64), "the only node is the root");
    assert_eq!(read["depth"], json!(1_u64), "one level deep");
    let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(summary["nodes"]["present"], json!(true), "nodes are there");
    assert_eq!(summary["nodes"]["segs"], json!(8_u64), "SEGS over 12");
    assert_eq!(summary["nodes"]["nodes"], json!(1_u64), "NODES over 28");
    assert_eq!(
        summary["counts"]["vertices"],
        json!(6_u64),
        "this map splits nothing"
    );
}

#[tokio::test]
async fn split_vertices_arrive_with_the_build_and_leave_with_the_nodes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = l_room_loaded(dir.path()).await;
    let built = call(&client, "build_nodes", json!({})).await;
    assert_eq!(built["map"], json!("MAP01"), "the only map is default");
    assert_eq!(
        built["segs"],
        json!(7_u64),
        "the cut doubles the bottom seg"
    );
    assert_eq!(built["subsectors"], json!(2_u64), "the arms are leaves");
    assert_eq!(built["nodes"], json!(1_u64), "one cut suffices");
    assert_eq!(
        built["split_vertices"],
        json!(1_u64),
        "the cut point had no vertex"
    );
    let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        summary["counts"]["vertices"],
        json!(7_u64),
        "the split vertex joined VERTEXES"
    );
    assert_eq!(
        summary["nodes"]["present"],
        json!(true),
        "and the tree landed"
    );
}

#[tokio::test]
async fn a_geometry_edit_strips_split_vertices_and_undo_restores_them() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = l_room_loaded(dir.path()).await;
    call(&client, "build_nodes", json!({})).await;
    let moved = call(
        &client,
        "move_vertex",
        json!({ "map": "MAP01", "index": 0_i64, "dx": 16_i64 }),
    )
    .await;
    assert_eq!(
        moved["nodes_invalidated"],
        json!(true),
        "moving a used vertex is geometry"
    );
    let after = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        after["counts"]["vertices"],
        json!(6_u64),
        "the split vertex was stripped with the invalidated nodes"
    );
    assert_eq!(after["nodes"]["present"], json!(false), "the run emptied");
    call(&client, "undo", json!({})).await;
    let restored = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        restored["counts"]["vertices"],
        json!(7_u64),
        "undo brings the split vertex back"
    );
    assert_eq!(restored["nodes"]["present"], json!(true), "and the tree");
    let nodes = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(nodes["format"], json!("vanilla"), "the undone build reads");
    assert_eq!(nodes["segs"], json!(7_u64), "with the cut segs");
}

#[tokio::test]
async fn build_nodes_refuses_unknown_variants_unknown_maps_and_empty_geometry() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "znodes" }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "not a variant");
    assert!(
        message.contains("classic") && message.contains("extended") && message.contains("gl"),
        "the message names all three: {message}"
    );
    let (missing_code, _, _) = call_err(&client, "build_nodes", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "no such block");
    let empty_dir = tempfile::tempdir().expect("a scratch directory");
    let empty_client = connect().await;
    let empty = pwad_empty_map(empty_dir.path());
    call(
        &empty_client,
        "open_iwad",
        json!({ "path": empty.display().to_string() }),
    )
    .await;
    let (no_code, no_message, _) =
        call_err(&empty_client, "build_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(no_code, "invalid_argument", "nothing to tree");
    assert!(
        no_message.contains("sidedefs"),
        "the hint names sides: {no_message}"
    );
    let (gl_code, gl_message, _) = call_err(
        &empty_client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    assert_eq!(gl_code, "invalid_argument", "gl needs sides too");
    assert!(
        gl_message.contains("sidedefs"),
        "the hint names sides: {gl_message}"
    );
    let (scope_code, _, _) = call_err(
        &empty_client,
        "get_map_summary",
        json!({ "map": "MAP01", "scope": "working" }),
    )
    .await;
    assert_eq!(scope_code, "not_found", "the refusal wrote nothing");
}

#[tokio::test]
async fn build_nodes_extended_writes_a_znod_lump_the_reader_confirms() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "EXTENDED" }),
    )
    .await;
    assert_eq!(built["variant"], json!("extended"), "the echo");
    assert_eq!(built["format"], json!("ZNOD"), "compressed by default");
    assert_eq!(
        built["segs"],
        json!(8_u64),
        "seven fronts and the shared back"
    );
    assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
    assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
    assert_eq!(built["new_vertices"], json!(0_u64), "flush corners");
    assert_eq!(built["warnings"], json!([]), "each leaf bounds one sector");
    assert_eq!(built["map"], json!("MAP01"), "the fragment");
    assert_eq!(
        built["nodes_invalidated"],
        json!(false),
        "a build is a product, not a geometry change"
    );
    assert_eq!(
        built["copied_to_working"],
        json!(true),
        "the map came in to edit"
    );
    assert!(
        built.get("split_vertices").is_none(),
        "the classic-only key stays out of the extended result: {built}"
    );
    let bytes = built["bytes"].as_u64().expect("a lump size");
    assert!(bytes > 4, "magic plus a payload: {bytes}");
    assert_extended_read(&client, &built, "znod").await;
    let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        summary["counts"]["vertices"],
        json!(6_u64),
        "an extended build never touches VERTEXES"
    );
    assert_eq!(summary["nodes"]["present"], json!(true), "nodes are there");
    assert_eq!(
        summary["nodes"]["nodes_format"],
        json!("ZNOD"),
        "the summary reports the magic",
    );
    assert!(
        summary["nodes"].get("segs").is_none(),
        "the vanilla counts give way to it: {summary}"
    );
}

#[tokio::test]
async fn extended_compression_only_changes_the_lump_not_the_records() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "extended" }),
    )
    .await;
    let packed = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP01", "detail": "full" }),
    )
    .await;
    assert_eq!(packed["format"], json!("znod"), "the compressed lump");
    let packed_body = fetch_json(&packed).await;
    let raw = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "extended", "compress": false }),
    )
    .await;
    assert_eq!(raw["format"], json!("XNOD"), "compress false stores raw");
    assert_eq!(
        raw["copied_to_working"],
        json!(false),
        "already in the working copy"
    );
    let open = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP01", "detail": "full" }),
    )
    .await;
    assert_eq!(open["format"], json!("xnod"), "the raw magic reads back");
    let open_body = fetch_json(&open).await;
    assert_eq!(open_body["format"], json!("xnod"), "the asset layout");
    assert_eq!(
        packed_body["format"],
        json!("znod"),
        "and its compressed twin"
    );
    assert!(
        ["extra_vertices", "segs", "subsectors", "nodes"]
            .iter()
            .all(|field| open_body[*field] == packed_body[*field]),
        "compressed and uncompressed decode to the same records"
    );
}

#[tokio::test]
async fn extended_build_on_udmf_writes_znodes_and_classic_refuses_there() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let built = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP07", "variant": "extended" }),
    )
    .await;
    assert_eq!(built["variant"], json!("extended"), "the echo");
    assert_eq!(built["format"], json!("ZNOD"), "compressed by default");
    assert_eq!(built["segs"], json!(8_u64), "the two-room map splits once");
    assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
    assert_eq!(built["new_vertices"], json!(0_u64), "flush corners");
    let working = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(
        working["format"],
        json!("znod"),
        "ZNODES holds the fresh run"
    );
    assert_eq!(working["segs"], built["segs"], "the reader agrees");
    assert_eq!(
        working["bytes"], built["bytes"],
        "the block's ZNODES replaced the fixture's XNOD"
    );
    let merged = call(&client, "get_nodes", json!({ "map": "MAP07" })).await;
    assert_eq!(
        merged["format"],
        json!("znod"),
        "the merged view shows the build"
    );
    let (code, message, _) = call_err(&client, "build_nodes", json!({ "map": "MAP07" })).await;
    assert_eq!(code, "invalid_argument", "classic has no place in UDMF");
    assert!(
        message.contains("UDMF maps use extended nodes"),
        "the message names the rule: {message}"
    );
    let (gl_code, gl_message, _) = call_err(
        &client,
        "build_nodes",
        json!({ "map": "MAP07", "variant": "gl" }),
    )
    .await;
    assert_eq!(gl_code, "invalid_argument", "gl writes the classic run too");
    assert!(
        gl_message.contains("UDMF maps use extended nodes"),
        "the same rule: {gl_message}"
    );
    let after = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(after["format"], json!("znod"), "the refusal wrote nothing");
}
