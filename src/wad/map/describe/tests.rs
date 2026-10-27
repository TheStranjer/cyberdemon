//! Shared builders for the description engine's tests: the two embedded
//! definition sets, the hand-drawn fixtures the topics measure — the
//! square, concave and split-wall rooms, the nested exit squares and the
//! room halls the neighbour and overview readings walk — and the lookups
//! every topic asks of a described map.
//!
//! [`plans`] adds the plan-built fixtures, [`facts`] the per-sector row,
//! [`neighbours`] the boundary readings, [`roles`] the role names,
//! [`overview`] the whole-map summary and [`index`] the one stored
//! [`SectorIndex`](crate::wad::map::geometry::SectorIndex) every row
//! answers from, checked against the free-function wrappers.

mod facts;
mod index;
mod neighbours;
mod overview;
mod plans;
mod roles;

use alloc::vec;
use alloc::vec::Vec;

use crate::defs::{Definitions, definitions_for};
use crate::plan::features::FeatureRow;
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::common::fixtures::{
    cell, hall, name, one_sided, room, side, slab, solid_line, vertex,
};
use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::{Linedef, Map, Sector, Thing};

use super::{Describe, Neighbour, SectorFacts};

/// The embedded `DOOM` definitions (the sector table's home).
pub(super) fn doom_defs() -> Definitions {
    definitions_for(Game::Doom).expect("the embedded Doom table is valid")
}

/// The embedded `DOOM II` definitions (the thing table's superset).
pub(super) fn doom2_defs() -> Definitions {
    definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
}

/// The 256x256 square room: `WALL1` on all four one-sided walls,
/// floor `0` under a 128-unit ceiling, light 160, tag 0, sector 0.
pub(super) fn square_map() -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(256, 0),
        vertex(256, 256),
        vertex(0, 256),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
        solid_line(3, 0, 3),
    ];
    map.sidedefs = (0_usize..4_usize)
        .map(|_| side("-", "-", "WALL1", 0_usize))
        .collect();
    map.sectors = vec![room(0, 128, 160, 0)];
    map
}

/// The concave room: a 512x256 foot with a 256x256 tower on its west
/// end, six `WALL1` sides around sector 0.
pub(super) fn elbow_map() -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(512, 0),
        vertex(512, 256),
        vertex(256, 256),
        vertex(256, 512),
        vertex(0, 512),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
        solid_line(3, 4, 3),
        solid_line(4, 5, 4),
        solid_line(5, 0, 5),
    ];
    map.sidedefs = (0_usize..6_usize)
        .map(|_| side("-", "-", "WALL1", 0_usize))
        .collect();
    map.sectors = vec![room(0, 128, 160, 0)];
    map
}

/// The square room with its east side split in two and its north side
/// in three: six `WALL1` sides and one `SWITCH1` middle around
/// sector 0.
pub(super) fn switch_map() -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(256, 0),
        vertex(256, 128),
        vertex(256, 256),
        vertex(192, 256),
        vertex(64, 256),
        vertex(0, 256),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
        solid_line(3, 4, 3),
        solid_line(4, 5, 4),
        solid_line(5, 6, 5),
        solid_line(6, 0, 6),
    ];
    map.sidedefs = vec![
        side("-", "-", "WALL1", 0),
        side("-", "-", "WALL1", 0),
        side("-", "-", "WALL1", 0),
        side("-", "-", "WALL1", 0),
        side("-", "-", "SWITCH1", 0),
        side("-", "-", "WALL1", 0),
        side("-", "-", "WALL1", 0),
    ];
    map.sectors = vec![room(0, 128, 160, 0)];
    map
}

/// Retune sector 0 of `map` in place.
pub(super) fn retune(map: &mut Map, row: Sector) {
    map.sectors = vec![row];
}

/// Stands `row` among `map`'s things.
pub(super) fn stand(map: &mut Map, row: Thing) {
    map.things.push(row);
}

