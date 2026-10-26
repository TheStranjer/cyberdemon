//! `create_sector_from_loop` as an MCP client calls it: the room grown next
//! to room 0 with its shared north wall opened, the nub welded to a wall's
//! mid-span through two T-junction splits, the platform raised inside room 0
//! with its four dressed edges, the `none` outer mode keeping every wall
//! one-sided, `keep_interior` preserving the pillar the earlier draw left and
//! the same draw without the flag overwriting it, and the malformed loops,
//! unknown maps, unknown vertices, unknown outer sectors and out-of-range
//! fields all being refused.
//!
//! The report and wall checks live here with the drawings they describe, the
//! single-wall one coming to the `carve` topic too; the request builders, row
//! readers and refusal wrappers come from [`crate::edit_sectors`].

use serde_json::{Value, json};

use crate::edit_sectors::{
    Client, doom_loaded, end, merged_sectors, pt, refuse_all, same_ends, square, validate_clean,
    working_linedef, working_linedefs, working_sector_row,
};
use crate::support::{call, call_err};

/// Checks the adjacent-room drawing report: the whole map rebuilds
/// deterministically, so the new piece order is part of the contract.
fn expect_room_report(result: &Value) {
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["sector"], json!(2_i32), "a third sector");
    assert_eq!(
        result["copied_to_working"],
        json!(true),
        "the block copied in"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the room is new geometry"
    );
    assert_eq!(
        result["vertices_created"],
        json!([6_i32, 7_i32]),
        "the two free corners were created, the room-0 top was reused"
    );
    assert_eq!(
        result["linedefs_created"],
        json!([2_i32, 4_i32, 7_i32]),
        "the three boundary walls are new pieces, the fourth edge is the old wall"
    );
    assert_eq!(
        result["sidedefs_created"],
        json!([2_i32, 4_i32, 5_i32, 9_i32]),
        "three new fronts and the shared wall's new room side"
    );
    assert_eq!(result["removed_linedefs"], json!([]), "nothing vanished");
    assert_eq!(
        result["linedef_map"],
        json!([0_i32, 3_i32, 5_i32, 1_i32, 8_i32, 9_i32, 6_i32]),
        "old linedef index to new index of its first piece"
    );
    assert_eq!(result["notes"], json!([]), "nobody lost their room");
}

/// Checks the shared north wall (new linedef 3), opened two-sided with
/// its texture moved onto the free faces.
fn expect_shared_wall(line: &Value) {
    assert_eq!(
        line["flag_names"],
        json!(["twosided"]),
        "blocking cleared, two-sided set"
    );
    assert_eq!(line["front"]["sector"], json!(0_i32), "faces the hall");
    assert_eq!(line["front"]["middle"], json!("-"), "the middle moved");
    assert_eq!(
        line["front"]["upper"],
        json!("WALL1"),
        "onto the free faces"
    );
    assert_eq!(line["back"]["sector"], json!(2_i32), "and the new room");
    assert_eq!(line["back"]["middle"], json!("-"), "bare on the back");
}

/// Checks platform edge `edge`, a two-sided transition dressed on both
/// faces: the hall below and the platform face each other, either way
/// round (the engine orients every line by its coordinates).
fn expect_platform_edge(line: &Value, edge: usize) {
    assert_eq!(line["flag_names"], json!(["twosided"]), "line {edge}");
    let (front, back) = (&line["front"], &line["back"]);
    let faces = [front["sector"].clone(), back["sector"].clone()];
    assert!(
        (faces[0] == json!(2_i32) && faces[1] == json!(0_i32))
            || (faces[0] == json!(0_i32) && faces[1] == json!(2_i32)),
        "hall and platform either way round: {faces:?}"
    );
    for side in [front, back] {
        assert_eq!(side["middle"], json!("-"), "the ledge face is open");
        assert_eq!(side["upper"], json!("STEP1"), "dressed above");
        assert_eq!(side["lower"], json!("STEP1"), "and below");
    }
}

/// Requires every linedef `created` by the last report to be a
/// one-sided wall wearing `middle`; `sector`, when given, is the
/// sector every wall must face.
pub(super) fn expect_single_walls(
    lines: &[Value],
    created: &[Value],
    sector: Option<i32>,
    middle: &str,
) {
    assert!(!created.is_empty(), "the report listed walls: {created:?}");
    for index in created {
        let position = usize::try_from(index.as_i64().expect("created indices are numbers"))
            .expect("non-negative");
        check_one_wall(
            lines
                .get(position)
                .unwrap_or_else(|| panic!("line {position}")),
            sector,
            middle,
        );
    }
}

/// Checks one carved wall: blocking, one-sided, wearing `middle` and,
/// when given, facing `sector`.
fn check_one_wall(line: &Value, sector: Option<i32>, middle: &str) {
    assert_eq!(line["flag_names"], json!(["blocking"]), "a single wall");
    assert_eq!(line["back"], Value::Null, "no side into the rock");
    assert_eq!(line["front"]["middle"], json!(middle), "its texture");
    let faced = line["front"]["sector"].clone();
    match sector {
        Some(want) => assert_eq!(faced, json!(want), "the wall faces its sector"),
        None => assert!(faced.is_number(), "the wall faces some sector"),
    }
}

