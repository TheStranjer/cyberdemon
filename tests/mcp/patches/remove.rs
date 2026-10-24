//! `remove_patch` deleting a working patch: the lump alone, or the lump and its
//! `PNAMES` entry with every texture reference renumbered through the shared
//! removal plan, in one undoable step; the `IWAD`-owned name that refuses and
//! names `replace_patch`, the `conflict` refusal a still-referenced texture
//! raises before `force`, and the listed name that never had a lump.

use serde_json::json;

use crate::patches::{
    FIXTURE_PNAMES, IWAD_SIDE, iwad_client, palette_png, patch_args, patch_row, pnames_list,
    remove, replace, texture_patches, working_names, write_file,
};
use crate::support::{call, call_err};

/// `add_patch` then `remove_patch`: the lump leaves the working
/// archive alone - `removed_from_pnames: false`, the name gone from
/// `list_patches`, the markers (and so the closed region) and the
/// fixture's unchanged `PNAMES` behind.
#[tokio::test]
async fn remove_patch_deletes_the_working_lump_and_answers_on_the_list() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "OFFLIST.png", &palette_png());
    call(
        &client,
        "add_patch",
        patch_args(&file, json!({ "name": "OFFLIST", "add_to_pnames": false })),
    )
    .await;
    let report = remove(&client, "OFFLIST", json!({})).await;
    assert_eq!(report["name"], json!("OFFLIST"), "the name echoes");
    assert_eq!(report["removed_lump"], json!(true), "the working lump went");
    assert_eq!(
        report["removed_from_pnames"],
        json!(false),
        "the flag was not asked for"
    );
    assert_eq!(
        report["textures_changed"],
        json!(0_u64),
        "no list write, no renumbering"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "OFFLIST" })).await;
    assert_eq!(listed["total"], json!(0_u64), "gone from list_patches");
    assert_eq!(
        working_names(&client).await,
        ["P_START", "P_END"],
        "the markers stayed, so the region stayed closed"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(pnames["total"], json!(FIXTURE_PNAMES), "PNAMES never moved");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["add_patch OFFLIST", "remove_patch OFFLIST"]),
        "each step is its own labelled edit, oldest first"
    );
}

/// The `PNAMES` half of the pair: `add_patch` lists the name by
/// default, and a `remove_patch` with `remove_from_pnames: true` drops
/// the entry again with no texture shifted.
#[tokio::test]
async fn remove_patch_can_drop_the_pnames_entry_too() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let on = write_file(dir.path(), "ONLIST.png", &palette_png());
    call(
        &client,
        "add_patch",
        patch_args(&on, json!({ "name": "ONLIST" })),
    )
    .await;
    let grown = call(&client, "list_pnames", json!({})).await;
    assert_eq!(grown["total"], json!(3_u64), "add_patch listed it");
    let cut = remove(&client, "ONLIST", json!({ "remove_from_pnames": true })).await;
    assert_eq!(cut["removed_lump"], json!(true), "both halves went");
    assert_eq!(
        cut["removed_from_pnames"],
        json!(true),
        "the entry dropped out"
    );
    assert_eq!(
        cut["textures_changed"],
        json!(0_u64),
        "no texture referred to it, nothing shifted"
    );
    let back = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames_list(&back),
        vec!["WALLP1", "STEPP1"],
        "count 2 again"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["add_patch ONLIST", "remove_patch ONLIST"]),
        "the add and its removal stand side by side, oldest first"
    );
}

/// `remove_patch` reads the working archive only: the `IWAD`'s own
/// `WALLP1` answers `not_found` pointing at `replace_patch`, and a bad
/// name is `invalid_argument` - nothing is written either way.
#[tokio::test]
async fn removing_an_iwad_owned_patch_refuses_naming_replace_patch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) = call_err(&client, "remove_patch", json!({ "name": "WALLP1" })).await;
    assert_eq!(code, "not_found", "no working copy exists");
    assert!(
        message.contains("replace_patch") && message.contains("working"),
        "the hint names the way to a working copy: {message}"
    );
    let (bad_code, bad_message, _) =
        call_err(&client, "remove_patch", json!({ "name": "TOOLONGNM" })).await;
    assert_eq!(bad_code, "invalid_argument", "nine characters");
    assert!(
        bad_message.contains("name"),
        "the message names the parameter: {bad_message}"
    );
    let listed = patch_row(&client, "WALLP1").await;
    assert_eq!(
        listed["source"],
        json!({ "layer": 0_u64 }),
        "the IWAD copy still serves"
    );
    assert!(
        working_names(&client).await.is_empty(),
        "nothing was written"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "and nothing was recorded");
}

