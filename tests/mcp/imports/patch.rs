//! The shared image pipeline as `import_image_as_patch` and its alias
//! `import_png_as_patch`: exact-palette quantization with an alpha hole,
//! colour-key and opaque transparency, `grAb`/preset/explicit offsets,
//! the `PNAMES` listing and its opt-out, the duplicate and `replace`
//! rule, and undo. The cyan test picture and the twin-import wrappers
//! the alias checks run through live here with the tests.

use std::path::Path;

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::imports::{
    Client, FIXTURE_PNAMES, HOLE, PIXELS, SIDE, assert_same_pipeline, expected_rgba, hole_png,
    iwad_client, palette_png, patch_rgba, source_args, write_file,
};
use crate::support::{call, call_err};

/// The cyan the colour key defaults to.
const CYAN: [u8; 4] = [0_u8, 255, 255, 255];

/// The 8x8 fully-opaque cyan PNG.
fn cyan_png() -> Vec<u8> {
    let pixels: Vec<u8> = vec![CYAN; PIXELS].concat();
    let img = RgbaImage::new(SIDE, SIDE, pixels).expect("test shape is valid");
    encode_png(&img, None).expect("test PNGs encode")
}

/// The alpha-hole round-trip: exact palette colours import as a patch
/// whose decoded pixels are the source's, hole included.
#[tokio::test]
async fn an_exact_png_imports_as_a_patch_that_decodes_back_identically() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "MYART.png", &hole_png());
    let report = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "MYART" })),
    )
    .await;
    assert_eq!(report["name"], json!("MYART"), "the name echoes");
    assert_eq!(
        report["lump_index"],
        json!(1_u64),
        "after the created P_START"
    );
    assert_eq!(
        report["left_offset"],
        json!(0_i64),
        "the wall-patch default without grAb"
    );
    assert_eq!(report["top_offset"], json!(0_i64), "zero both ways");
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
    assert_eq!(
        report["pnames_index"],
        json!(FIXTURE_PNAMES),
        "the new PNAMES entry follows the fixture pair"
    );
    assert_eq!(patch_rgba(&client, "MYART").await, expected_rgba());
    let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
    assert_eq!(listed["total"], json!(1_u64), "list_patches shows it");
    assert_eq!(
        listed["patches"][0]["namespace"],
        json!("patches"),
        "in the region"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_patch MYART"]),
        "one undoable edit labelled as documented"
    );
}

/// The alias stores the same picture with the same measurements.
#[tokio::test]
async fn the_alias_imports_the_same_picture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "MYART.png", &hole_png());
    let main = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "MAIN" })),
    )
    .await;
    let alias = call(
        &client,
        "import_png_as_patch",
        source_args(&file, json!({ "name": "ALIAS" })),
    )
    .await;
    assert_same_pipeline(&main, &alias);
    assert_eq!(
        alias["pnames_index"],
        json!(3_u64),
        "and lists in PNAMES too"
    );
    assert_eq!(
        patch_rgba(&client, "ALIAS").await,
        patch_rgba(&client, "MAIN").await,
        "the stored bytes match"
    );
}

/// Colour-key mode drops exactly the key colour, default and explicit.
#[tokio::test]
async fn the_colour_key_mode_drops_the_key_colour() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "CYAN.png", &cyan_png());
    let keyed = call(
        &client,
        "import_image_as_patch",
        source_args(
            &file,
            json!({ "name": "KEYED", "transparency": { "mode": "color" } }),
        ),
    )
    .await;
    assert_eq!(
        keyed["transparent_pixels"],
        json!(64_u64),
        "the default cyan key empties the picture"
    );
    let rgba = patch_rgba(&client, "KEYED").await;
    assert_eq!(
        rgba.len(),
        PIXELS.checked_mul(4).expect("rgba size"),
        "the shape"
    );
    assert!(
        rgba.chunks_exact(4).all(|pixel| pixel[3] == 0_u8),
        "every pixel is a hole"
    );
    let explicit = call(
        &client,
        "import_image_as_patch",
        source_args(
            &file,
            json!({
                "name": "EXPLICIT",
                "transparency": { "mode": "color", "color": "#00FFFF" },
            }),
        ),
    )
    .await;
    assert_eq!(
        explicit["transparent_pixels"],
        json!(64_u64),
        "the explicit key matches the default"
    );
}

