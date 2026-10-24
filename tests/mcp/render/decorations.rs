//! The extras a top-down frame carries over its map and the refusals that
//! bound them: an `underlay` source that is no picture answering
//! `format_error`, every overlay refused on the isometric projection with the
//! grid and the labels, `grid` lines landing on map-unit multiples in their
//! minor and major shades (a grid too fine noting its own skip), `labels`
//! numbering each sector inside itself and `annotations` refused past their
//! character and row limits and otherwise drawing their colour at their world
//! point. `render_topdown`' own defaults following the session settings
//! closes the group. The picture arguments come from
//! [`crate::render::overlays`].

use std::fs;

use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::render::overlays::{sketch_png, sketch_underlay};
use crate::render::{Transform, iwad_client, rendered_image};
use crate::support::{call, call_err, connect};

/// Whether any pixel within six of `at` equals `colour`.
fn painted_near(image: &RgbaImage, at: (u32, u32), colour: [u8; 4]) -> bool {
    let columns = (i64::from(at.0).saturating_sub(6)..=i64::from(at.0).saturating_add(6))
        .filter_map(|col| u32::try_from(col).ok());
    let rows = (i64::from(at.1).saturating_sub(6)..=i64::from(at.1).saturating_add(6))
        .filter_map(|row| u32::try_from(row).ok());
    columns
        .flat_map(move |col| {
            rows.clone()
                .map(move |row| image.get(col, row) == Some(colour))
        })
        .any(|hit| hit)
}

#[tokio::test]
async fn an_underlay_source_that_is_no_picture_is_a_format_error() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = dir.path().join("not-a-picture.bin");
    fs::write(&file, b"definitely not an image").expect("the file writes");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let (code, message, _) = call_err(
        &client,
        "render_topdown",
        json!({ "underlay": { "source": file.display().to_string() } }),
    )
    .await;
    assert_eq!(code, "format_error", "the bytes are no image");
    assert!(
        message.contains("image") || message.contains("format"),
        "the message says what failed: {message}"
    );
}

#[tokio::test]
async fn overlays_are_refused_on_the_isometric_projection() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    call(&client, "create_map", json!({ "name": "MAP01" })).await;
    let file = sketch_png(dir.path());
    let (code, message, _) = call_err(
        &client,
        "render_topdown",
        json!({ "projection": "isometric", "underlay": sketch_underlay(&file) }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "an overlay on isometric");
    assert!(
        message.contains("overlays need the topdown projection"),
        "the message names the projection: {message}"
    );
    let (grid_code, _, _) = call_err(
        &client,
        "render_topdown",
        json!({ "projection": "isometric", "grid": 64_i64 }),
    )
    .await;
    assert_eq!(grid_code, "invalid_argument", "a grid on isometric");
    let (label_code, _, _) = call_err(
        &client,
        "render_topdown",
        json!({ "projection": "isometric", "labels": { "sectors": true } }),
    )
    .await;
    assert_eq!(label_code, "invalid_argument", "labels on isometric");
}

#[tokio::test]
async fn grid_lines_land_on_map_unit_multiples_and_a_fine_grid_notes_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64,
            "grid": 64_i64, "show_things": false,
        }),
    )
    .await;
    assert_eq!(report["notes"], json!([]), "the grid drew");
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (minor_x, minor_y) = view.project(64, -16);
    assert_eq!(
        image.get(minor_x, minor_y).expect("a fitted pixel"),
        [42, 42, 42, 255],
        "the x = 64 minor line below the map"
    );
    let (major_x, major_y) = view.project(512, -16);
    assert_eq!(
        image.get(major_x, major_y).expect("a fitted pixel"),
        [60, 60, 60, 255],
        "the eighth line at x = 512 draws major"
    );
    let (free_x, free_y) = view.project(96, -16);
    assert_eq!(
        image.get(free_x, free_y).expect("a fitted pixel"),
        [0, 0, 0, 255],
        "between the lines stays background"
    );
    let (cross_x, cross_y) = view.project(96, 128);
    assert_eq!(
        image.get(cross_x, cross_y).expect("a fitted pixel"),
        [42, 42, 42, 255],
        "the y = 128 line crosses the middle"
    );
    let skipped = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 64_u64, "height": 64_u64, "grid": 8_i64,
        }),
    )
    .await;
    let notes = skipped["notes"].as_array().expect("one note");
    assert_eq!(notes.len(), 1, "the fine grid reports its skip: {notes:?}");
    assert!(
        notes
            .first()
            .and_then(Value::as_str)
            .is_some_and(|note| note.contains("grid skipped")),
        "the note names the skip: {notes:?}"
    );
}

