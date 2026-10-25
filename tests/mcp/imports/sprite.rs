//! The sprite importer `import_image_as_sprite` with its alias
//! `import_png_as_sprite`: both naming forms, the mirror pair, the
//! feet-on-origin default offsets, the sprites region, the rotation
//! warnings, the duplicate and `replace` rule and its refusal
//! catalogue. The naming and rotation readers live here with them.

use std::path::Path;

use serde_json::{Value, json};

use crate::imports::{
    Client, Refusal, SIDE, assert_same_pipeline, hole_png, iwad_client, palette_png, refuse_all,
    source_args, sprite_rgba, write_file,
};
use crate::support::{call, call_err};

/// One successful `import_image_as_sprite` of `file` with `extra`.
async fn sprite_import(client: &Client, file: &Path, extra: Value) -> Value {
    call(client, "import_image_as_sprite", source_args(file, extra)).await
}

/// The rotation row `list_sprites` answers with for `sprite` frame `A`.
async fn listed_rotation(client: &Client, sprite: &str, index: usize) -> Value {
    let listed = call(client, "list_sprites", json!({ "prefix": sprite })).await;
    listed["sprites"][0]["frames"][0]["rotations"][index].clone()
}

/// What a sprite report should name: the lump name, its four-character
/// prefix, its frame letter and the rotation the image serves.
type SpriteNaming = (&'static str, &'static str, &'static str, u64);

/// Assert a sprite report's four naming fields.
fn assert_naming(report: &Value, naming: SpriteNaming) {
    let (name, sprite, frame, rotation) = naming;
    assert_eq!(report["name"], json!(name), "the name echoes");
    assert_eq!(report["sprite"], json!(sprite), "the four-character prefix");
    assert_eq!(report["frame"], json!(frame), "the frame letter");
    assert_eq!(
        report["rotation"],
        json!(rotation),
        "the rotation the image itself serves"
    );
}

/// Assert a report carries the `center_bottom` default for the 8x8 test
/// picture: feet on the origin, half the width to the left.
fn assert_feet_on_origin(report: &Value) {
    assert_eq!(
        report["left_offset"],
        json!(4_i64),
        "center_bottom is width/2"
    );
    assert_eq!(
        report["top_offset"],
        json!(8_i64),
        "center_bottom is the full height"
    );
}

/// The sprite round trip: a whole lump name, the shared pipeline
/// measurements, the default feet-on-origin offsets and the undo label.
#[tokio::test]
async fn a_png_imports_as_a_sprite_under_its_full_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    let report = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
    assert_naming(&report, ("BOBAA1", "BOBA", "A", 1));
    assert_eq!(
        report["mirror"],
        Value::Null,
        "a six-character name mirrors nothing"
    );
    assert_eq!(
        report["warnings"],
        json!([]),
        "a fresh sprite clashes with nothing"
    );
    assert_eq!(
        report["lump_index"],
        json!(1_u64),
        "after the created S_START"
    );
    assert_feet_on_origin(&report);
    assert_eq!(
        report["transparent_pixels"],
        json!(1_u64),
        "the alpha hole dropped one pixel"
    );
    assert_eq!(report["colors_used"], json!(63_u64), "63 palette colours");
    assert_eq!(
        report["max_color_error"],
        json!(0_i64),
        "the palette is exact"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_sprite BOBAA1"]),
        "one undoable edit labelled as documented"
    );
}

/// The stored sprite sits in the working sprites region and reads back
/// through `get_sprite` and `list_sprites`.
#[tokio::test]
async fn an_imported_sprite_reads_back_through_the_graphics_tools() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
    let shown = call(
        &client,
        "get_sprite",
        json!({ "sprite": "BOBA", "frame": "A", "rotation": 1_u64 }),
    )
    .await;
    assert_eq!(shown["lump"], json!("BOBAA1"), "the new lump serves it");
    assert_eq!(shown["mirrored"], json!(false), "used as stored");
    assert_feet_on_origin(&shown);
    assert_eq!(
        listed_rotation(&client, "BOBA", 0).await,
        json!({ "rotation": 1_u64, "lump": "BOBAA1", "mirrored": false }),
        "list_sprites shows the new frame"
    );
    let working = call(
        &client,
        "list_lumps",
        json!({ "scope": "working", "namespace": "sprites" }),
    )
    .await;
    assert_eq!(
        working["lumps"][1]["name"],
        json!("BOBAA1"),
        "inside the created region"
    );
}

