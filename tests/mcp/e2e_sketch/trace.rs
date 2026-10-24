//! The chain's measuring slice: `trace_sketch` over the drawn floor plan
//! answers three welded rectangle regions whose shared walls carry one x
//! coordinate across each pair, two bridged gaps measuring the 40-px breaks at
//! their mapped centres and listed on exactly their two rooms, two marks - the
//! red dot in `exit`, the black X in `start` - each listed on its region, and
//! adjacency exactly for the two touching pairs. The door width the plan goes
//! on to ask for is checked here too: a multiple of the shared grid, at least
//! 64 and within half a grid of the measured break scaled, against the slack
//! constant that lives here with these asserts. The sheet, the shared `units`,
//! the region, gap and mark readers and the request builders all come from
//! [`crate::e2e_sketch`].

use serde_json::{Value, json};

use crate::e2e_sketch::{
    GRID, MIN_DOOR_WIDTH, ROOM_NAMES, SCALE_NUM, door_width, floor_plan, gap_between, id_pair,
    mark_of_colour, max_x, min_x, points_of, region_by_id, rooms_with_source_ids, source_id,
    trace_request, wanted_pair,
};
use crate::support::{call, connect, write_png};

/// How far a nearest-grid door width may drift from the scaled
/// measurement.
const WIDTH_SLACK: i64 = 8_i64;

/// The width derived from the A|B gap is a grid multiple of at
/// least 64 and within half a grid of the measured break scaled.
fn assert_door_widths(report: &Value, rooms: &[Value]) {
    let gap = gap_between(report, rooms, "start", "hall");
    let px = gap["width_px"].as_i64().expect("a width");
    let width = door_width(&gap["width_px"]);
    assert_eq!(
        width.checked_rem(GRID).expect("a grid above 0"),
        0,
        "the door width {width} is a grid multiple"
    );
    assert!(width >= MIN_DOOR_WIDTH, "and at least 64 wide");
    let scaled = px.checked_mul(SCALE_NUM).expect("a test width");
    let drift = width.max(scaled).saturating_sub(width.min(scaled));
    assert!(drift <= WIDTH_SLACK, "{width} against the scaled {scaled}");
}

#[tokio::test]
async fn the_trace_measures_three_regions_two_gaps_two_marks_and_adjacency() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "floor-plan.png", &floor_plan());
    let report = call(&client, "trace_sketch", trace_request(&file)).await;
    assert_eq!(report["image"]["width"], json!(600_u64), "the sheet");
    assert_eq!(
        report["image"]["height"],
        json!(400_u64),
        "the asked size stands"
    );
    assert_eq!(
        report["close_gaps_px"],
        json!(24_u64),
        "the asked radius is what was used at 1/1"
    );
    assert!(
        report["warnings"].as_array().expect("warnings").is_empty(),
        "a clean drawing is silent: {}",
        report["warnings"]
    );
    assert_regions_touch_on_one_coordinate(&report);
    assert_two_gaps_between_the_rooms(&report);
    assert_marks_in_the_right_rooms(&report);
}

/// Three rectangle regions whose shared walls carry one coordinate
/// across each pair, and adjacency exactly for the touching pairs.
fn assert_regions_touch_on_one_coordinate(report: &Value) {
    let regions = report["regions"].as_array().expect("a region list");
    assert_eq!(regions.len(), 3, "three enclosed rooms");
    let rooms = rooms_with_source_ids(report);
    assert_shared_wall(report, &rooms, "start", "hall");
    assert_shared_wall(report, &rooms, "hall", "exit");
    assert_adjacency(report, &rooms);
}

/// The two named rooms trace to rectangles whose shared wall is one
/// x coordinate for both.
fn assert_shared_wall(report: &Value, rooms: &[Value], one: &str, two: &str) {
    let west = region_by_id(report, &source_id(rooms, one));
    let east = region_by_id(report, &source_id(rooms, two));
    assert!(
        west["is_rectangle"].as_bool().expect("a flag"),
        "{one} is a rectangle"
    );
    assert!(
        east["is_rectangle"].as_bool().expect("a flag"),
        "{two} is a rectangle"
    );
    assert_eq!(
        max_x(&points_of(west)),
        min_x(&points_of(east)),
        "the {one}|{two} wall is one x for both rooms"
    );
}

