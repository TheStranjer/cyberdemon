//! The teleporter step's tests: the pad borders dressed to the tagged
//! destination and the two-way return leg, the fit and range refusals,
//! the bounded-room contract break, the row remap through a rebuild and
//! the door rows the rebuilds cannot lose or turn around.

use super::super::teleporters::remap_rows;
use super::super::{FeatureRow, apply_features};
use super::{
    TestRoles, back_sector, door, dress, front_sector, line_at, map_name, opening, refuse_features,
    two_rooms, wired,
};
use crate::error::PlanRef;
use crate::geom::Pt;
use crate::plan::compile::compile;
use crate::plan::model::{DoorStyle, Feature, KeyColor, Plan};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::TELEPORT_DESTINATION_THING;
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::Thing;

/// A `teleporter` feature from room `from` to room `to` on the
/// rooms' default centres.
pub(super) fn teleporter_feature(from: &str, to: &str, angle: i32, two_way: bool) -> Feature {
    Feature::Teleporter {
        from_room: from.to_owned(),
        to_room: to.to_owned(),
        pad: None,
        destination: None,
        angle,
        two_way,
    }
}

/// A `teleporter` feature with explicit square centres.
fn teleporter_at(from: &str, to: &str, pad: Option<Pt>, destination: Option<Pt>) -> Feature {
    Feature::Teleporter {
        from_room: from.to_owned(),
        to_room: to.to_owned(),
        pad,
        destination,
        angle: 0_i32,
        two_way: false,
    }
}

/// The two-room fixture joined by a plain opening, wearing
/// `features`.
fn teleport_fixture(features: Vec<Feature>) -> Plan {
    let mut plan = two_rooms(vec![opening("o1", "a", "b")]);
    plan.features = features;
    plan
}

/// The indices of every line carrying `special` and line tag `tag`.
fn lines_wearing(map: &Map, special: i32, tag: i32) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.special == special && line.tag == tag).then_some(index))
        .collect()
}

/// The teleport-destination things (type 14) of the map.
fn destination_things(map: &Map) -> Vec<&Thing> {
    map.things
        .iter()
        .filter(|thing| thing.kind == TELEPORT_DESTINATION_THING)
        .collect()
}

#[test]
fn the_plan_teleporter_dresses_its_pad_to_the_tagged_destination() {
    let plan = teleport_fixture(vec![teleporter_feature("a", "b", 45, false)]);
    let (compiled, rows) = wired(&plan);
    let row = rows.first().expect("one teleporter row");
    assert_eq!(row.kind, "teleporter");
    assert_eq!(row.room.as_deref(), Some("a"), "the pad's room");
    assert_eq!(row.sectors.len(), 2, "pad then destination");
    let pad = row.sectors.first().copied().expect("the pad sector");
    let dest = row.sectors.get(1).copied().expect("the destination sector");
    let tag = row.tag.expect("a fresh tag");
    assert_eq!(row.lines.len(), 4, "a border line on every side");
    for &line in &row.lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(def.special, 97_i32, "the WR teleport on line {line}");
        assert_eq!(def.tag, tag, "answering with the destination tag");
    }
    let dest_space = compiled.map.sectors.get(dest).expect("the destination");
    assert_eq!(
        dest_space.tag, tag,
        "the Doom search reads the tagged sector"
    );
    let pad_space = compiled.map.sectors.get(pad).expect("the pad");
    assert_eq!(pad_space.tag, 0_i32, "a one-way pad carries no tag");
    assert_eq!(
        pad_space.floor_flat, "FLOROLE",
        "the pad keeps the room's flat"
    );
    let things = destination_things(&compiled.map);
    assert_eq!(things.len(), 1, "one-way stands one thing");
    let thing = things.first().expect("the destination thing");
    assert_eq!(thing.angle, 45_i32, "the arrival facing");
    assert_eq!(
        point_in_sector(&compiled.map, i64::from(thing.x), i64::from(thing.y)),
        Some(dest),
        "the type-14 thing stands inside the destination square"
    );
}

#[test]
fn the_two_way_plan_teleporter_runs_the_return_leg() {
    let plan = teleport_fixture(vec![teleporter_feature("a", "b", 90, true)]);
    let (compiled, rows) = wired(&plan);
    let row = rows.first().expect("one teleporter row");
    let tag = row.tag.expect("the outward tag");
    let pad = row.sectors.first().copied().expect("the pad sector");
    assert_eq!(
        compiled.map.sectors.get(pad).expect("the pad").tag,
        tag.saturating_add(1),
        "the Doom pad answers with the return tag"
    );
    assert_eq!(
        lines_wearing(&compiled.map, 97_i32, tag).len(),
        row.lines.len(),
        "the outward leg dresses only the pad's borders"
    );
    assert_eq!(
        lines_wearing(&compiled.map, 97_i32, tag.saturating_add(1)).len(),
        4_usize,
        "the return leg dresses the destination's borders"
    );
    let things = destination_things(&compiled.map);
    assert_eq!(things.len(), 2, "a thing on each end");
    assert_eq!(
        things.iter().map(|thing| thing.angle).collect::<Vec<i32>>(),
        vec![90_i32, 270_i32],
        "the pair looks at each other"
    );
}

