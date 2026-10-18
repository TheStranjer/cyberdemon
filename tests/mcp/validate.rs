//! The `validate` tool group as seen by a real MCP client:
//! `validate_map`'s report over the fixtures, its breakage detection
//! through the editing tools, the severity filter, the code selection, the
//! paging limit, the definition awareness of loaded archives, the geometry
//! and resource checks and the node-lump warnings around `build_all_nodes`.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::map::doom::write_doom_map;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{
        iwad, iwad_broken_map, iwad_heretic, pwad_custom_defs, pwad_map_formats, two_room_map,
    };
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Load the two-room `IWAD` fixture as layer 0.
    async fn plain_iwad(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        client
    }

    /// The `code` strings of a report's issues, in report order.
    fn issue_codes(report: &Value) -> Vec<String> {
        report["issues"]
            .as_array()
            .expect("an issue list")
            .iter()
            .map(|issue| {
                issue["code"]
                    .as_str()
                    .expect("every issue carries its code")
                    .to_owned()
            })
            .collect()
    }

    /// The issue of this code, panicking when the report has none.
    fn issue<'report>(report: &'report Value, code: &str) -> &'report Value {
        report["issues"]
            .as_array()
            .expect("an issue list")
            .iter()
            .find(|row| row["code"] == json!(code))
            .unwrap_or_else(|| panic!("no {code} issue in {report}"))
    }

    /// Delete the player 1 start of `MAP01` through the editing tool.
    async fn drop_the_start(client: &Client) {
        call(
            client,
            "remove_thing",
            json!({ "map": "MAP01", "index": 0_i32 }),
        )
        .await;
    }

    /// The generated-lump codes among `codes`, for the build before/after
    /// pair of tests.
    fn lump_codes(codes: &[String]) -> Vec<String> {
        const LUMPS: [&str; 3] = ["nodes_missing", "blockmap_missing", "reject_size_mismatch"];
        codes
            .iter()
            .filter(|code| LUMPS.contains(&code.as_str()))
            .cloned()
            .collect::<Vec<String>>()
    }

    #[tokio::test]
    async fn the_two_room_fixture_warns_only_about_the_emptied_node_block() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
        assert_eq!(report["map"], json!("MAP01"), "the map travels");
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "the fixture geometry is closed, uncrossed and dressed: {report}"
        );
        assert_eq!(
            issue_codes(&report),
            vec![
                "missing_coop_starts".to_owned(),
                "nodes_missing".to_owned(),
                "blockmap_missing".to_owned(),
            ],
            "a solo map with an emptied node block deserves exactly these notes"
        );
        assert_eq!(
            issue(&report, "nodes_missing")["severity"],
            json!("warning"),
            "vanilla cannot play without a tree"
        );
        assert!(
            issue(&report, "nodes_missing")["message"]
                .as_str()
                .expect("text")
                .contains("build_all_nodes"),
            "the message names the fix: {}",
            issue(&report, "nodes_missing")
        );
        assert_eq!(
            report["checks_run"].as_array().expect("a list").len(),
            30,
            "every registered check ran, in order"
        );
        assert_eq!(report["truncated"], json!(false), "nothing stayed behind");
    }

    #[tokio::test]
    async fn building_the_nodes_clears_every_generated_lump_warning() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(&client, "build_all_nodes", json!({ "map": "MAP01" })).await;
        let built = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
        assert_eq!(
            built["counts"]["error"],
            json!(0_u64),
            "the map stays sound: {built}"
        );
        assert_eq!(
            built["counts"]["warning"],
            json!(0_u64),
            "the tree, the matrix and the grid are all built now: {built}"
        );
        let codes = issue_codes(&built);
        assert!(
            codes.contains(&"missing_coop_starts".to_owned()),
            "the coop note is still there: {codes:?}"
        );
        assert!(
            lump_codes(&codes).is_empty(),
            "no lump findings survive build_all_nodes: {codes:?}"
        );
    }

    #[tokio::test]
    async fn editing_a_start_and_a_sector_shows_up_in_the_report() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        drop_the_start(&client).await;
        call(&client, "add_sector", json!({ "map": "MAP01" })).await;
        let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
        assert_eq!(
            report["counts"]["error"],
            json!(3_u64),
            "the lost start and the two flats the added sector names: {report}"
        );
        assert_eq!(
            report["counts"]["warning"],
            json!(2_u64),
            "the orphaned sector and the missing node tree"
        );
        assert_eq!(
            report["counts"]["info"],
            json!(2_u64),
            "the coop note and the missing blockmap"
        );
        let start = issue(&report, "no_player1_start");
        assert_eq!(start["severity"], json!("error"), "nobody can spawn");
        assert!(
            start["message"].as_str().expect("text").contains("type 1"),
            "the message names the number: {start}"
        );
        let sector = issue(&report, "unused_sector");
        assert_eq!(sector["sectors"], json!([2_u64]), "the just-added sector");
        let missing_flats = report["issues"]
            .as_array()
            .expect("an issue list")
            .iter()
            .filter(|row| row["code"] == json!("missing_flat"))
            .count();
        assert_eq!(
            missing_flats, 2,
            "both default flats of the added sector miss: {report}"
        );
        let working = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "scope": "working" }),
        )
        .await;
        assert_eq!(
            working["counts"]["error"],
            json!(3_u64),
            "the copy shows too"
        );
    }

    #[tokio::test]
    async fn min_severity_hides_the_mild_rows_not_the_counts() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        drop_the_start(&client).await;
        let report = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "min_severity": "error" }),
        )
        .await;
        assert_eq!(
            issue_codes(&report),
            vec!["no_player1_start".to_owned()],
            "only the error survives the floor"
        );
        assert_eq!(
            report["counts"]["info"],
            json!(2_u64),
            "the counts keep the truth"
        );
        let warnings = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "min_severity": "WARNING" }),
        )
        .await;
        assert_eq!(
            issue_codes(&warnings),
            vec!["no_player1_start".to_owned(), "nodes_missing".to_owned()],
            "the case-insensitive floor keeps warnings and drops only info"
        );
    }

    #[tokio::test]
    async fn codes_runs_only_the_named_checks() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        drop_the_start(&client).await;
        let report = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "codes": ["no_player1_start"] }),
        )
        .await;
        assert_eq!(
            report["checks_run"],
            json!(["no_player1_start"]),
            "one check ran"
        );
        assert_eq!(
            issue_codes(&report),
            vec!["no_player1_start".to_owned()],
            "one finding"
        );
        assert_eq!(
            report["counts"]["info"],
            json!(0_u64),
            "the coop check never ran"
        );
    }

    #[tokio::test]
    async fn an_unknown_code_is_refused_with_the_valid_list() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "codes": ["not_a_check"] }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "a made-up code refuses");
        assert!(
            message.contains("not_a_check")
                && message.contains("duplicate_line")
                && message.contains("leave codes out"),
            "the message quotes the offender and lists the valid codes: {message}"
        );
    }

    #[tokio::test]
    async fn limit_pages_the_kept_issues() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        drop_the_start(&client).await;
        let report = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "limit": 1_u64 }),
        )
        .await;
        assert_eq!(
            issue_codes(&report),
            vec!["no_player1_start".to_owned()],
            "the first page holds the report-order leading row"
        );
        assert_eq!(
            report["truncated"],
            json!(true),
            "the coop note stayed behind"
        );
        assert_eq!(
            report["counts"]["info"],
            json!(2_u64),
            "and the counts say so"
        );
    }

    #[tokio::test]
    async fn the_broken_map_fixture_reports_its_dangling_sector() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_broken_map(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "min_severity": "error" }),
        )
        .await;
        let broken = issue(&report, "sector_out_of_range");
        assert_eq!(broken["sidedefs"], json!([4_u64]), "sidedef 4 dangles");
        assert!(
            broken["message"]
                .as_str()
                .expect("text")
                .contains("nonexistent sector"),
            "the message explains the range: {broken}"
        );
    }

    #[tokio::test]
    async fn thing_types_follow_the_loaded_custom_definitions() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let extra = pwad_custom_defs(dir.path());
        call(
            &client,
            "open_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
        let warning = issue(&report, "unknown_thing_type");
        assert_eq!(
            warning["severity"],
            json!("warning"),
            "a moved monster warns"
        );
        assert_eq!(warning["things"], json!([1_u64]), "the imp row");
        assert!(
            warning["message"].as_str().expect("text").contains("3001"),
            "the DEHACKED moved the imp to 20000: {warning}"
        );
    }

    #[tokio::test]
    async fn a_heretic_session_does_not_know_the_doom_imp() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_heretic(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(&client, "validate_map", json!({ "map": "E1M1" })).await;
        let unknown = issue(&report, "unknown_thing_type");
        assert_eq!(unknown["things"], json!([1_u64]), "the imp, not the start");
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "type 1 is a Heretic row"
        );
    }

    /// One `Hexen`-family map of the format fixture validates as cleanly
    /// as its `Doom` original: the family decides tables and tag checks.
    /// A `UDMF` block carries no node lumps at all (ports build them),
    /// the binary `Hexen` one keeps the emptied `BLOCKMAP` marker, so the
    /// expected info rows differ.
    async fn family_map_validates(client: &Client, map: &str, expected: &[&str]) {
        let report = call(client, "validate_map", json!({ "map": map })).await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "{map} stays in range"
        );
        assert_eq!(
            report["counts"]["warning"],
            json!(0_u64),
            "{map} needs no tag and its non-empty NODES counts as built"
        );
        assert_eq!(
            issue_codes(&report),
            expected
                .to_vec()
                .iter()
                .map(|code| (*code).to_owned())
                .collect::<Vec<String>>(),
            "{map}: the same clean two-room shape, whatever the family"
        );
    }

    #[tokio::test]
    async fn hexen_family_maps_validate_against_their_own_tables() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let extra = pwad_map_formats(dir.path());
        call(
            &client,
            "open_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        family_map_validates(&client, "MAP03", &["missing_coop_starts"]).await;
        family_map_validates(
            &client,
            "MAP04",
            &["missing_coop_starts", "blockmap_missing"],
        )
        .await;
    }

    #[tokio::test]
    async fn a_duplicated_linedef_reports_the_pair_and_the_shared_side() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP01", "to": "MAP02" }),
        )
        .await;
        let hexed = duplicated_linedefs_hex();
        call(
            &client,
            "replace_lump",
            json!({ "name": "LINEDEFS", "hex": hexed }),
        )
        .await;
        let report = call(&client, "validate_map", json!({ "map": "MAP02" })).await;
        let duplicate = issue(&report, "duplicate_line");
        assert_eq!(
            duplicate["severity"],
            json!("error"),
            "two lines, one place"
        );
        assert_eq!(
            duplicate["linedefs"],
            json!([0_u64, 7_u64]),
            "both rows named"
        );
        assert_eq!(
            duplicate["vertices"],
            json!([0_u64, 5_u64]),
            "the shared ends"
        );
        let shared = issue(&report, "shared_sidedef");
        assert_eq!(shared["sidedefs"], json!([0_u64]), "the side they share");
        assert_eq!(shared["linedefs"], json!([0_u64, 7_u64]), "named as users");
        assert_eq!(
            report["counts"]["error"],
            json!(3_u64),
            "the duplicate, the boundary it unbalances and the stretch it overlaps"
        );
        assert!(
            issue_codes(&report).contains(&"unclosed_sector".to_owned())
                && issue_codes(&report).contains(&"overlapping_lines".to_owned()),
            "both geometry checks see the copy: {report}"
        );
    }

    /// The fixture map's `LINEDEFS` bytes with linedef 0 appended again,
    /// hex-encoded for `replace_lump` (no linedef editing tool exists yet).
    fn duplicated_linedefs_hex() -> String {
        let mut map = two_room_map();
        let first = map
            .linedefs
            .first()
            .expect("the fixture map has linedefs")
            .clone();
        map.linedefs.push(first);
        let lumps = write_doom_map(&map).expect("the fixture map serializes");
        let lump = lumps
            .iter()
            .find(|candidate| candidate.name.as_bytes() == b"LINEDEFS")
            .expect("the encoded block carries LINEDEFS");
        lump.data
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<String>>()
            .join(" ")
    }

    #[tokio::test]
    async fn the_shared_map_parameters_are_enforced_as_documented() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let defaults = call(&client, "validate_map", json!({})).await;
        assert_eq!(
            defaults["map"],
            json!("MAP01"),
            "the only map is the default"
        );
        let (missing_code, _) = call_err(&client, "validate_map", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such block");
        let (scope_code, _) = call_err(&client, "validate_map", json!({ "scope": "nope" })).await;
        assert_eq!(scope_code, "invalid_argument", "not a scope");
        let (severity_code, severity_message) =
            call_err(&client, "validate_map", json!({ "min_severity": "fatal" })).await;
        assert_eq!(severity_code, "invalid_argument", "not a severity");
        assert!(
            severity_message.contains("\"warning\""),
            "the message names the values: {severity_message}"
        );
        let (limit_code, _) = call_err(&client, "validate_map", json!({ "limit": 5001_u64 })).await;
        assert_eq!(limit_code, "invalid_argument", "past the largest page");
        let (layer_code, _) = call_err(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "scope": "layer:5" }),
        )
        .await;
        assert_eq!(layer_code, "not_found", "an unloaded layer is not there");
    }

    #[tokio::test]
    async fn a_renamed_wall_texture_reports_missing_texture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(
            &client,
            "edit_sidedef",
            json!({ "map": "MAP01", "index": 0_i32, "middle": "NOPE" }),
        )
        .await;
        let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
        let missing = issue(&report, "missing_texture");
        assert_eq!(missing["severity"], json!("error"), "an undefined wall");
        assert_eq!(missing["sidedefs"], json!([0_u64]), "the edited side");
        assert!(
            missing["message"]
                .as_str()
                .expect("text")
                .contains("\"NOPE\""),
            "the message quotes the name: {missing}"
        );
        assert_eq!(
            report["counts"]["error"],
            json!(1_u64),
            "exactly the one face"
        );
    }

    #[tokio::test]
    async fn moving_a_vertex_to_cross_the_floor_line_reports_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(
            &client,
            "move_vertex",
            json!({ "map": "MAP01", "index": 5_i32, "x": 512_i32, "y": -256_i32 }),
        )
        .await;
        let report = call(
            &client,
            "validate_map",
            json!({ "map": "MAP01", "codes": ["crossing_lines"] }),
        )
        .await;
        assert_eq!(
            report["checks_run"],
            json!(["crossing_lines"]),
            "one check ran"
        );
        let crossing = issue(&report, "crossing_lines");
        assert_eq!(crossing["severity"], json!("error"), "an X in the walls");
        assert_eq!(
            crossing["linedefs"],
            json!([1_u64, 6_u64]),
            "the moved hall top now crosses the back room's bottom: {crossing}"
        );
    }

    #[tokio::test]
    async fn codes_accepts_every_new_check_by_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(
            &client,
            "validate_map",
            json!({
                "map": "MAP01",
                "codes": ["unclosed_sector", "ceiling_below_floor", "missing_flat", "nodes_missing"],
            }),
        )
        .await;
        assert_eq!(
            report["checks_run"],
            json!([
                "unclosed_sector",
                "ceiling_below_floor",
                "missing_flat",
                "nodes_missing"
            ]),
            "table order, requested subset"
        );
        let codes = issue_codes(&report);
        assert!(
            codes.contains(&"nodes_missing".to_owned()),
            "the fixture block was built empty: {report}"
        );
        assert!(
            !codes.iter().any(|code| code == "unclosed_sector"),
            "the two-room ring closes: {codes:?}"
        );
    }
}
