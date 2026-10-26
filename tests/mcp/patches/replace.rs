//! `replace_patch` swapping a patch the session already has: an `IWAD`-owned
//! patch gains a working override the merged view serves while `PNAMES` stays
//! put, a working-owned patch is overwritten in place at its own index, the
//! unknown name points at `add_patch`, the override undoes back to its layer
//! and redoes, and the shared parameter catalogue refuses here as it does for
//! `add_patch`. [`refuse_replacements`] is this topic's half of that table.

use serde_json::json;

use crate::patches::{
    Client, FIXTURE_PNAMES, IWAD_SIDE, Refusal, add, doom_patch, iwad_bytes, iwad_client,
    palette_png, patch_row, replace, source_args, stored_bytes, working_index, working_names,
    write_file,
};
use crate::support::{call, call_err};

/// Assert every `replace_patch` call in `cases` refuses as documented.
async fn refuse_replacements(client: &Client, cases: &[Refusal]) {
    for case in cases {
        let (label, args, expect_code, expect_text) = (case.0, &case.1, case.2, case.3);
        let (code, message, _) = call_err(client, "replace_patch", args.clone()).await;
        assert_eq!(code, *expect_code, "{label} code: {message}");
        assert!(
            message.contains(expect_text),
            "{label} message should name {expect_text:?}: {message}"
        );
    }
}

/// Replacing the `IWAD`'s own `WALLP1` with a converted `PNG` creates
/// a working override in a newly-created `patches` region; the `IWAD`
/// file on disk is never touched and `PNAMES` is never rewritten.
#[tokio::test]
async fn replace_patch_creates_a_working_override_of_an_iwad_patch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let before = iwad_bytes(dir.path());
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    let report = replace(&client, &file, "WALLP1").await;
    assert_eq!(report["name"], json!("WALLP1"), "the name echoes");
    assert_eq!(report["converted"], json!(true), "the picture half ran");
    assert_eq!(report["width"], json!(4_u64), "the new 4x4 width");
    assert_eq!(report["height"], json!(4_u64), "the new 4x4 height");
    assert_eq!(report["pnames_index"], json!(0_u64), "its old PNAMES index");
    assert_eq!(
        report["created_override"],
        json!(true),
        "only a lower layer held it"
    );
    let listed = patch_row(&client, "WALLP1").await;
    assert_eq!(listed["namespace"], json!("patches"), "in the region");
    assert_eq!(listed["source"], json!("working"), "from the working copy");
    assert_eq!(
        working_names(&client).await,
        ["P_START", "WALLP1", "P_END"],
        "the working archive gained the region and the override"
    );
    assert_eq!(iwad_bytes(dir.path()), before, "the IWAD file is unchanged");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["replace_patch WALLP1"]),
        "one undoable edit labelled as documented"
    );
}

/// The override is what the merged view now serves: `get_patch` reads
/// the new pixels and `list_patches` shows `source: "working"`, while
/// `list_pnames` keeps `WALLP1` at its old index.
#[tokio::test]
async fn the_override_wins_the_merged_view_and_pnames_stays_put() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    replace(&client, &file, "WALLP1").await;
    let picture = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
    assert_eq!(
        picture["width"],
        json!(4_u64),
        "get_patch serves the override"
    );
    assert_eq!(picture["height"], json!(4_u64), "both new dimensions");
    let listed = patch_row(&client, "WALLP1").await;
    assert_eq!(listed["source"], json!("working"), "the working copy wins");
    let pnames = call(&client, "list_pnames", json!({ "pattern": "WALLP1" })).await;
    assert_eq!(pnames["total"], json!(1_u64), "the one entry");
    assert_eq!(
        pnames["pnames"][0]["index"],
        json!(0_u64),
        "at its old index"
    );
    assert_eq!(
        pnames["pnames"][0]["lump_found"],
        json!(true),
        "and a lump still answers to it"
    );
    let all = call(&client, "list_pnames", json!({})).await;
    assert_eq!(all["total"], json!(FIXTURE_PNAMES), "PNAMES still has two");
}

