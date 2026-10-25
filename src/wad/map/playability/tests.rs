//! Shared builders for the playability walk's tests: the flag-bit and key
//! constants, the hand-built `hall` of rooms with its wires and things, and
//! the compiled plan behind [`built`]. Every helper is a function of its
//! arguments and the fixtures in [`crate::wad::map::common::fixtures`], so
//! no topic can disturb what another one measures.
//!
//! `movement` checks the lines a player can take, `features` the lifts,
//! teleporters and exits the walk opens, `plan_built` the features the plan
//! engine wires and `reports` everything the finished report says about a
//! map.

mod features;
mod movement;
mod plan_built;
mod reports;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::Playability;
use crate::plan::FeatureDress;
use crate::plan::compile::compile;
use crate::plan::features::{FeatureRow, apply_features};
use crate::plan::model::{Connection, ConnectionKind, Feature as PlanFeature, Plan, Room, Shape};
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{name, side, spawn, vertex};
use crate::wad::map::flags::{BLOCKING, THING_HEXEN_SINGLE, THING_SKILL_BITS, TWO_SIDED};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing};

/// The blue card key type.
pub(super) const BLUE_CARD: i32 = 5_i32;
/// The red skull key type.
pub(super) const RED_SKULL: i32 = 38_i32;
/// The teleport destination thing type.
pub(super) const TELEPORT_DEST: i32 = 14_i32;
/// The `Hexen` thing bits that put a thing in the single-player game.
pub(super) const HEXEN_PRESENCE: u32 = THING_SKILL_BITS | THING_HEXEN_SINGLE;

/// A hand-built room: `floor` under 128 units of ceiling.
pub(super) fn cell(floor: i32) -> (i32, i32) {
    (floor, floor.saturating_add(128))
}

/// A hand-built slab: a sector with no headroom at all, which is what a
/// shut door sector looks like to the walk.
pub(super) fn slab(floor: i32) -> (i32, i32) {
    (floor, floor)
}

/// The `x` of hand-built room column `column`.
fn edge(column: usize) -> i32 {
    i32::try_from(column).map_or(i32::MAX, |index| index.saturating_mul(256))
}

/// The middle of hand-built room `room`, where a thing certainly stands.
pub(super) fn middle(room: usize) -> (i32, i32) {
    (edge(room).saturating_add(128), 128)
}

/// A blocking one-sided wall from `v1` to `v2` wearing a fresh sidedef
/// that faces `sector`.
fn one_sided(v1: usize, v2: usize, sidedefs: &mut Vec<Sidedef>, sector: usize) -> Linedef {
    sidedefs.push(side("-", "-", "WALL1", sector));
    Linedef {
        v1,
        v2,
        flags: BLOCKING,
        front: Some(sidedefs.len().saturating_sub(1)),
        ..Default::default()
    }
}

/// A row of 256-unit rooms: `specs` is one `(floor, ceiling)` per room,
/// room `i` spanning `x = 256i` to `x = 256(i + 1)` and owning sector
/// `i`. Linedef `i` is the two-sided divider between rooms `i` and
/// `i + 1`, its front facing the lower-numbered room; the walls that
/// close each room follow the dividers. A player 1 start stands in the
/// middle of room 0.
pub(super) fn hall(specs: &[(i32, i32)]) -> Map {
    let count = specs.len();
    let bottom = |column: usize| column;
    let top = |column: usize| count.saturating_add(1).saturating_add(column);
    let mut vertices = Vec::new();
    for column in 0..=count {
        vertices.push(vertex(edge(column), 0));
    }
    for column in 0..=count {
        vertices.push(vertex(edge(column), 256));
    }
    let mut linedefs = Vec::new();
    let mut sidedefs = Vec::new();
    for room in 0..count.saturating_sub(1) {
        let next = room.saturating_add(1);
        sidedefs.push(side("-", "-", "-", room));
        sidedefs.push(side("-", "-", "-", next));
        linedefs.push(Linedef {
            v1: top(next),
            v2: bottom(next),
            flags: TWO_SIDED,
            front: Some(room.saturating_mul(2)),
            back: Some(room.saturating_mul(2).saturating_add(1)),
            ..Default::default()
        });
    }
    for room in 0..count {
        let next = room.saturating_add(1);
        linedefs.push(one_sided(top(room), top(next), &mut sidedefs, room));
        linedefs.push(one_sided(bottom(next), bottom(room), &mut sidedefs, room));
    }
    linedefs.push(one_sided(bottom(0), top(0), &mut sidedefs, 0));
    let last = count.saturating_sub(1);
    linedefs.push(one_sided(top(count), bottom(count), &mut sidedefs, last));
    Map {
        name: name("MAP01"),
        format: MapFormat::Doom,
        udmf_namespace: None,
        things: vec![spawn(1, 128, 128, 0)],
        vertices,
        linedefs,
        sidedefs,
        sectors: specs
            .iter()
            .map(|spec| Sector {
                floor_height: spec.0,
                ceiling_height: spec.1,
                floor_flat: "FLOOR1".to_owned(),
                ceiling_flat: "FLOOR1".to_owned(),
                light: 160,
                ..Default::default()
            })
            .collect(),
        other: Vec::new(),
    }
}

