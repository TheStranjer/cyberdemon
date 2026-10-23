//! `edit_pnames`' add / conflict / force / rename / `replace_all` paths.
//!
//! Appending a name uppercases it, skips what is already listed and copies
//! only `PNAMES` into the working archive; removing a referenced name
//! conflicts, then forces through with the surviving references renumbered;
//! a rename keeps every index in place and so every texture reference;
//! `replace_all` revalidates the whole new list; an unusable request is
//! refused without recording an edit and a missing `PNAMES` is created
//! above a resource layer.

use cyberdemon::wad::archive::Wad;
use cyberdemon::wad::textures::{parse_pnames, parse_texture_lump};
use serde_json::json;

use crate::fixtures::{iwad, pwad};
use crate::support::{call, call_err, connect};
use crate::textures::{iwad_client, listed, path_arg, undo_labels};

#[tokio::test]
async fn edit_pnames_add_copies_on_write_and_leaves_the_iwad_alone() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let base = iwad(dir.path());
    let client = connect().await;
    call(&client, "open_iwad", path_arg(&base)).await;
    let report = call(
        &client,
        "edit_pnames",
        json!({ "add": ["NEWP1", "wallp1"] }),
    )
    .await;
    assert_eq!(report["pnames"], json!(3_u64), "one name appended");
    assert_eq!(report["added"], json!(["NEWP1"]), "uppercased, once");
    assert_eq!(
        report["skipped"],
        json!(["WALLP1"]),
        "already present, reported"
    );
    assert_eq!(
        report["textures_changed"],
        json!(0_u64),
        "appends never shift"
    );
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        listed(&names, "pnames"),
        vec!["WALLP1", "STEPP1", "NEWP1"],
        "the merged view shows the new entry"
    );
    assert_eq!(
        names["pnames"][2]["lump_found"],
        json!(false),
        "NEWP1 has no lump yet"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listed(&working, "lumps"),
        vec!["PNAMES"],
        "only PNAMES was copied into the working archive"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["edit_pnames add"],
        "the whole change is one undoable edit"
    );
    let on_disk = Wad::read_file(&base).expect("the fixture file reads");
    let original = on_disk
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"PNAMES")
        .expect("the IWAD still holds its PNAMES");
    assert_eq!(
        parse_pnames(&original.data).expect("the original decodes"),
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "the IWAD file never changed"
    );
}

#[tokio::test]
async fn edit_pnames_remove_conflicts_then_forces_through() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) =
        call_err(&client, "edit_pnames", json!({ "remove": ["WALLP1"] })).await;
    assert_eq!(code, "conflict", "referenced name code");
    assert!(
        message.contains("WALL1 (TEXTURE1)"),
        "the message lists the referencing texture: {message}"
    );
    assert!(
        message.contains("force=true"),
        "the message names the escape: {message}"
    );
    let refused = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert!(
        refused["lumps"].as_array().expect("array").is_empty(),
        "the refused edit wrote nothing"
    );
    let report = call(
        &client,
        "edit_pnames",
        json!({ "remove": ["WALLP1"], "force": true }),
    )
    .await;
    assert_eq!(report["pnames"], json!(1_u64), "only STEPP1 is left");
    assert_eq!(report["removed"], json!(["WALLP1"]), "reported removed");
    assert_eq!(
        report["textures_changed"],
        json!(2_u64),
        "pruned and renumbered"
    );
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        textures["textures"][0]["patches"],
        json!(0_u64),
        "WALL1 lost its refs"
    );
    assert_eq!(
        textures["textures"][1]["patches"],
        json!(16_u64),
        "STEP1 kept its refs, renumbered"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        listed(&working, "lumps"),
        vec!["PNAMES", "TEXTURE1"],
        "PNAMES and the changed texture lump were copied in"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["edit_pnames remove"],
        "one undoable edit for both lumps"
    );
}

#[tokio::test]
async fn forced_remove_renumbers_so_surviving_refs_stay_true() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    call(
        &client,
        "edit_pnames",
        json!({ "remove": ["WALLP1"], "force": true }),
    )
    .await;
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        listed(&names, "pnames"),
        vec!["STEPP1"],
        "the surviving list"
    );
    assert_eq!(
        names["pnames"][0]["index"],
        json!(0_u64),
        "now at index zero"
    );
    let base = iwad(dir.path());
    let on_disk = Wad::read_file(&base).expect("the fixture file reads");
    let texture = on_disk
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"TEXTURE1")
        .expect("the IWAD still holds its TEXTURE1");
    let untouched = parse_texture_lump(&texture.data, 2).expect("the original decodes");
    assert_eq!(
        untouched.textures.first().expect("WALL1").patches.len(),
        32,
        "the IWAD's texture lump never changed"
    );
}

