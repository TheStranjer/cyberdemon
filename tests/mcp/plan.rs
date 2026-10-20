//! The `plan` tool group as seen by a real MCP client:
//! `build_map_from_plan` compiles a two-room plan with a `door` and an
//! `opening` into one written block whose sector and geometry counts
//! match the reported bookkeeping (and whose structural checks stay
//! clean, though the door lands closed with no special and no player
//! start exists yet), the active map adoption, `already_exists` for a
//! working-archive block without `replace` and its overwrite, the
//! one-step `undo` that takes the whole block back, plan-path-named
//! refusals (a flat rectangle, both or neither footprint, unknown kind
//! and style names), the `units` scale-and-mirror and `grid` snap that
//! carry an image-space plan into map units before compiling (with
//! unusable `units.scale_den`/`grid` values refused), a `UDMF` block
//! with its declared namespace, and a one-call `theme` override that
//! never moves the session's theme.

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

    /// The structural checks a compiled plan must pass; the thing
    /// checks are excluded because geometry-only plans place no player
    /// start yet.
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
    /// joined by a door low on the wall and an opening high on it.
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
            ]
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
        assert_eq!(result["nodes"], json!(null), "reserved for the nodes slice");
        assert_eq!(
            result["validation"],
            json!(null),
            "reserved for the finalize slice"
        );
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
            "a door connector is closed and un-wired: no sector special yet"
        );
        assert_eq!(
            door["floor_height"], door["ceiling_height"],
            "the door sector is collapsed"
        );
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
            "units": {"scale_num": 4_i64, "flip_y": true}
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
            "grid": 16_i64
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
}
