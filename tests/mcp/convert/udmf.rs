//! Same-family format rewrites of one map block.
//!
//! A binary `Doom` block becoming `UDMF` and coming back with its ten vanilla
//! members and its counts, the `Hexen` fixture map going to `UDMF` `zdoom`
//! and home with its `BEHAVIOR` intact, a `UDMF` block losing its `ZNODES`
//! and `GL` run, what a lossy `UDMF` map reports when it becomes binary,
//! how an edit keeps its carried global and unknown block, and
//! `Line_SetIdentification` travelling through the `UDMF` line id.

use serde_json::{Value, json};

use crate::convert::{
    DOOM_MEMBERS, LOSSY_COUNTS, LOSSY_DROPS, LOSSY_ROUNDED, NO_DROPS, NODES_HINT, TWO_ROOM_COUNTS,
    convert, counts, first_linedef, loaded, lump_names, map_row, row_of, warns,
};
use crate::fixtures::{iwad, pwad_conversions, pwad_map_formats};
use crate::support::call;

#[tokio::test]
async fn converting_map01_to_udmf_rewrites_the_block() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    let out = convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
    assert_eq!(out["from"], "doom", "read as binary Doom");
    assert_eq!(out["to"], "udmf", "written as UDMF");
    assert_eq!(
        out["namespace"], "doom",
        "a Doom family source defaults to the doom namespace"
    );
    assert_eq!(out["nodes_removed"], true, "the node data went");
    assert_eq!(out["dry_run"], false, "a real run says so");
    assert_eq!(out["dropped_fields"], row_of(NO_DROPS), "nothing was lost");
    assert_eq!(
        out["fractional_coordinates"],
        row_of(NO_DROPS),
        "the binary source had no fractions"
    );
    assert_eq!(out["translated_specials"], 0_i32, "a same-family step");
    assert_eq!(out["untranslated"], json!([]), "translates nothing");
    assert!(
        warns(&out, NODES_HINT),
        "the result suggests rebuilding nodes: {out}"
    );
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(row["format"], "udmf", "list_maps follows the format");
    assert_eq!(row["udmf_namespace"], "doom", "and the namespace");
    assert_eq!(row["has_nodes"], false, "no node data survived");
    assert_eq!(
        lump_names(&client, "MAP01", "merged").await,
        vec!["TEXTMAP", "ENDMAP"],
        "the block is now TEXTMAP plus ENDMAP"
    );
    assert_eq!(
        counts(&client, "MAP01", "merged").await,
        row_of(TWO_ROOM_COUNTS),
        "the element counts are unchanged"
    );
}

#[tokio::test]
async fn converting_a_udmf_block_back_to_doom_restores_the_members() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
    let back = convert(&client, json!({ "map": "MAP01", "to": "doom" })).await;
    assert_eq!(back["from"], "udmf", "read back as UDMF");
    assert_eq!(back["to"], "doom", "written as binary Doom");
    assert_eq!(
        back["namespace"],
        Value::Null,
        "a binary layout declares no namespace"
    );
    assert_eq!(
        back["dropped_fields"],
        row_of(NO_DROPS),
        "the round trip had nothing unmodelled to drop"
    );
    assert_eq!(
        lump_names(&client, "MAP01", "merged").await,
        DOOM_MEMBERS.to_vec(),
        "the ten vanilla members are back"
    );
    assert_eq!(
        counts(&client, "MAP01", "merged").await,
        row_of(TWO_ROOM_COUNTS),
        "and the counts still match"
    );
}

#[tokio::test]
async fn converting_the_hexen_fixture_keeps_behavior_and_comes_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let out = convert(&client, json!({ "map": "MAP04", "to": "udmf" })).await;
    assert_eq!(out["from"], "hexen", "read as binary Hexen");
    assert_eq!(
        out["namespace"], "zdoom",
        "a Hexen family source defaults to zdoom"
    );
    assert_eq!(
        lump_names(&client, "MAP04", "working").await,
        vec!["TEXTMAP", "BEHAVIOR", "ENDMAP"],
        "the script lump rides along, the node lumps do not"
    );
    assert_eq!(
        counts(&client, "MAP04", "working").await,
        row_of(TWO_ROOM_COUNTS),
        "the elements came through"
    );
    let back = convert(&client, json!({ "map": "MAP04", "to": "hexen" })).await;
    assert_eq!(back["to"], "hexen", "and went back");
    let row = map_row(&client, "MAP04", "working").await;
    assert_eq!(row["format"], "hexen", "the block reads as Hexen again");
    assert_eq!(
        row["has_nodes"], false,
        "the empty NODES placeholder is no node data"
    );
    let names = lump_names(&client, "MAP04", "working").await;
    assert!(
        names.contains(&"BEHAVIOR".to_owned()),
        "BEHAVIOR survived: {names:?}"
    );
    assert!(
        names.contains(&"BLOCKMAP".to_owned()),
        "the vanilla members are back: {names:?}"
    );
    assert_eq!(
        counts(&client, "MAP04", "working").await,
        row_of(TWO_ROOM_COUNTS),
        "with the same counts"
    );
}

