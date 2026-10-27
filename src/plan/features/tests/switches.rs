//! The switch and exit steps' tests: the wall-mounted panel's action,
//! tag reuse and dressing, the (secret) level ends, the refusal wording
//! for plain-door, untagged-remote and painted-over rooms, the fit and
//! hint of a panel that cannot mount, the reported line endpoints and
//! the walk-over boundary — its plain lines dressed and a door's own
//! left wired. Wall *choosing* lives in [`wall`].
//!
//! [`wall`]: wall

use super::super::FeatureRun;
use super::super::checks::remote_door;
use super::super::{FeatureDress, FeatureRow, report_line_ends};
use super::{
    BAND, TAG_ONE, door, dress, line_at, opening, opening_at, rect_room, refuse_features,
    two_rooms, wired, wired_dressed,
};
use crate::error::PlanRef;
use crate::geom::Pt;
use crate::plan::model::{
    Connection, ConnectionKind, DoorStyle, ExitStyle, Feature, Plan, Room, Shape, Side,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::model::Linedef;

mod points;
mod wall;

/// A `switch` feature row.
pub(super) fn switch_feature(
    room: &str,
    opens: &str,
    wall: Option<Side>,
    repeatable: bool,
) -> Feature {
    Feature::Switch {
        room: room.to_owned(),
        opens: opens.to_owned(),
        wall,
        repeatable,
        at: None,
    }
}

/// An `exit` feature row.
pub(super) fn exit_feature(
    room: &str,
    style: ExitStyle,
    secret: bool,
    wall: Option<Side>,
) -> Feature {
    Feature::Exit {
        room: room.to_owned(),
        style,
        secret,
        wall,
        at: None,
    }
}

/// A `switch` feature row standing at the plan point `at`.
fn switch_at(room: &str, opens: &str, wall: Option<Side>, at: Pt) -> Feature {
    Feature::Switch {
        room: room.to_owned(),
        opens: opens.to_owned(),
        wall,
        repeatable: false,
        at: Some(at),
    }
}

/// A `switch`-style `exit` feature row standing at the plan point `at`.
fn exit_at(room: &str, wall: Option<Side>, at: Option<Pt>) -> Feature {
    Feature::Exit {
        room: room.to_owned(),
        style: ExitStyle::Switch,
        secret: false,
        wall,
        at,
    }
}

/// The two-room remote-door plan carrying `features`.
pub(super) fn door_features(features: Vec<Feature>) -> Plan {
    let mut plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
    plan.features = features;
    plan
}

/// Two stacked 128 x 128 rooms joined by a remote door off-centre:
/// the lower room's north wall comes out as two equal 36-unit
/// segments either side of the band.
pub(super) fn stacked_doors() -> Plan {
    let band = Connection {
        id: Some("d1".to_owned()),
        from: "a".to_owned(),
        to: "b".to_owned(),
        kind: ConnectionKind::Door {
            style: DoorStyle::Remote,
        },
        at: Some((64_i64, 128_i64)),
        width: 48_i64,
    };
    Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (0, 128), (128, 128)),
        ],
        connections: vec![band],
        features: Vec::new(),
        wall_thickness: 16_i64,
    }
}

/// Three 128 x 128 rooms in a row at zero thickness, joined by
/// openings: the middle room owns no one-sided east or west wall.
pub(super) fn three_in_row() -> Plan {
    Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (128, 0), (128, 128)),
            rect_room("c", (256, 0), (128, 128)),
        ],
        connections: vec![opening("o1", "a", "b"), opening("o2", "b", "c")],
        features: Vec::new(),
        wall_thickness: 0_i64,
    }
}

/// The two endpoint coordinates of a linedef.
pub(super) type Corners = ((i32, i32), (i32, i32));

/// The `(x, y)` endpoints of linedef `index`.
pub(super) fn ends(map: &Map, index: usize) -> Corners {
    let line = map.linedefs.get(index).expect("test index");
    let first = map.vertices.get(line.v1).expect("test vertex");
    let second = map.vertices.get(line.v2).expect("test vertex");
    ((first.x, first.y), (second.x, second.y))
}

/// The front side's `middle` texture of linedef `line`.
fn front_middle(map: &Map, line: usize) -> String {
    let side = map
        .linedefs
        .get(line)
        .expect("test index")
        .front
        .expect("mounted lines keep a front");
    map.sidedefs.get(side).expect("test row").middle.clone()
}

/// The `index`th line's special and tag.
fn action_of_line(map: &Map, index: usize) -> (i32, i32) {
    let line = map.linedefs.get(index).expect("test index");
    (line.special, line.tag)
}

#[test]
fn a_switch_on_a_plain_door_is_refused() {
    let mut plan = two_rooms(vec![door("d1", DoorStyle::Normal, (128_i64, 32_i64))]);
    plan.features = vec![switch_feature("a", "d1", Some(Side::West), false)];
    let error = refuse_features(&plan);
    assert_eq!(
        error.plan_ref(),
        Some(PlanRef::feature("d1")),
        "the switch row carries the reference it cannot fire"
    );
    assert!(
        error
            .to_string()
            .contains("switch opens d1 which is not a remote door"),
        "the refusal words: {error}"
    );
}

