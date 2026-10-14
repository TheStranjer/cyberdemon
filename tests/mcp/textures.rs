//! The `textures` tool group as seen by a real MCP client: the `PNAMES`
//! and `TEXTURE1`/`TEXTURE2` listings over the fixture IWAD, the pattern
//! and paging rules, and `edit_pnames`' add / conflict / force / rename /
//! `replace_all` paths, the copy-on-write into the working archive, and the
//! one-edit undo bookkeeping.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::archive::Wad;
    use cyberdemon::wad::textures::{
        PatchRef, TextureDef, TextureLayout, TextureLump, parse_pnames, parse_texture_lump,
        write_texture_lump,
    };
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad};
    use crate::support::{call, call_err, connect};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Load the fixture `IWAD` (patches, `PNAMES`, `TEXTURE1`) as the base.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// `get_session_info`'s undo-label list.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", Value::Null).await;
        info["undo"]
            .as_array()
            .expect("undo is an array")
            .iter()
            .map(|entry| entry.as_str().expect("labels are strings").to_owned())
            .collect()
    }

    /// The `name` fields of a report list, in order.
    fn listed(report: &Value, key: &str) -> Vec<String> {
        report[key]
            .as_array()
            .expect("the list is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("names are strings").to_owned())
            .collect()
    }

    /// The space-separated uppercase hex of `data`, for `add_lump` inputs.
    fn hex_of(data: &[u8]) -> String {
        data.iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A `Strife`-layout lump (no column directory, three-field mappatch)
    /// for the layout-detection check.
    fn strife_bytes() -> Vec<u8> {
        let lump = TextureLump {
            layout: TextureLayout::Strife,
            textures: vec![TextureDef {
                name: "STRIFEY".to_owned(),
                flags_raw: 0,
                width: 40,
                height: 24,
                patches: vec![
                    PatchRef {
                        patch: 0,
                        x: 0,
                        y: 8,
                    },
                    PatchRef {
                        patch: 0,
                        x: 16,
                        y: 8,
                    },
                ],
            }],
        };
        write_texture_lump(&lump).expect("the test lump serializes")
    }

    #[tokio::test]
    async fn an_empty_session_lists_no_textures_or_names() {
        let client = connect().await;
        let textures = call(&client, "list_textures", json!({})).await;
        assert_eq!(textures["total"], json!(0_u64), "no texture lumps");
        assert_eq!(
            textures["textures"],
            json!([]),
            "an empty list, not an error"
        );
        let names = call(&client, "list_pnames", json!({})).await;
        assert_eq!(names["total"], json!(0_u64), "no PNAMES lump");
        assert_eq!(names["pnames"], json!([]), "an empty list, not an error");
    }

    #[tokio::test]
    async fn list_textures_reports_the_fixture_definitions() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "list_textures", json!({})).await;
        assert_eq!(report["total"], json!(2_u64), "WALL1 and STEP1");
        assert_eq!(
            listed(&report, "textures"),
            vec!["WALL1", "STEP1"],
            "lump order"
        );
        let wall = &report["textures"][0];
        assert_eq!(wall["width"], json!(64_u64), "the tiled width");
        assert_eq!(wall["height"], json!(128_u64), "the tiled height");
        assert_eq!(wall["patches"], json!(32_u64), "four columns, eight rows");
        assert_eq!(
            wall["lump"],
            json!("TEXTURE1"),
            "from the first texture lump"
        );
        assert_eq!(wall["source"], json!({"layer": 0_u64}), "the IWAD's copy");
        let step = &report["textures"][1];
        assert_eq!(step["patches"], json!(16_u64), "four columns, four rows");
    }

    #[tokio::test]
    async fn list_textures_filters_by_glob_and_pages() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let one = call(&client, "list_textures", json!({ "pattern": "w?ll1" })).await;
        assert_eq!(one["total"], json!(1_u64), "case-insensitive glob");
        assert_eq!(listed(&one, "textures"), vec!["WALL1"], "the matched name");
        let none = call(&client, "list_textures", json!({ "pattern": "Z*" })).await;
        assert_eq!(none["total"], json!(0_u64), "nothing matches");
        let page = call(
            &client,
            "list_textures",
            json!({ "offset": 1_u64, "limit": 1_u64 }),
        )
        .await;
        assert_eq!(page["total"], json!(2_u64), "total counts all matches");
        assert_eq!(listed(&page, "textures"), vec!["STEP1"], "the second row");
        let zero_args = json!({ "limit": 0_u64 });
        let (zero_code, zero_message) = call_err(&client, "list_textures", zero_args).await;
        assert_eq!(zero_code, "invalid_argument", "limit 0 code");
        assert!(
            zero_message.contains("between 1 and 2000"),
            "the message states the range: {zero_message}"
        );
        let big_args = json!({ "limit": 2001_u64 });
        let (big_code, _) = call_err(&client, "list_textures", big_args).await;
        assert_eq!(big_code, "invalid_argument", "limit 2001 code");
    }

    #[tokio::test]
    async fn list_textures_adds_a_working_texture2_and_spots_strife_layouts() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        call(
            &client,
            "add_lump",
            json!({ "name": "TEXTURE2", "hex": hex_of(&strife_bytes()) }),
        )
        .await;
        let report = call(&client, "list_textures", json!({})).await;
        assert_eq!(report["total"], json!(3_u64), "the second lump's texture");
        let last = &report["textures"][2];
        assert_eq!(last["name"], json!("STRIFEY"), "the appended definition");
        assert_eq!(
            last["lump"],
            json!("TEXTURE2"),
            "from the second texture lump"
        );
        assert_eq!(
            last["source"],
            json!("working"),
            "the working archive's copy"
        );
        assert_eq!(last["patches"], json!(2_u64), "the Strife mappatch list");
        // Force an edit that renumbers every texture lump: TEXTURE2 must be
        // written back in the Strife layout it was read in, which the
        // 26-byte size of a zero-patch Strife record proves (a Doom record
        // for the same texture would be 34 bytes).
        let forced = call(
            &client,
            "edit_pnames",
            json!({ "remove": ["WALLP1"], "force": true }),
        )
        .await;
        assert_eq!(
            forced["textures_changed"],
            json!(3_u64),
            "STRIFEY shifts too"
        );
        let still = call(&client, "list_textures", json!({ "pattern": "STRIFEY" })).await;
        assert_eq!(
            still["total"],
            json!(1_u64),
            "the rewritten Strife lump decodes"
        );
        assert_eq!(
            still["textures"][0]["patches"],
            json!(0_u64),
            "its WALLP1 references were dropped"
        );
        let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        let texture2 = working["lumps"]
            .as_array()
            .expect("array")
            .iter()
            .find(|row| row["name"] == json!("TEXTURE2"))
            .expect("TEXTURE2 was copied in");
        assert_eq!(
            texture2["size"],
            json!(26_u64),
            "the Strife layout was preserved on write-back"
        );
    }

    #[tokio::test]
    async fn list_pnames_reports_the_index_and_lump_presence() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "list_pnames", json!({})).await;
        assert_eq!(report["total"], json!(2_u64), "both patch names");
        assert_eq!(
            listed(&report, "pnames"),
            vec!["WALLP1", "STEPP1"],
            "index order"
        );
        assert_eq!(
            report["pnames"][0]["index"],
            json!(0_u64),
            "the first index"
        );
        assert_eq!(
            report["pnames"][0]["lump_found"],
            json!(true),
            "WALLP1 has a lump"
        );
        assert_eq!(
            report["pnames"][1]["lump_found"],
            json!(true),
            "STEPP1 has a lump"
        );
        let one = call(
            &client,
            "list_pnames",
            json!({ "pattern": "ste*", "limit": 1_u64 }),
        )
        .await;
        assert_eq!(one["total"], json!(1_u64), "the glob applied");
        assert_eq!(listed(&one, "pnames"), vec!["STEPP1"], "the matched entry");
        let (code, _) = call_err(&client, "list_pnames", json!({ "limit": 0_u64 })).await;
        assert_eq!(code, "invalid_argument", "limit 0 code");
    }

    #[tokio::test]
    async fn a_malformed_texture_lump_surfaces_as_format_error() {
        let client = connect().await;
        call(
            &client,
            "add_lump",
            json!({ "name": "PNAMES", "hex": "02 00 00 00 41" }),
        )
        .await;
        let (code, message) = call_err(&client, "list_pnames", json!({})).await;
        assert_eq!(code, "format_error", "truncated name list code");
        assert!(
            message.contains("end of data"),
            "the message shows where it ran out: {message}"
        );
    }

    #[tokio::test]
    async fn edit_pnames_add_copies_on_write_and_leaves_the_iwad_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&base)).await;
        let report = call(
            &client,
            "edit_pnames",
            json!({ "add": ["NEWP1", "wallp1"] }),
        )
        .await;
        assert_eq!(report["pnames"], json!(3_u64), "one name appended");
        assert_eq!(report["added"], json!(["NEWP1"]), "uppercased, once");
        assert_eq!(
            report["skipped"],
            json!(["WALLP1"]),
            "already present, reported"
        );
        assert_eq!(
            report["textures_changed"],
            json!(0_u64),
            "appends never shift"
        );
        let names = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            listed(&names, "pnames"),
            vec!["WALLP1", "STEPP1", "NEWP1"],
            "the merged view shows the new entry"
        );
        assert_eq!(
            names["pnames"][2]["lump_found"],
            json!(false),
            "NEWP1 has no lump yet"
        );
        let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            listed(&working, "lumps"),
            vec!["PNAMES"],
            "only PNAMES was copied into the working archive"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["edit_pnames add"],
            "the whole change is one undoable edit"
        );
        let on_disk = Wad::read_file(&base).expect("the fixture file reads");
        let original = on_disk
            .lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"PNAMES")
            .expect("the IWAD still holds its PNAMES");
        assert_eq!(
            parse_pnames(&original.data).expect("the original decodes"),
            vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
            "the IWAD file never changed"
        );
    }

    #[tokio::test]
    async fn edit_pnames_remove_conflicts_then_forces_through() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message) =
            call_err(&client, "edit_pnames", json!({ "remove": ["WALLP1"] })).await;
        assert_eq!(code, "conflict", "referenced name code");
        assert!(
            message.contains("WALL1 (TEXTURE1)"),
            "the message lists the referencing texture: {message}"
        );
        assert!(
            message.contains("force=true"),
            "the message names the escape: {message}"
        );
        let refused = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert!(
            refused["lumps"].as_array().expect("array").is_empty(),
            "the refused edit wrote nothing"
        );
        let report = call(
            &client,
            "edit_pnames",
            json!({ "remove": ["WALLP1"], "force": true }),
        )
        .await;
        assert_eq!(report["pnames"], json!(1_u64), "only STEPP1 is left");
        assert_eq!(report["removed"], json!(["WALLP1"]), "reported removed");
        assert_eq!(
            report["textures_changed"],
            json!(2_u64),
            "pruned and renumbered"
        );
        let textures = call(&client, "list_textures", json!({})).await;
        assert_eq!(
            textures["textures"][0]["patches"],
            json!(0_u64),
            "WALL1 lost its refs"
        );
        assert_eq!(
            textures["textures"][1]["patches"],
            json!(16_u64),
            "STEP1 kept its refs, renumbered"
        );
        let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            listed(&working, "lumps"),
            vec!["PNAMES", "TEXTURE1"],
            "PNAMES and the changed texture lump were copied in"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["edit_pnames remove"],
            "one undoable edit for both lumps"
        );
    }

    #[tokio::test]
    async fn forced_remove_renumbers_so_surviving_refs_stay_true() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        call(
            &client,
            "edit_pnames",
            json!({ "remove": ["WALLP1"], "force": true }),
        )
        .await;
        let names = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            listed(&names, "pnames"),
            vec!["STEPP1"],
            "the surviving list"
        );
        assert_eq!(
            names["pnames"][0]["index"],
            json!(0_u64),
            "now at index zero"
        );
        let base = iwad(dir.path());
        let on_disk = Wad::read_file(&base).expect("the fixture file reads");
        let texture = on_disk
            .lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"TEXTURE1")
            .expect("the IWAD still holds its TEXTURE1");
        let untouched = parse_texture_lump(&texture.data, 2).expect("the original decodes");
        assert_eq!(
            untouched.textures.first().expect("WALL1").patches.len(),
            32,
            "the IWAD's texture lump never changed"
        );
    }

    #[tokio::test]
    async fn edit_pnames_rename_keeps_texture_references() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "edit_pnames",
            json!({ "rename": [{ "from": "wallp1", "to": "WALLPX" }] }),
        )
        .await;
        assert_eq!(report["pnames"], json!(2_u64), "the count stands");
        assert_eq!(
            report["renamed"],
            json!([{ "from": "WALLP1", "to": "WALLPX" }]),
            "the uppercased pair"
        );
        assert_eq!(
            report["textures_changed"],
            json!(0_u64),
            "indices did not move"
        );
        let names = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            listed(&names, "pnames"),
            vec!["WALLPX", "STEPP1"],
            "renamed in place"
        );
        assert_eq!(
            names["pnames"][0]["lump_found"],
            json!(false),
            "WALLPX has no lump"
        );
        let textures = call(&client, "list_textures", json!({})).await;
        assert_eq!(
            textures["textures"][0]["patches"],
            json!(32_u64),
            "WALL1 still carries all its placements"
        );
    }

    #[tokio::test]
    async fn edit_pnames_replace_all_reindexes_and_revalidates() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message) =
            call_err(&client, "edit_pnames", json!({ "replace_all": ["WALLP1"] })).await;
        assert_eq!(code, "conflict", "STEPP1 vanishes while STEP1 refers to it");
        assert!(
            message.contains("STEP1 (TEXTURE1)") && !message.contains("WALL1"),
            "only STEP1 refers to the vanishing STEPP1: {message}"
        );
        let report = call(
            &client,
            "edit_pnames",
            json!({ "replace_all": ["WALLP1"], "force": true }),
        )
        .await;
        assert_eq!(report["pnames"], json!(1_u64), "the new complete list");
        assert_eq!(report["removed"], json!(["STEPP1"]), "the vanished entry");
        assert_eq!(report["added"], json!([]), "nothing new came in");
        assert_eq!(
            report["textures_changed"],
            json!(1_u64),
            "only STEP1 changed (its refs all vanished)"
        );
        let textures = call(&client, "list_textures", json!({})).await;
        assert_eq!(
            textures["textures"][0]["patches"],
            json!(32_u64),
            "WALL1's index-0 refs survive untouched"
        );
        assert_eq!(
            textures["textures"][1]["patches"],
            json!(0_u64),
            "STEP1 was pruned"
        );
    }

    #[tokio::test]
    async fn edit_pnames_validates_its_arguments() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message) = call_err(&client, "edit_pnames", json!({})).await;
        assert_eq!(code, "invalid_argument", "no operation code");
        assert!(
            message.contains("exactly one"),
            "the message states the rule: {message}"
        );
        let (both_code, _) = call_err(
            &client,
            "edit_pnames",
            json!({ "add": ["X"], "remove": ["WALLP1"] }),
        )
        .await;
        assert_eq!(both_code, "invalid_argument", "two operations code");
        let (name_code, name_message) = call_err(
            &client,
            "edit_pnames",
            json!({ "add": ["TOO LONG A NAME"] }),
        )
        .await;
        assert_eq!(name_code, "invalid_argument", "bad name code");
        assert!(
            name_message.contains("TOO LONG A NAME"),
            "the message echoes the name: {name_message}"
        );
        let (empty_code, empty_message) =
            call_err(&client, "edit_pnames", json!({ "add": [] })).await;
        assert_eq!(empty_code, "invalid_argument", "empty add code");
        assert!(
            empty_message.contains("at least one"),
            "the message states the rule: {empty_message}"
        );
        let (dupe_code, _) =
            call_err(&client, "edit_pnames", json!({ "replace_all": ["A", "a"] })).await;
        assert_eq!(dupe_code, "invalid_argument", "case-folded duplicate code");
        let (missing_code, _) = call_err(
            &client,
            "edit_pnames",
            json!({ "rename": [{ "from": "NOPE", "to": "XYZ" }] }),
        )
        .await;
        assert_eq!(missing_code, "not_found", "missing rename source code");
        assert!(
            undo_labels(&client).await.is_empty(),
            "none of the refusals recorded an edit"
        );
    }

    #[tokio::test]
    async fn edit_pnames_creates_a_missing_pnames_through_a_resource_layer() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad(dir.path());
        call(&client, "merge_wad", path_arg(&extra)).await;
        let report = call(&client, "edit_pnames", json!({ "add": ["FIRST"] })).await;
        assert_eq!(report["pnames"], json!(1_u64), "the list started fresh");
        let names = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            listed(&names, "pnames"),
            vec!["FIRST"],
            "the merged view shows it"
        );
        let (code, _) = call_err(
            &client,
            "edit_pnames",
            json!({ "rename": [{ "from": "SECOND", "to": "THIRD" }] }),
        )
        .await;
        assert_eq!(code, "not_found", "rename needs an existing entry");
    }
}
