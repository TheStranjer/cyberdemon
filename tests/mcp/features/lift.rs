//! `add_lift`: the fixture's 32-unit step as a platform (fresh tag on the
//! sector, the step line dressed `SR` 62 / `S1` 21 / `WR` 88 / `W1` 10 with
//! its front facing the lower hall and the theme's `lift_side` below the
//! ledge, the short-drop note present), a 96-unit ledge with no note, a
//! platform drawn across the step through the overlay engine, binary
//! `Hexen` `Plat_DownWaitUpStayLip` 206 with the tag in `args[0]` and the
//! `playeruse`/`playercross` field, the both/neither platform-selector
//! refusal, the named-fix refusals, and the one-step undo.

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, flag_names, formats_loaded, note_mentions, reported_lines,
    validate_clean, working_sector,
};
use crate::support::{call, call_err};

/// Requires `add_lift` to reject `args` with `invalid_argument`
/// naming `needle`, and the working archive to hold nothing.
async fn refuse_lift(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_lift", args).await;
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
        "the refused lift did not even copy the block into the working archive"
    );
}

/// Requires `add_lift` to reject `args` naming a missing sector with
/// the shared `not_found` index refusal mentioning `needle`, leaving
/// the working archive empty too.
async fn refuse_missing_lift(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, hint) = call_err(client, "add_lift", args).await;
    assert_eq!(code, "not_found", "index-refusal code for {note}");
    assert!(
        message.contains(needle) && message.contains("does not exist"),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    assert_eq!(
        hint, "describe_map or find_elements returns indices",
        "the shared index hint names the readers"
    );
    let (missing, _, _) = call_err(
        client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        missing, "not_found",
        "the refused lift did not even copy the block into the working archive"
    );
}

/// The fixture's step straddled by a new platform's outline: its
/// centre (272, 128) sits on the raised room.
fn platform_points() -> Value {
    json!([
        {"x": 224_i64, "y": 64_i64},
        {"x": 320_i64, "y": 64_i64},
        {"x": 320_i64, "y": 192_i64},
        {"x": 224_i64, "y": 192_i64},
    ])
}

/// Checks one Doom-family activation line of a lift: special
/// `special`, answering with `tag`, front at the lower hall and
/// wearing `WALL1`'s side band below the platform.
fn expect_lift_line(row: &Value, special: i32, tag: i32) {
    assert_eq!(row["special"], json!(special), "the lift action");
    assert_eq!(
        row["tag"],
        json!(tag),
        "the line answers with the platform tag"
    );
    assert_eq!(
        row["front"]["sector"],
        json!(0_i32),
        "the front faces the lower hall"
    );
    assert_eq!(
        row["back"]["sector"],
        json!(1_i32),
        "and the platform behind"
    );
    assert_eq!(
        row["front"]["lower"],
        json!("WALL1"),
        "the lift's side below the ledge"
    );
}

