//! The `plan` tool group as seen by a real MCP client:
//! `build_map_from_plan` compiles a two-room plan with a `door` and an
//! `opening` into one written block whose sector and geometry counts
//! match the reported bookkeeping (the door lands closed, tagged and
//! wired with its style's specials, fronts facing their rooms, and a
//! `features` row reports the wiring), the things and
//! `player_start` parameters standing rows into the map by name, room
//! fill and transformed point with presence flags round-tripping
//! through the written block, the active map adoption, `already_exists`
//! for a working-archive block without `replace` and its overwrite, the
//! one-step `undo` that takes the whole block back, plan-path-named
//! refusals (a flat rectangle, both or neither footprint, unknown kind
//! and style names, an ambiguous thing name, a plan without a player
//! start), the `units` scale-and-mirror and `grid` snap that
//! carry an image-space plan into map units before compiling (with
//! unusable `units.scale_den`/`grid` values refused), a `UDMF` block
//! with its declared namespace, a one-call `theme` override that
//! never moves the session's theme, and the `finalize` parameter: the
//! whole node run built into the written block inside the one undo
//! step (the classic run plus `REJECT` and `BLOCKMAP` for binary maps,
//! the extended `ZNODES` lump alone for `UDMF` ones) with the result's
//! `nodes` summary and errors-only `validation` pass, and the null
//! pair when `finalize` is not asked for. The `features` rows: a plan
//! carrying an exit, a switch, a lift and a teleporter builds exactly
//! like its feature-free twin, and a feature naming an unknown room or
//! an `opens` id no connection carries refuses naming `features[i]`.
//! The door wiring: a blue-key plan's `features` row matches the written
//! block — its lines carry special `26` and the band's tag with fronts
//! facing their rooms, validation stays error-free and one `undo` takes
//! it all back.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The structural checks a compiled plan must pass; thing checks
    /// run separately because only the thing tests place a start.
    const STRUCTURAL_CODES: [&str; 7] = [
        "crossing_lines",
        "overlapping_lines",
        "two_sided_flag_mismatch",
        "zero_length_line",
        "duplicate_line",
        "missing_texture",
        "missing_flat",
    ];

    /// Open the features `IWAD` (textured two-room `MAP01`, the door
    /// wardrobe and `FLAT10`) as the base layer.
    async fn features_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_features(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// The two-room plan: `a` west of `b`, sharing the line `x = 128`,
    /// joined by a door low on the wall and an opening high on it, with
    /// the player start the tool now demands standing in `a`.
    fn plan_for(map: &str) -> Value {
        json!({
            "map": map,
            "rooms": [
                {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "b", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
            ],
            "connections": [
                {"id": "d1", "from": "a", "to": "b", "kind": "door", "at": [128_i64, 32_i64]},
                {"from": "a", "to": "b", "kind": "opening", "at": [128_i64, 96_i64]}
            ],
            "player_start": {"room": "a"}
        })
    }

    /// The working sector row at `index`.
    async fn working_sector(client: &Client, map: &str, index: usize) -> Value {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["sectors"]}),
        )
        .await;
        report["sectors"]
            .as_array()
            .expect("sectors is an array")
            .get(index)
            .cloned()
            .expect("the sector row exists")
    }

    /// The length of a `"sectors"` array as a `u64`.
    fn sector_len(list: &Value) -> u64 {
        u64::try_from(list.as_array().expect("a sector list").len()).expect("small")
    }

    /// How many sectors the result assigns across rooms and connectors.
    fn reported_sector_count(result: &Value) -> u64 {
        let rooms = result["rooms"]
            .as_object()
            .expect("rooms is an object")
            .values()
            .map(sector_len);
        let bands = result["connections"]
            .as_array()
            .expect("connections is an array")
            .iter()
            .map(|row| &row["sectors"])
            .map(sector_len);
        rooms.chain(bands).sum()
    }

    /// Requires `build_map_from_plan` to reject `request` with
    /// `invalid_argument` naming `needle`.
    async fn refuse_with(client: &Client, request: Value, needle: &str) {
        let note = request.to_string();
        let (code, message) = call_err(client, "build_map_from_plan", request).await;
        assert_eq!(code, "invalid_argument", "refusal for {note}");
        assert!(
            message.contains(needle),
            "the refusal must mention {needle:?}: {message}"
        );
    }

    /// Requires the finalized build summary to report `variant` with
    /// positive seg, subsector and node counts.
    fn expect_real_tree(nodes: &Value, variant: &str) {
        assert_eq!(nodes["variant"], json!(variant), "the finalized layout");
        for field in ["segs", "subsectors", "nodes"] {
            assert!(
                nodes[field].as_u64().is_some_and(|count| count > 0),
                "{field} is positive: {nodes}",
            );
        }
    }

    /// Requires the read-back `get_nodes` report to agree with the
    /// finalized build summary, counts and all.
    fn expect_summary_matches_block(read: &Value, nodes: &Value) {
        for field in ["segs", "subsectors", "nodes"] {
            assert_eq!(
                read[field], nodes[field],
                "the summary counts match the written block",
            );
        }
    }

    /// Requires none of `absent` to name a lump of the listed block.
    fn expect_no_lumps(lumps: &[Value], absent: [&str; 4]) {
        for name in absent {
            assert!(
                !lumps.iter().any(|lump| lump == name),
                "{name} stays out of a finalized UDMF block: {lumps:?}",
            );
        }
    }

    #[tokio::test]
    async fn two_rooms_with_a_door_and_an_opening_report_their_bookkeeping() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "build_map_from_plan", plan_for("MAP05")).await;
        assert_eq!(result["map"], json!("MAP05"), "the block written");
        assert_eq!(result["format"], json!("doom"), "the default layout");
        assert_eq!(
            result["rooms"],
            json!({"a": [0_u64], "b": [1_u64]}),
            "rooms keep plan order"
        );
        assert_eq!(
            result["connections"],
            json!([
                {"id": "d1", "kind": "door", "sectors": [2_u64]},
                {"id": "a->b", "kind": "opening", "sectors": [3_u64]}
            ]),
            "the door keeps its id, the opening earns the derived label"
        );
        assert_eq!(
            result["bounds"],
            json!({"x1": 0_i64, "y1": 0_i64, "x2": 256_i64, "y2": 128_i64}),
            "the plan's outer corners"
        );
        assert_eq!(result["nodes"], json!(null), "null without finalize");
        assert_eq!(
            result["things_placed"],
            json!(1_u64),
            "just the player start in room a"
        );
        assert_eq!(result["validation"], json!(null), "null without finalize");
        assert_eq!(
            result["next_steps"],
            json!([
                "render_topdown to compare with the sketch",
                "render_view from the player start"
            ]),
            "what to look at next"
        );
    }

    #[tokio::test]
    async fn the_written_block_matches_the_report_and_stays_clean() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "build_map_from_plan", plan_for("MAP05")).await;
        let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
        assert_eq!(
            summary["name"],
            json!("MAP05"),
            "the built map became the active one"
        );
        assert_eq!(
            summary["counts"]["sectors"],
            json!(reported_sector_count(&result)),
            "two rooms and two bands own the whole map"
        );
        assert_eq!(
            summary["lines"]["one_sided"]
                .as_u64()
                .expect("a count")
                .saturating_add(summary["lines"]["two_sided"].as_u64().expect("a count")),
            summary["counts"]["linedefs"].as_u64().expect("a count"),
            "every linedef is exactly one-sided or two-sided"
        );
        assert_eq!(summary["bounds"]["min_x"], json!(0_i64), "the box");
        assert_eq!(summary["bounds"]["max_x"], json!(256_i64));
        assert_eq!(summary["bounds"]["min_y"], json!(0_i64));
        assert_eq!(summary["bounds"]["max_y"], json!(128_i64));
        let door = working_sector(&client, "MAP05", 2).await;
        assert_eq!(
            door["special"],
            json!(0_i32),
            "a Doom door acts from its lines: the sector keeps special 0"
        );
        assert_eq!(
            door["tag"],
            json!(1_i32),
            "the wired band answers the tag its lines carry"
        );
        assert_eq!(
            door["floor_height"], door["ceiling_height"],
            "the door sector is collapsed"
        );
        expect_door_feature_row(&result);
        let report = call(
            &client,
            "validate_map",
            json!({"map": "MAP05", "scope": "working", "codes": STRUCTURAL_CODES}),
        )
        .await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "the compiled geometry is clean: {:?}",
            report["issues"]
        );
    }

    /// The working linedef and sidedef rows of the assembled graph.
    async fn working_lines(client: &Client, map: &str) -> Vec<Value> {
        let graph = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["linedefs", "sidedefs"]}),
        )
        .await;
        graph["linedefs"]
            .as_array()
            .expect("linedefs is an array")
            .clone()
    }

    /// The `MAP05` plan's one door reports a `features` row: the door
    /// class, no room, band sector 2 and the fresh tag 1.
    fn expect_door_feature_row(result: &Value) {
        let rows = result["features"].as_array().expect("a feature list");
        assert_eq!(rows.len(), 1, "the door is the only feature: {result}");
        let row = rows.first().expect("the door row");
        assert_eq!(row["kind"], json!("door"), "the wired connector class");
        assert_eq!(row["room"], json!(null), "a door belongs to the wall");
        assert_eq!(row["sectors"], json!([2_u64]), "the band sector");
        assert_eq!(row["tag"], json!(1_u64), "the fresh shared tag");
    }

    /// Assert every row of `lines` whose index is in `indices` is a
    /// blue-key use line: special 26, tag 1, the band behind and a room
    /// in front.
    fn assert_locked_use_lines(lines: &[Value], indices: &[u64]) {
        let wired: Vec<&Value> = lines
            .iter()
            .filter(|line| indices.contains(&line["index"].as_u64().expect("an index")))
            .collect();
        assert_eq!(
            wired.len(),
            indices.len(),
            "every reported door line is in the block"
        );
        for line in wired {
            let index = line["index"].as_u64().expect("an index");
            assert_eq!(
                line["special"],
                json!(26_i64),
                "DR blue key on line {index}"
            );
            assert_eq!(line["tag"], json!(1_i64), "the shared tag on line {index}");
            assert_eq!(
                line["back"]["sector"],
                json!(2_i64),
                "the band sits behind line {index}"
            );
            let front = line["front"]["sector"].as_i64().expect("a front sector");
            assert!(
                front == 0_i64 || front == 1_i64,
                "the front faces its room on line {index}: {line}"
            );
        }
    }

    #[tokio::test]
    async fn a_blue_locked_door_lands_wired_and_reports_its_feature_row() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["connections"][0]["style"] = json!("locked_blue");
        let result = call(&client, "build_map_from_plan", request).await;
        expect_door_feature_row(&result);
        let indices: Vec<u64> = result["features"][0]["lines"]
            .as_array()
            .expect("two door lines")
            .iter()
            .map(|slot| slot.as_u64().expect("a line index"))
            .collect();
        let lines = working_lines(&client, "MAP05").await;
        assert_locked_use_lines(&lines, &indices);
        let jambs: Vec<&Value> = lines
            .iter()
            .filter(|line| line["back"] == json!(null) && line["front"]["sector"] == json!(2_i64))
            .collect();
        assert_eq!(
            jambs.len(),
            2,
            "the band's north reveal edges, split where the room's line ran; its south \
             edge stands on the map border"
        );
        assert!(
            jambs
                .iter()
                .all(|line| line["front"]["middle"] == json!("DOORBLU")),
            "the locked jambs wear the blue track: {jambs:?}"
        );
        let band = working_sector(&client, "MAP05", 2).await;
        assert_eq!(band["tag"], json!(1_i32), "the sector answers the tag");
        let report = call(
            &client,
            "validate_map",
            json!({"map": "MAP05", "scope": "working", "codes": STRUCTURAL_CODES}),
        )
        .await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "the wired map stays error-free: {:?}",
            report["issues"]
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["build_map_from_plan MAP05"]),
            "one labelled step takes the wiring too"
        );
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "the whole block went back");
    }

    #[tokio::test]
    async fn a_working_block_needs_replace_and_overwrite_keeps_one_block() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(&client, "create_map", json!({"name": "MAP01"})).await;
        let (code, message) = call_err(&client, "build_map_from_plan", plan_for("MAP01")).await;
        assert_eq!(code, "already_exists", "the working block stands");
        assert!(
            message.contains("replace"),
            "the refusal names the way out: {message}"
        );
        let untouched = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(
            untouched["counts"]["sectors"],
            json!(0_u64),
            "the refusal wrote nothing"
        );
        let mut request = plan_for("MAP01");
        request["replace"] = json!(true);
        call(&client, "build_map_from_plan", request).await;
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        let rows = maps["maps"].as_array().expect("a map list");
        assert_eq!(rows.len(), 1, "one working block after the replace");
        assert_eq!(rows.first().expect("the row")["name"], json!("MAP01"));
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(
            summary["counts"]["sectors"],
            json!(4_u64),
            "the planned map overwrote the empty one"
        );
    }

    #[tokio::test]
    async fn undo_takes_the_whole_block_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(&client, "build_map_from_plan", plan_for("MAP05")).await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["build_map_from_plan MAP05"]),
            "one labelled step"
        );
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "the whole block went back");
    }

    #[tokio::test]
    async fn plan_paths_name_every_refused_element() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut flat = plan_for("MAP05");
        flat["rooms"][0]["rect"]["w"] = json!(0_i64);
        refuse_with(&client, flat, "rooms[0]").await;
        let mut both = plan_for("MAP05");
        both["rooms"][1]["points"] = json!([[0_i64, 0_i64], [64_i64, 0_i64], [0_i64, 64_i64]]);
        refuse_with(&client, both, "rooms[1]").await;
        let mut neither = plan_for("MAP05");
        neither["rooms"][0].as_object_mut().expect("a room")["rect"] = json!(null);
        refuse_with(&client, neither, "rooms[0]").await;
        let mut portal = plan_for("MAP05");
        portal["connections"][0]["kind"] = json!("portal");
        refuse_with(&client, portal, "connections[0]").await;
        let mut slide = plan_for("MAP05");
        slide["connections"][0]["style"] = json!("slide");
        refuse_with(&client, slide, "normal, fast, stay_open").await;
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "no refusal wrote anything");
    }

    #[tokio::test]
    async fn rooms_that_do_not_touch_refuse_with_the_connection_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let far = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}},
                {"id": "b", "rect": {"x": 400_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}}
            ],
            "connections": [{"id": "d1", "from": "a", "to": "b", "kind": "opening"}]
        });
        let (code, message) = call_err(&client, "build_map_from_plan", far).await;
        assert_eq!(code, "invalid_argument", "rooms that miss");
        assert!(
            message.contains("connections[0]") && message.contains("do not touch"),
            "the path and the compiler's words: {message}"
        );
    }

    #[tokio::test]
    async fn image_space_units_scale_and_mirror_the_plan_into_map_units() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let request = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "shot", "rect": {"x": 0_i64, "y": 0_i64, "w": 100_i64, "h": 50_i64}}
            ],
            "connections": [],
            "units": {"scale_num": 4_i64, "flip_y": true},
            "player_start": {"room": "shot"}
        });
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["bounds"],
            json!({"x1": 0_i64, "y1": -200_i64, "x2": 400_i64, "y2": 0_i64}),
            "a 100x50 px room becomes a 400x200 room with y mirrored below 0"
        );
        let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
        assert_eq!(
            summary["bounds"]["min_x"],
            json!(0_i64),
            "the scale is written"
        );
        assert_eq!(summary["bounds"]["max_x"], json!(400_i64));
        assert_eq!(
            summary["bounds"]["min_y"],
            json!(-200_i64),
            "the mirror is written"
        );
        assert_eq!(summary["bounds"]["max_y"], json!(0_i64));
    }

    #[tokio::test]
    async fn the_grid_snaps_every_coordinate_to_the_nearest_line() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let request = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 33_i64, "y": 33_i64, "w": 64_i64, "h": 64_i64}}
            ],
            "connections": [],
            "grid": 16_i64,
            "player_start": {"room": "a"}
        });
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["bounds"],
            json!({"x1": 32_i64, "y1": 32_i64, "x2": 96_i64, "y2": 96_i64}),
            "the 33,33-origin room snaps to 32,32 (and its far corner 97,97 to 96,96)"
        );
        let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
        assert_eq!(
            summary["bounds"]["min_x"],
            json!(32_i64),
            "the snap is written"
        );
        assert_eq!(summary["bounds"]["min_y"], json!(32_i64));
        assert_eq!(summary["bounds"]["max_y"], json!(96_i64));
    }

    #[tokio::test]
    async fn unusable_units_and_grid_are_refused_naming_the_parameter() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut zero_den = plan_for("MAP05");
        zero_den["units"] = json!({"scale_den": 0_i64});
        refuse_with(&client, zero_den, "scale_den").await;
        let mut zero_num = plan_for("MAP05");
        zero_num["units"] = json!({"scale_num": 0_i64});
        refuse_with(&client, zero_num, "scale_num").await;
        let mut zero_grid = plan_for("MAP05");
        zero_grid["grid"] = json!(0_i64);
        refuse_with(&client, zero_grid, "grid").await;
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "no refusal wrote anything");
    }

    #[tokio::test]
    async fn a_udmf_plan_lands_as_a_named_text_block() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["format"] = json!("udmf");
        request["namespace"] = json!("zdoom");
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(result["format"], json!("udmf"), "the layout echoes");
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP05", "scope": "working"}),
        )
        .await;
        assert_eq!(summary["format"], json!("udmf"), "a TEXTMAP block");
        assert_eq!(
            summary["udmf_namespace"],
            json!("zdoom"),
            "the declared namespace"
        );
        assert_eq!(summary["counts"]["sectors"], json!(4_u64), "the same plan");
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"][0]["format"], json!("udmf"), "listed as UDMF");
    }

    #[tokio::test]
    async fn a_theme_override_dresses_one_call_and_leaves_the_session_alone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let before = call(&client, "get_texture_theme", json!({})).await;
        assert_eq!(before["theme"], json!("tech"), "the default theme");
        let mut request = plan_for("MAP05");
        request["theme"] = json!("HELL");
        call(&client, "build_map_from_plan", request).await;
        let after = call(&client, "get_texture_theme", json!({})).await;
        assert_eq!(after["theme"], json!("tech"), "the session never moved");
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"].as_array().expect("a list").len(), 1);
        let mut bad_theme = plan_for("MAP06");
        bad_theme["theme"] = json!("nope");
        let (code, message) = call_err(&client, "build_map_from_plan", bad_theme).await;
        assert_eq!(code, "invalid_argument", "an unknown theme is refused");
        assert!(
            message.contains("tech") && message.contains("hell"),
            "the refusal lists the themes: {message}"
        );
    }

    /// The `things` rows of the working map's assembled graph.
    async fn working_things(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["things"]}),
        )
        .await;
        report["things"]
            .as_array()
            .expect("things is an array")
            .clone()
    }

    /// The thing rows of type `kind`, by type number.
    fn things_of_kind(rows: &[Value], kind: i64) -> Vec<Value> {
        rows.iter()
            .filter(|row| row["type"].as_i64() == Some(kind))
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn named_things_fill_a_room_and_the_start_lands_in_its_centre() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["things"] = json!([{"name": "imp", "room": "a", "count": 4_u64}]);
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["things_placed"],
            json!(5_u64),
            "four imps and the player start"
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP05", "scope": "working"}),
        )
        .await;
        assert_eq!(
            summary["counts"]["things"],
            json!(5_u64),
            "the written block holds every placed row"
        );
        assert_eq!(
            summary["player_starts"],
            json!([1_u64]),
            "the room-form start is a real type 1"
        );
        let report = call(
            &client,
            "validate_map",
            json!({"map": "MAP05", "scope": "working", "codes": ["no_player1_start"]}),
        )
        .await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "no player-start error: {:?}",
            report["issues"]
        );
        let imps = things_of_kind(&working_things(&client, "MAP05").await, 3001_i64);
        assert_eq!(imps.len(), 4, "four imps in room a");
        assert!(
            imps.iter()
                .all(|row| row["x"].as_i64().is_some_and(|x| x > 0_i64)
                    && row["y"].as_i64().is_some_and(|y| y > 0_i64)),
            "every imp stands clear of the walls: {imps:?}"
        );
    }

    #[tokio::test]
    async fn a_pinned_named_point_travels_the_units_transform() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let request = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
            ],
            "connections": [],
            "units": {"scale_num": 4_i64},
            "things": [{"name": "barrel", "x": 10_i64, "y": 10_i64}],
            "player_start": {"room": "a"}
        });
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(result["things_placed"], json!(2_u64), "barrel plus start");
        let barrels = things_of_kind(&working_things(&client, "MAP05").await, 2035_i64);
        let barrel = barrels.first().expect("one barrel");
        assert_eq!(
            (barrel["x"].as_i64(), barrel["y"].as_i64()),
            (Some(40_i64), Some(40_i64)),
            "plan (10, 10) at scale 4 lands on map (40, 40)"
        );
    }

    #[tokio::test]
    async fn skills_and_multiplayer_only_round_trip_into_thing_flags() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["things"] = json!([
            {"type": 2035_u64, "x": 40_i64, "y": 40_i64,
             "skills": ["easy"], "multiplayer_only": true},
            {"type": 2035_u64, "room": "b", "count": 1_u64}
        ]);
        call(&client, "build_map_from_plan", request).await;
        let barrels = things_of_kind(&working_things(&client, "MAP05").await, 2035_i64);
        assert_eq!(barrels.len(), 2, "both barrel rows landed");
        let pinned = barrels.first().expect("two barrels");
        assert_eq!(
            pinned["flags"],
            json!(17_u64),
            "easy (1) + notsingle (16) in the Doom family"
        );
        assert_eq!(
            pinned["flag_names"],
            json!(["easy", "notsingle"]),
            "the names read back from the written block"
        );
        let plain = barrels.get(1).expect("two barrels");
        assert_eq!(
            plain["flag_names"],
            json!(["easy", "medium", "hard"]),
            "unset skills and mode default to present everywhere"
        );
    }

    #[tokio::test]
    async fn an_ambiguous_name_is_refused_listing_the_candidates() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["things"] = json!([{"name": "skul", "room": "a"}]);
        let (code, message) = call_err(&client, "build_map_from_plan", request).await;
        assert_eq!(code, "invalid_argument", "several things match 'skul'");
        assert!(message.contains("things[0]"), "the row is named: {message}");
        assert!(
            message.contains("Skull on Pole"),
            "the candidates: {message}"
        );
        assert!(
            message.contains("Pile of Skulls"),
            "the candidates: {message}"
        );
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
    }

    #[tokio::test]
    async fn a_plan_without_any_player_start_is_refused_by_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request
            .as_object_mut()
            .expect("the plan object")
            .remove("player_start");
        let (code, message) = call_err(&client, "build_map_from_plan", request.clone()).await;
        assert_eq!(code, "invalid_argument", "type 1 is mandatory");
        assert!(
            message.contains("a map needs a player 1 start"),
            "the documented advice: {message}"
        );
        let mut named = request;
        named["things"] = json!([{"type": 1_u64, "room": "a"}]);
        let result = call(&client, "build_map_from_plan", named).await;
        assert_eq!(
            result["things_placed"],
            json!(1_u64),
            "a thing of type 1 satisfies the check"
        );
    }

    #[tokio::test]
    async fn a_count_larger_than_the_room_warns_fits_only_and_keeps_what_fit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let request = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "closet", "rect": {"x": 0_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}}
            ],
            "connections": [],
            "things": [{"name": "imp", "room": "closet", "count": 4_u64}],
            "player_start": {"room": "closet"}
        });
        let result = call(&client, "build_map_from_plan", request).await;
        let warnings = result["warnings"].as_array().expect("a warning list");
        assert!(
            warnings
                .iter()
                .any(|note| note.as_str() == Some("room 'closet' fits only 1 of 4 things")),
            "the short room is noted: {warnings:?}"
        );
        assert_eq!(
            result["things_placed"],
            json!(2_u64),
            "one imp cleared the walls, plus the centre start"
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP05", "scope": "working"}),
        )
        .await;
        assert_eq!(summary["counts"]["things"], json!(2_u64), "as written");
    }

    #[tokio::test]
    async fn an_unknown_thing_room_names_its_row_and_room() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["things"] = json!([{"name": "imp", "room": "ghost"}]);
        refuse_with(&client, request, "things[0].room").await;
    }

    #[tokio::test]
    async fn a_finalized_build_ships_the_whole_node_run_and_clean_validation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["finalize"] = json!(true);
        let result = call(&client, "build_map_from_plan", request).await;
        expect_real_tree(&result["nodes"], "classic");
        assert_eq!(
            result["validation"]["errors"],
            json!([]),
            "the finalized map validates clean: {:?}",
            result["validation"],
        );
        assert_eq!(
            result["validation"]["counts"]["error"],
            json!(0_u64),
            "the exact count agrees with the empty list",
        );
        let read = call(
            &client,
            "get_nodes",
            json!({"map": "MAP05", "scope": "working"}),
        )
        .await;
        assert_eq!(read["format"], json!("vanilla"), "the run reads back");
        expect_summary_matches_block(&read, &result["nodes"]);
        let listed = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(
            listed["maps"][0]["has_nodes"],
            json!(true),
            "the block lists with node data",
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["build_map_from_plan MAP05"]),
            "the finalize build stays inside the one undo step",
        );
        let after = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(
            after["maps"],
            json!([]),
            "undo took the nodes with the block"
        );
    }

    #[tokio::test]
    async fn a_finalized_udmf_build_ships_znodes_and_no_binary_lumps() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["format"] = json!("udmf");
        request["finalize"] = json!(true);
        let result = call(&client, "build_map_from_plan", request).await;
        expect_real_tree(&result["nodes"], "extended");
        assert_eq!(
            result["nodes"]["format"],
            json!("ZNOD"),
            "compressed by the default",
        );
        assert_eq!(
            result["validation"]["errors"],
            json!([]),
            "no REJECT/BLOCKMAP complaints on a UDMF build: {:?}",
            result["validation"],
        );
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        let lumps = maps["maps"][0]["lumps"]
            .as_array()
            .expect("the row lists lumps");
        assert!(
            lumps.contains(&json!("ZNODES")),
            "the extended lump is in the block: {lumps:?}",
        );
        expect_no_lumps(lumps, ["REJECT", "BLOCKMAP", "SEGS", "SSECTORS"]);
        let read = call(
            &client,
            "get_nodes",
            json!({"map": "MAP05", "scope": "working"}),
        )
        .await;
        assert_eq!(read["format"], json!("znod"), "the lump reads back");
        expect_summary_matches_block(&read, &result["nodes"]);
    }

    #[tokio::test]
    async fn finalize_false_ships_the_block_without_node_data() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["finalize"] = json!(false);
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(result["nodes"], json!(null), "false is the unset default");
        assert_eq!(result["validation"], json!(null), "and validates nothing");
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(
            maps["maps"][0]["has_nodes"],
            json!(false),
            "the block ships bare",
        );
    }

    /// The complete worked example of `skill/reference/plan.md`: a
    /// sketch-pixel three-room plan with a door, a stair connector,
    /// named things and `finalize`.
    fn worked_example() -> Value {
        json!({
            "map": "MAP07",
            "units": {"scale_num": 4_i64, "flip_y": true},
            "rooms": [
                {"id": "court",  "rect": {"x": 0_i64,  "y": 0_i64, "w": 60_i64, "h": 48_i64}},
                {"id": "hall",   "rect": {"x": 60_i64, "y": 0_i64, "w": 48_i64, "h": 48_i64},
                 "light": 192},
                {"id": "crypt",  "rect": {"x": 60_i64, "y": 48_i64, "w": 48_i64, "h": 36_i64},
                 "floor": 64, "ceiling": 224, "light": 96}
            ],
            "connections": [
                {"id": "gate", "from": "court", "to": "hall", "kind": "door",
                 "at": [60_i64, 24_i64], "width": 48_i64, "style": "locked_red"},
                {"from": "hall", "to": "crypt", "kind": "stairs",
                 "at": [84_i64, 48_i64], "width": 64_i64}
            ],
            "things": [
                {"name": "imp", "room": "hall", "count": 3_u64},
                {"name": "barrel", "x": 16_i64, "y": 40_i64}
            ],
            "player_start": {"room": "court", "angle": 90_i64},
            "finalize": true
        })
    }

    #[tokio::test]
    async fn the_documented_worked_example_builds_finalized_and_validates_clean() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "build_map_from_plan", worked_example()).await;
        assert_eq!(
            result["rooms"],
            json!({"court": [0_u64], "hall": [1_u64], "crypt": [2_u64]}),
            "the three rooms keep their plan order",
        );
        assert_eq!(
            result["connections"],
            json!([
                {"id": "gate", "kind": "door", "sectors": [3_u64]},
                {"id": "hall->crypt", "kind": "stairs", "sectors": [4_u64, 5_u64, 6_u64, 7_u64]}
            ]),
            "one door band, four stair steps up to the crypt's floor",
        );
        assert_eq!(
            result["bounds"],
            json!({"x1": 0_i64, "y1": -336_i64, "x2": 432_i64, "y2": 0_i64}),
            "the 108x84 pixel plan at scale 4 with mirrored y",
        );
        assert_eq!(result["things_placed"], json!(5_u64), "imps, barrel, start");
        assert_eq!(
            result["features"],
            json!([
                {"kind": "door", "room": null, "sectors": [3_u64], "lines": [5_u64, 17_u64],
                 "tag": 1_u64}
            ]),
            "the wired gate reports its band, lines and tag",
        );
        assert_eq!(
            result["nodes"],
            json!({
                "variant": "classic",
                "segs": 61_u64,
                "subsectors": 15_u64,
                "nodes": 14_u64,
                "split_vertices": 5_u64,
                "warnings": []
            }),
            "the documented classic build summary",
        );
        assert_eq!(
            result["validation"],
            json!({"errors": [], "counts": {"error": 0_u64, "warning": 4_u64, "info": 6_u64}}),
            "no errors, and the full counts beside the empty list",
        );
        let read = call(
            &client,
            "get_nodes",
            json!({"map": "MAP07", "scope": "working"}),
        )
        .await;
        assert_eq!(
            read["format"],
            json!("vanilla"),
            "the block carries the run"
        );
    }

    #[tokio::test]
    async fn feature_rows_build_like_their_feature_free_twin_and_refuse_by_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut dressed = plan_for("MAP05");
        dressed["features"] = json!([
            {"kind": "exit", "room": "b", "style": "walk_over",
             "secret": true, "wall": "north"},
            {"kind": "switch", "room": "a", "opens": "d1", "wall": "east"},
            {"kind": "lift", "room": "b", "low_floor": 0_i64, "style": "use_wall"},
            {"kind": "teleporter", "from_room": "a", "to_room": "b",
             "pad": [64_i64, 32_i64], "destination": [192_i64, 96_i64],
             "angle": 90_i64, "two_way": true}
        ]);
        let with_features = call(&client, "build_map_from_plan", dressed.clone()).await;
        let assembled_dressed = call(&client, "assemble_map", json!({"map": "MAP05"})).await;
        call(&client, "undo", json!({})).await;
        let plain = call(&client, "build_map_from_plan", plan_for("MAP05")).await;
        let assembled_plain = call(&client, "assemble_map", json!({"map": "MAP05"})).await;
        assert_eq!(
            with_features, plain,
            "features install nothing yet: the results agree row for row"
        );
        assert_eq!(
            assembled_dressed, assembled_plain,
            "and the written block is the block the feature-free plan builds"
        );
        let mut ghost = dressed.clone();
        ghost["features"][0]["room"] = json!("ghost");
        refuse_with(&client, ghost, "features[0]").await;
        let mut orphan = dressed;
        orphan["features"][1]["opens"] = json!("nope");
        refuse_with(&client, orphan, "features[1]").await;
    }
}
