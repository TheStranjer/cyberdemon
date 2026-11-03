//! `add_door` between two touching rooms: the void strip carved, the recess
//! closed, both room-facing lines dressed and their map still `validate_map`
//! clean; the locked-blue key door with its coloured track; the `Hexen`-
//! family script doors (`Door_Raise` on binary `Hexen`, `Door_Open` on
//! `UDMF` `zdoom`); the inert tagged `remote` door; the `side: "back"` door
//! dug through a room's one-sided wall into the shop behind it; and the
//! one-step undo. Every listed refusal is here too: an over-wide door
//! naming the available length, a one-sided wall with nothing behind it,
//! the side that would reach the room behind such a wall, a depth that
//! reopens the room it came from, an unknown `side` name, the both/neither
//! wall parameter, bad style names and out-of-range sector/
//! linedef indices. A fullbright `render_view` through the theme override
//! shows the built door's `DOOR1` face from the room it opens.

use serde_json::{Value, json};

use crate::features::{
    Client, count_pixels, features_loaded, formats_loaded, l_room_loaded, note_mentions,
    refuse_missing_with, refuse_with, rendered_image, reported_lines, validate_clean,
    working_sector,
};
use crate::support::{call, call_err};

/// Checks one `Doom`-family use line: action `special`, line tag
/// `tag`, the room-facing `upper` wearing `DOOR1` and the far upper
/// clear; returns the sector the front faces.
fn expect_doom_use_line(row: &Value, special: i32) -> Value {
    assert_eq!(row["special"], json!(special), "the door action");
    assert_eq!(row["tag"], json!(0_i32), "tag 0 acts behind the line");
    assert_eq!(
        row["front"]["upper"],
        json!("DOOR1"),
        "the room-facing upper wears the door"
    );
    assert_eq!(
        row["back"]["upper"],
        json!("-"),
        "the door side stays clear"
    );
    row["front"]["sector"].clone()
}

/// The sectors the `Doom`-family use lines face.
fn expect_doom_use_lines(rows: &[Value], special: i32) -> Vec<Value> {
    rows.iter()
        .map(|row| expect_doom_use_line(row, special))
        .collect()
}

/// Checks the jamb rows: each one-sided, facing the door sector,
/// wearing `track` in the middle.
fn expect_track_lines(rows: &[Value], track: &str) {
    assert_eq!(rows.len(), 2, "a track wall on each side");
    for row in rows {
        assert_eq!(row["back"], Value::Null, "the jamb is one-sided");
        assert_eq!(row["front"]["middle"], json!(track), "the track");
        assert_eq!(row["front"]["sector"], json!(2_i32), "it faces the recess");
    }
}

/// Checks the inert `remote` rows: no action, pre-tagged `tag`.
fn expect_inert_lines(rows: &[Value], tag: i32) {
    assert!(!rows.is_empty(), "the door lines exist");
    for row in rows {
        assert_eq!(
            row["special"],
            json!(0_i32),
            "no action on the door's lines"
        );
        assert_eq!(row["tag"], json!(tag), "pre-tagged for the future switch");
    }
}

