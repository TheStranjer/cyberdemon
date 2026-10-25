//! `reorder_lumps`: one named lump moving to an index and another anchored
//! `before` or `after` it, a run of two moving by `count`, the run taken
//! `from_index`, a destination namespace creating its markers, a whole map
//! block travelling by count and the refusals around a run without a
//! destination, past the end, self-anchored or naming a missing lump, an
//! `IWAD` lump and the copy chosen by `from_namespace` and occurrence. The
//! `A` to `E` seeder and the twin-flat working archive live here.

use serde_json::json;

use crate::lumps::{
    Client, add, assert_untouched, iwad_only, names, replace_hex, row_named, undo_labels,
    working_names,
};
use crate::support::{call, call_err, connect};

/// Appends the global lumps `A` to `E`, in that order.
async fn letters(client: &Client) {
    for lump in ["A", "B", "C", "D", "E"] {
        add(client, lump, lump).await;
    }
}

#[tokio::test]
async fn reorder_lumps_moves_one_named_lump_and_anchors_another() {
    let client = connect().await;
    letters(&client).await;
    let front = call(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "index": 0_u64 }),
    )
    .await;
    assert_eq!(
        front,
        json!({ "moved": ["C"], "from_index": 2_u64, "to_index": 0_u64 }),
        "the documented result shape"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["C", "A", "B", "D", "E"],
        "C leads the directory now"
    );
    let anchored = call(
        &client,
        "reorder_lumps",
        json!({ "name": "B", "after": "E" }),
    )
    .await;
    assert_eq!(
        anchored["to_index"],
        json!(4_u64),
        "the directory without B has four lumps, so after E is index 4"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["C", "A", "D", "E", "B"],
        "B follows E, the destination counted after the removal"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec![
            "add_lump A",
            "add_lump B",
            "add_lump C",
            "add_lump D",
            "add_lump E",
            "reorder_lumps C",
            "reorder_lumps B"
        ],
        "each move is its own undoable step (the unit tests walk the stack back)"
    );
}

#[tokio::test]
async fn reorder_lumps_moves_a_run_of_two_before_a_named_lump() {
    let client = connect().await;
    letters(&client).await;
    let report = call(
        &client,
        "reorder_lumps",
        json!({ "name": "B", "count": 2_u64, "before": "A" }),
    )
    .await;
    assert_eq!(
        report,
        json!({ "moved": ["B", "C"], "from_index": 1_u64, "to_index": 0_u64 }),
        "the whole run is reported in directory order"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["B", "C", "A", "D", "E"],
        "B and C travelled together, still side by side"
    );
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "C", "scope": "working", "format": "text" }),
    )
    .await;
    assert_eq!(read["index"], json!(1_u64), "C kept its run position");
    assert_eq!(read["text"], json!("C"), "and its payload");
}

#[tokio::test]
async fn reorder_lumps_takes_a_run_from_from_index() {
    let client = connect().await;
    letters(&client).await;
    let report = call(
        &client,
        "reorder_lumps",
        json!({ "from_index": 0_u64, "count": 2_u64, "index": 3_u64 }),
    )
    .await;
    assert_eq!(
        report,
        json!({ "moved": ["A", "B"], "from_index": 0_u64, "to_index": 3_u64 }),
        "index 3 is the end of the three lumps left"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["C", "D", "E", "A", "B"],
        "the run was appended"
    );
}