#[test]
fn a_remote_door_the_wiring_never_tagged_is_no_switch_target() {
    let plan = door_features(Vec::new());
    let error = remote_door(&plan, &FeatureRun::default(), "d1")
        .expect_err("a remote row with no wired band");
    assert!(
        error
            .to_string()
            .contains("switch opens d1 which is not a remote door"),
        "unwired reads as not-a-target: {error}"
    );
}

#[test]
fn a_feature_in_a_painted_over_room_names_it() {
    let plan = Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (0, 0), (128, 128)),
        ],
        connections: Vec::new(),
        features: vec![exit_feature("a", ExitStyle::WalkOver, false, None)],
        wall_thickness: 16_i64,
    };
    plan.validate().expect("the plan validates");
    let error = refuse_features(&plan);
    assert_eq!(
        error.plan_ref(),
        Some(PlanRef::room("a")),
        "the vanished room is the element to move"
    );
    let message = error.to_string();
    assert!(
        message.contains("room 'a'") && message.contains("painted over"),
        "the refusal names the vanished room: {message}"
    );
}

#[test]
fn the_switch_centers_on_the_wall_and_fires_the_doors_own_tag() {
    let plan = door_features(vec![switch_feature("a", "d1", Some(Side::West), false)]);
    let (compiled, rows) = wired(&plan);
    let map = &compiled.map;
    let row = rows
        .iter()
        .find(|r| r.kind == "switch")
        .expect("a switch row");
    assert_eq!(row.room.as_deref(), Some("a"), "the room it stands in");
    assert_eq!(row.sectors, vec![BAND], "the door band it fires");
    assert_eq!(
        row.tag,
        Some(TAG_ONE),
        "the door's tag answers, no second mint"
    );
    assert_eq!(row.lines.len(), 1, "one switch line");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(
        action_of_line(map, line),
        (103_i32, TAG_ONE),
        "S1 opens the tagged door"
    );
    assert_eq!(
        front_middle(map, line),
        "SWTROLE",
        "the theme's switch panel"
    );
    let ((x1, y1), (x2, y2)) = ends(map, line);
    assert_eq!((x1, x2), (0_i32, 0_i32), "still on the west wall");
    assert_eq!(
        (y1.min(y2), y1.max(y2)),
        (48_i32, 80_i32),
        "the 32-unit panel centred on the wall's midpoint"
    );
    assert_eq!(
        map.sectors
            .iter()
            .filter(|space| space.tag != 0_i32)
            .count(),
        1,
        "only the band carries a tag: the switch reused the door's"
    );
}

#[test]
fn a_repeatable_switch_takes_the_sr_special() {
    let plan = door_features(vec![switch_feature("a", "d1", None, true)]);
    let (compiled, rows) = wired(&plan);
    let row = rows
        .iter()
        .find(|r| r.kind == "switch")
        .expect("a switch row");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(
        action_of_line(&compiled.map, line),
        (61_i32, TAG_ONE),
        "SR keeps working after use"
    );
}

#[test]
fn an_unnamed_switch_panel_keeps_the_wall_texture() {
    let themed = FeatureDress {
        switch: "-".to_owned(),
        ..dress()
    };
    let plan = door_features(vec![switch_feature("a", "d1", Some(Side::West), false)]);
    let (compiled, rows) = wired_dressed(&plan, MapFormat::Doom, &themed);
    let row = rows
        .iter()
        .find(|r| r.kind == "switch")
        .expect("a switch row");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(
        front_middle(&compiled.map, line),
        "WALROLE",
        "the \"-\" answer keeps the wall's own middle"
    );
    assert_eq!(
        action_of_line(&compiled.map, line).0,
        103_i32,
        "the action still lands"
    );
}

#[test]
fn the_exit_switch_carries_the_level_end() {
    let plan = door_features(vec![exit_feature(
        "b",
        ExitStyle::Switch,
        false,
        Some(Side::East),
    )]);
    let (compiled, rows) = wired(&plan);
    let map = &compiled.map;
    let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    assert_eq!(row.room.as_deref(), Some("b"));
    assert_eq!(row.sectors, vec![1_usize], "the room it stands in");
    assert_eq!(row.tag, None, "an exit acts on no tag");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(action_of_line(map, line), (11_i32, 0_i32), "the S1 exit");
    assert_eq!(front_middle(map, line), "EXTPANE", "the theme's exit panel");
}

#[test]
fn a_secret_exit_switch_takes_the_s1_secret() {
    let plan = door_features(vec![exit_feature("b", ExitStyle::Switch, true, None)]);
    let (compiled, rows) = wired(&plan);
    let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(
        action_of_line(&compiled.map, line).0,
        51_i32,
        "the S1 secret exit"
    );
}

