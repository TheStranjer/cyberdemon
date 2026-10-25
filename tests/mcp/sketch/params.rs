//! The knobs and the refusals: a downscaled sketch reporting original pixel
//! coordinates, `units` adding grid-snapped map coordinates (and their
//! absence without them), a crop leaving coordinates in image pixels,
//! `ink.mode: "color"` tracing only the matching pen, a blank sheet
//! measuring nothing without erroring, and the bad `crop`, `max_size`, pen
//! and grid answers.

use serde_json::json;

use crate::sketch::{
    assert_all_map_grids, assert_scaled_regions, max_x, paper, points_of, sketch, trace_args,
    two_pens, write_png,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn a_downscaled_sketch_reports_original_pixel_coordinates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let small_file = write_png(dir.path(), "small.png", &sketch(1));
    let big_file = write_png(dir.path(), "big.png", &sketch(6));
    let small = call(&client, "trace_sketch", trace_args(&small_file)).await;
    let mut big_args = trace_args(&big_file);
    big_args["max_size"] = json!(600_u64);
    big_args["close_gaps"] = json!(100_i64);
    let big = call(&client, "trace_sketch", big_args).await;
    assert_eq!(big["image"]["width"], json!(2_400_u64), "the source sheet");
    assert_eq!(
        big["image"]["analysed_scale"],
        json!({"num": 1_u64, "den": 4_u64}),
        "600 of 2400 is exactly a quarter"
    );
    assert!(
        (20_u64..=28_u64).contains(&big["stroke_px"].as_u64().expect("a stroke")),
        "the 24-px pen still measures about 24, got {}",
        big["stroke_px"]
    );
    let big_regions = big["regions"].as_array().expect("regions");
    let small_regions = small["regions"].as_array().expect("regions");
    assert_scaled_regions(small_regions, big_regions);
}

#[tokio::test]
async fn units_add_grid_snapped_map_coordinates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let mut with_units = trace_args(&file);
    with_units["units"] = json!({"scale_num": 4_i64, "flip_y": true});
    let report = call(&client, "trace_sketch", with_units).await;
    assert_all_map_grids(&report);
    assert!(
        !report["gaps"][0]["map_center"].is_null(),
        "the gap centre maps too"
    );
    assert!(
        !report["marks"][0]["map_center"].is_null(),
        "and the mark centres"
    );
}

#[tokio::test]
async fn without_units_the_map_coordinate_fields_stay_null() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    assert!(
        report["regions"][0]["map_points"].is_null(),
        "no units, no map"
    );
    assert!(report["gaps"][0]["map_center"].is_null());
    assert!(report["marks"][0]["map_center"].is_null());
}

#[tokio::test]
async fn a_crop_leaves_coordinates_in_original_image_pixels() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let whole = call(&client, "trace_sketch", trace_args(&file)).await;
    // A 1-px inset that keeps every wall at least `close_gaps` pixels
    // from the crop edge, so the closing behaves as on the full sheet
    // and only the crop offset differs.
    let mut cropped_args = trace_args(&file);
    cropped_args["crop"] = json!({"x": 1_u64, "y": 1_u64, "width": 390_u64, "height": 297_u64});
    let cropped = call(&client, "trace_sketch", cropped_args).await;
    assert_eq!(
        cropped["image"]["width"],
        json!(400_u64),
        "the source sheet"
    );
    assert_eq!(
        cropped["regions"], whole["regions"],
        "the offset is added back, so the rows match the whole-sheet run"
    );
}

#[tokio::test]
async fn colour_ink_traces_only_the_matching_pen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "pens.png", &two_pens());
    let mut args = json!({
        "source": file,
        "ink": {"mode": "color", "color": "#FF0000"},
    });
    args["close_gaps"] = json!(16_i64);
    let report = call(&client, "trace_sketch", args).await;
    assert!(report["threshold"].is_null(), "color mode applies no level");
    let regions = report["regions"].as_array().expect("a region list");
    assert_eq!(regions.len(), 1, "only the red rectangle encloses");
    let red_ring = points_of(&regions[0]);
    assert!(
        max_x(&red_ring) <= 101,
        "the traced ring is the red room: {red_ring:?}"
    );
}

#[tokio::test]
async fn a_blank_sheet_measures_nothing_but_is_not_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "blank.png", &paper(200, 150));
    let report = call(&client, "trace_sketch", json!({"source": file})).await;
    assert_eq!(report["regions"], json!([]), "no enclosed areas");
    assert_eq!(report["gaps"], json!([]), "nothing to bridge");
    assert_eq!(report["marks"], json!([]), "nothing to mark");
    assert_eq!(report["plan_rooms"], json!([]), "no rooms to plan");
    let warnings = report["warnings"].as_array().expect("a warning list");
    assert!(
        !warnings.is_empty(),
        "the empty sheet explains itself: {warnings:?}"
    );
    assert!(
        report["debug"]["url"].is_string(),
        "the debug image still ships"
    );
}

#[tokio::test]
async fn bad_crop_max_size_and_pen_are_invalid_arguments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let mut outside = trace_args(&file);
    outside["crop"] = json!({"x": 380_u64, "y": 0_u64, "width": 40_u64, "height": 100_u64});
    let (crop_code, crop_message, _) = call_err(&client, "trace_sketch", outside).await;
    assert_eq!(crop_code, "invalid_argument", "a crop past the edge");
    assert!(crop_message.contains("crop.x"), "{crop_message}");
    let mut small_max = trace_args(&file);
    small_max["max_size"] = json!(64_u64);
    let (size_code, size_message, _) = call_err(&client, "trace_sketch", small_max).await;
    assert_eq!(size_code, "invalid_argument", "max_size below the floor");
    assert!(size_message.contains("max_size"), "{size_message}");
    let no_pen = json!({"source": file, "ink": {"mode": "color"}});
    let (pen_code, pen_message, _) = call_err(&client, "trace_sketch", no_pen).await;
    assert_eq!(pen_code, "invalid_argument", "color without a colour");
    assert!(pen_message.contains("ink.color"), "{pen_message}");
}

#[tokio::test]
async fn bytes_that_are_no_image_report_a_format_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"not an image at all").expect("write the fixture");
    let (code, _message, _) = call_err(
        &client,
        "trace_sketch",
        json!({"source": path.display().to_string()}),
    )
    .await;
    assert_eq!(code, "format_error", "no format could be decoded");
}

#[tokio::test]
async fn a_zero_grid_is_refused_naming_the_parameter() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let mut grid_args = trace_args(&file);
    grid_args["grid"] = json!(0_i64);
    let (code, message, _) = call_err(&client, "trace_sketch", grid_args).await;
    assert_eq!(code, "invalid_argument", "snapping needs a grid");
    assert!(message.contains("grid"), "{message}");
}
