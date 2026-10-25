//! `replace_lump`: new bytes over a working lump in place, the hex and
//! fetched-source payloads, occurrence and namespace selection among the
//! working copies, the override a read-only layer's lump gets inside its
//! region or its whole map block with the layer and the file on disk left
//! untouched, the copied block winning the merged view, the winning copy of
//! two layer flats and the missing-lump, payload-field, malformed-argument and
//! missing-source refusals. The merged-hex reader, the duplicated-flat
//! session, the fixture flat's bytes on disk, the `replace_lump` hex
//! arguments and the hex builder live here with them.

use std::fs;
use std::path::Path;

use cyberdemon::wad::Wad;
use serde_json::{Value, json};

use crate::fixtures::{iwad, iwad_duplicated};
use crate::lumps::{
    Client, FLAT_BYTES, add, add_twins, assert_untouched, iwad_only, names, path_arg, replace_hex,
    row_named, undo_labels, working_names,
};
use crate::support::{call, call_err, connect};

/// A session with the duplicated-flat `IWAD`: two `FLOOR1` copies in the
/// flats region, for the selection tests.
async fn iwad_with_twins(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_duplicated(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The merged view's `get_lump` hex window for `name`.
async fn merged_hex(client: &Client, name: &str, length: u64) -> String {
    let read = call(
        client,
        "get_lump",
        json!({ "name": name, "format": "hex", "length": length }),
    )
    .await;
    read["hex"].as_str().expect("a hex body").to_owned()
}

/// The bytes of the `IWAD` fixture's `FLOOR1`, read straight from disk.
fn iwad_flat_bytes(dir: &Path) -> Vec<u8> {
    let wad = Wad::read_file(&iwad(dir)).expect("the fixture reads");
    wad.lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"FLOOR1")
        .expect("the IWAD holds FLOOR1")
        .data
        .to_vec()
}

#[tokio::test]
async fn replace_lump_writes_new_text_over_a_working_lump_in_place() {
    let client = connect().await;
    add(&client, "NOTE", "hello").await;
    add(&client, "OTHER", "x").await;
    let report = call(
        &client,
        "replace_lump",
        json!({ "name": "note", "text": "hi there" }),
    )
    .await;
    assert_eq!(
        report,
        json!({ "name": "NOTE", "index": 0_u64, "size": 8_u64, "created_override": false }),
        "the documented result, written in place"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["NOTE", "OTHER"],
        "the directory did not grow"
    );
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "NOTE", "scope": "working", "format": "text" }),
    )
    .await;
    assert_eq!(read["text"], json!("hi there"), "the new bytes stand");
    assert_eq!(read["index"].as_u64(), Some(0), "same position");
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump NOTE", "add_lump OTHER", "replace_lump NOTE"],
        "the replace is its own undoable step"
    );
}

#[tokio::test]
async fn replace_lump_takes_its_payload_from_hex_or_a_fetched_source() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    add(&client, "NOTE", "hello").await;
    let hexed = call(&client, "replace_lump", replace_hex("NOTE", "4D 55 53 1A")).await;
    assert_eq!(hexed["size"], json!(4_u64), "four hex pairs");
    assert_eq!(hexed["index"], json!(0_u64), "in place again");
    assert_eq!(
        merged_hex(&client, "NOTE", 4).await,
        "4D 55 53 1A",
        "the decoded bytes are what the view shows"
    );
    let payload = dir.path().join("note.txt");
    fs::write(&payload, b"from a file").expect("write the source file");
    let fetched = call(
        &client,
        "replace_lump",
        json!({ "name": "NOTE", "source": payload.display().to_string() }),
    )
    .await;
    assert_eq!(fetched["size"], json!(11_u64), "the file's bytes");
    assert_eq!(
        merged_hex(&client, "NOTE", 4).await,
        hex_of(b"from"),
        "the fetched bytes landed"
    );
}

/// The space-separated uppercase hex of `data`.
fn hex_of(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
async fn replace_lump_picks_a_working_copy_by_occurrence() {
    let client = connect().await;
    add_twins(&client).await;
    let first = call(
        &client,
        "replace_lump",
        json!({ "name": "TWIN", "text": "picked", "occurrence": 0_u64 }),
    )
    .await;
    assert_eq!(first["index"], json!(0_u64), "the first copy");
    assert_eq!(
        first["created_override"],
        json!(false),
        "both copies are working lumps"
    );
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "TWIN", "scope": "working", "occurrence": 0_u64, "format": "text" }),
    )
    .await;
    assert_eq!(read["text"], json!("picked"), "the first copy changed");
    assert_eq!(
        merged_hex(&client, "TWIN", 6).await,
        hex_of(b"second"),
        "the winning (last) copy kept its bytes"
    );
}

