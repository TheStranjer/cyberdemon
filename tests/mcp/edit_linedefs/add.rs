//! `add_linedef` as an MCP client calls it: one line appended through existing
//! vertex and sidedef indices, the same through created points and sides, the
//! vertex an earlier add already created coming back reused, the
//! `already_exists` duplicate found by either direction, the degenerate and
//! malformed-form refusals and the missing references as `not_found`, the
//! family-dependent flag defaults, the two-sided default a `back_side` brings
//! and the Hexen-family args with its tag rule, and the binary maps capping
//! points and offsets on the way in.

use serde_json::{Value, json};

use crate::edit_linedefs::{
    doom_loaded, formats_loaded, merged_linedefs, miss, refuse, vertex_count, working_linedef,
};
use crate::support::{call, call_err};

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
    let (code, message, _) = call_err(
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
    let (code, message, _) = call_err(
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
