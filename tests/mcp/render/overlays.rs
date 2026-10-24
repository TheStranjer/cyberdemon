//! The `underlay` picture a top-down frame draws beneath its map: the
//! reported rectangle and the picture showing through the pixels, one drawing
//! that never repeats past an edge (a shifted, negative-origin placement
//! alike), a transparent edge keeping the background and staying opaque to
//! the canvas, a frame that cannot reach the picture drawing nothing, and a
//! zero opacity doing the same. The canvas-rectangle helpers those pixel
//! verdicts are read with live here too; [`sketch_png`] and
//! [`sketch_underlay`] are `pub(super)` because the `decorations` topic
//! refuses an underlay with them.

use std::fs;
use std::path::{Path, PathBuf};

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::render::{Transform, non_background, rendered_image};
use crate::support::{call, connect};

/// A `50x40` reference `PNG`: opaque red on the left half, blue on the
/// right, written into `dir`.
pub(super) fn sketch_png(dir: &Path) -> PathBuf {
    let mut row = Vec::new();
    for _col in 0..25_u32 {
        row.extend_from_slice(&[255_u8, 0, 0, 255]);
    }
    for _col in 0..25_u32 {
        row.extend_from_slice(&[0, 0, 255, 255]);
    }
    let mut pixels = Vec::new();
    for _row in 0..40_u32 {
        pixels.extend_from_slice(&row);
    }
    let image = RgbaImage::new(50, 40, pixels).expect("the buffer matches the shape");
    let png = encode_png(&image, None).expect("the test picture encodes");
    let path = dir.join("sketch.png");
    fs::write(&path, png).expect("the sketch file writes");
    path
}

/// An `underlay` argument object for the sketch file: eight map units
/// per image pixel with the `y` mirror, exactly the plan builder's
/// image-space recipe.
pub(super) fn sketch_underlay(file: &Path) -> Value {
    json!({
        "source": file.display().to_string(),
        "units": { "scale_num": 8_i64, "flip_y": true },
    })
}

/// How far inside the reported rectangle the colour checks stay, clearing
/// the half-texel the nearest sampler carries past an edge.
const INSIDE_INSET: u32 = 10;

/// One canvas coordinate.
type Spot = (u32, u32);

/// One canvas pixel: its coordinate and its colour.
type Pixel = (u32, u32, [u8; 4]);

/// A canvas rectangle as two opposite corners.
type Extent = (Spot, Spot);

/// The canvas coordinates inside the rectangle spanned by the opposite
/// corners `near` and `far`, shrunk by `inset` on every side.
fn inset_pixels(near: Spot, far: Spot, inset: u32) -> Vec<Spot> {
    let left = near.0.saturating_add(inset);
    let right = far.0.saturating_sub(inset);
    let top = near.1.saturating_add(inset);
    let bottom = far.1.saturating_sub(inset);
    (top..bottom)
        .flat_map(|row| (left..right).map(move |column| (column, row)))
        .collect()
}

/// The colours of the pixels of `image` inside that shrunk rectangle.
fn pixels_in(image: &RgbaImage, near: Spot, far: Spot, inset: u32) -> Vec<Pixel> {
    inset_pixels(near, far, inset)
        .into_iter()
        .filter_map(|(column, row)| image.get(column, row).map(|pixel| (column, row, pixel)))
        .collect()
}

/// How many of `pixels` are `colour`.
fn count_colour(pixels: &[Pixel], colour: [u8; 4]) -> usize {
    pixels.iter().filter(|pixel| pixel.2 == colour).count()
}

/// The canvas pixels at least `margin` clear of the rectangle `near`/`far`
/// that are not the plain black background. The `margin` keeps the verdict
/// away from the rectangle's edge, across which the nearest sampler may
/// still carry a texel's colour for half a texel.
fn stray_pixels(image: &RgbaImage, near: Spot, far: Spot, margin: u32) -> Vec<Pixel> {
    let left = near.0.saturating_sub(margin);
    let right = far.0.saturating_add(margin);
    let top = near.1.saturating_sub(margin);
    let bottom = far.1.saturating_add(margin);
    (0..image.height)
        .flat_map(|row| (0..image.width).map(move |column| (column, row)))
        .filter(|&(column, row)| column <= left || column >= right || row <= top || row >= bottom)
        .filter_map(|(column, row)| {
            image
                .get(column, row)
                .filter(|pixel| *pixel != [0_u8, 0, 0, 255])
                .map(|pixel| (column, row, pixel))
        })
        .collect()
}

/// The rectangle the `underlay` report says the picture covers, in canvas
/// pixels: its two opposite map corners through the fitted `transform`.
fn underlay_rectangle(report: &Value, view: &Transform) -> Extent {
    let bounds = &report["underlay"]["map_bounds"];
    let corner = |x: &str, y: &str| {
        view.project(
            bounds[x].as_i64().expect("a reported bound"),
            bounds[y].as_i64().expect("a reported bound"),
        )
    };
    let first = corner("min_x", "max_y");
    let second = corner("max_x", "min_y");
    (
        (first.0.min(second.0), first.1.min(second.1)),
        (first.0.max(second.0), first.1.max(second.1)),
    )
}