#[tokio::test]
async fn replace_lump_refuses_an_occurrence_past_the_working_copies() {
    let client = connect().await;
    add_twins(&client).await;
    let (code, message, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "TWIN", "text": "t", "occurrence": 9_u64 }),
    )
    .await;
    assert_eq!(code, "not_found", "no ninth working copy");
    assert!(
        message.contains("only 2 lumps named TWIN"),
        "the message counts the working copies: {message}"
    );
    assert_eq!(
        merged_hex(&client, "TWIN", 6).await,
        hex_of(b"second"),
        "the refused call wrote nothing"
    );
}

#[tokio::test]
async fn replace_lump_needs_the_namespace_to_match_the_copy() {
    let client = connect().await;
    add_twins(&client).await;
    let (code, message, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "TWIN", "text": "t", "namespace": "flats" }),
    )
    .await;
    assert_eq!(code, "not_found", "no TWIN sits in a flats region");
    assert!(
        message.contains("flats region"),
        "the message names the region it looked in: {message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["TWIN", "TWIN"],
        "no region was created for the refusal"
    );
}

#[tokio::test]
async fn replace_lump_overrides_an_iwad_flat_inside_its_region() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let report = call(&client, "replace_lump", replace_hex("FLOOR1", "01 02 03")).await;
    assert_eq!(
        report,
        json!({ "name": "FLOOR1", "index": 1_u64, "size": 3_u64, "created_override": true }),
        "the override was created between new markers"
    );
    let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        names(&listing),
        vec!["F_START", "FLOOR1", "F_END"],
        "a flats region appeared in the working archive"
    );
    assert_eq!(
        row_named(&listing, "FLOOR1")["namespace"],
        json!("flats"),
        "the override sits in the region it came from"
    );
    assert_eq!(merged_hex(&client, "FLOOR1", 3).await, "01 02 03");
}

#[tokio::test]
async fn an_override_leaves_the_iwad_layer_and_its_file_untouched() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    call(&client, "replace_lump", replace_hex("FLOOR1", "01 02 03")).await;
    let layer = call(
        &client,
        "list_lumps",
        json!({ "scope": "layer:0", "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        row_named(&layer, "FLOOR1")["size"].as_u64(),
        Some(FLAT_BYTES),
        "the IWAD layer still holds its own flat"
    );
    assert_eq!(
        iwad_flat_bytes(dir.path()).len(),
        usize::try_from(FLAT_BYTES).expect("a small count"),
        "and the file on disk is unchanged"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["replace_lump FLOOR1"],
        "one undoable edit for the whole override"
    );
}

#[tokio::test]
async fn replace_lump_copies_the_whole_map_block_before_the_lump() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let report = call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
    assert!(
        report["created_override"]
            .as_bool()
            .expect("a created_override flag"),
        "map data from a layer is copied first"
    );
    assert_eq!(report["index"].as_u64(), Some(1), "the copied THINGS");
    assert_eq!(report["size"].as_u64(), Some(2), "the new payload");
    assert_eq!(
        working_names(&client).await,
        vec![
            "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
            "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the entire MAP01 block arrived, header first"
    );
}

#[tokio::test]
async fn a_copied_block_wins_the_merged_view_and_needs_copying_once() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
    let merged = call(&client, "list_lumps", json!({ "map": "MAP01" })).await;
    assert!(
        merged["lumps"]
            .as_array()
            .expect("an array")
            .iter()
            .all(|row| row["source"] == json!("working")),
        "the copied block is what an engine would see: {merged}"
    );
    let again = call(&client, "replace_lump", replace_hex("LINEDEFS", "CC")).await;
    assert_eq!(
        again["created_override"],
        json!(false),
        "the block is already in the working archive, so this is a plain overwrite"
    );
    assert_eq!(again["index"].as_u64(), Some(2), "the copied LINEDEFS");
    assert_eq!(
        undo_labels(&client).await,
        vec!["replace_lump THINGS", "replace_lump LINEDEFS"],
        "copy plus write is one step, then a plain overwrite"
    );
}