/// Checks one `Hexen`-family `Door_Raise` line: special 12, the
/// speed-16/delay-150 arguments and the use, repeat and monster
/// activation bits.
fn expect_door_raise(row: &Value) {
    assert_eq!(row["special"], json!(12_i32), "Door_Raise");
    assert_eq!(
        row["args"],
        json!([0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
        "speed 16, delay 150, the sector behind the line"
    );
    let names = row["flag_names"]
        .as_array()
        .expect("flag names is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<&str>>();
    for wanted in ["playeruse", "repeatspecial", "monsteractivate"] {
        assert!(names.contains(&wanted), "{wanted} activates it: {names:?}");
    }
}

/// Checks the `Hexen`-family `Door_Raise` rows.
fn expect_door_raise_lines(rows: &[Value]) {
    assert_eq!(rows.len(), 2, "one use line per room");
    for row in rows {
        expect_door_raise(row);
    }
}

/// Checks the `Hexen`-family `Door_Open` (`stay_open`) rows.
fn expect_door_open_lines(rows: &[Value]) {
    assert_eq!(rows.len(), 2, "one use line per room");
    for row in rows {
        assert_eq!(row["special"], json!(11_i32), "Door_Open");
        assert_eq!(row["args"], json!([0_i32, 16_i32, 0_i32, 0_i32, 0_i32]));
    }
}

#[tokio::test]
async fn a_door_between_touching_rooms_opens_both_rooms() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["style"], json!("normal"), "the default style");
    assert_eq!(result["door_sector"], json!(2_i32), "a third sector");
    assert_eq!(result["tag"], Value::Null, "plain doors carry no tag");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the doorway is new geometry"
    );
    assert_eq!(
        result["copied_to_working"],
        json!(true),
        "the block copied in"
    );
    assert!(
        note_mentions(&result, "void strip"),
        "the strip carve is noted: {}",
        result["notes"]
    );
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    assert_eq!(lines.len(), 2, "one use line per room");
    let fronts = expect_doom_use_lines(&lines, 1_i32);
    assert!(
        fronts.contains(&json!(0_i32)) && fronts.contains(&json!(1_i32)),
        "the fronts face their rooms: {fronts:?}"
    );
    let tracks = reported_lines(&client, "MAP01", &result, "jambs").await;
    expect_track_lines(&tracks, "DOORTRAK");
    let door = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(
        door["ceiling_height"], door["floor_height"],
        "the door sector is closed"
    );
    assert_eq!(door["floor_height"], json!(0_i32), "room A's floor");
    assert_eq!(
        door["floor_flat"],
        json!("FLAT10"),
        "the theme's recess floor"
    );
    assert_eq!(door["tag"], json!(0_i32), "untagged");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_locked_blue_door_wears_the_blue_track() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_door",
        json!({"between": [0_i32, 1_i32], "style": "locked_blue"}),
    )
    .await;
    assert_eq!(result["style"], json!("locked_blue"), "echoed canonically");
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    expect_doom_use_lines(&lines, 26_i32);
    let tracks = reported_lines(&client, "MAP01", &result, "jambs").await;
    expect_track_lines(&tracks, "DOORBLU");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_remote_door_is_tagged_and_inert() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_door",
        json!({"between": [0_i32, 1_i32], "style": "remote"}),
    )
    .await;
    assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
    let door = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(door["tag"], json!(2_i32), "the sector answers with it");
    assert_eq!(
        door["ceiling_height"], door["floor_height"],
        "the recess is closed and waits for a switch"
    );
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    expect_inert_lines(&lines, 2_i32);
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_wider_door_than_the_wall_names_the_available_length() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_with(
        &client,
        json!({"between": [0_i32, 1_i32], "width": 300_i32}),
        "256 map units long",
    )
    .await;
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "the refused door did not even copy the block into the working archive"
    );
}

#[tokio::test]
async fn a_wall_with_nothing_behind_refuses_the_recess() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = l_room_loaded(dir.path()).await;
    refuse_with(
        &client,
        json!({"linedef": 0_i32}),
        "no room behind this wall within 256 units; move the rooms closer or give a \
         larger depth",
    )
    .await;
}

#[tokio::test]
async fn an_explicit_depth_must_reach_another_sector() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = l_room_loaded(dir.path()).await;
    refuse_with(
        &client,
        json!({"linedef": 0_i32, "depth": 600_i32}),
        "opens back into sector 0 on every side",
    )
    .await;
    refuse_with(
        &client,
        json!({"linedef": 0_i32, "depth": 600_i32}),
        "pass side: \"back\"",
    )
    .await;
}