/// Requires the fitted frame to reach `margin` pixels past the rectangle
/// on every side, so checking the pixels out there means something.
fn assert_frame_reaches_past(image: &RgbaImage, near: Spot, far: Spot, margin: u32) {
    assert!(
        near.0 > margin
            && near.1 > margin
            && far.0.saturating_add(margin) < image.width
            && far.1.saturating_add(margin) < image.height,
        "the frame is larger than the picture on every side: {near:?} to {far:?} in {}x{}",
        image.width,
        image.height
    );
}

/// Requires the red-and-blue [`sketch_png`] to cover exactly its own
/// rectangle: the pixels [`INSIDE_INSET`] clear of `near`/`far` hold only
/// the picture's two colours, both of them met, and the pixels `margin`
/// clear of the rectangle hold only the background. The second half is the
/// ticket: a sample that ran past an image edge showed a second copy out
/// there.
fn assert_underlay_covers_its_rectangle(image: &RgbaImage, near: Spot, far: Spot, margin: u32) {
    let red = [255_u8, 0, 0, 255];
    let blue = [0_u8, 0, 255, 255];
    let inside = pixels_in(image, near, far, INSIDE_INSET);
    let wrong = inside
        .iter()
        .find(|pixel| pixel.2 != red && pixel.2 != blue);
    assert!(
        wrong.is_none(),
        "inside the picture only its own two colours stand, first offender {wrong:?}"
    );
    let reds = count_colour(&inside, red);
    let blues = count_colour(&inside, blue);
    assert!(
        reds > 0 && blues > 0,
        "both halves of the picture are painted: {reds} red, {blues} blue"
    );
    let stray = stray_pixels(image, near, far, margin);
    assert!(
        stray.is_empty(),
        "{} pixel(s) outside the picture's rectangle are not the background, first few \
         {:?}: the underlay must be drawn once and never repeat past an edge",
        stray.len(),
        stray.iter().take(4).collect::<Vec<_>>()
    );
}

/// A `4x3` `PNG` whose first row is opaque green and whose other two rows
/// are fully transparent.
fn green_and_clear_png(dir: &Path) -> PathBuf {
    let mut pixels = Vec::new();
    pixels.extend_from_slice(&[[0_u8, 255, 0, 255]; 4]);
    pixels.extend_from_slice(&[[0_u8, 0, 0, 0]; 8]);
    let image = RgbaImage::new(4, 3, pixels.concat()).expect("the buffer matches the shape");
    let png = encode_png(&image, None).expect("the test picture encodes");
    let path = dir.join("alpha.png");
    fs::write(&path, png).expect("the sketch file writes");
    path
}

#[tokio::test]
async fn an_underlay_frames_an_empty_map_and_shows_through_the_pixels() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 200_u64, "height": 160_u64,
            "underlay": sketch_underlay(&file),
        }),
    )
    .await;
    let underlay = &report["underlay"];
    assert_eq!(underlay["width"], json!(50_u64), "the picture width");
    assert_eq!(underlay["height"], json!(40_u64), "the picture height");
    assert_eq!(
        underlay["map_bounds"],
        json!({ "min_x": 0_i64, "min_y": -320_i64, "max_x": 400_i64, "max_y": 0_i64 }),
        "eight units per pixel, mirrored about y = 0"
    );
    assert_eq!(report["notes"], json!([]), "nothing was skipped");
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (red_x, red_y) = view.project(100, -100);
    let [red, _, blue, alpha] = image.get(red_x, red_y).expect("a fitted pixel");
    assert_eq!(alpha, 255_u8, "the background stays opaque");
    assert!(
        red > blue.saturating_add(40) && red > 64,
        "map (100, -100) is red half's pixel 13: {red},{blue}"
    );
    let (blue_x, blue_y) = view.project(300, -100);
    let [far_red, _, far_blue, _] = image.get(blue_x, blue_y).expect("a fitted pixel");
    assert!(
        far_blue > far_red.saturating_add(40) && far_blue > 64,
        "map (300, -100) is blue half's pixel 38: {far_red},{far_blue}"
    );
}

