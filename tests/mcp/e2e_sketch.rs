//! The `sketch-to-map` guide chain, end to end: a harness-drawn 600x400
//! floor plan (room A left 200x300, room B middle 160x160 sharing A's right
//! wall, room C right 200x300 sharing B's right wall, 5-px black walls with a
//! 40-px break in each shared column, a red 10x10 dot in C and a black 9x9 X
//! in A) is written to a temporary file, opened on a session (`open_iwad`
//! over the door-wardrobe fixture `IWAD`, `new_wad` for the work) and
//! measured by `trace_sketch` with `units` `scale_num: 2, flip_y: true` and
//! `grid: 16` into exactly three welded, rectangle-traced regions, two
//! bridged gaps sitting between the right region pairs, two marks and A-B /
//! B-C adjacency - every label read off the report's `id`/`between`/`region`
//! fields and mapped to its room by the geometry of its `points`, never
//! assumed. The plan that report answers with is built here as guide sections
//! 3-5 direct: the `plan_rooms` renamed `start`, `hall` and `exit` with the
//! exit floor raised to 32, a `stairs` connection at the B|C gap's measured
//! `center`, a `locked_blue` door at the A|B gap's centre with a width derived
//! from that gap's `width_px` through the same `units` (a 16-multiple of at
//! least 64), a `Blue Keycard` pinned at the X's measured centre in `start` -
//! reachable before the door - an `imp` pinned at the red dot's centre, the
//! player start in `start`, a `walk_over` exit feature and the `tech` theme -
//! and [`build_the_traced_plan`] stands the whole chain up for every slice.
//! The four slices are the topic modules: `trace` measures the drawing,
//! `build` proves the built map plays, `views` looks at it from above and from
//! the player start, and `save_and_lock` ships it and takes the key away.

use std::path::Path;

use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::render::Pt;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::iwad_features;
use crate::support::{call, connect, fill_rect, paper_sheet, write_png};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// Pixels across the drawn sheet.
const SHEET_WIDTH: u32 = 600_u32;

/// Pixels down the drawn sheet.
const SHEET_HEIGHT: u32 = 400_u32;

/// The opaque black pen of the walls and the X.
const INK: [u8; 4] = [0, 0, 0, 255];

/// The opaque red pen of the dot.
const RED: [u8; 4] = [255, 0, 0, 255];

/// The 5-px wall bars of the plan: A's west, top and bottom, the A|B
/// column split by its 40-px break, B's top and bottom, the B|C
/// column split by its break, and C's top, bottom and east. The
/// rooms measure 200x300, 160x160 and 200x300 as drawn (walls
/// included), and no wall comes nearer to the sheet's frame than
/// 25 px - beyond the 24-px closing radius, so the closing cannot
/// bridge wall to frame and invent edge gaps.
const WALLS: [(u32, u32, u32, u32); 12] = [
    (25, 50, 30, 350),
    (25, 50, 225, 55),
    (25, 345, 225, 350),
    (220, 50, 225, 180),
    (220, 220, 225, 350),
    (220, 120, 380, 125),
    (220, 275, 380, 280),
    (375, 50, 380, 180),
    (375, 220, 380, 350),
    (375, 50, 575, 55),
    (375, 345, 575, 350),
    (570, 50, 575, 350),
];

/// The X's centre in the left room, off-centre so the room-centre
/// player start cannot overlap it.
const CROSS_CENTRE: (u32, u32) = (95, 290);

/// The X's diagonal half-length: arms of 4 px make a 9x9 box.
const CROSS_ARM: i64 = 4_i64;

/// The red dot's centre in the right room.
const DOT_CENTRE: (u32, u32) = (475, 150);

/// The dot's half-side: 5 makes a 10x10 box.
const DOT_HALF: u32 = 5_u32;

/// Plan scale shared by `trace_sketch` and `build_map_from_plan`.
const SCALE_NUM: i64 = 2_i64;

/// Snap grid shared by both calls, map units.
const GRID: i64 = 16_i64;

/// Closing radius bridging the 40-px breaks (twice it is 48 px).
const CLOSE_GAPS: i64 = 24_i64;

/// The raised exit room's floor, map units.
const EXIT_FLOOR: i64 = 32_i64;

/// The narrowest door the guide allows, map units.
const MIN_DOOR_WIDTH: i64 = 64_i64;

/// The plan thing name of the card the blue door wants.
const KEY_NAME: &str = "Blue Keycard";

/// The map the chain builds beside the fixture's own `MAP01`.
const MAP: &str = "MAP05";

