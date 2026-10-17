//! The `edit_vertices` tool group as seen by a real MCP client: `add_vertex`
//! appending a point, reusing one that is already there without an edit and
//! refusing coordinates the binary format cannot store (`UDMF` taking them),
//! `move_vertex` placing one point and shifting a list, its parameter
//! conflicts, the `conflict` of a move that would collapse a linedef and the
//! `not_found` of an index past the vertex block, and `remove_vertex` with
//! the `conflict` on a used point, the cascaded removal verified through
//! `get_map_summary`, the pruning switches and the undo of every whole call.

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

    /// The fixture map's element counts, as `get_map_summary` reports them.
    fn fixture_counts() -> Value {
        json!({
            "things": 2_i32, "vertices": 6_i32, "linedefs": 7_i32,
            "sidedefs": 8_i32, "sectors": 2_i32,
        })
    }

    /// Open the `DOOM II` fixture (one two-room binary `Doom` `MAP01`) as
    /// the base layer and start an empty working archive over it.
    async fn doom_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        client
    }

    /// Open the map-format fixture (`UDMF` `MAP03`, binary `Hexen` `MAP04`,
    /// both the two-room map) as the base layer.
    async fn formats_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let extra = pwad_map_formats(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": extra.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        client
    }

    /// The working archive's vertex rows for `map`, in order, through
    /// `assemble_map`.
    async fn working_vertices(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["vertices"]}),
        )
        .await;
        report["vertices"]
            .as_array()
            .expect("vertices is an array")
            .clone()
    }

    /// The five element counts of `map` in `scope`, as `get_map_summary`
    /// reports them.
    async fn counts(client: &Client, map: &str, scope: &str) -> Value {
        let summary = call(
            client,
            "get_map_summary",
            json!({"map": map, "scope": scope}),
        )
        .await;
        summary["counts"].clone()
    }

    /// The labels `undo` would take back, newest last.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", json!({})).await;
        info["undo"]
            .as_array()
            .expect("undo is an array")
            .iter()
            .map(|label| label.as_str().expect("labels are strings").to_owned())
            .collect()
    }

    /// Requires `tool` to reject `args` with `invalid_argument`.
    async fn refuse(client: &Client, tool: &str, args: Value) {
        let note = args.to_string();
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
    }

    /// Requires a call to have left the working archive untouched: no map
    /// block copied in and no undo entry.
    async fn assert_untouched(client: &Client) {
        let (code, _) = call_err(
            client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(code, "not_found", "the working archive holds no map");
        assert!(undo_labels(client).await.is_empty(), "no undo entry");
    }

    #[tokio::test]
    async fn add_vertex_appends_a_point_and_the_nodes_stay_trusted() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "add_vertex", json!({"x": 128_i32, "y": 128_i32})).await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["index"], json!(6_i32), "appended after the six");
        assert_eq!(result["x"], json!(128_i32), "the point echoes");
        assert_eq!(result["y"], json!(128_i32));
        assert_eq!(result["reused"], json!(false), "a new vertex was stored");
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "no linedef ends at the appended point, so the node builder sees no change"
        );
        let rows = working_vertices(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "the map holds seven vertices now");
        assert_eq!(
            rows[6],
            json!({"index": 6_i32, "x": 128_i32, "y": 128_i32, "linedefs": []}),
            "the new point stands alone, referenced by no line"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["add vertex at 128, 128".to_owned()],
            "one undo step for the whole call"
        );
    }

    #[tokio::test]
    async fn add_vertex_reuses_an_existing_point_without_editing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "add_vertex", json!({"x": 256_i32, "y": 0_i32})).await;
        assert_eq!(result["index"], json!(1_i32), "vertex 1 is already here");
        assert_eq!(result["reused"], json!(true), "so nothing was appended");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "a reuse changes no geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(false),
            "and copies nothing in"
        );
        assert_untouched(&client).await;
        let forced = call(
            &client,
            "add_vertex",
            json!({"x": 256_i32, "y": 0_i32, "reuse_existing": false}),
        )
        .await;
        assert_eq!(forced["index"], json!(6_i32), "switched off, it appends");
        assert_eq!(forced["reused"], json!(false), "and reports the new point");
        assert_eq!(
            counts(&client, "MAP01", "working").await["vertices"],
            json!(7_i32),
            "two vertices now share the point"
        );
    }

    #[tokio::test]
    async fn add_vertex_refuses_coordinates_the_binary_format_cannot_store() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) =
            call_err(&client, "add_vertex", json!({"x": 40000_i32, "y": 0_i32})).await;
        assert_eq!(code, "invalid_argument", "above i16 refuses");
        assert!(
            message.contains("x 40000 is out of range") && message.contains("-32768..=32767"),
            "the message names the field and the range: {message}"
        );
        refuse(&client, "add_vertex", json!({"x": 0_i32, "y": -40000_i32})).await;
        assert_untouched(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the merged view stands"
        );
    }

    #[tokio::test]
    async fn wide_coordinates_land_in_udmf_and_refuse_in_binary_hexen() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_vertex",
            json!({"map": "MAP03", "x": 40000_i32, "y": -40000_i32}),
        )
        .await;
        assert_eq!(result["index"], json!(6_i32), "appended after the six");
        assert_eq!(result["x"], json!(40000_i32), "UDMF stores the wide point");
        assert_eq!(result["y"], json!(-40000_i32));
        let (code, message) = call_err(
            &client,
            "add_vertex",
            json!({"map": "MAP04", "x": 40000_i32, "y": 0_i32}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "a binary Hexen map refuses");
        assert!(
            message.contains("x 40000 is out of range"),
            "the message names the field: {message}"
        );
    }

    #[tokio::test]
    async fn move_vertex_places_one_point_and_shifts_a_whole_list() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let placed = call(
            &client,
            "move_vertex",
            json!({"index": 4_i32, "x": 300_i32, "y": 256_i32}),
        )
        .await;
        assert_eq!(
            placed["moved"],
            json!([{"index": 4_i32, "x": 300_i32, "y": 256_i32}]),
            "the corner is where it was told"
        );
        assert_eq!(
            placed["nodes_invalidated"],
            json!(true),
            "moving a point is geometry"
        );
        assert_eq!(
            placed["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        let shifted = call(
            &client,
            "move_vertex",
            json!({"index": [2_i32, 3_i32], "dx": -8_i32, "dy": 8_i32}),
        )
        .await;
        assert_eq!(
            shifted["moved"],
            json!([
                {"index": 2_i32, "x": 504_i32, "y": 8_i32},
                {"index": 3_i32, "x": 504_i32, "y": 264_i32},
            ]),
            "the list shifted as one block, ascending by index"
        );
        let rows = working_vertices(&client, "MAP01").await;
        assert_eq!(rows[4]["x"], json!(300_i32), "the placement stuck");
        assert_eq!(rows[2]["y"], json!(8_i32), "and so did the shift");
        assert_eq!(rows[0]["x"], json!(0_i32), "an untouched vertex stands");
        assert_eq!(
            undo_labels(&client).await,
            vec!["move 1 vertices".to_owned(), "move 2 vertices".to_owned()],
            "two calls, two undo steps"
        );
    }

    #[tokio::test]
    async fn move_vertex_refuses_a_move_that_collapses_a_linedef() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "move_vertex",
            json!({"index": 5_i32, "x": 0_i32, "y": 0_i32}),
        )
        .await;
        assert_eq!(code, "conflict", "linedef 0 would be zero-length");
        assert!(
            message.contains("linedef(s) 0 zero-length") && message.contains("other endpoint"),
            "the message names the line and the fix: {message}"
        );
        assert_untouched(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the map stands"
        );
    }

    #[tokio::test]
    async fn move_vertex_refuses_mixed_and_empty_parameters() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "move_vertex",
            json!({"index": 0_i32, "x": 1_i32, "dx": 2_i32}),
        )
        .await;
        refuse(
            &client,
            "move_vertex",
            json!({"index": 0_i32, "y": 1_i32, "dy": 2_i32}),
        )
        .await;
        refuse(
            &client,
            "move_vertex",
            json!({"index": 0_i32, "x": 1_i32, "dy": 2_i32}),
        )
        .await;
        refuse(
            &client,
            "move_vertex",
            json!({"index": [0_i32, 1_i32], "x": 4_i32}),
        )
        .await;
        refuse(
            &client,
            "move_vertex",
            json!({"index": [0_i32, 1_i32], "y": 4_i32}),
        )
        .await;
        refuse(&client, "move_vertex", json!({"index": 0_i32})).await;
        refuse(&client, "move_vertex", json!({"index": []})).await;
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn move_vertex_reports_a_missing_index_and_an_unstorable_result() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "move_vertex",
            json!({"index": 99_i32, "dx": 1_i32}),
        )
        .await;
        assert_eq!(code, "not_found", "the map holds six vertices");
        assert!(
            message.contains("vertex 99 does not exist (map has 6 vertices)"),
            "the message counts the vertices: {message}"
        );
        refuse(
            &client,
            "move_vertex",
            json!({"index": 0_i32, "x": 40000_i32}),
        )
        .await;
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn remove_vertex_drops_a_point_nothing_references() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(&client, "add_vertex", json!({"x": 32_i32, "y": 32_i32})).await;
        let result = call(&client, "remove_vertex", json!({"index": 6_i32})).await;
        assert_eq!(
            result["removed"]["vertices"],
            json!([6_i32]),
            "the loose point went"
        );
        assert_eq!(
            result["removed"]["linedefs"],
            json!([]),
            "no line depended on it"
        );
        assert_eq!(result["map"], json!("MAP01"), "the fragment names the map");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "nothing referenced the point, so the node builder sees no change"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            fixture_counts(),
            "the map is back to the fixture's counts"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec![
                "add vertex at 32, 32".to_owned(),
                "remove 1 vertices".to_owned()
            ],
            "add and remove are two separate steps"
        );
    }

    #[tokio::test]
    async fn remove_vertex_refuses_a_used_point_without_cascade() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(&client, "remove_vertex", json!({"index": 4_i32})).await;
        assert_eq!(code, "conflict", "three lines end at vertex 4");
        assert!(
            message.contains("vertex 4 is used by linedef(s) 1, 2, 4")
                && message.contains("cascade: true"),
            "the message lists the lines and the fix: {message}"
        );
        let (list_code, _) = call_err(
            &client,
            "remove_vertex",
            json!({"index": [4_i32], "cascade": false}),
        )
        .await;
        assert_eq!(list_code, "conflict", "a list is refused the same way");
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn remove_vertex_without_cascade_reports_a_missing_index() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(&client, "remove_vertex", json!({"index": 40_i32})).await;
        assert_eq!(code, "not_found", "the map holds six vertices");
        assert!(
            message.contains("vertex 40 does not exist (map has 6 vertices)"),
            "the message counts the vertices: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn a_cascaded_vertex_removal_takes_the_lines_that_used_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "remove_vertex",
            json!({"index": 4_i32, "cascade": true}),
        )
        .await;
        assert_eq!(result["removed"]["vertices"], json!([4_i32]), "the seed");
        assert_eq!(
            result["removed"]["linedefs"],
            json!([1_i32, 2_i32, 4_i32]),
            "the three lines that ended here"
        );
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([1_i32, 2_i32, 3_i32, 5_i32]),
            "the sides only those lines carried"
        );
        assert_eq!(result["removed"]["sectors"], json!([]), "no sector went");
        assert_eq!(result["removed"]["things"], json!([]), "and no thing");
        assert_eq!(
            result["removed"]["one_sided_linedefs"],
            json!([]),
            "every survivor kept its sides"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 2_i32, "vertices": 5_i32, "linedefs": 4_i32,
                "sidedefs": 4_i32, "sectors": 2_i32,
            }),
            "the cascade left no orphans"
        );
        let rows = working_vertices(&client, "MAP01").await;
        let corners = rows
            .iter()
            .map(|row| (row["x"].as_i64(), row["y"].as_i64()))
            .collect::<Vec<_>>();
        assert_eq!(
            corners,
            vec![
                (Some(0_i64), Some(0_i64)),
                (Some(256_i64), Some(0_i64)),
                (Some(512_i64), Some(0_i64)),
                (Some(512_i64), Some(256_i64)),
                (Some(0_i64), Some(256_i64)),
            ],
            "the removed corner is gone and the rest kept their points"
        );
    }

    #[tokio::test]
    async fn turning_side_pruning_off_leaves_the_orphaned_sides_behind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "remove_vertex",
            json!({"index": 4_i32, "cascade": true, "prune_sidedefs": false}),
        )
        .await;
        assert_eq!(
            result["removed"]["linedefs"],
            json!([1_i32, 2_i32, 4_i32]),
            "the linedef rule still ran"
        );
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([]),
            "the sides were left alone"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 2_i32, "vertices": 5_i32, "linedefs": 4_i32,
                "sidedefs": 8_i32, "sectors": 2_i32,
            }),
            "four of them reference no line any more"
        );
    }

    /// Adds, moves and removes one vertex, three separate undo steps.
    async fn add_move_remove_trio(client: &Client) {
        call(client, "add_vertex", json!({"x": 128_i32, "y": 128_i32})).await;
        call(
            client,
            "move_vertex",
            json!({"index": 6_i32, "dx": -128_i32}),
        )
        .await;
        call(client, "remove_vertex", json!({"index": 6_i32})).await;
    }

    #[tokio::test]
    async fn vertex_edits_undo_and_redo_as_one_step_each() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        add_move_remove_trio(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            fixture_counts(),
            "the trio cancelled out"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["remove 1 vertices"]),
            "the removal rolls back alone"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await["vertices"],
            json!(7_i32),
            "the point is back"
        );
        let moved_back = call(&client, "undo", json!({})).await;
        assert_eq!(
            moved_back["undone"],
            json!(["move 1 vertices"]),
            "the second step back is the move"
        );
        let added_back = call(&client, "undo", json!({})).await;
        assert_eq!(
            added_back["undone"],
            json!(["add vertex at 128, 128"]),
            "and the last one is the add"
        );
        let (code, _) = call_err(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(code, "not_found", "the copied block rolled back too");
        let redone = call(&client, "redo", json!({"steps": 3_i32})).await;
        assert_eq!(
            redone["redone"],
            json!([
                "add vertex at 128, 128",
                "move 1 vertices",
                "remove 1 vertices",
            ]),
            "all three replay in order"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            fixture_counts(),
            "and the map lands where the trio left it"
        );
    }

    #[tokio::test]
    async fn vertex_edits_leave_the_layer_map_untouched() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "move_vertex",
            json!({"index": [0_i32, 5_i32], "dy": 16_i32}),
        )
        .await;
        call(&client, "add_vertex", json!({"x": 64_i32, "y": 64_i32})).await;
        call(
            &client,
            "remove_vertex",
            json!({"index": 3_i32, "cascade": true}),
        )
        .await;
        assert_eq!(
            counts(&client, "MAP01", "layer:0").await,
            fixture_counts(),
            "the IWAD's MAP01 is exactly what it was"
        );
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            counts(&client, "MAP01", "working").await,
            "the merged view shows the edited copy"
        );
    }
}