#[tokio::test]
async fn a_lift_on_the_step_tags_the_platform_and_notes_the_short_drop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let theme = call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"lift_side": "WALL1", "lift_floor": "FLAT10"}}),
    )
    .await;
    assert_eq!(
        theme["roles"]["lift_side"]["texture"],
        json!("WALL1"),
        "the side override stands for the role"
    );
    let result = call(&client, "add_lift", json!({"sector": 1_i32})).await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(
        result["platform_sector"],
        json!(1_i32),
        "the raised room rides"
    );
    assert_eq!(result["style"], json!("use_wall"), "the default style");
    assert_eq!(
        result["repeatable"],
        json!(true),
        "a lift repeats by default"
    );
    assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
    assert_eq!(
        result["floor_height"],
        json!(32_i32),
        "the platform's high floor"
    );
    assert_eq!(result["low_floor"], json!(0_i32), "the hall below");
    assert_eq!(result["drop"], json!(32_i32), "the 32-unit step");
    assert!(
        note_mentions(&result, "a lift is not needed; stairs or a step will do"),
        "the small drop is noted, not refused: {}",
        result["notes"]
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "a standing edge and its tags are a property edit"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    assert_eq!(lines.len(), 1_usize, "the step line only");
    expect_lift_line(lines.first().expect("the activation line"), 62_i32, 2_i32);
    let platform = working_sector(&client, "MAP01", 1_usize).await;
    assert_eq!(platform["tag"], json!(2_i32), "the sector carries the tag");
    assert_eq!(
        platform["floor_flat"],
        json!("FLAT10"),
        "the theme's lift floor"
    );
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn lift_styles_and_repeats_follow_the_doom_table() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"lift_side": "WALL1"}}),
    )
    .await;
    let once = call(
        &client,
        "add_lift",
        json!({"sector": 1_i32, "style": "USE_WALL", "repeatable": false}),
    )
    .await;
    assert_eq!(once["style"], json!("use_wall"), "echoed canonically");
    let lines = reported_lines(&client, "MAP01", &once, "lines").await;
    expect_lift_line(lines.first().expect("the activation line"), 21_i32, 2_i32);
    let crossed = call(
        &client,
        "add_lift",
        json!({"sector": 1_i32, "style": "walk_trigger"}),
    )
    .await;
    assert_eq!(crossed["tag"], json!(3_i32), "the next fresh tag");
    let crossed_lines = reported_lines(&client, "MAP01", &crossed, "lines").await;
    expect_lift_line(
        crossed_lines.first().expect("the activation line"),
        88_i32,
        3_i32,
    );
    assert_eq!(
        working_sector(&client, "MAP01", 1_usize).await["tag"],
        json!(3_i32),
        "the sector is re-stamped to match the lines"
    );
}

#[tokio::test]
async fn a_tall_ledge_lifts_without_the_short_drop_note() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"lift_side": "WALL1"}}),
    )
    .await;
    call(
        &client,
        "edit_sector",
        json!({"index": 1_i32, "floor_height": 96_i32}),
    )
    .await;
    let result = call(&client, "add_lift", json!({"sector": 1_i32})).await;
    assert_eq!(result["floor_height"], json!(96_i32), "the raised ledge");
    assert_eq!(
        result["low_floor"],
        json!(0_i32),
        "the hall's floor by default"
    );
    assert_eq!(result["drop"], json!(96_i32), "a real ride");
    assert!(
        result["notes"]
            .as_array()
            .expect("notes is an array")
            .is_empty(),
        "no note for a 96-unit drop: {}",
        result["notes"]
    );
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    expect_lift_line(lines.first().expect("the activation line"), 62_i32, 2_i32);
    validate_clean(&client, "MAP01").await;
}

/// Checks every drawn-platform activation row: two-sided between the
/// lower hall (sector 0) and the new platform (sector 2), carrying
/// special `special` with the shared tag, its front at the hall and
/// wearing `WALL1` as the lift's side band.
fn expect_hall_lines(rows: &[Value], special: i32, tag: &Value) {
    for row in rows {
        assert_eq!(
            row["front"]["sector"],
            json!(0_i32),
            "facing the lower hall: {row}"
        );
        assert_eq!(
            row["back"]["sector"],
            json!(2_i32),
            "the platform behind: {row}"
        );
        assert_eq!(row["special"], json!(special), "the lift action: {row}");
        assert_eq!(row["tag"], *tag, "the shared tag: {row}");
        assert_eq!(
            row["front"]["lower"],
            json!("WALL1"),
            "the lift's side: {row}"
        );
    }
}

/// Checks one `Hexen`-family lift line: special 206
/// `Plat_DownWaitUpStayLip` with the platform tag in `args[0]`, no
/// line tag, the front facing the lower hall, and every name of
/// `wanted` among its flag names.
fn expect_hexen_plat_line(row: &Value, tag: i32, wanted: &[&str]) {
    assert_eq!(row["special"], json!(206_i32), "Plat_DownWaitUpStayLip");
    assert_eq!(
        row["args"],
        json!([tag, 32_i32, 105_i32, 0_i32, 0_i32]),
        "tag, speed, wait"
    );
    assert_eq!(row["tag"], json!(0_i32), "a Hexen line carries no tag");
    assert_eq!(
        row["front"]["sector"],
        json!(0_i32),
        "the front faces the lower hall"
    );
    let names = flag_names(row);
    for name in wanted {
        assert!(names.contains(name), "{name} activates it: {names:?}");
    }
}

