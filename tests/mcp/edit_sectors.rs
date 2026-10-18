//! The `edit_sectors` tool group as seen by a real MCP client:
//! `add_sector` with its defaults and custom fields, the bad-flat,
//! over-bright-light and height-order refusals, `edit_sector` over single
//! indices and lists with deltas (a property edit that keeps the nodes),
//! the absolute/delta conflict and the empty request, `remove_sector` with
//! the `conflict` on referenced sectors, the cascaded room removal verified
//! through `get_map_summary` and `assemble_map`, `create_sector_from_loop`
//! growing a room next to room 0, attached mid-wall (T-junction split) and
//! as a platform inside it, with `keep_interior` and the `none` outer
//! mode, `carve_void` punching a pillar and separating the rooms, and the
//! undo of a whole call. Every drawing ends with `validate_map` reporting
//! no error- or warning-severity findings.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad_map_formats};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Open the `DOOM II` fixture (one two-room binary `Doom` `MAP01`) as
    /// the base layer.
    async fn doom_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// Open the map-format fixture (`UDMF` `MAP03`, binary `Hexen` `MAP04`)
    /// as the base layer.
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

    /// The working archive's sector rows for `map`, in order, through
    /// `assemble_map`.
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

    /// The working `linedefs` row at `index` for `map`.
    async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["linedefs"]}),
        )
        .await;
        report["linedefs"]
            .as_array()
            .expect("linedefs is an array")
            .get(index)
            .cloned()
            .expect("the linedef row exists")
    }

    /// The merged view's sector rows for `map`, in order (the layers are
    /// what an unedited session shows; failed edits must leave it intact).
    async fn merged_sectors(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "merged", "include": ["sectors"]}),
        )
        .await;
        report["sectors"]
            .as_array()
            .expect("sectors is an array")
            .clone()
    }

    /// Requires `tool` to reject `args` with `invalid_argument`.
    async fn refuse(client: &Client, tool: &str, args: Value) {
        let note = args.to_string();
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
    }

    /// The working `sectors` row at `index` for `map`, through
    /// `assemble_map`.
    async fn working_sector_row(client: &Client, map: &str, index: usize) -> Value {
        working_sectors(client, map)
            .await
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| panic!("the sector row {index} exists"))
    }

    /// A `{x, y}` loop-corner parameter.
    fn pt(x: i32, y: i32) -> Value {
        json!({"x": x, "y": y})
    }

    /// A loop request over the square `(64, 64)` to `(192, 192)` with
    /// `tail` spliced in as further JSON fields.
    fn square(tail: &str) -> Value {
        format!(
            r#"{{"points": [{{"x": 64, "y": 64}}, {{"x": 192, "y": 64}},
                {{"x": 192, "y": 192}}, {{"x": 64, "y": 192}}]{tail}}}"#
        )
        .parse()
        .expect("test requests are valid JSON")
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

    /// Checks the adjacent-room drawing report: the whole map rebuilds
    /// deterministically, so the new piece order is part of the contract.
    fn expect_room_report(result: &Value) {
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["sector"], json!(2_i32), "a third sector");
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the room is new geometry"
        );
        assert_eq!(
            result["vertices_created"],
            json!([6_i32, 7_i32]),
            "the two free corners were created, the room-0 top was reused"
        );
        assert_eq!(
            result["linedefs_created"],
            json!([2_i32, 4_i32, 7_i32]),
            "the three boundary walls are new pieces, the fourth edge is the old wall"
        );
        assert_eq!(
            result["sidedefs_created"],
            json!([2_i32, 4_i32, 5_i32, 9_i32]),
            "three new fronts and the shared wall's new room side"
        );
        assert_eq!(result["removed_linedefs"], json!([]), "nothing vanished");
        assert_eq!(
            result["linedef_map"],
            json!([0_i32, 3_i32, 5_i32, 1_i32, 8_i32, 9_i32, 6_i32]),
            "old linedef index to new index of its first piece"
        );
        assert_eq!(result["notes"], json!([]), "nobody lost their room");
    }

    /// Checks the shared north wall (new linedef 3), opened two-sided with
    /// its texture moved onto the free faces.
    fn expect_shared_wall(line: &Value) {
        assert_eq!(
            line["flag_names"],
            json!(["twosided"]),
            "blocking cleared, two-sided set"
        );
        assert_eq!(line["front"]["sector"], json!(0_i32), "faces the hall");
        assert_eq!(line["front"]["middle"], json!("-"), "the middle moved");
        assert_eq!(
            line["front"]["upper"],
            json!("WALL1"),
            "onto the free faces"
        );
        assert_eq!(line["back"]["sector"], json!(2_i32), "and the new room");
        assert_eq!(line["back"]["middle"], json!("-"), "bare on the back");
    }

    /// Checks platform edge `edge`, a two-sided transition dressed on both
    /// faces: the hall below and the platform face each other, either way
    /// round (the engine orients every line by its coordinates).
    fn expect_platform_edge(line: &Value, edge: usize) {
        assert_eq!(line["flag_names"], json!(["twosided"]), "line {edge}");
        let (front, back) = (&line["front"], &line["back"]);
        let faces = [front["sector"].clone(), back["sector"].clone()];
        assert!(
            (faces[0] == json!(2_i32) && faces[1] == json!(0_i32))
                || (faces[0] == json!(0_i32) && faces[1] == json!(2_i32)),
            "hall and platform either way round: {faces:?}"
        );
        for side in [front, back] {
            assert_eq!(side["middle"], json!("-"), "the ledge face is open");
            assert_eq!(side["upper"], json!("STEP1"), "dressed above");
            assert_eq!(side["lower"], json!("STEP1"), "and below");
        }
    }

    #[tokio::test]
    async fn add_sector_appends_a_dead_sector_with_defaults() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "add_sector", json!({})).await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["index"], json!(2_i32), "appended after the two");
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the sector count is geometry"
        );
        assert_eq!(
            result["sector"],
            json!({
                "floor_height": 0_i32, "ceiling_height": 128_i32,
                "floor_flat": "FLOOR4_8", "ceiling_flat": "CEIL3_5",
                "light": 160_i32, "special": 0_i32, "tag": 0_i32,
            }),
            "the defaults echo"
        );
        let rows = working_sectors(&client, "MAP01").await;
        assert_eq!(rows.len(), 3, "the map holds three sectors now");
        let added = rows.last().expect("the appended sector");
        assert_eq!(added["floor_flat"], json!("FLOOR4_8"), "read back");
        assert_eq!(added["ceiling_flat"], json!("CEIL3_5"));
        assert_eq!(added["light"], json!(160_i32));
        assert_eq!(
            added["sidedefs"].as_array().expect("a list").len(),
            0,
            "no sidedef references it yet"
        );
    }

    #[tokio::test]
    async fn add_sector_stores_custom_values_and_refuses_bad_ones() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_sector",
            json!({"map": "MAP01", "floor_height": -8_i32, "ceiling_height": 256_i32,
                   "floor_flat": "floor1", "light": 255_i32, "special": 1_i32,
                   "tag": 66_i32}),
        )
        .await;
        assert_eq!(result["sector"]["floor_height"], json!(-8_i32), "stored");
        assert_eq!(result["sector"]["ceiling_height"], json!(256_i32));
        assert_eq!(
            result["sector"]["floor_flat"],
            json!("FLOOR1"),
            "uppercased"
        );
        assert_eq!(
            result["sector"]["ceiling_flat"],
            json!("CEIL3_5"),
            "absent stays the default"
        );
        assert_eq!(result["sector"]["light"], json!(255_i32));
        assert_eq!(result["sector"]["tag"], json!(66_i32));
        refuse(&client, "add_sector", json!({"floor_flat": "TOOLONG9C"})).await;
        refuse(&client, "add_sector", json!({"ceiling_flat": "-"})).await;
        refuse(&client, "add_sector", json!({"light": 300_i32})).await;
        refuse(&client, "add_sector", json!({"light": -1_i32})).await;
        refuse(&client, "add_sector", json!({"floor_height": 200_i32})).await;
        refuse(&client, "add_sector", json!({"tag": 70_000_i32})).await;
        let (map_code, _) = call_err(
            &client,
            "add_sector",
            json!({"map": "MAP09", "floor_flat": "FLOOR4_8"}),
        )
        .await;
        assert_eq!(map_code, "not_found", "unknown map");
        let rows = merged_sectors(&client, "MAP01").await;
        assert_eq!(
            rows.len(),
            3,
            "only the one successful add stands in the merged view"
        );
        assert_eq!(rows[2]["tag"], json!(66_i32), "and it is the custom one");
    }

    #[tokio::test]
    async fn edit_sector_changes_heights_and_keeps_the_nodes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "floor_height": 24_i32, "ceiling_height": 200_i32}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the default map");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "sector properties are not geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "first edit copied"
        );
        let rows = result["sectors"].as_array().expect("a sectors array");
        assert_eq!(rows.len(), 1, "one row reported");
        let row = &rows[0];
        assert_eq!(row["index"], json!(0_i32), "for the selected sector");
        assert_eq!(row["floor_height"], json!(24_i32), "the asked heights");
        assert_eq!(row["ceiling_height"], json!(200_i32));
        assert_eq!(row["floor_flat"], json!("FLOOR1"), "the rest stands");
        assert_eq!(row["light"], json!(160_i32));
        assert_eq!(row["tag"], json!(0_i32));
        let stored = working_sectors(&client, "MAP01").await;
        assert_eq!(
            stored[0]["floor_height"],
            json!(24_i32),
            "and the map holds it"
        );
        assert_eq!(
            stored[1]["floor_height"],
            json!(32_i32),
            "neighbours untouched"
        );
    }

    #[tokio::test]
    async fn edit_sector_applies_to_lists_and_shifts_with_deltas() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let first = call(
            &client,
            "edit_sector",
            json!({"index": [1_i32, 0_i32], "floor_delta": 16_i32}),
        )
        .await;
        let rows = first["sectors"].as_array().expect("a sectors array");
        assert_eq!(rows.len(), 2, "both sectors moved");
        assert_eq!(rows[0]["index"], json!(0_i32), "the rows come sorted");
        assert_eq!(rows[0]["floor_height"], json!(16_i32), "0 plus 16");
        assert_eq!(
            rows[1]["floor_height"],
            json!(48_i32),
            "32 plus 16 from its own value"
        );
        assert_eq!(
            first["nodes_invalidated"],
            json!(false),
            "heights are not geometry either"
        );
        let second = call(
            &client,
            "edit_sector",
            json!({"index": [0_i32, 1_i32], "ceiling_delta": -16_i32, "floor_flat": "ceil5"}),
        )
        .await;
        let lowered = second["sectors"].as_array().expect("a sectors array");
        assert_eq!(lowered[0]["ceiling_height"], json!(112_i32), "128 minus 16");
        assert_eq!(
            lowered[0]["floor_flat"],
            json!("CEIL5"),
            "names go on all selected"
        );
        assert_eq!(lowered[1]["ceiling_height"], json!(112_i32));
        assert_eq!(
            lowered[1]["floor_height"],
            json!(48_i32),
            "the earlier delta stands"
        );
        let stored = working_sectors(&client, "MAP01").await;
        assert_eq!(
            stored[0]["ceiling_height"],
            json!(112_i32),
            "written and read back"
        );
        assert_eq!(stored[0]["floor_flat"], json!("CEIL5"));
    }

    #[tokio::test]
    async fn edit_sector_refuses_conflicting_empty_and_impossible_requests() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "floor_height": 8_i32, "floor_delta": 8_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "ceiling_height": 8_i32, "ceiling_delta": 8_i32}),
        )
        .await;
        refuse(&client, "edit_sector", json!({"index": 0_i32})).await;
        refuse(&client, "edit_sector", json!({"index": []})).await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "floor_height": 200_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "light": 300_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "floor_flat": "flat-dash"}),
        )
        .await;
        refuse(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "tag": 70_000_i32}),
        )
        .await;
        let (index_code, message) = call_err(
            &client,
            "edit_sector",
            json!({"index": 9_i32, "light": 200_i32}),
        )
        .await;
        assert_eq!(index_code, "not_found", "an index past the end is missing");
        assert!(
            message.contains("sector 9 does not exist (map has 2 sectors)"),
            "the message counts the sectors: {message}"
        );
        let stored = merged_sectors(&client, "MAP01").await;
        assert_eq!(stored.len(), 2, "the refusals appended nothing");
        assert_eq!(stored[0]["floor_height"], json!(0_i32), "nothing changed");
        assert_eq!(stored[1]["light"], json!(192_i32));
    }

    #[tokio::test]
    async fn a_closed_door_equal_heights_is_allowed() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_sector",
            json!({"index": 0_i32, "ceiling_height": 0_i32}),
        )
        .await;
        assert_eq!(
            result["sectors"][0]["ceiling_height"],
            json!(0_i32),
            "floor and ceiling equal: a closed door is fine"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "still a property edit"
        );
    }

    #[tokio::test]
    async fn the_binary_height_and_tag_caps_and_the_flat_cap_follow_the_format() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        refuse(
            &client,
            "add_sector",
            json!({"map": "MAP04", "tag": 70_000_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sector",
            json!({"map": "MAP04", "index": 0_i32, "floor_height": 40_000_i32}),
        )
        .await;
        let udmf = call(
            &client,
            "add_sector",
            json!({"map": "MAP03", "ceiling_height": 40_000_i32, "tag": 70_000_i32,
                   "floor_flat": "brick01_metal"}),
        )
        .await;
        assert_eq!(
            udmf["sector"]["ceiling_height"],
            json!(40_000_i32),
            "UDMF stores the wide height"
        );
        assert_eq!(udmf["sector"]["tag"], json!(70_000_i32), "and the wide tag");
        assert_eq!(
            udmf["sector"]["floor_flat"],
            json!("BRICK01_METAL"),
            "and the thirteen-character flat"
        );
    }

    #[tokio::test]
    async fn remove_sector_refuses_used_sectors_and_drops_orphans() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(&client, "remove_sector", json!({"index": 0_i32})).await;
        assert_eq!(code, "conflict", "sidedefs frame sector 0");
        assert!(
            message.contains("sector 0 is used by sidedef(s) 0, 1, 2, 4")
                && message.contains("cascade"),
            "the message names the users and the fix: {message}"
        );
        assert_eq!(
            merged_sectors(&client, "MAP01").await.len(),
            2,
            "the conflict removed nothing"
        );
        call(&client, "add_sector", json!({"light": 64_i32})).await;
        let result = call(&client, "remove_sector", json!({"index": 2_i32})).await;
        assert_eq!(
            result["removed"]["sectors"],
            json!([2_i32]),
            "the added orphan goes"
        );
        assert_eq!(result["removed"]["sidedefs"], json!([]), "no side followed");
        assert_eq!(result["removed"]["linedefs"], json!([]), "and no line");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the sector count is geometry"
        );
        let rows = working_sectors(&client, "MAP01").await;
        assert_eq!(rows.len(), 2, "back to the fixture's two sectors");
    }

    #[tokio::test]
    async fn remove_sector_with_cascade_takes_the_room_with_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "remove_sector",
            json!({"index": 1_i32, "cascade": true}),
        )
        .await;
        assert_eq!(
            result["removed"]["sectors"],
            json!([1_i32]),
            "the room goes"
        );
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([3_i32, 5_i32, 6_i32, 7_i32]),
            "every side of it goes"
        );
        assert_eq!(
            result["removed"]["linedefs"],
            json!([4_i32, 5_i32, 6_i32]),
            "the walls only those sides held go"
        );
        assert_eq!(
            result["removed"]["vertices"],
            json!([2_i32, 3_i32]),
            "and their orphaned corners"
        );
        assert_eq!(
            result["removed"]["one_sided_linedefs"],
            json!([2_i32]),
            "the shared step line closed"
        );
        assert_eq!(
            result["removed"]["flipped_linedefs"],
            json!([]),
            "nothing flipped"
        );
    }

    #[tokio::test]
    async fn the_cascaded_sector_removal_shows_in_summary_and_assembly() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "remove_sector",
            json!({"index": 1_i32, "cascade": true}),
        )
        .await;
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(
            summary["counts"]["sectors"],
            json!(1_i32),
            "one sector left"
        );
        assert_eq!(
            summary["counts"]["sidedefs"],
            json!(4_i32),
            "four sides left"
        );
        assert_eq!(
            summary["counts"]["linedefs"],
            json!(4_i32),
            "four lines left"
        );
        assert_eq!(
            summary["counts"]["vertices"],
            json!(4_i32),
            "four vertices left"
        );
        let shared = working_linedef(&client, "MAP01", 2).await;
        assert_eq!(
            shared["back"],
            Value::Null,
            "the step line is one-sided now"
        );
        assert_eq!(
            shared["front"]["sector"],
            json!(0_i32),
            "its surviving side faces the hall"
        );
        assert_eq!(
            shared["flag_names"],
            json!(["blocking"]),
            "two-sided cleared, blocking set"
        );
    }

    /// Runs the add/edit/remove trio on a fresh sector of the fixture map.
    async fn add_edit_remove_trio(client: &Client) {
        call(client, "add_sector", json!({"light": 64_i32})).await;
        call(
            client,
            "edit_sector",
            json!({"index": 2_i32, "floor_height": 8_i32}),
        )
        .await;
        call(client, "remove_sector", json!({"index": 2_i32})).await;
    }

    #[tokio::test]
    async fn sector_edits_undo_and_redo_as_one_step_each() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        add_edit_remove_trio(&client).await;
        let after_remove = working_sectors(&client, "MAP01").await;
        assert_eq!(after_remove.len(), 2, "the trio cancelled out");
        let undo_remove = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo_remove["undone"],
            json!(["remove 1 sectors"]),
            "the removal rolls back alone"
        );
        let restored = working_sectors(&client, "MAP01").await;
        assert_eq!(restored.len(), 3, "the sector is back");
        assert_eq!(
            restored[2]["floor_height"],
            json!(8_i32),
            "with the edited floor"
        );
        let undo_edit = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo_edit["undone"],
            json!(["edit 1 sectors"]),
            "and so is the edit"
        );
        let half_undone = working_sectors(&client, "MAP01").await;
        assert_eq!(
            half_undone[2]["floor_height"],
            json!(0_i32),
            "the edit is gone"
        );
        assert_eq!(
            half_undone[2]["light"],
            json!(64_i32),
            "the add survives it"
        );
        let undo_add = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo_add["undone"],
            json!(["add sector"]),
            "and so is the add, copy included"
        );
        assert_eq!(
            merged_sectors(&client, "MAP01").await.len(),
            2,
            "back to the fixture's two"
        );
        let redo = call(&client, "redo", json!({ "steps": 3_u64 })).await;
        assert_eq!(
            redo["redone"],
            json!(["add sector", "edit 1 sectors", "remove 1 sectors"]),
            "all three steps replay in order"
        );
        let redone = working_sectors(&client, "MAP01").await;
        assert_eq!(
            redone.len(),
            2,
            "the redone removal removed the sector again"
        );
    }

    #[tokio::test]
    async fn create_sector_from_loop_grows_a_room_next_to_room_zero() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "create_sector_from_loop",
            json!({"points": [pt(0, 256), pt(256, 256), pt(256, 512), pt(0, 512)]}),
        )
        .await;
        expect_room_report(&result);
        expect_shared_wall(&working_linedef(&client, "MAP01", 3).await);
        let hall = working_sector_row(&client, "MAP01", 0).await;
        assert_eq!(
            hall["neighbors"],
            json!([1_i32, 2_i32]),
            "the hall now also neighbours the new room"
        );
        let room = working_sector_row(&client, "MAP01", 2).await;
        assert_eq!(room["neighbors"], json!([0_i32]), "one shared wall");
        assert_eq!(
            room["bbox"],
            json!([0_i32, 256_i32, 256_i32, 512_i32]),
            "the new box"
        );
        assert_eq!(
            room["floor_flat"],
            json!("FLOOR4_8"),
            "the sector defaults stored"
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(
            summary["counts"],
            json!({"things": 2_i32, "vertices": 8_i32, "linedefs": 10_i32,
                   "sidedefs": 12_i32, "sectors": 3_i32}),
            "the whole map grew as reported"
        );
        validate_clean(&client, "MAP01").await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["create sector from loop"]),
            "the drawing is one undo step"
        );
        let summary_after = call(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "merged"}),
        )
        .await;
        assert_eq!(
            summary_after["counts"]["linedefs"],
            json!(7_i32),
            "the base layer kept its original seven lines"
        );
    }

    #[tokio::test]
    async fn create_sector_from_loop_welds_to_a_wall_mid_span() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "create_sector_from_loop",
            json!({"points": [pt(64, 256), pt(192, 256), pt(192, 384), pt(64, 384)]}),
        )
        .await;
        assert_eq!(result["sector"], json!(2_i32), "a third sector");
        assert_eq!(
            result["linedefs_created"],
            json!([3_i32, 5_i32, 6_i32]),
            "the nub's three free edges are new"
        );
        assert_eq!(
            result["vertices_created"],
            json!([6_i32, 7_i32, 8_i32, 9_i32]),
            "the two T-junction points and the two corners are new"
        );
        assert_eq!(
            result["linedef_map"][1],
            json!(2_i32),
            "the old hall wall maps to its west piece"
        );
        assert_eq!(
            result["removed_linedefs"],
            json!([]),
            "the split wall still lives in its pieces"
        );
        let lines = working_linedefs(&client, "MAP01").await;
        let shared = lines
            .iter()
            .find(|line| same_ends(line, [end(64, 256), end(192, 256)]))
            .expect("the shared piece exists");
        expect_shared_wall(shared);
        assert_eq!(
            shared["front"]["x_offset"],
            json!(64_i32),
            "the texture slid with the piece start"
        );
        let hall = working_sector_row(&client, "MAP01", 0).await;
        assert_eq!(
            hall["neighbors"],
            json!([1_i32, 2_i32]),
            "the hall faces the nub across the opened piece"
        );
        let nub = working_sector_row(&client, "MAP01", 2).await;
        assert_eq!(
            nub["bbox"],
            json!([64_i32, 256_i32, 192_i32, 384_i32]),
            "the nub box"
        );
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn create_sector_from_loop_raises_a_platform_inside_room_zero() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "create_sector_from_loop",
            square(
                r#", "floor_height": 8, "ceiling_height": 192,
                     "upper_texture": "STEP1", "lower_texture": "step1""#,
            ),
        )
        .await;
        assert_eq!(result["sector"], json!(2_i32));
        assert_eq!(
            result["linedefs_created"],
            json!([3_i32, 4_i32, 5_i32, 6_i32]),
            "all four walls are new lines"
        );
        assert_eq!(
            result["vertices_created"],
            json!([6_i32, 7_i32, 8_i32, 9_i32]),
            "the four corners are new vertices"
        );
        assert_eq!(
            result["sidedefs_created"].as_array().expect("a list").len(),
            8_usize,
            "two sides per wall"
        );
        expect_platform_edge(&working_linedef(&client, "MAP01", 3).await, 3_usize);
        expect_platform_edge(&working_linedef(&client, "MAP01", 4).await, 4_usize);
        expect_platform_edge(&working_linedef(&client, "MAP01", 5).await, 5_usize);
        expect_platform_edge(&working_linedef(&client, "MAP01", 6).await, 6_usize);
        let platform = working_sector_row(&client, "MAP01", 2).await;
        assert_eq!(platform["floor_height"], json!(8_i32), "raised floor");
        assert_eq!(platform["ceiling_height"], json!(192_i32), "and ceiling");
        assert_eq!(platform["neighbors"], json!([0_i32]), "across is the hall");
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn create_sector_from_loop_with_none_outer_keeps_walls_single() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "create_sector_from_loop",
            square(r#", "outer_sector": "none""#),
        )
        .await;
        assert_eq!(
            result["linedefs_created"],
            json!([3_i32, 4_i32, 5_i32, 6_i32]),
            "the four sealed walls"
        );
        assert_eq!(
            result["sidedefs_created"].as_array().expect("a list").len(),
            4_usize,
            "one front side per wall"
        );
        let line = working_linedef(&client, "MAP01", 3).await;
        assert_eq!(line["flag_names"], json!(["blocking"]), "a sealed box");
        assert_eq!(line["back"], Value::Null, "no back side");
        assert_eq!(
            line["front"]["middle"],
            json!("STARTAN3"),
            "the default wall texture"
        );
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn create_sector_from_loop_keep_interior_preserves_the_pillar() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "create_sector_from_loop", square("")).await;
        let big = call(
            &client,
            "create_sector_from_loop",
            json!({
                "points": [pt(32, 32), pt(224, 32), pt(224, 224), pt(32, 224)],
                "keep_interior": true, "ceiling_height": 160_i64,
            }),
        )
        .await;
        assert_eq!(big["sector"], json!(3_i32), "a fourth sector");
        assert_eq!(
            big["empty_sectors"],
            json!([]),
            "the pillar keeps its area: {}",
            big["notes"]
        );
        let platform = working_sector_row(&client, "MAP01", 2).await;
        assert_eq!(
            platform["sidedefs"].as_array().expect("a list").len(),
            4_usize,
            "the pillar's four sides stand"
        );
        assert_eq!(
            platform["neighbors"],
            json!([3_i32]),
            "boxed in by the new room"
        );
        let room = working_sector_row(&client, "MAP01", 3).await;
        assert!(
            room["neighbors"]
                .as_array()
                .expect("a list")
                .contains(&json!(2_i32)),
            "the new room wraps the pillar"
        );
        validate_clean(&client, "MAP01").await;
    }

    #[tokio::test]
    async fn create_sector_from_loop_without_the_flag_overwrites_the_pillar() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "create_sector_from_loop", square("")).await;
        call(
            &client,
            "create_sector_from_loop",
            json!({
                "points": [pt(32, 32), pt(224, 32), pt(224, 224), pt(32, 224)],
                "keep_interior": true, "ceiling_height": 160_i64,
            }),
        )
        .await;
        let overwritten = call(
            &client,
            "create_sector_from_loop",
            json!({
                "points": [pt(16, 16), pt(240, 16), pt(240, 240), pt(16, 240)],
                "ceiling_height": 192_i64,
            }),
        )
        .await;
        assert_eq!(
            overwritten["empty_sectors"],
            json!([2_i32, 3_i32]),
            "without the flag the previous room and the pillar are overwritten"
        );
        let notes = overwritten["notes"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<&str>>();
        assert!(
            notes.iter().any(|note| note.contains("sector 3")),
            "the note names it: {notes:?}"
        );
    }

    /// Requires every linedef `created` by the last report to be a
    /// one-sided wall wearing `middle`; `sector`, when given, is the
    /// sector every wall must face.
    fn expect_single_walls(lines: &[Value], created: &[Value], sector: Option<i32>, middle: &str) {
        assert!(!created.is_empty(), "the report listed walls: {created:?}");
        for index in created {
            let position = usize::try_from(index.as_i64().expect("created indices are numbers"))
                .expect("non-negative");
            check_one_wall(
                lines
                    .get(position)
                    .unwrap_or_else(|| panic!("line {position}")),
                sector,
                middle,
            );
        }
    }

    /// Checks one carved wall: blocking, one-sided, wearing `middle` and,
    /// when given, facing `sector`.
    fn check_one_wall(line: &Value, sector: Option<i32>, middle: &str) {
        assert_eq!(line["flag_names"], json!(["blocking"]), "a single wall");
        assert_eq!(line["back"], Value::Null, "no side into the rock");
        assert_eq!(line["front"]["middle"], json!(middle), "its texture");
        let faced = line["front"]["sector"].clone();
        match sector {
            Some(want) => assert_eq!(faced, json!(want), "the wall faces its sector"),
            None => assert!(faced.is_number(), "the wall faces some sector"),
        }
    }

    /// A grid point parameter for [`same_ends`].
    fn end(x: i32, y: i32) -> (i32, i32) {
        (x, y)
    }

    /// Whether `line` spans exactly the two grid points `ends`.
    fn same_ends(line: &Value, ends: [(i32, i32); 2]) -> bool {
        let at = |key: &str| -> Option<(i32, i32)> {
            let point = line.get(key)?;
            Some((
                i32::try_from(point["x"].as_i64()?).ok()?,
                i32::try_from(point["y"].as_i64()?).ok()?,
            ))
        };
        match (at("v1"), at("v2")) {
            (Some(first), Some(second)) => {
                (first == ends[0] && second == ends[1]) || (first == ends[1] && second == ends[0])
            }
            _ => false,
        }
    }

    #[tokio::test]
    async fn carve_void_punches_a_pillar_in_the_hall() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "carve_void",
            json!({"points": [pt(96, 96), pt(160, 96), pt(160, 160), pt(96, 160)]}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the default map");
        assert_eq!(result["sector"], Value::Null, "a carve adds no sector");
        assert_eq!(
            result["linedefs_created"],
            json!([3_i32, 4_i32, 5_i32, 6_i32]),
            "the four walls around the rock"
        );
        assert_eq!(
            result["vertices_created"],
            json!([6_i32, 7_i32, 8_i32, 9_i32]),
            "the pillar corners"
        );
        assert_eq!(
            result["removed_linedefs"],
            json!([]),
            "no existing line died"
        );
        assert_eq!(
            result["removed_sectors_empty"],
            json!([]),
            "the hall survives"
        );
        expect_single_walls(
            &working_linedefs(&client, "MAP01").await,
            result["linedefs_created"].as_array().expect("a list"),
            Some(0_i32),
            "WALL1",
        );
        validate_clean(&client, "MAP01").await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["carve void"]),
            "the carve is one undo step"
        );
    }

    #[tokio::test]
    async fn carve_void_separates_the_two_rooms() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "carve_void",
            json!({"points": [pt(192, 0), pt(320, 0), pt(320, 256), pt(192, 256)]}),
        )
        .await;
        assert_eq!(
            result["removed_linedefs"],
            json!([2_i32]),
            "the shared step line is carved away"
        );
        assert_eq!(
            result["linedefs_created"],
            json!([3_i32, 4_i32]),
            "the trench walls, east of the hall and west of the room"
        );
        assert_eq!(
            result["removed_sectors_empty"],
            json!([]),
            "both rooms survive"
        );
        let lines = working_linedefs(&client, "MAP01").await;
        expect_single_walls(
            &lines,
            result["linedefs_created"].as_array().expect("a list"),
            None,
            "WALL1",
        );
        let trench = lines
            .iter()
            .find(|line| same_ends(line, [end(192, 0), end(192, 256)]))
            .expect("the hall-facing trench wall");
        assert_eq!(trench["front"]["sector"], json!(0_i32), "walls the hall");
        let far = lines
            .iter()
            .find(|line| same_ends(line, [end(320, 0), end(320, 256)]))
            .expect("the room-facing trench wall");
        assert_eq!(far["front"]["sector"], json!(1_i32), "walls the room");
        let hall = working_sector_row(&client, "MAP01", 0).await;
        let room = working_sector_row(&client, "MAP01", 1).await;
        assert!(
            hall["neighbors"].as_array().expect("a list").is_empty(),
            "the hall no longer touches the room: {}",
            hall["neighbors"]
        );
        assert!(
            room["neighbors"].as_array().expect("a list").is_empty(),
            "nor back: {}",
            room["neighbors"]
        );
        validate_clean(&client, "MAP01").await;
    }

    /// Carve requests the tool must reject with `invalid_argument`: a
    /// missing or doubled form, a weak, flat or crossing ring and a
    /// coordinate outside the binary `i16` range.
    fn invalid_carves() -> Vec<Value> {
        vec![
            json!({}),
            json!({"points": [pt(0, 0), pt(64, 64)]}),
            json!({"points": [pt(0, 0), pt(128, 0), pt(256, 0)]}),
            json!({"points": [pt(0, 0), pt(256, 256), pt(256, 0), pt(0, 256)]}),
            json!({"points": [pt(0, 0), pt(40000, 0), pt(0, 256)]}),
            json!({"points": [pt(0, 0), pt(64, 64)], "vertices": [0, 1, 2]}),
        ]
    }

    #[tokio::test]
    async fn carve_void_refuses_bad_polygons() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse_all(&client, "carve_void", invalid_carves()).await;
        let (code, _) = call_err(
            &client,
            "carve_void",
            json!({"vertices": [0_i32, 1_i32, 99_i32]}),
        )
        .await;
        assert_eq!(code, "not_found", "unknown vertex");
        let (map_code, _) = call_err(
            &client,
            "carve_void",
            json!({"map": "MAP09", "points": [pt(0, 0), pt(64, 0), pt(0, 64)]}),
        )
        .await;
        assert_eq!(map_code, "not_found", "unknown map");
        let rows = merged_sectors(&client, "MAP01").await;
        assert_eq!(rows.len(), 2, "the refusals carved nothing");
    }

    /// Loop requests the tool must reject with `invalid_argument`: a
    /// missing or doubled form, a weak, flat or crossing ring, and fields
    /// outside the format's ranges.
    fn invalid_loop_requests() -> Vec<Value> {
        vec![
            json!({}),
            square(r#", "vertices": [0, 1, 2]"#),
            json!({"points": [pt(0, 0), pt(128, 128)]}),
            json!({"points": [pt(0, 0), pt(128, 0), pt(256, 0)]}),
            json!({"points": [pt(0, 0), pt(256, 0), pt(64, 192), pt(256, 256)]}),
            square(", \"light\": 300"),
            square(", \"ceiling_flat\": \"-\""),
            square(", \"wall_texture\": \"TOOLONG12\""),
            square(", \"outer_sector\": \"sideways\""),
            json!({"points": [pt(0, 0), pt(40000, 0), pt(0, 256)]}),
        ]
    }

    /// Requires `tool` to refuse every one of `requests`.
    async fn refuse_all(client: &Client, tool: &str, requests: Vec<Value>) {
        for args in requests {
            refuse(client, tool, args).await;
        }
    }

    /// Requires `create_sector_from_loop` to refuse every one of
    /// `requests`.
    async fn refuse_loops(client: &Client, requests: &[Value]) {
        refuse_all(client, "create_sector_from_loop", requests.to_vec()).await;
    }

    #[tokio::test]
    async fn create_sector_from_loop_refuses_bad_requests() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse_loops(&client, &invalid_loop_requests()).await;
        let (code, _) = call_err(
            &client,
            "create_sector_from_loop",
            json!({"map": "MAP09", "points": [pt(0, 0), pt(128, 0), pt(0, 128)]}),
        )
        .await;
        assert_eq!(code, "not_found", "unknown map");
        let (vertex_code, _) = call_err(
            &client,
            "create_sector_from_loop",
            json!({"vertices": [0_i32, 1_i32, 99_i32]}),
        )
        .await;
        assert_eq!(vertex_code, "not_found", "unknown vertex");
        let (outer_code, message) = call_err(
            &client,
            "create_sector_from_loop",
            square(", \"outer_sector\": 9"),
        )
        .await;
        assert_eq!(outer_code, "not_found", "unknown outer sector");
        assert!(
            message.contains("map has 2 sectors"),
            "the message counts the sectors: {message}"
        );
        let rows = merged_sectors(&client, "MAP01").await;
        assert_eq!(rows.len(), 2, "the refusals drew nothing");
    }
}