/// The `CD-347` mall shape: a shop drawn in the void 64 units off a
/// fixture room's own one-sided wall. `points` is the shop's loop and `wall`
/// the fixture wall the shop faces; returns the drawn sector and that wall
/// as the rebuild renumbered it.
async fn shop_off_the_wall(client: &Client, points: [Value; 4], wall: usize) -> (Value, Value) {
    let drawn = call(client, "create_sector_from_loop", json!({"points": points})).await;
    let kept = drawn["linedef_map"]
        .get(wall)
        .and_then(Value::as_u64)
        .expect("the door wall survives the draw");
    (drawn["sector"].clone(), json!(kept))
}

/// The shop north of the L fixture, 64 units off its top wall (linedef 0).
fn north_of_the_l() -> ([Value; 4], usize) {
    (
        [
            json!({"x": 0_i64, "y": 320_i64}),
            json!({"x": 128_i64, "y": 320_i64}),
            json!({"x": 128_i64, "y": 448_i64}),
            json!({"x": 0_i64, "y": 448_i64}),
        ],
        0_usize,
    )
}

/// The shop west of the features fixture's hall, 64 units off its west wall
/// (linedef 0).
fn west_of_the_hall() -> ([Value; 4], usize) {
    (
        [
            json!({"x": -64_i64, "y": 64_i64}),
            json!({"x": -64_i64, "y": 192_i64}),
            json!({"x": -192_i64, "y": 192_i64}),
            json!({"x": -192_i64, "y": 64_i64}),
        ],
        0_usize,
    )
}

#[tokio::test]
async fn a_one_sided_wall_with_a_sector_behind_it_names_the_side() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = l_room_loaded(dir.path()).await;
    let (points, index) = north_of_the_l();
    let (_, wall) = shop_off_the_wall(&client, points, index).await;
    refuse_with(
        &client,
        json!({"linedef": wall}),
        "no room behind this wall within 256 units",
    )
    .await;
    refuse_with(
        &client,
        json!({"linedef": wall}),
        "a sector lies 64 units on this wall's back side: pass side: \"back\"",
    )
    .await;
    refuse_with(
        &client,
        json!({"linedef": wall, "depth": 64_i32}),
        format!(
            "the door recess on linedef {wall} opens back into sector 0 on every side: it \
             would connect nothing"
        )
        .as_str(),
    )
    .await;
    refuse_with(
        &client,
        json!({"linedef": wall, "depth": 64_i32}),
        format!("pass side: \"back\" to dig the door recess through linedef {wall}").as_str(),
    )
    .await;
    let left = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        left["counts"]["sectors"],
        json!(2_u64),
        "no refusal drew a sector"
    );
}

