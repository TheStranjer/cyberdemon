//! The `scripts` tool group as seen by a real MCP client: `get_dehacked`
//! reading fixture patches and `edit_dehacked` creating, copying-on-write,
//! setting, removing and replacing DeHackEd/BEX lump texts with undo,
//! byte-preserving `Latin-1` round trips and the `get_thing_types` round
//! trip; plus `edit_decorate` and `edit_zscript`
//! running single tagged text operations over script lumps, again with
//! creation, copy-on-write, undo and the definitions round trip.

#[cfg(test)]
mod tests {
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad, pwad_custom_defs};
    use crate::support::{call, call_err, connect};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// A client with the fixture `IWAD` as base and the short-DEHACKED
    /// fixture `PWAD` merged as resource layer 1.
    async fn fixture_client(dir: &std::path::Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        let extra = pwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        call(
            &client,
            "merge_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        client
    }

    /// The block header fields (`kind`, `number`, `name`) of a report row.
    fn head(block: &Value) -> Value {
        json!([block["kind"], block["number"], block["name"],])
    }

    /// The payload as `get_lump format=hex` spells it: space-separated
    /// uppercase bytes.
    fn hex_of(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<String>>()
            .join(" ")
    }

    /// Write a Latin-1 `.deh` file (byte `0xE9` in a value) into `dir`,
    /// answering its raw bytes and path.
    fn latin1_deh(dir: &std::path::Path) -> (Vec<u8>, String) {
        let raw: Vec<u8> = b"Misc\nname = Caf\xE9\nspeed = 1\n".to_vec();
        let file = dir.join("cafe.deh");
        std::fs::write(&file, &raw).expect("write the Latin-1 source file");
        (raw, file.display().to_string())
    }

    #[tokio::test]
    async fn a_source_fetch_stores_latin1_bytes_verbatim() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let (raw, path) = latin1_deh(dir.path());
        let fetched = call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "source": path }),
        )
        .await;
        assert_eq!(
            fetched["added"],
            json!(3_i64),
            "three new lines against nothing"
        );
        let stored = call(
            &client,
            "get_lump",
            json!({ "name": "MYDEH", "format": "hex" }),
        )
        .await;
        assert_eq!(
            stored["hex"],
            json!(hex_of(&raw)),
            "the fetched bytes are stored exactly"
        );
    }

    #[tokio::test]
    async fn editing_a_latin1_patch_preserves_every_untouched_byte() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let (_, path) = latin1_deh(dir.path());
        call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "source": path }),
        )
        .await;
        let edited = call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "set": [{ "block": "Misc", "key": "speed", "value": "2" }] }),
        )
        .await;
        assert_eq!(edited["changed"], json!(1_i64), "one line rewritten");
        assert_eq!(edited["added"], json!(0_i64), "nothing inserted");
        let after = call(
            &client,
            "get_lump",
            json!({ "name": "MYDEH", "format": "hex" }),
        )
        .await;
        assert_eq!(
            after["hex"],
            json!(hex_of(b"Misc\nname = Caf\xE9\nspeed = 2\n")),
            "the 0xE9 byte survived the edit byte for byte"
        );
        let read = call(
            &client,
            "get_dehacked",
            json!({ "name": "MYDEH", "include_text": true }),
        )
        .await;
        assert_eq!(
            read["text"],
            json!("Misc\nname = Caf\u{E9}\nspeed = 2\n"),
            "Latin-1 decoding, never a U+FFFD"
        );
        assert_eq!(
            read["blocks"][0]["fields"]["name"],
            json!("Caf\u{E9}"),
            "the parsed value round trips"
        );
    }

    #[tokio::test]
    async fn a_single_insertion_reports_one_added_line() {
        let client = connect().await;
        call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "text": "Misc\nfoo = 1\n" }),
        )
        .await;
        let inserted = call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "set": [{ "block": "Misc", "key": "bar", "value": "2" }] }),
        )
        .await;
        assert_eq!(inserted["added"], json!(1_i64), "the one inserted line");
        assert_eq!(inserted["removed"], json!(0_i64), "nothing dropped");
        assert_eq!(inserted["changed"], json!(0_i64), "nothing rewritten");
        assert_eq!(inserted["lines"], json!(3_i64), "the patch grew by one");
    }

    #[tokio::test]
    async fn get_reads_the_fixture_patch_from_the_layer_that_holds_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        let report = call(&client, "get_dehacked", json!({})).await;
        assert_eq!(report["name"], json!("DEHACKED"), "the default lump");
        assert_eq!(report["source"]["kind"], json!("resource"), "the PWAD copy");
        assert_eq!(report["source"]["index"], json!(1_i64), "layer 1");
        assert_eq!(report["header"]["doom_version"], json!(null), "no header");
        assert_eq!(report["warnings"], json!([]), "a bare Thing parses cleanly");
        assert_eq!(
            report["blocks"],
            json!([{
                "kind": "Thing",
                "number": 1_i64,
                "name": "Troop",
                "line": 1_i64,
                "fields": {},
            }]),
            "the one block, with its 1-based header line"
        );
        assert_eq!(report["bex"], json!([]), "no BEX sections");
        assert!(
            report.get("text").is_none(),
            "text stays out without include_text: {report}"
        );
    }

    #[tokio::test]
    async fn include_text_attaches_the_whole_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        let report = call(&client, "get_dehacked", json!({ "include_text": true })).await;
        assert_eq!(
            report["text"],
            json!("Thing 1 (Troop)\n"),
            "the raw text round trips"
        );
    }

    #[tokio::test]
    async fn a_missing_lump_is_not_found_and_a_bad_scope_is_refused() {
        let client = connect().await;
        let (code, message, _) = call_err(&client, "get_dehacked", json!({})).await;
        assert_eq!(code, "not_found", "nothing is loaded code");
        assert!(
            message.contains("edit_dehacked"),
            "the message names the creating tool: {message}"
        );
        let (scope_code, _, _) =
            call_err(&client, "get_dehacked", json!({ "scope": "nope" })).await;
        assert_eq!(scope_code, "invalid_argument", "bad scope code");
    }

    #[tokio::test]
    async fn set_moves_the_imp_through_a_copy_on_write_override() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        let report = call(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "Thing 12", "key": "ID #", "value": "20000" }] }),
        )
        .await;
        assert_eq!(report["name"], json!("DEHACKED"), "the echoed lump");
        assert!(
            report["created"].as_bool().expect("created"),
            "a new working override"
        );
        assert_eq!(
            report["added"],
            json!(3_i64),
            "blank, header and key inserted"
        );
        assert_eq!(report["removed"], json!(0_i64), "nothing dropped");
        assert_eq!(report["changed"], json!(0_i64), "nothing rewritten");
        assert_eq!(report["lines"], json!(4_i64), "the old line plus three");
        let moved = call(&client, "get_thing_types", json!({ "type": 20_000_i64 })).await;
        let row = moved["things"].get(0).expect("one row").clone();
        assert_eq!(row["name"], json!("Imp"), "the Imp moved");
        assert_eq!(
            row["source"],
            json!("dehacked"),
            "read through the merged view"
        );
        let freed = call(&client, "get_thing_types", json!({ "type": 3_001_i64 })).await;
        assert_eq!(freed["count"], json!(0_i64), "3001 no longer spawns it");
        let again = call(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "thing 12", "key": "id #", "value": "20001" }] }),
        )
        .await;
        assert!(
            !again["created"].as_bool().expect("created"),
            "now edited in place"
        );
        assert_eq!(again["changed"], json!(1_i64), "one line rewritten");
        assert_eq!(again["added"], json!(0_i64), "nothing inserted");
        assert_eq!(again["removed"], json!(0_i64), "nothing dropped");
        let working = call(&client, "get_dehacked", json!({ "scope": "working" })).await;
        assert_eq!(
            working["source"]["kind"],
            json!("working"),
            "the working copy"
        );
        let block = working["blocks"].get(1).expect("the Thing 12 block");
        assert_eq!(
            block["fields"]["id #"],
            json!("20001"),
            "the case-folded rewrite"
        );
    }

    #[tokio::test]
    async fn set_creates_a_header_bearing_patch_when_no_lump_exists() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        let report = call(
            &client,
            "edit_dehacked",
            json!({ "name": "MYDEH", "set": [{ "block": "Misc", "key": "total ammo", "value": "200" }] }),
        )
        .await;
        assert!(
            report["created"].as_bool().expect("created"),
            "a fresh lump"
        );
        assert_eq!(report["added"], report["lines"], "everything is new");
        assert_eq!(report["removed"], json!(0_i64), "an absent base drops none");
        let read = call(
            &client,
            "get_dehacked",
            json!({ "name": "MYDEH", "include_text": true }),
        )
        .await;
        assert_eq!(
            read["source"]["kind"],
            json!("working"),
            "in the working archive"
        );
        assert_eq!(
            read["header"]["doom_version"],
            json!(21_i64),
            "the canonical header came with it"
        );
        let text = read["text"].as_str().expect("text");
        assert!(
            text.starts_with(
                "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\nMisc\n"
            ),
            "the documented creation text: {text}"
        );
    }

    #[tokio::test]
    async fn remove_takes_keys_and_blocks_and_misses_change_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        call(
            &client,
            "edit_dehacked",
            json!({ "set": [
                { "block": "Thing 12", "key": "ID #", "value": "20000" },
                { "block": "Thing 12", "key": "Health", "value": "60" },
            ] }),
        )
        .await;
        let (code, message, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Frame 3" }] }),
        )
        .await;
        assert_eq!(code, "not_found", "missing block code");
        assert!(
            message.contains("nothing changed"),
            "the promise: {message}"
        );
        let (key_code, _, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Thing 12", "key": "nope" }] }),
        )
        .await;
        assert_eq!(key_code, "not_found", "missing key code");
        let removed = call(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Thing 12", "key": "id #" }] }),
        )
        .await;
        assert_eq!(removed["removed"], json!(1_i64), "one line went");
        assert_eq!(removed["added"], json!(0_i64), "nothing came");
        assert_eq!(removed["changed"], json!(0_i64), "nothing rewrote");
        let read = call(&client, "get_dehacked", json!({ "scope": "working" })).await;
        let block = read["blocks"].get(1).expect("the Thing 12 block");
        assert_eq!(
            block["fields"],
            json!({ "Health": "60" }),
            "only the removed key is gone"
        );
    }

    #[tokio::test]
    async fn text_and_source_modes_replace_the_whole_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = fixture_client(dir.path()).await;
        let report = call(
            &client,
            "edit_dehacked",
            json!({ "text": "Misc\nfoo = 1\n\n[STRINGS]\nHUDSTR_X = hi\n" }),
        )
        .await;
        assert_eq!(report["lines"], json!(5_i64), "the new line count");
        assert_eq!(
            report["changed"],
            json!(1_i64),
            "the old header line pairs with one new line"
        );
        assert_eq!(report["added"], json!(4_i64), "four true inserts");
        assert_eq!(report["removed"], json!(0_i64), "nothing dropped");
        let read = call(&client, "get_dehacked", json!({ "include_text": true })).await;
        assert_eq!(
            read["text"],
            json!("Misc\nfoo = 1\n\n[STRINGS]\nHUDSTR_X = hi\n"),
            "as passed"
        );
        assert_eq!(
            head(read["blocks"].get(0).expect("the Misc block")),
            json!(["Misc", null, null]),
            "reparsed"
        );
        assert_eq!(
            read["bex"].get(0).expect("a section")["section"],
            json!("STRINGS"),
            "bex read"
        );
        let file = dir.path().join("new.deh");
        std::fs::write(&file, b"Misc\nfoo = 2\n").expect("write the source file");
        let from_file = call(
            &client,
            "edit_dehacked",
            json!({ "source": file.display().to_string() }),
        )
        .await;
        assert_eq!(from_file["lines"], json!(2_i64), "two lines");
        assert_eq!(from_file["changed"], json!(1_i64), "one line rewritten");
        assert_eq!(
            from_file["removed"],
            json!(3_i64),
            "three lines dropped from the five"
        );
        assert_eq!(
            from_file["added"],
            json!(0_i64),
            "the foo value is a rewrite"
        );
        let after = call(&client, "get_dehacked", json!({ "include_text": true })).await;
        assert_eq!(
            after["text"],
            json!("Misc\nfoo = 2\n"),
            "the fetched bytes won"
        );
    }

    #[tokio::test]
    async fn an_edit_is_one_undoable_step() {
        let client = connect().await;
        call(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "Misc", "key": "foo", "value": "1" }] }),
        )
        .await;
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["edit_dehacked"]),
            "the label is exactly the tool name"
        );
        let (code, _, _) = call_err(&client, "get_dehacked", json!({})).await;
        assert_eq!(code, "not_found", "the created lump is gone");
    }

    #[tokio::test]
    async fn edit_refuses_zero_or_two_modes_and_malformed_arguments() {
        let client = connect().await;
        let (code, message, _) = call_err(&client, "edit_dehacked", json!({})).await;
        assert_eq!(code, "invalid_argument", "no mode code");
        assert!(message.contains("exactly one"), "the rule: {message}");
        let (both_code, _, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "text": "x\n", "set": [{ "block": "Misc", "key": "a", "value": "1" }] }),
        )
        .await;
        assert_eq!(both_code, "invalid_argument", "two modes code");
        let (block_code, _, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "Gong 3", "key": "a", "value": "1" }] }),
        )
        .await;
        assert_eq!(block_code, "invalid_argument", "bad block code");
        let (value_code, value_message, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "Misc", "key": "a", "value": "1\n2" }] }),
        )
        .await;
        assert_eq!(value_code, "invalid_argument", "newline value code");
        assert!(
            value_message.contains("newlines"),
            "the rule: {value_message}"
        );
        let (empty_code, _, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Misc", "key": " " }] }),
        )
        .await;
        assert_eq!(empty_code, "invalid_argument", "blank key code");
        let (name_code, _, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "name": "TOO LONG", "text": "x\n" }),
        )
        .await;
        assert_eq!(name_code, "invalid_argument", "bad lump name code");
        assert!(
            client.list_all_tools().await.is_ok(),
            "the session stayed healthy"
        );
    }

    #[tokio::test]
    async fn removal_from_an_absent_lump_is_not_found_and_creates_nothing() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Thing 12" }] }),
        )
        .await;
        assert_eq!(code, "not_found", "no lump to remove from");
        assert!(
            message.contains("none of the loaded"),
            "the message: {message}"
        );
        let info = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            info["undo"].as_array().expect("undo stack").len(),
            0,
            "no history"
        );
    }

    #[tokio::test]
    async fn edit_decorate_appending_creates_the_lump_and_feeds_get_thing_types() {
        let client = connect().await;
        let first = call(
            &client,
            "edit_decorate",
            json!({ "op": "append", "text": "actor TestImp 20100 {\n  radius 24\n}\n" }),
        )
        .await;
        assert_eq!(first["lump"], json!("DECORATE"), "the default lump");
        assert_eq!(first["created"], json!(true), "the lump was created");
        assert_eq!(first["lines"], json!(3_i64), "three lines");
        assert_eq!(
            first["changed_range"],
            json!([1_i64, 3_i64]),
            "all of them new"
        );
        assert_eq!(first["warnings"], json!([]), "balanced braces");
        let again = call(
            &client,
            "edit_decorate",
            json!({ "op": "append", "text": "actor Other {\n}\n" }),
        )
        .await;
        assert_eq!(
            again["created"],
            json!(false),
            "the working copy is edited in place"
        );
        assert_eq!(
            again["changed_range"],
            json!([4_i64, 5_i64]),
            "the two added lines"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "DECORATE", "format": "text" }),
        )
        .await;
        assert_eq!(
            read["text"],
            json!("actor TestImp 20100 {\n  radius 24\n}\nactor Other {\n}\n"),
            "the appends landed"
        );
        let things = call(&client, "get_thing_types", json!({ "type": 20_100_i64 })).await;
        let row = things["things"].get(0).expect("one row").clone();
        assert_eq!(row["name"], json!("TestImp"), "the actor is a thing");
        assert_eq!(row["radius"], json!(24_i64), "its radius");
        assert_eq!(
            row["source"],
            json!("decorate"),
            "read through the merged view"
        );
    }

    #[tokio::test]
    async fn edit_zscript_replace_lines_reports_the_numbered_excerpt() {
        let client = connect().await;
        let created = call(
            &client,
            "edit_zscript",
            json!({
                "op": "replace_all",
                "text": "class Candle : Actor\n{\n  Default { Radius 16; }\n}\n"
            }),
        )
        .await;
        assert_eq!(created["lump"], json!("ZSCRIPT"), "the default lump");
        assert_eq!(created["lines"], json!(4_i64), "four lines");
        let edited = call(
            &client,
            "edit_zscript",
            json!({
                "op": "replace_lines",
                "start": 3_i64,
                "end": 3_i64,
                "text": "  Default { Radius 20; DoomNum 25000; }"
            }),
        )
        .await;
        assert_eq!(
            edited["changed_range"],
            json!([3_i64, 3_i64]),
            "one line rewritten"
        );
        assert_eq!(
            edited["excerpt"],
            "1: class Candle : Actor\n2: {\n3:   Default { Radius 20; DoomNum 25000; }\n4: }",
            "the numbered window with two lines of context"
        );
        assert_eq!(
            edited["created"],
            json!(false),
            "the replace_all copy is the working one"
        );
        assert_eq!(edited["warnings"], json!([]), "braces still balanced");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "ZSCRIPT", "format": "text" }),
        )
        .await;
        assert_eq!(
            read["text"],
            json!("class Candle : Actor\n{\n  Default { Radius 20; DoomNum 25000; }\n}\n"),
            "the new line stored"
        );
    }

    #[tokio::test]
    async fn editing_an_included_layer_lump_copies_on_write() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad(dir.path());
        let custom = pwad_custom_defs(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        call(
            &client,
            "merge_wad",
            json!({ "path": custom.display().to_string() }),
        )
        .await;
        let before = call(&client, "get_thing_types", json!({ "type": 12_345_i64 })).await;
        assert_eq!(
            before["things"].get(0).expect("the Torch row")["radius"],
            json!(10_i64),
            "the fixture radius"
        );
        let edit = call(
            &client,
            "edit_zscript",
            json!({
                "lump": "ZEXTRA",
                "op": "find_replace",
                "find": "Radius 10",
                "replace": "Radius 30"
            }),
        )
        .await;
        assert_eq!(edit["lump"], json!("ZEXTRA"), "the #include-d file");
        assert_eq!(
            edit["created"],
            json!(true),
            "copied into the working archive"
        );
        assert_eq!(
            edit["changed_range"],
            json!([1_i64, 1_i64]),
            "the single rewritten line"
        );
        assert_eq!(edit["warnings"], json!([]), "braces stay balanced");
        let after = call(&client, "get_thing_types", json!({ "type": 12_345_i64 })).await;
        let row = after["things"].get(0).expect("the Torch row").clone();
        assert_eq!(
            row["radius"],
            json!(30_i64),
            "the override feeds the definitions"
        );
        assert_eq!(row["source"], json!("zscript"), "still read as ZScript");
    }

    #[tokio::test]
    async fn refused_line_ranges_answer_as_invalid_argument() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "edit_decorate",
            json!({ "op": "delete_lines", "start": 1_i64, "end": 1_i64 }),
        )
        .await;
        assert_eq!(code, "not_found", "no lump to delete from");
        assert!(
            message.contains("replace_all") && message.contains("append"),
            "the creating hint: {message}"
        );
        call(
            &client,
            "edit_zscript",
            json!({ "op": "replace_all", "text": "a\nb\n" }),
        )
        .await;
        let (range_code, range_message, _) = call_err(
            &client,
            "edit_zscript",
            json!({ "op": "replace_lines", "start": 2_i64, "end": 1_i64, "text": "x" }),
        )
        .await;
        assert_eq!(range_code, "invalid_argument", "start after end");
        assert!(
            range_message.contains("out of range"),
            "the bounds hint: {range_message}"
        );
        let (past_code, past_message, _) = call_err(
            &client,
            "edit_zscript",
            json!({ "op": "insert_lines", "line": 4_i64, "text": "x" }),
        )
        .await;
        assert_eq!(past_code, "invalid_argument", "past line count + 1");
        assert!(
            past_message.contains("3 appends"),
            "the append position: {past_message}"
        );
    }

    #[tokio::test]
    async fn refused_find_replace_and_lump_names_change_nothing() {
        let client = connect().await;
        call(
            &client,
            "edit_zscript",
            json!({ "op": "replace_all", "text": "a\nb\n" }),
        )
        .await;
        let (find_code, _, _) = call_err(
            &client,
            "edit_zscript",
            json!({ "op": "find_replace", "find": "zzz", "replace": "x" }),
        )
        .await;
        assert_eq!(find_code, "not_found", "no match");
        let (empty_code, _, _) = call_err(
            &client,
            "edit_zscript",
            json!({ "op": "find_replace", "find": "", "replace": "x" }),
        )
        .await;
        assert_eq!(empty_code, "invalid_argument", "empty find");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "ZSCRIPT", "format": "text" }),
        )
        .await;
        assert_eq!(
            read["text"],
            json!("a\nb\n"),
            "the refused edits changed nothing"
        );
        let (name_code, _, _) = call_err(
            &client,
            "edit_decorate",
            json!({ "lump": "TOO LONG", "op": "append", "text": "x" }),
        )
        .await;
        assert_eq!(name_code, "invalid_argument", "bad lump name");
        assert!(
            client.list_all_tools().await.is_ok(),
            "the session stayed healthy"
        );
    }

    #[tokio::test]
    async fn text_edits_are_one_undoable_step_each() {
        let client = connect().await;
        call(
            &client,
            "edit_decorate",
            json!({ "op": "replace_all", "text": "actor A {\n}\n" }),
        )
        .await;
        call(
            &client,
            "edit_zscript",
            json!({ "op": "append", "text": "class B : Actor { }\n" }),
        )
        .await;
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["edit_zscript ZSCRIPT"]),
            "the label is the tool and the lump"
        );
        let (code, _, _) = call_err(&client, "get_lump", json!({ "name": "ZSCRIPT" })).await;
        assert_eq!(code, "not_found", "the created ZSCRIPT is gone");
        let decorate = call(
            &client,
            "get_lump",
            json!({ "name": "DECORATE", "format": "text" }),
        )
        .await;
        assert_eq!(
            decorate["text"],
            json!("actor A {\n}\n"),
            "the earlier DECORATE edit stands"
        );
        let redo = call(&client, "redo", json!({})).await;
        assert_eq!(
            redo["redone"],
            json!(["edit_zscript ZSCRIPT"]),
            "and replays"
        );
    }

    #[tokio::test]
    async fn brace_warnings_count_only_the_code_text() {
        let client = connect().await;
        let report = call(
            &client,
            "edit_decorate",
            json!({
                "op": "replace_all",
                "text": "// an open brace {\nactor Broken 20111 {\n  msg = \"}{\"\n"
            }),
        )
        .await;
        assert_eq!(
            report["warnings"],
            json!(["unbalanced braces: 1 '{' vs 0 '}' outside strings and comments"]),
            "the comment and string braces never counted"
        );
    }

    #[tokio::test]
    async fn the_text_editors_advertise_their_tagged_operation_schemas() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        assert_text_tool_schema(&tools, "edit_decorate");
        assert_text_tool_schema(&tools, "edit_zscript");
    }

    /// Assert one text editor's schema keeps the `lump` selector and the
    /// six `op`-tagged branches.
    ///
    /// # Panics
    ///
    /// Panics when the tool is not listed or its schema lacks the expected
    /// fields.
    fn assert_text_tool_schema(tools: &[rmcp::model::Tool], wanted: &str) {
        let tool = tools
            .iter()
            .find(|listed| listed.name == wanted)
            .unwrap_or_else(|| panic!("{wanted} is listed"));
        let schema = serde_json::to_value(&tool.input_schema).expect("a serializable schema");
        assert!(
            schema["properties"]["lump"].is_object(),
            "{wanted} keeps the lump selector: {schema}"
        );
        let branches = schema["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{wanted} has one branch per operation"))
            .clone();
        let tags: Vec<&str> = branches
            .iter()
            .filter_map(|branch| branch.get("properties")?.get("op")?.get("const")?.as_str())
            .collect();
        assert_eq!(
            tags,
            vec![
                "replace_all",
                "append",
                "find_replace",
                "insert_lines",
                "replace_lines",
                "delete_lines"
            ],
            "{wanted} offers the six tagged operations"
        );
    }
}
