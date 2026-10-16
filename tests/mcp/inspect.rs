//! The `inspect` tool group as seen by a real MCP client: diffing session
//! states against each other and against WAD files on disk.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, not_a_wad, pwad};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Load the `IWAD` fixture as the session's base layer.
    async fn load_iwad(client: &Client, dir: &Path) {
        let base = iwad(dir);
        call(client, "open_iwad", path_arg(&base)).await;
    }

    /// Load the `IWAD` fixture and merge the `PWAD` into the working
    /// archive: the shape of "my PWAD sits on the game".
    async fn load_iwad_and_merge_working(client: &Client, dir: &Path) {
        load_iwad(client, dir).await;
        let extra = pwad(dir);
        call(
            client,
            "merge_wad",
            json!({ "path": extra.display().to_string(), "mode": "working" }),
        )
        .await;
    }

    /// The `lumps` rows of a diff report as `(name, status)` pairs.
    fn lump_rows(report: &Value) -> Vec<(String, String)> {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().expect("rows carry a name").to_owned(),
                    row["status"]
                        .as_str()
                        .expect("rows carry a status")
                        .to_owned(),
                )
            })
            .collect()
    }

    /// The `maps` rows of a diff report as `(name, status)` pairs.
    fn map_rows(report: &Value) -> Vec<(String, String)> {
        report["maps"]
            .as_array()
            .expect("maps is an array")
            .iter()
            .map(|unit| {
                (
                    unit["name"].as_str().expect("maps carry a name").to_owned(),
                    unit["status"]
                        .as_str()
                        .expect("maps carry a status")
                        .to_owned(),
                )
            })
            .collect()
    }

    /// The one row of a lump list named `name`, as a JSON value.
    fn lump_row(report: &Value, name: &str) -> Value {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .find(|row| row["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} is missing from the diff: {:?}", lump_rows(report)))
            .clone()
    }

    /// The one row of a map list named `name`, as a JSON value.
    fn map_row(report: &Value, name: &str) -> Value {
        report["maps"]
            .as_array()
            .expect("maps is an array")
            .iter()
            .find(|unit| unit["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} is missing from the diff: {:?}", map_rows(report)))
            .clone()
    }

    /// Assert the flat override and the fresh global of the default diff.
    fn assert_floor_override(report: &Value) {
        let floor = lump_row(report, "FLOOR1");
        assert_eq!(floor["status"], json!("override"), "the redefined flat");
        assert_eq!(
            floor["namespace"],
            json!("flats"),
            "keyed inside its region"
        );
        assert_eq!(floor["size_a"], json!(4_096_i64), "the base flat's bytes");
        assert_eq!(floor["size_b"], json!(4_096_i64), "and the working copy's");
        assert_eq!(
            lump_row(report, "DEHACKED")["status"],
            json!("new"),
            "a global only the working archive holds"
        );
    }

    /// Assert the brought `MAP02` unit of the default diff.
    fn assert_map02_new(report: &Value) {
        assert_eq!(
            map_rows(report),
            vec![("MAP02".to_owned(), "new".to_owned())],
            "the brought map is the only map report worth showing"
        );
        let map02 = map_row(report, "MAP02");
        assert_eq!(map02["counts_a"], Value::Null, "the base has no such block");
        assert_eq!(
            map02["counts_b"]["things"],
            json!(2_i64),
            "the new block decodes"
        );
        assert_eq!(
            map02["counts_b"]["linedefs"],
            json!(7_i64),
            "counted per side"
        );
    }

    #[tokio::test]
    async fn the_default_diff_answers_what_the_working_archive_overrides() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad_and_merge_working(&client, dir.path()).await;
        let report = call(&client, "diff_wads", json!({})).await;
        assert_eq!(report["a"], json!("base"), "the default left side");
        assert_eq!(report["b"], json!("working"), "the default right side");
        assert_floor_override(&report);
        assert_map02_new(&report);
        assert!(
            !lump_rows(&report)
                .iter()
                .any(|row| row.0 == "PLAYPAL" || row.1 == "unchanged"),
            "untouched and layer-only lumps stay out: {:?}",
            lump_rows(&report)
        );
        assert_eq!(report["summary"]["override"], json!(1_i64), "one override");
        assert_eq!(report["summary"]["new"], json!(2_i64), "DEHACKED and MAP02");
        assert_eq!(
            report["summary"]["removed"],
            json!(0_i64),
            "removal cannot happen"
        );
        assert_eq!(report["truncated"], json!(false), "well under the limit");
    }

    #[tokio::test]
    async fn identical_bytes_report_as_an_identical_override() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&extra)).await;
        call(
            &client,
            "replace_lump",
            json!({ "name": "DEHACKED", "text": "Thing 1 (Troop)\n" }),
        )
        .await;
        let report = call(&client, "diff_wads", json!({})).await;
        assert_eq!(
            lump_rows(&report),
            vec![("DEHACKED".to_owned(), "identical_override".to_owned())],
            "the copy that changes nothing is the only working unit"
        );
        assert_eq!(
            report["summary"]["identical_override"],
            json!(1_i64),
            "counted in its own bucket"
        );
        assert_eq!(
            report["summary"]["override"],
            json!(0_i64),
            "not a real override"
        );
    }

    #[tokio::test]
    async fn a_layer_against_a_file_path_uses_the_neutral_vocabulary() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let extra = pwad(dir.path());
        let report = call(
            &client,
            "diff_wads",
            json!({ "a": "layer:0", "b": extra.display().to_string() }),
        )
        .await;
        assert_eq!(report["a"], json!("layer:0"), "the given sides echo");
        assert_eq!(
            report["b"],
            json!(extra.display().to_string()),
            "including the path"
        );
        assert_eq!(
            lump_row(&report, "FLOOR1")["status"],
            json!("changed"),
            "the flat differs"
        );
        assert_eq!(
            lump_row(&report, "DEHACKED")["status"],
            json!("added"),
            "the patch file brings a global"
        );
        assert_eq!(
            lump_row(&report, "PLAYPAL")["status"],
            json!("removed"),
            "and drops the palette"
        );
        assert_eq!(
            lump_row(&report, "WALLP1")["namespace"],
            json!("patches"),
            "regions key the rows"
        );
        assert_eq!(report["summary"]["changed"], json!(1_i64), "one lump");
        assert_eq!(
            report["summary"]["added"],
            json!(2_i64),
            "DEHACKED and MAP02"
        );
    }

    #[tokio::test]
    async fn the_layer_file_diff_compares_map_blocks_as_units() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let extra = pwad(dir.path());
        let report = call(
            &client,
            "diff_wads",
            json!({
                "a": "layer:0",
                "b": extra.display().to_string(),
                "include_unchanged": true,
            }),
        )
        .await;
        assert_eq!(
            map_rows(&report),
            vec![
                ("MAP01".to_owned(), "removed".to_owned()),
                ("MAP02".to_owned(), "added".to_owned()),
            ],
            "each block is a unit of its own side"
        );
        let map01 = map_row(&report, "MAP01");
        assert_eq!(
            map01["counts_a"]["sectors"],
            json!(2_i64),
            "the missing side still counts"
        );
        assert_eq!(map01["counts_b"], Value::Null, "the other side has none");
    }

    #[tokio::test]
    async fn a_replaced_map_member_shows_up_as_a_changed_map_with_counts() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        call(
            &client,
            "replace_lump",
            json!({ "name": "THINGS", "hex": "01 02 03" }),
        )
        .await;
        let report = call(&client, "diff_wads", json!({})).await;
        assert_eq!(
            map_rows(&report),
            vec![("MAP01".to_owned(), "override".to_owned())],
            "the copy-on-write block overrides the base map"
        );
        let map01 = map_row(&report, "MAP01");
        let members = map01["lumps"].as_array().expect("members is an array");
        assert_eq!(members.len(), 1, "only the differing member is listed");
        assert_eq!(
            members.first().expect("one member")["name"],
            json!("THINGS"),
            "by name"
        );
        assert_eq!(
            members.first().expect("one member")["status"],
            json!("changed"),
            "member rows keep the neutral vocabulary"
        );
        assert_eq!(
            map01["counts_a"]["things"],
            json!(2_i64),
            "the base block decodes"
        );
        assert_eq!(map01["counts_b"], Value::Null, "the corrupt copy does not");
    }

    #[tokio::test]
    async fn bad_sides_are_refused_before_any_comparison() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let (unknown_code, unknown_message) =
            call_err(&client, "diff_wads", json!({ "a": "sideways" })).await;
        assert_eq!(unknown_code, "invalid_argument", "unknown side code");
        assert!(
            unknown_message.contains("absolute path"),
            "the message lists the forms: {unknown_message}"
        );
        let (relative_code, _) =
            call_err(&client, "diff_wads", json!({ "b": "wads/extra.wad" })).await;
        assert_eq!(relative_code, "invalid_argument", "relative path code");
        let (layer_code, _) = call_err(&client, "diff_wads", json!({ "b": "layer:x" })).await;
        assert_eq!(layer_code, "invalid_argument", "non-numeric layer code");
    }

    #[tokio::test]
    async fn missing_unreadable_and_out_of_range_sides_report_their_codes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let (missing_code, missing_message) = call_err(
            &client,
            "diff_wads",
            json!({ "b": dir.path().join("absent.wad").display().to_string() }),
        )
        .await;
        assert_eq!(missing_code, "not_found", "missing file code");
        assert!(
            missing_message.contains("absent.wad"),
            "the message names the file: {missing_message}"
        );
        let (range_code, _) = call_err(&client, "diff_wads", json!({ "b": "layer:3" })).await;
        assert_eq!(range_code, "not_found", "out of range layer code");
        let junk = not_a_wad(dir.path());
        let (junk_code, junk_message) = call_err(
            &client,
            "diff_wads",
            json!({ "b": junk.display().to_string() }),
        )
        .await;
        assert_eq!(junk_code, "format_error", "not a WAD code");
        assert!(
            junk_message.contains("magic"),
            "the message explains why: {junk_message}"
        );
    }

    #[tokio::test]
    async fn limits_page_both_lists_and_bad_ones_are_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let report = call(
            &client,
            "diff_wads",
            json!({ "a": "layer:0", "b": "merged", "include_unchanged": true, "limit": 2_u64 }),
        )
        .await;
        assert_eq!(
            lump_rows(&report).len(),
            2,
            "the page holds exactly two lumps"
        );
        assert_eq!(
            report["lumps"][0]["name"],
            json!("PLAYPAL"),
            "directory order"
        );
        assert_eq!(report["lumps"][1]["name"], json!("COLORMAP"), "row by row");
        assert_eq!(report["truncated"], json!(true), "more rows waited beyond");
        assert_eq!(
            report["summary"]["unchanged"],
            json!(17_i64),
            "sixteen lumps and one map, counted before paging"
        );
        let (zero_code, zero_message) =
            call_err(&client, "diff_wads", json!({ "limit": 0_u64 })).await;
        assert_eq!(zero_code, "invalid_argument", "zero is no page");
        assert!(
            zero_message.contains("1 and 5000"),
            "the message names the range: {zero_message}"
        );
        let (over_code, _) = call_err(&client, "diff_wads", json!({ "limit": 5001_u64 })).await;
        assert_eq!(over_code, "invalid_argument", "past the ceiling");
    }

    #[tokio::test]
    async fn two_empty_sides_diff_to_nothing() {
        let client = connect().await;
        let report = call(&client, "diff_wads", json!({ "include_unchanged": true })).await;
        assert_eq!(report["a"], json!("base"), "the default left side");
        assert_eq!(report["b"], json!("working"), "the default right side");
        assert!(lump_rows(&report).is_empty(), "nothing to compare");
        assert!(map_rows(&report).is_empty(), "no maps either");
        assert_eq!(report["truncated"], json!(false), "and nothing to page");
        assert_eq!(
            report["summary"]["unchanged"],
            json!(0_i64),
            "an empty tally"
        );
    }
}
