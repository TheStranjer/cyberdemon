//! The `features` rows over the wire: a `lift` row tagging its room's sector
//! and dressing the down-facing edges, a plan lift riding the shared wall of
//! the raised room, a `teleporter` row drawing the pad and destination squares
//! and standing its type-14 things in the written block, a `secret` room with a
//! `light_effect` ending lit and hidden with the Boom warning passed through, a
//! `switch` row mounting its panel on the chosen wall and firing the remote
//! door's own tag, both `exit` styles ending the level where the room stands, a
//! `walk_over` exit beside a plain door leaving that door wired and
//! warning about it, a switch on a plain door refused as not remote, and
//! rows naming an unknown
//! room or an `opens` id no connection carries refusing by plan path.

use serde_json::{Value, json};

use crate::plan::{
    Client, assert_line_actions, assert_walk_lines, expect_row, features_loaded, first_line,
    line_row, plan_for, refuse_with, row_line_indices, things_of_kind, working_lines,
    working_sector, working_things,
};
use crate::support::{call, call_err};

/// Assert every linedef indexed in `indices` is the hall-facing SR
/// ride in the theme's step band.
fn assert_ride_lines(lines: &[Value], indices: &[u64]) {
    assert_eq!(indices.len(), 2_usize, "the two shared-wall segments");
    for index in indices {
        let line = line_row(lines, *index);
        assert_eq!(
            line["special"],
            json!(62_i64),
            "the SR ride on line {index}"
        );
        assert_eq!(line["tag"], json!(1_i64), "answering with the platform tag");
        assert_eq!(
            line["front"]["sector"],
            json!(0_i64),
            "the front looks down at the hall"
        );
        assert_eq!(
            line["front"]["lower"],
            json!("STEP1"),
            "the theme's step band"
        );
    }
}

/// Lift and teleporter rows install on the compiled map and report
/// their rows after the connectors; every feature row still
/// reference-checks by plan path.
#[tokio::test]
async fn lift_and_teleporter_rows_install_and_refuse_by_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["features"] =
        json!([{"kind": "teleporter", "from_room": "a", "to_room": "b", "two_way": true}]);
    let result = call(&client, "build_map_from_plan", request).await;
    let rows = result["features"].as_array().expect("a feature list");
    assert_eq!(rows.len(), 2, "the door leads, the teleporter follows");
    assert_eq!(rows[0]["kind"], json!("door"), "the plain door wires first");
    assert_eq!(rows[0]["tag"], json!(1_u64), "its own tag");
    expect_teleporter_block(&client, rows.get(1).expect("the teleporter row")).await;
    let lines = working_lines(&client, "MAP05").await;
    assert_line_actions(&lines, &row_line_indices(&rows[0]), 1_i64, 1_i64);
    let things = working_things(&client, "MAP05").await;
    assert_eq!(
        things_of_kind(&things, 14_i64).len(),
        2_usize,
        "a type-14 thing on each square"
    );
    let mut ghost = plan_for("MAP06");
    ghost["features"] = json!([{"kind": "exit", "room": "ghost", "style": "walk_over"}]);
    refuse_with(&client, ghost, "features[0]").await;
    let mut orphan = plan_for("MAP06");
    orphan["features"] = json!([{"kind": "switch", "room": "a", "opens": "nope"}]);
    refuse_with(&client, orphan, "features[0]").await;
}

/// Assert the teleporter row and its written block: the pad room is
/// named, the sector list is pad then destination, the Doom
/// destination answers with the row's fresh tag and the two-way pad
/// with the return tag above it, and the row's lines carry the WR
/// teleport. (Door lines re-checked elsewhere prove the reported
/// indices survived the squares' rebuild.)
async fn expect_teleporter_block(client: &Client, row: &Value) {
    assert_eq!(row["kind"], json!("teleporter"));
    assert_eq!(row["room"], json!("a"), "the pad's room");
    assert_eq!(row["tag"], json!(2_u64), "a fresh tag after the door's");
    let sectors = row["sectors"].as_array().expect("two sectors");
    assert_eq!(sectors.len(), 2, "the pad then the destination");
    let pad = usize::try_from(
        sectors
            .first()
            .expect("the pad")
            .as_u64()
            .expect("an index"),
    )
    .expect("small");
    let dest = usize::try_from(
        sectors
            .get(1)
            .expect("the dest")
            .as_u64()
            .expect("an index"),
    )
    .expect("small");
    let dest_sector = working_sector(client, "MAP05", dest).await;
    assert_eq!(
        dest_sector["tag"],
        json!(2_i64),
        "the Doom destination answers with the outward tag"
    );
    let pad_sector = working_sector(client, "MAP05", pad).await;
    assert_eq!(
        pad_sector["tag"],
        json!(3_i64),
        "the two-way pad answers with the return tag"
    );
    let lines = working_lines(client, "MAP05").await;
    assert_line_actions(&lines, &row_line_indices(row), 97_i64, 2_i64);
}

