//! The one-shot `create_flat_from_image`: the `64x64` default and the
//! `128`/`256` squares with their vanilla-Doom warning, the echoed
//! `dither` and `size`, the default `tileable` seam blend, `contain`
//! padding, the layer-flat collision with the everywhere-used hint and
//! its `replace` override, `assign_role` for flat roles with the
//! wall-role refusal, and the single `undo`.

use std::path::Path;

use serde_json::{Value, json};

use crate::imports::{
    Client, FIXTURE_FLATS, Refusal, flat_expected_rgba, flat_png, flat_rgba, iwad_client, mentions,
    refuse_all, rgb_at, source_args, step_ops, write_file,
};
use crate::support::{call, call_err};

/// One successful `create_flat_from_image` call of `file` with `extra`.
async fn create_flat(client: &Client, file: &Path, extra: Value) -> Value {
    call(client, "create_flat_from_image", source_args(file, extra)).await
}

/// One failing `create_flat_from_image` call of `file` with `extra`.
async fn create_flat_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
    let (code, message, _) =
        call_err(client, "create_flat_from_image", source_args(file, extra)).await;
    (code, message)
}

/// The `create_flat_from_image` rule: any picture becomes a `64x64`
/// flat with the automatic `fit` (+ default `make_tileable`) steps, a
/// preview, a stored lump and one undoable edit.
#[tokio::test]
async fn create_flat_fits_any_picture_to_the_vanilla_square() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "PHOTO.png",
        &flat_png(100_u32, 80_u32, |_| false),
    );
    let report = create_flat(&client, &file, json!({ "name": "myfloor" })).await;
    assert_eq!(report["flat"], json!("MYFLOOR"), "the name uppercases");
    assert_eq!(
        step_ops(&report),
        vec!["fit", "make_tileable"],
        "the fit always runs and tiling is on by default"
    );
    assert_eq!(report["size"], json!(64_u64), "the vanilla side by default");
    assert_eq!(
        report["dither"],
        json!("floyd_steinberg"),
        "the process_image dither default echoes"
    );
    assert_eq!(report["overrides_existing"], json!(false), "a free name");
    assert!(report["preview"]["url"].is_string(), "a preview asset");
    assert_eq!(
        report["next_steps"][0],
        json!("edit_sector floor_flat/ceiling_flat"),
        "the documented way forward"
    );
    assert_eq!(report["warnings"], json!([]), "the picture is opaque");
    assert!(
        report["max_color_error"].as_u64().is_some(),
        "the quantization is measured"
    );
    let flat = call(&client, "get_flat", json!({ "name": "MYFLOOR" })).await;
    assert_eq!(flat["width"], json!(64_u64), "the vanilla square");
    assert_eq!(flat["height"], json!(64_u64), "both ways");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["create_flat MYFLOOR"]),
        "one undoable step"
    );
}

/// `tileable` (on by default) blends the edges; off keeps them.
#[tokio::test]
async fn the_tileable_switch_blends_the_flat_edges() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "RAMP.png",
        &flat_png(100_u32, 80_u32, |_| false),
    );
    let tiled = create_flat(&client, &file, json!({ "name": "TILED" })).await;
    let asdrawn = create_flat(
        &client,
        &file,
        json!({ "name": "ASDRAWN", "tileable": false }),
    )
    .await;
    assert_eq!(step_ops(&asdrawn), vec!["fit"], "no tiling step off");
    assert_eq!(
        step_ops(&tiled),
        vec!["fit", "make_tileable"],
        "on by default"
    );
    let blended = flat_rgba(&client, "TILED").await;
    let kept = flat_rgba(&client, "ASDRAWN").await;
    assert_eq!(blended.len(), kept.len(), "both are 64x64 RGBA");
    assert_ne!(blended, kept, "the 16-pixel band changed the seams");
}

/// One `create_flat_from_image` at square `side` under `lump`: checks
/// the `size` and `dither` echoes and the stored shape, and answers
/// whether the result warned about the vanilla limit.
async fn square_flat(client: &Client, file: &Path, side: u64, lump: &str) -> bool {
    let report = create_flat(
        client,
        file,
        json!({ "name": lump, "size": side, "tileable": false }),
    )
    .await;
    assert_eq!(report["size"], json!(side), "{side} echoes");
    assert_eq!(
        report["dither"],
        json!("floyd_steinberg"),
        "the shared default echoes"
    );
    let flat = call(client, "get_flat", json!({ "name": lump })).await;
    assert_eq!(flat["width"], json!(side), "the stored width");
    assert_eq!(flat["height"], json!(side), "the stored height");
    report["warnings"]
        .as_array()
        .expect("a warning list")
        .iter()
        .any(|warning| mentions(warning, &["vanilla Doom loads only 64x64 flats", "ZDoom"]))
}

