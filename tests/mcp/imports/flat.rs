//! The flat importer `import_image_as_flat` with its alias
//! `import_png_as_flat`: exact 64x64 round trips, nearest resizing to
//! 64 and 128, the echoed dither mode over the three quantizers, the
//! 256 scale with its echoed size, the `IWAD` override, the duplicate
//! and `replace` rule, ignored-and-counted alpha, and its refusal
//! catalogue. The flat raster builders, the grey dither fixtures and
//! the shape and alias assertions live here with the tests.

use std::path::Path;

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::{RgbaImage, resize_nearest};
use serde_json::{Value, json};

use crate::imports::{
    Client, FIXTURE_FLATS, Refusal, color_pixel, cycled, flat_expected_rgba, flat_png, flat_rgba,
    iwad_client, refuse_all, source_args, write_file,
};
use crate::support::{call, call_err, connect};

/// Pixels on a side of the 64x64 test flats.
const FLAT_SIDE: u32 = 64_u32;

/// Pixels in the 64x64 test flats.
const FLAT_PIXELS: usize = 4_096;

/// The `width`x`height` fully opaque raster cycling palette colours.
fn flat_raster(width: u32, height: u32) -> RgbaImage {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count).flat_map(|p| color_pixel(cycled(p))).collect();
    RgbaImage::new(width, height, pixels).expect("test shapes are valid")
}

/// One successful `import_image_as_flat` of `file` with `extra`.
async fn flat_import(client: &Client, file: &Path, extra: Value) -> Value {
    call(client, "import_image_as_flat", source_args(file, extra)).await
}

/// One failing `import_image_as_flat` of `file` with `extra`.
async fn flat_import_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
    let (code, message, _) =
        call_err(client, "import_image_as_flat", source_args(file, extra)).await;
    (code, message)
}

/// Assert a flat report stores the given shape and resize verdict.
fn assert_flat_shape(report: &Value, width: u64, height: u64, resized: bool) {
    assert_eq!(report["width"], json!(width), "the stored width");
    assert_eq!(report["height"], json!(height), "the stored height");
    assert_eq!(report["resized"], json!(resized), "the resize verdict");
}

/// Assert two flat-import reports agree on every measured field.
fn assert_same_flat(main: &Value, alias: &Value) {
    for field in [
        "width",
        "height",
        "size",
        "dither",
        "resized",
        "colors_used",
        "max_color_error",
        "transparent_pixels_ignored",
    ] {
        assert_eq!(
            alias[field], main[field],
            "the alias reports {field} identically"
        );
    }
}

/// The 64x64 exact-palette round trip: the flat stores the picture and
/// `get_flat` answers the pixels back unchanged.
#[tokio::test]
async fn an_exact_flat_png_imports_and_reads_back_pixel_perfect() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "MYFLAT.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let report = flat_import(&client, &file, json!({ "name": "MYFLAT" })).await;
    assert_eq!(report["name"], json!("MYFLAT"), "the name echoes");
    assert_eq!(
        report["lump_index"],
        json!(1_u64),
        "after the created F_START"
    );
    assert_flat_shape(&report, 64_u64, 64_u64, false);
    assert_eq!(report["size"], json!(64_u64), "the default side echoes");
    assert_eq!(
        report["dither"],
        json!("floyd_steinberg"),
        "the default dither echoes and is process_image's"
    );
    assert_eq!(
        report["colors_used"],
        json!(256_u64),
        "every palette colour"
    );
    assert_eq!(
        report["max_color_error"],
        json!(0_i64),
        "the palette is exact"
    );
    assert_eq!(
        report["transparent_pixels_ignored"],
        json!(0_u64),
        "an opaque source"
    );
    assert_eq!(
        flat_rgba(&client, "MYFLAT").await,
        flat_expected_rgba(FLAT_PIXELS),
        "the same pixels back"
    );
    let listed = call(&client, "list_flats", json!({ "pattern": "MYFLAT" })).await;
    assert_eq!(listed["total"], json!(1_u64), "list_flats shows it");
    assert_eq!(
        listed["flats"][0]["source"],
        json!("working"),
        "in the working archive"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_flat MYFLAT"]),
        "one undoable edit labelled as documented"
    );
}