/// The west-to-east guide names of the three traced rooms.
const ROOM_NAMES: [&str; 3] = ["start", "hall", "exit"];

/// The doomednum of the thing the plan's `player_start` row stands.
const PLAYER_ONE: i64 = 1_i64;

/// The drawn floor plan: three rooms with 5-px walls, a 40-px break
/// in each shared column, the red dot in the right room and the
/// black X in the left one.
fn floor_plan() -> RgbaImage {
    let mut sheet = paper_sheet(SHEET_WIDTH, SHEET_HEIGHT);
    for wall in WALLS {
        fill_rect(&mut sheet, wall, INK);
    }
    let (cx, cy) = DOT_CENTRE;
    let x0 = cx.checked_sub(DOT_HALF).expect("test dot fits");
    let y0 = cy.checked_sub(DOT_HALF).expect("test dot fits");
    let x1 = cx.checked_add(DOT_HALF).expect("test dot fits");
    let y1 = cy.checked_add(DOT_HALF).expect("test dot fits");
    fill_rect(&mut sheet, (x0, y0, x1, y1), RED);
    draw_cross(&mut sheet);
    sheet
}

/// The 1-px offsets of the X's four diagonal rays, step by step,
/// the centre met once by the matching `(0, 0)` pair.
fn cross_offsets() -> Vec<(i64, i64)> {
    let mut offsets: Vec<(i64, i64)> = Vec::new();
    for arm in 0..=CROSS_ARM {
        let back = arm.checked_neg().expect("test arm fits");
        offsets.extend([(arm, arm), (back, arm), (arm, back), (back, back)]);
    }
    offsets
}

/// One 1-px ink dab at `centre` moved by `offset`.
fn ink_dab(sheet: &mut RgbaImage, centre: (i64, i64), offset: (i64, i64)) {
    let column = centre.0.checked_add(offset.0).expect("test x fits");
    let row = centre.1.checked_add(offset.1).expect("test y fits");
    let x = u32::try_from(column).expect("test column fits");
    let y = u32::try_from(row).expect("test row fits");
    let edge_x = x.checked_add(1).expect("test edge fits");
    let edge_y = y.checked_add(1).expect("test edge fits");
    fill_rect(sheet, (x, y, edge_x, edge_y), INK);
}

/// Paints the 1-px-thick X at `CROSS_CENTRE`; its diagonal steps
/// join under the analyser's 8-connectivity.
fn draw_cross(sheet: &mut RgbaImage) {
    let (cx, cy) = CROSS_CENTRE;
    let centre = (i64::from(cx), i64::from(cy));
    for offset in cross_offsets() {
        ink_dab(sheet, centre, offset);
    }
}

/// The `units` object the guide says every call must share.
fn units() -> Value {
    json!({ "scale_num": SCALE_NUM, "flip_y": true })
}

/// The trace request over `source`: the shared `units` and `grid`,
/// and a closing radius that bridges the 40-px breaks.
fn trace_request(source: &str) -> Value {
    json!({
        "source": source,
        "units": units(),
        "grid": GRID,
        "close_gaps": CLOSE_GAPS,
    })
}

/// The session of the chain: the door-wardrobe `IWAD` (the fixture
/// whose pictures let the plan's `tech` theme dress doors, jambs,
/// panels, flats and walls) over an empty working `PWAD`.
async fn session(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_features(dir);
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    call(&client, "new_wad", json!({})).await;
    client
}

/// The `(x, y)` points of a region or plan-room row.
fn points_of(row: &Value) -> Vec<(i64, i64)> {
    row["points"]
        .as_array()
        .expect("a point list")
        .iter()
        .map(|point| {
            let pair = point.as_array().expect("an [x, y] pair");
            (
                pair.first().and_then(Value::as_i64).expect("an x"),
                pair.get(1).and_then(Value::as_i64).expect("a y"),
            )
        })
        .collect()
}

/// The smallest `x` of a point list.
fn min_x(points: &[(i64, i64)]) -> i64 {
    points.iter().map(|point| point.0).min().expect("a point")
}

/// The largest `x` of a point list.
fn max_x(points: &[(i64, i64)]) -> i64 {
    points.iter().map(|point| point.0).max().expect("a point")
}