#[tokio::test]
async fn sector_labels_sit_inside_their_sectors() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64,
            "show_things": false, "labels": { "sectors": true },
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let label = [255_u8, 255, 128, 255];
    let hall = view.project(128, 128);
    let room = view.project(384, 128);
    assert!(
        painted_near(&image, hall, label),
        "the hall numbers itself at its inside point {hall:?}"
    );
    assert!(
        painted_near(&image, room, label),
        "the back room numbers itself at its inside point {room:?}"
    );
}

#[tokio::test]
async fn annotation_rows_beyond_the_limits_are_refused_and_one_draws() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let long_text = "X".repeat(33_usize);
    let (code, message, _) = call_err(
        &client,
        "render_topdown",
        json!({ "annotations": [{ "x": 0_i64, "y": 0_i64, "text": long_text }] }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "33 characters");
    assert!(
        message.contains("32"),
        "the message names the character limit: {message}"
    );
    let many: Vec<Value> = (0_u32..65_u32)
        .map(|index| json!({ "x": 0_i64, "y": 0_i64, "text": format!("N{index}") }))
        .collect();
    let (count_code, count_message, _) =
        call_err(&client, "render_topdown", json!({ "annotations": many })).await;
    assert_eq!(count_code, "invalid_argument", "65 rows");
    assert!(
        count_message.contains("64"),
        "the message names the row limit: {count_message}"
    );
    let (colour_code, _, _) = call_err(
        &client,
        "render_topdown",
        json!({ "annotations": [{ "x": 0_i64, "y": 0_i64, "text": "A", "color": "green" }] }),
    )
    .await;
    assert_eq!(colour_code, "invalid_argument", "not a hex colour");
    let report = call(
        &client,
        "render_topdown",
        json!({
            "map": "MAP01", "width": 200_u64, "height": 120_u64, "show_things": false,
            "background": "#101010",
            "annotations": [{ "x": 128_i64, "y": 128_i64, "text": "hall", "color": "#FF00FF" }],
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    assert!(
        painted_near(&image, view.project(128, 128), [255, 0, 255, 255]),
        "the magenta note stands at its world point"
    );
}

#[tokio::test]
async fn render_topdown_defaults_follow_the_session_settings() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    call(
        &client,
        "set_render_resolution",
        json!({ "width": 320_u64, "height": 200_u64 }),
    )
    .await;
    call(
        &client,
        "set_render_options",
        json!({ "topdown_style": "automap", "show_things": false }),
    )
    .await;
    let report = call(&client, "render_topdown", json!({ "map": "MAP01" })).await;
    assert_eq!(report["width"], json!(320_u64), "the settings size");
    assert_eq!(report["height"], json!(200_u64), "the settings size");
    let image = rendered_image(&report).await;
    let view = Transform::of(&report);
    let (x, y) = view.project(0, 128);
    assert_eq!(
        image.get(x, y).expect("a fitted pixel"),
        [252, 0, 0, 255],
        "the settings' automap style drew the outer wall red"
    );
    let (px, py) = view.project(64, 128);
    assert_eq!(
        image.get(px, py).expect("a fitted pixel"),
        [0, 0, 0, 255],
        "show_things off left the player spot dark"
    );
}
