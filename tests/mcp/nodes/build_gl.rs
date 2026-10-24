//! `build_nodes` with `variant: "gl"`: the `glBSP` v2 block of
//! miniseg-closed subsectors attached right after the map block in `glBSP`
//! lump order, `list_maps` seeing the run, `get_nodes` counting it back, a
//! rebuild replacing it without duplicating the block, `undo` taking it away
//! and property edits leaving the fresh tree in place.
//!
//! `assert_gl_read_back` is the fresh run's read-back check; every other
//! helper comes from [`crate::nodes`].

use serde_json::{Value, json};

use crate::nodes::{Client, directory_names, l_room_loaded, plain_iwad};
use crate::support::call;

#[tokio::test]
async fn build_nodes_gl_writes_classic_lumps_and_the_readers_agree() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    assert_eq!(built["variant"], json!("gl"), "the echo");
    assert_eq!(
        built["segs"],
        json!(8_u64),
        "the classic run is written as well"
    );
    assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
    assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
    assert_eq!(built["split_vertices"], json!(0_u64), "flush corners");
    assert_eq!(
        built["gl_segs"],
        json!(8_u64),
        "every edge holds a real seg"
    );
    assert_eq!(
        built["minisegs"],
        json!(0_u64),
        "the shared line is covered both sides"
    );
    assert_eq!(
        built["gl_vertices"],
        json!(0_u64),
        "the GL run needs no fraction points"
    );
    assert_eq!(
        built["nodes_invalidated"],
        json!(false),
        "a build is a product"
    );
    assert_eq!(
        built["copied_to_working"],
        json!(true),
        "the map came in to edit"
    );
    let maps = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        maps["maps"][0]["has_gl_nodes"],
        json!(true),
        "list_maps sees the run"
    );
    assert_gl_read_back(&client, &built).await;
}

/// Requires `get_nodes` on `MAP01` to read the fresh `glBSP` v2 run:
/// the classic records still parsed and the GL block counted.
async fn assert_gl_read_back(client: &Client, built: &Value) {
    let read = call(client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(
        read["format"],
        json!("vanilla"),
        "the classic run still reads"
    );
    assert_eq!(
        read["gl"]["present"],
        json!(true),
        "and the GL block rides along"
    );
    assert_eq!(read["gl"]["magic"], json!("gNd2"), "the v2 magic");
    assert_eq!(read["gl"]["segs"], built["gl_segs"], "GL_SEGS rows counted");
    assert_eq!(read["gl"]["subsectors"], json!(2_u64), "GL_SSECT rows");
    assert_eq!(read["gl"]["nodes"], json!(1_u64), "GL_NODES rows");
    assert_eq!(read["gl"]["vertices"], built["gl_vertices"], "GL_VERT rows");
}

#[tokio::test]
async fn the_gl_block_sits_after_the_map_and_survives_rebuilds_and_undo() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    let names = directory_names(&client, "working").await;
    let map_at = names.iter().position(|n| n == "MAP01").expect("the block");
    let gl_at = names
        .iter()
        .position(|n| n == "GL_MAP01")
        .expect("the marker");
    let members = map_at
        .checked_add(11)
        .expect("ten members after the header");
    assert_eq!(gl_at, members, "the run sits right after the map block");
    let end = gl_at.checked_add(6).expect("six run lumps");
    assert_eq!(
        names[gl_at..end],
        [
            "GL_MAP01", "GL_VERT", "GL_SEGS", "GL_SSECT", "GL_NODES", "GL_PVS"
        ],
        "the run in glBSP order"
    );
    let again = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    assert_eq!(again["copied_to_working"], json!(false), "already editable");
    assert_eq!(
        directory_names(&client, "working").await,
        names,
        "a rebuild replaces, never duplicates"
    );
    call(&client, "undo", json!({})).await;
    assert_eq!(
        directory_names(&client, "working").await,
        names,
        "the first build stands"
    );
    call(&client, "undo", json!({})).await;
    let gone = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(
        gone["gl"]["present"],
        json!(false),
        "undo took the block away"
    );
    assert_eq!(gone["format"], json!("none"), "and the classic run with it");
}

#[tokio::test]
async fn gl_minisegs_close_the_uncovered_partition_of_an_l_room() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = l_room_loaded(dir.path()).await;
    let built = call(
        &client,
        "build_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    assert_eq!(built["segs"], json!(7_u64), "the classic count");
    assert_eq!(
        built["gl_segs"],
        json!(9_u64),
        "seven real segs plus two minisegs"
    );
    assert_eq!(
        built["minisegs"],
        json!(2_u64),
        "one per side of the doorway plane"
    );
    assert_eq!(
        built["gl_vertices"],
        json!(0_u64),
        "the classic run's split vertex is reused"
    );
    let read = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP01", "scope": "working" }),
    )
    .await;
    assert_eq!(
        read["gl"]["segs"],
        json!(9_u64),
        "the reader counts them back"
    );
    assert_eq!(read["gl"]["subsectors"], json!(2_u64), "two closed leaves");
}

#[tokio::test]
async fn a_built_map_survives_property_edits_and_reports_itself_undoable() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(&client, "build_nodes", json!({ "map": "MAP01" })).await;
    let lighted = call(
        &client,
        "edit_sector",
        json!({ "map": "MAP01", "index": 0_i64, "light": 200_i64 }),
    )
    .await;
    assert_eq!(
        lighted["nodes_invalidated"],
        json!(false),
        "a property edit keeps the fresh tree"
    );
    let kept = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(kept["format"], json!("vanilla"), "still vanilla records");
    assert_eq!(kept["segs"], json!(8_u64), "with every seg");
    call(&client, "undo", json!({})).await;
    let undone = call(&client, "undo", json!({})).await;
    let gone = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(
        gone["format"],
        json!("none"),
        "undoing the build erases the run"
    );
    assert_eq!(undone["redo_available"], json!(2_u64), "two steps to redo");
}