#[tokio::test]
async fn create_sector_from_loop_grows_a_room_next_to_room_zero() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "create_sector_from_loop",
        json!({"points": [pt(0, 256), pt(256, 256), pt(256, 512), pt(0, 512)]}),
    )
    .await;
    expect_room_report(&result);
    expect_shared_wall(&working_linedef(&client, "MAP01", 3).await);
    let hall = working_sector_row(&client, "MAP01", 0).await;
    assert_eq!(
        hall["neighbors"],
        json!([1_i32, 2_i32]),
        "the hall now also neighbours the new room"
    );
    let room = working_sector_row(&client, "MAP01", 2).await;
    assert_eq!(room["neighbors"], json!([0_i32]), "one shared wall");
    assert_eq!(
        room["bbox"],
        json!([0_i32, 256_i32, 256_i32, 512_i32]),
        "the new box"
    );
    assert_eq!(
        room["floor_flat"],
        json!("FLOOR1"),
        "the theme-resolved sector defaults stored"
    );
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        summary["counts"],
        json!({"things": 2_i32, "vertices": 8_i32, "linedefs": 10_i32,
               "sidedefs": 12_i32, "sectors": 3_i32}),
        "the whole map grew as reported"
    );
    validate_clean(&client, "MAP01").await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["create sector from loop"]),
        "the drawing is one undo step"
    );
    let summary_after = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "merged"}),
    )
    .await;
    assert_eq!(
        summary_after["counts"]["linedefs"],
        json!(7_i32),
        "the base layer kept its original seven lines"
    );
}

#[tokio::test]
async fn create_sector_from_loop_welds_to_a_wall_mid_span() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "create_sector_from_loop",
        json!({"points": [pt(64, 256), pt(192, 256), pt(192, 384), pt(64, 384)]}),
    )
    .await;
    assert_eq!(result["sector"], json!(2_i32), "a third sector");
    assert_eq!(
        result["linedefs_created"],
        json!([3_i32, 5_i32, 6_i32]),
        "the nub's three free edges are new"
    );
    assert_eq!(
        result["vertices_created"],
        json!([6_i32, 7_i32, 8_i32, 9_i32]),
        "the two T-junction points and the two corners are new"
    );
    assert_eq!(
        result["linedef_map"][1],
        json!(2_i32),
        "the old hall wall maps to its west piece"
    );
    assert_eq!(
        result["removed_linedefs"],
        json!([]),
        "the split wall still lives in its pieces"
    );
    let lines = working_linedefs(&client, "MAP01").await;
    let shared = lines
        .iter()
        .find(|line| same_ends(line, [end(64, 256), end(192, 256)]))
        .expect("the shared piece exists");
    expect_shared_wall(shared);
    assert_eq!(
        shared["front"]["x_offset"],
        json!(64_i32),
        "the texture slid with the piece start"
    );
    let hall = working_sector_row(&client, "MAP01", 0).await;
    assert_eq!(
        hall["neighbors"],
        json!([1_i32, 2_i32]),
        "the hall faces the nub across the opened piece"
    );
    let nub = working_sector_row(&client, "MAP01", 2).await;
    assert_eq!(
        nub["bbox"],
        json!([64_i32, 256_i32, 192_i32, 384_i32]),
        "the nub box"
    );
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn create_sector_from_loop_raises_a_platform_inside_room_zero() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "create_sector_from_loop",
        square(
            r#", "floor_height": 8, "ceiling_height": 192,
                 "upper_texture": "STEP1", "lower_texture": "step1""#,
        ),
    )
    .await;
    assert_eq!(result["sector"], json!(2_i32));
    assert_eq!(
        result["linedefs_created"],
        json!([3_i32, 4_i32, 5_i32, 6_i32]),
        "all four walls are new lines"
    );
    assert_eq!(
        result["vertices_created"],
        json!([6_i32, 7_i32, 8_i32, 9_i32]),
        "the four corners are new vertices"
    );
    assert_eq!(
        result["sidedefs_created"].as_array().expect("a list").len(),
        8_usize,
        "two sides per wall"
    );
    expect_platform_edge(&working_linedef(&client, "MAP01", 3).await, 3_usize);
    expect_platform_edge(&working_linedef(&client, "MAP01", 4).await, 4_usize);
    expect_platform_edge(&working_linedef(&client, "MAP01", 5).await, 5_usize);
    expect_platform_edge(&working_linedef(&client, "MAP01", 6).await, 6_usize);
    let platform = working_sector_row(&client, "MAP01", 2).await;
    assert_eq!(platform["floor_height"], json!(8_i32), "raised floor");
    assert_eq!(platform["ceiling_height"], json!(192_i32), "and ceiling");
    assert_eq!(platform["neighbors"], json!([0_i32]), "across is the hall");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn create_sector_from_loop_with_none_outer_keeps_walls_single() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "create_sector_from_loop",
        square(r#", "outer_sector": "none""#),
    )
    .await;
    assert_eq!(
        result["linedefs_created"],
        json!([3_i32, 4_i32, 5_i32, 6_i32]),
        "the four sealed walls"
    );
    assert_eq!(
        result["sidedefs_created"].as_array().expect("a list").len(),
        4_usize,
        "one front side per wall"
    );
    let line = working_linedef(&client, "MAP01", 3).await;
    assert_eq!(line["flag_names"], json!(["blocking"]), "a sealed box");
    assert_eq!(line["back"], Value::Null, "no back side");
    assert_eq!(
        line["front"]["middle"],
        json!("STEP1"),
        "the theme-resolved default wall texture"
    );
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn create_sector_from_loop_keep_interior_preserves_the_pillar() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    call(&client, "create_sector_from_loop", square("")).await;
    let big = call(
        &client,
        "create_sector_from_loop",
        json!({
            "points": [pt(32, 32), pt(224, 32), pt(224, 224), pt(32, 224)],
            "keep_interior": true, "ceiling_height": 160_i64,
        }),
    )
    .await;
    assert_eq!(big["sector"], json!(3_i32), "a fourth sector");
    assert_eq!(
        big["empty_sectors"],
        json!([]),
        "the pillar keeps its area: {}",
        big["notes"]
    );
    let platform = working_sector_row(&client, "MAP01", 2).await;
    assert_eq!(
        platform["sidedefs"].as_array().expect("a list").len(),
        4_usize,
        "the pillar's four sides stand"
    );
    assert_eq!(
        platform["neighbors"],
        json!([3_i32]),
        "boxed in by the new room"
    );
    let room = working_sector_row(&client, "MAP01", 3).await;
    assert!(
        room["neighbors"]
            .as_array()
            .expect("a list")
            .contains(&json!(2_i32)),
        "the new room wraps the pillar"
    );
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn create_sector_from_loop_without_the_flag_overwrites_the_pillar() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    call(&client, "create_sector_from_loop", square("")).await;
    call(
        &client,
        "create_sector_from_loop",
        json!({
            "points": [pt(32, 32), pt(224, 32), pt(224, 224), pt(32, 224)],
            "keep_interior": true, "ceiling_height": 160_i64,
        }),
    )
    .await;
    let overwritten = call(
        &client,
        "create_sector_from_loop",
        json!({
            "points": [pt(16, 16), pt(240, 16), pt(240, 240), pt(16, 240)],
            "ceiling_height": 192_i64,
        }),
    )
    .await;
    assert_eq!(
        overwritten["empty_sectors"],
        json!([2_i32, 3_i32]),
        "without the flag the previous room and the pillar are overwritten"
    );
    let notes = overwritten["notes"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<&str>>();
    assert!(
        notes.iter().any(|note| note.contains("sector 3")),
        "the note names it: {notes:?}"
    );
}

