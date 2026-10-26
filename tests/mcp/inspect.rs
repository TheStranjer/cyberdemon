//! The `inspect` tool group as seen by a real MCP client: diffing session
//! states against each other and against WAD files on disk.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, not_a_wad, pwad, pwad_redundant};
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
        let (unknown_code, unknown_message, _) =
            call_err(&client, "diff_wads", json!({ "a": "sideways" })).await;
        assert_eq!(unknown_code, "invalid_argument", "unknown side code");
        assert!(
            unknown_message.contains("absolute path"),
            "the message lists the forms: {unknown_message}"
        );
        let (relative_code, _, _) =
            call_err(&client, "diff_wads", json!({ "b": "wads/extra.wad" })).await;
        assert_eq!(relative_code, "invalid_argument", "relative path code");
        let (layer_code, _, _) = call_err(&client, "diff_wads", json!({ "b": "layer:x" })).await;
        assert_eq!(layer_code, "invalid_argument", "non-numeric layer code");
    }

    #[tokio::test]
    async fn missing_unreadable_and_out_of_range_sides_report_their_codes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let (missing_code, missing_message, _) = call_err(
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
        let (range_code, _, _) = call_err(&client, "diff_wads", json!({ "b": "layer:3" })).await;
        assert_eq!(range_code, "not_found", "out of range layer code");
        let junk = not_a_wad(dir.path());
        let (junk_code, junk_message, _) = call_err(
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
        let (zero_code, zero_message, _) =
            call_err(&client, "diff_wads", json!({ "limit": 0_u64 })).await;
        assert_eq!(zero_code, "invalid_argument", "zero is no page");
        assert!(
            zero_message.contains("1 and 5000"),
            "the message names the range: {zero_message}"
        );
        let (over_code, _, _) = call_err(&client, "diff_wads", json!({ "limit": 5001_u64 })).await;
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

    /// The `(operation, lump | map)` pairs of an `optimize_wad` report.
    fn action_rows(report: &Value) -> Vec<(String, String)> {
        report["actions"]
            .as_array()
            .expect("actions is an array")
            .iter()
            .map(|row| {
                let target = row["lump"]
                    .as_str()
                    .or_else(|| row["map"].as_str())
                    .expect("a row names its lump or map");
                (
                    row["operation"]
                        .as_str()
                        .expect("rows carry an operation")
                        .to_owned(),
                    target.to_owned(),
                )
            })
            .collect()
    }

    /// The working archive's lump count from `get_session_info`.
    async fn working_lumps(client: &Client) -> u64 {
        let session = call(client, "get_session_info", Value::Null).await;
        session["working"]["lumps"].as_u64().expect("a count")
    }

    /// The `add_lump` arguments for one redundant-lump step of the fixture
    /// session: a text payload when given and the duplicate rule.
    fn add_lump_args(text_name: &str, body: Option<&str>, allow: bool) -> Value {
        let mut args = json!({ "name": text_name });
        if let Some(payload) = body {
            args["text"] = json!(payload);
        }
        if allow {
            args["allow_duplicate"] = json!(true);
        }
        args
    }

    /// Load the `IWAD` fixture, fold the redundant `PWAD` into the working
    /// archive and add the duplicates and empty markers the sweeps find:
    /// two shadowed `NOTE`s, a concatenated `DECORATE` pair and an empty
    /// `F_START`/`F_END` pair, on top of the identical `PLAYPAL`/`ENDOOM`.
    async fn load_redundant_session(client: &Client, dir: &Path) {
        load_iwad(client, dir).await;
        let extra = pwad_redundant(dir);
        call(
            client,
            "merge_wad",
            json!({ "path": extra.display().to_string(), "mode": "working" }),
        )
        .await;
        for (name, text, allow) in [
            ("NOTE", Some("first"), false),
            ("NOTE", Some("last"), true),
            ("DECORATE", Some("alpha"), false),
            ("DECORATE", Some("beta"), true),
            ("F_START", None, false),
            ("F_END", None, false),
        ] {
            call(client, "add_lump", add_lump_args(name, text, allow)).await;
        }
    }

    /// Assert the dry-run plan of the redundant session: five directory
    /// entries go, the `DECORATE` pair stays, nothing changed yet.
    fn assert_redundant_plan(report: &Value) {
        assert_eq!(report["dry_run"], json!(true), "the default reports only");
        assert_eq!(
            action_rows(report),
            vec![
                ("identical_overrides".to_owned(), "PLAYPAL".to_owned()),
                ("identical_overrides".to_owned(), "ENDOOM".to_owned()),
                ("empty_namespaces".to_owned(), "F_START".to_owned()),
                ("empty_namespaces".to_owned(), "F_END".to_owned()),
                ("shadowed_duplicates".to_owned(), "NOTE".to_owned()),
            ],
            "the identical copies, the empty pair and the shadowed note, in sweep order"
        );
        assert_eq!(report["lumps_removed"], json!(5_i64), "five entries go");
        assert_eq!(
            report["bytes_saved"],
            json!(10752 + 4000 + 5_i64),
            "PLAYPAL, ENDOOM and the first NOTE's bytes"
        );
        assert_eq!(
            report["kept_concatenated"],
            json!(["DECORATE"]),
            "the concatenated pair is kept and reported"
        );
        assert_eq!(
            report["kept_for_animation"],
            json!({ "pnames": 0_i64, "patches": 0_i64, "flats": 0_i64 }),
            "no content sweep ran, so nothing was kept for an animation"
        );
    }

    #[tokio::test]
    async fn optimize_wad_dry_run_lists_the_redundant_lumps_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_redundant_session(&client, dir.path()).await;
        let before = call(&client, "get_session_info", Value::Null).await;
        let report = call(&client, "optimize_wad", json!({})).await;
        assert_redundant_plan(&report);
        assert_eq!(
            working_lumps(&client).await,
            8,
            "the working archive is untouched"
        );
        let after = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            after["undo"], before["undo"],
            "and a dry run records nothing"
        );
    }

    #[tokio::test]
    async fn optimize_wad_applies_the_plan_as_one_undoable_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_redundant_session(&client, dir.path()).await;
        let report = call(&client, "optimize_wad", json!({ "dry_run": false })).await;
        assert_eq!(report["dry_run"], json!(false), "the run applied");
        assert_eq!(report["lumps_removed"], json!(5_i64), "five entries went");
        assert_eq!(working_lumps(&client).await, 3, "three stay");
        let (code, _, _) = call_err(
            &client,
            "get_lump",
            json!({ "name": "PLAYPAL", "scope": "working", "format": "info" }),
        )
        .await;
        assert_eq!(code, "not_found", "the identical override is gone");
        let session = call(&client, "get_session_info", Value::Null).await;
        let labels = session["undo"].as_array().expect("an undo list");
        assert_eq!(
            labels.last().expect("at least one label"),
            &json!("optimize_wad"),
            "the whole run is one labelled step on top"
        );
        let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            undone["undone"],
            json!(["optimize_wad"]),
            "undo steps back over it"
        );
        assert_eq!(working_lumps(&client).await, 8, "and restores the archive");
    }

    #[tokio::test]
    async fn optimize_wad_refuses_unknown_operations_with_the_choices() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let (code, message, _) = call_err(
            &client,
            "optimize_wad",
            json!({ "operations": ["unused_things"] }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown operation code");
        assert!(
            message.contains("identical_overrides") && message.contains("unused_flats"),
            "the message lists every operation: {message}"
        );
        assert_eq!(working_lumps(&client).await, 0, "and nothing ran");
    }

    #[tokio::test]
    async fn optimize_wad_reports_a_format_error_for_a_malformed_pnames() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(&client, "open_wad", path_arg(&base)).await;
        call(
            &client,
            "replace_lump",
            json!({ "name": "PNAMES", "hex": "05 00 00 00" }),
        )
        .await;
        let (code, message, _) = call_err(
            &client,
            "optimize_wad",
            json!({ "operations": ["unused_pnames"], "dry_run": false }),
        )
        .await;
        assert_eq!(code, "format_error", "malformed PNAMES code");
        assert!(
            message.contains("PNAMES"),
            "the message says what is unreadable: {message}"
        );
        assert_eq!(working_lumps(&client).await, 33, "the archive is untouched");
    }

    /// Stands each of `flats` in the working archive's flats region.
    async fn add_flats(client: &Client, flats: &[&str]) {
        for flat in flats {
            call(
                client,
                "add_lump",
                json!({ "name": flat, "text": "flat-bytes", "namespace": "flats" }),
            )
            .await;
        }
    }

    /// The region `get_lump` reports `flat` in, for the working archive.
    async fn working_namespace(client: &Client, flat: &str) -> Value {
        let row = call(
            client,
            "get_lump",
            json!({ "name": flat, "scope": "working", "format": "info" }),
        )
        .await;
        row["namespace"].clone()
    }

    /// Asserts every one of `flats` still sits in the working flats region.
    async fn assert_kept(client: &Client, flats: &[&str]) {
        for flat in flats {
            assert_eq!(
                working_namespace(client, flat).await,
                json!("flats"),
                "{flat} stayed in the flats region"
            );
        }
    }

    /// A `PWAD` whose only animated flat is the one a sector wears: the run
    /// beside it has to survive `unused_flats`, and the plan has to say how
    /// much it kept for the animation.
    #[tokio::test]
    async fn optimize_wad_keeps_an_animation_and_counts_the_keeps() {
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        call(
            &client,
            "add_sector",
            json!({ "floor_flat": "NUKAGE1", "ceiling_flat": "FLOOR4" }),
        )
        .await;
        add_flats(&client, &["NUKAGE1", "NUKAGE2", "NUKAGE3", "SPARE"]).await;
        let report = call(
            &client,
            "optimize_wad",
            json!({ "operations": ["unused_flats"] }),
        )
        .await;
        assert_eq!(
            action_rows(&report),
            vec![("unused_flats".to_owned(), "SPARE".to_owned())],
            "a sector wearing NUKAGE1 keeps the run to NUKAGE3"
        );
        assert_eq!(
            report["kept_for_animation"],
            json!({ "pnames": 0_i64, "patches": 0_i64, "flats": 2_i64 }),
            "the two frames no sector named are reported as keeps"
        );
        let applied = call(
            &client,
            "optimize_wad",
            json!({ "operations": ["unused_flats"], "dry_run": false }),
        )
        .await;
        assert_eq!(applied["dry_run"], json!(false), "the plan was applied");
        let (code, _, _) = call_err(
            &client,
            "get_lump",
            json!({ "name": "SPARE", "scope": "working", "format": "info" }),
        )
        .await;
        assert_eq!(code, "not_found", "the flat outside the run is gone");
        assert_kept(&client, &["NUKAGE1", "NUKAGE2", "NUKAGE3"]).await;
    }

    #[tokio::test]
    async fn save_wad_dedupe_writes_a_smaller_file_that_reopens_identical() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        call(
            &client,
            "add_lump",
            json!({ "name": "DUPA", "text": "samebytes" }),
        )
        .await;
        call(
            &client,
            "add_lump",
            json!({ "name": "DUPB", "text": "samebytes" }),
        )
        .await;
        let plain = dir.path().join("plain.wad");
        let shared = dir.path().join("dedup.wad");
        let plain_report = call(&client, "save_wad", path_arg(&plain)).await;
        assert_eq!(
            plain_report["bytes_saved_by_dedupe"],
            json!(0_i64),
            "a plain save shares nothing and reports zero"
        );
        let mut args = path_arg(&shared);
        args["dedupe"] = json!(true);
        let dedup_report = call(&client, "save_wad", args).await;
        let plain_bytes = plain_report["bytes"].as_u64().expect("a count");
        let shared_bytes = dedup_report["bytes"].as_u64().expect("a count");
        assert!(
            shared_bytes < plain_bytes,
            "the deduped file is smaller: {shared_bytes} vs {plain_bytes}"
        );
        assert_eq!(
            dedup_report["bytes_saved_by_dedupe"],
            json!(9_i64),
            "one twin's payload"
        );
        assert_eq!(
            dedup_report["lumps"], plain_report["lumps"],
            "the directory still names every lump"
        );
        let plain_wad = cyberdemon::wad::Wad::read_file(&plain).expect("the plain file parses");
        let shared_wad = cyberdemon::wad::Wad::read_file(&shared).expect("the deduped file parses");
        assert_eq!(
            shared_wad.lumps, plain_wad.lumps,
            "shared offsets re-open as the identical lumps"
        );
        assert_eq!(
            std::fs::read(&shared).expect("the file").len(),
            usize::try_from(shared_bytes).expect("a small file"),
            "bytes match the file"
        );
    }
}