/// The `plan_rooms` rows sorted west to east by their own measured
/// points and renamed to the guide names, so no traced region label
/// is ever assumed.
fn named_rooms(report: &Value) -> Vec<Value> {
    let mut rooms: Vec<Value> = report["plan_rooms"]
        .as_array()
        .expect("a plan-room list")
        .clone();
    assert_eq!(rooms.len(), ROOM_NAMES.len(), "one room per name");
    rooms.sort_by_key(|room| min_x(&points_of(room)));
    for (room, name) in rooms.iter_mut().zip(ROOM_NAMES) {
        room["id"] = json!(name);
    }
    rooms
}

/// The renamed room rows with the traced region id they came from
/// remembered as `source_id`: region rows and plan-room rows are
/// paired by their identical points, never by order.
fn rooms_with_source_ids(report: &Value) -> Vec<Value> {
    let mut rooms = named_rooms(report);
    let regions = report["regions"].as_array().expect("a region list");
    assert_eq!(regions.len(), rooms.len(), "one region per plan room");
    for room in &mut rooms {
        let hit = regions
            .iter()
            .find(|row| row["points"] == room["points"])
            .unwrap_or_else(|| panic!("no region row for {}", room["id"]));
        room["source_id"] = hit["id"].clone();
    }
    rooms
}

/// The traced region id of the room named `name`.
fn source_id(rooms: &[Value], name: &str) -> String {
    rooms
        .iter()
        .find(|room| room["id"] == json!(name))
        .unwrap_or_else(|| panic!("no room {name}"))["source_id"]
        .as_str()
        .expect("a traced id")
        .to_owned()
}

/// The report's `regions` row for region id `id`.
fn region_by_id<'res>(report: &'res Value, id: &str) -> &'res Value {
    report["regions"]
        .as_array()
        .expect("a region list")
        .iter()
        .find(|row| row["id"] == json!(id))
        .unwrap_or_else(|| panic!("no region {id}"))
}

/// The sorted pair of the two id texts, for order-free compares of
/// `between` lists and adjacency rows.
fn id_pair(one: &Value, two: &Value) -> Vec<String> {
    let mut pair: Vec<String> = [one, two]
        .into_iter()
        .map(|value| value.as_str().expect("an id text").to_owned())
        .collect();
    pair.sort();
    pair
}

/// The sorted pair of traced ids the rooms named `one` and `two`
/// carry.
fn wanted_pair(rooms: &[Value], one: &str, two: &str) -> Vec<String> {
    let mut pair: Vec<String> = vec![source_id(rooms, one), source_id(rooms, two)];
    pair.sort();
    pair
}

/// Whether one gap row's `between` names exactly the wanted pair.
fn gap_names(gap: &Value, wanted: &[String]) -> bool {
    between_ids(gap) == wanted
}

/// The `between` ids of a gap row, sorted, empty when it names none.
fn between_ids(gap: &Value) -> Vec<String> {
    let mut ids: Vec<String> = gap["between"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// The single gap row whose `between` names the two rooms' traced
/// ids.
fn gap_between<'res>(report: &'res Value, rooms: &[Value], one: &str, two: &str) -> &'res Value {
    let wanted = wanted_pair(rooms, one, two);
    report["gaps"]
        .as_array()
        .expect("a gap list")
        .iter()
        .find(|gap| gap_names(gap, &wanted))
        .unwrap_or_else(|| panic!("no gap between {one} and {two}"))
}

/// The mark row coloured `color`.
fn mark_of_colour<'res>(report: &'res Value, color: &str) -> &'res Value {
    report["marks"]
        .as_array()
        .expect("a mark list")
        .iter()
        .find(|mark| mark["color"] == json!(color))
        .unwrap_or_else(|| panic!("no mark coloured {color}"))
}

/// The door width the guide derives from a measured break: the
/// gap's `width_px` through the plan scale, rounded to a multiple
/// of the shared grid and never narrower than [`MIN_DOOR_WIDTH`].
fn door_width(width_px: &Value) -> i64 {
    let px = width_px.as_i64().expect("a width");
    let scaled = px.checked_mul(SCALE_NUM).expect("a test width");
    let half = GRID.checked_div(2).expect("a grid above 1");
    let rounded = scaled
        .checked_add(half)
        .expect("a test width")
        .checked_div(GRID)
        .expect("a grid")
        .checked_mul(GRID)
        .expect("a width");
    rounded.max(MIN_DOOR_WIDTH)
}

/// One `[x, y]` JSON pair as a map point.
fn point_of(row: &Value) -> Pt {
    let list = row.as_array().expect("an [x, y] pair");
    (
        list.first().and_then(Value::as_i64).expect("an x"),
        list.get(1).and_then(Value::as_i64).expect("a y"),
    )
}

