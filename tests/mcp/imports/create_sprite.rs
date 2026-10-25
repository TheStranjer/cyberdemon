//! The one-shot `create_sprite_from_image`: background removal and
//! trim with the feet-on-origin default, the `remove_background` and
//! `trim` switches, the `height` fit and the too-tall note, `frames`
//! writing one lump per letter in one undoable step, the mirror-pair
//! naming, the rotation-coverage warnings, the all-background refusal
//! hint, the duplicate and `replace` rule and the naming-conflict
//! catalogue. The figure pictures live here with the tests.

use std::path::Path;

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::imports::{
    Client, Refusal, flat_png, iwad_client, mentions, refuse_all, rgba_pixel, source_args,
    sprite_rgba, step_ops, write_file,
};
use crate::support::{call, call_err};

/// The `40x60` figure PNG's pixel at `(x, y)`: red inside the `10x20`
/// block at `(10, 20)`, white outside it.
fn figure_pixel(x: u32, y: u32) -> [u8; 4] {
    if (10_u32..20_u32).contains(&x) && (20_u32..40_u32).contains(&y) {
        [255_u8, 0, 0, 255]
    } else {
        [255_u8, 255, 255, 255]
    }
}

/// A `40x60` palette PNG: white all over, a solid red `10x20` block.
fn figure_png() -> Vec<u8> {
    let pixels: Vec<u8> = (0_u32..60_u32)
        .flat_map(|y| (0_u32..40_u32).map(move |x| figure_pixel(x, y)))
        .flatten()
        .collect();
    let img = RgbaImage::new(40_u32, 60_u32, pixels).expect("40x60 is valid");
    encode_png(&img, None).expect("test PNG encodes")
}

/// A `12x12` fully white PNG.
fn white_png() -> Vec<u8> {
    let side = 12_usize;
    let count = side
        .checked_mul(side)
        .expect("test pixels")
        .checked_mul(4_usize)
        .expect("test bytes");
    let img = RgbaImage::new(12_u32, 12_u32, vec![255_u8; count]).expect("12x12 is valid");
    encode_png(&img, None).expect("test PNG encodes")
}

/// One successful `create_sprite_from_image` call of `file`.
async fn create_sprite(client: &Client, file: &Path, extra: Value) -> Value {
    call(client, "create_sprite_from_image", source_args(file, extra)).await
}

/// The pixel index just inside the figure block at row 20, column 12.
fn figure_index() -> usize {
    usize::try_from(
        20_u32
            .checked_mul(40_u32)
            .expect("row 20")
            .checked_add(12_u32)
            .expect("pixel 12"),
    )
    .expect("in range")
}

/// Every one of `names` answers `not_found` from `get_sprite`.
async fn sprites_are_gone(client: &Client, names: &[&str]) {
    for name in names {
        let (code, _, _) = call_err(client, "get_sprite", json!({ "name": name })).await;
        assert_eq!(code, "not_found", "{name} left with the edit");
    }
}

/// The default pipeline: background removal, trim, feet-on-origin
/// offsets, as one labelled edit with the definition hint.
#[tokio::test]
async fn create_sprite_removes_the_background_and_stands_on_its_feet() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let report = create_sprite(&client, &file, json!({ "sprite": "BOBA" })).await;
    assert_eq!(
        report["lumps"],
        json!(["BOBAA0"]),
        "the parts form names one lump"
    );
    assert_eq!(
        (report["width"].as_u64(), report["height"].as_u64()),
        (Some(10_u64), Some(20_u64)),
        "trimmed to the figure"
    );
    assert_eq!(report["left_offset"], json!(5_i64), "half the final width");
    assert_eq!(report["top_offset"], json!(20_i64), "feet on the origin");
    assert_eq!(
        step_ops(&report),
        vec!["remove_background", "trim"],
        "the automatic chain"
    );
    assert_eq!(report["notes"], json!([]), "nothing scaled");
    assert_eq!(report["warnings"], json!([]), "a free sprite");
    assert!(report["preview"]["url"].is_string(), "a preview asset");
    assert_eq!(
        report["next_steps"][0],
        json!(
            "a new thing type needs a definition: edit_decorate (ZDoom ports) or edit_dehacked to repoint an existing type's frames"
        ),
        "the definition hint"
    );
    let sprite = call(&client, "get_sprite", json!({ "name": "BOBAA0" })).await;
    assert_eq!(sprite["width"], json!(10_u64), "the stored lump");
    assert_eq!(sprite["top_offset"], json!(20_i64), "keeps the offsets");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["create_sprite BOBAA0"]),
        "one undoable step"
    );
}