/// Two equal rooms, `A` at `x 0..256` and `B` at `x 256..512`, whose
/// shared wall is split at `y = 128` into linedefs `6` and `7`, both
/// two-sided and facing `A` in front, wearing `lower_flags` and
/// `upper_flags`.
pub(super) fn split_divider_map(lower_flags: u32, upper_flags: u32) -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(256, 0),
        vertex(512, 0),
        vertex(512, 256),
        vertex(256, 256),
        vertex(0, 256),
        vertex(256, 128),
    ];
    let mut sidedefs = Vec::new();
    let mut linedefs = vec![
        one_sided(0, 1, &mut sidedefs, 0),
        one_sided(1, 2, &mut sidedefs, 1),
        one_sided(2, 3, &mut sidedefs, 1),
        one_sided(3, 4, &mut sidedefs, 1),
        one_sided(4, 5, &mut sidedefs, 0),
        one_sided(5, 0, &mut sidedefs, 0),
    ];
    for (v1, v2, flags) in [(1_usize, 6_usize, lower_flags), (6, 4, upper_flags)] {
        let front = sidedefs.len();
        sidedefs.push(side("-", "-", "-", 0));
        sidedefs.push(side("-", "-", "-", 1));
        linedefs.push(Linedef {
            v1,
            v2,
            flags,
            front: Some(front),
            back: Some(front.saturating_add(1)),
            ..Default::default()
        });
    }
    map.linedefs = linedefs;
    map.sidedefs = sidedefs;
    map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 0)];
    map
}

/// The nested squares - an outer `0..1000` sector and an inner
/// `400..600` sector of one-sided walls - plus a floating line inside
/// the inner square whose front opens into the outer sector and which
/// carries exit `special` with `flags`: the midpoint rule's test bench.
pub(super) fn nested_exit_map(format: MapFormat, special: i32, flags: u32) -> Map {
    let mut map = Map::new_empty(name("MAP01"), format);
    map.vertices = vec![
        vertex(0, 0),
        vertex(1000, 0),
        vertex(1000, 1000),
        vertex(0, 1000),
        vertex(400, 400),
        vertex(600, 400),
        vertex(600, 600),
        vertex(400, 600),
        vertex(450, 500),
        vertex(550, 500),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
        solid_line(3, 0, 3),
        solid_line(4, 5, 4),
        solid_line(5, 6, 5),
        solid_line(6, 7, 6),
        solid_line(7, 4, 7),
        Linedef {
            v1: 8,
            v2: 9,
            flags,
            special,
            front: Some(8),
            ..Default::default()
        },
    ];
    map.sidedefs = (0_usize..9_usize)
        .map(|index| {
            let inner = (4_usize..8_usize).contains(&index);
            side("-", "-", "WALL1", usize::from(inner))
        })
        .collect();
    map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 0)];
    map
}

/// The feature row `apply_features` reported for kind `kind`.
pub(super) fn row(rows: &[FeatureRow], kind: &str) -> FeatureRow {
    rows.iter()
        .find(|entry| entry.kind == kind)
        .cloned()
        .unwrap_or_else(|| panic!("a {kind} feature row"))
}

/// The sector standing at the middle of a 256-unit-wide plan room
/// column `column`.
pub(super) fn room_sector(map: &Map, column: usize) -> usize {
    let x = i32::try_from(column.saturating_mul(256)).unwrap_or(i32::MAX);
    point_in_sector(map, i64::from(x).saturating_add(128), 128_i64).expect("the room resolves")
}

/// Whether `facts` names the `role`.
pub(super) fn has_role(facts: &SectorFacts, role: &str) -> bool {
    facts.roles.iter().any(|found| found.as_str() == role)
}

/// The neighbour of `facts` across from sector `sector`.
pub(super) fn neighbour_across(facts: &SectorFacts, sector: usize) -> &Neighbour {
    facts
        .neighbours
        .iter()
        .find(|entry| entry.sector == sector)
        .unwrap_or_else(|| panic!("sector {sector} is a neighbour: {:?}", facts.neighbours))
}

/// The facts of `map`'s sector `index` through one shared [`Describe`].
pub(super) fn described(map: &Map, index: usize) -> SectorFacts {
    Describe::new(map, &doom_defs(), SKY_FLAT)
        .sector(index)
        .unwrap_or_else(|| panic!("sector {index} exists"))
}
