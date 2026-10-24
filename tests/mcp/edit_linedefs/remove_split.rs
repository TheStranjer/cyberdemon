//! `remove_linedef`, `split_linedef` and `join_linedefs` as an MCP client
//! calls them: removal pruning the orphaned sides and vertices and leaving them
//! behind on request, the missing indices and empty lists, one `add_linedef`
//! call undoing as one labelled step, `split_linedef` cutting linedef 0 of the
//! fixture map in two and `join_linedefs` welding the pair back whole with
//! counts through `get_map_summary`, and the off-line, endpoint, non-collinear,
//! third-vertex and property-conflict refusals surfacing as tool errors with
//! the right codes.

use serde_json::json;

use crate::edit_linedefs::{
    counts, doom_loaded, fixture_counts, merged_linedefs, miss, refuse, vertex_count,
    working_linedef, working_linedefs,
};
use crate::support::{call, call_err};

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
    let (code, message, _) = call_err(&client, "remove_linedef", json!({"index": 40_i32})).await;
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

#[tokio::test]
async fn split_linedef_cuts_linedef_zero_in_two() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let split = call(&client, "split_linedef", json!({"index": 0_i32})).await;
    assert_eq!(split["map"], json!("MAP01"), "the only map is default");
    assert_eq!(split["vertex"], json!(6_i32), "the midpoint vertex is new");
    assert_eq!(split["new_linedef"], json!(7_i32), "the half is appended");
    assert_eq!(
        split["new_sidedefs"],
        json!([8_i32]),
        "one copy: the line is one-sided"
    );
    assert_eq!(
        split["nodes_invalidated"],
        json!(true),
        "cutting is geometry"
    );
    assert_eq!(split["copied_to_working"], json!(true));
    assert_eq!(
        counts(&client, "MAP01", "working").await,
        json!({
            "things": 2_i32, "vertices": 7_i32, "linedefs": 8_i32,
            "sidedefs": 9_i32, "sectors": 2_i32,
        }),
        "the block gained one of each"
    );
    let half = working_linedef(&client, "MAP01", 7).await;
    assert_eq!(half["v1"]["y"], json!(128_i32), "it starts at the cut");
    assert_eq!(half["v2"]["y"], json!(256_i32), "and ends at the old end");
    assert_eq!(half["front"]["sidedef"], json!(8_i32), "on its own copy");
    assert_eq!(
        half["front"]["x_offset"],
        json!(128_i32),
        "shifted by the first piece"
    );
    let survivor = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(survivor["v2"]["index"], json!(6_i32), "it ends at the cut");
    assert_eq!(
        survivor["front"]["x_offset"],
        json!(0_i32),
        "keeping its side"
    );
}

#[tokio::test]
async fn join_linedefs_welds_a_split_pair_back_whole() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    call(&client, "split_linedef", json!({"index": 0_i32})).await;
    let join = call(
        &client,
        "join_linedefs",
        json!({"first": 0_i32, "second": 7_i32}),
    )
    .await;
    assert_eq!(
        join["linedef"],
        json!(0_i32),
        "the survivor keeps its index"
    );
    assert_eq!(join["removed"]["linedefs"], json!([7_i32]), "the half went");
    assert_eq!(join["removed"]["sidedefs"], json!([8_i32]), "with its copy");
    assert_eq!(
        join["removed"]["vertices"],
        json!([6_i32]),
        "and the absorbed cut vertex"
    );
    assert_eq!(
        join["nodes_invalidated"],
        json!(true),
        "joining is geometry too"
    );
    assert_eq!(
        counts(&client, "MAP01", "working").await,
        fixture_counts(),
        "the block is whole again"
    );
    let back = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(
        back["v2"]["index"],
        json!(5_i32),
        "the old endpoints are back"
    );
    assert_eq!(back["v2"]["y"], json!(256_i32));
    assert_eq!(
        back["front"]["sidedef"],
        json!(0_i32),
        "on the original side"
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(
        undo["undone"],
        json!(["join linedefs 0 and 7"]),
        "one labelled step"
    );
}

#[tokio::test]
async fn split_linedef_reports_bad_cuts_as_tool_errors() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    refuse(
        &client,
        "split_linedef",
        json!({"index": 0_i32, "at": {"x": 8_i32, "y": 128_i32}}),
    )
    .await;
    refuse(
        &client,
        "split_linedef",
        json!({"index": 0_i32, "at": {"x": 0_i32, "y": 256_i32}}),
    )
    .await;
    refuse(
        &client,
        "split_linedef",
        json!({"index": 0_i32, "at": {"x": 128_i32, "y": 300_i32}}),
    )
    .await;
    miss(&client, "split_linedef", json!({"index": 99_i32})).await;
    let rows = merged_linedefs(&client, "MAP01").await;
    assert_eq!(rows.len(), 7, "the refusals changed nothing");
}

#[tokio::test]
async fn join_linedefs_refuses_incompatible_pairs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "join_linedefs",
        json!({"first": 3_i32, "second": 0_i32}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "a corner is not a run");
    assert!(
        message.contains("collinear"),
        "the message names the rule: {message}"
    );
    let (sector_code, sector_message, _) = call_err(
        &client,
        "join_linedefs",
        json!({"first": 6_i32, "second": 3_i32}),
    )
    .await;
    assert_eq!(sector_code, "invalid_argument", "front sectors disagree");
    assert!(
        sector_message.contains("front sides"),
        "the message names the side: {sector_message}"
    );
    miss(
        &client,
        "join_linedefs",
        json!({"first": 0_i32, "second": 40_i32}),
    )
    .await;
    call(&client, "split_linedef", json!({"index": 0_i32})).await;
    call(
        &client,
        "add_linedef",
        json!({"v1": 6_i32, "v2": 2_i32, "front": 0_i32}),
    )
    .await;
    let (block_code, block_message, _) = call_err(
        &client,
        "join_linedefs",
        json!({"first": 0_i32, "second": 7_i32}),
    )
    .await;
    assert_eq!(block_code, "conflict", "a third line stands on the joint");
    assert!(
        block_message.contains("linedef 8"),
        "the blocker is named: {block_message}"
    );
    let rows = working_linedefs(&client, "MAP01").await;
    assert_eq!(rows.len(), 9, "the join refused; split and add stand");
}

#[tokio::test]
async fn join_linedefs_conflicts_on_properties_and_force_resolves_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    call(&client, "split_linedef", json!({"index": 0_i32})).await;
    call(
        &client,
        "edit_linedef",
        json!({"index": 7_i32, "set_flags": ["mapped"]}),
    )
    .await;
    let (code, message, _) = call_err(
        &client,
        "join_linedefs",
        json!({"first": 0_i32, "second": 7_i32}),
    )
    .await;
    assert_eq!(code, "conflict", "the halves now differ in flags");
    assert!(
        message.contains("flags") && message.contains("force: true"),
        "the differences and the fix are named: {message}"
    );
    let joined = call(
        &client,
        "join_linedefs",
        json!({"first": 0_i32, "second": 7_i32, "force": true}),
    )
    .await;
    assert_eq!(
        joined["linedef"],
        json!(0_i32),
        "the first linedef survived"
    );
    assert_eq!(
        counts(&client, "MAP01", "working").await,
        fixture_counts(),
        "the block is whole again"
    );
    let row = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(row["flag_names"], json!(["blocking"]), "and its flags won");
}
