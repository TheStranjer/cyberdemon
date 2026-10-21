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
//! Rendering, saving and the locked-out variant are the next slice's.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::image::RgbaImage;
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
    /// How far a nearest-grid door width may drift from the scaled
    /// measurement.
    const WIDTH_SLACK: i64 = 8_i64;
    /// The doomednum a `Blue Keycard` name resolves to.
    const BLUE_KEYCARD: i64 = 5_i64;
    /// The doomednum an `imp` name resolves to.
    const IMP: i64 = 3001_i64;
    /// The map the chain builds beside the fixture's own `MAP01`.
    const MAP: &str = "MAP05";
    /// The west-to-east guide names of the three traced rooms.
    const ROOM_NAMES: [&str; 3] = ["start", "hall", "exit"];

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
        let client = session(dir.path()).await;
        let file = write_png(dir.path(), "floor-plan.png", &floor_plan());
        let report = call(&client, "trace_sketch", trace_request(&file)).await;
        let request = plan_request(&report);
        let built = call(&client, "build_map_from_plan", request).await;
        assert_build_report(&built);
        assert_things_stand_on_their_marks(&client, &report).await;
        assert_playable(&client, &built).await;
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
                { "name": "Blue Keycard", "x": key["center"][0], "y": key["center"][1] },
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
}
