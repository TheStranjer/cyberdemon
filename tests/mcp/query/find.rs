//! `find_elements` over the two-room fixture: its ops and derived fields,
//! the `within` bounds beside an `add_thing`, the compact rows, the paging and
//! the `invalid_argument`/`not_found` refusals.
//!
//! The imp added inside the fixture's room and the `name glob "imp*"` query
//! bounded by sectors are the two helpers defined here; the imp is
//! `pub(super)` because the `replace_surfaces` and `replace_things` tests add
//! it too. Every other helper comes from [`crate::query`].

use serde_json::{Value, json};

use crate::query::{Client, find, indices, one, plain_iwad, refuse};
use crate::support::call;

#[tokio::test]
async fn sectors_filter_by_light_and_the_report_carries_rows() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let report = find(
        &client,
        json!({ "kind": "sectors", "where": one("light", "gt", &json!(170_i64)) }),
    )
    .await;
    assert_eq!(report["kind"], json!("sectors"), "the kind travels");
    assert_eq!(report["count"], json!(1_i64), "one bright room");
    assert_eq!(indices(&report), vec![1_u64], "sector 1 at light 192");
    assert_eq!(report["rows"][0]["light"], json!(192_i64), "the row light");
    assert_eq!(
        report["rows"][0]["floor"],
        json!(32_i64),
        "and the raised floor"
    );
    assert_eq!(
        report["rows"][0]["ceiling_flat"],
        json!("FLOOR1"),
        "and the flat"
    );
    assert_eq!(report["next_offset"], Value::Null, "one page took all");
}

#[tokio::test]
async fn an_unfiltered_query_keeps_every_sector() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let all = find(&client, json!({ "kind": "sectors" })).await;
    assert_eq!(all["count"], json!(2_i64), "no filters keeps every sector");
    assert_eq!(indices(&all), vec![0_u64, 1], "ascending");
}

#[tokio::test]
async fn derived_linedef_fields_find_the_shared_step_line() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let two_sided = find(
        &client,
        json!({ "kind": "linedefs", "where": one("two_sided", "eq", &json!(true)) }),
    )
    .await;
    assert_eq!(indices(&two_sided), vec![2_u64], "the step line is shared");
    assert_eq!(
        two_sided["rows"][0]["v1"],
        json!(4_i64),
        "from the north vertex"
    );
    assert_eq!(
        two_sided["rows"][0]["v2"],
        json!(1_i64),
        "to the south vertex"
    );
    assert_eq!(
        two_sided["rows"][0]["front_sector"],
        json!(0_i64),
        "fronting the hall"
    );
    assert_eq!(
        two_sided["rows"][0]["back_sector"],
        json!(1_i64),
        "over the step"
    );
    let step = find(
        &client,
        json!({ "kind": "linedefs", "where": one("front_lower", "eq", &json!("STEP1")) }),
    )
    .await;
    assert_eq!(
        indices(&step),
        vec![2_u64],
        "the same line through its face"
    );
}

#[tokio::test]
async fn linedef_flags_and_length_measure_the_walls() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let blocking = find(
        &client,
        json!({ "kind": "linedefs", "where": one("flags", "has", &json!("blocking")) }),
    )
    .await;
    assert_eq!(
        indices(&blocking),
        vec![0_u64, 1, 3, 4, 5, 6],
        "every wall but the step line blocks"
    );
    let long = find(
        &client,
        json!({ "kind": "linedefs", "where": one("length", "eq", &json!(256_i64)) }),
    )
    .await;
    assert_eq!(long["count"], json!(7_i64), "the box walls are all 256");
}

#[tokio::test]
async fn a_doom_query_finds_the_mbf21_block_bits_by_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "set_flags": ["blockplayers", "blocklandmonsters"]}),
    )
    .await;
    let players = find(
        &client,
        json!({"kind": "linedefs", "where": one("flags", "has", &json!("blockplayers"))}),
    )
    .await;
    assert_eq!(
        indices(&players),
        vec![2_u64],
        "the step line now blocks the player"
    );
    let walkers = find(
        &client,
        json!({"kind": "linedefs", "where": one("flags", "has_not", &json!("blocklandmonsters"))}),
    )
    .await;
    assert_eq!(
        walkers["count"],
        json!(6_i64),
        "and six lines are open to walking monsters"
    );
    refuse(
        &client,
        json!({"kind": "linedefs", "where": one("flags", "has", &json!("blockeverything"))}),
        "invalid_argument",
        "unknown linedef flag 'blockeverything'",
    )
    .await;
}

#[tokio::test]
async fn things_filter_by_type_and_name_glob() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let start = find(
        &client,
        json!({ "kind": "things", "where": one("type", "eq", &json!(1_i64)) }),
    )
    .await;
    assert_eq!(indices(&start), vec![0_u64], "the player start");
    assert_eq!(start["rows"][0]["name"], json!("Player 1 Start"), "named");
    assert_eq!(start["rows"][0]["x"], json!(64_i64), "at 64, 128");
    let imps = find(
        &client,
        json!({ "kind": "things", "where": one("name", "glob", &json!("imp*")) }),
    )
    .await;
    assert_eq!(
        indices(&imps),
        vec![1_u64],
        "the fixture imp, lowercase pattern"
    );
}

/// Add a second imp inside sector 1 and report the client's row index.
pub(super) async fn add_imp_in_room(client: &Client) {
    call(
        client,
        "add_thing",
        json!({ "map": "MAP01", "type": 3001_i64, "x": 384_i64, "y": 64_i64 }),
    )
    .await;
}

