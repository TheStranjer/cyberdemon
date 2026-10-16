//! The `edit_things` tool group as seen by a real MCP client: `add_thing`
//! with its family defaults, named flags and angle normalisation, the
//! `Doom`-family and binary range refusals, `edit_thing` over single
//! indices and lists with absolute and delta parameters, flag adjustment,
//! the conflict refusals, and `remove_thing` with the remaining
//! renumbering verified through `assemble_map`.

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

    /// The working archive's thing rows for `map`, in order, through
    /// `assemble_map`.
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

    /// The thing types of the working rows, in index order.
    fn types_of(rows: &[Value]) -> Vec<i64> {
        rows.iter()
            .map(|row| row["type"].as_i64().expect("a numeric type"))
            .collect()
    }

    /// Requires `tool` to reject `args` with `invalid_argument`.
    async fn refuse(client: &Client, tool: &str, args: Value) {
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {tool}");
    }

    /// Requires `add_thing` to reject `args` with the documented
    /// `Doom`-family field message.
    async fn refuse_doom_field(client: &Client, args: Value) {
        let (code, message) = call_err(client, "add_thing", args).await;
        assert_eq!(code, "invalid_argument", "family refusal code");
        assert!(
            message
                .contains("Doom-format things have no tid/z/special/args; use a Hexen or UDMF map"),
            "the message names the fix: {message}"
        );
    }

    /// Adds a thing with every `Hexen`-only field to `map` and requires it
    /// stored with the wide default flags.
    async fn add_wide_thing(client: &Client, map: &str) {
        let result = call(
            client,
            "add_thing",
            json!({"map": map, "type": 5_i32, "x": 64_i32, "y": 64_i32, "z": 32_i32,
                   "tid": 7_i32, "special": 1_i32, "args": [1_i32, 2_i32, 3_i32, 4_i32, 5_i32]}),
        )
        .await;
        assert_eq!(result["map"], json!(map), "the named map was edited");
        assert_eq!(
            result["thing"]["flags"],
            json!(2_023_i32),
            "0x7E7: every skill, class and mode on {map}"
        );
        assert_eq!(
            result["thing"]["flag_names"],
            json!([
                "easy", "medium", "hard", "class1", "class2", "class3", "single", "coop", "dm"
            ]),
            "the default reads back by name"
        );
        assert_eq!(result["thing"]["tid"], json!(7_i32), "the tid lands");
        assert_eq!(result["thing"]["z"], json!(32_i32), "and the height");
        assert_eq!(
            result["thing"]["args"],
            json!([1_i32, 2_i32, 3_i32, 4_i32, 5_i32]),
            "and the script arguments"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "{map} keeps its nodes"
        );
    }

    #[tokio::test]
    async fn add_thing_defaults_to_all_skill_flags_and_keeps_the_nodes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_thing",
            json!({"type": 3004_i32, "x": -128_i32, "y": 64_i32, "angle": 450_i32}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["index"], json!(2_i32), "appended after the two");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "things are not geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(result["thing"]["flags"], json!(7_i32), "the Doom default");
        assert_eq!(
            result["thing"]["flag_names"],
            json!(["easy", "medium", "hard"]),
            "named back"
        );
        assert_eq!(
            result["thing"]["angle"],
            json!(90_i32),
            "450 normalises to 90"
        );
        assert_eq!(result["thing"]["x"], json!(-128_i32), "the position stands");
        assert_eq!(
            result["thing"]["tid"],
            json!(0_i32),
            "and the rest defaults"
        );
        let rows = working_things(&client, "MAP01").await;
        assert_eq!(
            types_of(&rows),
            vec![1_i64, 3_001, 3_004],
            "the map holds three things now"
        );
        assert_eq!(rows[2]["angle"], json!(90_i32), "written and read back");
    }

    #[tokio::test]
    async fn add_thing_resolves_named_flags() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_thing",
            json!({"type": 2035_i32, "x": 0_i32, "y": 0_i32, "flags": ["ambush", "notdm"]}),
        )
        .await;
        assert_eq!(result["thing"]["flags"], json!(40_i32), "8 | 32");
        assert_eq!(
            result["thing"]["flag_names"],
            json!(["ambush", "notdm"]),
            "in table order"
        );
    }

    #[tokio::test]
    async fn add_thing_refuses_hexen_only_fields_on_a_doom_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let landed = call(
            &client,
            "add_thing",
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32}),
        )
        .await;
        assert_eq!(landed["index"], json!(2_i32), "the plain add lands");
        refuse_doom_field(
            &client,
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32, "tid": 5_i32}),
        )
        .await;
        refuse_doom_field(
            &client,
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32, "z": 8_i32}),
        )
        .await;
        refuse_doom_field(
            &client,
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32, "special": 1_i32}),
        )
        .await;
        refuse_doom_field(
            &client,
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32, "args": [0_i32, 0_i32, 0_i32, 0_i32, 1_i32]}),
        )
        .await;
        let rows = working_things(&client, "MAP01").await;
        assert_eq!(
            types_of(&rows),
            vec![1_i64, 3_001, 1],
            "only the plain add stayed"
        );
    }

    #[tokio::test]
    async fn add_thing_on_hexen_family_maps_carries_the_fields_and_the_wide_default() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        add_wide_thing(&client, "MAP03").await;
        add_wide_thing(&client, "MAP04").await;
        let udmf_rows = working_things(&client, "MAP03").await;
        let binary_rows = working_things(&client, "MAP04").await;
        let udmf_last = udmf_rows.last().expect("the added UDMF thing");
        let binary_last = binary_rows.last().expect("the added Hexen thing");
        assert_eq!(udmf_last["tid"], json!(7_i32), "read back from MAP03");
        assert_eq!(binary_last["z"], json!(32_i32), "read back from MAP04");
    }

    #[tokio::test]
    async fn add_thing_range_checks_the_binary_record_fields() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        refuse(
            &client,
            "add_thing",
            json!({"map": "MAP04", "type": 70_000_i32, "x": 0_i32, "y": 0_i32}),
        )
        .await;
        refuse(
            &client,
            "add_thing",
            json!({"map": "MAP04", "type": 5_i32, "x": 0x8000_i32, "y": 0_i32}),
        )
        .await;
        refuse(
            &client,
            "add_thing",
            json!({"map": "MAP04", "type": 5_i32, "x": 0_i32, "y": 0_i32, "special": 300_i32}),
        )
        .await;
        refuse(
            &client,
            "add_thing",
            json!({"map": "MAP04", "type": 5_i32, "x": 0_i32, "y": 0_i32, "args": [0_i32, 0_i32, 256_i32, 0_i32, 0_i32]}),
        )
        .await;
        let wide = call(
            &client,
            "add_thing",
            json!({"map": "MAP03", "type": 70_000_i32, "x": 0x8000_i32, "y": 0_i32}),
        )
        .await;
        assert_eq!(
            wide["thing"]["x"],
            json!(0x8000_i32),
            "UDMF stores the wide value"
        );
        assert_eq!(
            wide["thing"]["type"],
            json!(70_000_i32),
            "and the wide type"
        );
    }

    #[tokio::test]
    async fn add_thing_names_unknown_flags_and_reports_unknown_maps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "add_thing",
            json!({"type": 1_i32, "x": 0_i32, "y": 0_i32, "flags": ["flier"]}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown flag name");
        assert!(
            message.contains("'flier'") && message.contains("valid names are"),
            "the error lists the table: {message}"
        );
        let (missing_code, _) = call_err(
            &client,
            "add_thing",
            json!({"map": "MAP09", "type": 1_i32, "x": 0_i32, "y": 0_i32}),
        )
        .await;
        assert_eq!(missing_code, "not_found", "unknown map");
    }

    #[tokio::test]
    async fn edit_thing_moves_a_list_with_deltas() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_thing",
            json!({"index": [1_i32, 0_i32], "dx": 16_i32, "dy": 16_i32}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the default map");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "things are not geometry"
        );
        let rows = result["things"].as_array().expect("a things array");
        assert_eq!(rows.len(), 2, "both things moved");
        assert_eq!(rows[0]["index"], json!(0_i32), "the rows come sorted");
        assert_eq!(rows[0]["x"], json!(80_i32), "64 plus 16");
        assert_eq!(rows[0]["y"], json!(144_i32), "128 plus 16");
        assert_eq!(rows[1]["x"], json!(400_i32), "384 plus 16");
        let stored = working_things(&client, "MAP01").await;
        assert_eq!(stored[1]["x"], json!(400_i32), "and the map holds it");
    }

    #[tokio::test]
    async fn edit_thing_adjusts_flags_and_applies_absolute_values() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "set_flags": ["ambush"], "clear_flags": ["easy"],
                   "angle_delta": -90_i32}),
        )
        .await;
        let first = &result["things"][0];
        assert_eq!(first["flags"], json!(14_i32), "6 with ambush on");
        assert_eq!(
            first["flag_names"],
            json!(["medium", "hard", "ambush"]),
            "named in table order"
        );
        assert_eq!(first["angle"], json!(270_i32), "-90 wraps into 0..=359");
        let replaced = call(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "flags": ["hard"], "type": 2_i32, "x": 96_i32, "y": 128_i32,
                   "angle": 450_i32}),
        )
        .await;
        let row = &replaced["things"][0];
        assert_eq!(row["flags"], json!(4_i32), "the bitset replaced everything");
        assert_eq!(row["type"], json!(2_i32), "the type changed too");
        assert_eq!(row["x"], json!(96_i32), "absolute placement");
        assert_eq!(row["y"], json!(128_i32), "on both axes");
        assert_eq!(row["angle"], json!(90_i32), "and the normalised angle");
    }

    #[tokio::test]
    async fn edit_thing_refuses_conflicting_and_empty_requests() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "x": 8_i32, "dx": 8_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "y": 8_i32, "dy": 8_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "angle": 90_i32, "angle_delta": 90_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "flags": 3_i32, "set_flags": ["easy"]}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": 0_i32, "flags": 3_i32, "clear_flags": ["easy"]}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": [0_i32, 1_i32], "x": 8_i32}),
        )
        .await;
        refuse(
            &client,
            "edit_thing",
            json!({"index": [0_i32, 1_i32], "y": 8_i32}),
        )
        .await;
        refuse(&client, "edit_thing", json!({"index": 0_i32})).await;
        let (code, message) =
            call_err(&client, "edit_thing", json!({"index": 0_i32, "z": 8_i32})).await;
        assert_eq!(code, "invalid_argument", "a height refuses on a Doom map");
        assert!(
            message.contains("Doom-format things have no tid/z/special/args"),
            "the same family rule as add: {message}"
        );
        let (missing_code, _) =
            call_err(&client, "edit_thing", json!({"index": 99_i32, "dx": 1_i32})).await;
        assert_eq!(
            missing_code, "not_found",
            "an index past the end is missing"
        );
    }

    #[tokio::test]
    async fn remove_thing_lists_and_renumbers_the_survivors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "add_thing",
            json!({"type": 3004_i32, "x": 16_i32, "y": 16_i32}),
        )
        .await;
        let result = call(&client, "remove_thing", json!({"index": [2_i32, 0_i32]})).await;
        assert_eq!(
            result["removed"],
            json!([0_i32, 2_i32]),
            "original indices, sorted"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "nodes stay trusted"
        );
        assert_eq!(result["map"], json!("MAP01"), "the default map");
        let rows = working_things(&client, "MAP01").await;
        assert_eq!(types_of(&rows), vec![3_001_i64], "only the imp survives");
        assert_eq!(rows[0]["index"], json!(0_i32), "renumbered down to zero");
        let (missing_code, _) = call_err(&client, "remove_thing", json!({"index": 5_i32})).await;
        assert_eq!(
            missing_code, "not_found",
            "an index past the end is missing"
        );
        let (empty_code, _) = call_err(&client, "remove_thing", json!({"index": []})).await;
        assert_eq!(empty_code, "invalid_argument", "an empty list refuses");
    }
}
