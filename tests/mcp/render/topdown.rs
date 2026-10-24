//! `render_topdown`' plain pictures: the fitted transform and the fixture it
//! places, the automap style colouring the outer wall red, `fill_sectors`
//! painting each sector with half its light, the isometric projection
//! omitting the transform and drawing a wireframe, a highlighted linedef
//! standing red and three pixels wide, and the refusals: an unknown map, flat
//! bounds, an index past the end of every highlight list, a nameless
//! projection and a width below the floor. An empty map needs explicit bounds
//! and then frames them as pure background.

use serde_json::{Value, json};

use crate::fixtures::pwad_empty_map;
use crate::render::{Client, Transform, iwad_client, non_background, path_arg, rendered_image};
use crate::support::{call, call_err, connect};

/// Four `render_topdown` calls, one per highlight list, each naming an
/// index past the end of its table.
fn out_of_range_highlights() -> Vec<(&'static str, Value)> {
    vec![
        (
            "linedefs",
            json!({ "highlight": { "linedefs": [999_u64] } }),
        ),
        ("sectors", json!({ "highlight": { "sectors": [9_u64] } })),
        ("things", json!({ "highlight": { "things": [9_u64] } })),
        ("vertices", json!({ "highlight": { "vertices": [99_u64] } })),
    ]
}

/// Assert that each out-of-range highlight call is refused `not_found`.
async fn refuse_out_of_range(client: &Client, cases: &[(&'static str, Value)]) {
    let owned = cases.to_vec();
    for (field, args) in owned {
        let (index_code, _, _) = call_err(client, "render_topdown", args).await;
        assert_eq!(index_code, "not_found", "{field} index past the end");
    }
}

#[tokio::test]
async fn render_topdown_reports_a_fitted_transform_and_paints_the_fixture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({ "map": "MAP01", "width": 200_u64, "height": 120_u64 }),
    )
    .await;
    assert_eq!(report["map"], json!("MAP01"), "the rendered map");
    assert_eq!(report["projection"], json!("topdown"), "the projection");
    assert_eq!(report["width"], json!(200_u64), "the width");
    assert_eq!(report["height"], json!(120_u64), "the height");
    let asset = &report["asset"];
    assert_eq!(
        asset["filename"],
        json!("MAP01_topdown.png"),
        "the published name"
    );
    assert_eq!(asset["mime"], json!("image/png"), "the media type");
    assert!(
        asset["size"].as_u64().expect("a size") > 0,
        "the PNG is not empty"
    );
    let image = rendered_image(&report).await;
    assert_eq!((image.width, image.height), (200, 120), "the PNG shape");
    let view = Transform::of(&report);
    let (line_x, line_y) = view.project(256, 128);
    assert_eq!(
        image.get(line_x, line_y).expect("a fitted pixel"),
        [112, 112, 112, 255],
        "the two-sided step line at its midpoint"
    );
    let (player_x, player_y) = view.project(64, 128);
    assert_eq!(
        image.get(player_x, player_y).expect("a fitted pixel"),
        [64, 224, 64, 255],
        "the player start marker"
    );
}

#[tokio::test]
async fn the_automap_style_colours_the_outer_wall_red() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64, "style": "automap",
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (x, y) = view.project(0, 128);
    assert_eq!(
        image.get(x, y).expect("a fitted pixel"),
        [252, 0, 0, 255],
        "the one-sided west wall in automap red"
    );
}

#[tokio::test]
async fn fill_sectors_paints_each_sector_with_half_its_light() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64,
            "fill_sectors": true, "show_things": false,
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (hall_x, hall_y) = view.project(64, 200);
    let (room_x, room_y) = view.project(400, 200);
    assert_eq!(
        image.get(hall_x, hall_y).expect("a fitted pixel"),
        [80, 80, 80, 255],
        "the hall at light 160 fills grey 80"
    );
    assert_eq!(
        image.get(room_x, room_y).expect("a fitted pixel"),
        [96, 96, 96, 255],
        "the back room at light 192 fills grey 96"
    );
}

#[tokio::test]
async fn the_isometric_projection_omits_the_transform_and_draws_a_wireframe() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64,
            "projection": "isometric",
        }),
    )
    .await;
    assert_eq!(
        report["projection"],
        json!("isometric"),
        "the projection name"
    );
    assert_eq!(report["transform"], Value::Null, "no transform reported");
    assert_eq!(report["width"], json!(200_u64), "the width");
    assert_eq!(report["height"], json!(120_u64), "the height");
    let image = rendered_image(&report).await;
    assert_eq!((image.width, image.height), (200, 120), "the PNG shape");
    assert!(non_background(&image) > 0, "the wireframe drew");
}

#[tokio::test]
async fn highlighting_a_linedef_draws_it_red_and_three_pixels_wide() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64,
            "show_things": false, "highlight": { "linedefs": [2_u64] },
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (centre_x, line_y) = view.project(256, 128);
    let beside = centre_x.checked_sub(1_u32).expect("a fitted column");
    assert_eq!(
        image.get(beside, line_y).expect("a fitted pixel"),
        [255, 64, 64, 255],
        "highlight red left of the line centre"
    );
    assert_eq!(
        image.get(centre_x, line_y).expect("a fitted pixel"),
        [255, 64, 64, 255],
        "highlight red on the line centre"
    );
}

#[tokio::test]
async fn render_topdown_refuses_unknown_maps_bad_bounds_and_indexes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (missing_code, missing_message, _) =
        call_err(&client, "render_topdown", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "no such map");
    assert!(
        missing_message.contains("MAP99"),
        "the message names it: {missing_message}"
    );
    let (flat_code, flat_message, _) = call_err(
        &client,
        "render_topdown",
        json!({ "bounds": { "min_x": 100_i64, "min_y": 0_i64, "max_x": 100_i64, "max_y": 64_i64 } }),
    )
    .await;
    assert_eq!(flat_code, "invalid_argument", "flat bounds");
    assert!(
        flat_message.contains("min_x < max_x"),
        "the message states the rule: {flat_message}"
    );
    refuse_out_of_range(&client, &out_of_range_highlights()).await;
    let (proj_code, proj_message, _) = call_err(
        &client,
        "render_topdown",
        json!({ "projection": "birdseye" }),
    )
    .await;
    assert_eq!(proj_code, "invalid_argument", "not a projection");
    assert!(
        proj_message.contains("\"isometric\""),
        "the message lists the projections: {proj_message}"
    );
    let (size_code, _, _) = call_err(&client, "render_topdown", json!({ "width": 8_u64 })).await;
    assert_eq!(size_code, "invalid_argument", "a width below the floor");
}

#[tokio::test]
async fn an_empty_map_needs_explicit_bounds_and_then_frames_them() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let extra = pwad_empty_map(dir.path());
    call(&client, "open_wad", path_arg(&extra)).await;
    let (code, message, _) = call_err(&client, "render_topdown", json!({})).await;
    assert_eq!(code, "invalid_argument", "nothing to frame");
    assert!(
        message.contains("bounds"),
        "the message names the fix: {message}"
    );
    let report = call(
        &client,
        "render_topdown",
        json!({
            "width": 64_u64, "height": 64_u64,
            "bounds": { "min_x": -128_i64, "min_y": -128_i64, "max_x": 128_i64, "max_y": 128_i64 },
        }),
    )
    .await;
    assert_eq!(report["width"], json!(64_u64), "the requested size");
    let image = rendered_image(&report).await;
    assert_eq!(non_background(&image), 0, "an empty map is pure background");
}
