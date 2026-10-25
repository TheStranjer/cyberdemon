//! `add_lump`: the text, hex, source and marker payloads, placement by index,
//! `before` and `after`, namespace placement creating a region once and
//! reusing it, the twin rule with `allow_duplicate` and the region it names,
//! the conflicting-field and malformed-argument refusals that leave the
//! archive untouched, the missing source file reported as not found and a flat
//! joining the markers of an opened working archive.

use std::fs;

use serde_json::{Value, json};

use crate::fixtures::pwad;
use crate::lumps::{
    assert_untouched, names, path_arg, row_named, seed_three, source_arg, undo_labels,
    working_names,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn add_lump_text_appends_and_reports_the_new_row() {
    let client = connect().await;
    let report = call(
        &client,
        "add_lump",
        json!({ "name": "MYTEXT", "text": "hello world" }),
    )
    .await;
    assert_eq!(report["name"], json!("MYTEXT"), "echoed name");
    assert_eq!(report["index"], json!(0_u64), "appended at 0");
    assert_eq!(report["size"], json!(11_u64), "the text's bytes");
    assert_eq!(
        report["namespace"],
        Value::Null,
        "a loose lump has no region"
    );
    assert!(
        report.get("source").is_none(),
        "add_lump echoes no source: {report}"
    );
    let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(names(&listing), vec!["MYTEXT"], "the working archive grew");
    assert_eq!(row_named(&listing, "MYTEXT")["kind"], json!("text"));
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "mytext", "scope": "working", "format": "text" }),
    )
    .await;
    assert_eq!(read["text"], json!("hello world"), "written as-is");
    let info = call(&client, "get_session_info", Value::Null).await;
    assert_eq!(
        info["working"]["dirty"],
        json!(true),
        "the session is dirty"
    );
    assert_eq!(
        info["undo"],
        json!(["add_lump MYTEXT"]),
        "the undo label names the tool and the lump"
    );
}

#[tokio::test]
async fn add_lump_hex_source_and_empty_form_the_payloads() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let hexed = call(
        &client,
        "add_lump",
        json!({ "name": "MUSHEAD", "hex": "4D 55 53 1A" }),
    )
    .await;
    assert_eq!(hexed["size"], json!(4_u64), "four hex pairs");
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "MUSHEAD", "scope": "working", "format": "hex" }),
    )
    .await;
    assert_eq!(read["hex"], json!("4D 55 53 1A"), "the decoded bytes");
    let payload = dir.path().join("payload.bin");
    fs::write(&payload, [0xDE_u8, 0xAD, 0xBE, 0xEF]).expect("write the source file");
    let mut args = source_arg(&payload);
    args["name"] = json!("PAYLOAD");
    let fetched = call(&client, "add_lump", args).await;
    assert_eq!(fetched["index"], json!(1_u64), "appended second");
    assert_eq!(fetched["size"], json!(4_u64), "the file's bytes");
    let marker = call(&client, "add_lump", json!({ "name": "MYMARKER" })).await;
    assert_eq!(marker["index"], json!(2_u64), "appended third");
    assert_eq!(marker["size"], json!(0_u64), "no data field is a marker");
    assert_eq!(
        working_names(&client).await,
        vec!["MUSHEAD", "PAYLOAD", "MYMARKER"],
        "directory order after plain appends"
    );
}

#[tokio::test]
async fn add_lump_places_by_index_before_and_after() {
    let client = connect().await;
    seed_three(&client).await;
    let spliced = call(
        &client,
        "add_lump",
        json!({ "name": "X", "text": "x", "index": 1_u64 }),
    )
    .await;
    assert_eq!(spliced["index"], json!(1_u64), "reported position");
    let before = call(
        &client,
        "add_lump",
        json!({ "name": "Y", "text": "y", "before": "TWO" }),
    )
    .await;
    assert_eq!(before["index"], json!(2_u64), "just before TWO");
    let after = call(
        &client,
        "add_lump",
        json!({ "name": "Z", "text": "z", "after": "ONE" }),
    )
    .await;
    assert_eq!(after["index"], json!(1_u64), "just after ONE");
    assert_eq!(
        working_names(&client).await,
        vec!["ONE", "Z", "X", "Y", "TWO", "THREE"],
        "every anchor honoured in order"
    );
}

#[tokio::test]
async fn add_lump_namespace_placement_creates_and_reuses_a_region() {
    let client = connect().await;
    let flats = call(
        &client,
        "add_lump",
        json!({ "name": "FLATA", "text": "f", "namespace": "flats" }),
    )
    .await;
    assert_eq!(flats["index"], json!(1_u64), "between the created markers");
    assert_eq!(flats["namespace"], json!("flats"), "the new region");
    let second = call(
        &client,
        "add_lump",
        json!({ "name": "FLATB", "text": "g", "namespace": "flats" }),
    )
    .await;
    assert_eq!(second["index"], json!(2_u64), "before the existing F_END");
    let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        names(&listing),
        vec!["F_START", "FLATA", "FLATB", "F_END"],
        "the region was created once and reused"
    );
    assert_eq!(
        row_named(&listing, "FLATB")["kind"],
        json!("flat"),
        "classified by its region"
    );
    assert_eq!(
        row_named(&listing, "F_START")["namespace"],
        json!("flats"),
        "the markers carry the region too"
    );
}