/// Exactly the two touching pairs share walls, each with a measured
/// border.
fn assert_adjacency(report: &Value, rooms: &[Value]) {
    let adjacency = report["adjacency"].as_array().expect("an adjacency list");
    assert_eq!(adjacency.len(), 2, "the rooms touch in two pairs");
    assert_one_adjacency(report, rooms, "start", "hall");
    assert_one_adjacency(report, rooms, "hall", "exit");
}

/// One adjacency row names the two rooms' traced ids and carries a
/// positive shared border.
fn assert_one_adjacency(report: &Value, rooms: &[Value], one: &str, two: &str) {
    let wanted = wanted_pair(rooms, one, two);
    let hit = report["adjacency"]
        .as_array()
        .expect("an adjacency list")
        .iter()
        .find(|row| id_pair(&row["a"], &row["b"]) == wanted)
        .unwrap_or_else(|| panic!("{one} and {two} touch: {}", report["adjacency"]));
    assert!(
        hit["shared_px"].as_u64().expect("a span") > 0,
        "the {one}|{two} border measures: {hit}"
    );
}

/// Two gaps with the right `between` pairs, the 40-px breaks
/// measured, each listed on exactly its two rooms' rows and with a
/// mapped centre.
fn assert_two_gaps_between_the_rooms(report: &Value) {
    let rooms = rooms_with_source_ids(report);
    let gaps = report["gaps"].as_array().expect("a gap list");
    assert_eq!(gaps.len(), 2, "the two wall breaks: {gaps:?}");
    assert_door_widths(report, &rooms);
    assert_gap_between_two(report, &rooms, "start", "hall");
    assert_gap_between_two(report, &rooms, "hall", "exit");
}

/// The gap between two named rooms measures the 40-px break, maps
/// its centre and is listed on exactly those two region rows.
fn assert_gap_between_two(report: &Value, rooms: &[Value], one: &str, two: &str) {
    let gap = gap_between(report, rooms, one, two);
    let width = gap["width_px"].as_u64().expect("a width");
    assert!(
        (30_u64..=50_u64).contains(&width),
        "the 40-px break between {one} and {two} measures {width}"
    );
    assert!(
        gap["map_center"].is_array(),
        "the gap centre maps under the shared units"
    );
    let regions = report["regions"].as_array().expect("a region list");
    let borders: Vec<&str> = ROOM_NAMES
        .into_iter()
        .filter(|name| region_borders_gap(regions, rooms, name, gap))
        .collect();
    assert_eq!(
        borders,
        vec![one, two],
        "the {one}|{two} gap is listed on exactly its two rooms"
    );
}

/// Whether the room named `name` lists the gap's id on its region.
fn region_borders_gap(regions: &[Value], rooms: &[Value], name: &str, gap: &Value) -> bool {
    let id = source_id(rooms, name);
    let row = regions
        .iter()
        .find(|region| region["id"] == json!(id))
        .unwrap_or_else(|| panic!("no region {id}"));
    row["gaps"]
        .as_array()
        .expect("a gap list")
        .contains(&gap["id"])
}

/// Two marks: the red dot in `exit`, the black X in `start`, each
/// listed on its region and carrying a mapped centre.
fn assert_marks_in_the_right_rooms(report: &Value) {
    let rooms = rooms_with_source_ids(report);
    let marks = report["marks"].as_array().expect("a mark list");
    assert_eq!(marks.len(), 2, "the dot and the X: {marks:?}");
    let red = mark_of_colour(report, "#FF0000");
    let black = mark_of_colour(report, "#000000");
    assert_eq!(
        red["region"],
        json!(source_id(&rooms, "exit")),
        "the red dot sits in the right room"
    );
    assert_eq!(
        black["region"],
        json!(source_id(&rooms, "start")),
        "the black X sits in the left room"
    );
    assert_mark_listed(report, &rooms, red, "exit");
    assert_mark_listed(report, &rooms, black, "start");
}

/// One mark carries a mapped centre and is listed on the region of
/// the room named `name`.
fn assert_mark_listed(report: &Value, rooms: &[Value], mark: &Value, name: &str) {
    assert!(
        mark["map_center"].is_array(),
        "the mark {} centre maps",
        mark["id"]
    );
    let row = region_by_id(report, &source_id(rooms, name));
    assert!(
        row["marks"]
            .as_array()
            .expect("a mark list")
            .contains(&mark["id"]),
        "{name} lists its mark {}",
        mark["id"]
    );
}
