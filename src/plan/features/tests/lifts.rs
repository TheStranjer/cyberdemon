//! The lift step's tests: the tagged room and the turned, dressed
//! down-facing edges in both families, the small-drop note, the
//! join-rule refusals and the multi-sector guard, and the `step`
//! dressing fallback shared with the teleporter wall.

use super::super::lifts::{lift_join_rule, lift_side};
use super::super::teleporters::teleport_wall;
use super::super::{CompiledPlan, FeatureDress, FeatureRow, apply_features};
use super::{
    TestRoles, back_sector, dress, front_sector, line_at, rect_room, refuse_features, wired,
    wired_dressed,
};
use crate::error::PlanRef;
use crate::plan::compile::compile;
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Feature, Plan};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::LiftStyle;

/// A named connection of `kind` between rooms `from` and `to`,
/// centred on their shared boundary.
pub(super) fn join(id: &str, from: &str, to: &str, kind: ConnectionKind) -> Connection {
    Connection {
        id: Some(id.to_owned()),
        from: from.to_owned(),
        to: to.to_owned(),
        kind,
        at: None,
        width: 64_i64,
    }
}

/// A `lift` feature in room `room`: a use-wall ride, repeating when
/// `repeatable`.
pub(super) fn lift_feature(room: &str, repeatable: bool) -> Feature {
    Feature::Lift {
        room: room.to_owned(),
        low_floor: None,
        style: LiftStyle::UseWall,
        repeatable,
    }
}

/// Two stacked 128 x 128 rooms touching at `y = 128` with no wall
/// between them, joined by a `kind` connection, the upper room `b`
/// standing 64 units above `a` and carrying a default use-wall lift.
fn lift_fixture(kind: ConnectionKind) -> Plan {
    let mut upper = rect_room("b", (0, 128), (128, 128));
    upper.floor = 64_i32;
    Plan {
        rooms: vec![rect_room("a", (0, 0), (128, 128)), upper],
        connections: vec![join("c1", "b", "a", kind)],
        features: vec![lift_feature("b", true)],
        wall_thickness: 0_i64,
    }
}

/// The `lower` band worn by `line`'s front sidedef.
fn front_lower(map: &Map, line: usize) -> String {
    let side = map
        .linedefs
        .get(line)
        .expect("test index")
        .front
        .expect("a front side");
    map.sidedefs.get(side).expect("test row").lower.clone()
}

/// Assert the one lift row of `compiled`: room `b`'s sector tagged,
/// both shared-wall segments turned front-down onto room `a` and
/// carrying `special` with the row's tag.
#[track_caller]
fn assert_doom_lift(compiled: &CompiledPlan, row: &FeatureRow, special: i32) {
    assert_eq!(row.kind, "lift", "the lift row");
    assert_eq!(row.room.as_deref(), Some("b"), "the platform's room");
    let platform = row.sectors.first().copied().expect("one platform");
    assert_eq!(
        row.sectors,
        compiled.room_sectors.get("b").expect("room b").clone(),
        "the row names the room's own sector"
    );
    let tag = row.tag.expect("a fresh tag");
    let space = compiled.map.sectors.get(platform).expect("the platform");
    assert_eq!(space.tag, tag, "the platform answers with the tag");
    assert_eq!(
        space.floor_flat, "FLOROLE",
        "the ride keeps the room's flat"
    );
    assert_eq!(row.lines.len(), 2, "the two segments of the shared wall");
    let lower = compiled
        .room_sectors
        .get("a")
        .expect("room a")
        .first()
        .expect("one sector");
    for &line in &row.lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(def.special, special, "the lift action on line {line}");
        assert_eq!(def.tag, tag, "line {line} answers with the platform tag");
        assert_eq!(back_sector(&compiled.map, def), Some(platform));
        assert_eq!(
            front_sector(&compiled.map, def),
            *lower,
            "the front looks down"
        );
        assert_eq!(
            front_lower(&compiled.map, line),
            "STEPROLE",
            "the side wears step"
        );
    }
}

#[test]
fn a_plan_lift_tags_its_room_and_turns_the_down_facing_edges() {
    let (compiled, rows) = wired(&lift_fixture(ConnectionKind::Opening));
    assert_eq!(rows.len(), 1, "an opening wires no door");
    assert_doom_lift(&compiled, rows.first().expect("one row"), 62_i32);
}

#[test]
fn the_once_only_doom_ride_takes_the_s1_special() {
    let mut plan = lift_fixture(ConnectionKind::Opening);
    plan.features = vec![lift_feature("b", false)];
    let (compiled, rows) = wired(&plan);
    assert_doom_lift(&compiled, rows.first().expect("one row"), 21_i32);
}