/// `remove_background: false` keeps the white canvas opaque;
/// `trim: false` keeps the cleared border as holes, so the area
/// outside the figure is transparent.
#[tokio::test]
async fn the_background_switches_keep_the_canvas_or_its_holes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let kept = create_sprite(
        &client,
        &file,
        json!({ "name": "BOBAA1", "remove_background": false }),
    )
    .await;
    assert_eq!(
        (kept["width"].as_u64(), kept["height"].as_u64()),
        (Some(40_u64), Some(60_u64)),
        "the white is content, so the trim keeps the full canvas"
    );
    assert_eq!(kept["top_offset"], json!(60_i64), "feet on the full height");
    let kept_rgba = sprite_rgba(&client, "BOBAA1").await;
    assert_eq!(
        rgba_pixel(&kept_rgba, 0_usize)[3],
        255_u8,
        "the corner stays opaque"
    );
    let holed = create_sprite(&client, &file, json!({ "name": "BOBAA2", "trim": false })).await;
    assert_eq!(
        holed["transparent_pixels"],
        json!(2200_u64),
        "the white was dropped as holes"
    );
    let holed_rgba = sprite_rgba(&client, "BOBAA2").await;
    assert_eq!(
        rgba_pixel(&holed_rgba, 0_usize)[3],
        0_u8,
        "outside the figure is transparent"
    );
    assert_eq!(
        rgba_pixel(&holed_rgba, figure_index())[3],
        255_u8,
        "the figure is opaque"
    );
}

/// `height` scales the trimmed figure with the aspect kept, and the
/// offsets follow the final picture.
#[tokio::test]
async fn create_sprite_scales_to_the_requested_height() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let report = create_sprite(
        &client,
        &file,
        json!({ "sprite": "BOBA", "rotation": 1_u64, "height": 40_u64 }),
    )
    .await;
    assert_eq!(report["lumps"], json!(["BOBAA1"]), "the rotation carries");
    assert_eq!(
        (report["width"].as_u64(), report["height"].as_u64()),
        (Some(20_u64), Some(40_u64)),
        "10x20 scaled to 40 tall keeps 20 wide"
    );
    assert_eq!(report["left_offset"], json!(10_i64), "half the final width");
    assert_eq!(report["top_offset"], json!(40_i64), "the final height");
    assert_eq!(
        step_ops(&report),
        vec!["remove_background", "trim", "fit"],
        "the fit follows the trim"
    );
    assert_eq!(report["notes"], json!([]), "an explicit height is quiet");
}

/// `frames` writes the same final image once per letter, in one edit
/// that one undo rolls back entirely.
#[tokio::test]
async fn create_sprite_frames_writes_one_lump_per_letter_at_once() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let report = create_sprite(
        &client,
        &file,
        json!({ "sprite": "BOBA", "frames": ["A", "B"] }),
    )
    .await;
    assert_eq!(
        report["lumps"],
        json!(["BOBAA0", "BOBAB0"]),
        "both frames named"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["create_sprite BOBAA0"]),
        "the first lump labels the single step"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["create_sprite BOBAA0"]),
        "one undo rolls the pair back"
    );
    sprites_are_gone(&client, &["BOBAA0", "BOBAB0"]).await;
}

/// The parts form with `mirror_rotation` names the pair lump.
#[tokio::test]
async fn create_sprite_names_the_mirror_pair() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let report = create_sprite(
        &client,
        &file,
        json!({ "sprite": "BOBA", "frame": "B", "rotation": 2_u64, "mirror_rotation": 7_u64 }),
    )
    .await;
    assert_eq!(
        report["lumps"],
        json!(["BOBAB2B7"]),
        "the assembled pair name"
    );
    let sprite = call(&client, "get_sprite", json!({ "name": "BOBAB2B7" })).await;
    assert_eq!(sprite["width"], json!(10_u64), "the lump serves");
}

/// Rotations another lump already serves are warnings, not refusals.
#[tokio::test]
async fn create_sprite_warns_about_rotations_the_session_already_serves() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let report = create_sprite(
        &client,
        &file,
        json!({ "sprite": "TROO", "rotation": 2_u64 }),
    )
    .await;
    assert_eq!(report["lumps"], json!(["TROOA2"]), "the import lands");
    let warnings = report["warnings"].as_array().expect("a warning list");
    assert!(
        warnings
            .iter()
            .any(|warning| mentions(warning, &["frame A rotation 2 is also served by TROOA2A8"])),
        "the pair's coverage warns: {warnings:?}"
    );
}