/// `size` takes the bigger squares a `ZDoom`-family port reads, echoes
/// the side in the result and warns past the vanilla `64x64`.
#[tokio::test]
async fn create_flat_size_picks_the_square_and_warns_past_the_vanilla_side() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "SIZES.png",
        &flat_png(100_u32, 80_u32, |_| false),
    );
    assert!(
        !square_flat(&client, &file, 64_u64, "SQ64").await,
        "the vanilla shape is quiet"
    );
    assert!(
        square_flat(&client, &file, 128_u64, "SQ128").await,
        "128 warns"
    );
    assert!(
        square_flat(&client, &file, 256_u64, "SQ256").await,
        "256 warns"
    );
}

/// The acceptance round trip: a `256x256` flat made here reads back
/// through `get_flat` with exactly the source's pixels.
#[tokio::test]
async fn a_256_flat_created_here_reads_back_pixel_perfect() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "HIRES.png",
        &flat_png(256_u32, 256_u32, |_| false),
    );
    let report = create_flat(
        &client,
        &file,
        json!({
            "name": "BIGFLOOR",
            "size": 256_u64,
            "tileable": false,
            "dither": "none",
        }),
    )
    .await;
    assert_eq!(report["dither"], json!("none"), "the mode echoes");
    let warnings = report["warnings"].as_array().expect("a warning list");
    assert!(
        warnings
            .iter()
            .any(|warning| mentions(warning, &["256x256", "vanilla Doom loads only 64x64 flats"],)),
        "the warning names the shape and the limit: {warnings:?}"
    );
    assert_eq!(
        flat_rgba(&client, "BIGFLOOR").await,
        flat_expected_rgba(0x1_0000_usize),
        "the stored flat is the source picture"
    );
    let listed = call(&client, "list_flats", json!({ "pattern": "BIGFLOOR" })).await;
    assert_eq!(
        listed["flats"][0]["size"],
        json!(0x1_0000_u64),
        "the raw 256x256 byte count"
    );
}

/// `contain` letterboxes the whole picture: the padding quantizes to
/// the palette colour nearest to black and counts as kept-opaque.
#[tokio::test]
async fn contain_fits_the_whole_picture_with_padding() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "WIDE.png", &flat_png(64_u32, 40_u32, |_| false));
    let report = create_flat(
        &client,
        &file,
        json!({ "name": "PADDED", "fit": "contain", "tileable": false, "dither": "none" }),
    )
    .await;
    let warnings = report["warnings"].as_array().expect("a warning list");
    assert_eq!(warnings.len(), 1_usize, "the padding warns");
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|text| text.contains("kept opaque") && text.contains("1536 pixels")),
        "the warning counts the padding: {warnings:?}"
    );
    let rgba = flat_rgba(&client, "PADDED").await;
    assert_eq!(
        rgb_at(&rgba, 0_usize),
        [85_u8, 170, 85],
        "the padding took the palette colour nearest to black"
    );
    let row_12 = 12_usize.checked_mul(64).expect("row 12");
    assert_eq!(
        rgb_at(&rgba, row_12),
        [0_u8, 255, 0],
        "row 12 is source row 0"
    );
}

