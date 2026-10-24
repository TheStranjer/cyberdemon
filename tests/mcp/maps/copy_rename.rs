//! `copy_map`, `rename_map` and `remove_map`: the byte-for-byte copy of
//! the `IWAD` block read back through published lump assets, the copy and
//! removal as single undo steps, the `GL` marker renamed with its block and
//! dropped for a name that leaves no room for it, and the renames that move
//! working blocks, follow the active selection and refuse layer maps,
//! self renames and taken names.
//!
//! The faithful-copy `copy_map` call is `copy_to_map07` and the lump rows
//! as `(name, size, kind)` facts are `lump_facts`, both here; everything
//! else comes from [`crate::maps`].

use serde_json::{Value, json};

use crate::maps::{
    Client, DOOM_MEMBERS, formats_loaded, iwad_loaded, lump_bytes, lump_names, map_named,
};
use crate::support::{call, call_err};

/// The rows of a `get_map_lumps urls` report as `(name, size, kind)`
/// facts, so two reports of the same bytes compare directly.
fn lump_facts(report: &Value) -> Vec<(String, u64, String)> {
    report["lumps"]
        .as_array()
        .expect("lumps is an array")
        .iter()
        .map(|row| {
            (
                row["name"].as_str().expect("a name").to_owned(),
                row["size"].as_u64().expect("a size"),
                row["kind"].as_str().expect("a kind").to_owned(),
            )
        })
        .collect()
}

/// A `copy_map` call the faithful-copy tests share.
async fn copy_to_map07(client: &Client) -> Value {
    call(
        client,
        "copy_map",
        json!({ "from": "MAP01", "to": "map07" }),
    )
    .await
}

#[tokio::test]
async fn copy_map_copies_the_iwad_map_byte_for_byte() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let report = copy_to_map07(&client).await;
    assert_eq!(
        report,
        json!({"from": "MAP01", "to": "MAP07", "lumps": 11_u64}),
        "the documented copy result",
    );
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let row = map_named(&listing, "MAP07");
    assert_eq!(row["format"], json!("doom"), "the block arrived as doom");
    assert_eq!(row["lumps"], json!(DOOM_MEMBERS), "with every member");
    let source = call(
        &client,
        "get_map_lumps",
        json!({ "map": "MAP01", "scope": "layer:0", "urls": true }),
    )
    .await;
    let copy = call(
        &client,
        "get_map_lumps",
        json!({ "map": "MAP07", "scope": "working", "urls": true }),
    )
    .await;
    assert_eq!(lump_names(&source), lump_names(&copy), "the same lump set");
    assert_eq!(
        lump_facts(&source),
        lump_facts(&copy),
        "every row matches by name, size and kind",
    );
    assert_eq!(
        lump_bytes(&copy, "THINGS").await,
        lump_bytes(&source, "THINGS").await,
        "the THINGS bytes are identical",
    );
    assert_eq!(
        lump_bytes(&copy, "SIDEDEFS").await,
        lump_bytes(&source, "SIDEDEFS").await,
        "and the SIDEDEFS bytes are",
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(
        summary["counts"]["linedefs"],
        json!(7_u64),
        "the copy decodes whole"
    );
}

#[tokio::test]
async fn copy_map_undoes_as_one_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    copy_to_map07(&client).await;
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["copy_map MAP01 -> MAP07"]),
        "one undoable step",
    );
    let after = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(after["maps"], json!([]), "the copy rolled back wholly");
}

#[tokio::test]
async fn copy_map_renames_the_gl_marker_and_drops_it_for_long_names() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = formats_loaded(dir.path()).await;
    call(
        &client,
        "copy_map",
        json!({ "from": "MAP03", "to": "MAP08" }),
    )
    .await;
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let row = map_named(&listing, "MAP08");
    assert_eq!(row["has_gl_nodes"], json!(true), "the GL run came along");
    assert_eq!(
        row["lumps"],
        json!([
            "TEXTMAP", "ZNODES", "ENDMAP", "GL_MAP08", "GL_VERT", "GL_NODES"
        ]),
        "its marker follows the new name",
    );
    let (code, message, _) = call_err(
        &client,
        "copy_map",
        json!({ "from": "MAP03", "to": "MAP08" }),
    )
    .await;
    assert_eq!(code, "already_exists", "MAP08 is working now");
    assert!(message.contains("replace"), "the hint is there: {message}");
    call(
        &client,
        "copy_map",
        json!({ "from": "MAP03", "to": "MAP08", "replace": true }),
    )
    .await;
    let (missing_code, missing_message, _) = call_err(
        &client,
        "copy_map",
        json!({ "from": "MAP99", "to": "MAP09" }),
    )
    .await;
    assert_eq!(missing_code, "not_found", "no MAP99 in the merged view");
    assert!(
        missing_message.contains("MAP99"),
        "the message names it: {missing_message}",
    );
    let long = call(
        &client,
        "copy_map",
        json!({ "from": "MAP03", "to": "LONGNAME" }),
    )
    .await;
    assert_eq!(
        long["lumps"],
        json!(4_u64),
        "header plus three members, the GL run dropped",
    );
    let after = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        map_named(&after, "LONGNAME")["has_gl_nodes"],
        json!(false),
        "no marker for an eight-character name fits",
    );
}

