//! `build_all_nodes`, the one-shot tree-plus-reject-plus-blockmap build that
//! writes everything a binary map needs in a single undoable step: the three
//! products taking one `undo` back at once, the `gl` variant attaching the
//! `GL_*` block in that same step, `UDMF` maps getting `ZNODES` alone (and
//! refusing any named `reject_mode`, since a `UDMF` map carries no
//! `REJECT`) and the bad-argument and
//! barren-map refusals that write nothing and leave the history alone.
//!
//! `assert_all_nodes_result` is the plain fixture's result shape; every other
//! helper comes from [`crate::nodes`].

use serde_json::{Value, json};

use crate::fixtures::pwad_empty_map;
use crate::nodes::{directory_names, nodes_loaded, plain_iwad};
use crate::support::{call, call_err, connect};

/// The `build_all_nodes` result shape on the plain `MAP01` fixture.
fn assert_all_nodes_result(built: &Value) {
    assert_eq!(built["variant"], json!("classic"), "binary default");
    assert_eq!(
        built["nodes"]["variant"],
        json!("classic"),
        "the nodes echo"
    );
    assert_eq!(built["nodes"]["segs"], json!(8_u64), "the classic run");
    assert_eq!(
        built["nodes"]["subsectors"],
        json!(2_u64),
        "one leaf per room"
    );
    assert_eq!(
        built["nodes"]["nodes"],
        json!(1_u64),
        "the shared line cuts once"
    );
    assert_eq!(
        built["nodes"]["split_vertices"],
        json!(0_u64),
        "flush corners"
    );
    assert_eq!(built["reject"]["mode"], json!("zero"), "the default mode");
    assert_eq!(
        built["reject"]["bytes"],
        json!(1_u64),
        "two sectors need one byte"
    );
    assert_eq!(
        built["reject"]["rejected_pairs"],
        json!(0_u64),
        "nothing rejected"
    );
    assert_eq!(
        built["blockmap"]["columns"],
        json!(5_u64),
        "512 units span 5 cells"
    );
    assert_eq!(built["blockmap"]["rows"], json!(3_u64), "256 span 3");
    assert_eq!(
        built["blockmap"]["vanilla_safe"],
        json!(true),
        "a toy map is tiny"
    );
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
}

#[tokio::test]
async fn build_all_nodes_writes_tree_reject_and_blockmap_at_once() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(&client, "build_all_nodes", json!({ "map": "MAP01" })).await;
    assert_all_nodes_result(&built);
    let read = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(read["format"], json!("vanilla"), "the tree reads back");
    assert_eq!(
        read["segs"], built["nodes"]["segs"],
        "the reader agrees on segs"
    );
    let reject = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(
        reject["size_ok"],
        json!(true),
        "the matrix is the right size"
    );
    assert_eq!(reject["bytes"], json!(1_u64), "and freshly written");
    let blockmap = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
    assert!(
        blockmap["blocks"].as_u64().expect("a cell count") > 0,
        "the grid has blocks: {blockmap}"
    );
    assert_eq!(
        blockmap["columns"], built["blockmap"]["columns"],
        "and it agrees"
    );
}

#[tokio::test]
async fn build_all_nodes_undoes_as_one_step_back_to_the_empty_lumps() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(&client, "build_all_nodes", json!({ "map": "MAP01" })).await;
    assert_all_nodes_result(&built);
    let info = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        info["undo"],
        json!(["build all nodes"]),
        "exactly one new undo entry"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["build all nodes"]),
        "one step takes all three back"
    );
    let nodes = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(
        nodes["format"],
        json!("none"),
        "undo restored the empty nodes"
    );
    let empty_reject = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
    assert_eq!(
        empty_reject["bytes"],
        json!(0_u64),
        "the reject went back too"
    );
    let empty_map = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
    assert_eq!(empty_map["bytes"], json!(0_u64), "and the blockmap");
}

#[tokio::test]
async fn build_all_nodes_gl_variant_attaches_the_gl_block_in_the_same_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let built = call(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP01", "variant": "gl" }),
    )
    .await;
    assert_eq!(built["variant"], json!("gl"), "the echo");
    assert_eq!(
        built["nodes"]["segs"],
        json!(8_u64),
        "the classic run is written"
    );
    assert_eq!(
        built["nodes"]["gl_segs"],
        json!(8_u64),
        "every edge holds a real seg"
    );
    assert!(
        built["reject"].is_object(),
        "binary maps also get the reject"
    );
    assert!(built["blockmap"].is_object(), "and the blockmap");
    let read = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(
        read["gl"]["present"],
        json!(true),
        "the GL block is attached"
    );
    assert_eq!(read["gl"]["magic"], json!("gNd2"), "the v2 magic");
    let maps = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        maps["maps"][0]["has_gl_nodes"],
        json!(true),
        "list_maps sees the run"
    );
    let info = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        info["undo"],
        json!(["build all nodes"]),
        "tree, GL run, reject and grid: one step"
    );
    call(&client, "undo", json!({})).await;
    let names = directory_names(&client, "working").await;
    assert!(
        names.iter().all(|name| !name.starts_with("GL_")),
        "undo took the GL run with everything else: {names:?}"
    );
    let gone = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(gone["format"], json!("none"), "and the whole build");
}

