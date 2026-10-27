//! `render_view`' first-person frame: the camera echo and the hall it paints,
//! pitch moving the horizon down the frame, a room-side camera taking an
//! explicit eye height, `doom` lighting darkening the far wall below the
//! fullbright one, the session `gamma` landing on the frame exactly once at
//! encode, the floor textured with its flat on the world grid, the `SKY1`
//! texture over a sky ceiling (and the background colour with the sky
//! switch off) and the masked grate showing the far flat through its hole.
//! The fixture camera of [`view_args`], the fullbright switch of
//! [`fullbright`] and [`sky_room_client`] serve the `view_sprites` topic too.

use std::path::Path;

use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::render::gamma_table;
use serde_json::{Value, json};

use crate::fixtures::{iwad_hexen, iwad_view_textures};
use crate::render::{Client, iwad_client, path_arg, rendered_image};
use crate::support::{call, connect};

/// The `render_view` arguments over the fixture camera in the hall,
/// with `extra` merged in.
pub(super) fn view_args(extra: &Value) -> Value {
    let mut args = json!({
        "map": "MAP01", "x": 64_i64, "y": 128_i64,
        "width": 160_u64, "height": 100_u64,
    });
    let Some(base) = args.as_object_mut() else {
        return args;
    };
    let Some(over) = extra.as_object() else {
        return args;
    };
    for (key, value) in over {
        base.insert(key.clone(), value.clone());
    }
    args
}

/// Light everything fully so palette colours land unchanged.
pub(super) async fn fullbright(client: &Client) {
    call(
        client,
        "set_render_options",
        json!({ "lighting": "fullbright" }),
    )
    .await;
}

/// A fullbright client on the textured-view fixture `IWAD`.
pub(super) async fn sky_room_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_view_textures(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    fullbright(&client).await;
    client
}

/// The number of pixels of `image`'s rows `top..` (the rest of the
/// frame) that are exactly `color`.
fn count_pixels_in_rows(image: &RgbaImage, top: u32, color: [u8; 4]) -> usize {
    (top..image.height)
        .flat_map(|row| (0..image.width).map(move |col| (col, row)))
        .filter(|&(col, row)| image.get(col, row) == Some(color))
        .count()
}

/// Asserts the sky band at the frame top of a room-1 view reads the
/// `SKY1` texture colour: room 1 (`x 256..512`) has the `F_SKY1`
/// ceiling and its sky rows start at the frame top, well above the
/// horizon at row 50.
fn assert_sky_band(image: &RgbaImage) {
    for (column, row) in [
        (40_u32, 0_u32),
        (40, 10),
        (80, 0),
        (80, 10),
        (120, 0),
        (120, 10),
    ] {
        assert_eq!(
            image.get(column, row).expect("a pixel"),
            [200_u8, 55, 200, 255],
            "the F_SKY1 ceiling draws the SKY1 texture at column {column} row {row}"
        );
    }
}

#[tokio::test]
async fn render_view_reports_the_camera_and_paints_the_hall_view() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    fullbright(&client).await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    assert_eq!(report["map"], json!("MAP01"), "the rendered map");
    assert_eq!(
        report["masked_columns"],
        json!(0_u64),
        "the plain fixture map masks nothing"
    );
    assert_eq!(
        report["camera"],
        json!({
            "x": 64_i64, "y": 128_i64, "z": 41_i64, "angle": 0_i64,
            "pitch": 0_i64, "fov": 90_u64, "sector": 0_u64,
        }),
        "the effective camera, standing on the hall floor"
    );
    assert_eq!(
        report["asset"]["filename"],
        json!("MAP01_view.png"),
        "the published name"
    );
    assert_eq!(
        report["problems"],
        json!([]),
        "an indoor map whose flats, textures and sprites the session has reports \
         nothing: {}",
        report["problems"]
    );
    let image = rendered_image(&report).await;
    assert_eq!((image.width, image.height), (160, 100), "the PNG shape");
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        [108_u8, 147, 108, 255],
        "the centre at the horizon is room 1's far WALL1 wall through the open step \
         line, a fake-contrast step darker on its north-south line"
    );
    assert_eq!(
        image.get(80, 60).expect("a pixel"),
        [158_u8, 97, 158, 255],
        "low in the centre column the STEP1 lower texture faces the step"
    );
    assert_eq!(
        image.get(0, 99).expect("a pixel"),
        [1_u8, 254, 1, 255],
        "the bottom-left pixel is the FLOOR1 average as floor"
    );
    assert_eq!(
        image.get(20, 0).expect("a pixel"),
        [1_u8, 254, 1, 255],
        "near the top-left the hall's own ceiling still shows"
    );
}

