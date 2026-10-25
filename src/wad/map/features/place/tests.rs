//! Shared builders for the door placement geometry's tests: the refusal
//! reader, the point, line and sector makers, the shared-wall, L-run,
//! wide-wall and recess fixtures the calls measure against, the sampled
//! [`DoorWall`] and the shoelace area the polygons are checked with.
//!
//! [`boundary`] selects and projects: [`longest_boundary`],
//! [`nearest_boundary`], [`project_onto_line`], [`door_segment`],
//! [`door_wall`] and [`endpoints`]. [`depth_strip`] draws: the
//! [`outward_depth`] raycast with [`away_side`], the [`door_rect`] and
//! [`void_strip`] polygons and their refusals.
//!
//! [`away_side`]: crate::wad::map::features::place::depth::away_side

mod boundary;
mod depth_strip;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{side, vertex};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::ops::Point;

use super::DoorWall;

/// Whether `error` mentions `needle`, whatever code it carries: shape
/// refusals are `InvalidArgument`, missing-element refusals the shared
/// guard's `NotFound`.
pub(super) fn refuses(error: &CyberError, needle: &str) -> bool {
    error.message().contains(needle)
}

/// A point at `x`/`y`.
pub(super) const fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

/// A line from vertex `v1` to `v2` with the given sides.
pub(super) fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4_u32,
        front,
        back,
        ..Default::default()
    }
}

/// `count` plain sectors.
fn sectors(count: usize) -> Vec<Sector> {
    vec![Sector::default(); count]
}

/// Two rooms sharing the vertical line `(128, 0)` to `(128, 256)`:
/// sector 0 on its front (left) side, sector 1 behind.
pub(super) fn shared_line_map() -> Map {
    Map {
        format: MapFormat::Doom,
        vertices: vec![vertex(128, 0), vertex(128, 256)],
        linedefs: vec![line(0, 1, Some(0), Some(1))],
        sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
        sectors: sectors(2),
        ..Map::default()
    }
}

/// The same rooms sharing an L: `(0, 0)`-`(128, 0)` (linedef 0) and
/// `(128, 0)`-`(128, 128)` (linedef 1).
pub(super) fn shared_l_map() -> Map {
    Map {
        format: MapFormat::Doom,
        vertices: vec![vertex(0, 0), vertex(128, 0), vertex(128, 128)],
        linedefs: vec![line(0, 1, Some(0), Some(1)), line(1, 2, Some(0), Some(1))],
        sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
        sectors: sectors(2),
        ..Map::default()
    }
}

/// A horizontal wall `(0, 0)` to `(128, 0)` between sectors 0 and 1.
pub(super) fn wide_wall_map() -> Map {
    Map {
        format: MapFormat::Doom,
        vertices: vec![vertex(0, 0), vertex(128, 0)],
        linedefs: vec![line(0, 1, Some(0), Some(1))],
        sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
        sectors: sectors(2),
        ..Map::default()
    }
}

/// The recess fixture: room sector 0 north of the east-running
/// boundary `(0, 128)`-`(256, 128)` (its back side), hall sector 1
/// south of it (its front side), and a free far row at `y = 96`
/// (vertices 2 and 3) for tests to wall across.
pub(super) fn recess_map() -> Map {
    Map {
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 128),
            vertex(256, 128),
            vertex(0, 96),
            vertex(256, 96),
        ],
        linedefs: vec![line(0, 1, Some(0), Some(1))],
        sidedefs: vec![side("-", "-", "-", 1), side("-", "-", "-", 0)],
        sectors: sectors(4),
        ..Map::default()
    }
}

/// The chosen 64-wide cut on the horizontal wall, for the polygon
/// tests.
pub(super) fn sample_wall() -> DoorWall {
    DoorWall {
        linedef: 0_usize,
        point: point(64, 0),
        half_dir: point(32, 0),
        available_width: 128_i32,
    }
}

/// Twice the signed area of `polygon` in `i128` (shoelace).
pub(super) fn area2(polygon: &[Point]) -> i128 {
    polygon
        .iter()
        .zip(polygon.iter().skip(1).chain(polygon.iter()))
        .fold(0_i128, |total, (first, second)| {
            let x1 = i128::from(first.x);
            let y1 = i128::from(first.y);
            let x2 = i128::from(second.x);
            let y2 = i128::from(second.y);
            total
                .saturating_add(x1.saturating_mul(y2))
                .saturating_sub(x2.saturating_mul(y1))
        })
}