/// Stands a thing of `kind` at `at` with presence bits `flags` and
/// reports its index.
pub(super) fn stand(map: &mut Map, kind: i32, at: (i32, i32), flags: u32) -> usize {
    map.things.push(Thing {
        kind,
        x: at.0,
        y: at.1,
        angle: 0,
        flags,
        ..Default::default()
    });
    map.things.len().saturating_sub(1)
}

/// Sets the `(special, tag, flags)` triple on linedef `line` of `map`.
pub(super) fn wire(map: &mut Map, line: usize, action: (i32, i32, u32)) {
    if let Some(def) = map.linedefs.get_mut(line) {
        def.special = action.0;
        def.tag = action.1;
        def.flags = action.2;
    }
}

/// Clears the line special off each of `lines`, taking a built feature's
/// trigger away for the negative half of a test.
pub(super) fn clear_specials(map: &mut Map, lines: &[usize]) {
    let targets: BTreeSet<usize> = lines.iter().copied().collect();
    map.linedefs
        .iter_mut()
        .enumerate()
        .filter(|entry| targets.contains(&entry.0))
        .for_each(|entry| entry.1.special = 0_i32);
}

/// Retunes the hand-built map's player start to the `Hexen` presence
/// bits, which ask for the single-player mode bit as well as the skill.
pub(super) fn hexen_start(map: &mut Map) {
    if let Some(thing) = map.things.first_mut() {
        thing.flags = HEXEN_PRESENCE;
    }
}

/// The sector set of `list`, for the assertions below.
pub(super) fn sectors(list: &[usize]) -> BTreeSet<usize> {
    list.iter().copied().collect()
}

/// The warnings of `found` that mention `text`.
pub(super) fn notes_about(found: &Playability, text: &str) -> Vec<String> {
    found
        .warnings
        .iter()
        .filter(|warning| warning.contains(text))
        .cloned()
        .collect()
}

/// The door `add_door` builds between two touching rooms: the slab band
/// sits behind both room-facing lines, each carrying the `DR` red action,
/// and the room behind the far one stands thirty-two units up.
pub(super) fn paired_red_door() -> Map {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(32_i32)]);
    let far = map.linedefs.get_mut(1).expect("the hall's far divider");
    let (front, back) = (far.front, far.back);
    far.front = back;
    far.back = front;
    wire(&mut map, 0, (28_i32, 0_i32, TWO_SIDED));
    wire(&mut map, 1, (28_i32, 0_i32, TWO_SIDED));
    stand(&mut map, RED_SKULL, middle(0), 7);
    map
}

/// A theme answering every dressing role with a fixed name.
struct FlatRoles;

impl TextureRoles for FlatRoles {
    fn texture(&self, role: TextureRole) -> Option<&str> {
        Some(match role {
            TextureRole::Floor => "FLATROL",
            TextureRole::Ceiling => "CEIROL",
            TextureRole::Wall => "WALROL",
            TextureRole::Door => "DOORROL",
            TextureRole::DoorFloor => "DFLAROL",
            TextureRole::DoorTrack => "DTRROL",
            TextureRole::WindowSill => "WSIROL",
            TextureRole::Step => "STEPRO",
        })
    }
}

/// A rectangular plan room, 256 units tall, spanning `span` on `x` and
/// standing at `heights`' `(floor, ceiling)`.
pub(super) fn room(id: &str, span: (i64, i64), heights: (i32, i32)) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Rect {
            x: span.0,
            y: 0,
            w: span.1,
            h: 256,
        },
        floor: heights.0,
        ceiling: heights.1,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A connection of `kind` between the rooms of `pair`, `width` clear.
pub(super) fn link(id: &str, pair: (&str, &str), kind: ConnectionKind, width: i64) -> Connection {
    Connection {
        id: Some(id.to_owned()),
        from: pair.0.to_owned(),
        to: pair.1.to_owned(),
        kind,
        at: None,
        width,
    }
}

/// The plan every [`built`] test wraps: `rooms` joined by `links`.
pub(super) fn plan(rooms: Vec<Room>, links: Vec<Connection>, features: Vec<PlanFeature>) -> Plan {
    Plan {
        rooms,
        connections: links,
        features,
        wall_thickness: 16_i64,
    }
}

/// A compiled plan in `format` with its features wired.
pub(super) fn built(plan: &Plan, format: MapFormat) -> (Map, Vec<FeatureRow>) {
    let mut compiled =
        compile(plan, name("MAP01"), format, &FlatRoles).expect("the test plans compile");
    let dress = FeatureDress {
        door: "DOORROL".to_owned(),
        track: "DTRROL".to_owned(),
        track_red: "REDTRAK".to_owned(),
        track_blue: "BLUETRAK".to_owned(),
        track_yellow: "YELTRAK".to_owned(),
        switch: "SWITROL".to_owned(),
        exit_switch: "EXITROL".to_owned(),
        step: "STEPRO".to_owned(),
        wall: "WALROL".to_owned(),
    };
    let rows = apply_features(plan, &mut compiled, &dress).expect("the test features install");
    (compiled.map, rows)
}

/// The sector of the plan's one door band, read from the feature rows.
pub(super) fn door_band(rows: &[FeatureRow]) -> usize {
    *rows
        .iter()
        .find(|row| row.kind == "door")
        .and_then(|row| row.sectors.first())
        .expect("the door's band sector")
}
