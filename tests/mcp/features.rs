//! The `features` tool group as seen by a real MCP client: `add_door`
//! between two touching rooms (the void strip carved, the recess closed,
//! both room-facing lines dressed and their map still `validate_map`
//! clean), the locked-blue key door with its coloured track, the
//! `Hexen`-family script doors (`Door_Raise` on binary `Hexen`,
//! `Door_Open` on `UDMF` `zdoom`), the inert tagged `remote` door, the
//! one-step undo, and every listed refusal: an over-wide door naming the
//! available length, a one-sided wall with nothing behind it, the depth
//! probe, the both/neither wall parameter, bad style names and
//! out-of-range sector/linedef indices. `add_window` raises a blocking
//! glass strip between two rooms: its floor and ceiling sit on the
//! sill/lintel trims, every room-facing line is two-sided *and* blocking
//! with the sill below and the wall above, and a too-small opening or the
//! both/neither wall parameter refuses with the whole edit uncommitted.
//! Plus `add_switch`: a panel split
//! out of a plain wall wearing `SW1STRTN` and answering with the
//! target's tag, the fresh tag minted onto an untagged sector, the
//! whole-wall exit switch needing no split (`SW1EXIT`), the `Hexen`
//! `Door_Open`/`Portal` use lines, and the switch closing a `remote`
//! door's loop with exactly the tag `add_door` reported. And `add_exit`:
//! the level-ending switch on a named wall, the walk-over exit dressing
//! every two-sided boundary of a sector with 52/124 (`Doom`) or
//! `playercross` 243/244 (`Hexen`), the destination tag kept on the
//! `W1` line, and the missing-`linedef`/missing-`sector`/bad-`kind`
//! refusals. Then the two sector-special tools: `mark_secret` writing
//! the vanilla `9`, the `Boom` bit 128 (with its warning over a vanilla
//! type) or the `Hexen` bit 1024 and clearing each again, and
//! `set_light_effect` landing every effect in its family's basic-type
//! field with the bits above it intact, the optional light level riding
//! along, and the bad-effect/bad-level/out-of-range-sector refusals. A
//! fullbright `render_view` through the theme override shows the built
//! door's `DOOR1` face from the room it opens. And `add_stairs`: four
//! evenly rising step sectors climbing from the hall floor to a raised
//! room at 64 (`validate_map` stays clean, one `undo` rolls the whole
//! staircase back, risers in the theme's `step` texture), and the
//! headroom, `step_depth`, `step_height`, endpoint and width refusals.

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
        working_sectors(client, map)
            .await
            .get(index)
            .cloned()
            .expect("the sector row exists")
    }

    /// The working sector rows for `map`, in order.
    async fn working_sectors(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["sectors"]}),
        )
        .await;
        report["sectors"]
            .as_array()
            .expect("sectors is an array")
            .clone()
    }

    /// Checks that both rooms of the two-room fixture carry `light` and
    /// `special`: one call reaches every sector the list named.
    fn expect_rooms(rooms: &[Value], light: i32, special: i32) {
        assert_eq!(rooms.len(), 2_usize, "the fixture's two rooms");
        for (index, row) in rooms.iter().enumerate() {
            assert_eq!(
                row["light"],
                json!(light),
                "sector {index} is lit at {light}"
            );
            assert_eq!(
                row["special"],
                json!(special),
                "sector {index} carries special {special}"
            );
        }
    }

    /// Gives sector `index` of `map` the special `value`, so a special tool
    /// has something to keep (or warn) about.
    async fn set_special(client: &Client, map: &str, index: usize, value: i32) {
        call(
            client,
            "edit_sector",
            json!({"map": map, "index": index, "special": value}),
        )
        .await;
    }

    /// Requires `mark_secret` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_secret(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "mark_secret", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
    }

    /// Requires `set_light_effect` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_light(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "set_light_effect", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
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

    /// Requires `validate_map` to report no unexpected error- or
    /// warning-severity findings for the working `map`. Every placement
    /// rewrites the map block without node data (its result says so with
    /// `nodes_invalidated`), so `nodes_missing` is the expected noise this
    /// helper filters out, exactly as `edit_sectors` does.
    async fn validate_clean(client: &Client, map: &str) {
        const EXPECTED: [&str; 1] = ["nodes_missing"];
        let report = call(
            client,
            "validate_map",
            json!({"map": map, "scope": "working"}),
        )
        .await;
        let unexpected: Vec<String> = report["issues"]
            .as_array()
            .expect("an issue list")
            .iter()
            .filter(|issue| {
                let severity = issue["severity"].as_str().expect("a severity");
                let code = issue["code"].as_str().expect("a code");
                (severity == "error" || severity == "warning") && !EXPECTED.contains(&code)
            })
            .map(|issue| issue["message"].as_str().expect("a message").to_owned())
            .collect();
        assert!(
            unexpected.is_empty(),
            "no findings beyond the invalidated node lumps: {unexpected:#?}"
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

    /// Requires `add_window` to reject `args` with `invalid_argument`
    /// naming `needle`.
    async fn refuse_window(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "add_window", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
    }

    /// Checks one `add_window` result's view-line row: two-sided and
    /// blocking, the sill `lower` and the wall `upper` on its room side,
    /// and the strip side left open; returns the room's sector.
    fn window_view_room(row: &Value) -> Value {
        assert_ne!(row["back"], Value::Null, "the view line is two-sided");
        let names = flag_names(row);
        assert!(
            names.contains(&"twosided"),
            "the glass sees both ways: {names:?}"
        );
        assert!(
            names.contains(&"blocking"),
            "blocking is set so nobody walks through: {names:?}"
        );
        let (strip, room) = if row["front"]["sector"] == json!(2_i32) {
            (&row["front"], &row["back"])
        } else {
            (&row["back"], &row["front"])
        };
        assert_eq!(room["lower"], json!("STEP1"), "the sill under the glass");
        assert_eq!(room["upper"], json!("WALL1"), "the wall over it");
        assert_eq!(
            strip["upper"],
            json!("-"),
            "from inside the strip the line is open"
        );
        assert_eq!(strip["lower"], json!("-"), "above and below");
        room["sector"].clone()
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

    #[tokio::test]
    async fn a_window_between_touching_rooms_raises_a_blocking_view() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let theme = call(
            &client,
            "set_texture_theme",
            json!({"overrides": {"window_sill": "STEP1", "wall": "WALL1"}}),
        )
        .await;
        assert_eq!(
            theme["roles"]["window_sill"]["texture"],
            json!("STEP1"),
            "the sill override stands for the role"
        );
        let result = call(&client, "add_window", json!({"between": [0_i32, 1_i32]})).await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["window_sector"], json!(2_i32), "a third sector");
        assert_eq!(result["width"], json!(64_i32), "the default width");
        assert_eq!(result["sill"], json!(32_i32), "the default sill");
        assert_eq!(result["lintel"], json!(24_i32), "the default lintel");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the strip is new geometry"
        );
        assert_eq!(result["copied_to_working"], json!(true));
        assert!(
            note_mentions(&result, "void strip"),
            "the strip carve is noted: {}",
            result["notes"]
        );
        let window = working_sector(&client, "MAP01", 2_usize).await;
        assert_eq!(window["floor_height"], json!(64_i32), "max(0, 32) + 32");
        assert_eq!(
            window["ceiling_height"],
            json!(104_i32),
            "min(128, 128) - 24"
        );
        assert_eq!(window["special"], json!(0_i32), "inert");
        assert_eq!(window["tag"], json!(0_i32), "untagged");
        let lines = reported_lines(&client, "MAP01", &result, "lines").await;
        assert_eq!(lines.len(), 2, "one view line per room");
        let rooms: Vec<Value> = lines.iter().map(window_view_room).collect();
        assert!(
            rooms.contains(&json!(0_i32)) && rooms.contains(&json!(1_i32)),
            "both rooms see the glass: {rooms:?}"
        );
        validate_clean(&client, "MAP01").await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["add window"]),
            "the whole window rolls back together"
        );
    }

    #[tokio::test]
    async fn a_window_with_too_small_an_opening_refuses() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_window(
            &client,
            json!({"between": [0_i32, 1_i32], "sill": 90_i32, "lintel": 24_i32}),
            "at least 16",
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
            "the refused window did not even copy the block into the working archive"
        );
    }

    #[tokio::test]
    async fn the_window_wall_parameter_needs_exactly_one_form() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_window(&client, json!({}), "exactly one").await;
        refuse_window(
            &client,
            json!({"between": [0_i32, 1_i32], "linedef": 2_i32}),
            "exactly one",
        )
        .await;
        refuse_window(
            &client,
            json!({"between": [0_i32, 99_i32]}),
            "sector 99 is out of range",
        )
        .await;
        refuse_window(
            &client,
            json!({"linedef": 999_i32}),
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
            "no refused window even copied the block into the working archive"
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

    #[tokio::test]
    async fn a_secret_mark_follows_the_doom_flag_family() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let marked = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
        assert_eq!(marked["map"], json!("MAP01"), "the only map is default");
        assert_eq!(marked["secret"], json!(true), "marking is the default");
        assert_eq!(marked["family"], json!("doom"), "the vanilla family secret");
        assert_eq!(marked["sectors"], json!([0_i32]), "one row moved");
        assert_eq!(
            marked["warnings"],
            json!([]),
            "the vanilla type needs no note"
        );
        assert_eq!(
            marked["nodes_invalidated"],
            json!(false),
            "a sector property is not geometry"
        );
        assert_eq!(marked["copied_to_working"], json!(true));
        assert_eq!(
            working_sector(&client, "MAP01", 0_usize).await["special"],
            json!(9_i32),
            "the vanilla secret type"
        );
        let again = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
        assert_eq!(
            again["sectors"],
            json!([]),
            "already secret: nothing changed"
        );
        let plain = call(
            &client,
            "mark_secret",
            json!({"sectors": [0_i32], "secret": false}),
        )
        .await;
        assert_eq!(plain["secret"], json!(false), "the clear is echoed");
        assert_eq!(
            plain["sectors"],
            json!([0_i32]),
            "and it moved the row back"
        );
        assert_eq!(
            working_sector(&client, "MAP01", 0_usize).await["special"],
            json!(0_i32),
            "the vanilla 9 clears to plain"
        );
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_boom_secret_mark_keeps_every_other_bit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        set_special(&client, "MAP01", 0_usize, 32_i32).await;
        let boom = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
        assert_eq!(boom["sectors"], json!([0_i32]), "the Boom bit landed");
        assert_eq!(
            boom["warnings"],
            json!([]),
            "a generalized type is Boom's own"
        );
        assert_eq!(
            working_sector(&client, "MAP01", 0_usize).await["special"],
            json!(160_i32),
            "32 | 128, the generalized field intact"
        );
        let unmarked = call(
            &client,
            "mark_secret",
            json!({"sectors": [0_i32], "secret": false}),
        )
        .await;
        assert_eq!(unmarked["sectors"], json!([0_i32]));
        assert_eq!(
            working_sector(&client, "MAP01", 0_usize).await["special"],
            json!(32_i32),
            "exactly the flag came back off"
        );
    }

    #[tokio::test]
    async fn a_vanilla_secret_mark_carries_a_boom_warning() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        set_special(&client, "MAP01", 1_usize, 17_i32).await;
        let warned = call(&client, "mark_secret", json!({"sectors": [1_i32]})).await;
        assert_eq!(
            warned["sectors"],
            json!([1_i32]),
            "the vanilla type took the bit too"
        );
        let notes = warned["warnings"].as_array().expect("warnings is an array");
        assert_eq!(notes.len(), 1_usize, "one note: {notes:#?}");
        let note = notes.first().and_then(Value::as_str).expect("a string");
        assert!(
            note.contains("vanilla Doom ignores Boom bits"),
            "the note says what vanilla Doom does not read: {note}"
        );
        assert!(note.contains("sector 1"), "and names the sector: {note}");
        assert_eq!(
            working_sector(&client, "MAP01", 1_usize).await["special"],
            json!(145_i32),
            "17 | 128"
        );
        let back = call(
            &client,
            "mark_secret",
            json!({"sectors": [1_i32], "secret": false}),
        )
        .await;
        assert_eq!(
            back["warnings"],
            json!([]),
            "clearing has nothing to warn about"
        );
        assert_eq!(
            working_sector(&client, "MAP01", 1_usize).await["special"],
            json!(17_i32),
            "the flicker is the plain number again"
        );
    }

    #[tokio::test]
    async fn a_secret_mark_follows_the_hexen_flag_family() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        set_special(&client, "MAP04", 0_usize, 81_i32).await;
        let marked = call(
            &client,
            "mark_secret",
            json!({"map": "MAP04", "sectors": [0_i32, 1_i32]}),
        )
        .await;
        assert_eq!(
            marked["family"],
            json!("hexen"),
            "binary Hexen is its own family"
        );
        assert_eq!(marked["warnings"], json!([]), "the Hexen bit never warns");
        assert_eq!(marked["sectors"], json!([0_i32, 1_i32]), "both rows moved");
        let rooms = working_sectors(&client, "MAP04").await;
        assert_eq!(
            rooms.first().expect("sector 0")["special"],
            json!(1_105_i32),
            "81 | 1024 keeps the sector's own effect"
        );
        assert_eq!(
            rooms.get(1).expect("sector 1")["special"],
            json!(1_024_i32),
            "a plain sector takes the bare bit"
        );
        let cleared = call(
            &client,
            "mark_secret",
            json!({"map": "MAP04", "sectors": [0_i32, 1_i32], "secret": false}),
        )
        .await;
        assert_eq!(cleared["sectors"], json!([0_i32, 1_i32]), "both cleared");
        let back = working_sectors(&client, "MAP04").await;
        assert_eq!(back.first().expect("sector 0")["special"], json!(81_i32));
        assert_eq!(back.get(1).expect("sector 1")["special"], json!(0_i32));
        let udmf = call(
            &client,
            "mark_secret",
            json!({"map": "MAP03", "sectors": [1_i32]}),
        )
        .await;
        assert_eq!(
            udmf["family"],
            json!("hexen"),
            "a zdoom UDMF map is Hexen-family too"
        );
        assert_eq!(
            working_sector(&client, "MAP03", 1_usize).await["special"],
            json!(1_024_i32),
            "a plain sector of a zdoom map takes the bare bit, its tag untouched"
        );
        assert_eq!(
            working_sector(&client, "MAP03", 1_usize).await["tag"],
            json!(1_i32),
            "the tool writes the special column only"
        );
    }

    /// One `set_light_effect` case: the map to edit, the family it reports,
    /// the effect to send, the basic type that family stores and the whole
    /// special the sector must end with.
    struct EffectCase {
        /// The map to edit.
        map: &'static str,
        /// The flag family the tool reports.
        family: &'static str,
        /// The effect name to send.
        effect: &'static str,
        /// The basic type this family stores for it.
        basic: i32,
        /// The whole special after the effect lands: every bit outside the
        /// family's type field kept.
        stored: i32,
    }

    /// Runs one [`EffectCase`] against sector 0 of its map, which starts at
    /// `from`: a bit above each family's basic-type field plus a vanilla
    /// type inside it, so a correct write shows on both sides of the edge.
    async fn expect_effect(client: &Client, case: &EffectCase, from: i32) {
        set_special(client, case.map, 0_usize, from).await;
        let result = call(
            client,
            "set_light_effect",
            json!({"map": case.map, "sectors": [0_i32], "effect": case.effect}),
        )
        .await;
        let name = case.effect;
        assert_eq!(result["effect"], json!(name), "{name} echoes canonically");
        assert_eq!(
            result["family"],
            json!(case.family),
            "{name} reads its family"
        );
        assert_eq!(
            result["special"],
            json!(case.basic),
            "{name} is the family's type"
        );
        assert_eq!(result["light"], Value::Null, "{name} asked for no level");
        assert_eq!(result["sectors"], json!([0_i32]), "{name} moved one row");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "{name} is a property edit"
        );
        let row = working_sector(client, case.map, 0_usize).await;
        assert_eq!(
            row["special"],
            json!(case.stored),
            "{name} wrote only the type field of {from}"
        );
        assert_eq!(row["light"], json!(160_i32), "{name} left the level alone");
    }

    /// Runs every case of a family's table against its map's sector 0,
    /// resetting that sector to `from` before each call.
    async fn expect_effects(client: &Client, cases: &[EffectCase], from: i32) {
        for case in cases {
            expect_effect(client, case, from).await;
        }
    }

    /// The `Doom` family's table: each effect's vanilla type under the
    /// generalized bit (bit 12) the sector carried all along.
    fn doom_cases() -> [EffectCase; 6] {
        let rows = [
            ("flicker", 17_i32, 4_113_i32),
            ("strobe_fast", 2_i32, 4_098_i32),
            ("strobe_slow", 3_i32, 4_099_i32),
            ("glow", 8_i32, 4_104_i32),
            ("random_off", 1_i32, 4_097_i32),
            ("none", 0_i32, 4_096_i32),
        ];
        rows.map(|(effect, basic, stored)| EffectCase {
            map: "MAP01",
            family: "doom",
            effect,
            basic,
            stored,
        })
    }

    /// The `Hexen` family's table: the same effects `64` higher, under the
    /// flag at 256 and the secret bit at 1024 the sector carried.
    fn hexen_cases() -> [EffectCase; 6] {
        let rows = [
            ("flicker", 81_i32, 1_361_i32),
            ("strobe_fast", 66_i32, 1_346_i32),
            ("strobe_slow", 67_i32, 1_347_i32),
            ("glow", 72_i32, 1_352_i32),
            ("random_off", 65_i32, 1_345_i32),
            ("none", 0_i32, 1_280_i32),
        ];
        rows.map(|(effect, basic, stored)| EffectCase {
            map: "MAP04",
            family: "hexen",
            effect,
            basic,
            stored,
        })
    }

    #[tokio::test]
    async fn every_light_effect_lands_in_the_doom_type_field() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        expect_effects(&client, &doom_cases(), 4_105_i32).await;
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn every_light_effect_lands_in_the_hexen_type_field() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        expect_effects(&client, &hexen_cases(), 1_289_i32).await;
    }

    #[tokio::test]
    async fn a_light_effect_keeps_a_hexen_sector_secret() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = formats_loaded(dir.path()).await;
        call(
            &client,
            "mark_secret",
            json!({"map": "MAP04", "sectors": [0_i32]}),
        )
        .await;
        call(
            &client,
            "set_light_effect",
            json!({"map": "MAP04", "sectors": [0_i32], "effect": "flicker"}),
        )
        .await;
        assert_eq!(
            working_sector(&client, "MAP04", 0_usize).await["special"],
            json!(1_105_i32),
            "1024 | 81: the light repaint left the secret bit at 1024 alone"
        );
        let zdoom = call(
            &client,
            "set_light_effect",
            json!({"map": "MAP03", "sectors": [1_i32], "effect": "strobe_slow", "light": 64_i32}),
        )
        .await;
        assert_eq!(
            zdoom["family"],
            json!("hexen"),
            "a zdoom UDMF map takes the Hexen numbers too"
        );
        assert_eq!(zdoom["special"], json!(67_i32), "3 plus 64");
        assert_eq!(zdoom["sectors"], json!([1_i32]), "the row moved");
        let room = working_sector(&client, "MAP03", 1_usize).await;
        assert_eq!(room["special"], json!(67_i32), "the stored number");
        assert_eq!(room["light"], json!(64_i32), "and the level rode along");
    }

    #[tokio::test]
    async fn a_light_level_travels_with_the_effect() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        let result = call(
            &client,
            "set_light_effect",
            json!({"sectors": [0_i32, 1_i32], "effect": "glow", "light": 96_i32}),
        )
        .await;
        assert_eq!(result["light"], json!(96_i32), "the level the call applied");
        assert_eq!(result["sectors"], json!([0_i32, 1_i32]), "both rooms");
        let rooms = working_sectors(&client, "MAP01").await;
        expect_rooms(&rooms, 96_i32, 8_i32);
        let again = call(
            &client,
            "set_light_effect",
            json!({"sectors": [0_i32, 1_i32], "effect": "glow", "light": 96_i32}),
        )
        .await;
        assert_eq!(
            again["sectors"],
            json!([]),
            "the same request changed nothing"
        );
        let level = call(
            &client,
            "set_light_effect",
            json!({"sectors": [1_i32], "effect": "glow", "light": 128_i32}),
        )
        .await;
        assert_eq!(
            level["sectors"],
            json!([1_i32]),
            "a level alone counts as a change, the effect already standing"
        );
        assert_eq!(
            working_sector(&client, "MAP01", 1_usize).await["light"],
            json!(128_i32)
        );
    }

    #[tokio::test]
    async fn a_secret_mark_and_a_light_effect_undo_as_one_step_each() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
        call(
            &client,
            "set_light_effect",
            json!({"sectors": [1_i32], "effect": "flicker"}),
        )
        .await;
        let undone = call(&client, "undo", json!({"steps": 2_usize})).await;
        assert_eq!(
            undone["undone"],
            json!(["set light effect", "mark secret"]),
            "each call is its own undo entry, rolled back newest first"
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
                .iter()
                .map(|row| row["special"].clone())
                .collect::<Vec<Value>>(),
            vec![json!(0_i32), json!(0_i32)],
            "the layer was never written"
        );
    }

    #[tokio::test]
    async fn the_two_special_tools_validate_their_arguments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_secret(&client, json!({"sectors": []}), "sectors is empty").await;
        refuse_secret(
            &client,
            json!({"sectors": [99_i32]}),
            "sector 99 is out of range",
        )
        .await;
        refuse_light(
            &client,
            json!({"sectors": [], "effect": "glow"}),
            "sectors is empty",
        )
        .await;
        refuse_light(
            &client,
            json!({"sectors": [0_i32], "effect": "pulsate"}),
            "none, flicker, strobe_fast, strobe_slow, glow, random_off",
        )
        .await;
        refuse_light(
            &client,
            json!({"sectors": [0_i32], "effect": "glow", "light": 256_i32}),
            "light 256 is out of range",
        )
        .await;
        refuse_light(
            &client,
            json!({"sectors": [0_i32], "effect": "glow", "light": -1_i32}),
            "light -1 is out of range",
        )
        .await;
        refuse_light(
            &client,
            json!({"sectors": [42_i32], "effect": "glow"}),
            "sector 42 is out of range",
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
            "no refused special even copied the block into the working archive"
        );
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

    /// Requires `add_stairs` to reject `args` with `invalid_argument`
    /// naming `needle`, and the working archive to hold nothing.
    async fn refuse_stairs(client: &Client, args: Value, needle: &str) {
        let note = args.to_string();
        let (code, message) = call_err(client, "add_stairs", args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {note}");
        assert!(
            message.contains(needle),
            "the refusal for {note} must mention {needle:?}: {message}"
        );
        let (missing, _) = call_err(
            client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(
            missing, "not_found",
            "the refused staircase did not even copy the block into the working archive"
        );
    }

    /// Raises the fixture's rooms so a 64-unit climb has headroom:
    /// both ceilings to 160 and room 1's floor to 64.
    async fn raise_the_fixture(client: &Client) {
        call(
            client,
            "edit_sector",
            json!({"index": 0_i32, "ceiling_height": 160_i32}),
        )
        .await;
        call(
            client,
            "edit_sector",
            json!({"index": 1_i32, "floor_height": 64_i32, "ceiling_height": 160_i32}),
        )
        .await;
    }

    /// Checks the four created step rows (sectors 2 to 5): their even
    /// floors, the probed 160 ceiling, and that each stays inert.
    fn expect_step_sectors(rows: &[Value]) {
        let wanted = [
            (2_usize, 16_i32),
            (3_usize, 32_i32),
            (4_usize, 48_i32),
            (5_usize, 64_i32),
        ];
        for (index, floor) in wanted {
            let step = rows.get(index).expect("the step sector row exists");
            assert_eq!(step["floor_height"], json!(floor), "step sector {index}");
            assert_eq!(step["ceiling_height"], json!(160_i32), "the probed ceiling");
            assert_eq!(step["special"], json!(0_i32), "inert");
            assert_eq!(step["tag"], json!(0_i32), "untagged");
        }
    }

    /// Checks the four riser lines across the staircase (at `x` 64, 160,
    /// 256 and 352): each two-sided, and its `lower` wears the theme's
    /// `STEP1` on at least one face.
    fn expect_stairs_risers(rows: &[Value]) {
        let risers = [64_i64, 160_i64, 256_i64, 352_i64]
            .into_iter()
            .map(|x| {
                (
                    x,
                    find_riser(rows, x).expect("a two-sided riser line at that x"),
                )
            })
            .collect::<Vec<(i64, &Value)>>();
        assert_eq!(risers.len(), 4_usize, "four risers across the path");
        for (x, riser) in risers {
            assert!(
                riser["front"]["lower"] == json!("STEP1")
                    || riser["back"]["lower"] == json!("STEP1"),
                "the riser at x {x} wears the theme's step: {riser}"
            );
        }
    }

    /// The two-sided row in `rows` running north-south across the
    /// staircase at `x`, from `y 96` to `y 160`: one step's riser.
    fn find_riser(rows: &[Value], x: i64) -> Option<&Value> {
        let vertical = |first: &Value, second: &Value| {
            first["x"] == json!(x)
                && second["x"] == json!(x)
                && ((first["y"] == json!(96_i64) && second["y"] == json!(160_i64))
                    || (first["y"] == json!(160_i64) && second["y"] == json!(96_i64)))
        };
        let wanted = |row: &Value| -> bool {
            !row["back"].is_null()
                && (vertical(&row["v1"], &row["v2"]) || vertical(&row["v2"], &row["v1"]))
        };
        rows.iter().find(|row| wanted(row))
    }

    #[tokio::test]
    async fn stairs_climb_from_the_hall_to_a_raised_room() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        raise_the_fixture(&client).await;
        let result = call(
            &client,
            "add_stairs",
            json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["floor_from"], json!(0_i32), "the hall's floor");
        assert_eq!(result["floor_to"], json!(64_i32), "the raised room's floor");
        assert_eq!(result["step_height"], json!(16_i32), "the default climb");
        assert_eq!(result["width"], json!(64_i32), "the default width");
        assert_eq!(result["count"], json!(4_i32), "four 16-unit steps");
        assert_eq!(
            result["steps"],
            json!([
                {"sector": 2_i32, "floor_height": 16_i32, "ceiling_height": 160_i32},
                {"sector": 3_i32, "floor_height": 32_i32, "ceiling_height": 160_i32},
                {"sector": 4_i32, "floor_height": 48_i32, "ceiling_height": 160_i32},
                {"sector": 5_i32, "floor_height": 64_i32, "ceiling_height": 160_i32},
            ]),
            "the created sectors and their floors, in path order"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the staircase is new geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(false),
            "the sector edits above had already copied the block in"
        );
        let sectors = working_sectors(&client, "MAP01").await;
        expect_step_sectors(&sectors);
        expect_stairs_risers(&working_linedefs(&client, "MAP01").await);
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn a_staircase_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        call(
            &client,
            "add_stairs",
            json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
        )
        .await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["add stairs"]),
            "the whole staircase rolls back together"
        );
        let (code, _) = call_err(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
        )
        .await;
        assert_eq!(code, "not_found", "and the copy went with it");
    }

    #[tokio::test]
    async fn stairs_refuse_a_step_without_headroom() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "floor_to": 64_i32,
            }),
            "not enough headroom",
        )
        .await;
    }

    #[tokio::test]
    async fn stairs_step_depth_must_match_the_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "step_depth": 24_i32,
            }),
            "wants a path 48 map units",
        )
        .await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "step_depth": 0_i32,
            }),
            "step_depth must be a positive",
        )
        .await;
    }

    #[tokio::test]
    async fn stairs_step_height_stays_between_one_and_twenty_four() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "step_height": 25_i32,
            }),
            "is out of range",
        )
        .await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "step_height": 0_i32,
            }),
            "step_height 0 is out of range",
        )
        .await;
    }

    #[tokio::test]
    async fn stairs_endpoints_and_width_are_checked() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = features_loaded(dir.path()).await;
        refuse_stairs(
            &client,
            json!({"from": {"x": 5000_i64, "y": 5000_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
            "from point 5000, 5000 lies in no sector",
        )
        .await;
        refuse_stairs(
            &client,
            json!({
                "from": {"x": 64_i64, "y": 128_i64},
                "to": {"x": 448_i64, "y": 128_i64},
                "width": 0_i32,
            }),
            "width must be a positive",
        )
        .await;
        refuse_stairs(
            &client,
            json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 64_i64, "y": 128_i64}}),
            "zero-length",
        )
        .await;
    }
}
