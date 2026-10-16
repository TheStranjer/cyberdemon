//! The `edit_sectors` tool group as seen by a real MCP client:
//! `add_sector` with its defaults and custom fields, the bad-flat,
//! over-bright-light and height-order refusals, `edit_sector` over single
//! indices and lists with deltas (a property edit that keeps the nodes),
//! the absolute/delta conflict and the empty request, `remove_sector` with
//! the `conflict` on referenced sectors, the cascaded room removal verified
//! through `get_map_summary` and `assemble_map`, and the undo of a whole
//! call.

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
}
