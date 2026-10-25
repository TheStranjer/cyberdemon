//! The preview pipeline `process_image`: ops with the quantized and
//! `quantize: false` shapes, the `grayscale`/`blur` building blocks, the
//! `threshold`-then-`thin_stroke_filter` hairline pass, the
//! `adaptive_threshold` uneven-light binarization, the
//! `perspective_correct` quad straightening, the op-index errors, and the
//! default-dither preview keeping an asymmetric picture unmirrored.

use std::path::Path;

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::imports::{PIXELS, SIDE, fetch_asset, flat_png, iwad_client, source_args, write_file};
use crate::support::{asset_png, call, call_err, fill_rect, paper_sheet};

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

/// A `SIDE` x `SIDE` opaque PNG of one colour.
fn solid_png(color: [u8; 4]) -> Vec<u8> {
    let pixels: Vec<u8> = (0..PIXELS).flat_map(|_| color).collect();
    let img = RgbaImage::new(SIDE, SIDE, pixels).expect("test shape is valid");
    encode_png(&img, None).expect("test PNG encodes")
}

/// `process_image` with `grayscale` then `blur`: both steps report
/// their measures, and a single-colour opaque picture — the red,
/// luminance `124` — stays that exact grey through both.
#[tokio::test]
async fn process_image_grayscale_and_blur_keep_a_flat_picture_flat() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FLATRED.png", &solid_png([200, 100, 50, 255]));
    let report = call(
        &client,
        "process_image",
        source_args(
            &file,
            json!({
                "ops": [
                    { "op": "grayscale" },
                    { "op": "blur", "radius": 2_u64 },
                ],
            }),
        ),
    )
    .await;
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 2_usize, "grayscale then blur reported");
    assert_eq!(steps[0]["op"], json!("grayscale"), "in order");
    assert_eq!(
        steps[0]["distinct_colors_before"],
        json!(1_u64),
        "one colour"
    );
    assert_eq!(steps[0]["distinct_colors_after"], json!(1_u64), "one grey");
    assert_eq!(steps[1]["op"], json!("blur"), "the blur step");
    assert_eq!(steps[1]["radius"], json!(2_u64), "the radius echoes");
    let url = report["processed"]["url"]
        .as_str()
        .expect("a processed URL");
    let bytes = fetch_asset(url).await;
    let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("the asset is a PNG");
    assert_eq!((image.width, image.height), (SIDE, SIDE), "no shape change");
    assert!(
        image
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [124_u8, 124, 124, 255]),
        "the uniform luminance-124 grey stays exact through both ops",
    );
}

/// A `blur` radius outside `1..=256` refuses through `process_image`,
/// naming `ops[0].radius`.
#[tokio::test]
async fn process_image_names_the_field_of_a_blur_out_of_range() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "FLATRED.png", &solid_png([200, 100, 50, 255]));
    let (code, message, _) = call_err(
        &client,
        "process_image",
        source_args(&file, json!({ "ops": [{ "op": "blur", "radius": 0_u64 }] })),
    )
    .await;
    assert_eq!(code, "invalid_argument", "radius 0 refuses");
    assert!(
        message.contains("ops[0].radius") && message.contains("1..=256"),
        "the message names the field and range: {message}"
    );
}

/// A `12` x `12` opaque `PNG`: white paper carrying a `1` px grey row
/// (row `2`) and a `4` px grey band (rows `5..=8`).
fn ruling_png() -> Vec<u8> {
    let mut img =
        RgbaImage::new(12_u32, 12_u32, vec![255_u8; 576_usize]).expect("a 12x12 RGBA8 is valid");
    let hairline = (0..12_u32).map(|x| (x, 2_u32));
    let band = (0..12_u32).flat_map(|x| (5..9_u32).map(move |y| (x, y)));
    for (x, y) in hairline.chain(band) {
        assert!(img.set(x, y, [100_u8, 100, 100, 255]), "fixture in bounds");
    }
    encode_png(&img, None).expect("test PNG encodes")
}