/// Replacing a patch the working archive already owns overwrites it in
/// place: `created_override` is `false`, its directory index is
/// unchanged and the passthrough payload now stands there.
#[tokio::test]
async fn replace_patch_overwrites_a_working_owned_patch_in_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let png = write_file(dir.path(), "MYART.png", &palette_png());
    let added = add(&client, &png, "MYART").await;
    let added_index = working_index(&client, "MYART").await;
    let bytes = doom_patch();
    let patch_file = write_file(dir.path(), "MYART.patch", &bytes);
    let report = replace(&client, &patch_file, "MYART").await;
    assert_eq!(
        report["converted"],
        json!(false),
        "the passthrough half ran"
    );
    assert_eq!(
        report["created_override"],
        json!(false),
        "the working archive already owned it"
    );
    assert_eq!(
        report["pnames_index"], added["pnames_index"],
        "the same index"
    );
    assert_eq!(
        report["lump_index"],
        json!(added_index),
        "the same directory slot"
    );
    assert_eq!(
        working_index(&client, "MYART").await,
        added_index,
        "still in the same place"
    );
    assert_eq!(
        working_names(&client).await,
        ["P_START", "MYART", "P_END", "PNAMES"],
        "the directory order is unchanged"
    );
    assert_eq!(
        stored_bytes(&client, dir.path(), "MYART").await,
        bytes,
        "the payload now stands replaced"
    );
}

/// A name no layer holds comes back `not_found` naming `add_patch`,
/// with nothing written.
#[tokio::test]
async fn replacing_an_unknown_name_refuses_naming_add_patch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "MYART.png", &palette_png());
    let (code, message, _) = call_err(
        &client,
        "replace_patch",
        source_args(&file, json!({ "name": "NOPE" })),
    )
    .await;
    assert_eq!(code, "not_found", "the missing target");
    assert!(
        message.contains("add_patch"),
        "the message names the way in: {message}"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "NOPE" })).await;
    assert_eq!(listed["total"], json!(0_u64), "nothing was written");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "the refusal added no step");
}

/// Undo steps the created override back: `list_patches` reports the
/// `IWAD` source again and the working copy is gone; redo re-creates
/// the override.
#[tokio::test]
async fn replace_patch_override_undoes_to_the_layer_and_redoes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
    replace(&client, &file, "WALLP1").await;
    let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        undone["undone"],
        json!(["replace_patch WALLP1"]),
        "the labelled step went back"
    );
    let back = patch_row(&client, "WALLP1").await;
    assert_eq!(
        back["source"],
        json!({ "layer": 0_u64 }),
        "the IWAD copy serves again"
    );
    assert_eq!(back["width"], json!(IWAD_SIDE), "the IWAD's own pixels");
    let working = working_names(&client).await;
    assert!(
        !working.contains(&"WALLP1".to_owned()),
        "the working override is gone: {working:?}"
    );
    let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        redone["redone"],
        json!(["replace_patch WALLP1"]),
        "the step replayed"
    );
    let forward = patch_row(&client, "WALLP1").await;
    assert_eq!(forward["source"], json!("working"), "the override is back");
    assert_eq!(forward["width"], json!(4_u64), "with the new pixels");
}

/// `replace_patch` refuses the shared parameter catalogue the same way
/// `add_patch` does, and nothing lands: the `IWAD`'s `WALLP1` still
/// serves and the undo stack stays empty.
#[tokio::test]
async fn bad_replace_arguments_are_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let patch_file = write_file(dir.path(), "WALLP1.patch", &doom_patch());
    let cases: [Refusal; 3] = [
        (
            "nine characters",
            source_args(&patch_file, json!({ "name": "TOOLONGNM" })),
            "invalid_argument",
            "name",
        ),
        (
            "preset",
            source_args(
                &patch_file,
                json!({ "name": "WALLP1", "offsets": "middle" }),
            ),
            "invalid_argument",
            "center_bottom",
        ),
        (
            "offset past i16",
            source_args(
                &patch_file,
                json!({ "name": "WALLP1", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
            ),
            "invalid_argument",
            "does not fit",
        ),
    ];
    refuse_replacements(&client, &cases).await;
    let listed = patch_row(&client, "WALLP1").await;
    assert_eq!(
        listed["source"],
        json!({ "layer": 0_u64 }),
        "the IWAD copy still stands"
    );
    assert_eq!(listed["width"], json!(IWAD_SIDE), "with its original bytes");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "nothing was written");
}