/// The parts form assembles the eight-character mirror-pair name, and
/// `get_sprite` serves the mirrored rotation flipped.
#[tokio::test]
async fn the_parts_form_builds_the_mirror_pair_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    let report = sprite_import(
        &client,
        &file,
        json!({
            "sprite": "boba",
            "frame": "b",
            "rotation": 2_u64,
            "mirror_rotation": 8_u64,
        }),
    )
    .await;
    assert_naming(&report, ("BOBAB2B8", "BOBA", "B", 2));
    assert_eq!(report["mirror"], json!(8_u64), "the mirrored rotation");
    assert_eq!(
        report["warnings"],
        json!([]),
        "a fresh sprite clashes with nothing"
    );
    let flipped = call(
        &client,
        "get_sprite",
        json!({ "sprite": "BOBA", "frame": "B", "rotation": 8_u64 }),
    )
    .await;
    assert_eq!(
        flipped["lump"],
        json!("BOBAB2B8"),
        "the pair lump serves it"
    );
    assert_eq!(flipped["mirrored"], json!(true), "flipped for rotation 8");
    assert_eq!(
        flipped["left_offset"],
        json!(4_i64),
        "left' = width - left keeps a centred sprite centred"
    );
    assert_eq!(
        listed_rotation(&client, "BOBA", 1).await,
        json!({ "rotation": 8_u64, "lump": "BOBAB2B8", "mirrored": true }),
        "the listing carries both rotations"
    );
}

/// A `grAb` chunk beats the `center_bottom` default; a preset or an
/// explicit word outranks the chunk.
#[tokio::test]
async fn sprite_offsets_take_grab_then_the_chosen_preset() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let grabbed = write_file(
        dir.path(),
        "GRAB.png",
        &palette_png(SIDE, SIDE, None, Some((7_i32, -9_i32))),
    );
    let taken = sprite_import(&client, &grabbed, json!({ "name": "BOBAA2" })).await;
    assert_eq!(taken["left_offset"], json!(7_i64), "grAb beats the default");
    assert_eq!(taken["top_offset"], json!(-9_i64), "grAb beats the default");
    let zeroed = sprite_import(
        &client,
        &grabbed,
        json!({ "name": "BOBAA3", "offsets": "zero" }),
    )
    .await;
    assert_eq!(
        zeroed["left_offset"],
        json!(0_i64),
        "\"zero\" outranks the chunk"
    );
    assert_eq!(zeroed["top_offset"], json!(0_i64), "\"zero\" both ways");
    let plain = write_file(
        dir.path(),
        "PLAIN.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let middle = sprite_import(
        &client,
        &plain,
        json!({ "name": "BOBAA4", "offsets": "center" }),
    )
    .await;
    assert_eq!(middle["left_offset"], json!(4_i64), "width/2");
    assert_eq!(middle["top_offset"], json!(4_i64), "height/2");
    let literal = sprite_import(
        &client,
        &plain,
        json!({ "name": "BOBAA5", "offsets": { "x": -2_i64, "y": 20_i64 } }),
    )
    .await;
    assert_eq!(literal["left_offset"], json!(-2_i64), "the literal left");
    assert_eq!(literal["top_offset"], json!(20_i64), "the literal top");
    assert_eq!(
        literal["warnings"],
        json!([]),
        "five views of one frame do not clash"
    );
}

/// Every sprite lump of one frame warns about nothing until the
/// all-angles image or a covered rotation joins in.
#[tokio::test]
async fn a_sprite_import_warns_about_rotations_the_session_already_serves() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "TROO.png", &hole_png());
    let twin = sprite_import(&client, &file, json!({ "name": "TROOA2" })).await;
    let warnings = twin["warnings"].as_array().expect("a report");
    assert_eq!(warnings.len(), 1, "one overlap: {warnings:?}");
    let text = warnings[0].as_str().expect("a message");
    assert!(text.contains("rotation 2"), "names the rotation: {text}");
    assert!(text.contains("TROOA2A8"), "names the lump: {text}");
    let angles = sprite_import(&client, &file, json!({ "name": "PLAYA1" })).await;
    let mixed = angles["warnings"].as_array().expect("a report");
    assert_eq!(mixed.len(), 1, "one mixed frame: {mixed:?}");
    let message = mixed[0].as_str().expect("a message");
    assert!(message.contains("mixes"), "names the mixing: {message}");
    assert!(message.contains("PLAYA0"), "names the lump: {message}");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_sprite TROOA2", "import_sprite PLAYA1"]),
        "warnings never stop an import"
    );
}