#[tokio::test]
async fn edit_pnames_rename_keeps_texture_references() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "edit_pnames",
        json!({ "rename": [{ "from": "wallp1", "to": "WALLPX" }] }),
    )
    .await;
    assert_eq!(report["pnames"], json!(2_u64), "the count stands");
    assert_eq!(
        report["renamed"],
        json!([{ "from": "WALLP1", "to": "WALLPX" }]),
        "the uppercased pair"
    );
    assert_eq!(
        report["textures_changed"],
        json!(0_u64),
        "indices did not move"
    );
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        listed(&names, "pnames"),
        vec!["WALLPX", "STEPP1"],
        "renamed in place"
    );
    assert_eq!(
        names["pnames"][0]["lump_found"],
        json!(false),
        "WALLPX has no lump"
    );
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        textures["textures"][0]["patches"],
        json!(32_u64),
        "WALL1 still carries all its placements"
    );
}

#[tokio::test]
async fn edit_pnames_replace_all_reindexes_and_revalidates() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) =
        call_err(&client, "edit_pnames", json!({ "replace_all": ["WALLP1"] })).await;
    assert_eq!(code, "conflict", "STEPP1 vanishes while STEP1 refers to it");
    assert!(
        message.contains("STEP1 (TEXTURE1)") && !message.contains("WALL1"),
        "only STEP1 refers to the vanishing STEPP1: {message}"
    );
    let report = call(
        &client,
        "edit_pnames",
        json!({ "replace_all": ["WALLP1"], "force": true }),
    )
    .await;
    assert_eq!(report["pnames"], json!(1_u64), "the new complete list");
    assert_eq!(report["removed"], json!(["STEPP1"]), "the vanished entry");
    assert_eq!(report["added"], json!([]), "nothing new came in");
    assert_eq!(
        report["textures_changed"],
        json!(1_u64),
        "only STEP1 changed (its refs all vanished)"
    );
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(
        textures["textures"][0]["patches"],
        json!(32_u64),
        "WALL1's index-0 refs survive untouched"
    );
    assert_eq!(
        textures["textures"][1]["patches"],
        json!(0_u64),
        "STEP1 was pruned"
    );
}

#[tokio::test]
async fn edit_pnames_validates_its_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) = call_err(&client, "edit_pnames", json!({})).await;
    assert_eq!(code, "invalid_argument", "no operation code");
    assert!(
        message.contains("exactly one"),
        "the message states the rule: {message}"
    );
    let (both_code, _, _) = call_err(
        &client,
        "edit_pnames",
        json!({ "add": ["X"], "remove": ["WALLP1"] }),
    )
    .await;
    assert_eq!(both_code, "invalid_argument", "two operations code");
    let (name_code, name_message, _) = call_err(
        &client,
        "edit_pnames",
        json!({ "add": ["TOO LONG A NAME"] }),
    )
    .await;
    assert_eq!(name_code, "invalid_argument", "bad name code");
    assert!(
        name_message.contains("TOO LONG A NAME"),
        "the message echoes the name: {name_message}"
    );
    let (empty_code, empty_message, _) =
        call_err(&client, "edit_pnames", json!({ "add": [] })).await;
    assert_eq!(empty_code, "invalid_argument", "empty add code");
    assert!(
        empty_message.contains("at least one"),
        "the message states the rule: {empty_message}"
    );
    let (dupe_code, _, _) =
        call_err(&client, "edit_pnames", json!({ "replace_all": ["A", "a"] })).await;
    assert_eq!(dupe_code, "invalid_argument", "case-folded duplicate code");
    let (missing_code, _, _) = call_err(
        &client,
        "edit_pnames",
        json!({ "rename": [{ "from": "NOPE", "to": "XYZ" }] }),
    )
    .await;
    assert_eq!(missing_code, "not_found", "missing rename source code");
    assert!(
        undo_labels(&client).await.is_empty(),
        "none of the refusals recorded an edit"
    );
}

#[tokio::test]
async fn edit_pnames_creates_a_missing_pnames_through_a_resource_layer() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let extra = pwad(dir.path());
    call(&client, "merge_wad", path_arg(&extra)).await;
    let report = call(&client, "edit_pnames", json!({ "add": ["FIRST"] })).await;
    assert_eq!(report["pnames"], json!(1_u64), "the list started fresh");
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        listed(&names, "pnames"),
        vec!["FIRST"],
        "the merged view shows it"
    );
    let (code, _, _) = call_err(
        &client,
        "edit_pnames",
        json!({ "rename": [{ "from": "SECOND", "to": "THIRD" }] }),
    )
    .await;
    assert_eq!(code, "not_found", "rename needs an existing entry");
}
