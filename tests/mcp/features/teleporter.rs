//! `add_teleporter`: the drawn pad and destination squares (a fresh tag on
//! the destination sector, the type-14 destination thing standing at the
//! destination point, the theme's `teleport_floor` on the pad's floor,
//! every two-sided pad border dressed `WR` 97 with its front facing out of
//! the pad), the `two_way` return leg with its own tag, pad-sector stamp and
//! backwards-facing second thing, binary `Hexen` `Teleport` 70 with the TID
//! on the thing and in `args[0]` of `playercross`-plus-`repeatspecial`
//! lines, and the missing-point, non-positive-`pad_size` and void-point
//! refusals. `reported_things` is `pub(super)` here for the `place`
//! module's thing rows.

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, flag_names, formats_loaded, index_list, reported_lines,
    validate_clean, working_sector,
};
use crate::support::{call, call_err};

/// Requires `add_teleporter` to reject `args` with `invalid_argument`
/// naming `needle`, and the working archive to hold nothing.
async fn refuse_teleporter(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_teleporter", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    let (missing, _, _) = call_err(
        client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        missing, "not_found",
        "the refused teleporter did not even copy the block into the working archive"
    );
}

/// The pad-and-destination points the teleporter tests use: the pad
/// stands in the hall, the destination in the raised room.
fn teleporter_points() -> Value {
    json!({
        "pad": {"x": 64_i64, "y": 64_i64},
        "destination": {"x": 384_i64, "y": 64_i64},
    })
}

/// The working thing rows at the indices of a result's `things` list.
pub(super) async fn reported_things(client: &Client, map: &str, result: &Value) -> Vec<Value> {
    let indices = index_list(result, "things");
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["things"]}),
    )
    .await;
    let all = report["things"]
        .as_array()
        .expect("things is an array")
        .clone();
    let rows = indices
        .iter()
        .filter_map(|index| all.get(*index).cloned())
        .collect::<Vec<Value>>();
    assert_eq!(rows.len(), indices.len(), "every thing index exists");
    rows
}

/// Checks one teleport-destination thing row: type 14 standing at
/// `at`, with the arrival facing and thing TID of `facing`.
fn expect_teleport_thing(row: &Value, at: (i64, i64), facing: (i32, i32)) {
    assert_eq!(row["type"], json!(14_i32), "a teleport destination");
    assert_eq!(row["x"], json!(at.0), "the thing's x");
    assert_eq!(row["y"], json!(at.1), "the thing's y");
    assert_eq!(row["angle"], json!(facing.0), "the arriving facing");
    assert_eq!(row["tid"], json!(facing.1), "the thing's tid");
}

/// Checks the four `Doom`-family teleport borders of one square:
/// special 97 answering with `tag`, their front at the surrounding
/// room `outer` and the square `square` sector behind.
fn expect_doom_teleport_lines(rows: &[Value], square: usize, outer: usize, tag: i32) {
    assert_eq!(rows.len(), 4_usize, "a border line on every side");
    for row in rows {
        assert_eq!(row["special"], json!(97_i32), "the WR teleport");
        assert_eq!(row["tag"], json!(tag), "answering with the destination tag");
        assert_eq!(
            row["front"]["sector"],
            json!(outer),
            "the front faces out of the square into its room"
        );
        assert_eq!(
            row["back"]["sector"],
            json!(square),
            "and the square lies behind, so walking in crosses it front-first"
        );
    }
}

/// Checks the four `Hexen`-family teleport borders of one square:
/// every side dressed with `tid` in its `args[0]`.
fn expect_hexen_teleport_lines(rows: &[Value], tid: i32) {
    assert_eq!(rows.len(), 4_usize, "a border line on every side");
    for row in rows {
        expect_teleport_hexen_line(row, tid);
    }
}

/// Checks one `Hexen`-family teleport border: special 70 Teleport
/// with `tid` in `args[0]`, no line tag, the whole trigger field
/// replaced by `playercross` plus `repeatspecial`.
fn expect_teleport_hexen_line(row: &Value, tid: i32) {
    assert_eq!(row["special"], json!(70_i32), "Teleport");
    assert_eq!(
        row["args"],
        json!([tid, 0_i32, 0_i32, 0_i32, 0_i32]),
        "the destination TID in args[0]"
    );
    assert_eq!(row["tag"], json!(0_i32), "a Hexen line carries no tag");
    let names = flag_names(row);
    assert!(
        names.contains(&"playercross"),
        "the field reads as a cross: {names:?}"
    );
    assert!(
        names.contains(&"repeatspecial"),
        "a pad keeps working: {names:?}"
    );
    assert!(
        !names.contains(&"playeruse"),
        "crossed, not used: {names:?}"
    );
    assert!(names.contains(&"twosided"), "the border stands: {names:?}");
}

/// Checks the echoed fields of a one-way `add_teleporter` result on
/// the features fixture: both squares drawn, the fresh tag, the
/// null return leg and the invalidated nodes.
fn expect_one_way_report(result: &Value) {
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["pad_sector"], json!(2_i32), "the drawn pad");
    assert_eq!(
        result["destination_sector"],
        json!(3_i32),
        "the drawn destination"
    );
    assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
    assert_eq!(result["return_tag"], Value::Null, "one-way has no return");
    assert_eq!(result["pad_size"], json!(64_i32), "the default side");
    assert_eq!(result["two_way"], json!(false), "echoed back");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "both squares are new geometry"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    assert_eq!(
        result["destination_lines"]
            .as_array()
            .expect("a list")
            .len(),
        0_usize,
        "one-way dresses no destination borders"
    );
}