#[tokio::test]
async fn a_udmf_conversion_drops_znodes_and_the_gl_run() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let before = map_row(&client, "MAP03", "merged").await;
    assert_eq!(before["format"], "udmf", "the fixture is UDMF");
    assert_eq!(before["has_gl_nodes"], true, "with an attached GL run");
    assert_eq!(before["has_nodes"], true, "and a ZNODES lump");
    let out = convert(&client, json!({ "map": "MAP03", "to": "hexen" })).await;
    assert_eq!(out["namespace"], Value::Null, "the target is binary");
    assert_eq!(out["from"], "udmf", "read as UDMF");
    let after = map_row(&client, "MAP03", "working").await;
    assert_eq!(after["format"], "hexen", "the block became binary Hexen");
    assert_eq!(after["has_gl_nodes"], false, "the GL run left the archive");
    assert_eq!(after["has_nodes"], false, "and so did the node data");
    let names = lump_names(&client, "MAP03", "working").await;
    assert!(
        !names
            .iter()
            .any(|lump| lump == "ZNODES" || lump.starts_with("GL_")),
        "no UDMF node carrier survived: {names:?}"
    );
    assert!(
        names.contains(&"BEHAVIOR".to_owned()),
        "the Hexen writer synthesized BEHAVIOR: {names:?}"
    );
    assert_eq!(
        counts(&client, "MAP03", "working").await,
        row_of(TWO_ROOM_COUNTS),
        "the counts are unchanged"
    );
}

#[tokio::test]
async fn a_lossy_udmf_map_reports_what_a_binary_record_cannot_store() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_conversions(dir.path())).await;
    assert_eq!(
        counts(&client, "MAP05", "merged").await,
        row_of(LOSSY_COUNTS),
        "the fixture decodes"
    );
    let out = convert(&client, json!({ "map": "MAP05", "to": "doom" })).await;
    assert_eq!(
        out["dropped_fields"],
        row_of(LOSSY_DROPS),
        "every unmodelled field and preserved fraction is counted"
    );
    assert_eq!(
        out["fractional_coordinates"],
        row_of(LOSSY_ROUNDED),
        "only a modelled number's text counts as rounded: `alpha` and `scalex_top` \
         are unmodelled fields that simply drop"
    );
    assert!(
        warns(&out, "BEHAVIOR"),
        "a BEHAVIOR lump cannot stay in a binary Doom block: {out}"
    );
    assert!(
        warns(&out, "ednum"),
        "the carried global cannot stay either: {out}"
    );
    assert!(
        warns(&out, "vertex_slope"),
        "and neither can the unknown block: {out}"
    );
    assert_eq!(
        counts(&client, "MAP05", "working").await,
        row_of(LOSSY_COUNTS),
        "the rounded integers kept the element counts"
    );
    let row = map_row(&client, "MAP05", "working").await;
    assert_eq!(row["format"], "doom", "and the block reads as binary Doom");
}

#[tokio::test]
async fn editing_a_lossy_udmf_map_keeps_globals_and_unknown_blocks() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_conversions(dir.path())).await;
    call(
        &client,
        "edit_thing",
        json!({"map": "MAP05", "index": 0_i32, "x": 40_i32, "z": 0_i32}),
    )
    .await;
    let lump = call(
        &client,
        "get_lump",
        json!({"name": "TEXTMAP", "scope": "working", "format": "text"}),
    )
    .await;
    let text = lump["text"].as_str().expect("the TEXTMAP reads as text");
    assert!(
        text.contains("ednum = 5;"),
        "the edit's rewrite kept the global: {text}"
    );
    assert!(
        text.contains("vertex_slope {") && text.contains("angle = 45;"),
        "the edit's rewrite kept the unknown block and its fields: {text}"
    );
    assert!(text.contains("x = 40.000;"), "and the thing moved: {text}");
    let out = convert(&client, json!({"map": "MAP05", "to": "doom"})).await;
    assert!(
        warns(&out, "ednum"),
        "the conversion names the dropped global: {out}"
    );
    assert!(warns(&out, "vertex_slope"), "and the dropped block: {out}");
}

#[tokio::test]
async fn a_hexen_line_identification_travels_through_the_line_id() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_conversions(dir.path())).await;
    let before = first_linedef(&client, "MAP06", "merged").await;
    assert_eq!(before["special"], 121_i32, "the fixture line is 121");
    assert_eq!(before["args"][0], 7_i32, "carrying the id in args[0]");
    convert(&client, json!({ "map": "MAP06", "to": "udmf" })).await;
    let as_id = first_linedef(&client, "MAP06", "working").await;
    assert_eq!(as_id["tag"], 7_i32, "the id moved into the UDMF line id");
    assert_eq!(as_id["special"], 0_i32, "and the special went");
    convert(&client, json!({ "map": "MAP06", "to": "hexen" })).await;
    let again = first_linedef(&client, "MAP06", "working").await;
    assert_eq!(again["special"], 121_i32, "coming home it is 121 again");
    assert_eq!(again["args"][0], 7_i32, "with the id back in args[0]");
    assert_eq!(again["tag"], 0_i32, "and no model tag");
}