/// A layer-held flat name refuses with the everywhere-used hint;
/// `replace` accepts the override; a working twin then needs
/// `replace` too.
#[tokio::test]
async fn create_flat_collisions_refuse_until_replace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "CLASH.png",
        &flat_png(32_u32, 32_u32, |_| false),
    );
    let (code, message, _) = call_err(
        &client,
        "create_flat_from_image",
        source_args(&file, json!({ "name": "FLOOR1" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the IWAD flat stands");
    assert!(
        message.contains("this would override FLOOR1 everywhere it is used"),
        "the hint names the consequence: {message}"
    );
    assert!(
        message.contains("the IWAD"),
        "and names the layer: {message}"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "the refusal wrote nothing");
    let replaced = create_flat(&client, &file, json!({ "name": "FLOOR1", "replace": true })).await;
    assert_eq!(
        replaced["overrides_existing"],
        json!(true),
        "the override reports"
    );
    assert_eq!(
        step_ops(&replaced),
        vec!["fit", "make_tileable"],
        "the normal chain"
    );
    let listed = call(&client, "list_flats", json!({ "pattern": "FLOOR1" })).await;
    assert_eq!(
        listed["flats"][0]["source"],
        json!("working"),
        "the working copy now serves"
    );
    let (twin_code, twin_message, _) = call_err(
        &client,
        "create_flat_from_image",
        source_args(&file, json!({ "name": "FLOOR1" })),
    )
    .await;
    assert_eq!(twin_code, "already_exists", "the working twin stands");
    assert!(
        twin_message.contains("working archive"),
        "the message names the archive: {twin_message}"
    );
}

/// `assign_role` points a theme flat role at the new flat and warns
/// that undo will not roll the override back.
#[tokio::test]
async fn create_flat_assign_role_points_the_theme_at_the_new_flat() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "ROLED.png",
        &flat_png(64_u32, 64_u32, |_| false),
    );
    let report = create_flat(
        &client,
        &file,
        json!({ "name": "ROLED", "assign_role": "floor" }),
    )
    .await;
    assert_eq!(report["role"], json!("floor"), "the assigned role");
    assert!(
        report["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| mentions(
                warning,
                &[
                    "theme override floor -> ROLED",
                    "undo does not roll it back"
                ]
            )),
        "the warning explains the theme state: {}",
        report["warnings"]
    );
    let theme = call(&client, "get_texture_theme", json!({})).await;
    assert_eq!(
        theme["roles"]["floor"]["texture"],
        json!("ROLED"),
        "the role resolves"
    );
    assert_eq!(
        theme["roles"]["floor"]["matched"],
        json!("exact"),
        "as an override"
    );
}

/// One `create_flat_from_image` call asking for role `role`, refused.
async fn refuse_flat_role(client: &Client, file: &Path, role: &str) {
    let (code, message) =
        create_flat_err(client, file, json!({ "name": "NOPE", "assign_role": role })).await;
    assert_eq!(code, "invalid_argument", "{role} refuses");
    assert!(
        message.contains("role"),
        "the message explains {role}: {message}"
    );
}

/// Both a wall role and an unknown one refuse.
async fn refuse_flat_roles(client: &Client, file: &Path) {
    for role in ["wall", "roof"] {
        refuse_flat_role(client, file, role).await;
    }
}

/// A wall role (and an unknown one) refuse before anything is written.
#[tokio::test]
async fn create_flat_wall_roles_refuse_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "WALLROLE.png",
        &flat_png(64_u32, 64_u32, |_| false),
    );
    refuse_flat_roles(&client, &file).await;
    let listed = call(&client, "list_flats", json!({ "pattern": "NOPE" })).await;
    assert_eq!(listed["total"], json!(0_u64), "nothing was stored");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "no edit landed");
    let theme = call(&client, "get_texture_theme", json!({})).await;
    assert_ne!(
        theme["roles"]["floor"]["texture"],
        json!("NOPE"),
        "the theme is untouched"
    );
}

/// One `undo` removes the created flat.
#[tokio::test]
async fn one_undo_rolls_back_the_created_flat() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "UNDONE.png",
        &flat_png(100_u32, 80_u32, |_| false),
    );
    create_flat(&client, &file, json!({ "name": "UNDONE" })).await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["create_flat UNDONE"]),
        "the single step rolls back"
    );
    let (code, _, _) = call_err(&client, "get_flat", json!({ "name": "UNDONE" })).await;
    assert_eq!(code, "not_found", "the flat left with its edit");
}

/// Bad sources and parameters each report their code; nothing is
/// stored along the way.
#[tokio::test]
async fn bad_flat_creation_arguments_are_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
    let good = write_file(dir.path(), "GOOD.png", &flat_png(64_u32, 64_u32, |_| false));
    let cases: [Refusal; 5] = [
        (
            "junk",
            source_args(&junk, json!({ "name": "X" })),
            "format_error",
            "image",
        ),
        (
            "long name",
            source_args(&good, json!({ "name": "TOO LONG NAME" })),
            "invalid_argument",
            "name",
        ),
        (
            "wall role",
            source_args(&good, json!({ "name": "X", "assign_role": "door" })),
            "invalid_argument",
            "wall role",
        ),
        (
            "size",
            source_args(&good, json!({ "name": "X", "size": 96_u64 })),
            "invalid_argument",
            "64, 128, 256",
        ),
        (
            "missing",
            json!({ "source": dir.path().join("absent.png").display().to_string(), "name": "X" }),
            "not_found",
            "absent.png",
        ),
    ];
    refuse_all(&client, "create_flat_from_image", &cases).await;
    let flats = call(&client, "list_flats", json!({})).await;
    assert_eq!(flats["total"], json!(FIXTURE_FLATS), "nothing was stored");
}
