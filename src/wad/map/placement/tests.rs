//! Shared builders for the placement tests: the square-room and L-room
//! fixtures, the `DOOM` definitions the radius cases read, the one-area
//! [`distribute`] spec and the result readers.
//!
//! [`clearance`] measures: what fits, the exact distance test, the grid
//! walkers and whole [`distribute`] runs on the square, L, box and circle
//! areas. [`split_facing`] divides and faces: the proportional split, the
//! snap, the bearings and the seeded walk.

mod clearance;
mod split_facing;

use alloc::vec::Vec;

use super::{Area, DistributeSpec, Distribution, Facing};
use crate::defs::{Definitions, definitions_for};
use crate::geom::Pt;
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::common::fixtures::{side, solid_line, vertex};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// A valid parsed test name.
pub(super) fn map_name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A map holding `squares` as separate one-sector rooms, each
/// `(min, max)`-cornered box bounded by four blocking lines.
pub(super) fn square_rooms(squares: &[(i32, i32)]) -> Map {
    let mut map = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
    for square in squares {
        add_square(&mut map, square.0, square.1);
    }
    map
}

/// Append one square sector from (`min`, `min`) to (`max`, `max`).
fn add_square(map: &mut Map, min: i32, max: i32) {
    let sector = map.sectors.len();
    let base = map.vertices.len();
    map.sectors.push(Sector::default());
    let corners = [(min, min), (max, min), (max, max), (min, max)];
    for corner in corners {
        map.vertices.push(Vertex {
            x: corner.0,
            y: corner.1,
            extra: alloc::collections::BTreeMap::new(),
        });
    }
    let sides: Vec<usize> = (0_usize..4_usize)
        .map(|_| {
            let side = map.sidedefs.len();
            map.sidedefs.push(Sidedef {
                sector,
                ..Default::default()
            });
            side
        })
        .collect();
    let edges = [(0_usize, 1_usize), (1, 2), (2, 3), (3, 0)];
    for edge in edges {
        map.linedefs.push(Linedef {
            v1: base.saturating_add(edge.0),
            v2: base.saturating_add(edge.1),
            flags: 1_u32,
            front: sides.get(edge.0).copied(),
            ..Default::default()
        });
    }
}

/// The L-shaped single-sector room: the `0..256` box with its
/// upper-right quadrant removed, bounded by six one-sided lines.
pub(super) fn l_room() -> Map {
    let mut map = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
    let ring: [(i32, i32); 6] = [
        (0, 256),
        (128, 256),
        (128, 128),
        (256, 128),
        (256, 0),
        (0, 0),
    ];
    for point in &ring {
        map.vertices.push(vertex(point.0, point.1));
    }
    map.sectors.push(Sector::default());
    map.sidedefs.push(side("-", "-", "WALL1", 0));
    let indices: Vec<usize> = (0_usize..ring.len()).collect();
    let back = indices.iter().skip(1).chain(indices.iter().take(1));
    for (&from, &to) in indices.iter().zip(back) {
        map.linedefs.push(solid_line(from, to, 0_usize));
    }
    map
}

/// The session's `DOOM` definitions for the radius tests.
pub(super) fn doom_definitions() -> Definitions {
    definitions_for(Game::Doom).expect("the embedded tables parse")
}

/// A one-area spec filling `area` with `need` copies at `clearance`.
pub(super) fn spec(area: Area, need: usize, clearance: i64) -> DistributeSpec {
    DistributeSpec {
        areas: vec![area],
        needs: vec![need],
        clearance,
        facing: Facing::Fixed(0_i32),
        unknown_radius: 20_i64,
    }
}

/// The points a [`distribute`] result landed on.
pub(super) fn points(out: &Distribution) -> Vec<Pt> {
    out.placed.iter().map(|placed| placed.at).collect()
}

/// The angles a [`distribute`] result carries, in placement order.
pub(super) fn angles(out: &Distribution) -> Vec<i32> {
    out.placed.iter().map(|placed| placed.angle).collect()
}