#[tokio::test]
async fn a_newly_placed_imp_joins_the_glob() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let imps = find(
        &client,
        json!({ "kind": "things", "where": one("name", "glob", &json!("imp*")) }),
    )
    .await;
    assert_eq!(indices(&imps), vec![1_u64, 2], "the added imp joins");
    let ambush = find(
        &client,
        json!({ "kind": "things", "where": one("flags", "has", &json!("easy")) }),
    )
    .await;
    assert_eq!(
        ambush["count"],
        json!(3_i64),
        "every spawn carries a skill bit"
    );
}

/// A `name glob "imp*"` query bounded by these sectors.
fn imp_query(sectors: &Value) -> Value {
    json!({
        "kind": "things",
        "where": one("name", "glob", &json!("imp*")),
        "within": { "sectors": sectors },
    })
}

#[tokio::test]
async fn imps_are_bounded_by_their_sectors() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    add_imp_in_room(&client).await;
    let in_room = find(&client, imp_query(&json!([1_i64]))).await;
    assert_eq!(
        indices(&in_room),
        vec![1_u64, 2],
        "both imps stand in sector 1"
    );
    let in_hall = find(&client, imp_query(&json!([0_i64]))).await;
    assert_eq!(in_hall["count"], json!(0_i64), "none in the hall");
    assert!(
        in_hall["indices"].as_array().expect("a list").is_empty(),
        "an empty page, not an error"
    );
}

#[tokio::test]
async fn sidedefs_filter_by_their_textures() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let middle = find(
        &client,
        json!({ "kind": "sidedefs", "where": one("middle", "eq", &json!("WALL1")) }),
    )
    .await;
    assert_eq!(
        indices(&middle),
        vec![0_u64, 1, 4, 5, 6, 7],
        "the WALL1-dressed faces"
    );
    assert_eq!(
        middle["rows"][0]["sector"],
        json!(0_i64),
        "the sidedef shape"
    );
    let lower = find(
        &client,
        json!({ "kind": "sidedefs", "where": one("lower", "eq", &json!("STEP1")) }),
    )
    .await;
    assert_eq!(indices(&lower), vec![2_u64], "the step's lower face");
}

#[tokio::test]
async fn vertices_box_in_and_page_in_orders() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let boxed = find(
        &client,
        json!({ "kind": "vertices", "within": { "bbox": [0_i64, 0_i64, 256_i64, 256_i64] } }),
    )
    .await;
    assert_eq!(
        indices(&boxed),
        vec![0_u64, 1, 4, 5],
        "the hall box corners"
    );
    assert_eq!(
        boxed["rows"][1],
        json!({"index": 1_i64, "x": 256_i64, "y": 0_i64}),
        "the vertex shape"
    );
    let page1 = find(&client, json!({ "kind": "vertices", "limit": 2_i64 })).await;
    assert_eq!(indices(&page1), vec![0_u64, 1], "the first page of two");
    assert_eq!(page1["count"], json!(6_i64), "six vertices in total");
    assert_eq!(
        page1["next_offset"],
        json!(2_i64),
        "the rest continues at 2"
    );
}

#[tokio::test]
async fn paging_walks_to_the_end_and_past_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let last = find(
        &client,
        json!({ "kind": "vertices", "limit": 2_i64, "offset": 4_i64 }),
    )
    .await;
    assert_eq!(indices(&last), vec![4_u64, 5], "the final page");
    assert_eq!(last["next_offset"], Value::Null, "nothing left behind");
    let past = find(&client, json!({ "kind": "vertices", "offset": 99_i64 })).await;
    assert_eq!(past["count"], json!(6_i64), "the total stays the truth");
    assert!(
        past["indices"].as_array().expect("a list").is_empty(),
        "an offset past the end is empty, not an error"
    );
}

#[tokio::test]
async fn an_unknown_map_is_not_found() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    refuse(
        &client,
        json!({ "map": "MAP99", "kind": "sectors" }),
        "not_found",
        "MAP99",
    )
    .await;
}

#[tokio::test]
async fn bad_fields_ops_and_names_list_the_valid_ones() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    refuse(
        &client,
        json!({ "kind": "sectors", "where": one("aro", "eq", &json!(1_i64)) }),
        "invalid_argument",
        "valid fields are index, floor",
    )
    .await;
    refuse(
        &client,
        json!({ "kind": "sectors", "where": one("aro", "eq", &json!(1_i64)) }),
        "invalid_argument",
        "floor_flat",
    )
    .await;
    refuse(
        &client,
        json!({ "kind": "sectors", "where": one("floor_flat", "gt", &json!("A")) }),
        "invalid_argument",
        "does not apply",
    )
    .await;
    refuse(
        &client,
        json!({ "kind": "linedefs", "where": one("flags", "has", &json!("impassable")) }),
        "invalid_argument",
        "blocking",
    )
    .await;
    refuse(
        &client,
        json!({ "kind": "sectors", "where": one("light", "eq", &json!("x")) }),
        "invalid_argument",
        "needs a number",
    )
    .await;
}

#[tokio::test]
async fn limit_bounds_are_refused_before_anything_is_read() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    refuse(
        &client,
        json!({ "kind": "things", "limit": 0_i64 }),
        "invalid_argument",
        "between 1 and 1000",
    )
    .await;
    refuse(
        &client,
        json!({ "kind": "things", "limit": 1001_i64 }),
        "invalid_argument",
        "between 1 and 1000",
    )
    .await;
}