/// `none` keeps every pixel opaque, thresholds included.
#[tokio::test]
async fn opaque_mode_keeps_everything() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "CYAN.png", &cyan_png());
    let opaque = call(
        &client,
        "import_image_as_patch",
        source_args(
            &file,
            json!({ "name": "OPAQUE", "transparency": { "mode": "none" } }),
        ),
    )
    .await;
    assert_eq!(
        opaque["transparent_pixels"],
        json!(0_u64),
        "none keeps everything"
    );
    assert_eq!(opaque["colors_used"], json!(1_u64), "one distinct colour");
    let solid = patch_rgba(&client, "OPAQUE").await;
    assert!(
        solid.chunks_exact(4).all(|pixel| pixel[3] == 255_u8),
        "all opaque"
    );
    let hole = write_file(
        dir.path(),
        "SOFT.png",
        &palette_png(SIDE, SIDE, Some(HOLE), None),
    );
    let strict = call(
        &client,
        "import_image_as_patch",
        source_args(
            &hole,
            json!({ "name": "STRICT", "transparency": { "mode": "alpha", "threshold": 0_u64 } }),
        ),
    )
    .await;
    assert_eq!(
        strict["transparent_pixels"],
        json!(0_u64),
        "threshold 0 drops nothing"
    );
}

/// Offsets default to the `grAb` chunk and `"zero"` overrides it.
#[tokio::test]
async fn offsets_default_to_the_grab_chunk() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let grabbed = write_file(
        dir.path(),
        "GRABBED.png",
        &palette_png(SIDE, SIDE, None, Some((7_i32, -9_i32))),
    );
    let taken = call(
        &client,
        "import_image_as_patch",
        source_args(&grabbed, json!({ "name": "TAKEN" })),
    )
    .await;
    assert_eq!(taken["left_offset"], json!(7_i64), "grAb by default");
    assert_eq!(taken["top_offset"], json!(-9_i64), "grAb by default");
    let off = call(
        &client,
        "import_image_as_patch",
        source_args(&grabbed, json!({ "name": "IGNORED", "offsets": "zero" })),
    )
    .await;
    assert_eq!(
        off["left_offset"],
        json!(0_i64),
        "\"zero\" outranks the chunk"
    );
    assert_eq!(off["top_offset"], json!(0_i64), "\"zero\" both ways");
    let plain = write_file(
        dir.path(),
        "PLAIN.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let fallback = call(
        &client,
        "import_image_as_patch",
        source_args(&plain, json!({ "name": "FALLBACK", "offsets": "grab" })),
    )
    .await;
    assert_eq!(
        fallback["left_offset"],
        json!(0_i64),
        "grab without a chunk uses the tool default"
    );
}

/// The presets and the explicit pair place the origin as documented.
#[tokio::test]
async fn offset_presets_and_literals_place_the_origin() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let plain = write_file(
        dir.path(),
        "PLAIN.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let bottom = call(
        &client,
        "import_image_as_patch",
        source_args(
            &plain,
            json!({ "name": "BOTTOM", "offsets": "center_bottom" }),
        ),
    )
    .await;
    assert_eq!(bottom["left_offset"], json!(4_i64), "width/2");
    assert_eq!(bottom["top_offset"], json!(8_i64), "full height");
    let middle = call(
        &client,
        "import_image_as_patch",
        source_args(&plain, json!({ "name": "MIDDLE", "offsets": "center" })),
    )
    .await;
    assert_eq!(middle["left_offset"], json!(4_i64), "width/2");
    assert_eq!(middle["top_offset"], json!(4_i64), "height/2");
    let literal = call(
        &client,
        "import_image_as_patch",
        source_args(
            &plain,
            json!({ "name": "LITERAL", "offsets": { "x": -2_i64, "y": 3_i64 } }),
        ),
    )
    .await;
    assert_eq!(literal["left_offset"], json!(-2_i64), "the literal left");
    assert_eq!(literal["top_offset"], json!(3_i64), "the literal top");
}

