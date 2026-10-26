//! `add_patch`: converting a `PNG` through the shared import pipeline into the
//! created `P_START` region with its `PNAMES` entry, storing raw `Doom` patch
//! bytes byte-identically (or with only header bytes `4..8` rewritten by an
//! explicit offsets pair), the `add_to_pnames` opt-out, the duplicate refusal
//! that names `replace_patch`, the parameter refusal catalogue, the undo/redo
//! pair and the `IWAD`-owned name that does not clash.

use serde_json::{Value, json};

use crate::patches::{
    FIXTURE_PNAMES, PATCH_OFFSETS, Refusal, add, doom_patch, iwad_client, palette_png, refuse_all,
    source_args, stored_bytes, working_names, write_file,
};
use crate::support::{call, call_err};

/// The `PNG` half: the picture is quantized and encoded, lands in the
/// created `patches` region of the working archive, and `PNAMES` lists
/// it at the reported index.
#[tokio::test]
async fn add_patch_converts_a_picture_into_the_patches_region() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "MYART.png", &palette_png());
    let report = add(&client, &file, "MYART").await;
    assert_eq!(report["name"], json!("MYART"), "the name echoes");
    assert_eq!(report["converted"], json!(true), "the pipeline half ran");
    assert_eq!(report["width"], json!(4_u64), "the stored width");
    assert_eq!(report["height"], json!(4_u64), "the stored height");
    assert_eq!(report["left_offset"], json!(0_i64), "the zero default");
    assert_eq!(report["top_offset"], json!(0_i64), "zero both ways");
    assert_eq!(
        report["pnames_index"],
        json!(FIXTURE_PNAMES),
        "appended after the fixture pair"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
    assert_eq!(listed["total"], json!(1_u64), "list_patches shows it");
    assert_eq!(
        listed["patches"][0]["namespace"],
        json!("patches"),
        "in the region"
    );
    assert_eq!(
        listed["patches"][0]["source"],
        json!("working"),
        "from the working archive"
    );
    assert_eq!(
        working_names(&client).await,
        ["P_START", "MYART", "P_END", "PNAMES"],
        "the region markers were created and the grown PNAMES appended"
    );
    let pnames = call(&client, "list_pnames", json!({ "pattern": "MYART" })).await;
    assert_eq!(pnames["total"], json!(1_u64), "the listing has it");
    assert_eq!(
        pnames["pnames"][0]["index"], report["pnames_index"],
        "the listing agrees with the report"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["add_patch MYART"]),
        "one undoable edit labelled as documented"
    );
}

/// The passthrough half: `Doom` patch bytes stand byte-identical, and
/// an explicit offsets pair rewrites only header bytes `4..8`.
#[tokio::test]
async fn add_patch_stores_existing_doom_patch_bytes_unchanged() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let bytes = doom_patch();
    let file = write_file(dir.path(), "PASSTHRU.patch", &bytes);
    let kept = add(&client, &file, "KEPT").await;
    assert_eq!(kept["converted"], json!(false), "the passthrough half ran");
    assert_eq!(kept["width"], json!(4_u64), "the header's width");
    assert_eq!(kept["height"], json!(4_u64), "the header's height");
    assert_eq!(
        kept["left_offset"],
        json!(i64::from(PATCH_OFFSETS.0)),
        "the stored left offset"
    );
    assert_eq!(
        kept["top_offset"],
        json!(i64::from(PATCH_OFFSETS.1)),
        "the stored top offset"
    );
    assert_eq!(
        stored_bytes(&client, dir.path(), "KEPT").await,
        bytes,
        "byte for byte"
    );
    let shifted = call(
        &client,
        "add_patch",
        source_args(
            &file,
            json!({ "name": "SHIFTED", "offsets": { "x": 3_i64, "y": -5_i64 } }),
        ),
    )
    .await;
    assert_eq!(shifted["converted"], json!(false), "still a passthrough");
    assert_eq!(shifted["left_offset"], json!(3_i64), "the rewritten pair");
    assert_eq!(shifted["top_offset"], json!(-5_i64), "both fields");
    let moved = stored_bytes(&client, dir.path(), "SHIFTED").await;
    assert_eq!(moved.len(), bytes.len(), "the same length");
    assert_eq!(&moved[..4], &bytes[..4], "width and height stay");
    assert_ne!(&moved[4..8], &bytes[4..8], "the offsets changed");
    assert_eq!(&moved[8..], &bytes[8..], "the payload stays byte-identical");
}