/// Loop requests the tool must reject with `invalid_argument`: a
/// missing or doubled form, a weak, flat or crossing ring, and fields
/// outside the format's ranges.
fn invalid_loop_requests() -> Vec<Value> {
    vec![
        json!({}),
        square(r#", "vertices": [0, 1, 2]"#),
        json!({"points": [pt(0, 0), pt(128, 128)]}),
        json!({"points": [pt(0, 0), pt(128, 0), pt(256, 0)]}),
        json!({"points": [pt(0, 0), pt(256, 0), pt(64, 192), pt(256, 256)]}),
        square(", \"light\": 300"),
        square(", \"ceiling_flat\": \"-\""),
        square(", \"wall_texture\": \"TOOLONG12\""),
        square(", \"outer_sector\": \"sideways\""),
        json!({"points": [pt(0, 0), pt(40000, 0), pt(0, 256)]}),
    ]
}

/// Requires `create_sector_from_loop` to refuse every one of
/// `requests`.
async fn refuse_loops(client: &Client, requests: &[Value]) {
    refuse_all(client, "create_sector_from_loop", requests.to_vec()).await;
}

#[tokio::test]
async fn create_sector_from_loop_refuses_bad_requests() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    refuse_loops(&client, &invalid_loop_requests()).await;
    let (code, _, _) = call_err(
        &client,
        "create_sector_from_loop",
        json!({"map": "MAP09", "points": [pt(0, 0), pt(128, 0), pt(0, 128)]}),
    )
    .await;
    assert_eq!(code, "not_found", "unknown map");
    let (vertex_code, _, _) = call_err(
        &client,
        "create_sector_from_loop",
        json!({"vertices": [0_i32, 1_i32, 99_i32]}),
    )
    .await;
    assert_eq!(vertex_code, "not_found", "unknown vertex");
    let (outer_code, message, _) = call_err(
        &client,
        "create_sector_from_loop",
        square(", \"outer_sector\": 9"),
    )
    .await;
    assert_eq!(outer_code, "not_found", "unknown outer sector");
    assert!(
        message.contains("map has 2 sectors"),
        "the message counts the sectors: {message}"
    );
    let rows = merged_sectors(&client, "MAP01").await;
    assert_eq!(rows.len(), 2, "the refusals drew nothing");
}
