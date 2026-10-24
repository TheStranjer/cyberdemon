//! The playable-map suite: one six-room plan (locked door, lift, teleporter,
//! remote gate with its switch, `walk_over` exit, lit and secret rooms, a named
//! keycard and the player start) building with an empty `warnings` list and
//! every feature reading back from the written block - specials
//! `26`/`62`/`52`/`97`, the platform tag, the strobed exit sector `3`, the
//! secret sector `9` and a type-14 thing standing in the ledge; dropping the
//! keycard, the exit row or the switch replaces the silence with exactly the
//! matching advisory warning. And a `UDMF` `zdoom` locked door carrying the
//! Hexen-family `Door_LockedRaise` (`13` with the key in `args[3]`,
//! playeruse).

use serde_json::{Value, json};

use crate::plan::{
    Client, assert_line_actions, assert_walk_lines, expect_row, features_loaded, line_row,
    plan_for, room_sector, row_line_indices, things_of_kind, working_lines, working_sector,
    working_things,
};
use crate::support::call;

/// The six-room playable plan: a `foyer` opens into `start`, the
/// id'd `blue_door` locks the `corridor` off, a `use_wall` lift
/// raises the corridor to the `ledge`, a teleporter runs corridor to
/// ledge, the remote `gate` (its wall `switch` in the ledge) locks
/// the strobed `exit` room, and an ordinary `cell_door` hides the
/// `secret` room off the corridor. The blue card stands in `start`
/// beside the player start: every lock has its key, the map has its
/// exit and the remote has its switch, so the playability checks
/// have nothing to say.
fn six_room_plan(map: &str) -> Value {
    json!({
        "map": map,
        "format": "doom",
        "wall_thickness": 0_i64,
        "rooms": [
            {"id": "start", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "corridor", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "secret", "rect": {"x": 256_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
             "secret": true},
            {"id": "ledge", "rect": {"x": 128_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64},
             "floor": 24_i64},
            {"id": "exit", "rect": {"x": 128_i64, "y": 256_i64, "w": 128_i64, "h": 128_i64},
             "floor": 24_i64, "light_effect": "strobe_slow"},
            {"id": "foyer", "rect": {"x": -128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
        ],
        "connections": [
            {"id": "blue_door", "from": "start", "to": "corridor", "kind": "door",
             "style": "locked_blue"},
            {"id": "cell_door", "from": "corridor", "to": "secret", "kind": "door",
             "style": "normal"},
            {"id": "gap", "from": "corridor", "to": "ledge", "kind": "opening"},
            {"id": "gate", "from": "ledge", "to": "exit", "kind": "door",
             "style": "remote"},
            {"id": "home", "from": "start", "to": "foyer", "kind": "opening"}
        ],
        "features": [
            {"kind": "lift", "room": "ledge", "style": "use_wall", "low_floor": -40_i64},
            {"kind": "teleporter", "from_room": "corridor", "to_room": "ledge"},
            {"kind": "switch", "room": "ledge", "opens": "gate", "wall": "east"},
            {"kind": "exit", "room": "exit", "style": "walk_over"}
        ],
        "things": [{"name": "Blue Keycard", "room": "start"}],
        "player_start": {"room": "start"}
    })
}

/// Assert the remote `gate` writes no action of its own: every
/// reported band line is either still bare or carries the exit
/// step's `W1` end (the lines facing the exit room are also that
/// room's boundary), and none carries a tag or args.
fn assert_remote_gate_lines(lines: &[Value], indices: &[u64]) {
    for index in indices {
        let line = line_row(lines, *index);
        assert!(
            matches!(line["special"].as_i64(), Some(0_i64 | 52_i64)),
            "a remote line carries no door action of its own: {line}"
        );
        assert_eq!(line["tag"], json!(0_i64), "and no tag on line {index}");
    }
}

/// Assert the blue-key door row: tag `1`, the band sector answering
/// it, and every door line a special-`26` use line with the band
/// behind and one of the two rooms' sectors in front.
async fn assert_blue_door(client: &Client, map: &str, result: &Value, row: &Value) {
    assert_eq!(row["tag"], json!(1_u64), "the first door tags first");
    let band = row["sectors"][0].as_u64().expect("one band");
    let space = working_sector(client, map, usize::try_from(band).expect("small")).await;
    assert_eq!(space["tag"], json!(1_i64), "the band answers its tag");
    let start = room_sector(result, "start");
    let corridor = room_sector(result, "corridor");
    let lines = working_lines(client, map).await;
    let indices = row_line_indices(row);
    assert!(!indices.is_empty(), "the band meets its rooms");
    for index in indices {
        let line = line_row(&lines, index);
        assert_eq!(
            line["special"],
            json!(26_i64),
            "DR blue lock on line {index}"
        );
        assert_eq!(line["tag"], json!(1_i64), "on the band's tag");
        assert_eq!(line["back"]["sector"], json!(band), "the band sits behind");
        let front = line["front"]["sector"].as_u64().expect("a front sector");
        assert!(
            front == start || front == corridor,
            "the front faces its room on line {index}: {front}"
        );
    }
}

/// Assert the lift row: the ledge's own sector tagged, and its two
/// activation lines riding with special `62`, front down onto the
/// corridor.
async fn assert_lift(client: &Client, map: &str, result: &Value, row: &Value) {
    let tag = row["tag"].as_u64().expect("a fresh tag");
    let platform = row["sectors"][0].as_u64().expect("one platform");
    assert_eq!(
        platform,
        room_sector(result, "ledge"),
        "the room is the platform"
    );
    let space = working_sector(client, map, usize::try_from(platform).expect("small")).await;
    assert_eq!(
        space["tag"].as_u64().expect("a tag"),
        tag,
        "the platform answers with the row's tag"
    );
    let lines = working_lines(client, map).await;
    let indices = row_line_indices(row);
    assert_eq!(indices.len(), 2, "the two shared-wall segments");
    for index in indices {
        let line = line_row(&lines, index);
        assert_eq!(
            line["special"],
            json!(62_i64),
            "the SR ride on line {index}"
        );
        assert_eq!(
            line["tag"].as_u64().expect("a tag"),
            tag,
            "line {index} answers with the platform tag"
        );
        assert_eq!(
            line["front"]["sector"],
            json!(room_sector(result, "corridor")),
            "the front looks down at the corridor"
        );
    }
}

/// Assert the teleporter row: pad borders with the `WR` teleport on
/// the destination's fresh tag, and a type-14 thing standing in the
/// ledge.
async fn assert_teleporter(client: &Client, map: &str, row: &Value) {
    assert_eq!(row["room"], json!("corridor"), "the pad's room");
    let tag = row["tag"].as_u64().expect("a fresh tag");
    let sectors = row["sectors"].as_array().expect("two sectors");
    assert_eq!(sectors.len(), 2, "the pad then the destination");
    let dest = usize::try_from(sectors[1].as_u64().expect("an index")).expect("small");
    let space = working_sector(client, map, dest).await;
    assert_eq!(
        space["tag"].as_u64().expect("a tag"),
        tag,
        "the destination answers"
    );
    let lines = working_lines(client, map).await;
    for index in row_line_indices(row) {
        let line = line_row(&lines, index);
        assert_eq!(
            line["special"],
            json!(97_i64),
            "the WR teleport on line {index}"
        );
        assert_eq!(
            line["tag"].as_u64().expect("a tag"),
            tag,
            "to the tagged square"
        );
    }
    let pads = things_of_kind(&working_things(client, map).await, 14_i64)
        .into_iter()
        .filter(|thing| {
            thing["x"]
                .as_i64()
                .is_some_and(|x| x > 128_i64 && x < 256_i64)
                && thing["y"]
                    .as_i64()
                    .is_some_and(|y| y > 128_i64 && y < 256_i64)
        })
        .count();
    assert_eq!(pads, 1_usize, "the destination thing stands in the ledge");
}

/// Assert the `walk_over` exit row: special `52` on every reported
/// boundary line, and the exit room's sector strobing under vanilla
/// type `3` at its planned light level.
async fn assert_exit(client: &Client, map: &str, result: &Value, row: &Value) {
    assert_eq!(
        row["room"],
        json!("exit"),
        "the level ends in the exit room"
    );
    assert_eq!(row["tag"], json!(null), "an exit carries no tag");
    let indices = row_line_indices(row);
    assert!(!indices.is_empty(), "the room shares its gate boundary");
    let lines = working_lines(client, map).await;
    assert_walk_lines(&lines, &indices);
    let exit_room = usize::try_from(room_sector(result, "exit")).expect("small");
    let space = working_sector(client, map, exit_room).await;
    assert_eq!(
        space["special"],
        json!(3_i64),
        "strobe_slow lights the exit room"
    );
    assert_eq!(space["light"], json!(160_i64), "the level is kept");
}

/// Assert the secret row: the secret room's sectors carry vanilla
/// type `9`, with no lines and no tag.
async fn assert_secret(client: &Client, map: &str, result: &Value, row: &Value) {
    assert_eq!(row["room"], json!("secret"), "the hidden room");
    assert_eq!(row["lines"], json!([]), "a painted special owns no line");
    assert_eq!(row["tag"], json!(null), "and no tag");
    let secret = usize::try_from(room_sector(result, "secret")).expect("small");
    assert_eq!(
        row["sectors"],
        json!([secret]),
        "the row names the room's own sector"
    );
    let space = working_sector(client, map, secret).await;
    assert_eq!(space["special"], json!(9_i64), "the vanilla secret type");
}

/// The whole playable map at once: six rooms, locked door, lift,
/// teleporter, remote gate and switch, walk-over exit with a strobe,
/// a lit-free secret room and the keycard that opens it all. Every
/// feature reads back from the written block and the playability
/// checks have nothing to complain about.
#[tokio::test]
async fn the_six_room_plan_builds_a_whole_playable_map() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "build_map_from_plan", six_room_plan("MAP05")).await;
    assert_eq!(
        result["warnings"],
        json!([]),
        "key in hand, exit set, remote switched: {:?}",
        result["warnings"]
    );
    assert_eq!(
        result["things_placed"],
        json!(2_u64),
        "the card and the start"
    );
    let rows = result["features"].as_array().expect("a feature list");
    let kinds: Vec<&str> = rows
        .iter()
        .map(|row| row["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "door",
            "door",
            "door",
            "switch",
            "lift",
            "teleporter",
            "exit",
            "light",
            "secret"
        ],
        "the pipeline order over the whole plan"
    );
    expect_six_room_rows(&client, "MAP05", &result, rows).await;
    let report = call(&client, "validate_map", json!({"map": "MAP05"})).await;
    assert_eq!(
        report["counts"]["error"],
        json!(0_u64),
        "the playable map validates error-free: {:?}",
        report["issues"]
    );
}

/// Assert the nine `features` rows of the playable build against
/// the written block, in pipeline order: the three wired doors,
/// the switch on the gate's tag, the ride, the teleport, the exit,
/// and the painted light and secret.
async fn expect_six_room_rows(client: &Client, map: &str, result: &Value, rows: &[Value]) {
    assert_blue_door(client, map, result, expect_row(rows, 0, "door", None)).await;
    let lines = working_lines(client, map).await;
    assert_line_actions(
        &lines,
        &row_line_indices(expect_row(rows, 1, "door", None)),
        1_i64,
        2_i64,
    );
    let gate = expect_row(rows, 2, "door", None);
    assert_eq!(gate["tag"], json!(3_u64), "the remote takes the third tag");
    assert_remote_gate_lines(&lines, &row_line_indices(gate));
    let panel = expect_row(rows, 3, "switch", Some("ledge"));
    assert_eq!(
        panel["tag"],
        json!(3_u64),
        "the switch fires the gate's tag"
    );
    assert_line_actions(&lines, &row_line_indices(panel), 103_i64, 3_i64);
    assert_lift(
        client,
        map,
        result,
        expect_row(rows, 4, "lift", Some("ledge")),
    )
    .await;
    assert_teleporter(
        client,
        map,
        expect_row(rows, 5, "teleporter", Some("corridor")),
    )
    .await;
    assert_exit(
        client,
        map,
        result,
        expect_row(rows, 6, "exit", Some("exit")),
    )
    .await;
    let light = expect_row(rows, 7, "light", Some("exit"));
    assert_eq!(
        light["sectors"],
        json!([room_sector(result, "exit")]),
        "the strobe row painted the exit room"
    );
    assert_secret(
        client,
        map,
        result,
        expect_row(rows, 8, "secret", Some("secret")),
    )
    .await;
}

#[tokio::test]
async fn dropping_the_keycard_warns_only_the_blue_door() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = six_room_plan("MAP05");
    request["things"] = json!([]);
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["warnings"],
        json!(["blue door at blue_door but no blue key in the plan"]),
        "the one gap the plan leaves"
    );
}