#[tokio::test]
async fn replace_lump_reports_a_lump_no_loaded_archive_holds() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (code, message, _) = call_err(&client, "replace_lump", replace_hex("NOPE", "00")).await;
    assert_eq!(code, "not_found", "no archive holds NOPE");
    assert!(
        message.contains("add_lump"),
        "the message names the tool that creates one: {message}"
    );
    let (region_code, region_message, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "FLOOR1", "hex": "00", "namespace": "sprites" }),
    )
    .await;
    assert_eq!(region_code, "not_found", "FLOOR1 is no sprite");
    assert!(
        region_message.contains("sprites region"),
        "the message names the region: {region_message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn replace_lump_needs_exactly_one_payload_field() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (none_code, none_message, _) =
        call_err(&client, "replace_lump", json!({ "name": "PLAYPAL" })).await;
    assert_eq!(none_code, "invalid_argument", "a replace needs a payload");
    assert!(
        none_message.contains("exactly one"),
        "the message states the rule: {none_message}"
    );
    let (two_code, two_message, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "PLAYPAL", "text": "t", "hex": "00" }),
    )
    .await;
    assert_eq!(two_code, "invalid_argument", "one payload field only");
    assert!(
        two_message.contains("exactly one"),
        "the message states the rule: {two_message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn replace_lump_rejects_malformed_arguments_without_editing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (hex_code, hex_message, _) =
        call_err(&client, "replace_lump", replace_hex("PLAYPAL", "4Z")).await;
    assert_eq!(hex_code, "invalid_argument", "not a hex pair");
    assert!(
        hex_message.starts_with("hex "),
        "the error names the hex field: {hex_message}"
    );
    let (name_code, _, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "TOO LONG FOR WAD", "text": "t" }),
    )
    .await;
    assert_eq!(
        name_code, "invalid_argument",
        "the name is over eight bytes"
    );
    let (region_code, _, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "PLAYPAL", "namespace": "nope", "text": "t" }),
    )
    .await;
    assert_eq!(region_code, "invalid_argument", "not a namespace");
    assert_untouched(&client).await;
}

#[tokio::test]
async fn replace_lump_passes_a_missing_source_file_through_as_not_found() {
    let client = connect().await;
    add(&client, "NOTE", "keep").await;
    let (code, message, _) = call_err(
        &client,
        "replace_lump",
        json!({ "name": "NOTE", "source": "/definitely-not-here/cyberdemon.bin" }),
    )
    .await;
    assert_eq!(code, "not_found", "the fetch reports the missing file");
    assert!(
        message.contains("/definitely-not-here"),
        "the message names the path: {message}"
    );
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "NOTE", "scope": "working", "format": "text" }),
    )
    .await;
    assert_eq!(read["text"], json!("keep"), "the write never started");
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump NOTE"],
        "and no history was recorded"
    );
}

#[tokio::test]
async fn replace_lump_overrides_the_winning_copy_of_two_layer_flats() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_with_twins(dir.path()).await;
    let found = call(
        &client,
        "search_lumps",
        json!({ "pattern": "FLOOR1", "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        found["count"].as_u64(),
        Some(2),
        "the IWAD fixture holds two FLOOR1 copies"
    );
    let report = call(&client, "replace_lump", replace_hex("FLOOR1", "EE")).await;
    assert!(
        report["created_override"]
            .as_bool()
            .expect("a created_override flag"),
        "the override was created"
    );
    let flats = call(&client, "list_lumps", json!({ "namespace": "flats" })).await;
    let copies: Vec<&Value> = flats["lumps"]
        .as_array()
        .expect("an array")
        .iter()
        .filter(|row| row["name"] == json!("FLOOR1"))
        .collect();
    assert_eq!(copies.len(), 2, "the region still shows two copies");
    assert_eq!(
        copies.last().expect("the winning copy")["source"],
        json!("working"),
        "the override replaced the copy that wins, in place"
    );
    assert_eq!(
        copies.first().expect("the losing copy")["size"].as_u64(),
        Some(FLAT_BYTES),
        "the losing IWAD copy is untouched in the view"
    );
    assert_eq!(merged_hex(&client, "FLOOR1", 1).await, "EE");
}