/// `PNAMES` grows with the import unless the tool opts out.
#[tokio::test]
async fn pnames_lists_the_import_and_opt_out_leaves_it_alone() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "LISTED.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let listed = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "LISTED" })),
    )
    .await;
    assert_eq!(
        listed["pnames_index"],
        json!(2_u64),
        "appended after the fixture pair"
    );
    let after = call(&client, "list_pnames", json!({})).await;
    assert_eq!(after["total"], json!(3_u64), "PNAMES grew by one");
    assert_eq!(after["pnames"][2]["name"], json!("LISTED"), "the new entry");
    let skipped = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "UNLISTED", "add_to_pnames": false })),
    )
    .await;
    assert_eq!(skipped["pnames_index"], Value::Null, "opted out");
    let still = call(&client, "list_pnames", json!({})).await;
    assert_eq!(still["total"], json!(3_u64), "PNAMES did not grow");
    assert_eq!(
        still["pnames"][2]["name"],
        json!("LISTED"),
        "no UNLISTED entry"
    );
}

/// A name `PNAMES` already lists keeps its index and changes nothing.
#[tokio::test]
async fn an_already_listed_name_keeps_its_pnames_entry() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "WALLP1.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let reused = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "WALLP1" })),
    )
    .await;
    assert_eq!(
        reused["pnames_index"],
        json!(0_u64),
        "a name PNAMES already lists keeps its index"
    );
    let unchanged = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        unchanged["total"],
        json!(FIXTURE_PNAMES),
        "the reused name adds nothing"
    );
}

/// A same-name working lump refuses until `replace`, and each import
/// undoes separately.
#[tokio::test]
async fn a_duplicate_needs_replace_and_overwrites_in_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "TWIN.png", &palette_png(SIDE, SIDE, None, None));
    let first = import_twin(&client, &file, json!({ "name": "TWIN" })).await;
    let (code, message) = import_twin_err(&client, &file, json!({ "name": "TWIN" })).await;
    assert_eq!(code, "already_exists", "the twin refuses");
    assert!(
        message.contains("replace=true"),
        "the message names the way out: {message}"
    );
    let replaced = import_twin(&client, &file, json!({ "name": "TWIN", "replace": true })).await;
    assert_eq!(
        replaced["lump_index"], first["lump_index"],
        "replace overwrites in place"
    );
    assert_eq!(
        replaced["pnames_index"],
        json!(2_u64),
        "the entry stays put"
    );
    let working = call(
        &client,
        "list_lumps",
        json!({ "scope": "working", "namespace": "patches" }),
    )
    .await;
    let twins = working["lumps"]
        .as_array()
        .expect("a listing")
        .iter()
        .filter(|row| row["name"] == json!("TWIN"))
        .count();
    assert_eq!(twins, 1, "one copy stands");
}

/// One successful `import_image_as_patch` of `file` with `extra`.
async fn import_twin(client: &Client, file: &Path, extra: Value) -> Value {
    call(client, "import_image_as_patch", source_args(file, extra)).await
}

/// One failing `import_image_as_patch` of `file` with `extra`.
async fn import_twin_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
    let (code, message, _) =
        call_err(client, "import_image_as_patch", source_args(file, extra)).await;
    (code, message)
}

/// Undo steps each import back separately, the `PNAMES` override
/// included.
#[tokio::test]
async fn undo_reverts_the_import_and_its_pnames_write() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "TWIN.png", &palette_png(SIDE, SIDE, None, None));
    call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "TWIN" })),
    )
    .await;
    let replaced = call(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "TWIN", "replace": true })),
    )
    .await;
    assert_eq!(
        replaced["pnames_index"],
        json!(2_u64),
        "the second edit landed"
    );
    let undone = call(&client, "undo", json!({ "steps": 2_u64 })).await;
    assert_eq!(
        undone["undone"],
        json!(["import_patch TWIN", "import_patch TWIN"]),
        "both imports undo separately"
    );
    let gone = call_err(&client, "get_patch", json!({ "name": "TWIN" })).await;
    assert_eq!(gone.0, "not_found", "the lump left with its edit");
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "the PNAMES override undid too"
    );
}