#[tokio::test]
async fn dropping_the_exit_feature_warns_only_the_missing_exit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = six_room_plan("MAP05");
    request["features"]
        .as_array_mut()
        .expect("a feature list")
        .remove(3);
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["warnings"],
        json!(["the map has no exit"]),
        "the level would never end"
    );
}

#[tokio::test]
async fn dropping_the_switch_warns_only_the_unswitched_gate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = six_room_plan("MAP05");
    request["features"]
        .as_array_mut()
        .expect("a feature list")
        .remove(2);
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["warnings"],
        json!([
            "remote door at gate is opened by no switch: add a switch feature whose \
             opens is gate"
        ]),
        "the locked gate names its way out"
    );
}

/// A `UDMF` map in the `zdoom` namespace is the Hexen flag family:
/// the blue-locked door lines carry `Door_LockedRaise` (`13`) with
/// speed, wait and the blue key in the args and the `playeruse`
/// activation bit, and no line tag.
#[tokio::test]
async fn a_udmf_zdoom_locked_door_carries_the_hexen_locked_raise() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["connections"][0]["style"] = json!("locked_blue");
    request["format"] = json!("udmf");
    request["namespace"] = json!("zdoom");
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(result["format"], json!("udmf"), "the text layout");
    let rows = result["features"].as_array().expect("a feature list");
    let row = rows.first().expect("the door row");
    assert_eq!(row["kind"], json!("door"), "the locked door wires");
    let lines = working_lines(&client, "MAP05").await;
    let indices = row_line_indices(row);
    assert_eq!(indices.len(), 2, "the band meets each room once");
    expect_hexen_locked_lines(&lines, &indices);
}

/// Assert every reported door line is the `zdoom` `Door_LockedRaise`.
fn expect_hexen_locked_lines(lines: &[Value], indices: &[u64]) {
    for index in indices {
        expect_hexen_locked_line(line_row(lines, *index), *index);
    }
}

/// One `UDMF` `zdoom` locked-door line: `Door_LockedRaise` (`13`)
/// with the back-sector speed/wait/blue-key args, no line tag and
/// the `playeruse` activation bit.
fn expect_hexen_locked_line(line: &Value, index: u64) {
    assert_eq!(
        line["special"],
        json!(13_i64),
        "Door_LockedRaise on line {index}"
    );
    assert_eq!(
        line["args"],
        json!([0_i64, 16_i64, 150_i64, 130_i64, 0_i64]),
        "no sector arg, speed 16, wait 150, the blue key"
    );
    assert_eq!(
        line["tag"],
        json!(0_i64),
        "the Hexen action carries no line tag"
    );
    let flags = line["flags"].as_u64().expect("numeric flags");
    assert_eq!(flags & 0x400_u64, 0x400_u64, "playeruse on line {index}");
}
