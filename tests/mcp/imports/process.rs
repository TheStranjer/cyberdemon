//! The preview pipeline `process_image`: ops with the quantized and
//! `quantize: false` shapes, the op-index errors, and the
//! default-dither preview keeping an asymmetric picture unmirrored.

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::imports::{fetch_asset, flat_png, iwad_client, source_args, write_file};
use crate::support::{call, call_err};

/// The `RGBA` bytes of the published asset at `url`.
async fn asset_png_dimensions(url: &str) -> (u64, u64) {
    let bytes = fetch_asset(url).await;
    let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("the asset is a PNG");
    let width = u64::from(image.width);
    let height = u64::from(image.height);
    (width, height)
}

/// `process_image` over a 100x60 palette picture with `crop` then
/// `fit 64x64`: both assets decode to 64x64, the report has two
/// steps, and the quantized pixels are all fixture-palette colours.
#[tokio::test]
async fn process_image_publishes_processed_and_quantized_previews() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "WALLPHOTO.png",
        &flat_png(100_u32, 60_u32, |_| false),
    );
    let report = call(
        &client,
        "process_image",
        source_args(
            &file,
            json!({
                "ops": [
                    { "op": "crop", "x": 0_u64, "y": 0_u64, "width": 90_u64, "height": 60_u64 },
                    { "op": "fit", "width": 64_u64, "height": 64_u64, "mode": "cover" },
                ],
            }),
        ),
    )
    .await;
    assert_eq!(report["width"], json!(64_u64), "the processed shape");
    assert_eq!(report["height"], json!(64_u64), "the processed shape");
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 2_usize, "crop then fit reported");
    assert_eq!(steps[0]["op"], json!("crop"), "in order");
    assert_eq!(steps[1]["op"], json!("fit"), "in order");
    let processed_url = report["processed"]["url"]
        .as_str()
        .expect("a processed URL");
    assert_eq!(asset_png_dimensions(processed_url).await, (64_u64, 64_u64));
    let quantized_url = report["quantized"]["url"]
        .as_str()
        .expect("a quantized URL");
    assert_eq!(asset_png_dimensions(quantized_url).await, (64_u64, 64_u64));
    let bytes = fetch_asset(quantized_url).await;
    let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("a PNG");
    assert!(
        image.pixels.chunks_exact(4).all(is_ramp_colour),
        "every quantized pixel is a palette colour (i, 255-i, i)"
    );
    assert_eq!(
        report["colors_used"].as_u64().map(|count| count > 0_u64),
        Some(true),
        "the count answers with a number"
    );
    assert!(report["max_color_error"].is_number(), "the error answers");
    assert_eq!(
        report["transparent_pixels"],
        json!(0_u64),
        "an opaque source"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "the preview edits nothing");
}

/// Whether one pixel is a fully opaque fixture-ramp colour `(i, 255-i, i)`.
fn is_ramp_colour(pixel: &[u8]) -> bool {
    let &[red, green, blue, alpha] = pixel else {
        return false;
    };
    alpha == 255_u8 && red.saturating_add(green) == 255_u8 && blue == red
}

/// `quantize: false` publishes only the processed picture and answers
/// the quantization measures with `null`.
#[tokio::test]
async fn process_image_without_quantize_publishes_one_picture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "PLAIN.png",
        &flat_png(100_u32, 60_u32, |_| false),
    );
    let report = call(
        &client,
        "process_image",
        source_args(&file, json!({ "quantize": false })),
    )
    .await;
    assert_eq!(report["quantized"], Value::Null, "no quantized asset");
    assert_eq!(report["colors_used"], Value::Null, "no measure");
    assert_eq!(report["max_color_error"], Value::Null, "no error");
    assert_eq!(report["width"], json!(100_u64), "the untouched size");
    assert!(
        report["steps"].as_array().expect("steps").is_empty(),
        "no ops"
    );
}