/// `process_image` with `threshold` then `thin_stroke_filter` at radius
/// `1`: the hairline row comes back white paper, the `4` px band black
/// ink, and the step carries the two ink counts.
#[tokio::test]
async fn process_image_thin_stroke_filter_drops_the_hairline_row() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "RULING.png", &ruling_png());
    let report = call(
        &client,
        "process_image",
        source_args(
            &file,
            json!({
                "ops": [
                    { "op": "threshold" },
                    { "op": "thin_stroke_filter", "radius": 1_u64 },
                ],
            }),
        ),
    )
    .await;
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 2_usize, "threshold then the filter");
    assert_eq!(steps[1]["op"], json!("thin_stroke_filter"), "the op name");
    assert_eq!(steps[1]["radius"], json!(1_u64), "the radius echoes");
    assert_eq!(steps[1]["otsu"], json!(true), "level omitted is Otsu");
    assert_eq!(
        steps[1]["removed_ink_pixels"],
        json!(12_u64),
        "the 12-pixel hairline was dropped"
    );
    assert_eq!(
        steps[1]["kept_ink_pixels"],
        json!(48_u64),
        "the 4 px band kept its 48 ink pixels"
    );
    let url = report["processed"]["url"]
        .as_str()
        .expect("a processed URL");
    let image = asset_png(url).await;
    assert_eq!(
        (image.width, image.height),
        (12_u32, 12_u32),
        "no shape change"
    );
    assert!(
        (0..12_u32).all(|x| image.get(x, 2) == Some([255_u8, 255, 255, 255])),
        "the hairline row is white paper now"
    );
    assert!(
        (0..12_u32)
            .flat_map(|x| (5..9_u32).map(move |y| (x, y)))
            .all(|(x, y)| image.get(x, y) == Some([0_u8, 0, 0, 255])),
        "the 4 px band is black ink still"
    );
    assert_eq!(
        image.get(0, 0),
        Some([255_u8, 255, 255, 255]),
        "paper stays paper"
    );
}

/// The fixture's grey at `(x, y)`: the square's `20` inside
/// `13..=17 x 8..=12`, the `3`-per-column white-to-grey ramp elsewhere.
fn gradient_level(x: u32, y: u32) -> u8 {
    let inside = (13..=17_u32).contains(&x) && (8..=12_u32).contains(&y);
    let stepped = x.checked_mul(3).expect("fixture fits");
    let ramp = 255_u32.saturating_sub(stepped);
    let value = if inside { 20_u32 } else { ramp };
    u8::try_from(value).expect("ramp fits u8")
}

/// A `40` x `24` opaque `PNG`: a horizontal white-to-grey luminance
/// ramp (`255` at the left edge, `138` at the right) carrying a
/// `5` x `5` dark square (luminance `20`) at columns `13..=17`,
/// rows `8..=12`.
fn gradient_rect_png() -> Vec<u8> {
    let mut img =
        RgbaImage::new(40_u32, 24_u32, vec![255_u8; 3_840_usize]).expect("a 40x24 RGBA8 is valid");
    let points = (0..24_u32).flat_map(|y| (0..40_u32).map(move |x| (x, y)));
    for (x, y) in points {
        let shade = gradient_level(x, y);
        assert!(
            img.set(x, y, [shade, shade, shade, 255_u8]),
            "fixture in bounds"
        );
    }
    encode_png(&img, None).expect("test PNG encodes")
}

/// Asserts on the processed gradient picture: the `5 x 5` square is
/// opaque black ink everywhere, from the gradient's bright end to its
/// dark one, and the sampled background pixels are opaque white paper.
fn assert_adaptive_ink_and_paper(image: &RgbaImage) {
    let square = (8..=12_u32).flat_map(|y| (13..=17_u32).map(move |x| (x, y)));
    let unpicked: Vec<(u32, u32)> = square
        .filter(|&(x, y)| image.get(x, y) != Some([0_u8, 0, 0, 255]))
        .collect();
    assert!(
        unpicked.is_empty(),
        "the whole square is ink, at the bright and the dark end alike: {unpicked:?}"
    );
    let sampled = [
        (0_u32, 0_u32),
        (39, 0),
        (0, 23),
        (39, 23),
        (12, 10),
        (18, 12),
        (15, 7),
    ];
    let inked: Vec<(u32, u32)> = sampled
        .iter()
        .copied()
        .filter(|&(x, y)| image.get(x, y) != Some([255_u8, 255, 255, 255]))
        .collect();
    assert!(
        inked.is_empty(),
        "the sampled gradient pixels stay opaque paper: {inked:?}"
    );
}