#[tokio::test]
async fn an_unknown_side_and_a_side_on_the_wrong_wall_are_refused() {
    /// The `add_door` refusal for `args`, as its message text.
    async fn refused(client: &Client, args: Value) -> String {
        let (code, message, _) = call_err(client, "add_door", args).await;
        assert_eq!(code, "invalid_argument", "a rejected side is an argument");
        message
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let named = refused(&client, json!({"linedef": 0_i32, "side": "west"})).await;
    assert!(
        named.contains("unknown side \"west\": pass one of \"front\", \"back\""),
        "the refusal lists both: {named}"
    );
    let two_sided = refused(&client, json!({"linedef": 2_i32, "side": "back"})).await;
    assert!(
        two_sided.contains("linedef 2 has a sector on both of its sides"),
        "the wall says what it has: {two_sided}"
    );
    let joined = refused(&client, json!({"between": [0_i32, 1_i32], "side": "front"})).await;
    assert!(
        joined.contains("has a sector on both of its sides"),
        "and `between` meets the same refusal: {joined}"
    );
    let missing = refused(&client, json!({"linedef": 0_i32, "depth": 0_i32})).await;
    assert!(
        missing.contains("positive number of map units"),
        "a depth is checked before the side is used: {missing}"
    );
}

#[tokio::test]
async fn rooms_that_never_touch_are_told_the_back_side_joins_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = l_room_loaded(dir.path()).await;
    let (points, index) = north_of_the_l();
    let (shop, _) = shop_off_the_wall(&client, points, index).await;
    let (code, message, _) = call_err(
        &client,
        "add_door",
        json!({"between": [0_i32, shop], "side": "front"}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "the two sectors share no wall");
    assert!(
        message.contains("no two-sided boundary"),
        "and says so: {message}"
    );
    assert!(
        message.contains("side: \"back\""),
        "naming the dig that joins them: {message}"
    );
}

#[tokio::test]
async fn a_door_dug_through_a_one_sided_wall_joins_the_rooms_behind_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let (points, wanted) = west_of_the_hall();
    let (shop, wall) = shop_off_the_wall(&client, points, wanted).await;
    let sealed = call(&client, "check_playability", json!({})).await;
    assert!(
        sealed["sectors"]["unreachable"]
            .as_array()
            .expect("a sector list")
            .contains(&shop),
        "the drawn shop starts out cut off: {}",
        sealed["summary"]
    );
    let result = call(
        &client,
        "add_door",
        json!({"linedef": wall, "side": "back"}),
    )
    .await;
    let door = result["door_sector"].as_u64().expect("a door sector");
    assert_eq!(result["style"], json!("normal"), "the default action");
    assert!(
        !note_mentions(&result, "void strip"),
        "a one-sided dig carves no strip: {}",
        result["notes"]
    );
    assert!(
        note_mentions(
            &result,
            format!("through linedef {wall}'s back side").as_str()
        ),
        "the note names the side it dug: {}",
        result["notes"]
    );
    let lines = reported_lines(&client, "MAP01", &result, "lines").await;
    assert_eq!(lines.len(), 2, "the named wall and the shop's own wall");
    let fronts = expect_doom_use_lines(&lines, 1_i32);
    assert!(fronts.contains(&json!(0_i32)), "the hall faces its wall");
    assert!(fronts.contains(&shop), "and the shop faces it: {fronts:?}");
    expect_stub(&client, &result, door).await;
    validate_clean(&client, "MAP01").await;
    let walked = call(&client, "check_playability", json!({})).await;
    assert!(
        !walked["sectors"]["unreachable"]
            .as_array()
            .expect("a sector list")
            .contains(&shop),
        "the door reaches the shop: {}",
        walked["summary"]
    );
}

/// Checks the stub a back-side dig leaves in what was the void: two
/// floor-pinned track walls facing `door`, and the door sector itself
/// closed on the hall's floor with the theme's recess flat.
async fn expect_stub(client: &Client, result: &Value, door: u64) {
    let tracks = reported_lines(client, "MAP01", result, "jambs").await;
    assert_eq!(tracks.len(), 2, "a track wall on each side of the stub");
    for row in &tracks {
        assert_eq!(row["back"], Value::Null, "the jamb is one-sided");
        assert_eq!(row["front"]["middle"], json!("DOORTRAK"), "the track");
        assert_eq!(row["front"]["sector"], json!(door), "and faces the recess");
    }
    let open = working_sector(client, "MAP01", usize::try_from(door).expect("an index")).await;
    assert_eq!(
        open["ceiling_height"], open["floor_height"],
        "the recess is closed"
    );
    assert_eq!(
        open["floor_height"],
        json!(0_i32),
        "and stands on the hall's floor"
    );
    assert_eq!(open["floor_flat"], json!("FLAT10"), "the theme floor");
}

#[tokio::test]
async fn the_wall_parameter_needs_exactly_one_form() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_with(&client, json!({}), "exactly one").await;
    refuse_with(
        &client,
        json!({"between": [0_i32, 1_i32], "linedef": 2_i32}),
        "exactly one",
    )
    .await;
}