#[test]
fn a_hexen_plan_lift_runs_the_scripted_lip_plat() {
    let (compiled, rows) = wired_dressed(
        &lift_fixture(ConnectionKind::Opening),
        MapFormat::Hexen,
        &dress(),
    );
    let row = rows.first().expect("one lift row");
    let tag = row.tag.expect("a fresh tag");
    let platform = row.sectors.first().copied().expect("one platform");
    let space = compiled.map.sectors.get(platform).expect("the platform");
    assert_eq!(space.tag, tag, "the sector keeps the tag the args carry");
    assert_eq!(row.lines.len(), 2, "both shared-wall segments");
    for &line in &row.lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(def.special, 206_i32, "Plat_DownWaitUpStayLip");
        assert_eq!(
            def.args,
            [tag, 32_i32, 105_i32, 0_i32, 0_i32],
            "tag, speed, wait"
        );
        assert_eq!(def.tag, 0_i32, "a Hexen line carries no tag");
        assert_eq!(
            def.flags & 0x600_u32,
            0x600_u32,
            "playeruse and repeatspecial"
        );
    }
}

#[test]
fn a_short_plan_lift_notes_that_stairs_will_do() {
    let mut plan = lift_fixture(ConnectionKind::Opening);
    if let Some(upper) = plan.rooms.get_mut(1) {
        upper.floor = 32_i32;
    }
    let (compiled, rows) = wired(&plan);
    assert_eq!(
        rows.first().expect("one lift row").lines.len(),
        2_usize,
        "both shared-wall segments ride"
    );
    assert!(
        compiled
            .warnings
            .iter()
            .any(|note| note.contains("a lift is not needed")),
        "the small-drop note passes through: {:?}",
        compiled.warnings
    );
}

#[test]
fn a_lift_refuses_a_lower_room_joined_by_a_door() {
    let plan = lift_fixture(ConnectionKind::Door {
        style: DoorStyle::Normal,
    });
    let error = refuse_features(&plan);
    assert_eq!(
        error.plan_ref(),
        Some(PlanRef::room("b")),
        "the refusal cites the lift's own room row"
    );
    let message = error.to_string();
    assert!(
        message.contains("a lift needs openings to the lower rooms"),
        "the refusal words: {message}"
    );
    assert!(
        message.contains("lift room 'b'")
            && message.contains("lower room 'a'")
            && message.contains("'c1'")
            && message.contains("kind door"),
        "room, neighbour, connection and kind are named: {message}"
    );
}

#[test]
fn a_door_to_a_higher_room_passes_the_lift_rule() {
    let mut plan = lift_fixture(ConnectionKind::Opening);
    let mut tower = rect_room("c", (0, 256), (128, 128));
    tower.floor = 128_i32;
    tower.ceiling = 256_i32;
    plan.rooms.push(tower);
    plan.connections.push(join(
        "c2",
        "b",
        "c",
        ConnectionKind::Door {
            style: DoorStyle::Normal,
        },
    ));
    let (_, rows) = wired(&plan);
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(
        kinds,
        vec!["door", "lift"],
        "the door to a higher room joins fine: {kinds:?}"
    );
}

#[test]
fn the_lift_rule_ignores_a_room_the_plan_does_not_hold() {
    let plan = lift_fixture(ConnectionKind::Opening);
    lift_join_rule(&plan, "ghost").expect("no row, no rule");
}

#[test]
fn a_room_owning_several_sectors_refuses_the_plan_lift() {
    let plan = lift_fixture(ConnectionKind::Opening);
    for (count, owned) in [(2_usize, vec![1_usize, 2_usize]), (0_usize, Vec::new())] {
        let mut compiled =
            compile(&plan, super::identity(MapFormat::Doom), &TestRoles).expect("it compiles");
        compiled.room_sectors.insert("b".to_owned(), owned);
        let error = apply_features(&plan, &mut compiled, &dress())
            .expect_err("the platform must be one sector");
        assert_eq!(error.code(), "invalid_argument", "the guard refusal");
        assert_eq!(
            error.plan_ref(),
            Some(PlanRef::room("b")),
            "the lift row's room is the element named"
        );
        let message = error.to_string();
        assert!(
            message.contains("'b'") && message.contains(&format!("owns {count} sectors")),
            "the room and the count are named: {message}"
        );
    }
}

#[test]
fn an_unnamed_step_falls_the_lift_side_back_to_the_wall() {
    let themed = FeatureDress {
        step: "-".to_owned(),
        ..dress()
    };
    assert_eq!(
        lift_side(&themed),
        "WALROLE",
        "the fill pass's name rides on"
    );
    let bare = FeatureDress::default();
    assert_eq!(
        lift_side(&bare),
        "-",
        "no theme at all leaves the band bare"
    );
    assert_eq!(teleport_wall(&themed), "WALROLE");
    assert_eq!(teleport_wall(&bare), "-", "an unnamed wall stays bare");
}
