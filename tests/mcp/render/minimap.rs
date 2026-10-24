//! `render_minimap`' compact automap square: the whole map framed in automap
//! colours with its lines in their documented shades, thing dots by category
//! when asked, `palette_colors` answering the fixture `PLAYPAL`'s automap
//! indices, a small radius zooming in and moving the shared line out of the
//! window, the player arrow running white from its position to its tip, and
//! the refusals: sizes and radii outside their ranges, an unknown map and an
//! empty map that frames nothing until a centre and radius are given. The
//! window mapping the pixels are checked through is [`minimap_pixel`].

use serde_json::{Value, json};

use crate::fixtures::pwad_empty_map;
use crate::render::{Client, count_pixels, iwad_client, non_background, path_arg, rendered_image};
use crate::support::{call, call_err};

/// The pixel of world `x`/`y` in a minimap report: the documented
/// window mapping, `mid ± (v - centre) * size / (2 * radius)`
/// floor-rounded.
fn minimap_pixel(report: &Value, x: i64, y: i64) -> (u32, u32) {
    let size = report["size"].as_i64().expect("size");
    let centre_x = report["center"]["x"].as_i64().expect("centre x");
    let centre_y = report["center"]["y"].as_i64().expect("centre y");
    let radius = report["radius"].as_i64().expect("radius");
    let mid = size.checked_div(2).expect("a test size");
    let den = radius.checked_mul(2).expect("a test radius");
    let column = x
        .saturating_sub(centre_x)
        .checked_mul(size)
        .expect("a test product")
        .div_euclid(den);
    let row = y
        .saturating_sub(centre_y)
        .checked_mul(size)
        .expect("a test product")
        .div_euclid(den);
    (
        u32::try_from(mid.saturating_add(column)).expect("a fitted column"),
        u32::try_from(mid.saturating_sub(row)).expect("a fitted row"),
    )
}

/// One default-framed minimap of the fixture, in the given extra args.
async fn minimap_of(client: &Client, extra: Value) -> Value {
    let mut args = extra.as_object().cloned().unwrap_or_default();
    args.insert("map".to_owned(), json!("MAP01"));
    call(client, "render_minimap", Value::Object(args)).await
}

#[tokio::test]
async fn render_minimap_frames_the_whole_map_in_automap_colours() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = minimap_of(&client, json!({ "palette_colors": false })).await;
    assert_eq!(report["map"], json!("MAP01"), "the rendered map");
    assert_eq!(report["size"], json!(256_u64), "the default edge");
    assert_eq!(
        report["center"],
        json!({ "x": 256_i64, "y": 128_i64 }),
        "the map centre"
    );
    assert_eq!(
        report["radius"],
        json!(257_i64),
        "half the long span, padded"
    );
    assert_eq!(
        report["asset"]["filename"],
        json!("MAP01_minimap.png"),
        "the published name"
    );
    let image = rendered_image(&report).await;
    assert_eq!((image.width, image.height), (256, 256), "the square PNG");
    let (west_x, west_y) = minimap_pixel(&report, 0, 128);
    assert_eq!(
        image.get(west_x, west_y).expect("a fitted pixel"),
        [252, 0, 0, 255],
        "the one-sided west wall in automap red"
    );
    let (step_x, step_y) = minimap_pixel(&report, 256, 120);
    assert_eq!(
        image.get(step_x, step_y).expect("a fitted pixel"),
        [188, 120, 72, 255],
        "the shared step line is brown for its floor change"
    );
    assert!(non_background(&image) > 0, "the line map drew");
    let (dot_x, dot_y) = minimap_pixel(&report, 64, 128);
    assert_eq!(
        image.get(dot_x, dot_y).expect("a fitted pixel"),
        [0, 0, 0, 255],
        "things are unmarked by default"
    );
}

#[tokio::test]
async fn render_minimap_marks_things_with_category_dots_when_asked() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = minimap_of(
        &client,
        json!({ "palette_colors": false, "show_things": true }),
    )
    .await;
    let image = rendered_image(&report).await;
    let (start_x, start_y) = minimap_pixel(&report, 64, 128);
    assert_eq!(
        image.get(start_x, start_y).expect("a fitted pixel"),
        [64, 224, 64, 255],
        "the player start dot"
    );
    let next = start_x.checked_add(1_u32).expect("a fitted column");
    let below = start_y.checked_add(1_u32).expect("a fitted row");
    assert_eq!(
        image.get(next, below).expect("a fitted pixel"),
        [64, 224, 64, 255],
        "the dot is 2x2"
    );
    let (imp_x, imp_y) = minimap_pixel(&report, 384, 128);
    assert_eq!(
        image.get(imp_x, imp_y).expect("a fitted pixel"),
        [224, 64, 64, 255],
        "the imp dot"
    );
}