/// The `lift_request` plan: a 64-unit-high ledge room over a hall,
/// touching at zero thickness, joined by an opening and lifted.
fn lift_request() -> Value {
    json!({
        "map": "MAP05",
        "wall_thickness": 0_i64,
        "rooms": [
            {"id": "hall", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "ledge", "rect": {"x": 0_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64},
             "floor": 64_i64}
        ],
        "connections": [
            {"id": "gap", "from": "ledge", "to": "hall", "kind": "opening"}
        ],
        "features": [{"kind": "lift", "room": "ledge", "style": "use_wall"}],
        "player_start": {"room": "hall"}
    })
}

/// The plan lift: the raised room's own sector becomes the tagged
/// platform, the shared wall turns front-down with the SR ride in
/// the theme's step band — and the same join as a door is refused.
#[tokio::test]
async fn a_plan_lift_rides_the_shared_wall_of_the_raised_room() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "build_map_from_plan", lift_request()).await;
    let rows = result["features"].as_array().expect("a feature list");
    assert_eq!(rows.len(), 1, "an opening is no feature row: {rows:?}");
    let row = rows.first().expect("the lift row");
    assert_eq!(row["kind"], json!("lift"));
    assert_eq!(
        row["room"],
        json!("ledge"),
        "the raised room is the platform"
    );
    assert_eq!(row["sectors"], json!([1_u64]), "the room's own sector");
    assert_eq!(row["tag"], json!(1_u64), "a fresh tag");
    assert_eq!(
        working_sector(&client, "MAP05", 1_usize).await["tag"],
        json!(1_i64),
        "the platform answers with it"
    );
    let lines = working_lines(&client, "MAP05").await;
    assert_ride_lines(&lines, &row_line_indices(row));
    let mut joined = lift_request();
    joined["map"] = json!("MAP06");
    joined["connections"][0]["kind"] = json!("door");
    refuse_with(&client, joined, "a lift needs openings to the lower rooms").await;
}

/// A `secret` room with a `light_effect` ends up lit and hidden in
/// the written block — the strobe type under the Boom secret bit —
/// with the vanilla-Doom warning passed through.
#[tokio::test]
async fn a_lit_secret_room_builds_lit_and_hidden() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "build_map_from_plan",
        json!({
            "map": "MAP05",
            "rooms": [{
                "id": "vault",
                "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
                "secret": true,
                "light_effect": "strobe_slow"
            }],
            "connections": [],
            "player_start": {"room": "vault"}
        }),
    )
    .await;
    let rows = result["features"].as_array().expect("a feature list");
    let kinds: Vec<&str> = rows
        .iter()
        .map(|row| row["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(kinds, vec!["light", "secret"], "lights run before secrets");
    assert_eq!(rows[0]["sectors"], json!([0_u64]), "the room's sector");
    assert_eq!(
        rows[0]["lines"],
        json!([]),
        "a painted special owns no line"
    );
    assert_eq!(rows[0]["tag"], json!(null), "and no tag");
    let sector = working_sector(&client, "MAP05", 0_usize).await;
    assert_eq!(
        sector["special"],
        json!(131_i64),
        "strobe 3 with the Boom bit 128"
    );
    assert_eq!(sector["light"], json!(160_i64), "the level is kept");
    let warnings = result["warnings"].as_array().expect("a warning list");
    assert!(
        warnings.iter().any(|note| note
            .as_str()
            .is_some_and(|text| text.contains("special 131"))),
        "the vanilla warning passes through: {warnings:?}"
    );
}

/// A remote door and its switch: the build reports the rows in
/// pipeline order and the written block carries the action, the
/// door's own tag and the theme's panel.
#[tokio::test]
async fn a_switch_fires_the_remote_door_it_opens() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["connections"][0]["style"] = json!("remote");
    request["features"] = json!([{"kind": "switch", "room": "a", "opens": "d1", "wall": "west"}]);
    let result = call(&client, "build_map_from_plan", request).await;
    let rows = result["features"].as_array().expect("a feature list");
    assert_eq!(
        rows.len(),
        2,
        "the door leads, the switch follows: {rows:?}"
    );
    assert_eq!(rows[0]["kind"], json!("door"), "the door is wired first");
    assert_eq!(rows[0]["tag"], json!(1_u64), "the band keeps its tag");
    assert_eq!(rows[1]["kind"], json!("switch"), "the switch mounts after");
    assert_eq!(rows[1]["room"], json!("a"), "in room a");
    assert_eq!(rows[1]["sectors"], json!([2_u64]), "firing the door band");
    assert_eq!(rows[1]["tag"], json!(1_u64), "with the door's own tag");
    let lines = working_lines(&client, "MAP05").await;
    let panel = line_row(&lines, first_line(&rows[1]));
    assert_eq!(panel["special"], json!(103_i64), "S1 opens the remote door");
    assert_eq!(panel["tag"], json!(1_i64), "on the door's own tag");
    assert_eq!(
        panel["front"]["middle"],
        json!("SW1STRTN"),
        "the theme's switch panel"
    );
    call(&client, "undo", json!({})).await;
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "one undo takes the wiring back");
}