/// A picture that is all background refuses with the documented hint.
#[tokio::test]
async fn a_picture_that_is_all_background_refuses_with_the_hint() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "WHITE.png", &white_png());
    let (code, message, _) = call_err(
        &client,
        "create_sprite_from_image",
        source_args(&file, json!({ "sprite": "BOBA" })),
    )
    .await;
    assert_eq!(code, "invalid_argument", "nothing is left to store");
    assert!(
        message.contains("the whole image was removed as background"),
        "the message names the cause: {message}"
    );
    assert!(
        message.contains("lower background_tolerance")
            && message.contains("set remove_background false"),
        "both ways out are named: {message}"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "nothing was written");
}

/// A taller-than-`256` picture without `height` is scaled to `128`
/// tall and says so in the notes.
#[tokio::test]
async fn a_tall_picture_is_scaled_with_the_documented_note() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "TALL.png",
        &flat_png(60_u32, 300_u32, |_| false),
    );
    let report = create_sprite(
        &client,
        &file,
        json!({ "name": "BOBAA3", "remove_background": false, "trim": false }),
    )
    .await;
    assert_eq!(
        (report["width"].as_u64(), report["height"].as_u64()),
        (Some(26_u64), Some(128_u64)),
        "60x300 to 128 tall keeps the aspect"
    );
    assert_eq!(
        report["notes"],
        json!(["scaled to 128 px tall; pass height to choose"]),
        "the documented note"
    );
    assert_eq!(step_ops(&report), vec!["fit"], "the automatic fit");
}

/// A working twin refuses until `replace`, which overwrites in place;
/// the refusal adds no undo step.
#[tokio::test]
async fn create_sprite_duplicates_need_replace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    create_sprite(&client, &file, json!({ "sprite": "BOBA" })).await;
    let (code, message, _) = call_err(
        &client,
        "create_sprite_from_image",
        source_args(&file, json!({ "sprite": "BOBA" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the working twin stands");
    assert!(
        message.contains("replace=true"),
        "the message names the way out: {message}"
    );
    let replaced =
        create_sprite(&client, &file, json!({ "sprite": "BOBA", "replace": true })).await;
    assert_eq!(replaced["lumps"], json!(["BOBAA0"]), "the overwrite lands");
    let session = call(&client, "get_session_info", json!({})).await;
    let undo = session["undo"].as_array().expect("the undo stack");
    assert_eq!(undo.len(), 2_usize, "the refusal added no step: {undo:?}");
    assert_eq!(
        undo[0],
        json!("create_sprite BOBAA0"),
        "both steps are the create"
    );
}

/// Naming conflicts, a bad height and a bad colour each report
/// `invalid_argument`; nothing is stored along the way.
#[tokio::test]
async fn bad_sprite_creation_arguments_are_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FIGURE.png", &figure_png());
    let cases: [Refusal; 7] = [
        (
            "both forms",
            source_args(&file, json!({ "name": "TROOA1", "sprite": "TROO" })),
            "invalid_argument",
            "not both",
        ),
        (
            "frame and frames",
            source_args(
                &file,
                json!({ "sprite": "TROO", "frame": "A", "frames": ["B"] }),
            ),
            "invalid_argument",
            "frame and frames",
        ),
        (
            "name and frames",
            source_args(&file, json!({ "name": "TROOA1", "frames": ["B"] })),
            "invalid_argument",
            "invalid together with name",
        ),
        (
            "no selector",
            source_args(&file, Value::Null),
            "invalid_argument",
            "needs a sprite",
        ),
        (
            "empty frames",
            source_args(&file, json!({ "sprite": "TROO", "frames": [] })),
            "invalid_argument",
            "frames is empty",
        ),
        (
            "height",
            source_args(&file, json!({ "sprite": "BOBA", "height": 0_u64 })),
            "invalid_argument",
            "1..=512",
        ),
        (
            "background colour",
            source_args(
                &file,
                json!({ "sprite": "BOBA", "background_color": "green" }),
            ),
            "invalid_argument",
            "background_color",
        ),
    ];
    refuse_all(&client, "create_sprite_from_image", &cases).await;
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "nothing was stored");
}
