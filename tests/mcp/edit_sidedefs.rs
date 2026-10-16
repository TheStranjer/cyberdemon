//! The `edit_sidedefs` tool group as seen by a real MCP client:
//! `add_sidedef` with its defaults and texture/offset fields, the
//! missing-sector and bad-name refusals, `edit_sidedef` over single
//! indices and lists with absolute and delta offsets, the sector change
//! as a geometry edit, and `remove_sidedef` with the `conflict` on
//! referenced sides, the cascaded flip verified through `assemble_map`
//! and the undo of a whole call.

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

    /// The working archive's sidedef rows for `map`, in order, through
    /// `assemble_map`.
    async fn working_sidedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["sidedefs"]}),
        )
        .await;
        report["sidedefs"]
            .as_array()
            .expect("sidedefs is an array")
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

    /// The merged view's sidedef rows for `map`, in order (the layers are
    /// what an unedited session shows; failed edits must leave it intact).
    async fn merged_sidedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "merged", "include": ["sidedefs"]}),
        )
        .await;
        report["sidedefs"]
            .as_array()
            .expect("sidedefs is an array")
            .clone()
    }

    /// Requires `tool` to reject `args` with `invalid_argument`.
    async fn refuse(client: &Client, tool: &str, args: Value) {
        let note = args.to_string();
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
    }

    #[tokio::test]
    async fn add_sidedef_appends_an_unreferenced_side_with_defaults() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "add_sidedef", json!({"sector": 1_i32})).await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["index"], json!(8_i32), "appended after the eight");
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the sidedef block is geometry"
        );
        assert_eq!(
            result["sidedef"],
            json!({
                "sector": 1_i32, "upper": "-", "middle": "-", "lower": "-",
                "x_offset": 0_i32, "y_offset": 0_i32,
            }),
            "the defaults echo"
        );
        let rows = working_sidedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 9, "the map holds nine sidedefs now");
        assert_eq!(rows[8]["sector"], json!(1_i32), "read back over sector 1");
        assert_eq!(rows[8]["linedef"], Value::Null, "no linedef uses it yet");
        assert_eq!(rows[8]["side"], Value::Null, "so it is not a side");
        assert_eq!(rows[8]["middle"], json!("-"), "and the textures are dashes");
    }

    #[tokio::test]
    async fn add_sidedef_stores_textures_offsets_and_uppercases_names() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_sidedef",
            json!({"map": "MAP01", "sector": 0_i32, "upper": "brick",
                   "middle": "STARTAN3", "x_offset": 8_i32, "y_offset": -16_i32}),
        )
        .await;
        assert_eq!(result["sidedef"]["upper"], json!("BRICK"), "uppercased");
        assert_eq!(result["sidedef"]["middle"], json!("STARTAN3"));
        assert_eq!(
            result["sidedef"]["lower"],
            json!("-"),
            "absent stays a dash"
        );
        assert_eq!(
            result["sidedef"]["x_offset"],
            json!(8_i32),
            "the shifts land"
        );
        assert_eq!(result["sidedef"]["y_offset"], json!(-16_i32));
        let rows = working_sidedefs(&client, "MAP01").await;
        let added = rows.last().expect("the appended sidedef");
        assert_eq!(added["upper"], json!("BRICK"), "written and read back");
        assert_eq!(added["y_offset"], json!(-16_i32), "with the negative shift");
    }

    #[tokio::test]
    async fn add_sidedef_refuses_missing_sectors_bad_names_and_wide_offsets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(&client, "add_sidedef", json!({"sector": 9_i32})).await;
        assert_eq!(code, "not_found", "missing sector");
        assert!(
            message.contains("sector 9 does not exist (map has 2 sectors)"),
            "the message counts the sectors: {message}"
        );
        refuse(
            &client,
            "add_sidedef",
            json!({"sector": 0_i32, "middle": "TOOLONGTEXTURE"}),
        )
        .await;
        refuse(
            &client,
            "add_sidedef",
            json!({"sector": 0_i32, "upper": ""}),
        )
        .await;
        refuse(
            &client,
            "add_sidedef",
            json!({"sector": 0_i32, "x_offset": 0x8000_i32}),
        )
        .await;
        refuse(
            &client,
            "add_sidedef",
            json!({"sector": 0_i32, "y_offset": -0x8001_i32}),
        )
        .await;
        let (map_code, _) = call_err(
            &client,
            "add_sidedef",
            json!({"map": "MAP09", "sector": 0_i32}),
        )
        .await;
        assert_eq!(map_code, "not_found", "unknown map");
        let rows = merged_sidedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 8, "none of the refusals appended");
    }

    #[tokio::test]
    async fn the_binary_offset_cap_and_the_name_length_follow_the_format() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        refuse(
            &client,
            "add_sidedef",
            json!({"map": "MAP04", "sector": 0_i32, "x_offset": 40_000_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sidedef",
            json!({"map": "MAP04", "index": 0_i32, "y_offset_delta": 40_000_i32}),
        )
        .await;
        let udmf = call(
            &client,
            "add_sidedef",
            json!({"map": "MAP03", "sector": 1_i32, "x_offset": 40_000_i32,
                   "middle": "brick01_metal"}),
        )
        .await;
        assert_eq!(
            udmf["sidedef"]["x_offset"],
            json!(40_000_i32),
            "UDMF stores the wide offset"
        );
        assert_eq!(
            udmf["sidedef"]["middle"],
            json!("BRICK01_METAL"),
            "and the thirteen-character name"
        );
    }

    #[tokio::test]
    async fn edit_sidedef_changes_a_texture_without_touching_the_nodes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "middle": "brick5"}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the default map");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "a texture is not geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "first edit copied"
        );
        let rows = result["sidedefs"].as_array().expect("a sidedefs array");
        assert_eq!(rows.len(), 1, "one row reported");
        let row = &rows[0];
        assert_eq!(row["index"], json!(0_i32), "for the selected sidedef");
        assert_eq!(row["middle"], json!("BRICK5"), "uppercased and stored");
        assert_eq!(row["sector"], json!(0_i32), "the rest stands");
        assert_eq!(row["x_offset"], json!(0_i32));
        let stored = working_sidedefs(&client, "MAP01").await;
        assert_eq!(stored[0]["middle"], json!("BRICK5"), "and the map holds it");
        assert_eq!(stored[1]["middle"], json!("WALL1"), "neighbours untouched");
    }

    #[tokio::test]
    async fn edit_sidedef_applies_to_lists_and_shifts_with_deltas() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let first = call(
            &client,
            "edit_sidedef",
            json!({"index": [1_i32, 0_i32], "x_offset_delta": 8_i32, "y_offset_delta": -4_i32}),
        )
        .await;
        let rows = first["sidedefs"].as_array().expect("a sidedefs array");
        assert_eq!(rows.len(), 2, "both sides moved");
        assert_eq!(rows[0]["index"], json!(0_i32), "the rows come sorted");
        assert_eq!(rows[0]["x_offset"], json!(8_i32), "0 plus 8");
        assert_eq!(rows[0]["y_offset"], json!(-4_i32), "0 minus 4");
        assert_eq!(
            rows[1]["x_offset"],
            json!(8_i32),
            "the delta applies to each"
        );
        let second = call(
            &client,
            "edit_sidedef",
            json!({"index": [0_i32, 1_i32], "x_offset_delta": 4_i32, "lower": "step1"}),
        )
        .await;
        let moved = second["sidedefs"].as_array().expect("a sidedefs array");
        assert_eq!(
            moved[0]["x_offset"],
            json!(12_i32),
            "8 plus 4 from its own value"
        );
        assert_eq!(
            moved[1]["lower"],
            json!("STEP1"),
            "names go on all selected"
        );
        assert_eq!(
            second["nodes_invalidated"],
            json!(false),
            "offsets are not geometry either"
        );
        let stored = working_sidedefs(&client, "MAP01").await;
        assert_eq!(
            stored[0]["x_offset"],
            json!(12_i32),
            "written and read back"
        );
        assert_eq!(stored[7]["x_offset"], json!(0_i32), "the unselected holds");
    }

    #[tokio::test]
    async fn edit_sidedef_refuses_conflicting_and_empty_requests() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "x_offset": 4_i32, "x_offset_delta": 4_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "y_offset": 4_i32, "y_offset_delta": 4_i32}),
        )
        .await;
        refuse(&client, "edit_sidedef", json!({"index": 0_i32})).await;
        refuse(&client, "edit_sidedef", json!({"index": []})).await;
        refuse(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "upper": "bad name"}),
        )
        .await;
        refuse(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "x_offset": 40_000_i32}),
        )
        .await;
        let (index_code, _) = call_err(
            &client,
            "edit_sidedef",
            json!({"index": 99_i32, "middle": "X"}),
        )
        .await;
        assert_eq!(index_code, "not_found", "an index past the end is missing");
        let (sector_code, message) = call_err(
            &client,
            "edit_sidedef",
            json!({"index": 0_i32, "sector": 9_i32}),
        )
        .await;
        assert_eq!(sector_code, "not_found", "a sector past the end is missing");
        assert!(
            message.contains("sector 9"),
            "the message names the sector: {message}"
        );
        let stored = merged_sidedefs(&client, "MAP01").await;
        assert_eq!(stored[0]["middle"], json!("WALL1"), "nothing changed");
    }

    #[tokio::test]
    async fn edit_sidedef_sector_changes_are_a_geometry_change() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_sidedef",
            json!({"index": 7_i32, "sector": 0_i32}),
        )
        .await;
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "a sidedef's sector is geometry"
        );
        let row = &result["sidedefs"][0];
        assert_eq!(row["sector"], json!(0_i32), "the side faces the hall now");
        let stored = working_sidedefs(&client, "MAP01").await;
        assert_eq!(stored[7]["sector"], json!(0_i32), "written and read back");
    }

    #[tokio::test]
    async fn remove_sidedef_drops_an_unreferenced_side_and_conflicts_on_a_used_one() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "add_sidedef", json!({"sector": 0_i32})).await;
        let result = call(&client, "remove_sidedef", json!({"index": 8_i32})).await;
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([8_i32]),
            "the added orphan goes"
        );
        assert_eq!(result["removed"]["linedefs"], json!([]), "no line followed");
        assert_eq!(
            result["removed"]["flipped_linedefs"],
            json!([]),
            "and none was flipped"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "the sidedef block is geometry"
        );
        let (code, message) = call_err(&client, "remove_sidedef", json!({"index": 0_i32})).await;
        assert_eq!(code, "conflict", "sidedef 0 fronts a linedef");
        assert!(
            message.contains("sidedef 0 is used by linedef(s) 0") && message.contains("cascade"),
            "the message names the user and the fix: {message}"
        );
        let rows = working_sidedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 8, "the conflict removed nothing");
    }

    #[tokio::test]
    async fn remove_sidedef_with_cascade_flips_the_shared_linedef() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "remove_sidedef",
            json!({"index": 2_i32, "cascade": true}),
        )
        .await;
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([2_i32]),
            "the step line's front goes"
        );
        assert_eq!(
            result["removed"]["flipped_linedefs"],
            json!([2_i32]),
            "linedef 2 flipped to its back side"
        );
        assert_eq!(
            result["removed"]["one_sided_linedefs"],
            json!([2_i32]),
            "and closed into one side"
        );
        assert_eq!(result["removed"]["linedefs"], json!([]), "no line died");
        let rows = working_sidedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "one sidedef fewer");
        let shared = working_linedef(&client, "MAP01", 2).await;
        assert_eq!(shared["v1"]["index"], json!(1_i32), "the ends swapped");
        assert_eq!(shared["v2"]["index"], json!(4_i32));
        assert_eq!(
            shared["front"]["sidedef"],
            json!(2_i32),
            "old back, renumbered"
        );
        assert_eq!(
            shared["front"]["sector"],
            json!(1_i32),
            "facing the raised room"
        );
        assert_eq!(shared["back"], Value::Null, "the line is one-sided");
        assert_eq!(
            shared["flag_names"],
            json!(["blocking"]),
            "two-sided cleared"
        );
    }

    #[tokio::test]
    async fn sidedef_edits_undo_and_redo_as_one_step_each() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "add_sidedef",
            json!({"sector": 1_i32, "middle": "TEMP"}),
        )
        .await;
        call(
            &client,
            "edit_sidedef",
            json!({"index": 8_i32, "lower": "STEP1"}),
        )
        .await;
        let grown = working_sidedefs(&client, "MAP01").await;
        assert_eq!(grown[8]["middle"], json!("TEMP"), "the add landed");
        assert_eq!(grown[8]["lower"], json!("STEP1"), "and the edit on top");
        let undo_edit = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo_edit["undone"],
            json!(["edit 1 sidedefs"]),
            "the edit rolls back alone"
        );
        let half_undone = working_sidedefs(&client, "MAP01").await;
        assert_eq!(half_undone[8]["lower"], json!("-"), "the edit is gone");
        assert_eq!(
            half_undone[8]["middle"],
            json!("TEMP"),
            "the add survives it"
        );
        let undo_add = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo_add["undone"],
            json!(["add sidedef for sector 1"]),
            "and so is the add, copy included"
        );
        let merged = call(&client, "assemble_map", json!({"map": "MAP01"})).await;
        let base_rows = merged["sidedefs"].as_array().expect("an array");
        assert_eq!(base_rows.len(), 8, "back to the fixture's eight");
        let redo = call(&client, "redo", json!({ "steps": 2_u64 })).await;
        assert_eq!(
            redo["redone"],
            json!(["add sidedef for sector 1", "edit 1 sidedefs"]),
            "both steps replay in order"
        );
        let redone = working_sidedefs(&client, "MAP01").await;
        assert_eq!(redone.len(), 9, "the redone add is back");
        assert_eq!(redone[8]["lower"], json!("STEP1"), "with the redone edit");
    }
}