#[tokio::test]
async fn a_hexen_session_draws_the_sky_on_an_f_sky_ceiling() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad_hexen(dir.path());
    call(&client, "open_iwad", path_arg(&base)).await;
    fullbright(&client).await;
    call(
        &client,
        "edit_sector",
        json!({ "map": "MAP01", "index": 0_i32, "ceiling_flat": "F_SKY" }),
    )
    .await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    assert_eq!(
        report["camera"]["sector"],
        json!(0_u64),
        "standing in the hall under the F_SKY ceiling"
    );
    let problems = report["problems"].as_array().expect("a problems list");
    assert!(
        !problems
            .iter()
            .any(|row| row.as_str().unwrap_or_default().starts_with("missing sky")),
        "the fixture carries SKY1, so the sky is not a problem: {problems:?}"
    );
    let image = rendered_image(&report).await;
    for (column, row) in [
        (40_u32, 0_u32),
        (40, 10),
        (80, 0),
        (80, 10),
        (120, 0),
        (120, 10),
    ] {
        assert_eq!(
            image.get(column, row).expect("a pixel"),
            [200_u8, 55, 200, 255],
            "the Hexen session's F_SKY ceiling draws the sky at column {column} row {row}"
        );
    }
}

#[tokio::test]
async fn render_view_pitch_moves_the_horizon_down_the_frame() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    fullbright(&client).await;
    let report = call(
        &client,
        "render_view",
        view_args(&json!({ "pitch": 30_i64 })),
    )
    .await;
    assert_eq!(report["camera"]["pitch"], json!(30_i64), "echoed back");
    let image = rendered_image(&report).await;
    assert_eq!(
        image.get(80, 84).expect("a pixel"),
        [108_u8, 147, 108, 255],
        "the far wall slid down with the horizon: its rows are now around 80 to 97"
    );
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        [1_u8, 254, 1, 255],
        "the old centre row now looks over the hall's ceiling"
    );
}

#[tokio::test]
async fn render_view_takes_the_room_side_camera_and_an_explicit_eye_height() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_view",
        json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "z": 100_i64,
                "angle": 180_i64, "width": 160_u64, "height": 100_u64 }),
    )
    .await;
    assert_eq!(report["camera"]["sector"], json!(1_u64), "in room 1");
    assert_eq!(
        report["camera"]["z"],
        json!(100_i64),
        "the given eye height"
    );
    assert_eq!(
        report["camera"]["angle"],
        json!(180_i64),
        "looking back west into the hall"
    );
}

#[tokio::test]
async fn render_view_doom_lighting_darkens_the_far_wall() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    let image = rendered_image(&report).await;
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        [156_u8, 99, 156, 255],
        "light 192 at depth 448 lands on table 7 of the fixture ramp: \
         index 100 + 56, darker than the fullbright wall's (108, 147, 108)"
    );
}

/// The `level` gamma table's entry for one 8-bit `channel`.
fn gamma_lift(level: u8, channel: u8) -> u8 {
    *gamma_table(level)
        .get(usize::from(channel))
        .expect("a gamma table covers the u8 range")
}

/// `color` with `level`'s curve run over its three colour channels: the
/// single encode-time step `Canvas::apply_gamma` does on the finished frame.
fn lifted(level: u8, color: [u8; 4]) -> [u8; 4] {
    [
        gamma_lift(level, color[0]),
        gamma_lift(level, color[1]),
        gamma_lift(level, color[2]),
        color[3],
    ]
}

/// The pixels of `bright` that are *not* `plain`'s with `level`'s table
/// applied exactly once, in scan order.
fn unlifted(plain: &RgbaImage, bright: &RgbaImage, level: u8) -> Vec<(u32, u32)> {
    (0..bright.height)
        .flat_map(|row| (0..bright.width).map(move |column| (column, row)))
        .filter(|&(column, row)| {
            bright.get(column, row).expect("a pixel")
                != lifted(level, plain.get(column, row).expect("a pixel"))
        })
        .collect()
}

/// The pixels `level`'s curve actually moved between the two frames.
fn corrected(plain: &RgbaImage, bright: &RgbaImage) -> usize {
    (0..bright.height)
        .flat_map(|row| (0..bright.width).map(move |column| (column, row)))
        .filter(|&(column, row)| bright.get(column, row) != plain.get(column, row))
        .count()
}

