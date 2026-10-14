//! The `scripts` tool group as seen by a real MCP client: `get_dehacked`
//! reading fixture patches and `edit_dehacked` creating, copying-on-write,
//! setting, removing and replacing DeHackEd/BEX lump texts, with undo and
//! the `get_thing_types` round trip.

#[cfg(test)]
mod tests {
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad};
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
        let (code, message) = call_err(&client, "get_dehacked", json!({})).await;
        assert_eq!(code, "not_found", "nothing is loaded code");
        assert!(
            message.contains("edit_dehacked"),
            "the message names the creating tool: {message}"
        );
        let (scope_code, _) = call_err(&client, "get_dehacked", json!({ "scope": "nope" })).await;
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
            report["changed_lines"],
            json!(3_i64),
            "blank, header and key added"
        );
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
        assert_eq!(again["changed_lines"], json!(1_i64), "one line rewritten");
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
        assert_eq!(
            report["changed_lines"], report["lines"],
            "everything is new"
        );
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
        let (code, message) = call_err(
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
        let (key_code, _) = call_err(
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
        assert_eq!(removed["changed_lines"], json!(1_i64), "one line went");
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
            report["changed_lines"],
            json!(5_i64),
            "one old line replaced, four added"
        );
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
        assert_eq!(
            from_file["changed_lines"],
            json!(4_i64),
            "one differs, three dropped"
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
        let (code, _) = call_err(&client, "get_dehacked", json!({})).await;
        assert_eq!(code, "not_found", "the created lump is gone");
    }

    #[tokio::test]
    async fn edit_refuses_zero_or_two_modes_and_malformed_arguments() {
        let client = connect().await;
        let (code, message) = call_err(&client, "edit_dehacked", json!({})).await;
        assert_eq!(code, "invalid_argument", "no mode code");
        assert!(message.contains("exactly one"), "the rule: {message}");
        let (both_code, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "text": "x\n", "set": [{ "block": "Misc", "key": "a", "value": "1" }] }),
        )
        .await;
        assert_eq!(both_code, "invalid_argument", "two modes code");
        let (block_code, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "set": [{ "block": "Gong 3", "key": "a", "value": "1" }] }),
        )
        .await;
        assert_eq!(block_code, "invalid_argument", "bad block code");
        let (value_code, value_message) = call_err(
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
        let (empty_code, _) = call_err(
            &client,
            "edit_dehacked",
            json!({ "remove": [{ "block": "Misc", "key": " " }] }),
        )
        .await;
        assert_eq!(empty_code, "invalid_argument", "blank key code");
        let (name_code, _) = call_err(
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
        let (code, message) = call_err(
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
}
