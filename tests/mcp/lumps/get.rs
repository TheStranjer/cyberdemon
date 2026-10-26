//! The read tools: `get_lump`'s `info`, `text`, `hex` and `url` formats and
//! the `auto` choice by kind, its occurrence, namespace and scope selection,
//! the published asset's served bytes and the missing-lump, bad-name and
//! bad-format refusals, then `extract_lump` writing a lump to an absolute
//! path, its overwrite rule and the relative-path, missing-parent and
//! missing-lump refusals. The fixture `PLAYPAL` reader and the `DEHACKED`
//! extract arguments live here with them.

use std::fs;
use std::path::Path;

use cyberdemon::wad::Wad;
use serde_json::{Value, json};

use crate::fixtures::{iwad, iwad_duplicated};
use crate::lumps::{PLAYPAL_BYTES, hex_head, iwad_plus_pwad, path_arg};
use crate::support::{call, call_err, connect, fetch_asset};

/// The `PLAYPAL` payload of the `IWAD` fixture, read from disk.
fn playpal_bytes(dir: &Path) -> Vec<u8> {
    let path = iwad(dir);
    let wad = Wad::read_file(&path).expect("the fixture reads");
    wad.lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"PLAYPAL")
        .expect("the IWAD holds PLAYPAL")
        .data
        .to_vec()
}

#[tokio::test]
async fn get_lump_info_reports_the_row_and_nothing_more() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "info" }),
    )
    .await;
    assert_eq!(report["format"], json!("info"), "the applied format");
    assert_eq!(report["name"], json!("DEHACKED"), "row name");
    assert_eq!(report["size"], json!(16_u64), "the DeHackEd text");
    assert_eq!(report["kind"], json!("dehacked"), "row kind");
    assert_eq!(report["namespace"], Value::Null, "a global has no region");
    assert_eq!(
        report["source"],
        json!({ "layer": 1_u64 }),
        "the merged row shape carries the source",
    );
    assert!(report.get("index").is_some(), "and the scope index");
    assert!(report.get("text").is_none(), "info adds nothing");
    assert!(report.get("hex").is_none(), "info adds nothing");
    assert!(report.get("asset").is_none(), "info adds nothing");
}

#[tokio::test]
async fn get_lump_text_decodes_truncates_and_flags_lossiness() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "text" }),
    )
    .await;
    assert_eq!(report["format"], json!("text"), "the applied format");
    assert_eq!(report["text"], json!("Thing 1 (Troop)\n"), "clean text");
    assert_eq!(report["lossy"], json!(false), "valid UTF-8");
    assert_eq!(report["truncated"], json!(false), "under the cap");
    let cut = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "text", "max_chars": 5_u64 }),
    )
    .await;
    assert_eq!(cut["text"], json!("Thing"), "cut at the character cap");
    assert_eq!(cut["truncated"], json!(true), "the cap was hit");
    let lossy = call(
        &client,
        "get_lump",
        json!({ "name": "PLAYPAL", "format": "text", "max_chars": 8_u64 }),
    )
    .await;
    assert_eq!(lossy["lossy"], json!(true), "the palette is not UTF-8");
    assert_eq!(lossy["truncated"], json!(true), "10752 bytes over 8 chars");
    let text = lossy["text"].as_str().expect("a text body");
    assert!(
        text.chars().any(|ch| ch == '\u{FFFD}'),
        "invalid sequences are replaced: {text:?}"
    );
}

#[tokio::test]
async fn get_lump_hex_windows_the_payload() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "hex" }),
    )
    .await;
    assert_eq!(report["format"], json!("hex"), "the applied format");
    assert_eq!(
        report["hex"],
        json!("54 68 69 6E 67 20 31 20 28 54 72 6F 6F 70 29 0A"),
        "space-separated uppercase bytes",
    );
    assert_eq!(report["start"], json!(0_u64), "echoed window start");
    assert_eq!(report["length"], json!(16_u64), "the whole payload");
    let window = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "hex", "start": 6_u64, "length": 4_u64 }),
    )
    .await;
    assert_eq!(window["hex"], json!("31 20 28 54"), "\"1 (T\"");
    assert_eq!(window["start"], json!(6_u64), "echoed start");
    assert_eq!(window["length"], json!(4_u64), "echoed length");
    let clipped = call(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "hex", "start": 11_u64, "length": 4_096_u64 }),
    )
    .await;
    assert_eq!(
        clipped["hex"],
        json!("6F 6F 70 29 0A"),
        "clipped to the payload"
    );
    assert_eq!(clipped["length"], json!(5_u64), "the real window length");
    let (code, message, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "hex", "start": 100_u64 }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "start past the end");
    assert!(
        message.contains("past the end") && message.contains("16 bytes"),
        "the message shows the bounds: {message}"
    );
    let (cap_code, _, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "DEHACKED", "format": "hex", "length": 4_097_u64 }),
    )
    .await;
    assert_eq!(cap_code, "invalid_argument", "length over the cap");
}