#[test]
fn a_teleporter_square_that_does_not_fit_names_its_room() {
    for (feature, needle, room) in [
        (
            teleporter_at("a", "b", Some((8_i64, 8_i64)), None),
            "the teleporter pad does not fit inside room 'a' with 8 units of clearance",
            "a",
        ),
        (
            teleporter_at("a", "b", None, Some((128_i64, 64_i64))),
            "the teleporter destination does not fit inside room 'b' with 8 units of clearance",
            "b",
        ),
    ] {
        let error = refuse_features(&teleport_fixture(vec![feature]));
        assert_eq!(
            error.plan_ref(),
            Some(PlanRef::room(room)),
            "the square's own room row is the element to move"
        );
        let message = error.to_string();
        assert!(message.contains(needle), "{needle} must be in: {message}");
    }
}

#[test]
fn an_explicit_teleporter_centre_lands_where_the_wire_put_it() {
    let plan = teleport_fixture(vec![teleporter_at("a", "b", None, Some((200_i64, 64_i64)))]);
    let (compiled, rows) = wired(&plan);
    let row = rows.first().expect("one teleporter row");
    let dest = row.sectors.get(1).copied().expect("the destination");
    let thing = destination_things(&compiled.map)
        .first()
        .expect("the destination thing")
        .to_owned();
    assert_eq!(
        (thing.x, thing.y),
        (200_i32, 64_i32),
        "the wire point stands"
    );
    assert_eq!(
        point_in_sector(&compiled.map, i64::from(thing.x), i64::from(thing.y)),
        Some(dest),
        "and its square is the reported destination"
    );
}

#[test]
fn a_teleporter_centre_past_the_map_range_is_refused_by_role() {
    let plan = teleport_fixture(vec![teleporter_at(
        "a",
        "b",
        Some((0x0001_0000_0000_i64, 0_i64)),
        None,
    )]);
    let message = refuse_features(&plan).to_string();
    assert!(
        message
            .contains("the teleporter pad point 4294967296, 0 is outside the map coordinate range"),
        "the refusal names the point and its role: {message}"
    );
}

#[test]
fn a_room_the_compiled_map_bounds_nowhere_breaks_internally() {
    let plan = teleport_fixture(vec![teleporter_feature("a", "b", 0, false)]);
    let mut compiled =
        compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
    compiled.room_sectors.insert("a".to_owned(), vec![99_usize]);
    let error =
        apply_features(&plan, &mut compiled, &dress()).expect_err("sector 99 bounds nothing");
    assert_eq!(error.code(), "internal", "a contract break stays internal");
    let message = error.to_string();
    assert!(
        message.contains("pad") && message.contains("'a'"),
        "the role and room are named: {message}"
    );
}

#[test]
fn remap_rows_follow_the_rebuild_and_drop_the_lost_lines() {
    let mut rows = vec![
        FeatureRow {
            kind: "door",
            room: None,
            sectors: vec![2_usize],
            lines: vec![0_usize, 1_usize, 2_usize],
            tag: Some(1_i32),
            line_ends: Vec::new(),
        },
        FeatureRow {
            kind: "lift",
            room: Some("b".to_owned()),
            sectors: vec![1_usize],
            lines: vec![3_usize],
            tag: Some(2_i32),
            line_ends: Vec::new(),
        },
    ];
    remap_rows(
        &mut rows,
        &[
            Some(5_usize),
            None,
            Some(6_usize),
            Some(7_usize),
            None,
            None,
            None,
            None,
        ],
    );
    assert_eq!(
        rows.first().expect("two rows").lines,
        vec![5_usize, 6_usize],
        "a painted-over line drops out"
    );
    assert_eq!(rows.get(1).expect("two rows").lines, vec![7_usize]);
    assert_eq!(
        rows.first().expect("two rows").sectors,
        vec![2_usize],
        "sectors never move"
    );
}

#[test]
fn a_later_teleporter_keeps_earlier_rows_naming_their_real_lines() {
    let mut plan = two_rooms(vec![door("d1", DoorStyle::Normal, (128_i64, 32_i64))]);
    plan.features = vec![teleporter_feature("a", "b", 0, false)];
    let (compiled, rows) = wired(&plan);
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(
        kinds,
        vec!["door", "teleporter"],
        "the door leads: {kinds:?}"
    );
    let door_row = rows.first().expect("two rows");
    assert_eq!(door_row.lines.len(), 2_usize, "the door keeps its lines");
    for &line in &door_row.lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(def.special, 1_i32, "line {line} is still the DR door line");
        assert_eq!(def.tag, door_row.tag.expect("the door tag"));
    }
    let teleporter_row = rows.get(1).expect("two rows");
    for &line in &teleporter_row.lines {
        assert_eq!(line_at(&compiled.map, line).special, 97_i32);
    }
}

#[test]
fn a_later_rebuild_cannot_turn_the_wired_doors_around() {
    let mut plan = two_rooms(vec![door(
        "d1",
        DoorStyle::Locked(KeyColor::Blue),
        (128_i64, 64_i64),
    )]);
    plan.wall_thickness = 0_i64;
    plan.features = vec![teleporter_feature("a", "b", 0, false)];
    let (compiled, rows) = wired(&plan);
    let door_row = rows.first().expect("the door row");
    let band = door_row.sectors.first().copied().expect("the band");
    for &line in &door_row.lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(def.special, 26_i32, "the lock survives on line {line}");
        assert_eq!(
            back_sector(&compiled.map, def),
            Some(band),
            "the rebuild cannot put the band in front of line {line}"
        );
        assert!(
            matches!(front_sector(&compiled.map, def), 0_usize | 1_usize),
            "the room faces line {line} from the front: {}",
            front_sector(&compiled.map, def)
        );
    }
}
