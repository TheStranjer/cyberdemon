//! The `features` tool group as seen by a real MCP client: `add_door`
//! between two touching rooms (the void strip carved, the recess closed,
//! both room-facing lines dressed and their map still `validate_map`
//! clean), the locked-blue key door with its coloured track, the
//! `Hexen`-family script doors (`Door_Raise` on binary `Hexen`,
//! `Door_Open` on `UDMF` `zdoom`), the inert tagged `remote` door, the
//! one-step undo, and every listed refusal: an over-wide door naming the
//! available length, a one-sided wall with nothing behind it, the depth
//! probe, the both/neither wall parameter, bad style names and
//! out-of-range sector/linedef indices.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad_features, iwad_l_room, pwad_map_formats};
    use crate::support::{call, call_err, connect};

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
}