#[tokio::test]
async fn bad_indices_and_styles_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_missing_with(
        &client,
        json!({"between": [0_i32, 99_i32]}),
        "sector 99 does not exist",
    )
    .await;
    refuse_missing_with(
        &client,
        json!({"between": [99_i32, 0_i32]}),
        "sector 99 does not exist",
    )
    .await;
    refuse_missing_with(
        &client,
        json!({"linedef": 999_i32}),
        "linedef 999 does not exist",
    )
    .await;
    refuse_with(
        &client,
        json!({"between": [0_i32, 1_i32], "style": "sliding"}),
        "\"normal\", \"fast\", \"stay_open\", \"locked_red\", \"locked_blue\", \"locked_yellow\", \"remote\"",
    )
    .await;
    refuse_with(
        &client,
        json!({"between": [0_i32, 1_i32], "texture": "TOOLONGTEXTURE"}),
        "characters",
    )
    .await;
}

#[tokio::test]
async fn sectors_that_never_touch_share_no_door_wall() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_with(
        &client,
        json!({"between": [0_i32, 0_i32]}),
        "no two-sided boundary",
    )
    .await;
}

#[tokio::test]
async fn a_hexen_map_gets_the_script_door() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_door",
        json!({"map": "MAP04", "between": [0_i32, 1_i32]}),
    )
    .await;
    assert_eq!(result["style"], json!("normal"), "the default style");
    let lines = reported_lines(&client, "MAP04", &result, "lines").await;
    expect_door_raise_lines(&lines);
    let door = working_sector(&client, "MAP04", 2_usize).await;
    assert_eq!(
        door["ceiling_height"], door["floor_height"],
        "the door is closed"
    );
}

/// Checks the `Hexen`-family `Door_LockedRaise` rows for key `key`.
fn expect_locked_lines(rows: &[Value], key: i32) {
    assert_eq!(rows.len(), 2, "one use line per room");
    for row in rows {
        assert_eq!(row["special"], json!(13_i32), "Door_LockedRaise");
        assert_eq!(
            row["args"],
            json!([0_i32, 16_i32, 150_i32, key, 0_i32]),
            "the key class in args[3]"
        );
    }
}

#[tokio::test]
async fn a_hexen_locked_door_demands_its_coloured_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_door",
        json!({"map": "MAP04", "between": [0_i32, 1_i32], "style": "locked_blue"}),
    )
    .await;
    assert_eq!(result["style"], json!("locked_blue"), "echoed canonically");
    let lines = reported_lines(&client, "MAP04", &result, "lines").await;
    expect_locked_lines(&lines, 130_i32);
}

#[tokio::test]
async fn a_udmf_zdoom_stay_open_door_opens_once() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_door",
        json!({"map": "MAP03", "between": [0_i32, 1_i32], "style": "stay_open"}),
    )
    .await;
    assert_eq!(result["style"], json!("stay_open"), "echoed canonically");
    let lines = reported_lines(&client, "MAP03", &result, "lines").await;
    expect_door_open_lines(&lines);
}

#[tokio::test]
async fn a_door_undoes_as_one_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add normal door"]),
        "the whole door rolls back together"
    );
    let merged = call(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "merged", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        merged["sectors"]
            .as_array()
            .expect("sectors is an array")
            .len(),
        2_usize,
        "the layer was never written"
    );
}

#[tokio::test]
async fn a_themed_door_face_shows_from_the_room_it_opens() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let theme = call(
        &client,
        "set_texture_theme",
        json!({"overrides": {"door": "DOOR1"}}),
    )
    .await;
    assert_eq!(
        theme["roles"]["door"]["texture"],
        json!("DOOR1"),
        "the override stands for the role"
    );
    call(&client, "add_door", json!({"between": [0_i32, 1_i32]})).await;
    let report = call(
        &client,
        "render_view",
        json!({
            "map": "MAP01", "x": 64_i64, "y": 128_i64, "angle": 0_i64,
            "width": 160_u64, "height": 100_u64, "lighting": "fullbright",
        }),
    )
    .await;
    let image = rendered_image(&report).await;
    assert!(
        count_pixels(&image, [178_u8, 77, 178, 255]) > 0,
        "the closed door's DOOR1 face (the fixture patch's index 170, one fake-contrast \
         step darker) shows from room 0 across the doorway"
    );
}