/// `process_image` with the uneven-light binarizer: `adaptive_threshold`
/// at radius `4`, offset `10` inks the whole `5 x 5` square even where
/// the gradient darkens beneath it, reports the flattened background's
/// extremes (`144..=249` against the raw `138..=255`), and leaves every
/// gradient pixel opaque white paper.
#[tokio::test]
async fn process_image_adaptive_threshold_inks_a_square_on_a_gradient() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "GRADIENT.png", &gradient_rect_png());
    let report = call(
        &client,
        "process_image",
        source_args(
            &file,
            json!({
                "ops": [
                    { "op": "adaptive_threshold", "radius": 4_u64, "offset": 10_u64 },
                ],
            }),
        ),
    )
    .await;
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 1_usize, "one op ran");
    let step = &steps[0];
    assert_eq!(step["op"], json!("adaptive_threshold"), "the op name");
    assert_eq!(step["radius"], json!(4_u64), "the radius echoes");
    assert_eq!(step["offset"], json!(10_u64), "the offset echoes");
    assert_eq!(step["otsu"], json!(false), "the test is local, never Otsu");
    assert_eq!(step["ink_pixels"], json!(25_u64), "exactly the 5x5 square");
    assert_eq!(
        step["background_min"],
        json!(144_u64),
        "the flattened dark end: mean(150, 147, 144, 141, 138)"
    );
    assert_eq!(
        step["background_max"],
        json!(249_u64),
        "the flattened bright end: mean(255, 252, 249, 246, 243)"
    );
    let url = report["processed"]["url"]
        .as_str()
        .expect("a processed URL");
    let image = asset_png(url).await;
    assert_eq!(
        (image.width, image.height),
        (40_u32, 24_u32),
        "no shape change"
    );
    assert_adaptive_ink_and_paper(&image);
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

/// An `8x8` picture of four solid colour blocks: red top-left, lime
/// top-right, blue bottom-right, black bottom-left.
fn quadrant_png() -> Vec<u8> {
    let mut sheet = paper_sheet(SIDE, SIDE);
    fill_rect(&mut sheet, (0, 0, 4, 4), [255_u8, 0, 0, 255]);
    fill_rect(&mut sheet, (4, 0, SIDE, 4), [0_u8, 255, 0, 255]);
    fill_rect(&mut sheet, (4, 4, SIDE, SIDE), [0_u8, 0, 255, 255]);
    fill_rect(&mut sheet, (0, 4, 4, SIDE), [0_u8, 0, 0, 255]);
    encode_png(&sheet, None).expect("test PNG encodes")
}

/// Arguments straightening the corner quad of [`quadrant_png`] into an
/// `8x8` rectangle.
fn straighten_args(file: &Path) -> Value {
    source_args(
        file,
        json!({
            "ops": [{
                "op": "perspective_correct",
                "corners": [
                    { "x": 1_i64, "y": 1_i64 },
                    { "x": 6_i64, "y": 1_i64 },
                    { "x": 6_i64, "y": 6_i64 },
                    { "x": 1_i64, "y": 5_i64 },
                ],
                "width": 8_u64,
                "height": 8_u64,
            }],
        }),
    )
}

/// `process_image` with `perspective_correct`: the skewed quad of
/// four corner colours is straightened to `8x8` and each corner
/// colour lands exactly on its rectangle corner.
#[tokio::test]
async fn process_image_perspective_corrects_a_skewed_quad_to_a_rectangle() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "SKEW.png", &quadrant_png());
    let report = call(&client, "process_image", straighten_args(&file)).await;
    assert_eq!(report["width"], json!(8_u64), "the rectangle's width");
    assert_eq!(report["height"], json!(8_u64), "the rectangle's height");
    let step = &report["steps"][0];
    assert_eq!(step["op"], json!("perspective_correct"), "the step name");
    assert_eq!(
        step["corners"],
        json!([
            [1_i64, 1_i64],
            [6_i64, 1_i64],
            [6_i64, 6_i64],
            [1_i64, 5_i64]
        ]),
        "the corner pairs echo"
    );
    let url = report["processed"]["url"]
        .as_str()
        .expect("a processed URL");
    let image = asset_png(url).await;
    assert_eq!(
        image.get(0, 0),
        Some([255_u8, 0, 0, 255]),
        "the top-left corner straightens to red"
    );
    assert_eq!(
        image.get(7, 0),
        Some([0_u8, 255, 0, 255]),
        "the top-right corner straightens to lime"
    );
    assert_eq!(
        image.get(7, 7),
        Some([0_u8, 0, 255, 255]),
        "the bottom-right corner straightens to blue"
    );
    assert_eq!(
        image.get(0, 7),
        Some([0_u8, 0, 0, 255]),
        "the bottom-left corner straightens to black"
    );
}