#[tokio::test]
async fn get_lump_url_publishes_the_raw_lump_bytes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let expected = playpal_bytes(dir.path());
    let report = call(
        &client,
        "get_lump",
        json!({ "name": "PLAYPAL", "format": "url" }),
    )
    .await;
    assert_eq!(report["format"], json!("url"), "the applied format");
    assert_eq!(
        report["asset"]["filename"],
        json!("PLAYPAL.lmp"),
        "a palette has no dedicated extension",
    );
    assert_eq!(
        report["asset"]["mime"],
        json!("application/octet-stream"),
        "the media type follows the extension",
    );
    assert_eq!(report["asset"]["size"], json!(PLAYPAL_BYTES), "the size");
    assert!(
        report.get("preview_hex").is_none(),
        "an explicit url asks for no preview",
    );
    let url = report["asset"]["url"].as_str().expect("an asset url");
    assert!(
        url.starts_with("http://localhost:"),
        "a localhost origin: {url}"
    );
    let fetched = fetch_asset(url).await;
    assert_eq!(fetched, expected, "the served bytes are the lump verbatim");
}

#[tokio::test]
async fn get_lump_auto_picks_text_info_and_url_by_kind() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let texty = call(&client, "get_lump", json!({ "name": "DEHACKED" })).await;
    assert_eq!(
        texty["format"],
        json!("text"),
        "a dehacked lump reads as text"
    );
    assert_eq!(
        texty["text"],
        json!("Thing 1 (Troop)\n"),
        "the decoded body"
    );
    let marker = call(&client, "get_lump", json!({ "name": "F_START" })).await;
    assert_eq!(marker["format"], json!("info"), "an empty lump is info");
    assert_eq!(marker["kind"], json!("marker"), "the row kind");
    let expected = playpal_bytes(dir.path());
    let binary = call(&client, "get_lump", json!({ "name": "PLAYPAL" })).await;
    assert_eq!(
        binary["format"],
        json!("url"),
        "binary goes to the asset server"
    );
    let preview = binary["preview_hex"].as_str().expect("a preview");
    assert_eq!(preview, hex_head(&expected, 64), "the first 64 bytes");
    let url = binary["asset"]["url"].as_str().expect("an asset url");
    let fetched = fetch_asset(url).await;
    assert_eq!(
        fetched, expected,
        "the full PLAYPAL bytes match the fixture"
    );
}

#[tokio::test]
async fn get_lump_occurrence_numbers_the_matches_and_defaults_to_the_winner() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad_duplicated(dir.path());
    call(&client, "open_iwad", path_arg(&base)).await;
    let last = call(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64 }),
    )
    .await;
    assert_eq!(last["hex"], json!("03 03"), "the default is the last copy");
    assert_eq!(
        last["source"],
        json!({ "layer": 0_u64 }),
        "still the base layer's lump",
    );
    let first = call(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64, "occurrence": 0_u64 }),
    )
    .await;
    assert_eq!(
        first["hex"],
        json!("01 01"),
        "occurrence 0 is the first copy"
    );
    let flats = call(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "namespace": "flats", "format": "hex", "length": 2_u64 }),
    )
    .await;
    assert_eq!(flats["hex"], json!("03 03"), "the flats filter keeps both");
    assert_eq!(flats["namespace"], json!("flats"), "the row region");
    let (sparse_code, sparse_message, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "namespace": "sprites" }),
    )
    .await;
    assert_eq!(sparse_code, "not_found", "no floor in sprites");
    assert!(
        sparse_message.contains("not found in merged"),
        "the message names the scope: {sparse_message}"
    );
    let (code, message, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "occurrence": 2_u64 }),
    )
    .await;
    assert_eq!(code, "not_found", "only two copies exist");
    assert!(
        message.contains("only 2 lumps named FLOOR1") && message.contains("0 to 1"),
        "the message counts the matches: {message}"
    );
}

#[tokio::test]
async fn get_lump_scopes_read_the_same_name_out_of_each_archive() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let layer = call(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "scope": "layer:0", "format": "hex", "length": 2_u64 }),
    )
    .await;
    assert_eq!(layer["hex"], json!("01 01"), "the IWAD's flat");
    assert_eq!(layer["source"], json!({ "layer": 0_u64 }), "layer source");
    let merged = call(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64 }),
    )
    .await;
    assert_eq!(
        merged["hex"],
        json!("02 02"),
        "the PWAD's override won the merge"
    );
    assert_eq!(merged["source"], json!({ "layer": 1_u64 }), "merged source");
    let (code, _, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "FLOOR1", "scope": "layer:9" }),
    )
    .await;
    assert_eq!(code, "not_found", "layer 9 is not loaded");
}