#[tokio::test]
async fn reorder_lumps_into_a_namespace_creates_the_markers() {
    let client = connect().await;
    letters(&client).await;
    let report = call(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "count": 2_u64, "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        report,
        json!({ "moved": ["C", "D"], "from_index": 2_u64, "to_index": 4_u64 }),
        "the run landed between the new markers"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["A", "B", "E", "F_START", "C", "D", "F_END"],
        "a flats region was created at the end and the run sits inside it"
    );
    let flats = call(
        &client,
        "list_lumps",
        json!({ "scope": "working", "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        names(&flats),
        vec!["F_START", "C", "D", "F_END"],
        "the classifier sees the moved run as region members"
    );
}

#[tokio::test]
async fn reorder_lumps_moves_a_whole_map_block_by_count() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
    add(&client, "NOTE", "n").await;
    let report = call(
        &client,
        "reorder_lumps",
        json!({ "name": "MAP01", "count": 11_u64, "index": 1_u64 }),
    )
    .await;
    assert_eq!(
        report["moved"],
        json!([
            "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
            "SECTORS", "REJECT", "BLOCKMAP"
        ]),
        "the header and every member travelled as one run"
    );
    assert_eq!(
        report["to_index"],
        json!(1_u64),
        "index 1 of the single lump the block left behind"
    );
    let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        names(&listing).first(),
        Some(&"NOTE".to_owned()),
        "the note is first and the block follows it whole: {listing}"
    );
    let block = call(
        &client,
        "list_lumps",
        json!({ "scope": "working", "map": "MAP01" }),
    )
    .await;
    assert_eq!(
        block["total"].as_u64(),
        Some(11),
        "the block is still one block"
    );
    assert_eq!(
        row_named(&block, "MAP01")["index"].as_u64(),
        Some(1),
        "its header starts where the run landed"
    );
    assert_eq!(
        undo_labels(&client).await.last(),
        Some(&"reorder_lumps MAP01".to_owned()),
        "one undoable step for eleven lumps"
    );
}

#[tokio::test]
async fn reorder_lumps_refuses_a_run_without_a_destination() {
    let client = connect().await;
    letters(&client).await;
    let (both_code, both_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "from_index": 2_u64, "index": 0_u64 }),
    )
    .await;
    assert_eq!(both_code, "invalid_argument", "name and from_index");
    assert!(
        both_message.contains("exactly one"),
        "the message states the rule: {both_message}"
    );
    let (refined_code, refined_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "from_index": 2_u64, "occurrence": 0_u64, "index": 0_u64 }),
    )
    .await;
    assert_eq!(refined_code, "invalid_argument", "occurrence needs name");
    assert!(
        refined_message.contains("from_index"),
        "the message names the field: {refined_message}"
    );
    let (zero_code, zero_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "count": 0_u64, "index": 0_u64 }),
    )
    .await;
    assert_eq!(zero_code, "invalid_argument", "an empty run");
    assert!(
        zero_message.contains("at least 1"),
        "the message states the floor: {zero_message}"
    );
    let (none_code, none_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "occurrence": 1_u64 }),
    )
    .await;
    assert_eq!(none_code, "invalid_argument", "a destination is required");
    assert!(
        none_message.contains("no append default"),
        "the message says so: {none_message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["A", "B", "C", "D", "E"],
        "the plan refusals touched nothing"
    );
    assert_eq!(undo_labels(&client).await.len(), 5, "and recorded nothing");
}

#[tokio::test]
async fn reorder_lumps_refuses_a_run_past_the_end_a_self_anchor_and_missing_names() {
    let client = connect().await;
    letters(&client).await;
    let (overflow_code, overflow_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "D", "count": 4_u64, "index": 0_u64 }),
    )
    .await;
    assert_eq!(
        overflow_code, "invalid_argument",
        "the run leaves the archive"
    );
    assert!(
        overflow_message.contains("can hold at most 2"),
        "the message names the room: {overflow_message}"
    );
    let (index_code, index_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "A", "count": 2_u64, "index": 4_u64 }),
    )
    .await;
    assert_eq!(
        index_code, "invalid_argument",
        "index 4 of three lumps left"
    );
    assert!(
        index_message.contains("between 0 and 3"),
        "the range excludes the moved run: {index_message}"
    );
    let (inside_code, inside_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "B", "count": 3_u64, "after": "D" }),
    )
    .await;
    assert_eq!(inside_code, "invalid_argument", "the anchor is in the run");
    assert!(
        inside_message.contains("one of the moved lumps"),
        "the message says why: {inside_message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["A", "B", "C", "D", "E"],
        "every refusal left the order alone"
    );
    assert_eq!(
        undo_labels(&client).await.len(),
        5,
        "and none of them was recorded"
    );
}