/// A bad op parameter fails with `invalid_argument` naming the op
/// index, and the processed asset `URL` stays a valid source for the
/// import tools.
#[tokio::test]
async fn process_image_names_the_failing_op_and_its_output_reimports() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "PLAIN.png",
        &flat_png(100_u32, 60_u32, |_| false),
    );
    let (code, message, _) = call_err(
        &client,
        "process_image",
        source_args(
            &file,
            json!({
                "ops": [
                    { "op": "fit", "width": 32_u64, "height": 32_u64, "mode": "cover" },
                    { "op": "crop", "x": 0_u64, "y": 0_u64, "width": 0_u64, "height": 4_u64 },
                ],
            }),
        ),
    )
    .await;
    assert_eq!(code, "invalid_argument", "the op refuses");
    assert!(
        message.contains("ops[1]"),
        "the message names the op index: {message}"
    );
    let preview = call(&client, "process_image", source_args(&file, Value::Null)).await;
    let url = preview["processed"]["url"].as_str().expect("a URL");
    let imported = call(
        &client,
        "import_image_as_patch",
        json!({ "source": url, "name": "FROMPREV" }),
    )
    .await;
    assert_eq!(imported["width"], json!(100_u64), "the asset URL reimports");
}

/// The source pixel at raster position `p` of [`weighted_png`]:
/// bright grey in the left half of a 32-pixel row, dark grey in the
/// right.
fn weighted_pixel(p: usize) -> [u8; 4] {
    let x = p.checked_rem(32).expect("test rows");
    let shade = if x < 16 { 240_u8 } else { 16_u8 };
    [shade, shade, shade, 255_u8]
}

/// A left-weighted 32x8 opaque PNG: columns 0..16 are bright grey,
/// columns 16..32 dark grey.
fn weighted_png() -> Vec<u8> {
    let pixels: Vec<u8> = (0..256_usize).flat_map(weighted_pixel).collect();
    let img = RgbaImage::new(32, 8, pixels).expect("test shape is valid");
    encode_png(&img, None).expect("test PNG encodes")
}

/// The red-channel sum of the `RGBA` pixels of `line` in `range`.
fn red_sum(line: &[u8], range: core::ops::Range<usize>) -> u32 {
    line[range]
        .chunks_exact(4)
        .map(|pixel| u32::from(pixel[0]))
        .sum()
}

/// The red-channel sums of the left and right halves of one 32-pixel
/// `RGBA` row of the quantized preview.
fn half_reds(line: &[u8]) -> (u32, u32) {
    (red_sum(line, 0..64), red_sum(line, 64..128))
}

/// The raster rows that are not brighter on the left than on the
/// right, over 32-pixel `RGBA` rows.
fn unweighted_rows(pixels: &[u8]) -> Vec<usize> {
    pixels
        .chunks_exact(128)
        .enumerate()
        .filter_map(|(row, line)| {
            let (left, right) = half_reds(line);
            (left <= right).then_some(row)
        })
        .collect()
}

/// The default dither (`floyd_steinberg`) writes every pixel at its
/// own position: each row of the quantized preview stays brighter on
/// the left than on the right, where the source is, so no serpentine
/// row comes out mirrored.
#[tokio::test]
async fn process_image_dithering_keeps_the_picture_left_weighted() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "WEIGHT.png", &weighted_png());
    let report = call(&client, "process_image", source_args(&file, Value::Null)).await;
    assert_eq!(report["width"], json!(32_u64), "the untouched shape");
    let url = report["quantized"]["url"]
        .as_str()
        .expect("a quantized URL");
    let bytes = fetch_asset(url).await;
    let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("a PNG");
    assert_eq!(
        (image.width, image.height),
        (32_u32, 8_u32),
        "the quantized shape stands"
    );
    assert_eq!(
        unweighted_rows(&image.pixels),
        Vec::<usize>::new(),
        "every row of the quantized preview stays left-weighted"
    );
}