#[tokio::test]
async fn get_lump_reports_missing_lumps_names_and_formats() {
    let client = connect().await;
    let (code, message, _) = call_err(&client, "get_lump", json!({ "name": "NOPE" })).await;
    assert_eq!(code, "not_found", "no such lump");
    assert_eq!(message, "lump NOPE not found in merged", "the message");
    let (name_code, _, _) = call_err(&client, "get_lump", json!({ "name": "TOO LONG NAME" })).await;
    assert_eq!(name_code, "invalid_argument", "not a lump name");
    let (format_code, format_message, _) = call_err(
        &client,
        "get_lump",
        json!({ "name": "NOPE", "format": "binary" }),
    )
    .await;
    assert_eq!(format_code, "invalid_argument", "not a format");
    assert!(
        format_message.contains("\"url\"") && format_message.contains("\"auto\""),
        "the hint lists the formats: {format_message}"
    );
}

/// The `{ "name": "DEHACKED", "path": ... }` arguments for `file`.
fn extract_args(file: &Path) -> Value {
    json!({ "name": "DEHACKED", "path": file.display().to_string() })
}

#[tokio::test]
async fn extract_lump_writes_the_bytes_and_replaces_only_when_told() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let target = dir.path().join("dehacked.txt");
    let report = call(&client, "extract_lump", extract_args(&target)).await;
    assert_eq!(
        report["path"],
        json!(target.display().to_string()),
        "echoed path"
    );
    assert_eq!(report["bytes"], json!(16_u64), "the payload size");
    assert_eq!(
        fs::read(&target).expect("the file landed"),
        b"Thing 1 (Troop)\n",
        "the raw bytes are on disk",
    );
    let (code, message, _) = call_err(&client, "extract_lump", extract_args(&target)).await;
    assert_eq!(code, "already_exists", "the target stands there");
    assert!(
        message.contains("overwrite true"),
        "the message names the fix: {message}"
    );
    let mut force = extract_args(&target);
    force["overwrite"] = json!(true);
    let again = call(&client, "extract_lump", force).await;
    assert_eq!(again["bytes"], json!(16_u64), "overwrite rewrote it");
    assert_eq!(
        fs::read(&target).expect("the file is still there"),
        b"Thing 1 (Troop)\n",
        "the rewritten file holds the lump bytes",
    );
}

#[tokio::test]
async fn two_concurrent_extracts_to_one_path_never_share_a_staging_file() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let target = dir.path().join("contested.bin");
    let playpal = json!({
        "name": "PLAYPAL",
        "path": target.display().to_string(),
        "overwrite": true,
    });
    let dehacked = json!({
        "name": "DEHACKED",
        "path": target.display().to_string(),
        "overwrite": true,
    });
    let (small, large) = tokio::join!(
        call(&client, "extract_lump", dehacked),
        call(&client, "extract_lump", playpal),
    );
    assert!(
        small["bytes"].is_number() && large["bytes"].is_number(),
        "both writers land: {small} {large}"
    );
    let on_disk = fs::read(&target).expect("the file reads back");
    assert!(
        on_disk == b"Thing 1 (Troop)\n".to_vec()
            || on_disk.len() == usize::try_from(PLAYPAL_BYTES).expect("test size fits"),
        "the target holds one lump whole, never a mix: {} bytes",
        on_disk.len()
    );
    let siblings: Vec<String> = fs::read_dir(dir.path())
        .expect("read the scratch directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(siblings.is_empty(), "no staging files remain: {siblings:?}");
}

#[tokio::test]
async fn extract_lump_refuses_relative_paths_missing_parents_and_lumps() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let relative = call_err(
        &client,
        "extract_lump",
        json!({ "name": "DEHACKED", "path": "dehacked.txt" }),
    )
    .await;
    assert_eq!(relative.0, "invalid_argument", "a relative path is refused");
    assert!(
        relative.1.contains("not absolute"),
        "the message says what to pass: {}",
        relative.1
    );
    let nowhere = dir.path().join("missing-dir").join("x.bin");
    let (parent_code, parent_message, _) =
        call_err(&client, "extract_lump", extract_args(&nowhere)).await;
    assert_eq!(parent_code, "not_found", "the parent directory is missing");
    assert!(
        parent_message.contains("parent directory"),
        "the message names the problem: {parent_message}"
    );
    let missing_lump = dir.path().join("y.bin");
    let lump_args = json!({ "name": "NOPE", "path": missing_lump.display().to_string() });
    let (lump_code, _, _) = call_err(&client, "extract_lump", lump_args).await;
    assert_eq!(lump_code, "not_found", "the lump does not exist");
    assert!(!missing_lump.exists(), "a refused extract writes nothing");
}