#[tokio::test]
async fn render_view_applies_the_session_gamma_once_at_encode() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    fullbright(&client).await;
    let plain = rendered_image(&call(&client, "render_view", view_args(&Value::Null)).await).await;
    call(&client, "set_render_options", json!({ "gamma": 2_u64 })).await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    assert_eq!(
        report["camera"],
        json!({
            "x": 64_i64, "y": 128_i64, "z": 41_i64, "angle": 0_i64,
            "pitch": 0_i64, "fov": 90_u64, "sector": 0_u64,
        }),
        "the same camera, so only the gamma differs"
    );
    let gamma2 = rendered_image(&report).await;
    let wall = plain.get(80, 50).expect("a pixel");
    assert_eq!(
        wall,
        [108_u8, 147, 108, 255],
        "the hall's far wall through the open step line, at the session's default \
         gamma 0"
    );
    let once = lifted(2, wall);
    let twice = lifted(2, once);
    assert_eq!(
        once,
        [123_u8, 162, 123, 255],
        "the gamma-2 table's one lift of that wall colour"
    );
    assert_ne!(
        twice, once,
        "a second lift lands on {twice:?}, so the checks below tell one application \
         from two"
    );
    assert_eq!(
        gamma2.get(80, 50).expect("a pixel"),
        once,
        "the known wall pixel is the gamma-0 pixel corrected once"
    );
    assert_eq!(
        unlifted(&plain, &gamma2, 2),
        Vec::new(),
        "no pixel of the frame may be the gamma-0 one left alone or lifted twice \
         (each coordinate listed differs from the gamma-2 table applied once)"
    );
    let moved = corrected(&plain, &gamma2);
    assert!(
        moved > 1_000,
        "gamma 2 lifts the mid-tones all over the frame, not one pixel: {moved}"
    );
}

#[tokio::test]
async fn render_view_textures_the_floor_with_its_flat_on_the_world_grid() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad_view_textures(dir.path());
    call(&client, "open_iwad", path_arg(&base)).await;
    fullbright(&client).await;
    let report = call(
        &client,
        "render_view",
        view_args(&json!({ "pitch": -45_i64 })),
    )
    .await;
    let image = rendered_image(&report).await;
    let left = count_pixels_in_rows(&image, 50, [10_u8, 245, 10, 255]);
    let right = count_pixels_in_rows(&image, 50, [20_u8, 235, 20, 255]);
    assert!(
        left > 0,
        "the CHECKER's index-10 half shows in the lower half: texel columns 0..32 of \
         the CHECKER 64x64 grid"
    );
    assert!(
        right > 0,
        "and the index-20 half too: {left}/{right} split by texel column 32"
    );
}

#[tokio::test]
async fn render_view_draws_the_sky_texture_over_a_sky_ceiling() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let report = call(
        &client,
        "render_view",
        json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "angle": 180_i64,
                "width": 160_u64, "height": 100_u64 }),
    )
    .await;
    assert_eq!(
        report["camera"]["sector"],
        json!(1_u64),
        "standing in the sky room"
    );
    let image = rendered_image(&report).await;
    assert_sky_band(&image);
    assert!(
        !report["problems"]
            .as_array()
            .expect("problems is a list")
            .iter()
            .any(|problem| problem.as_str().is_some_and(|text| text.contains("SKY1"))),
        "the fixture SKY1 loaded: {}",
        report["problems"]
    );
    call(&client, "set_render_options", json!({ "sky": false })).await;
    let off = call(
        &client,
        "render_view",
        json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "angle": 180_i64,
                "width": 160_u64, "height": 100_u64 }),
    )
    .await;
    let dark = rendered_image(&off).await;
    assert_eq!(
        dark.get(80, 0).expect("a pixel"),
        [0_u8, 0, 0, 255],
        "with the sky switch off the sky ceiling reads the background colour"
    );
}

#[tokio::test]
async fn render_view_paints_a_masked_grate_and_shows_the_far_flat_through_its_hole() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let report = call(
        &client,
        "render_view",
        view_args(&json!({ "map": "MAP02" })),
    )
    .await;
    assert_eq!(
        report["camera"]["sector"],
        json!(0_u64),
        "the camera stands in the hall before the grate"
    );
    let masked = report["masked_columns"]
        .as_u64()
        .expect("masked_columns is a number");
    assert!(masked >= 1, "the transparent pass painted {masked} columns");
    let image = rendered_image(&report).await;
    assert_eq!(
        image.get(70, 10).expect("a pixel"),
        [70_u8, 185, 70, 255],
        "the grate's texel left of the hole paints palette 70, fullbright"
    );
    assert_eq!(
        image.get(72, 10).expect("a pixel"),
        [70_u8, 185, 70, 255],
        "and the texel right of it"
    );
    assert_eq!(
        image.get(71, 9).expect("a pixel"),
        [70_u8, 185, 70, 255],
        "and one above"
    );
    assert_eq!(
        image.get(71, 11).expect("a pixel"),
        [70_u8, 185, 70, 255],
        "and one below"
    );
    assert_eq!(
        image.get(71, 10).expect("a pixel"),
        [1_u8, 254, 1, 255],
        "the GRATEP hole lets the FLOOR1 flat of the far ceiling show through"
    );
}