/// A 100x50 picture without `resize` refuses, naming the shapes and the
/// option, and stores nothing.
#[tokio::test]
async fn an_odd_sized_picture_without_resize_is_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 50_u32, |_| false));
    let (code, message) = flat_import_err(&client, &file, json!({ "name": "ODD" })).await;
    assert_eq!(code, "invalid_argument", "no resize, no flat");
    assert!(
        message.contains("64x128"),
        "the message names the shapes: {message}"
    );
    assert!(
        message.contains("resize"),
        "the message names the option: {message}"
    );
    let listed = call(&client, "list_flats", json!({ "pattern": "ODD" })).await;
    assert_eq!(listed["total"], json!(0_u64), "nothing was stored");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "no edit landed");
}

/// `resize: "nearest"` scales to 64x64 by default and to 128x128 when
/// asked, storing the nearest samples; a 64x128 picture is kept.
#[tokio::test]
async fn resizing_nearest_scales_the_picture_to_the_square_size() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 50_u32, |_| false));
    let small = flat_import(
        &client,
        &file,
        json!({ "name": "SMALL", "resize": "nearest" }),
    )
    .await;
    assert_flat_shape(&small, 64_u64, 64_u64, true);
    let scaled = resize_nearest(&flat_raster(100_u32, 50_u32), FLAT_SIDE, FLAT_SIDE);
    assert_eq!(flat_rgba(&client, "SMALL").await, scaled.pixels);
    let big = flat_import(
        &client,
        &file,
        json!({ "name": "BIG", "resize": "nearest", "size": 128_u64 }),
    )
    .await;
    assert_flat_shape(&big, 128_u64, 128_u64, true);
    let flats = call(&client, "list_flats", json!({ "pattern": "BIG" })).await;
    assert_eq!(
        flats["flats"][0]["size"],
        json!(0x4000_u64),
        "the raw 128x128 byte count"
    );
    let tall = write_file(
        dir.path(),
        "TALL.png",
        &flat_png(FLAT_SIDE, 128_u32, |_| false),
    );
    let kept = flat_import(&client, &tall, json!({ "name": "TALL" })).await;
    assert_flat_shape(&kept, 64_u64, 128_u64, false);
}

/// A flat only the `IWAD` holds does not clash: the import lands as a
/// working override the merged view resolves to.
#[tokio::test]
async fn a_flat_import_overrides_the_iwad_copy() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "FLOOR1.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let report = flat_import(&client, &file, json!({ "name": "FLOOR1" })).await;
    assert_eq!(report["name"], json!("FLOOR1"), "the override imports");
    assert_eq!(report["lump_index"], json!(1_u64), "in the created region");
    let listed = call(&client, "list_flats", json!({ "pattern": "FLOOR1" })).await;
    assert_eq!(
        listed["total"],
        json!(1_u64),
        "last wins in the merged view"
    );
    assert_eq!(
        listed["flats"][0]["source"],
        json!("working"),
        "the working copy serves"
    );
    assert_eq!(
        flat_rgba(&client, "FLOOR1").await,
        flat_expected_rgba(FLAT_PIXELS),
        "the new pixels serve"
    );
}

