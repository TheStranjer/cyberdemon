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
//! pair when `finalize` is not asked for. The `features` rows: a `lift`
//! row tags its room's sector and dresses the down-facing edges, a
//! `teleporter` row draws the pad and destination squares and stands
//! its type-14 things in the written block, a `secret` room with a
//! `light_effect` ends lit and hidden with the Boom warning passed
//! through; a `switch` row mounts its panel on the chosen wall and
//! fires the remote door's own tag, an `exit` row ends the
//! level on the room's wall (`switch`) or shared boundaries
//! (`walk_over`), and rows naming an unknown room or an `opens` id no
//! connection carries refuse naming `features[i]`.
//! The door wiring: a blue-key plan's `features` row matches the written
//! block — its lines carry special `26` and the band's tag with fronts
//! facing their rooms, validation stays error-free and one `undo` takes
//! it all back. The playable-map suite: one six-room plan (locked door,
//! lift, teleporter, remote gate with its switch, `walk_over` exit, lit
//! and secret rooms, a named keycard and the player start) builds with
//! an empty `warnings` list and every feature reads back from the
//! written block — specials `26`/`62`/`52`/`97`, the platform tag, the
//! strobed exit sector `3`, the secret sector `9` and a type-14 thing
//! standing in the ledge; dropping the keycard, the exit row or the
//! switch replaces the silence with exactly the matching advisory
//! warning. And a `UDMF` `zdoom` locked door carries the Hexen-family
//! `Door_LockedRaise` (`13` with the key in `args[3]`, playeruse).

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
        let (code, message, _) = call_err(client, "build_map_from_plan", request).await;
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

    /// The row of linedef `index` in an assembled `lines` list.
    fn line_row(lines: &[Value], index: u64) -> &Value {
        lines
            .iter()
            .find(|row| row["index"] == json!(index))
            .unwrap_or_else(|| panic!("line {index} is in the block"))
    }

    /// The first linedef index a `features` row reports.
    fn first_line(row: &Value) -> u64 {
        row["lines"][0].as_u64().expect("one reported line")
    }

    /// Assert every row of `lines` whose index is in `indices` carries
    /// the `W1` walk-over level end.
    fn assert_walk_lines(lines: &[Value], indices: &[u64]) {
        let crossed: Vec<&Value> = lines
            .iter()
            .filter(|line| indices.contains(&line["index"].as_u64().expect("an index")))
            .collect();
        assert_eq!(
            crossed.len(),
            indices.len(),
            "every reported line is in the block"
        );
        for line in crossed {
            let index = line["index"].as_u64().expect("an index");
            assert_eq!(
                line["special"],
                json!(52_i64),
                "the W1 exit on line {index}"
            );
        }
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
        let (code, message, _) = call_err(&client, "build_map_from_plan", plan_for("MAP01")).await;
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
        let (code, message, _) = call_err(&client, "build_map_from_plan", far).await;
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
        let (code, message, _) = call_err(&client, "build_map_from_plan", bad_theme).await;
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
        let (code, message, _) = call_err(&client, "build_map_from_plan", request).await;
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
        let (code, message, _) = call_err(&client, "build_map_from_plan", request.clone()).await;
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
            result["warnings"],
            json!([
                "red door at gate but no red key in the plan",
                "the map has no exit"
            ]),
            "the playability checks answer for the example's gaps"
        );
        expect_worked_example_features_and_finalization(&result);
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

    /// The worked example's wired gate row and its finalized `nodes`
    /// summary and errors-only `validation` pass, exactly as
    /// `skill/reference/plan.md` documents them.
    fn expect_worked_example_features_and_finalization(result: &Value) {
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
    }

    /// The `lines` a reported feature row names, as indices.
    fn row_line_indices(row: &Value) -> Vec<u64> {
        row["lines"]
            .as_array()
            .expect("a line list")
            .iter()
            .map(|slot| slot.as_u64().expect("an index"))
            .collect()
    }

    /// Assert every linedef indexed in `indices` carries `special` and
    /// line tag `tag`.
    fn assert_line_actions(lines: &[Value], indices: &[u64], special: i64, tag: i64) {
        for index in indices {
            let line = line_row(lines, *index);
            assert_eq!(
                line["special"],
                json!(special),
                "the special of line {index}"
            );
            assert_eq!(line["tag"], json!(tag), "the tag of line {index}");
        }
    }

    /// Assert every linedef indexed in `indices` is the hall-facing SR
    /// ride in the theme's step band.
    fn assert_ride_lines(lines: &[Value], indices: &[u64]) {
        assert_eq!(indices.len(), 2_usize, "the two shared-wall segments");
        for index in indices {
            let line = line_row(lines, *index);
            assert_eq!(
                line["special"],
                json!(62_i64),
                "the SR ride on line {index}"
            );
            assert_eq!(line["tag"], json!(1_i64), "answering with the platform tag");
            assert_eq!(
                line["front"]["sector"],
                json!(0_i64),
                "the front looks down at the hall"
            );
            assert_eq!(
                line["front"]["lower"],
                json!("STEP1"),
                "the theme's step band"
            );
        }
    }

    /// Lift and teleporter rows install on the compiled map and report
    /// their rows after the connectors; every feature row still
    /// reference-checks by plan path.
    #[tokio::test]
    async fn lift_and_teleporter_rows_install_and_refuse_by_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["features"] =
            json!([{"kind": "teleporter", "from_room": "a", "to_room": "b", "two_way": true}]);
        let result = call(&client, "build_map_from_plan", request).await;
        let rows = result["features"].as_array().expect("a feature list");
        assert_eq!(rows.len(), 2, "the door leads, the teleporter follows");
        assert_eq!(rows[0]["kind"], json!("door"), "the plain door wires first");
        assert_eq!(rows[0]["tag"], json!(1_u64), "its own tag");
        expect_teleporter_block(&client, rows.get(1).expect("the teleporter row")).await;
        let lines = working_lines(&client, "MAP05").await;
        assert_line_actions(&lines, &row_line_indices(&rows[0]), 1_i64, 1_i64);
        let things = working_things(&client, "MAP05").await;
        assert_eq!(
            things_of_kind(&things, 14_i64).len(),
            2_usize,
            "a type-14 thing on each square"
        );
        let mut ghost = plan_for("MAP06");
        ghost["features"] = json!([{"kind": "exit", "room": "ghost", "style": "walk_over"}]);
        refuse_with(&client, ghost, "features[0]").await;
        let mut orphan = plan_for("MAP06");
        orphan["features"] = json!([{"kind": "switch", "room": "a", "opens": "nope"}]);
        refuse_with(&client, orphan, "features[0]").await;
    }

    /// Assert the teleporter row and its written block: the pad room is
    /// named, the sector list is pad then destination, the Doom
    /// destination answers with the row's fresh tag and the two-way pad
    /// with the return tag above it, and the row's lines carry the WR
    /// teleport. (Door lines re-checked elsewhere prove the reported
    /// indices survived the squares' rebuild.)
    async fn expect_teleporter_block(client: &Client, row: &Value) {
        assert_eq!(row["kind"], json!("teleporter"));
        assert_eq!(row["room"], json!("a"), "the pad's room");
        assert_eq!(row["tag"], json!(2_u64), "a fresh tag after the door's");
        let sectors = row["sectors"].as_array().expect("two sectors");
        assert_eq!(sectors.len(), 2, "the pad then the destination");
        let pad = usize::try_from(
            sectors
                .first()
                .expect("the pad")
                .as_u64()
                .expect("an index"),
        )
        .expect("small");
        let dest = usize::try_from(
            sectors
                .get(1)
                .expect("the dest")
                .as_u64()
                .expect("an index"),
        )
        .expect("small");
        let dest_sector = working_sector(client, "MAP05", dest).await;
        assert_eq!(
            dest_sector["tag"],
            json!(2_i64),
            "the Doom destination answers with the outward tag"
        );
        let pad_sector = working_sector(client, "MAP05", pad).await;
        assert_eq!(
            pad_sector["tag"],
            json!(3_i64),
            "the two-way pad answers with the return tag"
        );
        let lines = working_lines(client, "MAP05").await;
        assert_line_actions(&lines, &row_line_indices(row), 97_i64, 2_i64);
    }

    /// The `lift_request` plan: a 64-unit-high ledge room over a hall,
    /// touching at zero thickness, joined by an opening and lifted.
    fn lift_request() -> Value {
        json!({
            "map": "MAP05",
            "wall_thickness": 0_i64,
            "rooms": [
                {"id": "hall", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "ledge", "rect": {"x": 0_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64},
                 "floor": 64_i64}
            ],
            "connections": [
                {"id": "gap", "from": "ledge", "to": "hall", "kind": "opening"}
            ],
            "features": [{"kind": "lift", "room": "ledge", "style": "use_wall"}],
            "player_start": {"room": "hall"}
        })
    }

    /// The plan lift: the raised room's own sector becomes the tagged
    /// platform, the shared wall turns front-down with the SR ride in
    /// the theme's step band — and the same join as a door is refused.
    #[tokio::test]
    async fn a_plan_lift_rides_the_shared_wall_of_the_raised_room() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "build_map_from_plan", lift_request()).await;
        let rows = result["features"].as_array().expect("a feature list");
        assert_eq!(rows.len(), 1, "an opening is no feature row: {rows:?}");
        let row = rows.first().expect("the lift row");
        assert_eq!(row["kind"], json!("lift"));
        assert_eq!(
            row["room"],
            json!("ledge"),
            "the raised room is the platform"
        );
        assert_eq!(row["sectors"], json!([1_u64]), "the room's own sector");
        assert_eq!(row["tag"], json!(1_u64), "a fresh tag");
        assert_eq!(
            working_sector(&client, "MAP05", 1_usize).await["tag"],
            json!(1_i64),
            "the platform answers with it"
        );
        let lines = working_lines(&client, "MAP05").await;
        assert_ride_lines(&lines, &row_line_indices(row));
        let mut joined = lift_request();
        joined["map"] = json!("MAP06");
        joined["connections"][0]["kind"] = json!("door");
        refuse_with(&client, joined, "a lift needs openings to the lower rooms").await;
    }

    /// A `secret` room with a `light_effect` ends up lit and hidden in
    /// the written block — the strobe type under the Boom secret bit —
    /// with the vanilla-Doom warning passed through.
    #[tokio::test]
    async fn a_lit_secret_room_builds_lit_and_hidden() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "build_map_from_plan",
            json!({
                "map": "MAP05",
                "rooms": [{
                    "id": "vault",
                    "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
                    "secret": true,
                    "light_effect": "strobe_slow"
                }],
                "connections": [],
                "player_start": {"room": "vault"}
            }),
        )
        .await;
        let rows = result["features"].as_array().expect("a feature list");
        let kinds: Vec<&str> = rows
            .iter()
            .map(|row| row["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(kinds, vec!["light", "secret"], "lights run before secrets");
        assert_eq!(rows[0]["sectors"], json!([0_u64]), "the room's sector");
        assert_eq!(
            rows[0]["lines"],
            json!([]),
            "a painted special owns no line"
        );
        assert_eq!(rows[0]["tag"], json!(null), "and no tag");
        let sector = working_sector(&client, "MAP05", 0_usize).await;
        assert_eq!(
            sector["special"],
            json!(131_i64),
            "strobe 3 with the Boom bit 128"
        );
        assert_eq!(sector["light"], json!(160_i64), "the level is kept");
        let warnings = result["warnings"].as_array().expect("a warning list");
        assert!(
            warnings.iter().any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("special 131"))),
            "the vanilla warning passes through: {warnings:?}"
        );
    }

    /// A remote door and its switch: the build reports the rows in
    /// pipeline order and the written block carries the action, the
    /// door's own tag and the theme's panel.
    #[tokio::test]
    async fn a_switch_fires_the_remote_door_it_opens() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["connections"][0]["style"] = json!("remote");
        request["features"] =
            json!([{"kind": "switch", "room": "a", "opens": "d1", "wall": "west"}]);
        let result = call(&client, "build_map_from_plan", request).await;
        let rows = result["features"].as_array().expect("a feature list");
        assert_eq!(
            rows.len(),
            2,
            "the door leads, the switch follows: {rows:?}"
        );
        assert_eq!(rows[0]["kind"], json!("door"), "the door is wired first");
        assert_eq!(rows[0]["tag"], json!(1_u64), "the band keeps its tag");
        assert_eq!(rows[1]["kind"], json!("switch"), "the switch mounts after");
        assert_eq!(rows[1]["room"], json!("a"), "in room a");
        assert_eq!(rows[1]["sectors"], json!([2_u64]), "firing the door band");
        assert_eq!(rows[1]["tag"], json!(1_u64), "with the door's own tag");
        let lines = working_lines(&client, "MAP05").await;
        let panel = line_row(&lines, first_line(&rows[1]));
        assert_eq!(panel["special"], json!(103_i64), "S1 opens the remote door");
        assert_eq!(panel["tag"], json!(1_i64), "on the door's own tag");
        assert_eq!(
            panel["front"]["middle"],
            json!("SW1STRTN"),
            "the theme's switch panel"
        );
        call(&client, "undo", json!({})).await;
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "one undo takes the wiring back");
    }

    /// Both exit styles land on the written block: the walk-over row
    /// dresses the room's shared boundaries with `52` and the switch
    /// row carries the secret level end in the theme's exit panel.
    #[tokio::test]
    async fn the_two_exit_styles_end_the_level_where_the_room_stands() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["features"] = json!([
            {"kind": "exit", "room": "b", "style": "walk_over"},
            {"kind": "exit", "room": "b", "style": "switch", "secret": true, "wall": "east"}
        ]);
        let result = call(&client, "build_map_from_plan", request).await;
        let rows = result["features"].as_array().expect("a feature list");
        assert_eq!(rows[0]["kind"], json!("door"), "the plain door wires first");
        let walked = rows.get(1).expect("the walk-over row");
        let mounted = rows.get(2).expect("the switch row");
        assert_eq!(walked["kind"], json!("exit"), "exits come after the door");
        assert_eq!(walked["room"], json!("b"), "the room it stands in");
        assert_eq!(walked["sectors"], json!([1_u64]), "its own sector");
        assert_eq!(walked["tag"], json!(null), "an exit carries no tag");
        assert_eq!(mounted["kind"], json!("exit"));
        let walk_lines: Vec<u64> = walked["lines"]
            .as_array()
            .expect("a line list")
            .iter()
            .map(|slot| slot.as_u64().expect("an index"))
            .collect();
        assert!(!walk_lines.is_empty(), "room b shares its band walls");
        let lines = working_lines(&client, "MAP05").await;
        assert_walk_lines(&lines, &walk_lines);
        let secret = line_row(&lines, first_line(mounted));
        assert_eq!(secret["special"], json!(51_i64), "the S1 secret exit");
        assert_eq!(secret["tag"], json!(0_i64), "an exit acts on no tag");
        assert_eq!(
            secret["front"]["middle"],
            json!("SW1EXIT"),
            "the theme's exit panel"
        );
    }

    /// A switch pointed at a non-remote door is refused, and nothing is
    /// written.
    #[tokio::test]
    async fn a_switch_on_a_plain_door_is_refused_as_not_remote() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["features"] =
            json!([{"kind": "switch", "room": "a", "opens": "d1", "wall": "west"}]);
        let (code, message, _) = call_err(&client, "build_map_from_plan", request).await;
        assert_eq!(code, "invalid_argument", "a plain door is no target");
        assert!(
            message.contains("switch opens d1 which is not a remote door"),
            "the refusal words: {message}"
        );
        let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
        assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
    }

    /// The six-room playable plan: a `foyer` opens into `start`, the
    /// id'd `blue_door` locks the `corridor` off, a `use_wall` lift
    /// raises the corridor to the `ledge`, a teleporter runs corridor to
    /// ledge, the remote `gate` (its wall `switch` in the ledge) locks
    /// the strobed `exit` room, and an ordinary `cell_door` hides the
    /// `secret` room off the corridor. The blue card stands in `start`
    /// beside the player start: every lock has its key, the map has its
    /// exit and the remote has its switch, so the playability checks
    /// have nothing to say.
    fn six_room_plan(map: &str) -> Value {
        json!({
            "map": map,
            "format": "doom",
            "wall_thickness": 0_i64,
            "rooms": [
                {"id": "start", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "corridor", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "secret", "rect": {"x": 256_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
                 "secret": true},
                {"id": "ledge", "rect": {"x": 128_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64},
                 "floor": 24_i64},
                {"id": "exit", "rect": {"x": 128_i64, "y": 256_i64, "w": 128_i64, "h": 128_i64},
                 "floor": 24_i64, "light_effect": "strobe_slow"},
                {"id": "foyer", "rect": {"x": -128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
            ],
            "connections": [
                {"id": "blue_door", "from": "start", "to": "corridor", "kind": "door",
                 "style": "locked_blue"},
                {"id": "cell_door", "from": "corridor", "to": "secret", "kind": "door",
                 "style": "normal"},
                {"id": "gap", "from": "corridor", "to": "ledge", "kind": "opening"},
                {"id": "gate", "from": "ledge", "to": "exit", "kind": "door",
                 "style": "remote"},
                {"id": "home", "from": "start", "to": "foyer", "kind": "opening"}
            ],
            "features": [
                {"kind": "lift", "room": "ledge", "style": "use_wall", "low_floor": -40_i64},
                {"kind": "teleporter", "from_room": "corridor", "to_room": "ledge"},
                {"kind": "switch", "room": "ledge", "opens": "gate", "wall": "east"},
                {"kind": "exit", "room": "exit", "style": "walk_over"}
            ],
            "things": [{"name": "Blue Keycard", "room": "start"}],
            "player_start": {"room": "start"}
        })
    }

    /// The one sector the result reports for room `room`.
    fn room_sector(result: &Value, room: &str) -> u64 {
        result["rooms"][room]
            .as_array()
            .expect("a room sector list")
            .first()
            .expect("plan rooms own one sector")
            .as_u64()
            .expect("an index")
    }

    /// Assert the remote `gate` writes no action of its own: every
    /// reported band line is either still bare or carries the exit
    /// step's `W1` end (the lines facing the exit room are also that
    /// room's boundary), and none carries a tag or args.
    fn assert_remote_gate_lines(lines: &[Value], indices: &[u64]) {
        for index in indices {
            let line = line_row(lines, *index);
            assert!(
                matches!(line["special"].as_i64(), Some(0_i64 | 52_i64)),
                "a remote line carries no door action of its own: {line}"
            );
            assert_eq!(line["tag"], json!(0_i64), "and no tag on line {index}");
        }
    }

    /// Assert the row at `index` is a `kind` row naming `room` (when
    /// `room` is `Some`).
    fn expect_row<'rows>(
        rows: &'rows [Value],
        index: usize,
        kind: &str,
        room: Option<&str>,
    ) -> &'rows Value {
        let row = rows
            .get(index)
            .unwrap_or_else(|| panic!("row {index} must be a {kind}"));
        assert_eq!(row["kind"], json!(kind), "row {index}'s class");
        assert_eq!(
            row["room"],
            room.map_or(json!(null), |name| json!(name)),
            "row {index} stands in its room"
        );
        row
    }

    /// Assert the blue-key door row: tag `1`, the band sector answering
    /// it, and every door line a special-`26` use line with the band
    /// behind and one of the two rooms' sectors in front.
    async fn assert_blue_door(client: &Client, map: &str, result: &Value, row: &Value) {
        assert_eq!(row["tag"], json!(1_u64), "the first door tags first");
        let band = row["sectors"][0].as_u64().expect("one band");
        let space = working_sector(client, map, usize::try_from(band).expect("small")).await;
        assert_eq!(space["tag"], json!(1_i64), "the band answers its tag");
        let start = room_sector(result, "start");
        let corridor = room_sector(result, "corridor");
        let lines = working_lines(client, map).await;
        let indices = row_line_indices(row);
        assert!(!indices.is_empty(), "the band meets its rooms");
        for index in indices {
            let line = line_row(&lines, index);
            assert_eq!(
                line["special"],
                json!(26_i64),
                "DR blue lock on line {index}"
            );
            assert_eq!(line["tag"], json!(1_i64), "on the band's tag");
            assert_eq!(line["back"]["sector"], json!(band), "the band sits behind");
            let front = line["front"]["sector"].as_u64().expect("a front sector");
            assert!(
                front == start || front == corridor,
                "the front faces its room on line {index}: {front}"
            );
        }
    }

    /// Assert the lift row: the ledge's own sector tagged, and its two
    /// activation lines riding with special `62`, front down onto the
    /// corridor.
    async fn assert_lift(client: &Client, map: &str, result: &Value, row: &Value) {
        let tag = row["tag"].as_u64().expect("a fresh tag");
        let platform = row["sectors"][0].as_u64().expect("one platform");
        assert_eq!(
            platform,
            room_sector(result, "ledge"),
            "the room is the platform"
        );
        let space = working_sector(client, map, usize::try_from(platform).expect("small")).await;
        assert_eq!(
            space["tag"].as_u64().expect("a tag"),
            tag,
            "the platform answers with the row's tag"
        );
        let lines = working_lines(client, map).await;
        let indices = row_line_indices(row);
        assert_eq!(indices.len(), 2, "the two shared-wall segments");
        for index in indices {
            let line = line_row(&lines, index);
            assert_eq!(
                line["special"],
                json!(62_i64),
                "the SR ride on line {index}"
            );
            assert_eq!(
                line["tag"].as_u64().expect("a tag"),
                tag,
                "line {index} answers with the platform tag"
            );
            assert_eq!(
                line["front"]["sector"],
                json!(room_sector(result, "corridor")),
                "the front looks down at the corridor"
            );
        }
    }

    /// Assert the teleporter row: pad borders with the `WR` teleport on
    /// the destination's fresh tag, and a type-14 thing standing in the
    /// ledge.
    async fn assert_teleporter(client: &Client, map: &str, row: &Value) {
        assert_eq!(row["room"], json!("corridor"), "the pad's room");
        let tag = row["tag"].as_u64().expect("a fresh tag");
        let sectors = row["sectors"].as_array().expect("two sectors");
        assert_eq!(sectors.len(), 2, "the pad then the destination");
        let dest = usize::try_from(sectors[1].as_u64().expect("an index")).expect("small");
        let space = working_sector(client, map, dest).await;
        assert_eq!(
            space["tag"].as_u64().expect("a tag"),
            tag,
            "the destination answers"
        );
        let lines = working_lines(client, map).await;
        for index in row_line_indices(row) {
            let line = line_row(&lines, index);
            assert_eq!(
                line["special"],
                json!(97_i64),
                "the WR teleport on line {index}"
            );
            assert_eq!(
                line["tag"].as_u64().expect("a tag"),
                tag,
                "to the tagged square"
            );
        }
        let pads = things_of_kind(&working_things(client, map).await, 14_i64)
            .into_iter()
            .filter(|thing| {
                thing["x"]
                    .as_i64()
                    .is_some_and(|x| x > 128_i64 && x < 256_i64)
                    && thing["y"]
                        .as_i64()
                        .is_some_and(|y| y > 128_i64 && y < 256_i64)
            })
            .count();
        assert_eq!(pads, 1_usize, "the destination thing stands in the ledge");
    }

    /// Assert the `walk_over` exit row: special `52` on every reported
    /// boundary line, and the exit room's sector strobing under vanilla
    /// type `3` at its planned light level.
    async fn assert_exit(client: &Client, map: &str, result: &Value, row: &Value) {
        assert_eq!(
            row["room"],
            json!("exit"),
            "the level ends in the exit room"
        );
        assert_eq!(row["tag"], json!(null), "an exit carries no tag");
        let indices = row_line_indices(row);
        assert!(!indices.is_empty(), "the room shares its gate boundary");
        let lines = working_lines(client, map).await;
        assert_walk_lines(&lines, &indices);
        let exit_room = usize::try_from(room_sector(result, "exit")).expect("small");
        let space = working_sector(client, map, exit_room).await;
        assert_eq!(
            space["special"],
            json!(3_i64),
            "strobe_slow lights the exit room"
        );
        assert_eq!(space["light"], json!(160_i64), "the level is kept");
    }

    /// Assert the secret row: the secret room's sectors carry vanilla
    /// type `9`, with no lines and no tag.
    async fn assert_secret(client: &Client, map: &str, result: &Value, row: &Value) {
        assert_eq!(row["room"], json!("secret"), "the hidden room");
        assert_eq!(row["lines"], json!([]), "a painted special owns no line");
        assert_eq!(row["tag"], json!(null), "and no tag");
        let secret = usize::try_from(room_sector(result, "secret")).expect("small");
        assert_eq!(
            row["sectors"],
            json!([secret]),
            "the row names the room's own sector"
        );
        let space = working_sector(client, map, secret).await;
        assert_eq!(space["special"], json!(9_i64), "the vanilla secret type");
    }

    /// The whole playable map at once: six rooms, locked door, lift,
    /// teleporter, remote gate and switch, walk-over exit with a strobe,
    /// a lit-free secret room and the keycard that opens it all. Every
    /// feature reads back from the written block and the playability
    /// checks have nothing to complain about.
    #[tokio::test]
    async fn the_six_room_plan_builds_a_whole_playable_map() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "build_map_from_plan", six_room_plan("MAP05")).await;
        assert_eq!(
            result["warnings"],
            json!([]),
            "key in hand, exit set, remote switched: {:?}",
            result["warnings"]
        );
        assert_eq!(
            result["things_placed"],
            json!(2_u64),
            "the card and the start"
        );
        let rows = result["features"].as_array().expect("a feature list");
        let kinds: Vec<&str> = rows
            .iter()
            .map(|row| row["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(
            kinds,
            vec![
                "door",
                "door",
                "door",
                "switch",
                "lift",
                "teleporter",
                "exit",
                "light",
                "secret"
            ],
            "the pipeline order over the whole plan"
        );
        expect_six_room_rows(&client, "MAP05", &result, rows).await;
        let report = call(&client, "validate_map", json!({"map": "MAP05"})).await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "the playable map validates error-free: {:?}",
            report["issues"]
        );
    }

    /// Assert the nine `features` rows of the playable build against
    /// the written block, in pipeline order: the three wired doors,
    /// the switch on the gate's tag, the ride, the teleport, the exit,
    /// and the painted light and secret.
    async fn expect_six_room_rows(client: &Client, map: &str, result: &Value, rows: &[Value]) {
        assert_blue_door(client, map, result, expect_row(rows, 0, "door", None)).await;
        let lines = working_lines(client, map).await;
        assert_line_actions(
            &lines,
            &row_line_indices(expect_row(rows, 1, "door", None)),
            1_i64,
            2_i64,
        );
        let gate = expect_row(rows, 2, "door", None);
        assert_eq!(gate["tag"], json!(3_u64), "the remote takes the third tag");
        assert_remote_gate_lines(&lines, &row_line_indices(gate));
        let panel = expect_row(rows, 3, "switch", Some("ledge"));
        assert_eq!(
            panel["tag"],
            json!(3_u64),
            "the switch fires the gate's tag"
        );
        assert_line_actions(&lines, &row_line_indices(panel), 103_i64, 3_i64);
        assert_lift(
            client,
            map,
            result,
            expect_row(rows, 4, "lift", Some("ledge")),
        )
        .await;
        assert_teleporter(
            client,
            map,
            expect_row(rows, 5, "teleporter", Some("corridor")),
        )
        .await;
        assert_exit(
            client,
            map,
            result,
            expect_row(rows, 6, "exit", Some("exit")),
        )
        .await;
        let light = expect_row(rows, 7, "light", Some("exit"));
        assert_eq!(
            light["sectors"],
            json!([room_sector(result, "exit")]),
            "the strobe row painted the exit room"
        );
        assert_secret(
            client,
            map,
            result,
            expect_row(rows, 8, "secret", Some("secret")),
        )
        .await;
    }

    #[tokio::test]
    async fn dropping_the_keycard_warns_only_the_blue_door() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = six_room_plan("MAP05");
        request["things"] = json!([]);
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["warnings"],
            json!(["blue door at blue_door but no blue key in the plan"]),
            "the one gap the plan leaves"
        );
    }

    #[tokio::test]
    async fn dropping_the_exit_feature_warns_only_the_missing_exit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = six_room_plan("MAP05");
        request["features"]
            .as_array_mut()
            .expect("a feature list")
            .remove(3);
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["warnings"],
            json!(["the map has no exit"]),
            "the level would never end"
        );
    }

    #[tokio::test]
    async fn dropping_the_switch_warns_only_the_unswitched_gate() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = six_room_plan("MAP05");
        request["features"]
            .as_array_mut()
            .expect("a feature list")
            .remove(2);
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(
            result["warnings"],
            json!([
                "remote door at gate is opened by no switch: add a switch feature whose \
                 opens is gate"
            ]),
            "the locked gate names its way out"
        );
    }

    /// A `UDMF` map in the `zdoom` namespace is the Hexen flag family:
    /// the blue-locked door lines carry `Door_LockedRaise` (`13`) with
    /// speed, wait and the blue key in the args and the `playeruse`
    /// activation bit, and no line tag.
    #[tokio::test]
    async fn a_udmf_zdoom_locked_door_carries_the_hexen_locked_raise() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let mut request = plan_for("MAP05");
        request["connections"][0]["style"] = json!("locked_blue");
        request["format"] = json!("udmf");
        request["namespace"] = json!("zdoom");
        let result = call(&client, "build_map_from_plan", request).await;
        assert_eq!(result["format"], json!("udmf"), "the text layout");
        let rows = result["features"].as_array().expect("a feature list");
        let row = rows.first().expect("the door row");
        assert_eq!(row["kind"], json!("door"), "the locked door wires");
        let lines = working_lines(&client, "MAP05").await;
        let indices = row_line_indices(row);
        assert_eq!(indices.len(), 2, "the band meets each room once");
        expect_hexen_locked_lines(&lines, &indices);
    }

    /// Assert every reported door line is the `zdoom` `Door_LockedRaise`.
    fn expect_hexen_locked_lines(lines: &[Value], indices: &[u64]) {
        for index in indices {
            expect_hexen_locked_line(line_row(lines, *index), *index);
        }
    }

    /// One `UDMF` `zdoom` locked-door line: `Door_LockedRaise` (`13`)
    /// with the back-sector speed/wait/blue-key args, no line tag and
    /// the `playeruse` activation bit.
    fn expect_hexen_locked_line(line: &Value, index: u64) {
        assert_eq!(
            line["special"],
            json!(13_i64),
            "Door_LockedRaise on line {index}"
        );
        assert_eq!(
            line["args"],
            json!([0_i64, 16_i64, 150_i64, 130_i64, 0_i64]),
            "no sector arg, speed 16, wait 150, the blue key"
        );
        assert_eq!(
            line["tag"],
            json!(0_i64),
            "the Hexen action carries no line tag"
        );
        let flags = line["flags"].as_u64().expect("numeric flags");
        assert_eq!(flags & 0x400_u64, 0x400_u64, "playeruse on line {index}");
    }
}