#[tokio::test]
async fn add_lump_refuses_twins_but_namespaces_and_allow_duplicate_let_them_through() {
    let client = connect().await;
    call(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "one" }),
    )
    .await;
    let (code, message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "two" }),
    )
    .await;
    assert_eq!(code, "already_exists", "same name, same (global) area");
    assert!(
        message.contains("allow_duplicate") && message.contains("replace_lump"),
        "the message names both exits: {message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["TWIN"],
        "the refused add stored nothing"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump TWIN"],
        "and left no history"
    );
    let forced = call(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "two", "allow_duplicate": true }),
    )
    .await;
    assert_eq!(forced["index"], json!(1_u64), "the twin is appended");
    let region = call(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "three", "namespace": "sprites" }),
    )
    .await;
    assert_eq!(
        region["namespace"],
        json!("sprites"),
        "a region twin is no global clash"
    );
    let (again_code, again_message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "four", "namespace": "sprites" }),
    )
    .await;
    assert_eq!(
        again_code, "already_exists",
        "now the sprites region has a twin"
    );
    assert!(
        again_message.contains("the sprites region"),
        "the message names the region: {again_message}"
    );
}

#[tokio::test]
async fn add_lump_rejects_conflicting_fields_without_editing() {
    let client = connect().await;
    let (payload_code, payload_message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "A", "text": "t", "hex": "4D55" }),
    )
    .await;
    assert_eq!(payload_code, "invalid_argument", "two payload fields");
    assert!(
        payload_message.contains("at most one"),
        "the message states the rule: {payload_message}"
    );
    let (text_source_code, _, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "A", "text": "t", "source": "/x/y.bin" }),
    )
    .await;
    assert_eq!(text_source_code, "invalid_argument", "text plus source");
    let (place_code, place_message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "A", "index": 1_u64, "before": "B" }),
    )
    .await;
    assert_eq!(place_code, "invalid_argument", "two placement fields");
    assert!(
        place_message.contains("at most one"),
        "the message states the rule: {place_message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn add_lump_rejects_malformed_arguments_without_editing() {
    let client = connect().await;
    let (hex_code, hex_message, _) =
        call_err(&client, "add_lump", json!({ "name": "A", "hex": "4Z" })).await;
    assert_eq!(hex_code, "invalid_argument", "not a hex pair");
    assert!(
        hex_message.starts_with("hex "),
        "the error names the hex field: {hex_message}"
    );
    let (odd_code, _, _) =
        call_err(&client, "add_lump", json!({ "name": "A", "hex": "4D5" })).await;
    assert_eq!(odd_code, "invalid_argument", "an odd digit count");
    let (index_code, index_message, _) =
        call_err(&client, "add_lump", json!({ "name": "A", "index": 99_u64 })).await;
    assert_eq!(
        index_code, "invalid_argument",
        "index past the empty directory"
    );
    assert!(
        index_message.contains("past the end"),
        "the message says where the index may go: {index_message}"
    );
    let (anchor_code, _, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "A", "before": "GHOST" }),
    )
    .await;
    assert_eq!(anchor_code, "not_found", "no anchor lump to place by");
    let (name_code, _, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "TOO LONG FOR WAD", "text": "t" }),
    )
    .await;
    assert_eq!(
        name_code, "invalid_argument",
        "the name is over eight bytes"
    );
    let (region_code, _, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "A", "text": "t", "namespace": "nope" }),
    )
    .await;
    assert_eq!(region_code, "invalid_argument", "not a namespace");
    assert_untouched(&client).await;
}

#[tokio::test]
async fn add_lump_passes_a_missing_source_file_through_as_not_found() {
    let client = connect().await;
    let (code, message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "GONE", "source": "/definitely-not-here/cyberdemon.bin" }),
    )
    .await;
    assert_eq!(code, "not_found", "the fetch reports the missing file");
    assert!(
        message.contains("/definitely-not-here"),
        "the message names the path: {message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn namespace_placement_joins_the_markers_of_an_opened_working_archive() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let extra = pwad(dir.path());
    call(&client, "open_wad", path_arg(&extra)).await;
    let report = call(
        &client,
        "add_lump",
        json!({ "name": "FLOOR2", "hex": "00", "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        report["index"],
        json!(2_u64),
        "just before the FF_END alias"
    );
    assert_eq!(
        report["namespace"],
        json!("flats"),
        "the alias region counts"
    );
    assert_eq!(
        working_names(&client).await,
        vec![
            "FF_START", "FLOOR1", "FLOOR2", "FF_END", "DEHACKED", "MAP02", "THINGS", "LINEDEFS",
            "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the flat joined the existing region without creating a new one"
    );
    let (code, message, _) = call_err(
        &client,
        "add_lump",
        json!({ "name": "FLOOR1", "hex": "01", "namespace": "flats" }),
    )
    .await;
    assert_eq!(code, "already_exists", "the region already holds that name");
    assert!(
        message.contains("the flats region"),
        "the message names the region: {message}"
    );
}