#[tokio::test]
async fn an_underlay_is_drawn_once_and_never_repeats_past_its_edges() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 640_u64, "height": 400_u64,
            // a frame larger than the picture: three map units per two
            // image pixels, so whole canvas columns stand past its edges
            "bounds": {
                "min_x": -100_i64, "min_y": -120_i64,
                "max_x": 200_i64, "max_y": 60_i64,
            },
            "underlay": {
                "source": file.display().to_string(),
                "units": { "scale_num": 3_i64, "scale_den": 2_i64, "flip_y": true },
                "opacity": 100_u64,
            },
        }),
    )
    .await;
    assert_eq!(
        report["underlay"]["map_bounds"],
        json!({ "min_x": 0_i64, "min_y": -60_i64, "max_x": 75_i64, "max_y": 0_i64 }),
        "one and a half units per pixel, mirrored about y = 0"
    );
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (near, far) = underlay_rectangle(&report, &view);
    assert_frame_reaches_past(&image, near, far, 20);
    assert_underlay_covers_its_rectangle(&image, near, far, 12);
}

#[tokio::test]
async fn an_underlay_at_a_negative_offset_draws_nothing_around_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 640_u64, "height": 400_u64,
            // the picture shifted into the frame's right half, unmirrored,
            // so the frame also reaches world space the origin puts at
            // negative image coordinates on both axes
            "bounds": {
                "min_x": -100_i64, "min_y": -300_i64,
                "max_x": 400_i64, "max_y": 100_i64,
            },
            "underlay": {
                "source": file.display().to_string(),
                "units": {
                    "scale_num": 2_i64, "origin_x": 200_i64,
                    "origin_y": -100_i64, "flip_y": false,
                },
                "opacity": 100_u64,
            },
        }),
    )
    .await;
    assert_eq!(
        report["underlay"]["map_bounds"],
        json!({ "min_x": 200_i64, "min_y": -100_i64, "max_x": 300_i64, "max_y": -20_i64 }),
        "two units per pixel from the shifted origin, unmirrored"
    );
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (near, far) = underlay_rectangle(&report, &view);
    assert_frame_reaches_past(&image, near, far, 20);
    assert_underlay_covers_its_rectangle(&image, near, far, 12);
}

#[tokio::test]
async fn a_transparent_underlay_edge_keeps_the_background() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = green_and_clear_png(dir.path());
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 200_u64, "height": 200_u64,
            "bounds": {
                "min_x": -64_i64, "min_y": -64_i64,
                "max_x": 64_i64, "max_y": 64_i64,
            },
            "underlay": {
                "source": file.display().to_string(),
                "units": { "scale_num": 8_i64, "flip_y": true },
                "opacity": 100_u64,
            },
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (near, far) = underlay_rectangle(&report, &view);
    assert_frame_reaches_past(&image, near, far, 16);
    let green = [0_u8, 255, 0, 255];
    let background = [0_u8, 0, 0, 255];
    let inside = pixels_in(&image, near, far, 0);
    let wrong = inside
        .iter()
        .find(|pixel| pixel.2 != green && pixel.2 != background);
    assert!(
        wrong.is_none(),
        "the picture's rectangle holds its green row or the background, not {wrong:?}"
    );
    assert!(count_colour(&inside, green) > 0, "its opaque row paints");
    assert!(
        count_colour(&inside, background) > 0,
        "its transparent rows leave the background alone"
    );
    let stray = stray_pixels(&image, near, far, 24);
    assert!(
        stray.is_empty(),
        "outside the picture, past its transparent rows and every edge alike, stands only \
         the background: {} stray pixel(s), first few {:?}",
        stray.len(),
        stray.iter().take(4).collect::<Vec<_>>()
    );
    let holes = image
        .pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[3] != 255)
        .count();
    assert_eq!(
        holes, 0,
        "the overview is fully opaque: a transparent texel blends into the background \
         instead of opening a hole in it"
    );
}

#[tokio::test]
async fn an_underlay_the_frame_cannot_reach_draws_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 64_u64, "height": 64_u64,
            // a window so far from the picture's rectangle that even the
            // canvas beyond it cannot reach the picture
            "bounds": {
                "min_x": -4000_i64, "min_y": -1200_i64,
                "max_x": -3000_i64, "max_y": -800_i64,
            },
            "underlay": sketch_underlay(&file),
        }),
    )
    .await;
    assert_eq!(
        report["underlay"]["map_bounds"],
        json!({ "min_x": 0_i64, "min_y": -320_i64, "max_x": 400_i64, "max_y": 0_i64 }),
        "the picture still reports where it stands"
    );
    let image = rendered_image(&report).await;
    assert_eq!(
        non_background(&image),
        0,
        "an underlay the window cannot reach is pure background"
    );
}

#[tokio::test]
async fn a_zero_opacity_underlay_draws_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let mut args = sketch_underlay(&file);
    args["opacity"] = json!(0_u64);
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 64_u64, "height": 64_u64,
            "underlay": args,
        }),
    )
    .await;
    assert_eq!(
        report["underlay"]["width"],
        json!(50_u64),
        "the picture still reports"
    );
    let image = rendered_image(&report).await;
    assert_eq!(
        non_background(&image),
        0,
        "an empty map with a clear underlay is pure background"
    );
}