#[tokio::test]
async fn a_doom_teleporter_tags_the_destination_and_dresses_the_pad_outwards() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let theme = call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"teleport_floor": "FLAT10"}}),
    )
    .await;
    assert_eq!(
        theme["roles"]["teleport_floor"]["texture"],
        json!("FLAT10"),
        "the pad floor override stands for the role"
    );
    let mut args = teleporter_points();
    args["destination"]["angle"] = json!(90_i32);
    let result = call(&client, "add_teleporter", args).await;
    expect_one_way_report(&result);
    let pad = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(
        pad["floor_flat"],
        json!("FLAT10"),
        "the theme's teleport floor"
    );
    assert_eq!(pad["floor_height"], json!(0_i32), "the hall's floor");
    assert_eq!(
        pad["tag"],
        json!(0_i32),
        "the outward leg tags the destination, not the pad"
    );
    let destination = working_sector(&client, "MAP01", 3_usize).await;
    assert_eq!(
        destination["tag"],
        json!(2_i32),
        "the vanilla search finds the thing through this tag"
    );
    assert_eq!(destination["floor_height"], json!(32_i32), "room 1's floor");
    let lines = reported_lines(&client, "MAP01", &result, "pad_lines").await;
    expect_doom_teleport_lines(&lines, 2_usize, 0_usize, 2_i32);
    let things = reported_things(&client, "MAP01", &result).await;
    expect_teleport_thing(
        things.first().expect("the destination thing"),
        (384_i64, 64_i64),
        (90_i32, 0_i32),
    );
    validate_clean(&client, "MAP01").await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add teleporter"]),
        "the whole teleporter rolls back together"
    );
}

#[tokio::test]
async fn a_two_way_teleporter_tags_the_pad_for_the_way_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut args = teleporter_points();
    args["destination"]["angle"] = json!(90_i32);
    args["two_way"] = json!(true);
    let result = call(&client, "add_teleporter", args).await;
    assert_eq!(result["two_way"], json!(true), "echoed back");
    assert_eq!(result["tag"], json!(2_i32), "the outward leg's tag");
    assert_eq!(
        result["return_tag"],
        json!(3_i32),
        "the return leg's own fresh tag"
    );
    let pad = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(
        pad["tag"],
        json!(3_i32),
        "the pad sector answers with the return tag"
    );
    let lines = reported_lines(&client, "MAP01", &result, "pad_lines").await;
    expect_doom_teleport_lines(&lines, 2_usize, 0_usize, 2_i32);
    let back = reported_lines(&client, "MAP01", &result, "destination_lines").await;
    expect_doom_teleport_lines(&back, 3_usize, 1_usize, 3_i32);
    let things = reported_things(&client, "MAP01", &result).await;
    assert_eq!(things.len(), 2_usize, "one destination thing per leg");
    expect_teleport_thing(
        things.first().expect("the destination thing"),
        (384_i64, 64_i64),
        (90_i32, 0_i32),
    );
    expect_teleport_thing(
        things.get(1).expect("the return thing"),
        (64_i64, 64_i64),
        (270_i32, 0_i32),
    );
    validate_clean(&client, "MAP01").await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add two-way teleporter"]),
        "both legs roll back as one step"
    );
}

#[tokio::test]
async fn a_hexen_teleporter_threads_the_tid_through_crossing_lines() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let mut args = teleporter_points();
    args["map"] = json!("MAP04");
    args["destination"]["angle"] = json!(45_i32);
    args["two_way"] = json!(true);
    let result = call(&client, "add_teleporter", args).await;
    assert_eq!(result["tag"], json!(2_i32), "the outward TID");
    assert_eq!(result["return_tag"], json!(3_i32), "the return TID");
    assert_eq!(result["pad_sector"], json!(2_i32));
    assert_eq!(result["destination_sector"], json!(3_i32));
    let lines = reported_lines(&client, "MAP04", &result, "pad_lines").await;
    expect_hexen_teleport_lines(&lines, 2_i32);
    let back = reported_lines(&client, "MAP04", &result, "destination_lines").await;
    expect_hexen_teleport_lines(&back, 3_i32);
    let things = reported_things(&client, "MAP04", &result).await;
    expect_teleport_thing(
        things.first().expect("the destination thing"),
        (384_i64, 64_i64),
        (45_i32, 2_i32),
    );
    expect_teleport_thing(
        things.get(1).expect("the return thing"),
        (64_i64, 64_i64),
        (225_i32, 3_i32),
    );
}

#[tokio::test]
async fn teleporter_arguments_are_validated() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_teleporter(&client, json!({}), "pad is required").await;
    refuse_teleporter(
        &client,
        json!({"pad": {"x": 64_i64, "y": 64_i64}}),
        "destination is required",
    )
    .await;
    refuse_teleporter(
        &client,
        json!({"destination": {"x": 384_i64, "y": 64_i64}}),
        "pad is required",
    )
    .await;
    refuse_teleporter(
        &client,
        json!({"pad": {"x": 64_i64, "y": 64_i64}, "destination": {"x": 384_i64, "y": 64_i64}, "pad_size": 0_i32}),
        "positive",
    )
    .await;
    refuse_teleporter(
        &client,
        json!({"pad": {"x": -64_i64, "y": -64_i64}, "destination": {"x": 384_i64, "y": 64_i64}}),
        "the pad point -64, -64 lies in no sector",
    )
    .await;
    refuse_teleporter(
        &client,
        json!({"pad": {"x": 64_i64, "y": 64_i64}, "destination": {"x": 5000_i64, "y": 5000_i64}}),
        "the destination point 5000, 5000 lies in no sector",
    )
    .await;
}
