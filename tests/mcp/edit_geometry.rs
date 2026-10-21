//! The `edit_geometry` tool group as seen by a real MCP client:
//! `delete_geometry` with its always-on cascade verified against the
//! two-room fixture (dry run and real run report the same removal, the dry
//! run edits nothing), a mixed thing-and-vertex selection, the
//! `invalid_argument` of an empty selection, the `not_found` of an index
//! past a block, the pruning switches leaving orphans behind and the undo
//! of the whole call. `transform_selection` with its sector-expansion
//! translate (room and imp move, the hall and its player do not), the
//! whole-map half turn that undoes itself, the `invalid_argument` of an
//! out-of-range scale on a binary map (where `UDMF` accepts the same
//! move), the zero-length `conflict`, the reported-but-unmerged
//! coincidences and the node-preserving things-only move.

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

    /// The `removed` block of deleting the fixture's sector 1 with the
    /// default pruning: the whole back room, the shared line closed.
    fn sector_one_cascade() -> Value {
        json!({
            "vertices": [2_i32, 3_i32],
            "linedefs": [4_i32, 5_i32, 6_i32],
            "sidedefs": [3_i32, 5_i32, 6_i32, 7_i32],
            "sectors": [1_i32],
            "things": [],
            "flipped_linedefs": [],
            "one_sided_linedefs": [2_i32],
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
    /// both the two-room map) over an empty working archive.
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

    /// Requires a call to have left the working archive untouched: no map
    /// block copied in and no undo entry.
    async fn assert_untouched(client: &Client) {
        let (code, _, _) = call_err(
            client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(code, "not_found", "the working archive holds no map");
        assert!(undo_labels(client).await.is_empty(), "no undo entry");
    }

    /// Requires `delete_geometry` to reject `args` with `expected` and
    /// return the message.
    async fn refuse(client: &Client, expected: &str, args: Value) -> String {
        let note = args.to_string();
        let (code, message, _) = call_err(client, "delete_geometry", args).await;
        assert_eq!(code, expected, "refusal code for {note}");
        message
    }

    #[tokio::test]
    async fn a_dry_run_reports_the_cascade_and_edits_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "delete_geometry",
            json!({"sectors": [1_i32], "dry_run": true}),
        )
        .await;
        assert_eq!(result["removed"], sector_one_cascade(), "the report");
        assert_eq!(result["dry_run"], json!(true), "the call reports only");
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "a dry run invalidates nothing"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(false),
            "and copies nothing in"
        );
        assert_untouched(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the merged view stands"
        );
    }

    #[tokio::test]
    async fn a_real_run_applies_the_reported_cascade_and_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "delete_geometry", json!({"sectors": [1_i32]})).await;
        assert_eq!(
            result["removed"],
            sector_one_cascade(),
            "the same report the dry run gave"
        );
        assert_eq!(result["dry_run"], json!(false), "a real run says so");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "deleting a room is geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 2_i32, "vertices": 4_i32, "linedefs": 4_i32,
                "sidedefs": 4_i32, "sectors": 1_i32,
            }),
            "the back room is gone and the cascade left no orphans"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["delete 1 map elements".to_owned()],
            "one undo entry for the whole call"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["delete 1 map elements"]),
            "the label comes back"
        );
        let (code, _, _) = call_err(
            &client,
            "get_map_summary",
            json!({"map": "MAP01", "scope": "working"}),
        )
        .await;
        assert_eq!(code, "not_found", "the copied block rolled back too");
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the map is the fixture again"
        );
    }

    #[tokio::test]
    async fn a_mixed_selection_of_a_thing_and_a_vertex_goes_as_one_edit() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "delete_geometry",
            json!({"things": [1_i32], "vertices": [5_i32]}),
        )
        .await;
        assert_eq!(result["removed"]["things"], json!([1_i32]), "the imp");
        assert_eq!(result["removed"]["vertices"], json!([5_i32]), "the corner");
        assert_eq!(
            result["removed"]["linedefs"],
            json!([0_i32, 1_i32]),
            "the two lines that ended at it"
        );
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([0_i32, 1_i32]),
            "the sides only those lines carried"
        );
        assert_eq!(result["removed"]["sectors"], json!([]), "no sector went");
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 1_i32, "vertices": 5_i32, "linedefs": 5_i32,
                "sidedefs": 6_i32, "sectors": 2_i32,
            }),
            "thing and geometry dropped together"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["delete 2 map elements".to_owned()],
            "two seeded indices, one step"
        );
    }

    #[tokio::test]
    async fn deleting_things_alone_keeps_the_node_lumps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(&client, "delete_geometry", json!({"things": [0_i32]})).await;
        assert_eq!(
            result["removed"]["things"],
            json!([0_i32]),
            "the start went"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "things are not buildable geometry"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await["things"],
            json!(1_i32),
            "the imp survives"
        );
    }

    #[tokio::test]
    async fn an_empty_selection_is_invalid_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let message = refuse(&client, "invalid_argument", json!({})).await;
        assert!(
            message.contains("vertices, linedefs, sidedefs, sectors and things"),
            "the message names the five lists: {message}"
        );
        refuse(
            &client,
            "invalid_argument",
            json!({"vertices": [], "sectors": []}),
        )
        .await;
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn an_index_the_map_does_not_hold_is_not_found_per_kind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let vertex = refuse(&client, "not_found", json!({"vertices": [99_i32]})).await;
        assert!(
            vertex.contains(
                "vertex 99 does not exist (the map has 6 vertices, valid indices are 0..=5)"
            ),
            "the message names the kind and counts the vertices: {vertex}"
        );
        let linedef = refuse(&client, "not_found", json!({"linedefs": [7_i32]})).await;
        assert!(
            linedef.contains(
                "linedef 7 does not exist (the map has 7 linedefs, valid indices are 0..=6)"
            ),
            "the message names the kind and counts the linedefs: {linedef}"
        );
        let sidedef = refuse(&client, "not_found", json!({"sidedefs": [8_i32]})).await;
        assert!(
            sidedef.contains(
                "sidedef 8 does not exist (the map has 8 sidedefs, valid indices are 0..=7)"
            ),
            "the message names the kind and counts the sidedefs: {sidedef}"
        );
        let sector = refuse(&client, "not_found", json!({"sectors": [2_i32]})).await;
        assert!(
            sector.contains(
                "sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)"
            ),
            "the message names the kind and counts the sectors: {sector}"
        );
        let thing = refuse(&client, "not_found", json!({"things": [2_i32]})).await;
        assert!(
            thing
                .contains("thing 2 does not exist (the map has 2 things, valid indices are 0..=1)"),
            "the message names the kind and counts the things: {thing}"
        );
        assert_untouched(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the map stands"
        );
    }

    #[tokio::test]
    async fn a_dry_run_refuses_a_missing_index_before_reporting() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let message = refuse(
            &client,
            "not_found",
            json!({"sectors": [5_i32], "dry_run": true}),
        )
        .await;
        assert!(
            message.contains(
                "sector 5 does not exist (the map has 2 sectors, valid indices are 0..=1)"
            ),
            "the message names kind and index: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn turning_the_prunings_off_keeps_the_orphans_behind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "delete_geometry",
            json!({
                "linedefs": [5_i32, 6_i32],
                "prune_sidedefs": false,
                "prune_vertices": false,
            }),
        )
        .await;
        assert_eq!(
            result["removed"]["linedefs"],
            json!([5_i32, 6_i32]),
            "exactly the seed"
        );
        assert_eq!(result["removed"]["sidedefs"], json!([]), "the sides stay");
        assert_eq!(result["removed"]["vertices"], json!([]), "the corner stays");
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 2_i32, "vertices": 6_i32, "linedefs": 5_i32,
                "sidedefs": 8_i32, "sectors": 2_i32,
            }),
            "two orphaned sides and an orphaned vertex survive"
        );
    }

    #[tokio::test]
    async fn prune_vertices_off_spares_the_cascaded_corners() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "delete_geometry",
            json!({"sectors": [1_i32], "prune_vertices": false}),
        )
        .await;
        assert_eq!(
            result["removed"]["sidedefs"],
            json!([3_i32, 5_i32, 6_i32, 7_i32]),
            "the sector rule is not a pruning"
        );
        assert_eq!(
            result["removed"]["linedefs"],
            json!([4_i32, 5_i32, 6_i32]),
            "sideless lines still go"
        );
        assert_eq!(
            result["removed"]["vertices"],
            json!([]),
            "the back-room corners are spared"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await,
            json!({
                "things": 2_i32, "vertices": 6_i32, "linedefs": 4_i32,
                "sidedefs": 4_i32, "sectors": 1_i32,
            }),
            "the two orphaned corners survive unreferenced"
        );
    }

    #[tokio::test]
    async fn unknown_maps_are_not_found_and_the_wrong_scope_stays_quiet() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        refuse(
            &client,
            "not_found",
            json!({"map": "MAP09", "sectors": [0_i32]}),
        )
        .await;
        assert_untouched(&client).await;
        call(
            &client,
            "delete_geometry",
            json!({"map": "MAP01", "sectors": [1_i32]}),
        )
        .await;
        let dry = call(
            &client,
            "delete_geometry",
            json!({"map": "MAP01", "things": [0_i32], "dry_run": true}),
        )
        .await;
        assert_eq!(
            dry["removed"],
            json!({
                "vertices": [], "linedefs": [], "sidedefs": [],
                "sectors": [], "things": [0_i32],
                "flipped_linedefs": [], "one_sided_linedefs": [],
            }),
            "the dry run reads the edited working block"
        );
        assert_eq!(
            counts(&client, "MAP01", "working").await["sectors"],
            json!(1_i32),
            "and did not touch it twice"
        );
    }

    #[tokio::test]
    async fn a_udmf_map_cascades_the_same_way() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "delete_geometry",
            json!({"map": "MAP03", "sectors": [1_i32], "dry_run": true}),
        )
        .await;
        assert_eq!(result["removed"], sector_one_cascade(), "same engine");
        assert_eq!(result["map"], json!("MAP03"), "the named map");
        let applied = call(
            &client,
            "delete_geometry",
            json!({"map": "MAP03", "sectors": [1_i32]}),
        )
        .await;
        assert_eq!(
            applied["nodes_invalidated"],
            json!(true),
            "the UDMF block's ZNODES go with the geometry"
        );
        assert_eq!(
            counts(&client, "MAP03", "working").await["sectors"],
            json!(1_i32),
            "the back room is gone"
        );
    }

    /// The `assemble_map` rows of one `kind` for `map` in `scope`.
    async fn rows(client: &Client, kind: &str, map: &str, scope: &str) -> Vec<Value> {
        let report = call(
            client,
            "assemble_map",
            json!({"map": map, "scope": scope, "include": [kind]}),
        )
        .await;
        report[kind]
            .as_array()
            .unwrap_or_else(|| panic!("{kind} is an array"))
            .clone()
    }

    /// The working `map`'s row of `kind` at `index`.
    async fn working_row(client: &Client, kind: &str, map: &str, index: usize) -> Value {
        rows(client, kind, map, "working")
            .await
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| panic!("the working {map} holds no {kind} {index}"))
    }

    /// Requires `transform_selection` to reject `args` with `expected` and
    /// return the message.
    async fn refuse_transform(client: &Client, expected: &str, args: Value) -> String {
        let note = args.to_string();
        let (code, message, _) = call_err(client, "transform_selection", args).await;
        assert_eq!(code, expected, "refusal code for {note}");
        message
    }

    #[tokio::test]
    async fn translating_a_sector_moves_the_room_and_its_imp_only() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "transform_selection",
            json!({
                "selection": {"sectors": [1_i32]},
                "translate": {"dx": 0_i32, "dy": 64_i32},
            }),
        )
        .await;
        assert_eq!(result["map"], json!("MAP01"), "the only map is default");
        assert_eq!(result["vertices_moved"], json!(4_i32), "four corners");
        assert_eq!(result["things_moved"], json!(1_i32), "the imp rode along");
        assert_eq!(
            result["vertices_selected"],
            json!([1_i32, 2_i32, 3_i32, 4_i32]),
            "the expanded boundary"
        );
        assert_eq!(result["things_selected"], json!([1_i32]), "the thing index");
        assert_eq!(
            result["origin"],
            json!({"x": 384_i64, "y": 128_i64}),
            "the selection's bounding-box centre"
        );
        assert_eq!(result["coincident_vertices"], json!([]), "nothing stacked");
        assert_eq!(
            result["nodes_invalidated"],
            json!(true),
            "moving vertices is geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        let corner = working_row(&client, "vertices", "MAP01", 1).await;
        assert_eq!(corner["x"], json!(256_i32), "the shared corner moved");
        assert_eq!(corner["y"], json!(64_i32), "up by 64");
        let top = working_row(&client, "vertices", "MAP01", 3).await;
        assert_eq!(top["y"], json!(320_i32), "the ceiling-side corner too");
        let hall = working_row(&client, "vertices", "MAP01", 0).await;
        assert_eq!(
            (hall["x"].clone(), hall["y"].clone()),
            (json!(0_i32), json!(0_i32)),
            "the hall corner stayed"
        );
        let imp = working_row(&client, "things", "MAP01", 1).await;
        assert_eq!(imp["y"], json!(192_i32), "the imp moved with its room");
        let player = working_row(&client, "things", "MAP01", 0).await;
        assert_eq!(player["x"], json!(64_i32), "the player stands in sector 0");
        assert_eq!(player["y"], json!(128_i32), "and did not move");
        assert_eq!(
            undo_labels(&client).await,
            vec!["transform 4 vertices and 1 thing".to_owned()],
            "one undo entry for the whole transform"
        );
    }

    #[tokio::test]
    async fn a_half_turn_about_a_point_and_back_restores_the_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let before_vertices = rows(&client, "vertices", "MAP01", "merged").await;
        let before_things = rows(&client, "things", "MAP01", "merged").await;
        let turn = json!({
            "selection": {"all": true},
            "rotate": {"degrees": 180_i64},
            "origin": {"x": 256_i32, "y": 128_i32},
        });
        let first = call(&client, "transform_selection", turn.clone()).await;
        assert_eq!(first["vertices_moved"], json!(6_i32), "every vertex");
        assert_eq!(first["things_moved"], json!(2_i32), "every thing");
        assert_eq!(
            first["origin"],
            json!({"x": 256_i64, "y": 128_i64}),
            "the requested pivot wins over the default"
        );
        let corner = working_row(&client, "vertices", "MAP01", 0).await;
        assert_eq!(
            (corner["x"].clone(), corner["y"].clone()),
            (json!(512_i32), json!(256_i32)),
            "the far corner took the hall corner's place"
        );
        let player = working_row(&client, "things", "MAP01", 0).await;
        assert_eq!(player["x"], json!(448_i32), "the player crossed over");
        assert_eq!(player["angle"], json!(180_i32), "and faces back");
        call(&client, "transform_selection", turn).await;
        assert_eq!(
            rows(&client, "vertices", "MAP01", "working").await,
            before_vertices,
            "two half turns are the identity on the vertices"
        );
        assert_eq!(
            rows(&client, "things", "MAP01", "working").await,
            before_things,
            "and on the things, facings included"
        );
    }

    #[tokio::test]
    async fn a_scale_past_the_i16_range_refuses_a_binary_map_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let message = refuse_transform(
            &client,
            "invalid_argument",
            json!({
                "selection": {"all": true},
                "scale": {"num": 200_i32, "den": 1_i32},
            }),
        )
        .await;
        assert!(
            message.contains("i16") && message.contains("convert_map_format"),
            "the error names the limit and the fix: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn the_same_scale_lands_fine_on_a_udmf_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let result = call(
            &client,
            "transform_selection",
            json!({
                "map": "MAP03",
                "selection": {"vertices": [0_i32, 2_i32]},
                "scale": {"num": 200_i32, "den": 1_i32},
            }),
        )
        .await;
        assert_eq!(result["vertices_moved"], json!(2_i32), "UDMF stores i32");
        let corner = working_row(&client, "vertices", "MAP03", 0).await;
        assert_eq!(corner["x"], json!(-50_944_i32), "far past i16, kept");
    }

    #[tokio::test]
    async fn a_scale_that_collapses_a_linedef_is_a_conflict() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let message = refuse_transform(
            &client,
            "conflict",
            json!({
                "selection": {"all": true},
                "scale": {"num": 1_i32, "den": 1000_i32},
            }),
        )
        .await;
        assert!(
            message.contains("linedef") && message.contains("zero-length"),
            "the conflict names the line: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn vertices_stacking_on_unmoved_ones_are_reported_not_merged() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "transform_selection",
            json!({
                "selection": {"vertices": [5_i32]},
                "translate": {"dx": 256_i32, "dy": -256_i32},
            }),
        )
        .await;
        assert_eq!(
            result["coincident_vertices"],
            json!([[5_i32, 1_i32]]),
            "the [moved, unmoved] pair"
        );
        assert_eq!(result["vertices_moved"], json!(1_i32), "one corner moved");
        assert_eq!(
            counts(&client, "MAP01", "working").await["vertices"],
            json!(6_i32),
            "nothing was merged away"
        );
        let landed = working_row(&client, "vertices", "MAP01", 5).await;
        assert_eq!(landed["x"], json!(256_i32), "on vertex 1's spot");
        assert_eq!(landed["y"], json!(0_i32), "at floor level");
    }

    #[tokio::test]
    async fn moving_only_things_keeps_the_node_lumps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let result = call(
            &client,
            "transform_selection",
            json!({
                "selection": {"things": [1_i32]},
                "translate": {"dx": 8_i32, "dy": 0_i32},
            }),
        )
        .await;
        assert_eq!(result["vertices_moved"], json!(0_i32), "no vertex moved");
        assert_eq!(result["things_moved"], json!(1_i32), "just the imp");
        assert_eq!(
            result["origin"],
            json!({"x": 384_i64, "y": 128_i64}),
            "the thing's own centre"
        );
        assert_eq!(
            result["nodes_invalidated"],
            json!(false),
            "things are not buildable geometry"
        );
        assert_eq!(
            result["copied_to_working"],
            json!(true),
            "the block copied in"
        );
        let imp = working_row(&client, "things", "MAP01", 1).await;
        assert_eq!(imp["x"], json!(392_i32), "the imp slid east");
        assert_eq!(
            undo_labels(&client).await,
            vec!["transform 0 vertices and 1 thing".to_owned()],
            "one step"
        );
    }

    #[tokio::test]
    async fn an_empty_selection_or_a_missing_operation_is_invalid() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let empty = refuse_transform(
            &client,
            "invalid_argument",
            json!({"selection": {}, "translate": {"dx": 1_i32, "dy": 0_i32}}),
        )
        .await;
        assert!(
            empty.contains("nothing selected"),
            "the message names the case: {empty}"
        );
        refuse_transform(
            &client,
            "invalid_argument",
            json!({
                "selection": {"vertices": [], "sectors": []},
                "translate": {"dx": 1_i32, "dy": 0_i32},
            }),
        )
        .await;
        let none = refuse_transform(
            &client,
            "invalid_argument",
            json!({"selection": {"all": true}}),
        )
        .await;
        assert!(
            none.contains("scale, rotate and translate"),
            "the message names the three: {none}"
        );
        let ambiguous = refuse_transform(
            &client,
            "invalid_argument",
            json!({
                "selection": {"all": true},
                "rotate": {"degrees": 90_i64, "millidegrees": 90000_i64},
            }),
        )
        .await;
        assert!(
            ambiguous.contains("exactly one"),
            "the message says how to fix it: {ambiguous}"
        );
        let negative = refuse_transform(
            &client,
            "invalid_argument",
            json!({
                "selection": {"all": true},
                "scale": {"num": -1_i32, "den": 2_i32},
            }),
        )
        .await;
        assert!(
            negative.contains("positive"),
            "the message names the rule: {negative}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn a_selection_index_past_a_block_is_not_found_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let linedef = refuse_transform(
            &client,
            "not_found",
            json!({
                "selection": {"linedefs": [9_i32]},
                "translate": {"dx": 1_i32, "dy": 0_i32},
            }),
        )
        .await;
        assert!(
            linedef.contains(
                "linedef 9 does not exist (the map has 7 linedefs, valid indices are 0..=6)"
            ),
            "the message names the kind and counts: {linedef}"
        );
        let sector = refuse_transform(
            &client,
            "not_found",
            json!({
                "selection": {"sectors": [2_i32]},
                "rotate": {"degrees": 90_i64},
            }),
        )
        .await;
        assert!(
            sector.contains(
                "sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)"
            ),
            "the sector kind too: {sector}"
        );
        assert_untouched(&client).await;
        assert_eq!(
            counts(&client, "MAP01", "merged").await,
            fixture_counts(),
            "the map stands"
        );
    }
}
