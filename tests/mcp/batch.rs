//! The `batch` tool group as seen by a real MCP client: a whole editing
//! sequence applied by `run_batch` and reverted by one `undo` (and replayed
//! by one `redo`), the atomic rollback of a failing step, the non-atomic
//! run that keeps the successful steps, `$ref` chaining between steps, the
//! batch-level refusals that run nothing, the session's runtime refusals of
//! the whole-archive and saved-state tools while a batch is open, and the
//! check that every listed tool except the refused groups dispatches
//! through the batch.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::model::Tool;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The tools a batch may not run.
    const FORBIDDEN: [&str; 8] = [
        "run_batch",
        "undo",
        "redo",
        "open_iwad",
        "open_wad",
        "new_wad",
        "merge_wad",
        "save_wad",
    ];

    /// The default label of [`build_steps`].
    const BUILD_LABEL: &str = "batch: edit_sector, add_thing, edit_sector";

    /// Open the `DOOM II` fixture (one two-room binary `Doom` `MAP01`:
    /// lights 160/192, sector 1 floored at 32, two things) as the base.
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

    /// One batch step.
    fn step<T: serde::Serialize>(tool: &str, arguments: T) -> Value {
        json!({"tool": tool, "arguments": arguments})
    }

    /// The batch parameter object.
    fn steps(list: &[Value]) -> Value {
        json!({"steps": list})
    }

    /// Three steps: darken sector 0, add a medikit, lower sector 1's floor.
    fn build_steps() -> Vec<Value> {
        vec![
            step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
            step(
                "add_thing",
                json!({"type": 2011_i32, "x": 100_i32, "y": 100_i32}),
            ),
            step(
                "edit_sector",
                json!({"index": 1_i32, "floor_height": 24_i32}),
            ),
        ]
    }

    /// The `assemble_map` report row of one sector, from `scope`.
    async fn sector(client: &Client, index: usize, scope: &str) -> Value {
        let report = call(
            client,
            "assemble_map",
            json!({"map": "MAP01", "scope": scope, "include": ["sectors"]}),
        )
        .await;
        report["sectors"]
            .as_array()
            .expect("sectors is an array")
            .get(index)
            .cloned()
            .expect("the sector row exists")
    }

    /// The `assemble_map` thing rows of one scope.
    async fn things(client: &Client, scope: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": "MAP01", "scope": scope, "include": ["things"]}),
        )
        .await;
        report["things"]
            .as_array()
            .expect("things is an array")
            .clone()
    }

    /// Requires all three of [`build_steps`]' changes to stand in `scope`.
    async fn assert_applied(client: &Client, scope: &str) {
        assert_eq!(
            sector(client, 0, scope).await["light"],
            json!(64_i32),
            "the light edit applied"
        );
        assert_eq!(
            sector(client, 1, scope).await["floor_height"],
            json!(24_i32),
            "the floor edit applied"
        );
        assert_eq!(things(client, scope).await.len(), 3, "the medikit is in");
    }

    /// The working archive's lump names, in directory order.
    async fn working_names(client: &Client) -> Vec<String> {
        let listing = call(client, "list_lumps", json!({"scope": "working"})).await;
        listing["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("rows carry a name").to_owned())
            .collect()
    }

    /// Adds `count` marker lumps named `MARK00..`, one undoable edit each.
    async fn fill_history(client: &Client, count: usize) {
        for index in 0_usize..count {
            let name = format!("MARK{index:02}");
            call(client, "add_lump", json!({"name": name})).await;
        }
    }

    /// The names `list_all_tools` reports that a batch may run.
    fn callable_names(tools: &[Tool]) -> Vec<String> {
        tools
            .iter()
            .map(|tool| tool.name.to_string())
            .filter(|name| !FORBIDDEN.contains(&name.as_str()))
            .collect()
    }

    /// Runs a one-step `run_batch` for every name: the call itself must
    /// succeed (a refusal means the batch cannot dispatch the tool), and
    /// the step is reported under the tool's own name.
    async fn dispatched(client: &Client, names: &[String]) -> usize {
        let mut count = 0_usize;
        for name in names {
            let report = call(client, "run_batch", steps(&[json!({"tool": name})])).await;
            let ran = report["steps"].as_array().expect("one step reported");
            assert_eq!(ran.len(), 1, "{name} dispatched as one step");
            assert_eq!(ran[0]["tool"], json!(name), "{name} is the step");
            count = count.checked_add(1).expect("a test count");
        }
        count
    }

    /// Requires `run_batch` to refuse `args` with `invalid_argument`.
    async fn refuses(client: &Client, args: Value, note: &str) {
        let (code, _, _) = call_err(client, "run_batch", args).await;
        assert_eq!(code, "invalid_argument", "{note} refuses");
    }

    #[tokio::test]
    async fn a_batch_applies_every_step_and_one_undo_reverts_the_whole_of_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let report = call(&client, "run_batch", steps(&build_steps())).await;
        assert_eq!(report["ok"], json!(true), "all three steps ran: {report}");
        assert_eq!(report["rolled_back"], json!(false), "nothing failed");
        assert_eq!(report["steps"].as_array().expect("steps").len(), 3);
        assert_eq!(
            report["undo_label"],
            json!(BUILD_LABEL),
            "the default label lists the tools"
        );
        assert_applied(&client, "working").await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!([BUILD_LABEL]),
            "one undo takes back all three edits"
        );
        assert_eq!(
            sector(&client, 0, "merged").await["light"],
            json!(160_i32),
            "the undone light is the fixture's again"
        );
        assert_eq!(
            things(&client, "merged").await.len(),
            2,
            "the added thing is gone from the view"
        );
        let redone = call(&client, "redo", json!({})).await;
        assert_eq!(
            redone["redone"],
            json!([BUILD_LABEL]),
            "one redo re-applies the whole batch"
        );
        assert_applied(&client, "working").await;
    }

    /// Two applied edits and a refused sector index: the failing plan of a
    /// three-step batch whose third step is the bad one.
    fn two_then_bad() -> Vec<Value> {
        vec![
            step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
            step(
                "add_thing",
                json!({"type": 2011_i32, "x": 100_i32, "y": 100_i32}),
            ),
            step("edit_sector", json!({"index": 999_i32, "light": 1_i32})),
        ]
    }

    #[tokio::test]
    async fn a_failing_atomic_step_rolls_the_batch_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let report = call(&client, "run_batch", steps(&two_then_bad())).await;
        assert_eq!(report["ok"], json!(false), "a step failed");
        assert_eq!(
            report["rolled_back"],
            json!(true),
            "the atomic batch rolled back"
        );
        assert_eq!(
            report["undo_label"],
            Value::Null,
            "nothing was left to undo"
        );
        let steps = report["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 3, "the run stopped at the failure");
        assert_eq!(steps[0]["ok"], json!(true), "the first step ran");
        assert_eq!(steps[1]["ok"], json!(true), "the second step ran");
        assert_eq!(steps[2]["ok"], json!(false), "the third step failed");
        assert_eq!(
            steps[2]["error"]["error"],
            json!("not_found"),
            "the refused sector index is reported: {}",
            steps[2]
        );
        assert_eq!(
            steps[2]["result"],
            Value::Null,
            "a failed step has no result"
        );
        assert_eq!(
            sector(&client, 0, "merged").await["light"],
            json!(160_i32),
            "the rolled-back map shows the layers untouched"
        );
        assert!(
            working_names(&client).await.is_empty(),
            "the working archive holds nothing, not even the copied-in block"
        );
        let (code, _, _) = call_err(&client, "undo", json!({})).await;
        assert_eq!(
            code, "not_found",
            "the rollback consumed the batch's history"
        );
    }

    #[tokio::test]
    async fn a_non_atomic_batch_keeps_the_successful_steps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let mut args = steps(&two_then_bad());
        args["atomic"] = json!(false);
        args["label"] = json!("build the room");
        let report = call(&client, "run_batch", args).await;
        assert_eq!(report["ok"], json!(false), "one step still failed");
        assert_eq!(
            report["rolled_back"],
            json!(false),
            "a non-atomic batch keeps going"
        );
        assert_eq!(
            report["undo_label"],
            json!("build the room"),
            "the given label stands"
        );
        let steps = report["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 3, "every step ran");
        assert_eq!(
            steps[2]["error"]["error"],
            json!("not_found"),
            "and the failure is reported"
        );
        assert_eq!(
            sector(&client, 0, "working").await["light"],
            json!(64_i32),
            "the successful light edit stands"
        );
        assert_eq!(things(&client, "working").await.len(), 3, "and the thing");
        let undone = call(&client, "undo", json!({ "steps": 1_usize })).await;
        assert_eq!(
            undone["undone"],
            json!(["build the room"]),
            "the kept edits are one undo step"
        );
        assert!(
            working_names(&client).await.is_empty(),
            "one undo removed all of them"
        );
    }

    #[tokio::test]
    async fn a_ref_feeds_the_added_things_index_into_the_next_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step(
                    "add_thing",
                    json!({"type": 2011_i32, "x": 100_i32, "y": 100_i32}),
                ),
                step(
                    "edit_thing",
                    json!({"index": {"$ref": "0/index"}, "angle": 90_i32}),
                ),
            ]),
        )
        .await;
        assert_eq!(report["ok"], json!(true), "both steps ran: {report}");
        let added = report["steps"][0]["result"]["index"].clone();
        assert!(
            added.is_number(),
            "step 0 reported the new thing's index: {}",
            report["steps"][0]["result"]
        );
        assert_eq!(
            report["steps"][1]["result"]["things"][0]["index"], added,
            "the reference carried step 0's new thing index into step 1"
        );
        assert_eq!(
            report["steps"][1]["result"]["things"][0]["angle"],
            json!(90_i32),
            "and the edit applied to that thing"
        );
        let rows = things(&client, "working").await;
        let placed = rows
            .iter()
            .find(|row| row["index"] == added)
            .expect("the added thing is in the map");
        assert_eq!(placed["angle"], json!(90_i32), "verified on the map");
    }

    #[tokio::test]
    async fn a_forward_reference_fails_only_its_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step(
                    "edit_sector",
                    json!({"index": {"$ref": "1/index"}, "light": 64_i32}),
                ),
                step("get_session_info", Value::Null),
            ]),
        )
        .await;
        assert_eq!(report["ok"], json!(false), "the reference refused");
        assert_eq!(
            report["rolled_back"],
            json!(true),
            "the atomic batch stopped at step 0"
        );
        let steps = report["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "step 1 never ran");
        assert_eq!(
            steps[0]["error"]["error"],
            json!("invalid_argument"),
            "a forward reference is invalid_argument"
        );
        assert!(
            steps[0]["error"]["message"]
                .as_str()
                .expect("a message")
                .contains("has not run yet"),
            "the message says why: {}",
            steps[0]["error"]
        );
    }

    #[tokio::test]
    async fn forbidden_unknown_oversized_and_empty_batches_run_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "add_lump", json!({"name": "MARK"})).await;
        let (code, message, _) = call_err(
            &client,
            "run_batch",
            steps(&[
                step("add_lump", json!({"name": "SHOULD_NOT_EXIST"})),
                step("undo", Value::Null),
            ]),
        )
        .await;
        assert_eq!(code, "invalid_argument", "a forbidden tool refuses");
        assert!(
            message.contains("step 1") && message.contains("undo"),
            "the refusal names the step and tool: {message}"
        );
        let (unknown_code, unknown_message, _) = call_err(
            &client,
            "run_batch",
            steps(&[step("no_such_tool", Value::Null)]),
        )
        .await;
        assert_eq!(unknown_code, "invalid_argument", "an unknown tool refuses");
        assert!(
            unknown_message.contains("step 0") && unknown_message.contains("no_such_tool"),
            "the refusal names the step and tool: {unknown_message}"
        );
        let many: Vec<Value> = (0_usize..51_usize)
            .map(|_| step("get_session_info", Value::Null))
            .collect();
        refuses(&client, steps(&many), "51 steps").await;
        refuses(&client, steps(&[]), "no steps").await;
        assert_eq!(
            working_names(&client).await,
            vec!["MARK".to_owned()],
            "none of the refused batches ran anything"
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["undo"],
            json!(["add_lump MARK"]),
            "and recorded no history"
        );
    }

    #[tokio::test]
    async fn a_batch_that_would_overrun_the_history_is_refused_before_running() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        fill_history(&client, 75).await;
        // 30 steps fit the count limit but 75 + 30 > 100, so the rollback
        // could not reach the batch's start: the batch is refused outright.
        let many: Vec<Value> = (0_usize..30_usize)
            .map(|_| step("add_lump", json!({"name": "OVERFLOW"})))
            .collect();
        let (code, message, _) = call_err(&client, "run_batch", steps(&many)).await;
        assert_eq!(code, "invalid_argument", "75 + 30 > 100 refuses");
        assert!(
            message.contains("undo") || message.contains("split"),
            "the message says what to do: {message}"
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["undo"].as_array().expect("undo").len(),
            75,
            "nothing ran"
        );
    }

    #[tokio::test]
    async fn every_listed_tool_except_the_forbidden_ones_is_dispatchable() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let names = callable_names(&tools);
        assert!(names.len() >= 70, "the catalog is large: {}", names.len());
        let checked = dispatched(&client, &names).await;
        assert_eq!(checked, names.len(), "every name dispatched");
    }

    #[tokio::test]
    async fn a_read_only_batch_records_no_undo_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step("get_session_info", Value::Null),
                step("list_maps", Value::Null),
            ]),
        )
        .await;
        assert_eq!(report["ok"], json!(true), "both reads ran");
        assert_eq!(
            report["undo_label"],
            Value::Null,
            "a batch that edited nothing leaves no undo entry"
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(info["undo"], json!([]), "the history is untouched");
    }

    /// Writes the merged view to a file outside any batch and returns its
    /// `{"path": ...}` arguments.
    async fn saved_view(client: &Client, dir: &Path, file: &str) -> Value {
        let path = dir.join(file);
        call(
            client,
            "save_wad",
            json!({"path": path.display().to_string(), "mode": "full"}),
        )
        .await;
        json!({"path": path.display().to_string()})
    }

    #[tokio::test]
    async fn the_working_archive_swap_is_refused_while_the_batch_runs() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let snapshot = dir.path().join("snapshot.wad");
        call(
            &client,
            "save_wad",
            json!({"path": snapshot.display().to_string()}),
        )
        .await;
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
                step(
                    "open_wad",
                    json!({
                        "path": snapshot.display().to_string(),
                        "discard_changes": true,
                    }),
                ),
            ]),
        )
        .await;
        assert_eq!(report["ok"], json!(false), "the swap refused: {report}");
        assert_eq!(
            report["rolled_back"],
            json!(true),
            "the atomic batch rolled the edit back"
        );
        assert_eq!(
            report["steps"][1]["error"]["error"],
            json!("conflict"),
            "the session refuses it: {}",
            report["steps"][1]["error"]
        );
        assert_eq!(
            sector(&client, 0, "merged").await["light"],
            json!(160_i32),
            "the refused swap and the rolled-back fold left the layers' map alone"
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(info["undo"], json!([]), "the fold left no entry");
        let alone = call(
            &client,
            "open_wad",
            json!({
                "path": snapshot.display().to_string(),
                "discard_changes": true,
            }),
        )
        .await;
        assert_eq!(
            alone["path"],
            json!(snapshot.display().to_string()),
            "between batches the same call runs"
        );
    }

    #[tokio::test]
    async fn a_pwad_save_step_is_refused_before_the_write_and_a_full_export_runs() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let saved = dir.path().join("mid-batch.wad");
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
                step("save_wad", json!({"path": saved.display().to_string()})),
            ]),
        )
        .await;
        assert_eq!(report["rolled_back"], json!(true), "the save refused");
        assert_eq!(
            report["steps"][1]["error"]["error"],
            json!("conflict"),
            "the session refuses the save mark: {}",
            report["steps"][1]["error"]
        );
        assert!(
            !saved.try_exists().expect("a testable path"),
            "the refusal comes before the write"
        );
        let packed = dir.path().join("packed.wad");
        let exported = call(
            &client,
            "run_batch",
            steps(&[
                step("edit_sector", json!({"index": 0_i32, "light": 96_i32})),
                step(
                    "save_wad",
                    json!({"path": packed.display().to_string(), "mode": "full"}),
                ),
            ]),
        )
        .await;
        assert_eq!(exported["ok"], json!(true), "the export ran: {exported}");
        assert!(
            packed.try_exists().expect("a testable path"),
            "mode full writes while the batch runs"
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["undo"].as_array().expect("undo").len(),
            1,
            "the export recorded nothing; the edit folded into one entry"
        );
    }

    #[tokio::test]
    async fn a_working_merge_step_folds_into_the_batch_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let view = saved_view(&client, dir.path(), "packed.wad").await;
        let mut merge_args = view.clone();
        merge_args["mode"] = json!("working");
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
                step("merge_wad", &merge_args),
            ]),
        )
        .await;
        assert_eq!(report["ok"], json!(true), "both steps ran: {report}");
        assert_eq!(
            report["steps"][1]["result"]["mode"],
            json!("working"),
            "the merge folded"
        );
        let label = report["undo_label"].clone();
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["undo"],
            json!([label]),
            "the merge edit stands inside the batch's one entry"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(undone["undone"], json!([label]), "one undo takes both");
        assert!(
            working_names(&client).await.is_empty(),
            "the fold's pre-batch archive is back"
        );
    }

    #[tokio::test]
    async fn a_layer_merge_step_is_refused_and_rolls_the_batch_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let view = saved_view(&client, dir.path(), "extras.wad").await;
        let mut layer_args = view.clone();
        layer_args["mode"] = json!("layer");
        let report = call(
            &client,
            "run_batch",
            steps(&[
                step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
                step("merge_wad", &layer_args),
            ]),
        )
        .await;
        assert_eq!(report["rolled_back"], json!(true), "the layer refused");
        assert_eq!(
            report["steps"][1]["error"]["error"],
            json!("conflict"),
            "the session refuses the stack: {}",
            report["steps"][1]["error"]
        );
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["layers"].as_array().expect("layers").len(),
            1,
            "no layer was stacked"
        );
    }

    #[tokio::test]
    async fn a_nested_run_batch_step_is_refused_by_the_plan() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message, _) = call_err(
            &client,
            "run_batch",
            steps(&[
                step("add_lump", json!({"name": "SHOULD_NOT_EXIST"})),
                step(
                    "run_batch",
                    steps(&[step("add_lump", json!({"name": "NESTED"}))]),
                ),
            ]),
        )
        .await;
        assert_eq!(code, "invalid_argument", "a nested batch refuses");
        assert!(
            message.contains("step 1") && message.contains("run_batch"),
            "the refusal names the step and tool: {message}"
        );
        assert!(
            working_names(&client).await.is_empty(),
            "the refused plan ran nothing"
        );
    }

    #[tokio::test]
    async fn a_failing_inner_edit_folds_nothing_into_the_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let mut args = steps(&[
            step("edit_sector", json!({"index": 0_i32, "light": 64_i32})),
            step("edit_sector", json!({"index": 999_i32, "light": 1_i32})),
        ]);
        args["atomic"] = json!(false);
        args["label"] = json!("partial");
        let report = call(&client, "run_batch", args).await;
        assert_eq!(report["ok"], json!(false), "one step failed");
        assert_eq!(
            report["rolled_back"],
            json!(false),
            "a non-atomic batch keeps the good edit"
        );
        assert_eq!(report["undo_label"], json!("partial"), "one entry");
        let info = call(&client, "get_session_info", json!(null)).await;
        assert_eq!(
            info["undo"],
            json!(["partial"]),
            "the failed edit left nothing beside the fold"
        );
        assert_eq!(
            sector(&client, 0, "working").await["light"],
            json!(64_i32),
            "the good edit stands"
        );
        call(&client, "undo", json!({})).await;
        assert!(
            working_names(&client).await.is_empty(),
            "one undo reverts the fold to the pre-batch archive"
        );
    }
}