/// A same-name working sprite refuses until `replace`, which overwrites
/// in place and never warns about the lump it replaced.
#[tokio::test]
async fn a_sprite_duplicate_needs_replace_and_overwrites_in_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    let first = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
    let (code, message, _) = call_err(
        &client,
        "import_image_as_sprite",
        source_args(&file, json!({ "name": "BOBAA1" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the twin refuses");
    assert!(message.contains("sprites"), "names the region: {message}");
    assert!(
        message.contains("replace=true"),
        "the message names the way out: {message}"
    );
    let replaced =
        sprite_import(&client, &file, json!({ "name": "BOBAA1", "replace": true })).await;
    assert_eq!(
        replaced["lump_index"], first["lump_index"],
        "replace overwrites in place"
    );
    assert_eq!(
        replaced["warnings"],
        json!([]),
        "the copy is not an overlap"
    );
    let listed = call(&client, "list_sprites", json!({ "prefix": "BOBA" })).await;
    let rows = listed["sprites"][0]["frames"][0]["rotations"]
        .as_array()
        .expect("a listing");
    assert_eq!(rows.len(), 1, "one rotation stands, not two");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_sprite BOBAA1", "import_sprite BOBAA1"]),
        "each import is its own undoable edit"
    );
}

/// The sprite naming refusals over one readable `file`.
fn sprite_refusals(file: &Path) -> [Refusal; 8] {
    [
        (
            "too long",
            source_args(file, json!({ "name": "TOOLONG1" })),
            "invalid_argument",
            "sprite lump name",
        ),
        (
            "rotation 9",
            source_args(file, json!({ "name": "TROOA9" })),
            "invalid_argument",
            "rotation digit",
        ),
        (
            "mirror equals rotation",
            source_args(
                file,
                json!({ "sprite": "BOBA", "rotation": 2_u64, "mirror_rotation": 2_u64 }),
            ),
            "invalid_argument",
            "equals rotation",
        ),
        (
            "mirror with rotation 0",
            source_args(
                file,
                json!({ "sprite": "BOBA", "rotation": 0_u64, "mirror_rotation": 8_u64 }),
            ),
            "invalid_argument",
            "no mirror partner",
        ),
        (
            "both forms",
            source_args(file, json!({ "name": "BOBAA1", "sprite": "BOBA" })),
            "invalid_argument",
            "not both",
        ),
        (
            "neither form",
            source_args(file, Value::Null),
            "invalid_argument",
            "needs a sprite",
        ),
        (
            "short prefix",
            source_args(file, json!({ "sprite": "BOB" })),
            "invalid_argument",
            "four ASCII",
        ),
        (
            "bad frame",
            source_args(file, json!({ "sprite": "BOBA", "frame": "AA" })),
            "invalid_argument",
            "frame letter",
        ),
    ]
}

/// Bad sprite names and naming combinations each report
/// `invalid_argument`, and nothing is stored along the way.
#[tokio::test]
async fn bad_sprite_naming_is_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    let cases = sprite_refusals(&file);
    refuse_all(&client, "import_image_as_sprite", &cases).await;
    let listed = call(&client, "list_sprites", json!({ "prefix": "BOBA" })).await;
    assert_eq!(listed["total"], json!(0_u64), "no sprite group landed");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "no edit landed");
}

/// The alias stores the same picture with the same measurements under
/// the parts form too.
#[tokio::test]
async fn the_sprite_alias_imports_the_same_picture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "BOBA.png", &hole_png());
    let main = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
    let alias = call(
        &client,
        "import_png_as_sprite",
        source_args(
            &file,
            json!({ "sprite": "POSS", "rotation": 3_u64, "mirror_rotation": 7_u64 }),
        ),
    )
    .await;
    assert_same_pipeline(&main, &alias);
    assert_naming(&alias, ("POSSA3A7", "POSS", "A", 3));
    assert_eq!(alias["mirror"], json!(7_u64), "and carries the pair");
    assert_eq!(
        sprite_rgba(&client, "BOBAA1").await,
        sprite_rgba(&client, "POSSA3A7").await,
        "the stored pixels match"
    );
}