/// `add_to_pnames: false` answers `pnames_index: null` and leaves the
/// list as it stands.
#[tokio::test]
async fn add_patch_can_stay_out_of_pnames() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "OFFLIST.png", &palette_png());
    let report = call(
        &client,
        "add_patch",
        source_args(&file, json!({ "name": "OFFLIST", "add_to_pnames": false })),
    )
    .await;
    assert_eq!(report["pnames_index"], Value::Null, "opted out");
    assert_eq!(
        report["converted"],
        json!(true),
        "the picture still converted"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "OFFLIST" })).await;
    assert_eq!(listed["total"], json!(1_u64), "the lump stands");
    assert_eq!(
        listed["patches"][0]["in_pnames"],
        json!(false),
        "PNAMES does not know it"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "PNAMES is unchanged by the add"
    );
}

/// A working twin refuses, pointing at `replace_patch`, and stores
/// nothing: the undo stack keeps the single first edit.
#[tokio::test]
async fn a_duplicate_add_refuses_and_names_replace_patch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "TWIN.png", &palette_png());
    add(&client, &file, "TWIN").await;
    let (code, message, _) = call_err(
        &client,
        "add_patch",
        source_args(&file, json!({ "name": "TWIN" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the twin refuses");
    assert!(
        message.contains("replace_patch"),
        "the message names the way out: {message}"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "TWIN" })).await;
    assert_eq!(listed["total"], json!(1_u64), "one copy stands");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["add_patch TWIN"]),
        "the refusal added no step"
    );
}

/// Missing sources, bad parameters and undecodable non-patch bytes each
/// report their code, and nothing lands.
#[tokio::test]
async fn bad_add_arguments_are_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let patch_file = write_file(dir.path(), "SRC.patch", &doom_patch());
    let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
    let cases: [Refusal; 6] = [
        (
            "missing",
            json!({
                "name": "X",
                "source": dir.path().join("absent.png").display().to_string(),
            }),
            "not_found",
            "absent.png",
        ),
        (
            "nine characters",
            source_args(&patch_file, json!({ "name": "TOOLONGNM" })),
            "invalid_argument",
            "name",
        ),
        (
            "preset",
            source_args(&patch_file, json!({ "name": "X", "offsets": "middle" })),
            "invalid_argument",
            "center_bottom",
        ),
        (
            "mode",
            source_args(
                &patch_file,
                json!({ "name": "X", "transparency": { "mode": "blur" } }),
            ),
            "invalid_argument",
            "\"none\"",
        ),
        (
            "offset past i16",
            source_args(
                &patch_file,
                json!({ "name": "X", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
            ),
            "invalid_argument",
            "does not fit",
        ),
        (
            "junk",
            source_args(&junk, json!({ "name": "X" })),
            "format_error",
            "image",
        ),
    ];
    refuse_all(&client, &cases).await;
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "nothing was stored");
}

/// Undo removes the lump and the `PNAMES` write together; redo brings
/// both back.
#[tokio::test]
async fn undo_and_redo_move_the_patch_and_its_pnames_entry() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "MYART.png", &palette_png());
    add(&client, &file, "MYART").await;
    let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        undone["undone"],
        json!(["add_patch MYART"]),
        "the labelled step went back"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
    assert_eq!(listed["total"], json!(0_u64), "the lump left");
    assert!(
        working_names(&client).await.is_empty(),
        "the created region left with it"
    );
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "the PNAMES write undid too"
    );
    let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
    assert_eq!(
        redone["redone"],
        json!(["add_patch MYART"]),
        "the step replayed"
    );
    let back = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
    assert_eq!(back["total"], json!(1_u64), "the lump is back");
    assert_eq!(
        back["patches"][0]["in_pnames"],
        json!(true),
        "and PNAMES lists it again"
    );
    let after = call(&client, "list_pnames", json!({})).await;
    assert_eq!(after["total"], json!(3_u64), "the grown list stands");
}

/// A lump the `IWAD` holds does not clash: the add becomes a working
/// override the merged view resolves to.
#[tokio::test]
async fn add_patch_overrides_an_iwad_patch_of_the_same_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "WALLP1.png", &palette_png());
    let report = add(&client, &file, "WALLP1").await;
    assert_eq!(report["name"], json!("WALLP1"), "the override lands");
    assert_eq!(report["converted"], json!(true), "from the picture half");
    assert_eq!(
        report["pnames_index"],
        json!(0_u64),
        "the name PNAMES already lists keeps its index"
    );
    let listed = call(&client, "list_patches", json!({ "pattern": "WALLP1" })).await;
    assert_eq!(
        listed["total"],
        json!(1_u64),
        "last wins in the merged view"
    );
    assert_eq!(
        listed["patches"][0]["source"],
        json!("working"),
        "the working copy serves"
    );
}
