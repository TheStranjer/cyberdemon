//! The `features` tool group as seen by a real MCP client: `add_door`
//! between two touching rooms (the void strip carved, the recess closed,
//! both room-facing lines dressed and their map still `validate_map`
//! clean), the locked-blue key door with its coloured track, the
//! `Hexen`-family script doors (`Door_Raise` on binary `Hexen`,
//! `Door_Open` on `UDMF` `zdoom`), the inert tagged `remote` door, the
//! one-step undo, and every listed refusal: an over-wide door naming the
//! available length, a one-sided wall with nothing behind it, the depth
//! probe, the both/neither wall parameter, bad style names and
//! out-of-range sector/linedef indices. Plus `add_switch`: a panel split
//! out of a plain wall wearing `SW1STRTN` and answering with the
//! target's tag, the fresh tag minted onto an untagged sector, the
//! whole-wall exit switch needing no split (`SW1EXIT`), the `Hexen`
//! `Door_Open`/`Portal` use lines, and the switch closing a `remote`
//! door's loop with exactly the tag `add_door` reported. And `add_exit`:
//! the level-ending switch on a named wall, the walk-over exit dressing
//! every two-sided boundary of a sector with 52/124 (`Doom`) or
//! `playercross` 243/244 (`Hexen`), the destination tag kept on the
//! `W1` line, and the missing-`linedef`/missing-`sector`/bad-`kind`
//! refusals; a fullbright `render_view` through the theme override
//! shows the built door's `DOOR1` face from the room it opens.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::codec::decode_image;
    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad_features, iwad_l_room, pwad_map_formats};
    use crate::support::{call, call_err, connect, http_get};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

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

    /// Open the map-format fixture (`UDMF` `MAP03`, binary `Hexen`
    /// `MAP04`, both the two-room map) as the base layer.
    async fn formats_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let extra = pwad_map_formats(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": extra.display().to_string()}),
        )
        .await;
        client
    }

    /// Open the single-room L fixture (no room behind any wall).
    async fn l_room_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_l_room(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// The working linedefs for `map`, in order.
    async fn working_linedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["linedefs"]}),
        )
        .await;
        report["linedefs"]
            .as_array()
            .expect("linedefs is an array")
            .clone()
    }

    /// The working sector row at `index` for `map`.
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

    /// The working linedef rows at the indices of an `add_door` result's
    /// `field` list.
    async fn reported_lines(client: &Client, map: &str, result: &Value, field: &str) -> Vec<Value> {
        let indices = index_list(result, field);
        let all = working_linedefs(client, map).await;
        let rows = indices
            .iter()
            .filter_map(|index| all.get(*index).cloned())
            .collect::<Vec<Value>>();
        assert_eq!(rows.len(), indices.len(), "every {field} index exists");
        rows
    }

    /// Requires `validate_map` to report no error- or warning-severity
    /// findings for the working `map`.
    async fn validate_clean(client: &Client, map: &str) {
        let report = call(
            client,
            "validate_map",
            json!({"map": map, "scope": "working"}),
        )
        .await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_i32),
            "no geometry errors: {}",
            report["issues"]
        );
        assert_eq!(
            report["counts"]["warning"],
            json!(0_i32),
            "no doubtful shapes: {}",
            report["issues"]
        );
    }

    /// Requires `add_door` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_with(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "add_door", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
    }

    /// Requires `add_switch` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_switch(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "add_switch", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
    }

    /// Requires `add_exit` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_exit(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "add_exit", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
    }

    /// The index of the one-sided working linedef running between the
    /// map points `(x1, y1)` and `(x2, y2)` in either direction.
    async fn solid_wall(client: &Client, map: &str, a: (i64, i64), b: (i64, i64)) -> usize {
        let rows = working_linedefs(client, map).await;
        let touches = |first: &Value, second: &Value| {
            first["x"] == json!(a.0)
                && first["y"] == json!(a.1)
                && second["x"] == json!(b.0)
                && second["y"] == json!(b.1)
        };
        let wanted = |row: &Value| -> bool {
            row["back"].is_null()
                && (touches(&row["v1"], &row["v2"]) || touches(&row["v2"], &row["v1"]))
        };
        rows.iter()
            .position(wanted)
            .unwrap_or_else(|| panic!("no one-sided {map} line from {a:?} to {b:?}"))
    }

    /// The indices of an `add_door` result list field.
    fn index_list(result: &Value, field: &str) -> Vec<usize> {
        result[field]
            .as_array()
            .expect("the index list is an array")
            .iter()
            .map(|value| {
                usize::try_from(value.as_u64().expect("indices are unsigned")).expect("fits")
            })
            .collect()
    }

    /// Whether one of the report's `notes` mentions `needle`.
    fn note_mentions(result: &Value, needle: &str) -> bool {
        result["notes"]
            .as_array()
            .expect("notes is an array")
            .iter()
            .filter_map(Value::as_str)
            .any(|note| note.contains(needle))
    }

    /// Checks one `Doom`-family use line: action `special`, line tag
    /// `tag`, the room-facing `upper` wearing `DOOR1` and the far upper
    /// clear; returns the sector the front faces.
    fn expect_doom_use_line(row: &Value, special: i32) -> Value {
        assert_eq!(row["special"], json!(special), "the door action");
        assert_eq!(row["tag"], json!(0_i32), "tag 0 acts behind the line");
        assert_eq!(
            row["front"]["upper"],
            json!("DOOR1"),
            "the room-facing upper wears the door"
        );
        assert_eq!(
            row["back"]["upper"],
            json!("-"),
            "the door side stays clear"
        );
        row["front"]["sector"].clone()
    }

    /// The sectors the `Doom`-family use lines face.
    fn expect_doom_use_lines(rows: &[Value], special: i32) -> Vec<Value> {
        rows.iter()
            .map(|row| expect_doom_use_line(row, special))
            .collect()
    }

    /// Checks the jamb rows: each one-sided, facing the door sector,
    /// wearing `track` in the middle.
    fn expect_track_lines(rows: &[Value], track: &str) {
        assert_eq!(rows.len(), 2, "a track wall on each side");
        for row in rows {
            assert_eq!(row["back"], Value::Null, "the jamb is one-sided");
            assert_eq!(row["front"]["middle"], json!(track), "the track");
            assert_eq!(row["front"]["sector"], json!(2_i32), "it faces the recess");
        }
    }

    /// Checks the inert `remote` rows: no action, pre-tagged `tag`.
    fn expect_inert_lines(rows: &[Value], tag: i32) {
        assert!(!rows.is_empty(), "the door lines exist");
        for row in rows {
            assert_eq!(
                row["special"],
                json!(0_i32),
                "no action on the door's lines"
            );
            assert_eq!(row["tag"], json!(tag), "pre-tagged for the future switch");
        }
    }

    /// Checks one `Hexen`-family `Door_Raise` line: special 12, the
    /// speed-16/delay-150 arguments and the use, repeat and monster
    /// activation bits.
    fn expect_door_raise(row: &Value) {
        assert_eq!(row["special"], json!(12_i32), "Door_Raise");
        assert_eq!(
            row["args"],
            json!([0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
            "speed 16, delay 150, the sector behind the line"
        );
        let names = row["flag_names"]
            .as_array()
            .expect("flag names is an array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<&str>>();
        for wanted in ["playeruse", "repeatspecial", "monsteractivate"] {
            assert!(names.contains(&wanted), "{wanted} activates it: {names:?}");
        }
    }

    /// Checks the `Hexen`-family `Door_Raise` rows.
    fn expect_door_raise_lines(rows: &[Value]) {
        assert_eq!(rows.len(), 2, "one use line per room");
        for row in rows {
            expect_door_raise(row);
        }
    }

    /// Checks the `Hexen`-family `Door_Open` (`stay_open`) rows.
    fn expect_door_open_lines(rows: &[Value]) {
        assert_eq!(rows.len(), 2, "one use line per room");
        for row in rows {
            assert_eq!(row["special"], json!(11_i32), "Door_Open");
            assert_eq!(row["args"], json!([0_i32, 16_i32, 0_i32, 0_i32, 0_i32]));
        }
    }

    #[tokio::test]
    async fn a_door_between_touching_rooms_opens_both_rooms() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["style"], json!("normal"), "the default style");
        assert_eq!(result["door_sector"], json!(2_i32), "a third sector");
        assert_eq!(result["tag"], Value::Null, "plain doors carry no tag");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the doorway is new geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert!(
            note_mentions(&result, "void strip"),
            "the strip carve is noted: {}",
            result["notes"]
        );
        let lines = reported_lines(&client, "MAP01", &result, "lines").await;
        assert_eq!(lines.len(), 2, "one use line per room");
        let fronts = expect_doom_use_lines(&lines, 1_i32);
        assert!(
            fronts.contains(&json!(0_i32)) && fronts.contains(&json!(1_i32)),
            "the fronts face their rooms: {fronts:?}"
        );
        let tracks = reported_lines(&client, "MAP01", &result, "jambs").await;
        expect_track_lines(&tracks, "DOORTRAK");
        let door = working_sector(&client, "MAP01", 2_usize).await;
        assert_eq!(
            door["ceiling_height"], door["floor_height"],
            "the door sector is closed"
        );
        assert_eq!(door["floor_height"], json!(0_i32), "room A's floor");
        assert_eq!(
            door["floor_flat"],
            json!("FLAT10"),
            "the theme's recess floor"
        );
        assert_eq!(door["tag"], json!(0_i32), "untagged");
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_locked_blue_door_wears_the_blue_track() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_door",
            json!({"between": [0_i32, 1_i32], "style": "locked_blue"}),
        )
        .await;
        assert_eq!(result["style"], json!("locked_blue"), "echoed canonically");
        let lines = reported_lines(&client, "MAP01", &result, "lines").await;
        expect_doom_use_lines(&lines, 26_i32);
        let tracks = reported_lines(&client, "MAP01", &result, "jambs").await;
        expect_track_lines(&tracks, "DOORBLU");
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_remote_door_is_tagged_and_inert() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_door",
            json!({"between": [0_i32, 1_i32], "style": "remote"}),
        )
        .await;
        assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
        let door = working_sector(&client, "MAP01", 2_usize).await;
        assert_eq!(door["tag"], json!(2_i32), "the sector answers with it");
        assert_eq!(
            door["ceiling_height"], door["floor_height"],
            "the recess is closed and waits for a switch"
        );
        let lines = reported_lines(&client, "MAP01", &result, "lines").await;
        expect_inert_lines(&lines, 2_i32);
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_wider_door_than_the_wall_names_the_available_length() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 1_i32], "width": 300_i32}),
            "256 map units long",
        )
        .await;
        let (code, _) = call_err(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(
            code, "not_found",
            "the refused door did not even copy the block into the working archive"
        );
    }

    #[tokio::test]
    async fn a_wall_with_nothing_behind_refuses_the_recess() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = l_room_loaded(dir.path()).await;
        refuse_with(
            &client,
            json!({"linedef": 0_i32}),
            "no room behind this wall within 256 units; move the rooms closer or give a \
             larger depth",
        )
        .await;
    }

    #[tokio::test]
    async fn an_explicit_depth_must_land_inside_a_sector() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = l_room_loaded(dir.path()).await;
        refuse_with(
            &client,
            json!({"linedef": 0_i32, "depth": 600_i32}),
            "no room behind this wall within 256 units",
        )
        .await;
    }

    #[tokio::test]
    async fn the_wall_parameter_needs_exactly_one_form() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_with(&client, json!({}), "exactly one").await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 1_i32], "linedef": 2_i32}),
            "exactly one",
        )
        .await;
    }

    #[tokio::test]
    async fn bad_indices_and_styles_are_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 99_i32]}),
            "sector 99 is out of range",
        )
        .await;
        refuse_with(
            &client,
            json!({"between": [99_i32, 0_i32]}),
            "sector 99 is out of range",
        )
        .await;
        refuse_with(
            &client,
            json!({"linedef": 999_i32}),
            "linedef 999 is out of range",
        )
        .await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 1_i32], "style": "sliding"}),
            "normal, fast, stay_open, locked_red, locked_blue, locked_yellow, remote",
        )
        .await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 1_i32], "texture": "TOOLONGTEXTURE"}),
            "characters",
        )
        .await;
    }

    #[tokio::test]
    async fn sectors_that_never_touch_share_no_door_wall() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_with(
            &client,
            json!({"between": [0_i32, 0_i32]}),
            "no two-sided boundary",
        )
        .await;
    }

    #[tokio::test]
    async fn a_hexen_map_gets_the_script_door() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_door",
            json!({"map": "MAP04", "between": [0_i32, 1_i32]}),
        )
        .await;
        assert_eq!(result["style"], json!("normal"), "the default style");
        let lines = reported_lines(&client, "MAP04", &result, "lines").await;
        expect_door_raise_lines(&lines);
        let door = working_sector(&client, "MAP04", 2_usize).await;
        assert_eq!(
            door["ceiling_height"], door["floor_height"],
            "the door is closed"
        );
    }

    /// Checks the `Hexen`-family `Door_LockedRaise` rows for key `key`.
    fn expect_locked_lines(rows: &[Value], key: i32) {
        assert_eq!(rows.len(), 2, "one use line per room");
        for row in rows {
            assert_eq!(row["special"], json!(13_i32), "Door_LockedRaise");
            assert_eq!(
                row["args"],
                json!([0_i32, 16_i32, 150_i32, key, 0_i32]),
                "the key class in args[3]"
            );
        }
    }

    #[tokio::test]
    async fn a_hexen_locked_door_demands_its_coloured_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_door",
            json!({"map": "MAP04", "between": [0_i32, 1_i32], "style": "locked_blue"}),
        )
        .await;
        assert_eq!(result["style"], json!("locked_blue"), "echoed canonically");
        let lines = reported_lines(&client, "MAP04", &result, "lines").await;
        expect_locked_lines(&lines, 130_i32);
    }

    #[tokio::test]
    async fn a_udmf_zdoom_stay_open_door_opens_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_door",
            json!({"map": "MAP03", "between": [0_i32, 1_i32], "style": "stay_open"}),
        )
        .await;
        assert_eq!(result["style"], json!("stay_open"), "echoed canonically");
        let lines = reported_lines(&client, "MAP03", &result, "lines").await;
        expect_door_open_lines(&lines);
    }

    #[tokio::test]
    async fn a_door_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["add normal door"]),
            "the whole door rolls back together"
        );
        let merged = call(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "merged", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(
            merged["sectors"]
                .as_array()
                .expect("sectors is an array")
                .len(),
            2_usize,
            "the layer was never written"
        );
    }

    /// The working linedef row at `index` for `map`.
    async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
        working_linedefs(client, map)
            .await
            .get(index)
            .cloned()
            .expect("the linedef row exists")
    }

    #[tokio::test]
    async fn a_switch_splits_the_wall_and_answers_with_the_targets_tag() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_switch",
            json!({
                "linedef": 0_i32,
                "action": "open_door",
                "target_sector": 1_i32,
                "width": 32_i32,
            }),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["action"], json!("open_door"), "echoed canonically");
        assert_eq!(result["linedef"], json!(7_i32), "the chunk is its own line");
        assert_eq!(result["split"], json!(true), "the long wall was cut");
        assert_eq!(result["special"], json!(103_i32), "the S1 door action");
        assert_eq!(result["tag"], json!(1_i32), "sector 1's own tag");
        assert_eq!(result["texture"], json!("SW1STRTN"), "the theme's switch");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the split is new geometry"
        );
        assert_eq!(result["copied_to_working"], json!(true));
        let chunk = working_linedef(&client, "MAP01", 7_usize).await;
        let ends = [
            (
                chunk["v1"]["x"].as_i64().expect("x"),
                chunk["v1"]["y"].as_i64().expect("y"),
            ),
            (
                chunk["v2"]["x"].as_i64().expect("x"),
                chunk["v2"]["y"].as_i64().expect("y"),
            ),
        ];
        assert!(
            ends.contains(&(0_i64, 112_i64)) && ends.contains(&(0_i64, 144_i64)),
            "the 32-unit chunk is centred on the wall's midpoint: {ends:?}"
        );
        assert_eq!(
            chunk["special"],
            json!(103_i32),
            "the chunk carries the action"
        );
        assert_eq!(chunk["tag"], json!(1_i32), "and the tag");
        assert_eq!(
            chunk["front"]["middle"],
            json!("SW1STRTN"),
            "the panel wears the switch"
        );
        let standing = working_linedef(&client, "MAP01", 0_usize).await;
        assert_eq!(
            standing["special"],
            json!(0_i32),
            "the outer piece is inert"
        );
        assert_eq!(standing["front"]["middle"], json!("WALL1"), "and plain");
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn an_untagged_target_sector_gets_a_fresh_tag() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_switch",
            json!({"linedef": 0_i32, "action": "lower_floor", "target_sector": 0_i32}),
        )
        .await;
        assert_eq!(result["action"], json!("lower_floor"));
        assert_eq!(result["special"], json!(23_i32), "the S1 floor action");
        assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
        let line = working_linedef(&client, "MAP01", 7_usize).await;
        assert_eq!(line["tag"], json!(2_i32), "the line answers with it");
        let target = working_sector(&client, "MAP01", 0_usize).await;
        assert_eq!(target["tag"], json!(2_i32), "and so does the sector now");
    }

    #[tokio::test]
    async fn an_exit_switch_on_the_whole_wall_needs_no_split_and_no_target() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_switch",
            json!({"linedef": 0_i32, "action": "exit", "width": 256_i32}),
        )
        .await;
        assert_eq!(result["linedef"], json!(0_i32), "the line is the chunk");
        assert_eq!(result["split"], json!(false), "nothing was cut");
        assert_eq!(result["special"], json!(11_i32), "the S1 exit");
        assert_eq!(result["tag"], json!(0_i32), "an exit answers with no tag");
        assert_eq!(
            result["texture"],
            json!("SW1EXIT"),
            "the theme's exit switch"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "a dressed wall is not new geometry"
        );
        let line = working_linedef(&client, "MAP01", 0_usize).await;
        assert_eq!(line["front"]["middle"], json!("SW1EXIT"));
    }

    #[tokio::test]
    async fn a_hexen_map_gets_the_script_switch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let door = call(
            &client,
            "add_switch",
            json!({
                "map": "MAP04",
                "linedef": 0_i32,
                "action": "open_door",
                "target_sector": 1_i32,
                "repeatable": true,
            }),
        )
        .await;
        assert_eq!(door["special"], json!(11_i32), "Door_Open");
        assert_eq!(
            door["tag"],
            json!(1_i32),
            "the sector's own tag is reported"
        );
        assert_eq!(door["linedef"], json!(7_i32), "the chunk is its own line");
        let door_line = working_linedef(&client, "MAP04", 7_usize).await;
        assert_eq!(
            door_line["args"],
            json!([1_i32, 16_i32, 0_i32, 0_i32, 0_i32]),
            "tag in args[0], speed 16"
        );
        assert_eq!(
            door_line["tag"],
            json!(0_i32),
            "a Hexen line carries no tag"
        );
        let door_flags = flag_names(&door_line);
        assert!(
            door_flags.contains(&"playeruse"),
            "it is a use line: {door_flags:?}"
        );
        assert!(door_flags.contains(&"repeatspecial"), "and it repeats");
    }

    #[tokio::test]
    async fn a_hexen_portal_switch_uses_once_and_carries_no_arguments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let exit = call(
            &client,
            "add_switch",
            json!({"map": "MAP04", "linedef": 3_i32, "action": "secret_exit"}),
        )
        .await;
        assert_eq!(exit["special"], json!(244_i32), "the secret Portal");
        assert_eq!(exit["tag"], json!(0_i32), "an exit acts on no tag");
        assert_eq!(exit["split"], json!(true), "the south wall was cut");
        let index = usize::try_from(exit["linedef"].as_u64().expect("an index")).expect("fits");
        let exit_line = working_linedef(&client, "MAP04", index).await;
        assert_eq!(
            exit_line["args"],
            json!([0_i32, 0_i32, 0_i32, 0_i32, 0_i32]),
            "a portal takes no arguments"
        );
        assert_eq!(exit_line["special"], json!(244_i32), "the chunk carries it");
        let exit_flags = flag_names(&exit_line);
        assert!(
            exit_flags.contains(&"playeruse"),
            "it is a use line: {exit_flags:?}"
        );
        assert!(
            !exit_flags.contains(&"repeatspecial"),
            "a level change fires once: {exit_flags:?}"
        );
    }

    /// The `flag_names` of a linedef row.
    fn flag_names(row: &Value) -> Vec<&str> {
        row["flag_names"]
            .as_array()
            .expect("flag names is an array")
            .iter()
            .filter_map(Value::as_str)
            .collect()
    }

    #[tokio::test]
    async fn switch_arguments_are_validated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_switch(
            &client,
            json!({"linedef": 0_i32, "action": "open_door"}),
            "target_sector",
        )
        .await;
        refuse_switch(
            &client,
            json!({"linedef": 0_i32, "action": "press"}),
            "open_door, exit, secret_exit, raise_floor, lower_floor",
        )
        .await;
        refuse_switch(
            &client,
            json!({"linedef": 999_i32, "action": "exit"}),
            "linedef 999 is out of range",
        )
        .await;
        refuse_switch(
            &client,
            json!({"linedef": 0_i32, "action": "raise_floor", "target_sector": 99_i32}),
            "sector 99 is out of range",
        )
        .await;
        refuse_switch(
            &client,
            json!({"linedef": 0_i32, "action": "open_door", "target_sector": 0_i32, "width": 0_i32}),
            "positive",
        )
        .await;
        let (code, _) = call_err(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(
            code, "not_found",
            "no refused switch even copied the block into the working archive"
        );
    }

    #[tokio::test]
    async fn a_switch_opens_the_remote_door_that_carries_its_tag() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let door = call(
            &client,
            "add_door",
            json!({"between": [0_i32, 1_i32], "style": "remote"}),
        )
        .await;
        let wall = solid_wall(&client, "MAP01", (0_i64, 0_i64), (0_i64, 256_i64)).await;
        let result = call(
            &client,
            "add_switch",
            json!({
                "linedef": wall,
                "action": "open_door",
                "target_sector": door["door_sector"],
            }),
        )
        .await;
        assert_eq!(
            result["tag"], door["tag"],
            "the switch answers with exactly the tag the remote door got"
        );
        assert_eq!(result["tag"], json!(2_i32), "the minted tag");
        assert_eq!(result["special"], json!(103_i32), "the S1 door action");
        let lines = working_linedefs(&client, "MAP01").await;
        let index = usize::try_from(result["linedef"].as_u64().expect("an index")).expect("fits");
        let line = lines.get(index).expect("the switch line");
        assert_eq!(line["tag"], json!(2_i32), "the line carries the tag");
        assert_eq!(line["special"], json!(103_i32));
        let door_sector = working_sector(&client, "MAP01", 2_usize).await;
        assert_eq!(door_sector["tag"], json!(2_i32), "the sector still answers");
        assert_eq!(
            door_sector["ceiling_height"], door_sector["floor_height"],
            "closed until a player uses the switch"
        );
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn an_exit_switch_mounts_on_the_wall_it_names() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_exit",
            json!({"kind": "switch", "linedef": 0_i32}),
        )
        .await;
        assert_eq!(result["kind"], json!("switch"), "the trigger echoes");
        assert_eq!(result["secret"], json!(false), "the normal exit");
        assert_eq!(
            result["lines"],
            json!([7_i32]),
            "the split chunk carries it"
        );
        assert_eq!(result["special"], json!(11_i32), "the S1 exit");
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the wall split is new geometry"
        );
        let line = working_linedef(&client, "MAP01", 7_usize).await;
        assert_eq!(line["special"], json!(11_i32), "the chunk carries the exit");
        assert_eq!(line["tag"], json!(0_i32), "a switch exit acts on no tag");
        assert_eq!(
            line["front"]["middle"],
            json!("SW1EXIT"),
            "the theme's exit panel"
        );
        validate_clean(&client, "MAP01").await;
        let second = features_loaded(dir.path()).await;
        let secret = call(
            &second,
            "add_exit",
            json!({"kind": "switch", "linedef": 0_i32, "secret": true}),
        )
        .await;
        assert_eq!(secret["special"], json!(51_i32), "the S1 secret exit");
        assert_eq!(secret["secret"], json!(true), "echoed back");
        assert_eq!(secret["lines"], json!([7_i32]), "the same wall chunk shape");
        validate_clean(&second, "MAP01").await;
    }

    #[tokio::test]
    async fn a_walk_over_exit_dresses_every_boundary_of_the_sector() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_exit",
            json!({"kind": "walk_over", "sector": 0_i32}),
        )
        .await;
        assert_eq!(result["kind"], json!("walk_over"), "the trigger echoes");
        assert_eq!(
            result["lines"],
            json!([2_i32]),
            "the sector's only two-sided line, not its one-sided walls"
        );
        assert_eq!(result["special"], json!(52_i32), "the W1 exit");
        assert_eq!(result["secret"], json!(false));
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "dressing lines is not new geometry"
        );
        assert_eq!(result["copied_to_working"], json!(true));
        let step = working_linedef(&client, "MAP01", 2_usize).await;
        assert_eq!(step["special"], json!(52_i32), "the boundary carries it");
        assert_eq!(step["tag"], json!(0_i32), "a plain line's tag stays plain");
        let from_behind = call(
            &client,
            "add_exit",
            json!({"kind": "walk_over", "sector": 1_i32}),
        )
        .await;
        assert_eq!(
            from_behind["lines"],
            json!([2_i32]),
            "sector 1 shares the line, so it counts from either side"
        );
        validate_clean(&client, "MAP01").await;
        let secret = call(
            &client,
            "add_exit",
            json!({"kind": "walk_over", "sector": 0_i32, "secret": true}),
        )
        .await;
        assert_eq!(secret["special"], json!(124_i32), "the W1 secret exit");
        assert_eq!(secret["secret"], json!(true), "echoed back");
        let line = working_linedef(&client, "MAP01", 2_usize).await;
        assert_eq!(line["special"], json!(124_i32), "the line now ends the map");
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_walk_over_exit_keeps_a_vanilla_destination_tag() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(
            &client,
            "edit_linedef",
            json!({"index": 2_i32, "tag": 3_i32}),
        )
        .await;
        let result = call(
            &client,
            "add_exit",
            json!({"kind": "walk_over", "sector": 0_i32}),
        )
        .await;
        assert_eq!(result["special"], json!(52_i32));
        let line = working_linedef(&client, "MAP01", 2_usize).await;
        assert_eq!(line["special"], json!(52_i32), "the W1 exit");
        assert_eq!(
            line["tag"],
            json!(3_i32),
            "vanilla 52 reads the line's tag as the destination map, so it survives"
        );
    }

    #[tokio::test]
    async fn a_hexen_walk_over_exit_crosses_the_boundary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_exit",
            json!({"map": "MAP04", "kind": "walk_over", "sector": 0_i32}),
        )
        .await;
        assert_eq!(result["special"], json!(243_i32), "the Portal exit line");
        assert_eq!(result["lines"], json!([2_i32]), "the shared boundary");
        assert_eq!(result["secret"], json!(false));
        let line = working_linedef(&client, "MAP04", 2_usize).await;
        assert_eq!(line["special"], json!(243_i32));
        let names = flag_names(&line);
        assert!(
            names.contains(&"playercross"),
            "the activation field reads as a cross: {names:?}"
        );
        assert!(!names.contains(&"playeruse"), "not a use line: {names:?}");
        let secret = call(
            &client,
            "add_exit",
            json!({"map": "MAP04", "kind": "walk_over", "sector": 1_i32, "secret": true}),
        )
        .await;
        assert_eq!(secret["special"], json!(244_i32), "the secret Portal");
        assert_eq!(secret["secret"], json!(true), "echoed back");
        let again = working_linedef(&client, "MAP04", 2_usize).await;
        assert_eq!(
            again["special"],
            json!(244_i32),
            "the same line, from room 1"
        );
        assert!(flag_names(&again).contains(&"playercross"), "still cross");
    }

    #[tokio::test]
    async fn exit_arguments_are_validated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_exit(&client, json!({"kind": "switch"}), "linedef").await;
        refuse_exit(&client, json!({"kind": "walk_over"}), "sector").await;
        refuse_exit(
            &client,
            json!({"kind": "stairs", "linedef": 0_i32}),
            "is not an exit kind",
        )
        .await;
        refuse_exit(
            &client,
            json!({"kind": "walk_over", "sector": 99_i32}),
            "sector 99 is out of range",
        )
        .await;
        refuse_exit(
            &client,
            json!({"kind": "switch", "linedef": 999_i32}),
            "linedef 999 is out of range",
        )
        .await;
        let (code, _) = call_err(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(
            code, "not_found",
            "no refused exit even copied the block into the working archive"
        );
        let lonely = l_room_loaded(dir.path()).await;
        refuse_exit(
            &lonely,
            json!({"kind": "walk_over", "sector": 0_i32}),
            "no two-sided boundary",
        )
        .await;
    }

    /// Fetch and decode a render report's PNG asset.
    async fn rendered_image(report: &Value) -> RgbaImage {
        let url = report["asset"]["url"].as_str().expect("an asset url");
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        decode_image(&body).expect("the view is a readable PNG")
    }

    /// The number of pixels of `image` that are exactly `color`.
    fn count_pixels(image: &RgbaImage, color: [u8; 4]) -> usize {
        (0..image.height)
            .flat_map(|row| (0..image.width).map(move |col| (col, row)))
            .filter(|&(col, row)| image.get(col, row) == Some(color))
            .count()
    }

    #[tokio::test]
    async fn a_themed_door_face_shows_from_the_room_it_opens() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let theme = call(
            &client,
            "set_texture_theme",
            json!({"overrides": {"door": "DOOR1"}}),
        )
        .await;
        assert_eq!(
            theme["roles"]["door"]["texture"],
            json!("DOOR1"),
            "the override stands for the role"
        );
        call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
        let report = call(
            &client,
            "render_view",
            json!({
                "map": "MAP01", "x": 64_i64, "y": 128_i64, "angle": 0_i64,
                "width": 160_u64, "height": 100_u64, "lighting": "fullbright",
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        assert!(
            count_pixels(&image, [178_u8, 77, 178, 255]) > 0,
            "the closed door's DOOR1 face (the fixture patch's index 170, one fake-contrast \
             step darker) shows from room 0 across the doorway"
        );
    }
}
