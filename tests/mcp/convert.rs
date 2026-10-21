//! The `convert` tool group as seen by a real MCP client.
//!
//! `convert_map_format` rewriting a binary `Doom` block as `UDMF` and back
//! (`list_maps` follows the format, `get_map_summary` counts stay), the
//! `Hexen` fixture map going to `UDMF` `zdoom` and home with its `BEHAVIOR`
//! intact, a `UDMF` block losing its `ZNODES` and `GL` run, what a lossy
//! `UDMF` map reports when it becomes binary, `Line_SetIdentification`
//! travelling through the `UDMF` line id, the cross-family `Doom` ↔ `Hexen`
//! translations in both directions (the table trips into `UDMF` and into
//! one-byte `Hexen` records, a row's activation arriving as `flag_names`,
//! an action too wide for `Hexen` bytes clearing with an `untranslated`
//! entry and its own warning, ids restored as Doom tags, specials with no
//! row reported `untranslated`), `dry_run` changing neither archive nor
//! history and reporting the numbers the real step writes, one `undo`
//! taking a whole conversion back, and every refusal: already-there,
//! unknown map and malformed parameters.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad_conversions, pwad_map_formats, two_room_map};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The hint every conversion carries.
    const NODES_HINT: &str = "run build_nodes";

    /// The fixture map's element counts, as `get_map_summary` reports them.
    const TWO_ROOM_COUNTS: &str = "{\"things\": 2, \"vertices\": 6, \"linedefs\": 7, \
                                  \"sidedefs\": 8, \"sectors\": 2}";

    /// Nothing dropped and nothing rounded.
    const NO_DROPS: &str = "{\"things\": 0, \"linedefs\": 0, \"sidedefs\": 0, \
                            \"sectors\": 0, \"vertices\": 0}";

    /// `MAP05`'s counts, as `get_map_summary` reports them.
    const LOSSY_COUNTS: &str = "{\"things\": 1, \"vertices\": 4, \"linedefs\": 4, \
                               \"sidedefs\": 1, \"sectors\": 1}";

    /// `MAP05`'s unmodelled fields and preserved fractions per kind.
    const LOSSY_DROPS: &str = "{\"things\": 3, \"linedefs\": 1, \"sidedefs\": 2, \
                               \"sectors\": 2, \"vertices\": 3}";

    /// `MAP05`'s elements whose `extra` held a rounded number, per kind.
    const LOSSY_ROUNDED: &str = "{\"things\": 1, \"linedefs\": 0, \"sidedefs\": 1, \
                                 \"sectors\": 1, \"vertices\": 3}";

    /// The ten vanilla members of a binary `Doom` block, in directory order.
    const DOOM_MEMBERS: [&str; 10] = [
        "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS",
        "REJECT", "BLOCKMAP",
    ];

    /// One of the JSON rows above, parsed.
    fn row_of(text: &str) -> Value {
        serde_json::from_str(text).expect("the fixture rows are JSON")
    }

    /// A connected client with `path` loaded as its base layer.
    async fn loaded(path: &Path) -> Client {
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({ "path": path.display().to_string() }),
        )
        .await;
        client
    }

    /// One `convert_map_format` call.
    async fn convert(client: &Client, args: Value) -> Value {
        call(client, "convert_map_format", args).await
    }

    /// One `convert_map_format` refusal on `map`, as `(code, message)`.
    async fn refusal(client: &Client, map: &str, mut request: Value) -> (String, String) {
        request["map"] = json!(map);
        let (code, message, _) = call_err(client, "convert_map_format", request).await;
        (code, message)
    }

    /// The `list_maps` row of `name` in `scope`.
    async fn map_row(client: &Client, name: &str, scope: &str) -> Value {
        let listed = call(client, "list_maps", json!({ "scope": scope })).await;
        let rows = listed
            .get("maps")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no maps array in {listed}"));
        rows.iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            .cloned()
            .unwrap_or_else(|| panic!("{name} is not listed in the {scope} scope: {listed}"))
    }

    /// The block's member lump names, from `get_map_lumps`.
    async fn lump_names(client: &Client, name: &str, scope: &str) -> Vec<String> {
        let lumps = call(
            client,
            "get_map_lumps",
            json!({ "map": name, "scope": scope }),
        )
        .await;
        let rows = lumps
            .get("lumps")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no lumps array in {lumps}"));
        rows.iter()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    /// The five element counts of a `get_map_summary` result.
    async fn counts(client: &Client, name: &str, scope: &str) -> Value {
        let summary = call(
            client,
            "get_map_summary",
            json!({ "map": name, "scope": scope }),
        )
        .await;
        summary.get("counts").cloned().unwrap_or(Value::Null)
    }

    /// The undo labels a `get_session_info` result reports, oldest first.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", json!({})).await;
        info.get("undo")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The working scope's map count, which refusals and dry runs leave empty.
    async fn working_maps(client: &Client) -> usize {
        let listed = call(client, "list_maps", json!({ "scope": "working" })).await;
        listed
            .get("maps")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default()
    }

    /// The `assemble_map` linedef rows of one block.
    async fn linedef_rows(client: &Client, name: &str, scope: &str) -> Vec<Value> {
        let graph = call(
            client,
            "assemble_map",
            json!({ "map": name, "scope": scope, "include": ["linedefs"] }),
        )
        .await;
        graph
            .get("linedefs")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no linedefs in {graph}"))
            .clone()
    }

    /// The `assemble_map` linedef row of index `index` in one block.
    async fn linedef_row(client: &Client, name: &str, scope: &str, index: i32) -> Value {
        let wanted = json!(index);
        linedef_rows(client, name, scope)
            .await
            .into_iter()
            .find(|line| line["index"] == wanted)
            .unwrap_or_else(|| panic!("{name} has no linedef {index}"))
    }

    /// The first linedef row of `assemble_map` for one block.
    async fn first_linedef(client: &Client, name: &str, scope: &str) -> Value {
        let lines = linedef_rows(client, name, scope).await;
        lines
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("{name} has no linedef: {lines:?}"))
    }

    /// Whether one conversion result warns with a text containing `wanted`.
    fn warns(result: &Value, wanted: &str) -> bool {
        result["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("no warnings array in {result}"))
            .iter()
            .filter_map(Value::as_str)
            .any(|note| note.contains(wanted))
    }

    /// Whether the `flag_names` of one `assemble_map` row carry every
    /// name in `wanted`.
    fn flag_names_contain(row: &Value, wanted: &[&str]) -> bool {
        let names: Vec<&str> = row["flag_names"]
            .as_array()
            .unwrap_or_else(|| panic!("no flag_names in {row}"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        wanted.iter().all(|name| names.contains(name))
    }

    /// The four cross-family report fields of one conversion result, for
    /// comparing what a dry run says with what the real step did.
    fn cross_family(result: &Value) -> Value {
        json!({
            "translated_specials": result["translated_specials"],
            "untranslated": result["untranslated"],
            "thing_flags_changed": result["thing_flags_changed"],
            "sector_types_changed": result["sector_types_changed"],
        })
    }

    /// Asserts one refusal message says the map already is in that layout.
    fn assert_no_op(message: &str) {
        assert!(
            message.contains("already is a") && message.contains("nothing to convert"),
            "the message says what it is: {message}"
        );
    }

    #[tokio::test]
    async fn converting_map01_to_udmf_rewrites_the_block() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        let out = convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
        assert_eq!(out["from"], "doom", "read as binary Doom");
        assert_eq!(out["to"], "udmf", "written as UDMF");
        assert_eq!(
            out["namespace"], "doom",
            "a Doom family source defaults to the doom namespace"
        );
        assert_eq!(out["nodes_removed"], true, "the node data went");
        assert_eq!(out["dry_run"], false, "a real run says so");
        assert_eq!(out["dropped_fields"], row_of(NO_DROPS), "nothing was lost");
        assert_eq!(
            out["fractional_coordinates"],
            row_of(NO_DROPS),
            "the binary source had no fractions"
        );
        assert_eq!(out["translated_specials"], 0_i32, "a same-family step");
        assert_eq!(out["untranslated"], json!([]), "translates nothing");
        assert!(
            warns(&out, NODES_HINT),
            "the result suggests rebuilding nodes: {out}"
        );
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(row["format"], "udmf", "list_maps follows the format");
        assert_eq!(row["udmf_namespace"], "doom", "and the namespace");
        assert_eq!(row["has_nodes"], false, "no node data survived");
        assert_eq!(
            lump_names(&client, "MAP01", "merged").await,
            vec!["TEXTMAP", "ENDMAP"],
            "the block is now TEXTMAP plus ENDMAP"
        );
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            row_of(TWO_ROOM_COUNTS),
            "the element counts are unchanged"
        );
    }

    #[tokio::test]
    async fn converting_a_udmf_block_back_to_doom_restores_the_members() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
        let back = convert(&client, json!({ "map": "MAP01", "to": "doom" })).await;
        assert_eq!(back["from"], "udmf", "read back as UDMF");
        assert_eq!(back["to"], "doom", "written as binary Doom");
        assert_eq!(
            back["namespace"],
            Value::Null,
            "a binary layout declares no namespace"
        );
        assert_eq!(
            back["dropped_fields"],
            row_of(NO_DROPS),
            "the round trip had nothing unmodelled to drop"
        );
        assert_eq!(
            lump_names(&client, "MAP01", "merged").await,
            DOOM_MEMBERS.to_vec(),
            "the ten vanilla members are back"
        );
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            row_of(TWO_ROOM_COUNTS),
            "and the counts still match"
        );
    }

    #[tokio::test]
    async fn a_doom_door_travels_through_the_zdoom_table_and_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        call(
            &client,
            "edit_linedef",
            json!({ "map": "MAP01", "index": 2_i32, "special": 1_i32, "tag": 0_i32 }),
        )
        .await;
        let out = convert(
            &client,
            json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
        )
        .await;
        assert_eq!(out["from"], "doom", "a Doom family source");
        assert_eq!(out["namespace"], "zdoom", "into a Hexen family namespace");
        assert_eq!(
            out["translated_specials"], 1_i32,
            "the door line rode the table"
        );
        assert_eq!(out["untranslated"], json!([]), "row 1 translates");
        assert_eq!(
            out["thing_flags_changed"], 2_i32,
            "both things moved families"
        );
        assert_eq!(out["sector_types_changed"], 0_i32, "plain sectors stand");
        let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
        assert_eq!(door["special"], 12_i32, "Door_Raise");
        assert!(
            flag_names_contain(&door, &["playeruse", "repeatspecial", "monsteractivate"]),
            "row 1's USE|MONST|REP activation: {door}"
        );
        assert_eq!(
            door["args"],
            json!([0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
            "the row's template"
        );
        let back = convert(&client, json!({ "map": "MAP01", "to": "doom" })).await;
        assert_eq!(back["from"], "udmf", "read as zdoom UDMF");
        assert_eq!(back["to"], "doom", "written as binary Doom");
        assert_eq!(
            back["translated_specials"], 1_i32,
            "the same line came back through the table"
        );
        assert_eq!(back["untranslated"], json!([]), "and matched a row");
        let names = lump_names(&client, "MAP01", "merged").await;
        assert_eq!(names, DOOM_MEMBERS.to_vec(), "the block is binary Doom");
        let home = linedef_row(&client, "MAP01", "merged", 2_i32).await;
        assert_eq!(home["special"], 1_i32, "special 1 again");
        assert_eq!(home["tag"], 0_i32, "with its tag restored in the tag field");
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            row_of(TWO_ROOM_COUNTS),
            "and the same elements throughout"
        );
    }

    #[tokio::test]
    async fn a_tagged_door_line_becomes_a_binary_hexen_generalized_action() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        call(
            &client,
            "edit_linedef",
            json!({ "map": "MAP01", "index": 2_i32, "special": 26_i32, "tag": 3_i32 }),
        )
        .await;
        let out = convert(&client, json!({ "map": "MAP01", "to": "hexen" })).await;
        assert_eq!(out["from"], "doom", "a Doom family source");
        assert_eq!(out["to"], "hexen", "written as binary Hexen");
        assert_eq!(
            out["namespace"],
            Value::Null,
            "a binary layout declares none"
        );
        assert_eq!(out["translated_specials"], 1_i32, "row 26 rode the table");
        assert_eq!(
            out["untranslated"],
            json!([]),
            "and every value fits a byte"
        );
        assert_eq!(
            out["thing_flags_changed"], 2_i32,
            "both things moved families"
        );
        assert_eq!(out["sector_types_changed"], 0_i32, "plain sectors stand");
        let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
        assert_eq!(door["special"], 13_i32, "Door_LockedRaise");
        assert_eq!(
            door["args"],
            json!([0_i32, 16_i32, 150_i32, 130_i32, 3_i32]),
            "the row's template with the tag in its slot"
        );
        assert_eq!(door["tag"], 0_i32, "binary Hexen models no line tag");
        assert!(
            flag_names_contain(&door, &["playeruse", "repeatspecial"]),
            "row 26's USE|REP activation: {door}"
        );
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(row["format"], "hexen", "the block reads as binary Hexen");
        let names = lump_names(&client, "MAP01", "working").await;
        assert!(
            names.contains(&"BEHAVIOR".to_owned()),
            "the Hexen writer synthesized BEHAVIOR: {names:?}"
        );
    }

    #[tokio::test]
    async fn an_action_too_wide_for_hexen_bytes_clears_but_survives_udmf() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let doomed = loaded(&iwad(dir.path())).await;
        call(
            &doomed,
            "edit_linedef",
            json!({ "map": "MAP01", "index": 2_i32, "special": 100_i32 }),
        )
        .await;
        let out = convert(&doomed, json!({ "map": "MAP01", "to": "hexen" })).await;
        assert_eq!(
            out["translated_specials"], 0_i32,
            "row 100's 273 is no byte"
        );
        assert_eq!(
            out["untranslated"],
            json!([{"linedef": 2_i32, "special": 100_i32}]),
            "the line and its lost special are named"
        );
        assert!(
            warns(&out, "linedef 2 special 100 was set to 0")
                && warns(&out, "Stairs_BuildUpDoomCrush")
                && warns(&out, "convert to a UDMF Hexen-family map"),
            "the line is told its own action is the one no byte can hold: {out}"
        );
        let cleared = linedef_row(&doomed, "MAP01", "working", 2_i32).await;
        assert_eq!(cleared["special"], 0_i32, "the line ends special 0");
        let second = tempfile::tempdir().expect("a second scratch directory");
        let zdoom = loaded(&iwad(second.path())).await;
        call(
            &zdoom,
            "edit_linedef",
            json!({ "map": "MAP01", "index": 2_i32, "special": 100_i32 }),
        )
        .await;
        let there = convert(
            &zdoom,
            json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
        )
        .await;
        assert_eq!(there["translated_specials"], 1_i32, "a UDMF target fits");
        assert_eq!(there["untranslated"], json!([]), "and nothing is lost");
        let kept = linedef_row(&zdoom, "MAP01", "working", 2_i32).await;
        assert_eq!(kept["special"], 273_i32, "the action stands intact");
        assert_eq!(
            kept["args"],
            json!([0_i32, 32_i32, 16_i32, 0_i32, 0_i32]),
            "the row's template at tag 0"
        );
    }

    #[tokio::test]
    async fn a_dry_run_touches_neither_the_archive_nor_the_history() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        let out = convert(
            &client,
            json!({ "map": "MAP01", "to": "udmf", "namespace": "strife", "dry_run": true }),
        )
        .await;
        assert_eq!(out["dry_run"], true, "the call reports only");
        assert_eq!(
            out["namespace"], "strife",
            "and still resolves the namespace"
        );
        assert_eq!(
            out["nodes_removed"], true,
            "it would have removed the nodes"
        );
        assert_eq!(
            undo_labels(&client).await,
            Vec::<String>::new(),
            "a dry run adds no undo entry"
        );
        assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(row["format"], "doom", "the map is still binary Doom");
        let info = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            info["working"]["dirty"], false,
            "and the session is still clean"
        );
    }

    #[tokio::test]
    async fn a_forward_dry_run_reports_the_numbers_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        let wanted = json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" });
        let mut dry_request = wanted.clone();
        dry_request["dry_run"] = json!(true);
        let dry = convert(&client, dry_request).await;
        assert_eq!(dry["dry_run"], true, "the call reports only");
        assert_eq!(
            dry["namespace"], "zdoom",
            "and still resolves the namespace"
        );
        assert_eq!(
            cross_family(&dry),
            json!({
                "translated_specials": 0_i32,
                "untranslated": [],
                "thing_flags_changed": 2_i32,
                "sector_types_changed": 0_i32,
            }),
            "the forward stages ran on the copy"
        );
        assert!(warns(&dry, NODES_HINT), "the hint rides along: {dry}");
        assert_eq!(
            undo_labels(&client).await,
            Vec::<String>::new(),
            "a dry run adds no undo entry"
        );
        assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(row["format"], "doom", "the map is still binary Doom");
        let real = convert(&client, wanted).await;
        assert_eq!(cross_family(&real), cross_family(&dry), "the same numbers");
        assert_eq!(real["warnings"], dry["warnings"], "and the same notices");
    }

    #[tokio::test]
    async fn one_undo_takes_a_whole_conversion_step_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
        assert_eq!(
            undo_labels(&client).await,
            vec!["convert MAP01 to udmf"],
            "one step for the copy and the rewrite"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"][0], "convert MAP01 to udmf",
            "the label comes back"
        );
        assert_eq!(undone["remaining_undo"], 0_i32, "the stack is empty again");
        assert_eq!(undone["dirty"], false, "and the session is clean again");
        assert_eq!(
            working_maps(&client).await,
            0,
            "the copied block rolled back too"
        );
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(
            row["format"], "doom",
            "the merged view shows the layer's map again"
        );
    }

    #[tokio::test]
    async fn one_undo_takes_a_whole_forward_conversion_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        call(
            &client,
            "edit_linedef",
            json!({ "map": "MAP01", "index": 2_i32, "special": 1_i32 }),
        )
        .await;
        convert(
            &client,
            json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
        )
        .await;
        assert_eq!(
            undo_labels(&client).await,
            vec!["edit 1 linedefs", "convert MAP01 to udmf"],
            "one step for the copy, the translation and the rewrite"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"][0], "convert MAP01 to udmf",
            "the label comes back"
        );
        assert_eq!(undone["remaining_undo"], 1_i32, "the edit step stands");
        assert_eq!(undone["dirty"], true, "the edited block is still working");
        assert_eq!(working_maps(&client).await, 1, "and its block is copied in");
        let row = map_row(&client, "MAP01", "merged").await;
        assert_eq!(
            row["format"], "doom",
            "the source block came back under the translation"
        );
        let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
        assert_eq!(door["special"], 1_i32, "the pre-conversion edit survived");
        assert!(
            flag_names_contain(&door, &["twosided"])
                && !flag_names_contain(&door, &["playeruse", "monsteractivate"]),
            "the row's Hexen trigger bits went with the step: {door}"
        );
    }

    #[tokio::test]
    async fn converting_the_hexen_fixture_keeps_behavior_and_comes_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let out = convert(&client, json!({ "map": "MAP04", "to": "udmf" })).await;
        assert_eq!(out["from"], "hexen", "read as binary Hexen");
        assert_eq!(
            out["namespace"], "zdoom",
            "a Hexen family source defaults to zdoom"
        );
        assert_eq!(
            lump_names(&client, "MAP04", "working").await,
            vec!["TEXTMAP", "BEHAVIOR", "ENDMAP"],
            "the script lump rides along, the node lumps do not"
        );
        assert_eq!(
            counts(&client, "MAP04", "working").await,
            row_of(TWO_ROOM_COUNTS),
            "the elements came through"
        );
        let back = convert(&client, json!({ "map": "MAP04", "to": "hexen" })).await;
        assert_eq!(back["to"], "hexen", "and went back");
        let row = map_row(&client, "MAP04", "working").await;
        assert_eq!(row["format"], "hexen", "the block reads as Hexen again");
        assert_eq!(
            row["has_nodes"], false,
            "the empty NODES placeholder is no node data"
        );
        let names = lump_names(&client, "MAP04", "working").await;
        assert!(
            names.contains(&"BEHAVIOR".to_owned()),
            "BEHAVIOR survived: {names:?}"
        );
        assert!(
            names.contains(&"BLOCKMAP".to_owned()),
            "the vanilla members are back: {names:?}"
        );
        assert_eq!(
            counts(&client, "MAP04", "working").await,
            row_of(TWO_ROOM_COUNTS),
            "with the same counts"
        );
    }

    #[tokio::test]
    async fn a_udmf_conversion_drops_znodes_and_the_gl_run() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let before = map_row(&client, "MAP03", "merged").await;
        assert_eq!(before["format"], "udmf", "the fixture is UDMF");
        assert_eq!(before["has_gl_nodes"], true, "with an attached GL run");
        assert_eq!(before["has_nodes"], true, "and a ZNODES lump");
        let out = convert(&client, json!({ "map": "MAP03", "to": "hexen" })).await;
        assert_eq!(out["namespace"], Value::Null, "the target is binary");
        assert_eq!(out["from"], "udmf", "read as UDMF");
        let after = map_row(&client, "MAP03", "working").await;
        assert_eq!(after["format"], "hexen", "the block became binary Hexen");
        assert_eq!(after["has_gl_nodes"], false, "the GL run left the archive");
        assert_eq!(after["has_nodes"], false, "and so did the node data");
        let names = lump_names(&client, "MAP03", "working").await;
        assert!(
            !names
                .iter()
                .any(|lump| lump == "ZNODES" || lump.starts_with("GL_")),
            "no UDMF node carrier survived: {names:?}"
        );
        assert!(
            names.contains(&"BEHAVIOR".to_owned()),
            "the Hexen writer synthesized BEHAVIOR: {names:?}"
        );
        assert_eq!(
            counts(&client, "MAP03", "working").await,
            row_of(TWO_ROOM_COUNTS),
            "the counts are unchanged"
        );
    }

    #[tokio::test]
    async fn a_lossy_udmf_map_reports_what_a_binary_record_cannot_store() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_conversions(dir.path())).await;
        assert_eq!(
            counts(&client, "MAP05", "merged").await,
            row_of(LOSSY_COUNTS),
            "the fixture decodes"
        );
        let out = convert(&client, json!({ "map": "MAP05", "to": "doom" })).await;
        assert_eq!(
            out["dropped_fields"],
            row_of(LOSSY_DROPS),
            "every unmodelled field and preserved fraction is counted"
        );
        assert_eq!(
            out["fractional_coordinates"],
            row_of(LOSSY_ROUNDED),
            "only a modelled number's text counts as rounded: `alpha` and `scalex_top` \
             are unmodelled fields that simply drop"
        );
        assert!(
            warns(&out, "BEHAVIOR"),
            "a BEHAVIOR lump cannot stay in a binary Doom block: {out}"
        );
        assert_eq!(
            counts(&client, "MAP05", "working").await,
            row_of(LOSSY_COUNTS),
            "the rounded integers kept the element counts"
        );
        let row = map_row(&client, "MAP05", "working").await;
        assert_eq!(row["format"], "doom", "and the block reads as binary Doom");
    }

    #[tokio::test]
    async fn a_hexen_line_identification_travels_through_the_line_id() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_conversions(dir.path())).await;
        let before = first_linedef(&client, "MAP06", "merged").await;
        assert_eq!(before["special"], 121_i32, "the fixture line is 121");
        assert_eq!(before["args"][0], 7_i32, "carrying the id in args[0]");
        convert(&client, json!({ "map": "MAP06", "to": "udmf" })).await;
        let as_id = first_linedef(&client, "MAP06", "working").await;
        assert_eq!(as_id["tag"], 7_i32, "the id moved into the UDMF line id");
        assert_eq!(as_id["special"], 0_i32, "and the special went");
        convert(&client, json!({ "map": "MAP06", "to": "hexen" })).await;
        let again = first_linedef(&client, "MAP06", "working").await;
        assert_eq!(again["special"], 121_i32, "coming home it is 121 again");
        assert_eq!(again["args"][0], 7_i32, "with the id back in args[0]");
        assert_eq!(again["tag"], 0_i32, "and no model tag");
    }

    #[tokio::test]
    async fn a_hexen_map_converts_back_to_a_doom_family_layout() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_conversions(dir.path())).await;
        let out = convert(&client, json!({ "map": "MAP06", "to": "doom" })).await;
        assert_eq!(out["from"], "hexen", "read as binary Hexen");
        assert_eq!(out["to"], "doom", "written as binary Doom");
        assert_eq!(
            out["translated_specials"], 0_i32,
            "the id fold needs no table"
        );
        assert_eq!(
            out["untranslated"],
            json!([]),
            "nothing failed to translate"
        );
        assert_eq!(
            out["thing_flags_changed"], 1_i32,
            "the lone thing folds back"
        );
        assert_eq!(
            out["sector_types_changed"], 0_i32,
            "its one plain sector stands"
        );
        assert!(
            warns(&out, "BEHAVIOR"),
            "the generated BEHAVIOR cannot ride into a Doom block: {out}"
        );
        let line = first_linedef(&client, "MAP06", "working").await;
        assert_eq!(line["special"], 0_i32, "Line_SetIdentification folded away");
        assert_eq!(line["tag"], 7_i32, "its id came back as the Doom tag");
        assert_eq!(
            lump_names(&client, "MAP06", "working").await,
            DOOM_MEMBERS.to_vec(),
            "the block is a vanilla Doom run again"
        );
        let row = map_row(&client, "MAP06", "working").await;
        assert_eq!(row["format"], "doom", "and it reads back as Doom");
    }

    #[tokio::test]
    async fn a_zdoom_map_converts_back_to_binary_doom() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let out = convert(&client, json!({ "map": "MAP03", "to": "doom" })).await;
        assert_eq!(out["from"], "udmf", "read as zdoom UDMF");
        assert_eq!(out["to"], "doom", "written as binary Doom");
        assert_eq!(
            out["namespace"],
            Value::Null,
            "a binary target declares none"
        );
        assert_eq!(
            out["translated_specials"], 0_i32,
            "plain lines need no table"
        );
        assert_eq!(out["untranslated"], json!([]), "and report nothing");
        assert_eq!(
            out["thing_flags_changed"], 2_i32,
            "the things fold to Doom bits"
        );
        assert_eq!(out["nodes_removed"], true, "the ZNODES and GL run went");
        let row = map_row(&client, "MAP03", "working").await;
        assert_eq!(row["format"], "doom", "the block reads as Doom");
        assert_eq!(row["has_gl_nodes"], false, "and lost the GL run");
        assert_eq!(
            counts(&client, "MAP03", "working").await,
            row_of(TWO_ROOM_COUNTS),
            "with the same elements"
        );
    }

    #[tokio::test]
    async fn a_hexen_map_converts_to_a_doom_family_namespace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let wanted = json!({ "map": "MAP04", "to": "udmf", "namespace": "heretic" });
        let out = convert(&client, wanted).await;
        assert_eq!(out["from"], "hexen", "read as binary Hexen");
        assert_eq!(out["to"], "udmf", "written as UDMF");
        assert_eq!(
            out["namespace"], "heretic",
            "a best-effort Doom family spelling"
        );
        assert_eq!(
            out["translated_specials"], 0_i32,
            "plain lines need no table"
        );
        assert_eq!(
            out["thing_flags_changed"], 2_i32,
            "the things fold to Doom bits"
        );
        let names = lump_names(&client, "MAP04", "working").await;
        assert!(
            names.contains(&"BEHAVIOR".to_owned()),
            "a UDMF target keeps the script lump: {names:?}"
        );
        let row = map_row(&client, "MAP04", "working").await;
        assert_eq!(row["format"], "udmf", "the block reads as UDMF");
        assert_eq!(
            row["udmf_namespace"], "heretic",
            "and declares the named namespace"
        );
        assert_eq!(
            counts(&client, "MAP04", "working").await,
            row_of(TWO_ROOM_COUNTS),
            "with the same elements"
        );
    }

    #[tokio::test]
    async fn a_zdoom_special_with_no_row_comes_back_untranslated() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        call(
            &client,
            "edit_linedef",
            json!({ "map": "MAP03", "index": 0_i32, "special": 999_i32 }),
        )
        .await;
        let out = convert(&client, json!({ "map": "MAP03", "to": "doom" })).await;
        assert_eq!(
            out["untranslated"],
            json!([{"linedef": 0_i32, "special": 999_i32}]),
            "the line and its lost special are named"
        );
        assert_eq!(out["translated_specials"], 0_i32, "nothing made the trip");
        assert!(
            warns(&out, "no Doom translation"),
            "the grouped warning follows: {out}"
        );
        let line = first_linedef(&client, "MAP03", "working").await;
        assert_eq!(line["special"], 0_i32, "and the line ends special 0");
    }

    #[tokio::test]
    async fn a_reverse_dry_run_touches_neither_the_archive_nor_the_history() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let out = convert(
            &client,
            json!({ "map": "MAP04", "to": "doom", "dry_run": true }),
        )
        .await;
        assert_eq!(out["dry_run"], true, "the call reports only");
        assert_eq!(out["from"], "hexen", "read as binary Hexen");
        assert_eq!(out["to"], "doom", "would write binary Doom");
        assert_eq!(
            out["thing_flags_changed"], 2_i32,
            "and still counts the translation"
        );
        assert!(
            warns(&out, "BEHAVIOR") && warns(&out, NODES_HINT),
            "the losses it reports: {out}"
        );
        assert_eq!(
            undo_labels(&client).await,
            Vec::<String>::new(),
            "a dry run adds no undo entry"
        );
        assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
        let row = map_row(&client, "MAP04", "merged").await;
        assert_eq!(row["format"], "hexen", "the map is still binary Hexen");
    }

    #[tokio::test]
    async fn one_undo_takes_a_whole_reverse_conversion_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        convert(&client, json!({ "map": "MAP04", "to": "doom" })).await;
        assert_eq!(
            undo_labels(&client).await,
            vec!["convert MAP04 to doom"],
            "one step for the copy, the translation and the rewrite"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"][0], "convert MAP04 to doom",
            "the label comes back"
        );
        assert_eq!(undone["remaining_undo"], 0_i32, "the stack is empty again");
        assert_eq!(
            working_maps(&client).await,
            0,
            "the copied block rolled back too"
        );
        let row = map_row(&client, "MAP04", "merged").await;
        assert_eq!(
            row["format"], "hexen",
            "the merged view shows the layer's Hexen map again"
        );
    }

    #[tokio::test]
    async fn a_no_op_request_names_the_layout_in_hand() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&pwad_map_formats(dir.path())).await;
        let wanted = json!({ "to": "udmf", "namespace": "ZDoom" });
        let same_namespace = refusal(&client, "MAP03", wanted).await;
        assert_eq!(
            same_namespace.0, "invalid_argument",
            "the same namespace, any case"
        );
        assert_no_op(&same_namespace.1);
        let defaulted = refusal(&client, "MAP03", json!({ "to": "udmf" })).await;
        assert_eq!(
            defaulted.0, "invalid_argument",
            "the family default names zdoom"
        );
        assert_no_op(&defaulted.1);
        assert!(
            defaulted.1.contains("zdoom"),
            "a no-op UDMF label carries the namespace: {}",
            defaulted.1
        );
        let same_binary = refusal(&client, "MAP04", json!({ "to": "hexen" })).await;
        assert_eq!(same_binary.0, "invalid_argument", "the layout in hand");
        assert_no_op(&same_binary.1);
        assert_eq!(
            undo_labels(&client).await,
            Vec::<String>::new(),
            "a refusal enters no history entry"
        );
    }

    #[tokio::test]
    async fn bad_parameters_and_unknown_maps_are_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = loaded(&iwad(dir.path())).await;
        let unknown_map = refusal(&client, "MAP09", json!({ "to": "udmf" })).await;
        assert_eq!(unknown_map.0, "not_found", "no MAP09 here");
        let bad_format = refusal(&client, "MAP01", json!({ "to": "zmap" })).await;
        assert_eq!(bad_format.0, "invalid_argument", "zmap is no format");
        assert!(
            bad_format.1.contains("\"udmf\""),
            "the choices are listed: {}",
            bad_format.1
        );
        let namespace_args = json!({ "to": "hexen", "namespace": "zdoom" });
        let namespace_on_binary = refusal(&client, "MAP01", namespace_args).await;
        assert_eq!(
            namespace_on_binary.0, "invalid_argument",
            "a binary target declares no namespace"
        );
        let not_a_namespace = json!({ "to": "udmf", "namespace": "not a name" });
        let bad_namespace = refusal(&client, "MAP01", not_a_namespace).await;
        assert_eq!(
            bad_namespace.0, "invalid_argument",
            "spaces are no namespace"
        );
        let oversized_name = json!({ "map": "too long a name", "to": "udmf" });
        let bad_name = call_err(&client, "convert_map_format", oversized_name).await;
        assert_eq!(bad_name.0, "invalid_argument", "twelve characters is over");
        assert_eq!(
            undo_labels(&client).await,
            Vec::<String>::new(),
            "no refusal entered the history"
        );
    }

    #[tokio::test]
    async fn the_fixture_counts_match_the_library_model() {
        let map = two_room_map();
        assert_eq!(map.things.len(), 2_usize, "the fixture has two things");
        assert_eq!(map.vertices.len(), 6_usize, "and six vertices");
        assert_eq!(map.linedefs.len(), 7_usize, "seven linedefs");
        assert_eq!(map.sidedefs.len(), 8_usize, "eight sidedefs");
        assert_eq!(map.sectors.len(), 2_usize, "two sectors");
    }
}