#[tokio::test]
async fn palette_colours_come_from_the_fixture_playpal_automap_indices() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = minimap_of(&client, Value::Null).await;
    let image = rendered_image(&report).await;
    let (west_x, west_y) = minimap_pixel(&report, 0, 128);
    assert_eq!(
        image.get(west_x, west_y).expect("a fitted pixel"),
        [176, 79, 176, 255],
        "palette index 176 is the wall colour"
    );
    let (step_x, step_y) = minimap_pixel(&report, 256, 120);
    assert_eq!(
        image.get(step_x, step_y).expect("a fitted pixel"),
        [64, 191, 64, 255],
        "palette index 64 is the floor-change colour"
    );
}

#[tokio::test]
async fn a_small_radius_zooms_and_moves_the_shared_line_away() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = minimap_of(
        &client,
        json!({
            "palette_colors": false,
            "center": { "x": 64_i64, "y": 128_i64 },
            "radius": 64_u64,
        }),
    )
    .await;
    assert_eq!(report["center"]["x"], json!(64_i64), "the given centre");
    assert_eq!(report["radius"], json!(64_u64), "the given radius");
    let image = rendered_image(&report).await;
    assert_eq!(
        count_pixels(&image, [188, 120, 72, 255]),
        0,
        "the step line at x 256 is outside this window"
    );
    assert!(
        count_pixels(&image, [252, 0, 0, 255]) > 0,
        "the west wall still crosses the zoomed square"
    );
    let (hall_x, hall_y) = minimap_pixel(&report, 64, 128);
    assert_eq!(
        image.get(hall_x, hall_y).expect("a fitted pixel"),
        [0, 0, 0, 255],
        "the shared line no longer runs through the centre"
    );
}

#[tokio::test]
async fn the_player_arrow_is_white_from_its_position_to_its_tip() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = minimap_of(
        &client,
        json!({
            "palette_colors": false,
            "center": { "x": 64_i64, "y": 128_i64 },
            "radius": 64_u64,
            "player": { "x": 64_i64, "y": 128_i64, "angle": 0_i64 },
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let (at_x, at_y) = minimap_pixel(&report, 64, 128);
    assert_eq!(
        image.get(at_x, at_y).expect("a fitted pixel"),
        [255, 255, 255, 255],
        "the arrow starts at the player position"
    );
    let tip_x = at_x.checked_add(16_u32).expect("a fitted column");
    assert_eq!(
        image.get(tip_x, at_y).expect("a fitted pixel"),
        [255, 255, 255, 255],
        "the shaft runs 16 pixels east"
    );
    let behind = at_x.checked_sub(8_u32).expect("a fitted column");
    assert_eq!(
        image.get(behind, at_y).expect("a fitted pixel"),
        [0, 0, 0, 255],
        "nothing trails the tail"
    );
}

#[tokio::test]
async fn minimap_sizes_and_radii_outside_their_ranges_are_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (small_code, small_message, _) =
        call_err(&client, "render_minimap", json!({ "size": 16_u64 })).await;
    assert_eq!(small_code, "invalid_argument", "an edge below the floor");
    assert!(
        small_message.contains("32..=1024") && small_message.contains("16"),
        "the message shows the range and the value: {small_message}"
    );
    let (big_code, _, _) = call_err(&client, "render_minimap", json!({ "size": 2_048_u64 })).await;
    assert_eq!(big_code, "invalid_argument", "an edge above the ceiling");
    let (zero_code, zero_message, _) =
        call_err(&client, "render_minimap", json!({ "radius": 0_u64 })).await;
    assert_eq!(
        zero_code, "invalid_argument",
        "a zero radius frames nothing"
    );
    assert!(
        zero_message.contains("1..=65536") && zero_message.contains("radius"),
        "the message names the field and the range: {zero_message}"
    );
    let (far_code, _, _) =
        call_err(&client, "render_minimap", json!({ "radius": 100_000_u64 })).await;
    assert_eq!(far_code, "invalid_argument", "a radius past the ceiling");
}

#[tokio::test]
async fn render_minimap_refuses_unknown_maps_and_windowless_empty_maps() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (missing_code, missing_message, _) =
        call_err(&client, "render_minimap", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "no such map");
    assert!(
        missing_message.contains("MAP99"),
        "the message names it: {missing_message}"
    );
    let extra = pwad_empty_map(dir.path());
    call(&client, "open_wad", path_arg(&extra)).await;
    let (empty_code, empty_message, _) = call_err(&client, "render_minimap", json!({})).await;
    assert_eq!(empty_code, "invalid_argument", "nothing to frame");
    assert!(
        empty_message.contains("center") && empty_message.contains("radius"),
        "the message names both fixes: {empty_message}"
    );
    let report = call(
        &client,
        "render_minimap",
        json!({
            "center": { "x": 0_i64, "y": 0_i64 },
            "radius": 32_u64,
        }),
    )
    .await;
    assert_eq!(report["radius"], json!(32_u64), "the given radius");
    let image = rendered_image(&report).await;
    assert_eq!(non_background(&image), 0, "an empty map is pure black");
}
