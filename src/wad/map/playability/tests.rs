//! Shared builders for the playability walk's tests: the flag-bit and key
//! constants, the hand-built `hall` of rooms with its wires and things, and
//! the compiled plan behind [`built`]. Every helper is a function of its
//! arguments and the fixtures in [`crate::wad::map::common::fixtures`], so
//! no topic can disturb what another one measures.
//!
//! `movement` checks the lines a player can take, `features` the lifts,
//! teleporters and exits the walk opens, `triggers` the activation kinds -
//! use, walk, shoot, push and monster-only - and the `Boom` generalized
//! rows the walk decodes, `plan_built` the features the plan engine wires
//! and `reports` everything the finished report says about a map.

mod features;
mod movement;
mod plan_built;
mod reports;
mod triggers;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use super::Playability;
use crate::plan::FeatureDress;
use crate::plan::compile::compile;
use crate::plan::emit::MapIdentity;
use crate::plan::features::{FeatureRow, apply_features};
use crate::plan::model::{Connection, ConnectionKind, Feature as PlanFeature, Plan, Room, Shape};
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{cell, hall, middle, name, slab};
use crate::wad::map::flags::{THING_HEXEN_SINGLE, THING_SKILL_BITS, TWO_SIDED};
use crate::wad::map::{Map, Thing};

/// The blue card key type.
pub(super) const BLUE_CARD: i32 = 5_i32;
/// The red skull key type.
pub(super) const RED_SKULL: i32 = 38_i32;
/// The teleport destination thing type.
pub(super) const TELEPORT_DEST: i32 = 14_i32;
/// The `Hexen` thing bits that put a thing in the single-player game.
pub(super) const HEXEN_PRESENCE: u32 = THING_SKILL_BITS | THING_HEXEN_SINGLE;

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
    let identity = MapIdentity {
        name: name("MAP01"),
        format,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    };
    let mut compiled = compile(plan, identity, &FlatRoles).expect("the test plans compile");
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