/// A same-name working flat refuses until `replace`, which overwrites
/// in place.
#[tokio::test]
async fn a_flat_duplicate_needs_replace_and_overwrites_in_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "TWIN.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let first = flat_import(&client, &file, json!({ "name": "TWIN" })).await;
    let (code, message) = flat_import_err(&client, &file, json!({ "name": "TWIN" })).await;
    assert_eq!(code, "already_exists", "the twin refuses");
    assert!(message.contains("flats"), "names the region: {message}");
    assert!(
        message.contains("replace=true"),
        "the message names the way out: {message}"
    );
    let replaced = flat_import(&client, &file, json!({ "name": "TWIN", "replace": true })).await;
    assert_eq!(
        replaced["lump_index"], first["lump_index"],
        "replace overwrites in place"
    );
    let listed = call(&client, "list_flats", json!({ "pattern": "TWIN" })).await;
    assert_eq!(listed["total"], json!(1_u64), "one copy stands");
}

/// The alias stores the same flat with the same measurements.
#[tokio::test]
async fn the_flat_alias_imports_the_same_picture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "MYFLAT.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let main = flat_import(&client, &file, json!({ "name": "MAIN" })).await;
    let alias = call(
        &client,
        "import_png_as_flat",
        source_args(&file, json!({ "name": "ALIAS" })),
    )
    .await;
    assert_same_flat(&main, &alias);
    assert_eq!(
        flat_rgba(&client, "ALIAS").await,
        flat_rgba(&client, "MAIN").await,
        "the stored pixels match"
    );
}

/// Alpha below 128 is counted, kept opaque, and stored at its colour.
#[tokio::test]
async fn flat_imports_keep_alpha_pixels_and_count_them() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let picked = |p: usize| p.checked_rem(7_usize).is_some_and(|rest| rest == 0_usize);
    let expected = (0..FLAT_PIXELS).filter(|p| picked(*p)).count();
    let file = write_file(
        dir.path(),
        "SEMI.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, picked),
    );
    let report = flat_import(&client, &file, json!({ "name": "SEMI" })).await;
    assert_eq!(
        report["transparent_pixels_ignored"],
        json!(u64::try_from(expected).expect("test count fits")),
        "every seventh pixel had alpha below 128"
    );
    assert_eq!(
        report["colors_used"],
        json!(256_u64),
        "the alpha carried no colour information away"
    );
    assert_eq!(
        flat_rgba(&client, "SEMI").await,
        flat_expected_rgba(FLAT_PIXELS),
        "all pixels stored opaque at their colours"
    );
}

/// A `size`x`size` opaque picture of one grey, off the fixture ramp.
fn grey_png(size: u32, level: u8) -> Vec<u8> {
    let count = usize::try_from(size)
        .expect("test size")
        .checked_mul(usize::try_from(size).expect("test size"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count)
        .flat_map(|_| [level, level, level, 255_u8])
        .collect();
    let img = RgbaImage::new(size, size, pixels).expect("test shapes are valid");
    encode_png(&img, None).expect("test PNGs encode")
}

/// Whether every pixel of a fetched `RGBA` picture is one colour.
fn one_colour(rgba: &[u8]) -> bool {
    let first = rgba.chunks_exact(4).next().expect("a non-empty picture");
    rgba.chunks_exact(4).all(|pixel| pixel == first)
}

/// One grey flat imported in dither mode `mode` under `lump`, asserting
/// the `dither` and `size` echoes and answering with its stored pixels.
async fn grey_flat_in(client: &Client, file: &Path, mode: &str, lump: &str) -> Vec<u8> {
    let report = flat_import(client, file, json!({ "name": lump, "dither": mode })).await;
    assert_eq!(report["dither"], json!(mode), "{mode} echoes");
    assert_eq!(report["size"], json!(64_u64), "{mode} echoes its side too");
    flat_rgba(client, lump).await
}

/// `dither` decides how an off-palette picture is quantized and the
/// result echoes the mode: `none` keeps one flat colour, both dither
/// modes spread it, and leaving `dither` out is `process_image`'s
/// `floyd_steinberg`.
#[tokio::test]
async fn a_flat_import_quantizes_in_the_echoed_dither_mode() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "GREY.png", &grey_png(FLAT_SIDE, 90_u8));
    let plain = grey_flat_in(&client, &file, "none", "DNONE").await;
    let ordered = grey_flat_in(&client, &file, "ordered", "DORDER").await;
    let floyd = grey_flat_in(&client, &file, "floyd_steinberg", "DFLOYD").await;
    assert!(one_colour(&plain), "nearest keeps the flat grey one colour");
    assert!(!one_colour(&ordered), "ordered spreads the grey");
    assert!(!one_colour(&floyd), "floyd_steinberg spreads the grey");
    assert_ne!(plain, floyd, "the mode changes the stored pixels");
    let echo = flat_import(&client, &file, json!({ "name": "DDEFAULT" })).await;
    assert_eq!(
        echo["dither"],
        json!("floyd_steinberg"),
        "the default is process_image's"
    );
    assert_eq!(
        flat_rgba(&client, "DDEFAULT").await,
        floyd,
        "the default quantizes like an explicit floyd_steinberg"
    );
}