/// The `find_elements` row of the map's one player 1 start.
async fn player_start(client: &Client) -> Value {
    let found = call(
        client,
        "find_elements",
        json!({
            "map": MAP,
            "kind": "things",
            "where": [{ "field": "type", "op": "eq", "value": PLAYER_ONE }],
        }),
    )
    .await;
    assert_eq!(found["count"], json!(1_u64), "exactly one player start");
    found["rows"].get(0).expect("the start row").clone()
}

/// The `{"path": ...}` argument object for `file`.
fn path_arg(file: &Path) -> Value {
    json!({ "path": file.display().to_string() })
}

/// `MAP`'s five element counts, as `get_map_summary` reports them.
async fn map_counts(client: &Client) -> Value {
    let summary = call(client, "get_map_summary", json!({ "map": MAP })).await;
    assert_eq!(summary["name"], json!(MAP), "the summary names the map");
    summary["counts"].clone()
}

/// Everything the chain's later slices read: the session with the
/// traced plan built as [`MAP`], the drawn plan's `PNG` path (a valid
/// `render_topdown` `underlay` source), the `trace_sketch` report it
/// was measured from and the `build_map_from_plan` report that stood
/// the map up.
struct Chain {
    /// The connected client holding the built session.
    client: Client,
    /// The sketch file's path text.
    sketch: String,
    /// The `trace_sketch` report over `sketch`.
    trace: Value,
    /// The `build_map_from_plan` report for the traced plan.
    built: Value,
}

/// Draws the floor plan, opens the session, traces the drawing and
/// builds the traced plan: the whole first slice's chain, for every
/// test that looks at, saves or varies the built map.
async fn build_the_traced_plan(dir: &Path) -> Chain {
    let client = session(dir).await;
    let sketch = write_png(dir, "floor-plan.png", &floor_plan());
    let trace = call(&client, "trace_sketch", trace_request(&sketch)).await;
    let built = call(&client, "build_map_from_plan", plan_request(&trace)).await;
    Chain {
        client,
        sketch,
        trace,
        built,
    }
}

/// One plan room row: the traced footprint under its guide name,
/// with the raised floor the exit room asks for.
fn plan_room_row(room: &Value) -> Value {
    if room["id"] == json!("exit") {
        return json!({ "id": room["id"], "points": room["points"], "floor": EXIT_FLOOR });
    }
    json!({ "id": room["id"], "points": room["points"] })
}

/// The plan built from the trace report exactly as the guide
/// directs: renamed `plan_rooms` with the exit raised, one
/// connection per measured gap at its measured centre (the A|B one
/// a `locked_blue` door whose `width` derives from that gap's
/// `width_px` through the same `units`), the two marks as named
/// things pinned at their measured points, the player start in
/// `start`, a `walk_over` exit in `exit` and the `tech` theme.
fn plan_request(report: &Value) -> Value {
    let rooms = rooms_with_source_ids(report);
    let door = gap_between(report, &rooms, "start", "hall");
    let steps = gap_between(report, &rooms, "hall", "exit");
    let key = mark_of_colour(report, "#000000");
    let imp = mark_of_colour(report, "#FF0000");
    let planned: Vec<Value> = rooms.iter().map(plan_room_row).collect();
    json!({
        "map": MAP,
        "theme": "tech",
        "units": units(),
        "grid": GRID,
        "rooms": planned,
        "connections": [
            {
                "id": "blue_gate",
                "from": "start",
                "to": "hall",
                "kind": "door",
                "style": "locked_blue",
                "at": door["center"],
                "width": door_width(&door["width_px"]),
            },
            {
                "id": "steps",
                "from": "hall",
                "to": "exit",
                "kind": "stairs",
                "at": steps["center"],
            }
        ],
        "features": [{ "kind": "exit", "room": "exit", "style": "walk_over" }],
        "things": [
            { "name": KEY_NAME, "x": key["center"][0], "y": key["center"][1] },
            { "name": "imp", "x": imp["center"][0], "y": imp["center"][1] },
        ],
        "player_start": { "room": "start" },
        "finalize": true,
    })
}

#[cfg(test)]
#[path = "e2e_sketch/build.rs"]
mod build;

#[cfg(test)]
#[path = "e2e_sketch/save_and_lock.rs"]
mod save_and_lock;

#[cfg(test)]
#[path = "e2e_sketch/trace.rs"]
mod trace;

#[cfg(test)]
#[path = "e2e_sketch/views.rs"]
mod views;
