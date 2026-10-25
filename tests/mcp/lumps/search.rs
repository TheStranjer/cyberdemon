//! The `search_lumps` tool: name globs with the save-layout offset each row
//! carries, the kind and namespace criteria, the text and hex content needles
//! with the lowest match winning when both hit, the at-least-one-criterion and
//! bad-hex refusals, the truncation flag at the limit and the scope argument
//! reading one archive instead of the merged view.

use serde_json::{Value, json};

use crate::lumps::{
    COLORMAP_BYTES, HEADER_BYTES, MAP_BYTES, PLAYPAL_BYTES, iwad_plus_pwad, names, row_named,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn search_lumps_matches_name_globs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let flats = call(&client, "search_lumps", json!({ "pattern": "F*" })).await;
    assert_eq!(flats["scope"], json!("merged"), "the default scope");
    assert_eq!(flats["count"].as_u64(), Some(3), "the flats region");
    assert_eq!(flats["truncated"], json!(false), "well below the limit");
    assert_eq!(
        names(&flats),
        vec!["F_START", "FLOOR1", "F_END"],
        "glob order"
    );
    assert_eq!(
        row_named(&flats, "FLOOR1")["match_offset"],
        Value::Null,
        "no content criterion means no offset",
    );
    let one = call(&client, "search_lumps", json!({ "pattern": "?LOOR1" })).await;
    assert_eq!(
        names(&one),
        vec!["FLOOR1"],
        "question matches one character"
    );
    assert_eq!(
        row_named(&one, "FLOOR1")["offset"].as_u64(),
        Some(HEADER_BYTES),
        "the row shape carries the save-layout offset",
    );
}

#[tokio::test]
async fn search_lumps_filters_by_kind_and_namespace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let palettes = call(&client, "search_lumps", json!({ "kind": "palette" })).await;
    assert_eq!(names(&palettes), vec!["PLAYPAL"], "the palette lump");
    let flats = call(&client, "search_lumps", json!({ "namespace": "flats" })).await;
    assert_eq!(
        names(&flats),
        vec!["F_START", "FLOOR1", "PNGFLAT", "F_END"],
        "everything inside the flats region",
    );
}

#[tokio::test]
async fn search_lumps_finds_text_and_reports_the_offset() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let hits = call(&client, "search_lumps", json!({ "contains_text": "troop" })).await;
    assert_eq!(
        names(&hits),
        vec!["DEHACKED"],
        "case-insensitive ASCII match"
    );
    assert_eq!(
        row_named(&hits, "DEHACKED")["match_offset"].as_u64(),
        Some(9),
        "\"Thing 1 (Troop)\" has Troop at byte 9",
    );
    assert_eq!(
        row_named(&hits, "DEHACKED")["source"],
        json!({ "layer": 1_u64 }),
        "found through the merged view",
    );
}

#[tokio::test]
async fn search_lumps_finds_hex_bytes_with_and_without_spaces() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let spaced = call(&client, "search_lumps", json!({ "contains_hex": "28 54" })).await;
    assert_eq!(
        names(&spaced),
        vec!["DEHACKED"],
        "\"(T\" lives in the DeHackEd text"
    );
    assert_eq!(
        row_named(&spaced, "DEHACKED")["match_offset"].as_u64(),
        Some(8),
        "first occurrence offset",
    );
    let plain = call(
        &client,
        "search_lumps",
        json!({ "contains_hex": "5468696e67" }),
    )
    .await;
    assert_eq!(
        names(&plain),
        vec!["DEHACKED"],
        "lowercase digits, no spaces"
    );
    assert_eq!(
        row_named(&plain, "DEHACKED")["match_offset"].as_u64(),
        Some(0),
        "the payload starts with \"Thing\"",
    );
    let both = call(
        &client,
        "search_lumps",
        json!({ "contains_text": "troop", "contains_hex": "28 54" }),
    )
    .await;
    assert_eq!(
        row_named(&both, "DEHACKED")["match_offset"].as_u64(),
        Some(8),
        "the lowest content match wins when both needles hit",
    );
}

#[tokio::test]
async fn search_lumps_needs_at_least_one_criterion() {
    let client = connect().await;
    let (code, message, _) = call_err(&client, "search_lumps", json!({})).await;
    assert_eq!(code, "invalid_argument", "no criteria code");
    assert!(
        message.contains("pattern") && message.contains("contains_hex"),
        "the message names the criteria: {message}"
    );
}

#[tokio::test]
async fn search_lumps_rejects_bad_hex() {
    let client = connect().await;
    let (odd_code, odd_message, _) =
        call_err(&client, "search_lumps", json!({ "contains_hex": "4D5" })).await;
    assert_eq!(odd_code, "invalid_argument", "odd digit count");
    assert!(
        odd_message.contains("odd"),
        "the message says odd: {odd_message}"
    );
    let (digit_code, digit_message, _) =
        call_err(&client, "search_lumps", json!({ "contains_hex": "0G" })).await;
    assert_eq!(digit_code, "invalid_argument", "non-hex digit");
    assert!(
        digit_message.contains("'G'"),
        "the message names the digit: {digit_message}"
    );
}

#[tokio::test]
async fn search_lumps_flags_truncation_at_the_limit() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let page = call(
        &client,
        "search_lumps",
        json!({ "pattern": "*", "limit": 2_u64 }),
    )
    .await;
    assert_eq!(page["count"].as_u64(), Some(2), "capped at the limit");
    assert_eq!(page["truncated"], json!(true), "more matched than returned");
    assert_eq!(names(&page).len(), 2, "two rows");
    let small = call(
        &client,
        "search_lumps",
        json!({ "pattern": "?LOOR1", "limit": 2_u64 }),
    )
    .await;
    assert_eq!(small["truncated"], json!(false), "one match, not truncated");
    assert_eq!(small["count"].as_u64(), Some(1), "one row");
}

#[tokio::test]
async fn search_lumps_honours_the_scope_argument() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let base = call(
        &client,
        "search_lumps",
        json!({ "pattern": "FLOOR1", "scope": "layer:0" }),
    )
    .await;
    assert_eq!(
        base["scope"],
        json!({ "layer": 0_u64 }),
        "echoed layer scope"
    );
    assert_eq!(names(&base), vec!["FLOOR1"], "the IWAD's copy");
    assert_eq!(
        row_named(&base, "FLOOR1")["source"],
        json!({ "layer": 0_u64 }),
        "reported from the layer, not the winner",
    );
    assert_eq!(
        row_named(&base, "FLOOR1")["offset"].as_u64(),
        Some(
            HEADER_BYTES
                .checked_add(PLAYPAL_BYTES)
                .and_then(|sum| sum.checked_add(COLORMAP_BYTES))
                .and_then(|sum| sum.checked_add(MAP_BYTES))
                .expect("small sums")
        ),
        "the IWAD's own save layout",
    );
    let none = call(
        &client,
        "search_lumps",
        json!({ "pattern": "*", "scope": "working" }),
    )
    .await;
    assert_eq!(
        none["count"].as_u64(),
        Some(0),
        "the working archive is empty"
    );
    let (code, _, _) = call_err(
        &client,
        "search_lumps",
        json!({ "pattern": "FLOOR1", "scope": "layer:9" }),
    )
    .await;
    assert_eq!(code, "not_found", "out of range layer");
}
