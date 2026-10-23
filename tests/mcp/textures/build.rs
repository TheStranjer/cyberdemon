//! `build_texture`'s definitions, patch-name growth and conflict rules and
//! `remove_texture`'s stripping.
//!
//! A build appends its definition, adds a patch name to `PNAMES` when the
//! patch needs one, refuses a patch with no lump behind it until allowed,
//! conflicts with an existing texture before replacing it in place, creates
//! the `TEXTURE2` it is aimed at and notes non-power-of-two tiling. Removal
//! drops one definition and keeps `PNAMES`. Both copy the touched lump into
//! the working archive as one undoable edit, leave the `IWAD` file alone and
//! refuse every malformed argument without writing. The refusal reader, the
//! invalid-argument sweep and the saved-archive assertion used by these tests
//! live here; the assertion that the fixture `IWAD` on disk is untouched
//! comes from [`crate::textures::list`].

use std::path::Path;

use cyberdemon::wad::archive::Wad;
use cyberdemon::wad::textures::{parse_pnames, parse_texture_lump};
use serde_json::{Value, json};

use crate::fixtures::iwad;
use crate::support::{call, call_err};
use crate::textures::list::assert_iwad_texture1_count;
use crate::textures::{Client, iwad_client, listed, path_arg, undo_labels};

/// The `(code, message)` of a refused `build_texture` call.
async fn build_err(client: &Client, args: Value) -> (String, String) {
    let (code, message, _) = call_err(client, "build_texture", args).await;
    (code, message)
}

/// Asserts every given `build_texture` call fails with
/// `invalid_argument`.
async fn assert_builds_refused(client: &Client, cases: Vec<Value>) {
    for args in cases {
        let (code, _) = build_err(client, args.clone()).await;
        assert_eq!(code, "invalid_argument", "code for {args}");
    }
}

/// Asserts a saved archive carries the appended `DOORP1` name and a
/// `DOOR1` texture referring to it by the new index at the origin.
fn assert_saved_build(out: &Path) {
    let saved = Wad::read_file(out).expect("the save reads");
    let pnames = saved
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"PNAMES")
        .expect("PNAMES was saved");
    assert_eq!(
        parse_pnames(&pnames.data).expect("it decodes"),
        vec![
            "WALLP1".to_owned(),
            "STEPP1".to_owned(),
            "DOORP1".to_owned()
        ],
        "the name was appended"
    );
    let texture1 = saved
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"TEXTURE1")
        .expect("TEXTURE1 was saved");
    let parsed = parse_texture_lump(&texture1.data, 3).expect("it decodes");
    let door = parsed
        .textures
        .iter()
        .find(|definition| definition.name == "DOOR1")
        .expect("the built texture");
    let first = door.patches.first().expect("one placement");
    assert_eq!(
        (first.patch, first.x, first.y),
        (2, 0, 0),
        "the appended index and the literal origins"
    );
}

#[tokio::test]
async fn build_texture_adds_a_definition_and_undoes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let base = iwad(dir.path());
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "build_texture",
        json!({
            "name": "newtex",
            "width": 32_u64,
            "height": 32_u64,
            "patches": [
                { "patch": "WALLP1", "x": 0_i64, "y": 0_i64 },
                { "patch": "STEPP1", "x": 0_i64, "y": 16_i64 },
            ],
        }),
    )
    .await;
    assert_eq!(report["name"], json!("NEWTEX"), "stored uppercase");
    assert_eq!(report["lump"], json!("TEXTURE1"), "the default target");
    assert_eq!(
        report["created"],
        json!(true),
        "TEXTURE1 came into the working archive"
    );
    assert_eq!(report["pnames_added"], json!([]), "both patches are listed");
    assert_eq!(report["warnings"], json!([]), "32 is a power of two");
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        listed(&textures, "textures"),
        vec!["WALL1", "STEP1", "NEWTEX"],
        "appended after the fixture pair"
    );
    let new = &textures["textures"][2];
    assert_eq!(new["width"], json!(32_u64), "the requested width");
    assert_eq!(new["patches"], json!(2_u64), "both placements");
    assert_eq!(new["source"], json!("working"), "the override wins");
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listed(&working, "lumps"),
        vec!["TEXTURE1"],
        "PNAMES was not touched: both patches were already listed"
    );
    assert_iwad_texture1_count(&base, 2);
    assert_eq!(
        undo_labels(&client).await,
        vec!["build_texture NEWTEX"],
        "the whole change is one undoable edit"
    );
    call(&client, "undo", json!({})).await;
    let back = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        listed(&back, "textures"),
        vec!["WALL1", "STEP1"],
        "undo steps the build back"
    );
}