/// A C-shaped 128 x 128 room whose west edge is bitten between
/// `y = 48` and `y = 80`: two equal 48-unit west walls, a lower one
/// running `y = 0..48` and an upper one `y = 80..128`.
pub(super) fn c_room() -> Plan {
    Plan {
        rooms: vec![Room {
            id: "c".to_owned(),
            shape: Shape::Polygon {
                points: vec![
                    (0_i64, 0_i64),
                    (128, 0),
                    (128, 128),
                    (0, 128),
                    (0, 80),
                    (48, 80),
                    (48, 48),
                    (0, 48),
                ],
            },
            floor: 0_i32,
            ceiling: 128_i32,
            light: 160_i32,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }],
        connections: Vec::new(),
        features: Vec::new(),
        wall_thickness: 16_i64,
    }
}

/// The run's last pass pairs every reported line with the endpoints the
/// finished map holds; an index it does not hold reports nothing.
#[test]
fn the_reporting_pass_pairs_each_line_with_its_endpoints() {
    let (compiled, _) = wired(&c_room());
    let mut rows = vec![FeatureRow {
        kind: "exit",
        room: Some("c".to_owned()),
        sectors: vec![0_usize],
        lines: vec![0_usize, 9_999_usize],
        tag: None,
        line_ends: Vec::new(),
    }];
    report_line_ends(&mut rows, &compiled.map);
    let row = rows.first().expect("one row");
    assert_eq!(row.line_ends.len(), 1, "the dangling index reports nothing");
    let end = row.line_ends.first().expect("one pair");
    let ((x1, y1), (x2, y2)) = ends(&compiled.map, 0_usize);
    assert_eq!(
        (end.index, end.x1, end.y1, end.x2, end.y2),
        (0_usize, x1, y1, x2, y2),
        "the pair is the map's own line"
    );
}

#[test]
fn a_walk_over_exit_dresses_the_rooms_two_sided_boundaries() {
    let mut plan = three_in_row();
    plan.features = vec![exit_feature("b", ExitStyle::WalkOver, false, None)];
    let (compiled, rows) = wired(&plan);
    let map = &compiled.map;
    let expected: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| boundary_of(map, line, 1_usize).then_some(index))
        .collect();
    assert!(!expected.is_empty(), "the fixture shares walls");
    let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    assert_eq!(row.lines, expected, "every two-sided boundary of the room");
    assert_eq!(row.sectors, vec![1_usize]);
    assert_eq!(row.tag, None);
    for &index in &expected {
        assert_eq!(
            action_of_line(map, index),
            (52_i32, 0_i32),
            "W1 exits keep the bare tags"
        );
    }
}

/// Whether `line` is two-sided with sector `sector` on one side.
fn boundary_of(map: &Map, line: &Linedef, sector: usize) -> bool {
    let side = |slot: Option<usize>| {
        slot.and_then(|index| map.sidedefs.get(index))
            .map(|row| row.sector)
    };
    let (Some(front), Some(back)) = (side(line.front), side(line.back)) else {
        return false;
    };
    front == sector || back == sector
}

/// The two-room plan a `style` door and a plain opening both join: the
/// door band low on the shared wall, the opening high on it.
fn door_and_gap(style: DoorStyle) -> Plan {
    two_rooms(vec![
        door("d1", style, (128_i64, 32_i64)),
        opening_at("o1", (128_i64, 96_i64)),
    ])
}

#[test]
fn a_walk_over_exit_next_to_a_door_leaves_the_door_wired() {
    let mut plan = door_and_gap(DoorStyle::Normal);
    plan.features = vec![exit_feature("b", ExitStyle::WalkOver, false, None)];
    let (compiled, rows) = wired(&plan);
    let map = &compiled.map;
    let door_row = rows.iter().find(|r| r.kind == "door").expect("a door row");
    let exit_row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    assert!(
        !exit_row.lines.is_empty(),
        "the opening's boundary ends the level"
    );
    assert!(
        !door_row.lines.is_empty(),
        "the door wires both rooms' sides"
    );
    for &line in &door_row.lines {
        assert_eq!(
            line_at(map, line).special,
            1_i32,
            "line {line} still opens the door"
        );
        assert!(
            !exit_row.lines.contains(&line),
            "no door line ends the level"
        );
    }
    for &line in &exit_row.lines {
        assert_eq!(line_at(map, line).special, 52_i32, "the W1 on line {line}");
    }
    let notes = &compiled.warnings;
    assert_eq!(
        notes.len(),
        1,
        "the skipped door line is the only note: {notes:?}"
    );
    assert!(
        notes
            .first()
            .expect("one note")
            .contains("walk_over exit left"),
        "the note says what was skipped: {notes:?}"
    );
    assert!(
        notes.first().expect("one note").contains("special 1"),
        "and names the action it kept: {notes:?}"
    );
}

#[test]
fn a_walk_over_exit_behind_a_lone_door_names_the_door_and_the_way_out() {
    let mut plan = door_and_gap(DoorStyle::Normal);
    plan.connections
        .retain(|band| band.id.as_deref() != Some("o1"));
    plan.features = vec![exit_feature("b", ExitStyle::WalkOver, false, None)];
    let message = refuse_features(&plan).to_string();
    assert!(
        message.contains("has no plain two-sided boundary to walk over"),
        "{message}"
    );
    assert!(
        message.contains("special 1"),
        "the door is named: {message}"
    );
    assert!(message.contains("switch"), "and the way out: {message}");
}
