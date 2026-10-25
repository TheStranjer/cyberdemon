//! The three-room measurements: the regions, gap, marks and debug `PNG`
//! the plain sketch traces into, the L's six corners against the
//! rectangles' four, the one welded coordinate and one adjacency across the
//! shared wall, the door break as the one gap between the stacked rooms and
//! the dot and the X as colour- and region-tagged marks.

use serde_json::json;

use crate::sketch::{
    PAPER, assert_channels_close, assert_rectangle, debug_image, max_y, min_y, points_of, region,
    sketch, trace_args, write_png,
};
use crate::support::{call, connect};

#[tokio::test]
async fn three_rooms_trace_to_regions_a_gap_two_marks_and_a_debug_png() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    assert_eq!(report["image"]["width"], json!(400_u64), "the source sheet");
    assert_eq!(report["image"]["height"], json!(300_u64));
    assert_eq!(
        report["image"]["analysed_scale"],
        json!({"num": 1_u64, "den": 1_u64}),
        "a small sheet is analysed at full size"
    );
    assert_eq!(report["stroke_px"], json!(4_u64), "the pen measures 4 px");
    assert!(
        report["threshold"].is_number(),
        "the detected level is reported: {}",
        report["threshold"]
    );
    assert_eq!(report["close_gaps_px"], json!(16_u64), "the asked radius");
    let regions = report["regions"].as_array().expect("a region list");
    assert_eq!(regions.len(), 3, "three enclosed areas");
    assert_eq!(report["warnings"], json!([]), "a clean sketch is silent");
    assert_eq!(report["next_steps"].as_array().expect("steps").len(), 4);
}

#[tokio::test]
async fn the_l_room_keeps_six_points_and_the_rectangles_four() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    let l_shape = points_of(region(&report, "R1"));
    assert_eq!(l_shape.len(), 6, "the L ring has six corners");
    assert!(
        !region(&report, "R1")["is_rectangle"]
            .as_bool()
            .expect("a flag")
    );
    assert_rectangle(&report, "R2");
    assert_rectangle(&report, "R3");
    assert_eq!(
        max_y(&l_shape),
        164,
        "coordinates are original-image pixels, in pixel-corner space"
    );
}

#[tokio::test]
async fn the_shared_wall_carries_one_coordinate_and_one_adjacency() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    let upper = points_of(region(&report, "R2"));
    let lower = points_of(region(&report, "R3"));
    assert_eq!(
        max_y(&upper),
        min_y(&lower),
        "the shared wall is one y for both regions"
    );
    let adjacency = report["adjacency"].as_array().expect("an adjacency list");
    assert_eq!(adjacency.len(), 1, "only the stacked rooms touch");
    let pair = adjacency.first().expect("the pair");
    assert_eq!(pair["a"], json!("R2"), "the lower label first");
    assert_eq!(pair["b"], json!("R3"));
    assert!(
        pair["shared_px"].as_u64().is_some_and(|span| span > 0),
        "the shared border is measured: {pair}"
    );
}

#[tokio::test]
async fn the_door_break_is_one_gap_between_the_stacked_rooms() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    let gaps = report["gaps"].as_array().expect("a gap list");
    assert_eq!(gaps.len(), 1, "exactly the door break was bridged");
    let door = gaps.first().expect("the door row");
    assert_eq!(door["id"], json!("G1"), "scan-order labels");
    assert_eq!(
        door["between"],
        json!(["R2", "R3"]),
        "the pair it separates"
    );
    assert!(
        (24_u64..=36_u64).contains(&door["width_px"].as_u64().expect("a width")),
        "the 30-px break measures about 30, got {}",
        door["width_px"]
    );
    assert_eq!(
        region(&report, "R2")["gaps"],
        json!(["G1"]),
        "listed on both rooms"
    );
    assert_eq!(region(&report, "R3")["gaps"], json!(["G1"]));
    assert_eq!(
        region(&report, "R1")["gaps"],
        json!([]),
        "the L holds no break"
    );
}

#[tokio::test]
async fn the_dot_and_the_x_come_back_as_colour_and_region_tagged_marks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    let marks = report["marks"].as_array().expect("a mark list");
    assert_eq!(marks.len(), 2, "the dot and the X: {marks:?}");
    let red = marks
        .iter()
        .find(|mark| mark["color"] == json!("#FF0000"))
        .expect("the red mark");
    let black = marks
        .iter()
        .find(|mark| mark["color"] == json!("#000000"))
        .expect("the black mark");
    assert_eq!(red["region"], json!("R1"), "the dot sits in the L");
    assert_eq!(black["region"], json!("R2"), "the X sits in the upper room");
    assert!(
        (70_u64..=95_u64).contains(&red["area_px"].as_u64().expect("an area")),
        "the 9x9 dot covers about 81 px, got {}",
        red["area_px"]
    );
    let r1_marks = region(&report, "R1")["marks"].as_array().expect("a list");
    assert!(r1_marks.contains(&red["id"]), "the room lists its mark");
    assert_channels_close(red["color"].as_str().expect("hex"), [255_u64, 0_u64, 0_u64]);
}

#[tokio::test]
async fn the_debug_overlay_is_a_decodable_png_of_the_analysed_size() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let report = call(&client, "trace_sketch", trace_args(&file)).await;
    let overlay = debug_image(&report).await;
    assert_eq!(
        (overlay.width, overlay.height),
        (400_u32, 300_u32),
        "at analysis scale, 1/1 here"
    );
    let painted = (0..overlay.height)
        .flat_map(|row| (0..overlay.width).map(move |column| (column, row)))
        .filter(|&(column, row)| overlay.get(column, row) != Some(PAPER))
        .count();
    assert!(
        painted > 500,
        "the overlay draws regions, ids and marks: {painted}"
    );
}