/// Both exit styles land on the written block: the walk-over row
/// dresses the room's shared boundaries with `52` and the switch
/// row carries the secret level end in the theme's exit panel.
#[tokio::test]
async fn the_two_exit_styles_end_the_level_where_the_room_stands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["features"] = json!([
        {"kind": "exit", "room": "b", "style": "walk_over"},
        {"kind": "exit", "room": "b", "style": "switch", "secret": true, "wall": "east"}
    ]);
    let result = call(&client, "build_map_from_plan", request).await;
    let rows = result["features"].as_array().expect("a feature list");
    assert_eq!(rows[0]["kind"], json!("door"), "the plain door wires first");
    let walked = rows.get(1).expect("the walk-over row");
    let mounted = rows.get(2).expect("the switch row");
    assert_eq!(walked["kind"], json!("exit"), "exits come after the door");
    assert_eq!(walked["room"], json!("b"), "the room it stands in");
    assert_eq!(walked["sectors"], json!([1_u64]), "its own sector");
    assert_eq!(walked["tag"], json!(null), "an exit carries no tag");
    assert_eq!(mounted["kind"], json!("exit"));
    let walk_lines: Vec<u64> = walked["lines"]
        .as_array()
        .expect("a line list")
        .iter()
        .map(|slot| slot.as_u64().expect("an index"))
        .collect();
    assert!(!walk_lines.is_empty(), "room b shares its band walls");
    let lines = working_lines(&client, "MAP05").await;
    assert_walk_lines(&lines, &walk_lines);
    let secret = line_row(&lines, first_line(mounted));
    assert_eq!(secret["special"], json!(51_i64), "the S1 secret exit");
    assert_eq!(secret["tag"], json!(0_i64), "an exit acts on no tag");
    assert_eq!(
        secret["front"]["middle"],
        json!("SW1EXIT"),
        "the theme's exit panel"
    );
}

/// A `walk_over` exit in a room the plan also joins by a plain door: the
/// door's own boundary lines keep their action, the exit dresses the
/// opening's, the build warns about what it skipped, and the finished
/// map still plays through the door to the level end.
#[tokio::test]
async fn a_walk_over_exit_beside_a_door_keeps_the_door_and_plays() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["features"] = json!([{"kind": "exit", "room": "b", "style": "walk_over"}]);
    let result = call(&client, "build_map_from_plan", request).await;
    let rows = result["features"].as_array().expect("a feature list");
    let door = expect_row(rows, 0, "door", None);
    let exit = expect_row(rows, 1, "exit", Some("b"));
    let lines = working_lines(&client, "MAP05").await;
    let door_lines = row_line_indices(door);
    assert_line_actions(&lines, &door_lines, 1_i64, 1_i64);
    let exit_lines = row_line_indices(exit);
    assert!(!exit_lines.is_empty(), "the opening's lines end the level");
    for index in &exit_lines {
        assert!(
            !door_lines.contains(index),
            "line {index} is an exit and a door at once"
        );
    }
    assert_walk_lines(&lines, &exit_lines);
    let notes = result["warnings"].as_array().expect("a warning list");
    assert_eq!(notes.len(), 1, "the skip is the only note: {notes:?}");
    let note = notes
        .first()
        .expect("one note")
        .as_str()
        .expect("a text note");
    assert!(note.contains("walk_over exit left"), "{note}");
    assert!(note.contains("(special 1)"), "{note}");
    let plays = call(
        &client,
        "check_playability",
        json!({"map": "MAP05", "render": false}),
    )
    .await;
    assert_eq!(plays["ok"], json!(true), "the map finishes: {plays}");
    assert_eq!(
        plays["blockers"],
        json!([]),
        "the door the exit stepped over still opens: {plays}"
    );
}

/// A switch pointed at a non-remote door is refused, and nothing is
/// written.
#[tokio::test]
async fn a_switch_on_a_plain_door_is_refused_as_not_remote() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["features"] = json!([{"kind": "switch", "room": "a", "opens": "d1", "wall": "west"}]);
    let (code, message, _) = call_err(&client, "build_map_from_plan", request).await;
    assert_eq!(code, "invalid_argument", "a plain door is no target");
    assert!(
        message.contains("switch opens d1 which is not a remote door"),
        "the refusal words: {message}"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
}