#[tokio::test]
async fn build_texture_appends_a_new_patch_name_to_pnames() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    call(
        &client,
        "add_lump",
        json!({ "name": "DOORP1", "hex": "00 01 02 03", "namespace": "patches" }),
    )
    .await;
    let before = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        before["total"],
        json!(2_u64),
        "add_lump does not touch PNAMES"
    );
    let report = call(
        &client,
        "build_texture",
        json!({
            "name": "DOOR1",
            "width": 16_u64,
            "height": 16_u64,
            "patches": [{ "patch": "doorp1", "x": 0_i64, "y": 0_i64 }],
        }),
    )
    .await;
    assert_eq!(
        report["pnames_added"],
        json!(["DOORP1"]),
        "the name entered PNAMES"
    );
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(names["total"], json!(3_u64), "PNAMES grew");
    assert_eq!(
        names["pnames"][2]["lump_found"],
        json!(true),
        "the appended name resolves to the added lump"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    let stored = listed(&working, "lumps");
    assert!(
        stored.contains(&"PNAMES".to_owned()) && stored.contains(&"TEXTURE1".to_owned()),
        "both definition lumps are working overrides: {stored:?}"
    );
    // The saved file carries the definition and the new index round-trip.
    let out = dir.path().join("built.wad");
    call(&client, "save_wad", path_arg(&out)).await;
    assert_saved_build(&out);
}

#[tokio::test]
async fn build_texture_refuses_missing_patch_lumps_until_allowed() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let args = json!({
        "name": "GHOST",
        "width": 16_u64,
        "height": 16_u64,
        "patches": [{ "patch": "GHOSTP", "x": 0_i64, "y": 0_i64 }],
    });
    let (code, message) = build_err(&client, args.clone()).await;
    assert_eq!(code, "not_found", "no lump behind the patch");
    assert!(
        message.contains("GHOSTP") && message.contains("allow_missing_patches"),
        "the message names the patch and the escape: {message}"
    );
    let refused = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert!(
        refused["lumps"].as_array().expect("array").is_empty(),
        "the refusal wrote nothing"
    );
    let allowed = json!({ "allow_missing_patches": true, "name": "GHOST",
        "width": 16_u64, "height": 16_u64,
        "patches": [{ "patch": "GHOSTP", "x": 0_i64, "y": 0_i64 }] });
    let report = call(&client, "build_texture", allowed).await;
    assert_eq!(
        report["pnames_added"],
        json!(["GHOSTP"]),
        "the escape builds anyway"
    );
    let names = call(&client, "list_pnames", json!({ "pattern": "GHOSTP" })).await;
    assert_eq!(
        names["pnames"][0]["lump_found"],
        json!(false),
        "listed, but still no lump behind it"
    );
}

#[tokio::test]
async fn build_texture_conflicts_then_replaces_in_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message) = build_err(
        &client,
        json!({
            "name": "WALL1",
            "width": 16_u64,
            "height": 16_u64,
            "patches": [{ "patch": "WALLP1", "x": 0_i64, "y": 0_i64 }],
        }),
    )
    .await;
    assert_eq!(code, "already_exists", "WALL1 stands in TEXTURE1");
    assert!(
        message.contains("TEXTURE1") && message.contains("replace=true"),
        "the message names the lump and the escape: {message}"
    );
    let report = call(
        &client,
        "build_texture",
        json!({
            "name": "WALL1",
            "width": 16_u64,
            "height": 16_u64,
            "replace": true,
            "patches": [{ "patch": "WALLP1", "x": 0_i64, "y": 0_i64 }],
        }),
    )
    .await;
    assert_eq!(report["lump"], json!("TEXTURE1"), "the same lump");
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        listed(&textures, "textures"),
        vec!["WALL1", "STEP1"],
        "no third record, the position was kept"
    );
    assert_eq!(
        textures["textures"][0]["width"],
        json!(16_u64),
        "the new width"
    );
    assert_eq!(
        textures["textures"][0]["patches"],
        json!(1_u64),
        "the new placement list"
    );
    assert_eq!(
        textures["textures"][1]["patches"],
        json!(16_u64),
        "STEP1 is untouched"
    );
}