#[tokio::test]
async fn build_all_nodes_on_udmf_writes_znodes_only() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let built = call(&client, "build_all_nodes", json!({ "map": "MAP07" })).await;
    assert_eq!(
        built["variant"],
        json!("extended"),
        "UDMF defaults to extended"
    );
    assert_eq!(
        built["nodes"]["format"],
        json!("ZNOD"),
        "compressed by default"
    );
    assert_eq!(
        built["reject"],
        Value::Null,
        "ZDoom builds the reject itself"
    );
    assert_eq!(built["blockmap"], Value::Null, "and the blockmap");
    let names = directory_names(&client, "working").await;
    assert!(names.contains(&"ZNODES".to_owned()), "ZNODES landed");
    assert!(
        !names.contains(&"BLOCKMAP".to_owned()),
        "no BLOCKMAP was added: {names:?}"
    );
    let read = call(
        &client,
        "get_nodes",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(read["format"], json!("znod"), "the fresh run reads back");
}

#[tokio::test]
async fn build_all_nodes_on_udmf_refuses_a_named_reject_mode() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = nodes_loaded(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP07", "reject_mode": "zero" }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "UDMF carries no REJECT");
    assert!(
        message.contains("UDMF maps carry no REJECT"),
        "the message names the rule: {message}"
    );
    assert!(
        message.contains("leave reject_mode out"),
        "the message says what to do: {message}"
    );
    let (forced_code, forced_message, _) = call_err(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP07", "variant": "classic", "reject_mode": "components" }),
    )
    .await;
    assert_eq!(
        forced_code, "invalid_argument",
        "a mode is refused with any variant"
    );
    assert!(
        forced_message.contains("UDMF maps carry no REJECT"),
        "the shared rule: {forced_message}"
    );
    let (gl_code, gl_message, _) = call_err(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP07", "variant": "gl" }),
    )
    .await;
    assert_eq!(gl_code, "invalid_argument", "gl writes the classic run too");
    assert!(
        gl_message.contains("UDMF maps use extended nodes"),
        "the shared rule: {gl_message}"
    );
    let info = call(&client, "get_session_info", json!({})).await;
    assert_eq!(info["undo"], json!([]), "no refusal touched the history");
    let names = directory_names(&client, "working").await;
    assert!(names.is_empty(), "and nothing was written: {names:?}");
    let built = call(&client, "build_all_nodes", json!({ "map": "MAP07" })).await;
    assert_eq!(built["variant"], json!("extended"), "the plain build runs");
    assert_eq!(built["reject"], Value::Null, "and still writes no matrix");
}

#[tokio::test]
async fn build_all_nodes_refuses_bad_arguments_and_barren_maps_without_writing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let (variant_code, variant_message, _) = call_err(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP01", "variant": "znodes" }),
    )
    .await;
    assert_eq!(variant_code, "invalid_argument", "not a variant");
    assert!(
        variant_message.contains("classic")
            && variant_message.contains("extended")
            && variant_message.contains("gl"),
        "the message names all three: {variant_message}"
    );
    let (mode_code, mode_message, _) = call_err(
        &client,
        "build_all_nodes",
        json!({ "map": "MAP01", "reject_mode": "visibility" }),
    )
    .await;
    assert_eq!(mode_code, "invalid_argument", "not a mode");
    assert!(
        mode_message.contains("zero") && mode_message.contains("components"),
        "the message names both values: {mode_message}"
    );
    let (missing_code, _, _) =
        call_err(&client, "build_all_nodes", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "no such block");
    let info = call(&client, "get_session_info", json!({})).await;
    assert_eq!(info["undo"], json!([]), "no refusal touched the history");
    let empty_dir = tempfile::tempdir().expect("a scratch directory");
    let empty_client = connect().await;
    let empty = pwad_empty_map(empty_dir.path());
    call(
        &empty_client,
        "open_iwad",
        json!({ "path": empty.display().to_string() }),
    )
    .await;
    let (code, message, _) =
        call_err(&empty_client, "build_all_nodes", json!({ "map": "MAP01" })).await;
    assert_eq!(code, "invalid_argument", "the builder's refusal");
    assert!(
        message.contains("sidedefs") || message.contains("vertices"),
        "the hint names the barren geometry: {message}"
    );
    let (working_code, _, _) = call_err(
        &empty_client,
        "get_map_summary",
        json!({ "map": "MAP01", "scope": "working" }),
    )
    .await;
    assert_eq!(
        working_code, "not_found",
        "the refusal wrote no partial changes"
    );
}
