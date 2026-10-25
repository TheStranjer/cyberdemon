//! The `sketch-to-map` guide chain, part one: a harness-drawn 600x400
//! floor plan (room A left 200x300, room B middle 160x160 sharing A's
//! right wall, room C right 200x300 sharing B's right wall, 5-px black
//! walls with a 40-px break in each shared column, a red 10x10 dot in C
//! and a black 9x9 X in A) is written to a temporary file, opened on a
//! session (`open_iwad` over the door-wardrobe fixture `IWAD`, `new_wad`
//! for the work) and measured by `trace_sketch` with `units`
//! `scale_num: 2, flip_y: true` and `grid: 16` into exactly three
//! welded, rectangle-traced regions, two bridged gaps sitting between
//! the right region pairs, two marks (the red one inside the right
//! room, the black X inside the left one) and A-B / B-C adjacency -
//! every label read off the report's `id`/`between`/`region` fields and
//! mapped to its room by the geometry of its `points`, never assumed.
//! The plan is then built programmatically from that report as guide
//! sections 3-5 direct: the `plan_rooms` renamed `start`, `hall` and
//! `exit` with the exit floor raised to 32, a `stairs` connection at
//! the B|C gap's measured `center`, a `locked_blue` door at the A|B
//! gap's centre with a width derived from that gap's `width_px` through
//! the same `units` (a 16-multiple of at least 64), a `Blue Keycard`
//! pinned at the X's measured centre in `start` - reachable before the
//! door - an `imp` pinned at the red dot's centre, the player start in
//! `start`, a `walk_over` exit feature and the `tech` theme.
//! `build_map_from_plan` with `finalize: true` answers no `validation`
//! errors, the key and the imp stand exactly at their marks' reported
//! `map_center` (read back through `find_elements`), and
//! `check_playability` answers `ok: true` with every sector reachable
//! (counted against `describe_map`, the plan's own room, door-band and
//! stair-step sectors covering the whole index range), the blue lock
//! present in `keys`/`locks` and opened, no blocker, no stuck things.
//! Part two looks at that map, ships it and takes the key away: the
//! `render_topdown` overview with the sketch as `underlay` and the sector
//! labels on, read at the break's mapped centre for the door's automap
//! line colours; the `render_view` frame from the player start's own
//! coordinates and facing, whose centre column is geometry and not ground;
//! `save_wad` then `new_wad` then `open_wad`, answering the same element
//! counts and `ok` again; and the same plan rebuilt without its
//! `Blue Keycard` row, answering `ok: false` with a `locked` blue blocker
//! on one of the door's own linedefs whose `hint`, repeated in
//! `next_steps`, names `add_thing`.
//!
//! The last section is the same regression at library level, with no
//! client and no asset round trip: [`cyberdemon::gfx::ops::apply_ops`] over
//! the ruled photo fixture, proving the hairline-cleaning chain turns a
//! photograph's uneven light into one clean binarized sheet - walls kept
//! pixel-for-pixel, ruling dropped, and the shadowed top of the room
//! classing exactly like the lit bottom of it. Over that, one wire proof:
//! `process_image` over the sheet's `PNG` file reports both ops step by
//! step and its published asset, fetched back over `HTTP`, is the same
//! clean sheet through the `PNG` encode/decode round trip.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::image::RgbaImage;
    use cyberdemon::gfx::ops::{ImageOp, OpsReport, apply_ops, parse_hex_color};
    use cyberdemon::plan::Units;
    use cyberdemon::render::{AutomapColors, Pt, Rgba, scale_down};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{
        PHOTO_ROOM, PHOTO_RULING_PERIOD, PHOTO_RULING_PHASE, PHOTO_TICK, PHOTO_WALL_PX, asset_png,
        call, connect, fill_rect, paper_sheet, ruled_photo_sheet, write_png,
    };

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
    /// How far a nearest-grid door width may drift from the scaled
    /// measurement.
    const WIDTH_SLACK: i64 = 8_i64;
    /// The doomednum a `Blue Keycard` name resolves to.
    const BLUE_KEYCARD: i64 = 5_i64;
    /// The doomednum an `imp` name resolves to.
    const IMP: i64 = 3001_i64;
    /// The doomednum of the thing the plan's `player_start` row stands.
    const PLAYER_ONE: i64 = 1_i64;
    /// The plan thing name of the card the blue door wants.
    const KEY_NAME: &str = "Blue Keycard";
    /// The map the chain builds beside the fixture's own `MAP01`.
    const MAP: &str = "MAP05";
    /// The west-to-east guide names of the three traced rooms.
    const ROOM_NAMES: [&str; 3] = ["start", "hall", "exit"];
    /// Overview width: wide enough for the built door band to be ten
    /// output pixels across, one column per line.
    const OVERVIEW_WIDTH: u32 = 800_u32;
    /// Overview height.
    const OVERVIEW_HEIGHT: u32 = 600_u32;
    /// The plan's default `wall_thickness`, map units: the depth of the
    /// band the door is cut through, so its two lines stand half this far
    /// either side of the break's snapped centre.
    const WALL_THICKNESS: i64 = 16_i64;
    /// How far a built line may stand off the pixel its map coordinate
    /// lands on: every endpoint is floor-rounded into the picture.
    const PIXEL_SLACK: i64 = 1_i64;
    /// The automap line colours a black-and-white drawing can never blend
    /// into: [`AutomapColors::DEFAULT`]'s wall red, floor-change brown and
    /// ceiling-change yellow. Its fourth class, the `other` grey, is the
    /// very colour the washed paper reaches under the default 50% blend,
    /// so a grey pixel proves nothing and is never counted.
    const LINE_CLASSES: [Rgba; 3] = [
        AutomapColors::DEFAULT.wall,
        AutomapColors::DEFAULT.floor_change,
        AutomapColors::DEFAULT.ceiling_change,
    ];

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
    fn gap_between<'res>(
        report: &'res Value,
        rooms: &[Value],
        one: &str,
        two: &str,
    ) -> &'res Value {
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

    #[tokio::test]
    async fn the_traced_plan_builds_a_fully_playable_map() {
        let dir = tempfile::tempdir().expect("temp dir");
        let chain = build_the_traced_plan(dir.path()).await;
        assert_build_report(&chain.built);
        assert_things_stand_on_their_marks(&chain.client, &chain.trace).await;
        assert_playable(&chain.client, &chain.built).await;
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

    /// The build answers the three named rooms, the `door` and `stairs`
    /// connections in plan order, the `door` and `exit` feature rows,
    /// all three things placed, no judgment-gap warnings and no errors
    /// from the finalized validation.
    fn assert_build_report(built: &Value) {
        let rooms = built["rooms"].as_object().expect("a room bookkeeping");
        for name in ROOM_NAMES {
            assert!(
                rooms.contains_key(name),
                "room {name} owns sectors: {rooms:?}"
            );
        }
        assert_eq!(rooms.len(), ROOM_NAMES.len(), "exactly the three rooms");
        let connections = built["connections"].as_array().expect("a list");
        let connection_kinds: Vec<&str> = connections
            .iter()
            .map(|row| row["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(
            connection_kinds,
            vec!["door", "stairs"],
            "the connections in plan order"
        );
        let features = built["features"].as_array().expect("a list");
        let feature_kinds: Vec<&str> = features
            .iter()
            .map(|row| row["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(
            feature_kinds,
            vec!["door", "exit"],
            "doors first, exits last"
        );
        assert_eq!(
            features.get(1).expect("the exit row")["room"],
            json!("exit"),
            "the level ends in the raised room"
        );
        assert_eq!(
            built["things_placed"],
            json!(3_u64),
            "the key, the imp and the start"
        );
        assert_eq!(
            built["warnings"],
            json!([]),
            "key in hand, exit set: {:?}",
            built["warnings"]
        );
        assert_eq!(
            built["validation"]["errors"],
            json!([]),
            "the finalized map validates error-free: {}",
            built["validation"]
        );
    }

    /// The pinned `x`/`y` plan points travel the shared `units` exactly
    /// like the trace's `map_center` fields: the keycard stands at the
    /// X's map centre, the imp at the red dot's.
    async fn assert_things_stand_on_their_marks(client: &Client, report: &Value) {
        let key = mark_of_colour(report, "#000000")["map_center"].clone();
        expect_thing_at(client, BLUE_KEYCARD, &key).await;
        let imp = mark_of_colour(report, "#FF0000")["map_center"].clone();
        expect_thing_at(client, IMP, &imp).await;
    }

    /// One `find_elements` thing of type `kind` standing at the report
    /// point `map_center`.
    async fn expect_thing_at(client: &Client, kind: i64, map_center: &Value) {
        let found = call(
            client,
            "find_elements",
            json!({
                "map": MAP,
                "kind": "things",
                "where": [{ "field": "type", "op": "eq", "value": kind }],
            }),
        )
        .await;
        assert_eq!(found["count"], json!(1_u64), "one thing of type {kind}");
        let row = found["rows"].get(0).expect("the thing row");
        let wanted = map_center.as_array().expect("a mapped centre");
        assert_eq!(
            row["x"],
            json!(wanted.first().and_then(Value::as_i64).expect("an x")),
            "the pinned x lands"
        );
        assert_eq!(
            row["y"],
            json!(wanted.get(1).and_then(Value::as_i64).expect("a y")),
            "and the pinned y"
        );
    }

    /// `check_playability` answers `ok`: every sector the built map owns
    /// is reached, the blue lock stands in `locks` opened with its key
    /// reached in `keys`, and nothing is blocked or stuck.
    async fn assert_playable(client: &Client, built: &Value) {
        let report = call(client, "check_playability", json!({ "map": MAP })).await;
        assert!(
            report["ok"].as_bool().expect("a verdict"),
            "the traced map plays: {report}"
        );
        assert_eq!(report["blockers"], json!([]), "nothing holds a sector out");
        assert_eq!(report["stuck_things"], json!([]), "every thing stands");
        assert_all_sectors_reachable(client, built, &report).await;
        assert_blue_key_opened(&report);
        assert!(
            report["exits"]
                .as_array()
                .expect("an exit list")
                .iter()
                .any(|exit| exit["reachable"].as_bool().expect("a flag")),
            "a reachable level end: {}",
            report["exits"]
        );
    }

    /// The walk reaches every sector, its total is `describe_map`'s own
    /// sector count, and the plan's room, door-band and stair-step
    /// sectors are exactly the whole map.
    async fn assert_all_sectors_reachable(client: &Client, built: &Value, report: &Value) {
        let described = call(
            client,
            "describe_map",
            json!({ "map": MAP, "render": false }),
        )
        .await;
        let total = report["sectors"]["total"].as_u64().expect("a total");
        assert_eq!(
            described["summary"]["counts"]["sectors"],
            json!(total),
            "the walk and the describe read the same map"
        );
        assert_eq!(
            report["sectors"]["unreachable"],
            json!([]),
            "no sector stays out of reach"
        );
        assert_eq!(
            report["sectors"]["reachable"],
            json!(total),
            "all of them reached"
        );
        assert_sector_cover(built, total);
    }

    /// The indices of every sector the build report names: each room's
    /// and each connection's, deduplicated and ascending.
    fn planned_sectors(built: &Value) -> Vec<u64> {
        let room_lists: Vec<&Value> = built["rooms"]
            .as_object()
            .expect("rooms")
            .values()
            .collect();
        let band_lists: Vec<&Value> = built["connections"]
            .as_array()
            .expect("connections")
            .iter()
            .map(|row| &row["sectors"])
            .collect();
        let mut rows: Vec<u64> = room_lists
            .iter()
            .chain(band_lists.iter())
            .flat_map(|list| list.as_array().expect("a sector list"))
            .map(|sector| sector.as_u64().expect("an index"))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// The planned sectors, room floors and connector bands alike, are
    /// exactly the indices `0..total`, once each, and there are at
    /// least the three rooms' worth.
    fn assert_sector_cover(built: &Value, total: u64) {
        let planned = planned_sectors(built);
        assert!(
            planned.len() >= ROOM_NAMES.len(),
            "at least one sector per planned room"
        );
        let whole: Vec<u64> = (0_u64..total).collect();
        assert_eq!(planned, whole, "the plan's sectors are the whole map");
    }

    /// The walk saw the one blue key and reached it, saw the blue lock
    /// and opened it, and no door stayed shut behind the player.
    fn assert_blue_key_opened(report: &Value) {
        let keys = report["keys"].as_array().expect("a key list");
        assert_eq!(keys.len(), 1, "the one keycard: {keys:?}");
        assert_eq!(
            keys.first().expect("the key")["color"],
            json!("blue"),
            "the card answers the blue lock"
        );
        assert!(
            keys.first().expect("the key")["reachable"]
                .as_bool()
                .expect("a flag"),
            "the key stands where the player reaches it before the door"
        );
        let locks = report["locks"].as_array().expect("a lock list");
        assert!(!locks.is_empty(), "the blue door is a lock the walk saw");
        for lock in locks {
            assert_eq!(lock["color"], json!("blue"), "the one locked gate");
            assert!(
                lock["opened"].as_bool().expect("a flag"),
                "the key opens it: {lock}"
            );
        }
        assert_eq!(
            report["doors_never_opened"],
            json!([]),
            "no door was left shut"
        );
        assert_eq!(
            report["unreachable_things"],
            json!([]),
            "every thing stands in a reached sector"
        );
    }

    /// The `transform` a top-down report answers with, read the way the
    /// tool documents it: `px = offset_x + x * scale_num / scale_den` and
    /// `py = offset_y - y * scale_num / scale_den`, floored.
    struct Fit {
        /// Pixel span of the fitted axis.
        num: i64,
        /// World span of the fitted axis.
        den: i64,
        /// Pixel column of world `x = 0`.
        offset_x: i64,
        /// Pixel row of world `y = 0`.
        offset_y: i64,
    }

    impl Fit {
        /// Reads the fitted mapping out of a `render_topdown` report.
        fn of(report: &Value) -> Self {
            let transform = &report["transform"];
            Self {
                num: transform["scale_num"].as_i64().expect("a scale_num"),
                den: transform["scale_den"].as_i64().expect("a scale_den"),
                offset_x: transform["offset_x"].as_i64().expect("an offset_x"),
                offset_y: transform["offset_y"].as_i64().expect("an offset_y"),
            }
        }

        /// The output pixel of the map point `x`/`y`, through the very
        /// [`scale_down`] the renderer places its own points with.
        fn pixel(&self, x: i64, y: i64) -> Pt {
            (
                self.offset_x
                    .saturating_add(scale_down(x, self.num, self.den)),
                self.offset_y
                    .saturating_sub(scale_down(y, self.num, self.den)),
            )
        }

        /// `units` map units as output pixels, never narrower than one.
        fn span(&self, units: i64) -> i64 {
            scale_down(units, self.num, self.den).max(1_i64)
        }
    }

    /// The chain's `units` as the library type: [`units`]' scale and
    /// vertical mirror, with the shared [`GRID`] for the snapped readings
    /// and grid `1` for the unsnapped underlay placement every
    /// `render_topdown` uses. [`break_centre`] and
    /// [`assert_underlay_placement`] check their reports against it, so
    /// the test's copy of the transform cannot drift from the tools'.
    const fn plan_units() -> Units {
        Units {
            scale_num: SCALE_NUM,
            scale_den: 1_i64,
            origin_x: 0_i64,
            origin_y: 0_i64,
            grid: GRID,
            flip_y: true,
        }
    }

    /// One `[x, y]` JSON pair as a map point.
    fn point_of(row: &Value) -> Pt {
        let list = row.as_array().expect("an [x, y] pair");
        (
            list.first().and_then(Value::as_i64).expect("an x"),
            list.get(1).and_then(Value::as_i64).expect("a y"),
        )
    }

    /// The `A|B` break's centre in map units, read off the trace report's
    /// drawing-pixel `center` through [`plan_units`]: unsnapped, the way
    /// the underlay is sampled, and snapped to the shared grid, the way
    /// the plan's door is placed. The snapped pair is checked against the
    /// report's own `map_center`, never assumed.
    fn break_centre(gap: &Value) -> (Pt, Pt) {
        let drawn = point_of(&gap["center"]);
        let units = plan_units();
        let unsnapped = units.to_map(drawn.0, drawn.1).expect("the centre maps");
        let snapped = units
            .point(GRID, drawn.0, drawn.1)
            .expect("the centre snaps");
        assert_eq!(
            snapped,
            point_of(&gap["map_center"]),
            "the report's map_center is the shared units over the shared grid"
        );
        (unsnapped, snapped)
    }

    /// The `render_topdown` request the verification guide draws a
    /// sketch-built map with: the sketch itself as `underlay` through the
    /// very `units` the plan was built on (no `grid`, an underlay never
    /// snaps), the `automap` line colours and the sector indices.
    fn topdown_over_sketch(sketch: &str) -> Value {
        json!({
            "map": MAP,
            "width": OVERVIEW_WIDTH,
            "height": OVERVIEW_HEIGHT,
            "style": "automap",
            "labels": { "sectors": true },
            "underlay": { "source": sketch, "units": units() },
        })
    }

    /// The published asset's `URL` text.
    fn asset_url(report: &Value) -> &str {
        report["asset"]["url"].as_str().expect("an asset url")
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

    /// The one pixel `image` draws at world-pixel `x`/`y`, `None` off the
    /// picture.
    fn pixel_at(image: &RgbaImage, x: i64, y: i64) -> Option<Rgba> {
        image.get(u32::try_from(x).ok()?, u32::try_from(y).ok()?)
    }

    /// Whether `image`'s row `at.1` carries one of the automap line
    /// colours within [`PIXEL_SLACK`] columns of `at.0`.
    fn a_line_stands(image: &RgbaImage, at: Pt) -> bool {
        let low = at.0.saturating_sub(PIXEL_SLACK);
        let high = at.0.saturating_add(PIXEL_SLACK);
        (low..=high).any(|column| {
            pixel_at(image, column, at.1).is_some_and(|pixel| LINE_CLASSES.contains(&pixel))
        })
    }

    /// The two output pixels the built door's lines stand on: half the
    /// wall thickness either side of the break's snapped centre, the pair
    /// of lines the plan's `door` connector is wired on.
    fn door_track_pixels(fit: &Fit, centre: Pt) -> [Pt; 2] {
        let half = WALL_THICKNESS.checked_div(2).expect("a wall above 1");
        [
            fit.pixel(centre.0.saturating_sub(half), centre.1),
            fit.pixel(centre.0.saturating_add(half), centre.1),
        ]
    }

    /// The line-coloured pixels of the row `at.1` across `reach` columns
    /// either side of `at.0`: the built lines standing on the drawn wall
    /// band.
    fn band_line_pixels(image: &RgbaImage, at: Pt, reach: i64) -> usize {
        let low = at.0.saturating_sub(reach);
        let high = at.0.saturating_add(reach);
        (low..=high)
            .filter(|column| {
                pixel_at(image, *column, at.1).is_some_and(|pixel| LINE_CLASSES.contains(&pixel))
            })
            .count()
    }

    /// The build report's `door` feature row: the lines and band sectors
    /// the plan's locked door was wired onto.
    fn door_feature(built: &Value) -> &Value {
        built["features"]
            .as_array()
            .expect("a feature list")
            .iter()
            .find(|row| row["kind"] == json!("door"))
            .expect("the plan's blue gate is a door feature")
    }

    /// The linedef indices the built door stands on.
    fn door_lines(built: &Value) -> &[Value] {
        door_feature(built)["lines"]
            .as_array()
            .expect("a line list")
            .as_slice()
    }

    /// The overview's `underlay` row is the sketch where the shared
    /// `units` put it: the picture's own size and the map rectangle its
    /// four corners land in.
    fn assert_underlay_placement(overview: &Value, trace: &Value) {
        let row = &overview["underlay"];
        assert_eq!(row["width"], trace["image"]["width"], "the sketch's width");
        assert_eq!(
            row["height"], trace["image"]["height"],
            "the sketch's height"
        );
        let wide = row["width"].as_i64().expect("a width");
        let high = row["height"].as_i64().expect("a height");
        let units = plan_units();
        let (west, north) = units.to_map(0_i64, 0_i64).expect("the origin maps");
        let (east, south) = units.to_map(wide, high).expect("the far corner maps");
        let bounds = &row["map_bounds"];
        assert_eq!(bounds["min_x"], json!(west), "the underlay's west edge");
        assert_eq!(bounds["max_x"], json!(east), "and its east edge");
        assert_eq!(bounds["min_y"], json!(south), "its south edge");
        assert_eq!(bounds["max_y"], json!(north), "and its north edge");
    }

    /// The built door lands on the drawn wall: the break's own measured
    /// centre falls between the two pixels the door's lines stand on,
    /// half a wall thickness either side of it, and each of those pixels
    /// carries an automap line colour — as many line pixels as the door
    /// has linedefs across the band. The exact centre pixel is the band's
    /// own sector label, which is why the reading is the pair of tracks.
    fn assert_door_on_the_wall(image: &RgbaImage, overview: &Value, trace: &Value, built: &Value) {
        let rooms = rooms_with_source_ids(trace);
        let gap = gap_between(trace, &rooms, "start", "hall");
        let (measured, snapped) = break_centre(gap);
        let limit = GRID.checked_div(2).expect("a grid above 1");
        let drift = measured.0.saturating_sub(snapped.0).abs();
        assert!(
            drift <= limit,
            "the grid snap moves the centre by at most half a cell: {measured:?} / {snapped:?}"
        );
        let fit = Fit::of(overview);
        let tracks = door_track_pixels(&fit, snapped);
        let centre = fit.pixel(measured.0, measured.1);
        assert!(
            centre >= tracks[0] && centre <= tracks[1],
            "the drawn break's centre {centre:?} lies between the door's lines {tracks:?}"
        );
        for track in tracks {
            assert!(
                a_line_stands(image, track),
                "the door line at {track:?} draws in an automap line colour, not {:?}",
                pixel_at(image, track.0, track.1)
            );
        }
        let drawn = band_line_pixels(
            image,
            centre,
            fit.span(WALL_THICKNESS).saturating_add(PIXEL_SLACK),
        );
        let lines = door_lines(built).len();
        assert!(
            drawn >= lines,
            "the door's {lines} lines land on the drawn wall, {drawn} line pixels were counted"
        );
    }

    #[tokio::test]
    async fn the_topdown_overview_lands_the_built_door_on_the_drawn_wall() {
        let dir = tempfile::tempdir().expect("temp dir");
        let chain = build_the_traced_plan(dir.path()).await;
        let overview = call(
            &chain.client,
            "render_topdown",
            topdown_over_sketch(&chain.sketch),
        )
        .await;
        assert_eq!(overview["map"], json!(MAP), "the built map drew");
        assert_eq!(overview["projection"], json!("topdown"), "the flat fit");
        assert_eq!(
            overview["notes"],
            json!([]),
            "the overview skipped nothing: {}",
            overview["notes"]
        );
        assert_underlay_placement(&overview, &chain.trace);
        let image = asset_png(asset_url(&overview)).await;
        assert_eq!(
            i64::from(image.width),
            overview["width"].as_i64().expect("a width"),
            "the asked width stands"
        );
        assert_eq!(
            i64::from(image.height),
            overview["height"].as_i64().expect("a height"),
            "and the asked height"
        );
        assert_door_on_the_wall(&image, &overview, &chain.trace, &chain.built);
    }

    /// The `render_view` request the camera at the plan's own start stands
    /// on: the thing's reported `x`, `y` and `angle`, which faces down the
    /// sketch's west-to-east axis at the blue door.
    fn view_from(start: &Value) -> Value {
        json!({
            "map": MAP,
            "x": start["x"],
            "y": start["y"],
            "angle": start["angle"],
        })
    }

    /// The camera the frame answers with is the start thing itself,
    /// standing in the plan's `start` sector with an eye above its floor.
    fn assert_camera_is_the_start(view: &Value, start: &Value, built: &Value) {
        let camera = &view["camera"];
        for field in ["x", "y", "angle"] {
            assert_eq!(camera[field], start[field], "the camera echoes the {field}");
        }
        assert_eq!(
            camera["sector"], built["rooms"]["start"][0],
            "the camera stands in the start room: {camera}"
        );
        assert!(
            camera["z"].as_i64().expect("an eye height") > 0_i64,
            "the eye is above the floor it stands on"
        );
    }

    /// The pixel the session's own render settings fill the ground with.
    async fn ground_pixel(client: &Client) -> Rgba {
        let settings = call(client, "get_render_settings", Value::Null).await;
        let text = settings["background"]
            .as_str()
            .expect("a background colour");
        parse_hex_color(text).expect("the settings' own colour parses")
    }

    /// The pixels of the frame's centre column that are not the ground:
    /// straight ahead of the camera stands the blue door.
    fn painted_centre_column(image: &RgbaImage, ground: Rgba) -> usize {
        let column = image.width.checked_div(2).expect("a frame above 1 wide");
        (0..image.height)
            .filter(|row| image.get(column, *row) != Some(ground))
            .count()
    }

    #[tokio::test]
    async fn the_view_from_the_player_start_draws_the_door_ahead() {
        let dir = tempfile::tempdir().expect("temp dir");
        let chain = build_the_traced_plan(dir.path()).await;
        let start = player_start(&chain.client).await;
        let view = call(&chain.client, "render_view", view_from(&start)).await;
        assert_eq!(view["map"], json!(MAP), "the built map drew");
        assert_camera_is_the_start(&view, &start, &chain.built);
        let image = asset_png(asset_url(&view)).await;
        let ground = ground_pixel(&chain.client).await;
        let painted = painted_centre_column(&image, ground);
        let height = usize::try_from(image.height).expect("a test frame");
        assert_eq!(
            usize::try_from(view["height"].as_u64().expect("a height")).expect("a test frame"),
            height,
            "the frame is the size the report answers"
        );
        let half = height.checked_div(2).expect("a frame above 1 tall");
        assert!(painted > 0, "the centre column carries something drawn");
        assert!(
            painted >= half,
            "the middle of the frame is geometry, not ground: {painted} of {height} pixels"
        );
        let ahead = (
            image.width.checked_div(2).expect("a frame above 1 wide"),
            image.height.checked_div(2).expect("a frame above 1 tall"),
        );
        assert_ne!(
            image.get(ahead.0, ahead.1).expect("the frame's centre"),
            ground,
            "dead ahead, at the horizon, stands the blue door, not bare ground"
        );
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

    #[tokio::test]
    async fn the_saved_wad_reopens_as_the_same_playable_map() {
        let dir = tempfile::tempdir().expect("temp dir");
        let chain = build_the_traced_plan(dir.path()).await;
        let before = map_counts(&chain.client).await;
        let file = dir.path().join("sketch-chain.wad");
        let saved = call(&chain.client, "save_wad", path_arg(&file)).await;
        assert_eq!(saved["path"], path_arg(&file)["path"], "the echoed target");
        assert_eq!(
            saved["dirty"],
            json!(false),
            "the save leaves nothing unsaved"
        );
        assert!(
            saved["bytes"].as_u64().expect("a byte count") > 0,
            "a file was written"
        );
        call(&chain.client, "new_wad", json!({})).await;
        let opened = call(&chain.client, "open_wad", path_arg(&file)).await;
        assert!(
            opened["lumps"].as_u64().expect("a lump count") > 0,
            "the saved archive opens again"
        );
        assert_eq!(
            map_counts(&chain.client).await,
            before,
            "the reloaded map holds the same elements"
        );
        let plays = call(&chain.client, "check_playability", json!({ "map": MAP })).await;
        assert!(
            plays["ok"].as_bool().expect("a verdict"),
            "the reloaded map plays: {plays}"
        );
        assert_eq!(plays["blockers"], json!([]), "and nothing is held out");
    }

    /// The traced plan with its `Blue Keycard` row gone and `replace` set:
    /// the same map, with the blue door as a way nobody can open.
    fn plan_without_the_key(trace: &Value) -> Value {
        let mut request = plan_request(trace);
        let things = request["things"].as_array_mut().expect("the plan's things");
        things.retain(|row| row["name"] != json!(KEY_NAME));
        request["replace"] = json!(true);
        request
    }

    /// The rebuild stood two things, not three, and the plan's own
    /// judgment-gap warnings say what is missing.
    fn assert_the_key_is_missing(built: &Value) {
        assert_eq!(
            built["things_placed"],
            json!(2_u64),
            "the imp and the player start: {}",
            built["things_placed"]
        );
        let warnings = built["warnings"].as_array().expect("a warning list");
        assert!(
            warnings.iter().any(|row| {
                let text = row.as_str().expect("a warning line");
                text.contains("blue") && text.contains("key")
            }),
            "the build reports the card the door wants: {warnings:?}"
        );
    }

    /// The `locked` blocker row standing on one of the built door's own
    /// linedefs: the reason the walk gives, the colour of card it wants
    /// and the door band's own sector it holds out of reach.
    fn locked_door_blocker<'rep>(report: &'rep Value, built: &Value) -> &'rep Value {
        let lines = door_lines(built);
        let band = door_feature(built)["sectors"]
            .as_array()
            .expect("a sector list");
        let row = report["blockers"]
            .as_array()
            .expect("a blocker list")
            .iter()
            .find(|row| {
                row["reason"] == json!("locked")
                    && row["color"] == json!("blue")
                    && lines.contains(&row["line"])
            })
            .unwrap_or_else(|| {
                panic!(
                    "a locked blue door holds a sector out: {}",
                    report["blockers"]
                )
            });
        assert_eq!(
            row["reason"],
            json!("locked"),
            "the walk names the rule that kept the sector out: {row}"
        );
        assert_eq!(
            row["color"],
            json!("blue"),
            "and the card the door wants: {row}"
        );
        assert!(
            band.contains(&row["sector"]),
            "the door band itself is the held-out sector: {row} against {band:?}"
        );
        row
    }

    /// The locked row explains itself with a real instruction: it names
    /// the colour, the key and the tool that places one, and the report's
    /// `next_steps` carries that same line.
    fn assert_the_hint_names_the_key(report: &Value, blocker: &Value) {
        let hint = blocker["hint"].as_str().expect("a hint");
        assert!(!hint.is_empty(), "the blocker answers how to fix it");
        assert!(
            hint.contains("blue") && hint.contains("key"),
            "the hint names the card the door wants: {hint}"
        );
        assert!(
            hint.contains("add_thing"),
            "and the tool that places it: {hint}"
        );
        let steps = report["next_steps"].as_array().expect("a next-steps list");
        assert!(
            steps.contains(&json!(hint)),
            "the fix leads the next steps: {steps:?}"
        );
    }

    /// The walk's reading of the same map: `ok` is false, every blue lock
    /// stayed shut, the hall behind the door is on the unreachable list
    /// and no level end can be worked.
    fn assert_the_player_is_held_out(report: &Value, built: &Value) {
        assert!(
            !report["ok"].as_bool().expect("a verdict"),
            "a locked door with no key does not play: {}",
            report["summary"]
        );
        assert!(
            report["keys"].as_array().expect("a key list").is_empty(),
            "the walk found no card to pick up"
        );
        for lock in report["locks"].as_array().expect("a lock list") {
            assert_eq!(
                lock["color"],
                json!("blue"),
                "the one lock is the blue gate"
            );
            assert!(
                !lock["opened"].as_bool().expect("a flag"),
                "and it stayed shut: {lock}"
            );
        }
        assert_held_out_of_the_hall(report, built);
    }

    /// The sectors behind the blue door — the plan's `hall` and `exit`
    /// rooms — stayed out of reach, and so did the level end.
    fn assert_held_out_of_the_hall(report: &Value, built: &Value) {
        let unreachable = report["sectors"]["unreachable"]
            .as_array()
            .expect("an unreachable list");
        let mut held_out: Vec<&Value> = Vec::new();
        for room in ["hall", "exit"] {
            held_out.extend(built["rooms"][room].as_array().expect("a sector list"));
        }
        assert!(
            held_out.len() >= 2,
            "the hall and the exit are both behind the door: {held_out:?}"
        );
        for sector in &held_out {
            assert!(
                unreachable.contains(sector),
                "the sector {sector} stayed behind the locked door: {unreachable:?}"
            );
        }
        assert!(
            report["exits"]
                .as_array()
                .expect("an exit list")
                .iter()
                .all(|exit| !exit["reachable"].as_bool().expect("a flag")),
            "no level end could be worked: {}",
            report["exits"]
        );
    }

    #[tokio::test]
    async fn a_blue_door_without_its_key_holds_the_player_out() {
        let dir = tempfile::tempdir().expect("temp dir");
        let chain = build_the_traced_plan(dir.path()).await;
        let rebuilt = call(
            &chain.client,
            "build_map_from_plan",
            plan_without_the_key(&chain.trace),
        )
        .await;
        assert_the_key_is_missing(&rebuilt);
        let report = call(&chain.client, "check_playability", json!({ "map": MAP })).await;
        let blocker = locked_door_blocker(&report, &rebuilt);
        assert_the_hint_names_the_key(&report, blocker);
        assert_the_player_is_held_out(&report, &rebuilt);
    }

    // The chain proof, at library level: apply_ops over the ruled photo
    // fixture, with no client and no asset round trip.

    /// The two ops of the photo pass's cleaning chain, on the parameters the
    /// [`ruled_photo_sheet`] fixture is tuned against. The background op
    /// flattens the sheet's shadow and vignette and inks everything darker
    /// than its own local paper, so the pale ruling and the near-black walls
    /// alike come out black; the opening then drops every stroke thinner than
    /// its `3x3` element, keeping the 3-px walls and erasing the 1-px ruling.
    fn photo_chain() -> [ImageOp; 2] {
        [
            ImageOp::AdaptiveThreshold {
                radius: 6_u32,
                offset: 10_u8,
                invert: false,
            },
            ImageOp::ThinStrokeFilter {
                radius: 1_u32,
                level: None,
                invert: false,
            },
        ]
    }

    /// The middle row of the room's top wall bar and the columns of it the
    /// samples read: `PHOTO_ROOM`'s box carries [`PHOTO_WALL_PX`] 3-px bars,
    /// so its top bar is rows 90, 91 and 92, centred on 91 — the middle line
    /// a `3x3` opening erodes to and dilates back.
    const TOP_WALL: (u32, u32, u32) = (91_u32, 130_u32, 410_u32);
    /// The middle row of the room's bottom wall bar (rows 297, 298 and 299)
    /// and the same sampled columns.
    const BOTTOM_WALL: (u32, u32, u32) = (298_u32, 130_u32, 410_u32);
    /// The middle column of the room's left wall bar (columns 120, 121 and
    /// 122) and the rows of it the samples read.
    const LEFT_WALL: (u32, u32, u32) = (121_u32, 100_u32, 290_u32);
    /// The middle column of the room's right wall bar (columns 417, 418 and
    /// 419) and the same sampled rows.
    const RIGHT_WALL: (u32, u32, u32) = (418_u32, 100_u32, 290_u32);
    /// The paper spans the ruling is sampled in: the sheet but for the 5 px
    /// either side of the room's wall box, where the bars themselves stand.
    const RULING_SPANS: [(u32, u32); 2] = [(8_u32, 115_u32), (425_u32, 592_u32)];
    /// The eight rows ending on the top wall bar's centre row, and the eight
    /// ending on the bottom bar's: six rows of paper over two rows of wall in
    /// each, the first band deep in the fixture's shadow and the second in
    /// full light.
    const LIGHT_BAND_TOPS: (u32, u32) = (84_u32, 291_u32);
    /// The classes those eight rows must show where a wall bar stands under
    /// them: six of paper, then the bar's own row above its centre and its
    /// centre row. The shadowed band shows them no less than the lit one.
    const WALL_BAND: [bool; 8] = [false, false, false, false, false, false, true, true];
    /// The classes of the same eight rows over bare paper, either side of the
    /// room: paper all the way down, in the shadow no less than in the light.
    const PAPER_BAND: [bool; 8] = [false; 8];
    /// The columns the light bands are read down with the room's bars under
    /// them: the room's own inside, clear of its side bars, which run from the
    /// top wall to the bottom one so their two bands are not mirrors.
    const WALL_BAND_COLUMNS: (u32, u32) = (124_u32, 416_u32);
    /// The columns the same bands are read down over bare paper, either side
    /// of the room's box.
    const PAPER_BAND_COLUMNS: [(u32, u32); 2] = [(100_u32, 120_u32), (420_u32, 440_u32)];
    /// The stride between the light-band samples: dense enough to cross both
    /// walls and to catch a band of invented ink a few pixels wide.
    const BAND_STRIDE: u32 = 3_u32;
    /// The vignette's outer block at each top corner: `(0, 0, 84, 44)` and its
    /// mirror at the right, the darkest paper on the sheet and the only place
    /// where its hard luminance steps are stacked.
    const VIGNETTE: (u32, u32) = (84_u32, 44_u32);
    /// How far inside the sheet's frame no ink may stand: the background
    /// window's reach at the clipped border, where the shadow ramp's darkest
    /// rows would otherwise print a band the opening keeps.
    const FRAME_MARGIN: u32 = 4_u32;
    /// The opaque white both ops paint for paper.
    const CLEAN_PAPER: [u8; 4] = [255_u8, 255, 255, 255];

    /// `adaptive_threshold` then `thin_stroke_filter` over the ruled photo:
    /// the shadowed room's walls stand in clean black, its ruling is gone, and
    /// the shadowed band over the top wall classifies exactly like the lit band
    /// over the bottom one — the uneven-light regression, with no client and no
    /// asset round trip in the way.
    #[test]
    fn apply_ops_cleans_the_ruled_photo() {
        let sheet = ruled_photo_sheet(SHEET_WIDTH, SHEET_HEIGHT);
        let (clean, report) = apply_ops(sheet, &photo_chain()).expect("the photo chain runs");
        assert_eq!(
            (clean.width, clean.height),
            (SHEET_WIDTH, SHEET_HEIGHT),
            "a binarize-then-open chain never changes the shape"
        );
        assert_cleaning_report(&report);
        assert_the_walls_stand(&clean);
        assert_nothing_else_stands(&clean, &report);
        assert_the_ruling_is_gone(&clean);
        assert_the_light_is_even(&clean);
        assert_the_vignette_is_paper(&clean);
        assert_the_frame_is_paper(&clean);
    }

    /// One `u64` measurement of a report step, read by field name.
    fn measurement(step: &Value, field: &str) -> u64 {
        step[field].as_u64().expect("a count measurement")
    }

    /// The chain's two report steps: the binarizer's ink with no global level
    /// of its own, then the opening on the Otsu level of that black-and-white
    /// sheet, which drops many times more ink than it keeps and splits the
    /// first step's ink exactly between what it dropped and what it kept.
    fn assert_cleaning_report(report: &OpsReport) {
        let steps = &report.steps;
        assert_eq!(steps.len(), 2_usize, "one entry per op");
        assert_eq!(steps[0]["op"], json!("adaptive_threshold"), "the first op");
        assert_eq!(
            steps[0]["otsu"],
            json!(false),
            "the binarizer picks no global level"
        );
        assert_eq!(steps[1]["op"], json!("thin_stroke_filter"), "the second op");
        assert_eq!(
            steps[1]["otsu"],
            json!(true),
            "the opening thresholds on Otsu's level"
        );
        let ink = measurement(&steps[0], "ink_pixels");
        let kept = measurement(&steps[1], "kept_ink_pixels");
        let dropped = measurement(&steps[1], "removed_ink_pixels");
        assert!(ink > 0_u64, "the binarizer inks the sheet: {ink} pixels");
        assert_eq!(
            dropped.saturating_add(kept),
            ink,
            "the opening only splits the ink the binarizer painted"
        );
        assert!(
            dropped > kept.saturating_mul(5_u64),
            "the ~29k-px ruling outweighs the ~3k-px walls five times over: \
             {dropped} dropped against {kept} kept"
        );
        let darkest = measurement(&steps[0], "background_min");
        let lightest = measurement(&steps[0], "background_max");
        assert!(
            lightest.saturating_sub(darkest) > 100_u64,
            "the background the op flattened really is uneven: {darkest} to {lightest}"
        );
    }

    /// The ink of a cleaned sheet, counted over the whole picture: every pixel
    /// the two ops left as anything but the exact white of [`CLEAN_PAPER`].
    fn ink_count(image: &RgbaImage) -> usize {
        image
            .pixels
            .chunks_exact(CLEAN_PAPER.len())
            .filter(|pixel| *pixel != CLEAN_PAPER)
            .count()
    }

    /// The room's own ink area, in pixels: its four [`PHOTO_WALL_PX`] bars
    /// along the edges of `PHOTO_ROOM`, the four `bar x bar` corner blocks
    /// counted once, plus the interior tick.
    fn wall_area() -> u64 {
        let (x0, y0, x1, y1) = PHOTO_ROOM;
        let (tx0, ty0, tx1, ty1) = PHOTO_TICK;
        let bar = u64::from(PHOTO_WALL_PX);
        let long = u64::from(x1.saturating_sub(x0)).saturating_mul(bar);
        let short = u64::from(y1.saturating_sub(y0)).saturating_mul(bar);
        let corners = bar.saturating_mul(bar).saturating_mul(4_u64);
        let tick =
            u64::from(tx1.saturating_sub(tx0)).saturating_mul(u64::from(ty1.saturating_sub(ty0)));
        long.saturating_add(long)
            .saturating_add(short)
            .saturating_add(short)
            .saturating_sub(corners)
            .saturating_add(tick)
    }

    /// The cleaned sheet carries the opening's kept ink and nothing else: the
    /// picture's own ink count, the report's `kept_ink_pixels` and the room's
    /// area agree to the pixel, so no band or blob stands anywhere outside it.
    fn assert_nothing_else_stands(image: &RgbaImage, report: &OpsReport) {
        let kept = measurement(&report.steps[1], "kept_ink_pixels");
        let ink = u64::try_from(ink_count(image)).expect("a test count");
        assert_eq!(
            ink, kept,
            "the opening kept exactly the ink the sheet carries"
        );
        assert_eq!(
            kept,
            wall_area(),
            "the kept ink is the room's four bars and its tick, and nothing more"
        );
    }

    /// Whether the cleaned sheet draws the pixel at `(column, row)` as ink.
    fn draws_ink(image: &RgbaImage, column: u32, row: u32) -> bool {
        image.get(column, row) != Some(CLEAN_PAPER)
    }

    /// Every pixel of the cleaned sheet's row `row` between columns `low` and
    /// `high` is the exact black the opening paints its kept ink in.
    fn assert_row_is_ink(image: &RgbaImage, row: u32, low: u32, high: u32) {
        for column in low..high {
            assert_eq!(
                image.get(column, row),
                Some(INK),
                "the wall at ({column}, {row}) stands"
            );
        }
    }

    /// Every pixel of the cleaned sheet's column `column` between rows `low`
    /// and `high` is the exact black of the opening's kept ink.
    fn assert_column_is_ink(image: &RgbaImage, column: u32, low: u32, high: u32) {
        for row in low..high {
            assert_eq!(
                image.get(column, row),
                Some(INK),
                "the wall at ({column}, {row}) stands"
            );
        }
    }

    /// Every sampled wall-centreline pixel is kept ink: a bar as thick as the
    /// opening's element erodes to its middle line and dilates back to its
    /// full width, so the middle row of each horizontal bar and the middle
    /// column of each vertical one stand.
    fn assert_the_walls_stand(image: &RgbaImage) {
        assert_row_is_ink(image, TOP_WALL.0, TOP_WALL.1, TOP_WALL.2);
        assert_row_is_ink(image, BOTTOM_WALL.0, BOTTOM_WALL.1, BOTTOM_WALL.2);
        assert_column_is_ink(image, LEFT_WALL.0, LEFT_WALL.1, LEFT_WALL.2);
        assert_column_is_ink(image, RIGHT_WALL.0, RIGHT_WALL.1, RIGHT_WALL.2);
    }

    /// Every row of the sheet carrying the fixture's ruling, read off its
    /// phase and period.
    fn ruled_rows(height: u32) -> Vec<u32> {
        let phase = usize::try_from(PHOTO_RULING_PHASE).expect("a test phase");
        let period = usize::try_from(PHOTO_RULING_PERIOD).expect("a test period");
        let rows = usize::try_from(height).expect("a test height");
        (phase..rows)
            .step_by(period)
            .filter_map(|row| u32::try_from(row).ok())
            .collect()
    }

    /// One sampled column of the cleaned sheet is paper at every ruled row: a
    /// 1-px line holds no `3x3` square, so the opening drops it however hard
    /// the binarizer inked it.
    fn assert_no_ruling_in_column(image: &RgbaImage, column: u32, rows: &[u32]) {
        for row in rows {
            assert_eq!(
                image.get(column, *row),
                Some(CLEAN_PAPER),
                "the ruling at ({column}, {row}) is gone"
            );
        }
    }

    /// One sampled span of the cleaned sheet: every column of it carries no
    /// ruling.
    fn assert_no_ruling_in_span(image: &RgbaImage, low: u32, high: u32, rows: &[u32]) {
        for column in low..high {
            assert_no_ruling_in_column(image, column, rows);
        }
    }

    /// Every sampled ruling pixel of the sheet, over its whole height and in
    /// both paper margins, comes back the exact white of paper.
    fn assert_the_ruling_is_gone(image: &RgbaImage) {
        let rows = ruled_rows(image.height);
        assert!(!rows.is_empty(), "the sheet carries ruled rows");
        for (low, high) in RULING_SPANS {
            assert_no_ruling_in_span(image, low, high, &rows);
        }
    }

    /// The strided columns of a sampled span.
    fn sampled_columns(low: u32, high: u32, stride: u32) -> Vec<u32> {
        let step = usize::try_from(stride).expect("a test stride");
        let first = usize::try_from(low).expect("a test column");
        let last = usize::try_from(high).expect("a test column");
        (first..last)
            .step_by(step)
            .filter_map(|column| u32::try_from(column).ok())
            .collect()
    }

    /// One sampled column's two bands against the classes they must show: the
    /// eight rows over the shadowed top wall and the eight over the fully lit
    /// bottom one classify alike, pixel for pixel. The sheet's own paper
    /// darkens by 60 luminance levels or more between the two bands, so a
    /// single global level classes one of them wrong; only a local background
    /// gives both the same rows of paper and of wall.
    fn assert_band_matches(image: &RgbaImage, column: u32, band: [bool; 8]) {
        let (shadowed, lit) = LIGHT_BAND_TOPS;
        for (offset, ink) in band.iter().copied().enumerate() {
            let step = u32::try_from(offset).expect("a test offset");
            let in_shadow = shadowed.saturating_add(step);
            let in_light = lit.saturating_add(step);
            assert_eq!(
                draws_ink(image, column, in_shadow),
                ink,
                "the shadowed row {in_shadow} of column {column}"
            );
            assert_eq!(
                draws_ink(image, column, in_light),
                ink,
                "the lit row {in_light} of column {column}"
            );
        }
    }

    /// Every sampled column of one span shows the band's classes in both the
    /// shadowed band and the lit one.
    fn assert_bands_of_span_match(image: &RgbaImage, low: u32, high: u32, band: [bool; 8]) {
        for column in sampled_columns(low, high, BAND_STRIDE) {
            assert_band_matches(image, column, band);
        }
    }

    /// The uneven-light regression: the shadowed band over the room's top wall
    /// and the lit band over its bottom wall show the same classes down the
    /// room's inside, and both bands stay clean paper down the bare margins
    /// outside it.
    fn assert_the_light_is_even(image: &RgbaImage) {
        let (low, high) = WALL_BAND_COLUMNS;
        assert_bands_of_span_match(image, low, high, WALL_BAND);
        for (first, last) in PAPER_BAND_COLUMNS {
            assert_bands_of_span_match(image, first, last, PAPER_BAND);
        }
    }

    /// Every pixel of the cleaned sheet's row `row` between columns `low` and
    /// `high` is the exact white of paper.
    fn assert_row_is_paper(image: &RgbaImage, row: u32, low: u32, high: u32) {
        for column in low..high {
            assert!(
                !draws_ink(image, column, row),
                "the sheet at ({column}, {row}) is paper"
            );
        }
    }

    /// Every pixel of the cleaned sheet's column `column` between rows `low`
    /// and `high` is the exact white of paper.
    fn assert_column_is_paper(image: &RgbaImage, column: u32, low: u32, high: u32) {
        for row in low..high {
            assert!(
                !draws_ink(image, column, row),
                "the sheet at ({column}, {row}) is paper"
            );
        }
    }

    /// Both corner vignettes, out to their outer block, come back uniformly
    /// paper: their steps stay under the op's `offset` at every depth, so the
    /// darkest paper on the sheet is still background, and the ruling crossing
    /// them is still only a hairline for the opening to drop.
    fn assert_the_vignette_is_paper(image: &RgbaImage) {
        let (wide, high) = VIGNETTE;
        let right = image.width.saturating_sub(wide);
        for row in 0_u32..high {
            assert_row_is_paper(image, row, 0_u32, wide);
            assert_row_is_paper(image, row, right, image.width);
        }
    }

    /// The two frame rows `band` in from the sheet's top and bottom edges,
    /// across its whole width, are paper.
    fn assert_frame_rows_are_paper(image: &RgbaImage, band: u32) {
        let bottom = image.height.saturating_sub(band.saturating_add(1_u32));
        assert_row_is_paper(image, band, 0_u32, image.width);
        assert_row_is_paper(image, bottom, 0_u32, image.width);
    }

    /// The two frame columns `band` in from the sheet's left and right edges,
    /// down its whole height, are paper.
    fn assert_frame_columns_are_paper(image: &RgbaImage, band: u32) {
        let right = image.width.saturating_sub(band.saturating_add(1_u32));
        assert_column_is_paper(image, band, 0_u32, image.height);
        assert_column_is_paper(image, right, 0_u32, image.height);
    }

    /// No ink within [`FRAME_MARGIN`] pixels of any frame edge: neither the
    /// clipped window at the shadow ramp's dark end nor a step at the sheet's
    /// border is allowed to grow a band the opening would then keep.
    fn assert_the_frame_is_paper(image: &RgbaImage) {
        for band in 0_u32..FRAME_MARGIN {
            assert_frame_rows_are_paper(image, band);
            assert_frame_columns_are_paper(image, band);
        }
    }

    // The wire proof: the same cleaning chain run through `process_image`,
    // read off the tool's JSON report and off the published PNG fetched
    // over HTTP, including the encode/decode round trip.

    /// The `process_image` request over `source`: the photo cleaning chain
    /// as wire parameters, the same `radius` and `offset` [`photo_chain`]
    /// runs at library level. `quantize: false` keeps the proof free of a
    /// session — the quantized preview is the one thing that wants a
    /// `PLAYPAL` — and the chain's report and its full `RGBA`
    /// `processed` picture are what this test reads.
    fn photo_chain_request(source: &str) -> Value {
        json!({
            "source": source,
            "ops": [
                { "op": "adaptive_threshold", "radius": 6_u64, "offset": 10_u64 },
                { "op": "thin_stroke_filter", "radius": 1_u64 },
            ],
            "quantize": false,
        })
    }

    /// The first wire step: the local binarizer echoes its `radius` and
    /// `offset`, picks no global level, inks the sheet, and its flattened
    /// background's extremes prove the local-means window really ran over
    /// the fixture's gradient. Returns the step's ink count.
    fn assert_photo_chain_flatten_step(step: &Value) -> u64 {
        assert_eq!(
            step["op"],
            json!("adaptive_threshold"),
            "the first op echoes"
        );
        assert_eq!(step["radius"], json!(6_u64), "the asked radius echoes");
        assert_eq!(step["offset"], json!(10_u64), "and the asked offset");
        assert_eq!(
            step["otsu"],
            json!(false),
            "the binarizer picks no global level"
        );
        let ink = step["ink_pixels"].as_u64().expect("an ink count");
        assert!(ink > 0_u64, "the binarizer inks the sheet: {ink} pixels");
        let darkest = step["background_min"].as_u64().expect("a floor");
        let lightest = step["background_max"].as_u64().expect("a ceiling");
        assert!(
            lightest > darkest,
            "the flattened background really is uneven: {darkest} to {lightest}",
        );
        ink
    }

    /// The second wire step: the opening echoes its `radius`, thresholds on
    /// `Otsu`'s reported level, drops many times more ink than it keeps, and
    /// splits the binarizer's ink exactly between what it dropped and what
    /// it kept.
    fn assert_photo_chain_opening_step(step: &Value, ink: u64) {
        assert_eq!(step["op"], json!("thin_stroke_filter"), "the second op");
        assert_eq!(step["radius"], json!(1_u64), "the asked radius echoes");
        assert_eq!(
            step["otsu"],
            json!(true),
            "the opening thresholds on Otsu's level"
        );
        assert!(step["level"].is_number(), "the used level reports: {step}");
        let kept = step["kept_ink_pixels"].as_u64().expect("a kept count");
        let removed = step["removed_ink_pixels"]
            .as_u64()
            .expect("a removed count");
        assert!(
            removed > kept.saturating_mul(5_u64),
            "the ruling outweighs the walls five times over: \
             {removed} dropped against {kept} kept",
        );
        assert_eq!(
            removed.saturating_add(kept),
            ink,
            "the opening only splits the ink the binarizer painted",
        );
    }

    /// The wire report's two steps, one per op, in order.
    fn assert_photo_chain_steps(report: &Value) {
        let steps = report["steps"].as_array().expect("a step list");
        assert_eq!(steps.len(), 2_usize, "one entry per op");
        let ink = assert_photo_chain_flatten_step(&steps[0]);
        assert_photo_chain_opening_step(&steps[1], ink);
    }

    /// The uneven-light regression through the whole wire path: over the
    /// ruled photo written to a file, `process_image` reports both cleaning
    /// steps, publishes a full-size `PNG` that `HTTP` serves back intact,
    /// and the fetched picture keeps every wall centrelined pixel in clean
    /// black, returns every sampled ruling pixel as clean paper, classes
    /// the shadowed band over the top wall exactly like the lit band over
    /// the bottom one, and stays free of ink near the frame — including
    /// through the `PNG` encode/decode round trip.
    #[tokio::test]
    async fn photo_chain_reports_both_ops_and_publishes_a_clean_png() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let sheet = ruled_photo_sheet(SHEET_WIDTH, SHEET_HEIGHT);
        let file = write_png(dir.path(), "PHOTO.png", &sheet);
        let report = call(&client, "process_image", photo_chain_request(&file)).await;
        assert_eq!(report["width"], json!(600_u64), "the sheet's width stands");
        assert_eq!(report["height"], json!(400_u64), "and its height");
        assert_photo_chain_steps(&report);
        let url = report["processed"]["url"]
            .as_str()
            .expect("a processed url");
        let clean = asset_png(url).await;
        assert_eq!(
            clean.width, SHEET_WIDTH,
            "the published PNG decodes at the asked width"
        );
        assert_eq!(
            clean.height, SHEET_HEIGHT,
            "and at the asked height, unchanged"
        );
        assert_the_walls_stand(&clean);
        assert_the_ruling_is_gone(&clean);
        assert_the_light_is_even(&clean);
        assert_the_frame_is_paper(&clean);
    }
}