#[tokio::test]
async fn a_drawn_platform_lifts_from_where_the_player_stands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"lift_side": "WALL1"}}),
    )
    .await;
    let result = call(
        &client,
        "add_lift",
        json!({"platform": {"points": platform_points()}}),
    )
    .await;
    assert_eq!(result["platform_sector"], json!(2_i32), "the drawn sector");
    assert_eq!(
        result["floor_height"],
        json!(32_i32),
        "the raised room's height at the centre"
    );
    assert_eq!(result["low_floor"], json!(0_i32), "the hall below");
    assert!(
        note_mentions(&result, "a lift is not needed"),
        "a 32-unit ride: {}",
        result["notes"]
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the platform is new geometry"
    );
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    assert!(!lines.is_empty(), "the hall-facing edges activate");
    expect_hall_lines(&lines, 62_i32, &result["tag"]);
    let platform = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(
        platform["tag"], result["tag"],
        "the sector answers with the fresh tag"
    );
    assert_eq!(
        platform["floor_flat"],
        json!("FLAT10"),
        "the theme's lift floor"
    );
    assert_eq!(
        platform["floor_height"],
        json!(32_i32),
        "the copied high floor"
    );
    assert_eq!(platform["ceiling_height"], json!(128_i32), "and ceiling");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_hexen_map_gets_the_scripted_plat() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_lift",
        json!({"map": "MAP04", "sector": 1_i32}),
    )
    .await;
    assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
    let lines = reported_lines(&client, "MAP04", &result, "lines").await;
    assert_eq!(lines.len(), 1_usize, "the step line only");
    expect_hexen_plat_line(
        lines.first().expect("the step line"),
        2_i32,
        &["twosided", "playeruse", "repeatspecial"],
    );
    let crossed = call(
        &client,
        "add_lift",
        json!({"map": "MAP04", "sector": 1_i32, "style": "walk_trigger", "repeatable": false}),
    )
    .await;
    assert_eq!(crossed["tag"], json!(3_i32), "the next minted tag");
    let crossed_lines = reported_lines(&client, "MAP04", &crossed, "lines").await;
    let crossed_line = crossed_lines.first().expect("the step line");
    expect_hexen_plat_line(crossed_line, 3_i32, &["twosided", "playercross"]);
    let crossed_names = flag_names(crossed_line);
    assert!(
        !crossed_names.contains(&"playeruse"),
        "the field replaced outright: {crossed_names:?}"
    );
    assert!(
        !crossed_names.contains(&"repeatspecial"),
        "a one-shot clears the repeat bit: {crossed_names:?}"
    );
    assert_eq!(
        working_sector(&client, "MAP04", 1_usize).await["tag"],
        json!(3_i32),
        "the sector and the args agree"
    );
}

#[tokio::test]
async fn the_platform_parameter_needs_exactly_one_form() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_lift(&client, json!({}), "exactly one").await;
    refuse_lift(
        &client,
        json!({"platform": {"points": platform_points()}, "sector": 1_i32}),
        "exactly one",
    )
    .await;
}

#[tokio::test]
async fn lift_requests_name_their_fix() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_missing_lift(
        &client,
        json!({"sector": 99_i32}),
        "sector 99 does not exist",
    )
    .await;
    refuse_lift(
        &client,
        json!({"sector": 0_i32}),
        "not above its lift floor",
    )
    .await;
    refuse_lift(
        &client,
        json!({"sector": 1_i32, "style": "slide"}),
        "use_wall, walk_trigger",
    )
    .await;
    refuse_lift(
        &client,
        json!({"platform": {"points": [{"x": 0_i64, "y": 0_i64}, {"x": 64_i64, "y": 0_i64}]}}),
        "at least three corners",
    )
    .await;
    refuse_lift(
        &client,
        json!({"platform": {"points": [
            {"x": 5000_i64, "y": 5000_i64},
            {"x": 5064_i64, "y": 5000_i64},
            {"x": 5064_i64, "y": 5064_i64},
            {"x": 5000_i64, "y": 5064_i64},
        ]}}),
        "centre 5032, 5032 lies in no sector",
    )
    .await;
}

#[tokio::test]
async fn a_lift_undoes_as_one_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "add_lift", json!({"sector": 1_i32})).await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add use_wall lift"]),
        "the whole lift rolls back together"
    );
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(code, "not_found", "and the copy went with it");
}
