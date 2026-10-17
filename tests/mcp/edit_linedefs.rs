//! The `edit_linedefs` tool group as seen by a real MCP client:
//! `add_linedef` through existing indices and through created points and
//! sides, the `already_exists` duplicate and the degenerate, form and
//! reference refusals, the family-dependent flag defaults and the
//! Hexen-family tag rule, `edit_linedef` flag replacement and adjustment,
//! the flip verified through `assemble_map`, the `back: null` closing, the
//! in-place `front_side` texture-offset edit and the nothing-to-change
//! refusal, and `remove_linedef` with and without its pruning.

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

    /// The working archive's linedef rows for `map`, in order, through
    /// `assemble_map`.
    async fn working_linedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": "working", "include": ["linedefs"]}),
        )
        .await;
        report["linedefs"]
            .as_array()
            .expect("linedefs is an array")
            .clone()
    }

    /// The working `linedefs` row at `index` for `map`.
    async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
        working_linedefs(client, map)
            .await
            .into_iter()
            .nth(index)
            .expect("the linedef row exists")
    }

    /// The merged view's linedef rows for `map`, in order (the layers are
    /// what an unedited session shows; failed edits must leave it intact).
    async fn merged_linedefs(client: &Client, map: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "include": ["linedefs"]}),
        )
        .await;
        report["linedefs"]
            .as_array()
            .expect("linedefs is an array")
            .clone()
    }

    /// The merged `linedefs` row at `index` for `map`.
    async fn merged_linedef(client: &Client, map: &str, index: usize) -> Value {
        merged_linedefs(client, map)
            .await
            .into_iter()
            .nth(index)
            .expect("the linedef row exists")
    }

    /// The vertex count of `map` in `scope` (`"working"` or `"merged"`).
    async fn vertex_count(client: &Client, map: &str, scope: &str) -> usize {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": scope, "include": ["vertices"]}),
        )
        .await;
        report["vertices"]
            .as_array()
            .expect("vertices is an array")
            .len()
    }

    /// Requires `tool` to reject `args` with `invalid_argument`.
    async fn refuse(client: &Client, tool: &str, args: Value) {
        let note = args.to_string();
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "invalid_argument", "refusal code for {tool} {note}");
    }

    /// Requires `tool` to reject `args` with `not_found`.
    async fn miss(client: &Client, tool: &str, args: Value) {
        let note = args.to_string();
        let (code, _) = call_err(client, tool, args).await;
        assert_eq!(code, "not_found", "not_found code for {tool} {note}");
    }

    #[tokio::test]
    async fn add_linedef_with_existing_vertices_and_sidedef_appends_one_line() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_linedef",
            json!({"v1": 2_i32, "v2": 4_i32, "front": 6_i32}),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["index"], json!(7_i32), "appended after the seven");
        assert_eq!(result["created_vertices"], json!([]), "nothing created");
        assert_eq!(result["created_sidedefs"], json!([]), "no side either");
        assert_eq!(
            result["linedef"],
            json!({
                "v1": 2_i32, "v2": 4_i32, "flags": 1_i32, "flag_names": ["blocking"],
                "special": 0_i32, "tag": 0_i32, "args": [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                "front": 6_i32, "back": null,
            }),
            "the one-sided default blocks"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "a new linedef is geometry"
        );
        let row = working_linedef(&client, "MAP01", 7).await;
        assert_eq!(row["v1"]["index"], json!(2_i32), "read back over 2 to 4");
        assert_eq!(row["v1"]["x"], json!(512_i32), "the endpoint coordinates");
        assert_eq!(row["v2"]["index"], json!(4_i32));
        assert_eq!(row["front"]["sidedef"], json!(6_i32), "sidedef 6 fronts");
        assert_eq!(row["back"], Value::Null, "one-sided");
    }

    #[tokio::test]
    async fn add_linedef_from_points_creates_vertices_and_a_front_side() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_linedef",
            json!({"map": "MAP01", "from": {"x": 0_i32, "y": 128_i32},
                   "to": {"x": 256_i32, "y": 128_i32},
                   "front_side": {"sector": 1_i32, "middle": "wall1", "x_offset": 8_i32},
                   "back": 3_i32, "flags": ["twosided", "mapped"], "special": 10_i32,
                   "tag": 1_i32}),
        )
        .await;
        assert_eq!(result["index"], json!(7_i32), "the eighth line");
        assert_eq!(
            result["created_vertices"],
            json!([6_i32, 7_i32]),
            "both points are new"
        );
        assert_eq!(
            result["created_sidedefs"],
            json!([8_i32]),
            "the front side was created"
        );
        assert_eq!(
            result["linedef"]["flags"],
            json!(260_i32),
            "twosided|mapped"
        );
        assert_eq!(
            result["linedef"]["flag_names"],
            json!(["twosided", "mapped"]),
            "named back for the family"
        );
        assert_eq!(
            result["linedef"]["back"],
            json!(3_i32),
            "the referenced back"
        );
        assert_eq!(result["linedef"]["special"], json!(10_i32), "the trigger");
        assert_eq!(
            result["linedef"]["tag"],
            json!(1_i32),
            "sector 1 is tagged 1"
        );
        let row = working_linedef(&client, "MAP01", 7).await;
        assert_eq!(row["v1"]["x"], json!(0_i32), "the new start point");
        assert_eq!(row["v1"]["y"], json!(128_i32));
        assert_eq!(row["v2"]["x"], json!(256_i32), "the new end point");
        assert_eq!(
            row["front"]["sidedef"],
            json!(8_i32),
            "the created side fronts"
        );
        assert_eq!(row["front"]["sector"], json!(1_i32), "over sector 1");
        assert_eq!(row["front"]["middle"], json!("WALL1"), "uppercased");
        assert_eq!(row["front"]["x_offset"], json!(8_i32), "and shifted");
    }

    #[tokio::test]
    async fn add_linedef_from_points_reuses_the_vertex_an_earlier_add_created() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "add_linedef",
            json!({"map": "MAP01", "from": {"x": 0_i32, "y": 128_i32},
                   "to": {"x": 256_i32, "y": 128_i32},
                   "front_side": {"sector": 1_i32}}),
        )
        .await;
        let reuse = call(
            &client,
            "add_linedef",
            json!({"from": {"x": 0_i32, "y": 128_i32},
                   "to": {"x": 512_i32, "y": 128_i32}, "front": 8_i32}),
        )
        .await;
        assert_eq!(
            reuse["created_vertices"],
            json!([8_i32]),
            "the first point came back reused"
        );
        assert_eq!(reuse["linedef"]["v1"], json!(6_i32), "the existing vertex");
        assert_eq!(reuse["index"], json!(8_i32), "the ninth line");
        assert_eq!(
            reuse["linedef"]["flag_names"],
            json!(["blocking"]),
            "one-sided again"
        );
    }

    #[tokio::test]
    async fn add_linedef_refuses_the_duplicate_by_either_direction() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "add_linedef",
            json!({"v1": 5_i32, "v2": 0_i32, "front": 0_i32}),
        )
        .await;
        assert_eq!(code, "already_exists", "linedef 0 runs 0 to 5");
        assert!(
            message.contains("linedef 0") && message.contains("edit_linedef"),
            "the message names the line and the fix: {message}"
        );
        let rows = merged_linedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "the refusal added nothing");
    }

    #[tokio::test]
    async fn add_linedef_refuses_degenerate_endpoints_and_missing_fronts() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "add_linedef",
            json!({"v1": 4_i32, "v2": 4_i32, "front": 0_i32}),
        )
        .await;
        refuse(
            &client,
            "add_linedef",
            json!({"from": {"x": 256_i32, "y": 256_i32},
                   "to": {"x": 256_i32, "y": 256_i32}, "front": 0_i32}),
        )
        .await;
        refuse(&client, "add_linedef", json!({"v1": 0_i32, "v2": 2_i32})).await;
        refuse(&client, "add_linedef", json!({"front": 0_i32})).await;
        refuse(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "from": {"x": 1_i32, "y": 1_i32},
                   "to": {"x": 2_i32, "y": 2_i32}, "front": 0_i32}),
        )
        .await;
        refuse(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front": 0_i32,
                   "front_side": {"sector": 0_i32}}),
        )
        .await;
        refuse(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "back": 1_i32, "back_side": {"sector": 0_i32}}),
        )
        .await;
        let rows = merged_linedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "none of the refusals appended");
    }

    #[tokio::test]
    async fn add_linedef_reports_missing_references_as_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        miss(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 99_i32, "front": 0_i32}),
        )
        .await;
        miss(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front": 99_i32}),
        )
        .await;
        miss(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front_side": {"sector": 9_i32}}),
        )
        .await;
        miss(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front": 0_i32, "back": 99_i32}),
        )
        .await;
        miss(
            &client,
            "add_linedef",
            json!({"map": "MAP09", "v1": 0_i32, "v2": 2_i32, "front": 0_i32}),
        )
        .await;
    }

    #[tokio::test]
    async fn a_back_side_makes_the_default_line_two_sided() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front": 0_i32, "back_side": {"sector": 1_i32}}),
        )
        .await;
        assert_eq!(
            result["linedef"]["flags"],
            json!(4_i32),
            "the two-sided default"
        );
        assert_eq!(
            result["linedef"]["flag_names"],
            json!(["twosided"]),
            "and it names"
        );
        assert_eq!(result["linedef"]["back"], json!(8_i32), "the created back");
        assert_eq!(
            result["created_sidedefs"],
            json!([8_i32]),
            "reported as created"
        );
    }

    #[tokio::test]
    async fn hexen_family_maps_take_args_and_refuse_a_tag() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "add_linedef",
            json!({"map": "MAP03", "v1": 0_i32, "v2": 2_i32, "front": 0_i32, "tag": 5_i32}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "the zdoom namespace is Hexen");
        assert!(
            message.contains("have no tag"),
            "the message names the field: {message}"
        );
        let arged = call(
            &client,
            "add_linedef",
            json!({"map": "MAP03", "v1": 0_i32, "v2": 2_i32, "front": 0_i32,
                   "special": 80_i32, "args": [7_i32, 0_i32, 0_i32, 0_i32, 0_i32]}),
        )
        .await;
        assert_eq!(
            arged["linedef"]["args"],
            json!([7_i32, 0_i32, 0_i32, 0_i32, 0_i32]),
            "args land"
        );
        assert_eq!(
            arged["linedef"]["flag_names"],
            json!(["blocking", "playercross"]),
            "the Hexen table names the zero activation field"
        );
        refuse(
            &client,
            "add_linedef",
            json!({"map": "MAP04", "v1": 0_i32, "v2": 2_i32, "front": 0_i32,
                   "args": [300_i32, 0_i32, 0_i32, 0_i32, 0_i32]}),
        )
        .await;
        refuse(
            &client,
            "edit_linedef",
            json!({"map": "MAP04", "index": 2_i32, "tag": 5_i32}),
        )
        .await;
    }

    #[tokio::test]
    async fn binary_maps_cap_points_and_offsets_on_the_way_in() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "add_linedef",
            json!({"from": {"x": 40_000_i32, "y": 0_i32}, "to": {"x": 0_i32, "y": 0_i32},
                   "front": 0_i32}),
        )
        .await;
        refuse(
            &client,
            "add_linedef",
            json!({"v1": 0_i32, "v2": 2_i32, "front_side": {"sector": 0_i32, "x_offset": 40_000_i32}}),
        )
        .await;
        let rows = merged_linedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "the range refusals appended nothing");
        assert_eq!(
            vertex_count(&client, "MAP01", "merged").await,
            6,
            "and no vertex came in"
        );
    }

    #[tokio::test]
    async fn edit_linedef_replaces_and_adjusts_the_flag_bits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let replaced = call(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "flags": ["blocking", "mapped"]}),
        )
        .await;
        assert_eq!(replaced["linedefs"][0]["flags"], json!(257_i32), "1 | 256");
        assert_eq!(
            replaced["linedefs"][0]["flag_names"],
            json!(["blocking", "mapped"]),
            "named back"
        );
        assert_eq!(
            replaced["nodes_invalidated"],
            json!(false),
            "flags are a property edit"
        );
        let set = call(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "set_flags": ["secret"]}),
        )
        .await;
        assert_eq!(set["linedefs"][0]["flags"], json!(289_i32), "257 | 32");
        let cleared = call(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "clear_flags": ["mapped", "secret"]}),
        )
        .await;
        assert_eq!(
            cleared["linedefs"][0]["flags"],
            json!(1_i32),
            "back to blocking"
        );
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "flags": 1_i32, "set_flags": ["secret"]}),
        )
        .await;
        let stored = working_linedef(&client, "MAP01", 0).await;
        assert_eq!(stored["flag_names"], json!(["blocking"]), "read back");
    }

    #[tokio::test]
    async fn edit_linedef_sets_special_and_tag_on_lists() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_linedef",
            json!({"index": [4_i32, 5_i32, 6_i32], "special": 12_i32, "tag": 1_i32}),
        )
        .await;
        let rows = result["linedefs"].as_array().expect("a linedefs array");
        assert_eq!(rows.len(), 3, "three rows reported");
        assert_eq!(rows[0]["index"], json!(4_i32), "ascending");
        assert_eq!(rows[2]["index"], json!(6_i32));
        assert_eq!(rows[0]["special"], json!(12_i32), "the special landed");
        assert_eq!(rows[2]["tag"], json!(1_i32), "on every selected line");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "specials and tags are not geometry"
        );
        let stored = working_linedef(&client, "MAP01", 6).await;
        assert_eq!(stored["special"], json!(12_i32), "written and read back");
        assert_eq!(stored["front"]["sidedef"], json!(7_i32), "the sides stand");
    }

    #[tokio::test]
    async fn edit_linedef_refuses_empty_conflicting_and_null_requests() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(&client, "edit_linedef", json!({"index": 0_i32})).await;
        refuse(&client, "edit_linedef", json!({"index": []})).await;
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "front": null}),
        )
        .await;
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "back": null, "back_side": {"middle": "X"}}),
        )
        .await;
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "front_side": {}}),
        )
        .await;
        miss(
            &client,
            "edit_linedef",
            json!({"index": 99_i32, "special": 1_i32}),
        )
        .await;
        miss(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "v2": 99_i32}),
        )
        .await;
        miss(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "back": 99_i32}),
        )
        .await;
        let (zero_code, zero_message) = call_err(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "v2": 0_i32}),
        )
        .await;
        assert_eq!(zero_code, "invalid_argument", "zero-length refusal");
        assert!(
            zero_message.contains("zero-length"),
            "the message names the rule: {zero_message}"
        );
        let stored = merged_linedef(&client, "MAP01", 0).await;
        assert_eq!(stored["special"], json!(0_i32), "nothing changed");
        assert_eq!(stored["v2"]["index"], json!(5_i32), "the endpoint stands");
    }

    #[tokio::test]
    async fn edit_linedef_flip_keeps_the_front_on_the_right() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_linedef",
            json!({"index": 2_i32, "flip": true}),
        )
        .await;
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "a flipped line is geometry"
        );
        assert_eq!(
            result["linedefs"][0]["v1"],
            json!(1_i32),
            "the ends swapped"
        );
        assert_eq!(result["linedefs"][0]["v2"], json!(4_i32));
        assert_eq!(
            result["linedefs"][0]["front"],
            json!(3_i32),
            "the old back fronts now"
        );
        assert_eq!(result["linedefs"][0]["back"], json!(2_i32));
        let row = working_linedef(&client, "MAP01", 2).await;
        assert_eq!(row["v1"]["x"], json!(256_i32), "v1 at the old end");
        assert_eq!(row["v1"]["y"], json!(0_i32), "the hall floor corner");
        assert_eq!(row["v2"]["y"], json!(256_i32), "v2 at the old start");
        assert_eq!(
            row["front"]["sector"],
            json!(1_i32),
            "the raised room's side stayed the physical left, now the right"
        );
        assert_eq!(row["back"]["sector"], json!(0_i32), "the hall side trails");
        assert_eq!(row["flag_names"], json!(["twosided"]), "flags untouched");
    }

    #[tokio::test]
    async fn edit_linedef_back_null_closes_the_line_one_sided() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_linedef",
            json!({"index": 2_i32, "back": null}),
        )
        .await;
        assert_eq!(
            result["linedefs"][0]["back"],
            Value::Null,
            "the reference went"
        );
        assert_eq!(result["linedefs"][0]["flags"], json!(1_i32), "blocking on");
        assert_eq!(
            result["linedefs"][0]["flag_names"],
            json!(["blocking"]),
            "twosided cleared"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "losing a side is geometry"
        );
        let row = working_linedef(&client, "MAP01", 2).await;
        assert_eq!(row["back"], Value::Null, "and the map agrees");
        assert_eq!(row["front"]["sidedef"], json!(2_i32), "the front stands");
    }

    #[tokio::test]
    async fn edit_linedef_front_side_edits_the_referenced_sidedef_offsets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_linedef",
            json!({"index": 2_i32, "front_side": {"x_offset": 16_i32, "upper": "brick"}}),
        )
        .await;
        assert_eq!(
            result["linedefs"][0]["front"],
            json!(2_i32),
            "sidedef 2 fronts"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "textures and offsets are not geometry"
        );
        let row = working_linedef(&client, "MAP01", 2).await;
        assert_eq!(row["front"]["x_offset"], json!(16_i32), "the shift landed");
        assert_eq!(
            row["front"]["upper"],
            json!("BRICK"),
            "the texture uppercased"
        );
        assert_eq!(
            row["front"]["lower"],
            json!("STEP1"),
            "unasked fields stand"
        );
        assert_eq!(
            row["front"]["middle"],
            json!("-"),
            "as the fixture had them"
        );
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 0_i32, "back_side": {"middle": "X"}}),
        )
        .await;
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 2_i32, "front_side": {"x_offset": 40_000_i32}}),
        )
        .await;
    }

    #[tokio::test]
    async fn edit_linedef_moves_references_and_rejects_a_collapsed_line() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "edit_linedef",
            json!({"index": 3_i32, "v1": 2_i32, "front": 4_i32}),
        )
        .await;
        assert_eq!(result["linedefs"][0]["v1"], json!(2_i32), "the start moved");
        assert_eq!(
            result["linedefs"][0]["front"],
            json!(4_i32),
            "the side moved"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "moving an endpoint is geometry"
        );
        refuse(
            &client,
            "edit_linedef",
            json!({"index": 3_i32, "v2": 2_i32}),
        )
        .await;
        let stored = working_linedef(&client, "MAP01", 3).await;
        assert_eq!(
            stored["v1"]["x"],
            json!(512_i32),
            "the line now starts at 512,0"
        );
        assert_eq!(
            stored["front"]["sidedef"],
            json!(4_i32),
            "and the new front read back"
        );
    }

    #[tokio::test]
    async fn remove_linedef_prunes_orphaned_sides_and_vertices() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "remove_linedef", json!({"index": [5_i32, 6_i32]})).await;
        assert_eq!(
            result["removed"]["linedefs"],
            json!([5_i32, 6_i32]),
            "the selected lines"
        );
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([6_i32, 7_i32]),
            "their sides went with them"
        );
        assert_eq!(
            result["removed"]["vertices"],
            json!([2_i32]),
            "and their shared unreferenced point"
        );
        assert_eq!(result["removed"]["sectors"], json!([]), "sectors never go");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "removing lines is geometry"
        );
        let rows = working_linedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 5, "five lines survive");
        assert_eq!(
            vertex_count(&client, "MAP01", "working").await,
            5,
            "the vertex count dropped"
        );
    }

    #[tokio::test]
    async fn remove_linedef_can_leave_the_orphans_behind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "remove_linedef",
            json!({"index": [5_i32, 6_i32], "prune_sidedefs": false, "prune_vertices": false}),
        )
        .await;
        assert_eq!(
            result["removed"]["linedefs"],
            json!([5_i32, 6_i32]),
            "the selected lines"
        );
        assert_eq!(result["removed"]["sidedefs"], json!([]), "the sides stay");
        assert_eq!(result["removed"]["vertices"], json!([]), "and the point");
        assert_eq!(
            vertex_count(&client, "MAP01", "working").await,
            6,
            "nothing was pruned"
        );
        let report = call(
            &client,
            "assemble_map",
            json!({"map": "MAP01", "scope": "working", "include": ["sidedefs"]}),
        )
        .await;
        assert_eq!(
            report["sidedefs"]
                .as_array()
                .expect("sidedefs is an array")
                .len(),
            8,
            "all eight sides are still in the block"
        );
    }

    #[tokio::test]
    async fn remove_linedef_reports_missing_indices_and_empty_lists() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let (code, message) = call_err(&client, "remove_linedef", json!({"index": 40_i32})).await;
        assert_eq!(code, "not_found", "past the end of the block");
        assert!(
            message.contains("linedef 40"),
            "the message names it: {message}"
        );
        refuse(&client, "remove_linedef", json!({"index": []})).await;
        let rows = merged_linedefs(&client, "MAP01").await;
        assert_eq!(rows.len(), 7, "nothing removed");
    }

    #[tokio::test]
    async fn an_add_linedef_call_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        call(
            &client,
            "add_linedef",
            json!({"from": {"x": 0_i32, "y": 128_i32}, "to": {"x": 256_i32, "y": 128_i32},
                   "front_side": {"sector": 0_i32}}),
        )
        .await;
        let grown = working_linedefs(&client, "MAP01").await;
        assert_eq!(grown.len(), 8, "the line came in");
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["add linedef from 0, 128 to 256, 128"]),
            "one labelled step"
        );
        let back = call(&client, "assemble_map", json!({"map": "MAP01"})).await;
        assert_eq!(
            back["linedefs"]
                .as_array()
                .expect("linedefs is an array")
                .len(),
            7,
            "the line, its vertices and its side rolled back"
        );
        assert_eq!(
            back["vertices"]
                .as_array()
                .expect("vertices is an array")
                .len(),
            6,
            "the created vertices went"
        );
    }
}