#[tokio::test]
async fn reorder_lumps_reports_a_missing_run_or_anchor_as_not_found() {
    let client = connect().await;
    letters(&client).await;
    let (missing_code, missing_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "GHOST", "index": 0_u64 }),
    )
    .await;
    assert_eq!(missing_code, "not_found", "no such working lump");
    assert!(
        missing_message.contains("cannot be moved"),
        "the message explains the layers: {missing_message}"
    );
    let (past_code, past_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "C", "occurrence": 1_u64, "index": 0_u64 }),
    )
    .await;
    assert_eq!(
        past_code, "not_found",
        "one C, so occurrence 1 is out of range"
    );
    assert!(
        past_message.contains("0 to 0"),
        "the message names the range: {past_message}"
    );
    let (anchor_code, anchor_message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "A", "after": "GHOST" }),
    )
    .await;
    assert_eq!(anchor_code, "not_found", "no such anchor");
    assert!(
        anchor_message.contains("leaves in place"),
        "the message names the run rule: {anchor_message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["A", "B", "C", "D", "E"],
        "every refusal left the order alone"
    );
    assert_eq!(
        undo_labels(&client).await.len(),
        5,
        "and none of them was recorded"
    );
}

#[tokio::test]
async fn reorder_lumps_cannot_move_an_iwad_lump() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "PLAYPAL", "index": 0_u64 }),
    )
    .await;
    assert_eq!(code, "not_found", "an IWAD lump cannot be moved");
    assert!(
        message.contains("read-only"),
        "the message explains the layers: {message}"
    );
    assert!(
        message.contains("add_lump"),
        "the message names the way in: {message}"
    );
    assert_untouched(&client).await;
}

/// A working archive holding a loose `FLATA` and a flats-region `FLATA`,
/// so copy selection has something to choose between.
async fn twin_flats(client: &Client) {
    add(client, "FLATA", "loose").await;
    call(
        client,
        "add_lump",
        json!({ "name": "FLATA", "text": "region", "namespace": "flats" }),
    )
    .await;
}

#[tokio::test]
async fn reorder_lumps_picks_the_copy_with_from_namespace_and_occurrence() {
    let client = connect().await;
    twin_flats(&client).await;
    assert_eq!(
        working_names(&client).await,
        vec!["FLATA", "F_START", "FLATA", "F_END"],
        "a loose copy and a region copy"
    );
    let (code, message, _) = call_err(
        &client,
        "reorder_lumps",
        json!({ "name": "FLATA", "from_namespace": "sprites", "index": 0_u64 }),
    )
    .await;
    assert_eq!(code, "not_found", "no sprite FLATA");
    assert!(
        message.contains("the sprites region"),
        "the message names the region it looked in: {message}"
    );
    let region_copy = call(
        &client,
        "reorder_lumps",
        json!({ "name": "FLATA", "from_namespace": "flats", "index": 0_u64 }),
    )
    .await;
    assert_eq!(
        region_copy["from_index"],
        json!(2_u64),
        "from_namespace picked the region copy"
    );
    let first = call(
        &client,
        "get_lump",
        json!({
            "name": "FLATA",
            "scope": "working",
            "occurrence": 0_u64,
            "format": "text"
        }),
    )
    .await;
    assert_eq!(first["text"], json!("region"), "the moved copy leads");
    let loose_copy = call(
        &client,
        "reorder_lumps",
        json!({ "name": "FLATA", "occurrence": 1_u64, "after": "F_END" }),
    )
    .await;
    assert_eq!(
        loose_copy["from_index"],
        json!(1_u64),
        "occurrence 1 is the second of the two matches"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["FLATA", "F_START", "F_END", "FLATA"],
        "the second copy is loose again, behind the markers"
    );
}