/// `replace_patch` gives `WALLP1` a working copy; removing it with
/// `remove_from_pnames` while `WALL1` still refers to it is a
/// `conflict` and NOTHING leaves - not the lump, not the list.
#[tokio::test]
async fn a_removal_texture_still_refuses_conflicts_without_force() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    replace(&client, &file, "WALLP1").await;
    let (code, message, _) = call_err(
        &client,
        "remove_patch",
        json!({ "name": "WALLP1", "remove_from_pnames": true }),
    )
    .await;
    assert_eq!(code, "conflict", "WALL1 refers to the entry");
    assert!(
        message.contains("WALL1") && message.contains("TEXTURE1"),
        "the message names the texture and its lump: {message}"
    );
    assert!(
        message.contains("force=true"),
        "the message states the escape: {message}"
    );
    assert_eq!(
        working_names(&client).await,
        ["P_START", "WALLP1", "P_END"],
        "the override lump is still there"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "PNAMES is still there"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["replace_patch WALLP1"]),
        "the refusal added no step"
    );
}

/// The same removal forced through: the working lump, the `PNAMES`
/// entry and the renumbered `TEXTURE1` leave in one edit - `WALL1`
/// loses its 32 references, `STEP1`'s survive renumbered from index 1
/// to 0, and `STEPP1` still resolves.
#[tokio::test]
async fn forcing_the_removal_drops_the_lump_the_entry_and_renumbers() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    replace(&client, &file, "WALLP1").await;
    let report = remove(
        &client,
        "WALLP1",
        json!({ "remove_from_pnames": true, "force": true }),
    )
    .await;
    assert_eq!(report["removed_lump"], json!(true), "the override went");
    assert_eq!(
        report["removed_from_pnames"],
        json!(true),
        "the entry went with it"
    );
    assert_eq!(
        report["textures_changed"],
        json!(2_u64),
        "WALL1 pruned, STEP1 renumbered"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames_list(&pnames),
        vec!["STEPP1"],
        "WALLP1 is off the list"
    );
    assert_eq!(
        pnames["pnames"][0]["index"],
        json!(0_u64),
        "the survivor shifted down"
    );
    assert_eq!(
        texture_patches(&client, "WALL1").await,
        0_u64,
        "its 32 references fell"
    );
    assert_eq!(
        texture_patches(&client, "STEP1").await,
        16_u64,
        "STEP1 kept its references"
    );
    let step = call(&client, "get_patch", json!({ "name": "STEPP1" })).await;
    assert_eq!(
        step["width"],
        json!(IWAD_SIDE),
        "the renumbered patch resolves"
    );
    assert_eq!(
        working_names(&client).await,
        ["P_START", "P_END", "PNAMES", "TEXTURE1"],
        "lump gone, list and texture rewritten as overrides"
    );
}

/// Undo steps the forced removal back in ONE entry: lump, old list and
/// the old texture bytes return together; redo repeats the whole
/// change.
#[tokio::test]
async fn undo_and_redo_walk_the_forced_removal_in_one_step() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    replace(&client, &file, "WALLP1").await;
    remove(
        &client,
        "WALLP1",
        json!({ "remove_from_pnames": true, "force": true }),
    )
    .await;
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["replace_patch WALLP1", "remove_patch WALLP1"]),
        "the removal is one labelled entry"
    );
    let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        undone["undone"],
        json!(["remove_patch WALLP1"]),
        "one step back"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames_list(&pnames),
        vec!["WALLP1", "STEPP1"],
        "the old list is back"
    );
    assert_eq!(
        texture_patches(&client, "WALL1").await,
        32_u64,
        "the old texture bytes are back"
    );
    let back = patch_row(&client, "WALLP1").await;
    assert_eq!(
        back["source"],
        json!("working"),
        "the override stands again"
    );
    let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        redone["redone"],
        json!(["remove_patch WALLP1"]),
        "the change replayed"
    );
    let after = call(&client, "list_pnames", json!({})).await;
    assert_eq!(pnames_list(&after), vec!["STEPP1"], "and undid again");
}

/// A name the working `PNAMES` lists but no working lump carries is
/// still removable with `remove_from_pnames`: `removed_lump` reports
/// `false`, the entry leaves, and no `not_found` is raised.
#[tokio::test]
async fn a_pnames_entry_without_a_lump_is_removable_without_a_lump_write() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    call(&client, "edit_pnames", json!({ "add": ["GHOSTP"] })).await;
    let report = remove(&client, "GHOSTP", json!({ "remove_from_pnames": true })).await;
    assert_eq!(
        report["removed_lump"],
        json!(false),
        "the name never had a working lump"
    );
    assert_eq!(report["removed_from_pnames"], json!(true), "its entry left");
    assert_eq!(
        report["textures_changed"],
        json!(0_u64),
        "appended last, nothing shifted"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames_list(&pnames),
        vec!["WALLP1", "STEPP1"],
        "the ghost is off the list"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["edit_pnames add", "remove_patch GHOSTP"]),
        "one labelled removal on top of the list edit"
    );
}