/// The `size` parameter scales to 256 too, and the result echoes it
/// beside the stored shape.
#[tokio::test]
async fn a_flat_import_scales_to_256_and_echoes_the_size() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 50_u32, |_| false));
    let report = flat_import(
        &client,
        &file,
        json!({ "name": "HIRES", "resize": "nearest", "size": 256_u64, "dither": "none" }),
    )
    .await;
    assert_flat_shape(&report, 256_u64, 256_u64, true);
    assert_eq!(report["size"], json!(256_u64), "the requested side echoes");
    assert_eq!(report["dither"], json!("none"), "the passed mode echoes");
    let flats = call(&client, "list_flats", json!({ "pattern": "HIRES" })).await;
    assert_eq!(
        flats["flats"][0]["size"],
        json!(0x1_0000_u64),
        "the raw 256x256 byte count"
    );
    let expected = resize_nearest(&flat_raster(100_u32, 50_u32), 256_u32, 256_u32);
    assert_eq!(
        flat_rgba(&client, "HIRES").await,
        expected.pixels,
        "the nearest samples landed"
    );
}

/// Bad flat sources and parameters each report their code, and nothing
/// is stored along the way.
#[tokio::test]
async fn bad_flat_sources_and_parameters_are_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let good = write_file(
        dir.path(),
        "GOOD.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
    let unnamed = write_file(
        dir.path(),
        "###.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let cases: [Refusal; 6] = [
        (
            "missing",
            json!({ "source": dir.path().join("absent.png").display().to_string() }),
            "not_found",
            "absent.png",
        ),
        (
            "junk",
            source_args(&junk, Value::Null),
            "format_error",
            "image",
        ),
        (
            "unnamed",
            source_args(&unnamed, Value::Null),
            "invalid_argument",
            "derive",
        ),
        (
            "resize",
            source_args(&junk, json!({ "name": "X", "resize": "linear" })),
            "invalid_argument",
            "\"nearest\"",
        ),
        (
            "size",
            source_args(
                &junk,
                json!({ "name": "X", "resize": "nearest", "size": 32_u64 }),
            ),
            "invalid_argument",
            "64, 128, 256",
        ),
        (
            "palette",
            source_args(&good, json!({ "name": "X", "palette": 99_u64 })),
            "not_found",
            "PLAYPAL",
        ),
    ];
    refuse_all(&client, "import_image_as_flat", &cases).await;
    let flats = call(&client, "list_flats", json!({})).await;
    assert_eq!(flats["total"], json!(FIXTURE_FLATS), "nothing was stored");
}

/// Without a `PLAYPAL` the quantization step has nothing to answer.
#[tokio::test]
async fn a_flat_import_without_a_playpal_cannot_quantize() {
    let client = connect().await;
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = write_file(
        dir.path(),
        "ART.png",
        &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
    );
    let (code, message) = flat_import_err(&client, &file, json!({ "name": "ART" })).await;
    assert_eq!(code, "not_found", "no palette to quantize against");
    assert!(
        message.contains("PLAYPAL"),
        "the message names it: {message}"
    );
}