#[tokio::test]
async fn rename_map_moves_working_blocks_and_follows_the_selection() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    call(
        &client,
        "copy_map",
        json!({ "from": "MAP01", "to": "MAP07" }),
    )
    .await;
    call(&client, "set_active_map", json!({ "map": "MAP07" })).await;
    let report = call(
        &client,
        "rename_map",
        json!({ "from": "MAP07", "to": "MAP08" }),
    )
    .await;
    assert_eq!(
        report,
        json!({"from": "MAP07", "to": "MAP08"}),
        "the documented rename result",
    );
    let session = call(&client, "get_session_info", Value::Null).await;
    assert_eq!(
        session["active_map"],
        json!("MAP08"),
        "the selection followed the map",
    );
    let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
    let maps = listing["maps"].as_array().expect("maps is an array");
    assert_eq!(maps.len(), 1, "the only working block is the renamed one");
    assert_eq!(
        maps.first().expect("one row")["name"],
        json!("MAP08"),
        "by name"
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["rename_map MAP07 -> MAP08"]),
        "the rename is one undoable step",
    );
    let after = call(
        &client,
        "get_map_summary",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(after["name"], json!("MAP07"), "undo restores the old name");
}

#[tokio::test]
async fn rename_map_refuses_layer_maps_self_renames_and_missing_names() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_loaded(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "rename_map",
        json!({ "from": "MAP01", "to": "MAP07" }),
    )
    .await;
    assert_eq!(code, "not_found", "MAP01 is only the IWAD's");
    assert!(
        message.contains("copy_map MAP01 MAP07"),
        "the message points at the tool that works: {message}",
    );
    call(
        &client,
        "copy_map",
        json!({ "from": "MAP01", "to": "MAP07" }),
    )
    .await;
    let (self_code, _, _) = call_err(
        &client,
        "rename_map",
        json!({ "from": "MAP07", "to": "MAP07" }),
    )
    .await;
    assert_eq!(self_code, "invalid_argument", "a self rename is a refusal");
    let (missing_code, _, _) = call_err(
        &client,
        "rename_map",
        json!({ "from": "MAP99", "to": "MAP08" }),
    )
    .await;
    assert_eq!(missing_code, "not_found", "MAP99 is not working");
    call(&client, "create_map", json!({ "name": "MAP08" })).await;
    let (taken_code, taken_message, _) = call_err(
        &client,
        "rename_map",
        json!({ "from": "MAP07", "to": "MAP08" }),
    )
    .await;
    assert_eq!(taken_code, "already_exists", "MAP08 is taken");
    assert!(
        taken_message.contains("remove_map"),
        "the message names the way out: {taken_message}",
    );
    let still = call(
        &client,
        "get_map_summary",
        json!({ "map": "MAP07", "scope": "working" }),
    )
    .await;
    assert_eq!(still["name"], json!("MAP07"), "the refusal renamed nothing");
}

#[tokio::test]
async fn rename_and_remove_map_track_the_gl_run_and_the_selection() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = formats_loaded(dir.path()).await;
    call(
        &client,
        "copy_map",
        json!({ "from": "MAP03", "to": "MAP09" }),
    )
    .await;
    call(&client, "set_active_map", json!({ "map": "MAP09" })).await;
    call(
        &client,
        "rename_map",
        json!({ "from": "MAP09", "to": "E1M8" }),
    )
    .await;
    let renamed = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        map_named(&renamed, "E1M8")["lumps"],
        json!([
            "TEXTMAP", "ZNODES", "ENDMAP", "GL_E1M8", "GL_VERT", "GL_NODES"
        ]),
        "the marker followed the new name too",
    );
    let removed = call(&client, "remove_map", json!({ "name": "E1M8" })).await;
    assert_eq!(
        removed,
        json!({"map": "E1M8", "lumps_removed": 7_u64}),
        "the block and its GL run went together",
    );
    let session = call(&client, "get_session_info", Value::Null).await;
    assert_eq!(
        session["active_map"],
        Value::Null,
        "and the selection was cleared",
    );
    let empty = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(empty["maps"], json!([]), "nothing is left in working");
    let (code, message, _) = call_err(&client, "remove_map", json!({ "name": "MAP03" })).await;
    assert_eq!(code, "not_found", "the layer's own MAP03 stays put");
    assert!(
        message.contains("read-only"),
        "the message explains why: {message}",
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["remove_map E1M8"]),
        "the removal is one undoable step",
    );
    let back = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        map_named(&back, "E1M8")["has_gl_nodes"],
        json!(true),
        "undo brings block and GL run back",
    );
}