#[tokio::test]
async fn build_texture_creates_the_targeted_texture2_and_warns_about_tiling() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "build_texture",
        json!({
            "name": "ALT1",
            "width": 33_u64,
            "height": 16_u64,
            "patches": [{ "patch": "WALLP1", "x": 0_i64, "y": 0_i64 }],
            "target": "TEXTURE2",
        }),
    )
    .await;
    assert_eq!(report["lump"], json!("TEXTURE2"), "the explicit target");
    assert_eq!(
        report["created"],
        json!(true),
        "TEXTURE2 existed nowhere and was created"
    );
    let warnings = report["warnings"].as_array().expect("array");
    assert_eq!(warnings.len(), 1, "the odd width is noted: {report}");
    assert!(
        warnings
            .first()
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("power of two")),
        "the warning explains the tiling"
    );
    let textures = call(&client, "list_textures", json!({ "pattern": "ALT1" })).await;
    assert_eq!(
        textures["textures"][0]["lump"],
        json!("TEXTURE2"),
        "in the new lump"
    );
    assert_eq!(
        textures["textures"][0]["patches"],
        json!(1_u64),
        "the placement survived"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listed(&working, "lumps"),
        vec!["TEXTURE2"],
        "only the created lump is written"
    );
}

#[tokio::test]
async fn build_texture_validates_its_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let ok_patch = || json!([{ "patch": "WALLP1", "x": 0_i64, "y": 0_i64 }]);
    let cases = Vec::from([
        json!({ "name": "TOO LONG NAME", "width": 16_u64, "height": 16_u64, "patches": ok_patch() }),
        json!({ "name": "OK1", "width": 0_u64, "height": 16_u64, "patches": ok_patch() }),
        json!({ "name": "OK1", "width": 16_u64, "height": 4097_u64, "patches": ok_patch() }),
        json!({ "name": "OK1", "width": 16_u64, "height": 16_u64, "patches": [] }),
        json!({ "name": "OK1", "width": 16_u64, "height": 16_u64, "patches": ok_patch(),
                "target": "TEXTURES" }),
        json!({ "name": "OK1", "width": 16_u64, "height": 16_u64,
                "patches": [{ "patch": "bad name", "x": 0_i64, "y": 0_i64 }] }),
    ]);
    assert_builds_refused(&client, cases).await;
    let (empty_code, empty_message, _) = call_err(
        &client,
        "build_texture",
        json!({ "name": "X", "width": 8_u64, "height": 8_u64, "patches": [] }),
    )
    .await;
    assert_eq!(empty_code, "invalid_argument", "empty patches code");
    assert!(
        empty_message.contains("at least one"),
        "the message states the rule: {empty_message}"
    );
    assert!(
        undo_labels(&client).await.is_empty(),
        "no refusal wrote anything"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert!(
        working["lumps"].as_array().expect("array").is_empty(),
        "and left no override"
    );
}

#[tokio::test]
async fn remove_texture_drops_a_definition_and_undoes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let base = iwad(dir.path());
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "remove_texture", json!({ "name": "step1" })).await;
    assert_eq!(report["name"], json!("STEP1"), "echoed uppercase");
    assert_eq!(report["lump"], json!("TEXTURE1"), "the defining lump");
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        listed(&textures, "textures"),
        vec!["WALL1"],
        "STEP1 is gone"
    );
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(names["total"], json!(2_u64), "PNAMES is not touched");
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listed(&working, "lumps"),
        vec!["TEXTURE1"],
        "copied on write"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["remove_texture STEP1"],
        "one labelled edit"
    );
    assert_iwad_texture1_count(&base, 2);
    call(&client, "undo", json!({})).await;
    let back = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        listed(&back, "textures"),
        vec!["WALL1", "STEP1"],
        "undo brings STEP1 back"
    );
}

#[tokio::test]
async fn remove_texture_reports_unknown_names_and_bad_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) = call_err(&client, "remove_texture", json!({ "name": "NOPE" })).await;
    assert_eq!(code, "not_found", "unknown texture code");
    assert!(
        message.contains("list_textures"),
        "the message points at the listing: {message}"
    );
    let (bad_code, _, _) = call_err(
        &client,
        "remove_texture",
        json!({ "name": "TOO LONG NAME" }),
    )
    .await;
    assert_eq!(bad_code, "invalid_argument", "bad name code");
    assert!(
        undo_labels(&client).await.is_empty(),
        "the refusals recorded no edit"
    );
}
