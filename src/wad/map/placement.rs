//! Pure thing-placement core: the clearance primitives the plan builder's
//! room fills share with the `place_things` distribution engine.
//!
//! Nothing here touches archives, sessions or `MCP` types. [`fits`] answers
//! whether a thing of one clearance may stand in a sector; [`keeps_distance`]
//! is the exact `i128` point-to-segment test every clearance check runs on;
//! [`axis_points`] walks a grid. On top of them [`distribute`](fn@distribute)
//! spreads whole areas: for each [`Area`] it lays a square grid spaced twice
//! the clearance (at least 16 units) centred in the area's bounding box,
//! keeps the points inside the area and clear of every one-sided or blocking
//! line and of every existing thing, orders what is kept by distance from
//! the area centre (ties by `y` then `x`) and spreads the picks by taking
//! every k-th candidate. [`split_proportional`] answers how many of a total
//! belong to each sector: by bounding-box area, largest-remainder, ties to
//! the lower sector index. [`Facing`] chooses each pick's angle, snapped to
//! multiples of 45 degrees; [`Facing::Seeded`] is the engine's only allowed
//! randomness: a xorshift32 advanced deterministically from the caller's
//! `seed`.
//!
//! The measurement primitives, [`Area`] and the map readers stay here; how
//! a pick faces lives in the private `facing` module and the whole
//! distribution engine in the private `distribute` module, both re-exported
//! below; the engine's tests are the private `tests` module, its fixture
//! builders shared, the clearance cases and the split-and-facing cases one
//! topic file apiece.
//!
//! Every geometry read goes through one [`SectorIndex`] (CD-290a made the
//! free geometry functions exactly that): the run carries a map with the
//! index built once for the operation, and a whole run's containment, box
//! and boundary checks answer from it instead of rescanning the linedefs
//! per point. The free [`fits`] and [`boundary_clear`] helpers keep their
//! signatures and index the map for their single answer.

mod distribute;
mod facing;

pub use distribute::{DistributeSpec, Distribution, Placed, distribute, split_proportional};
pub use facing::{Facing, snap_angle};

use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::fixed::midpoint;
use crate::geom::Pt;
use crate::geom::segment::distance_point_segment;
use crate::wad::map::consts::PLAYER1_START;
use crate::wad::map::flags::{FlagFamily, blocks_players};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::model::{Linedef, Map};

/// The blocking radius assumed for a type the definitions do not list,
/// as the plan builder's room fills use it.
pub const DEFAULT_RADIUS: i64 = 16_i64;
/// Extra clearance every placed thing keeps from the sector boundary,
/// on top of its blocking radius, map units.
pub const CLEARANCE: i64 = 8_i64;
/// The largest number of grid points one area axis contributes, so a
/// huge bounding box cannot spin the fill forever.
pub(super) const MAX_AXIS_POINTS: usize = 4_096_usize;

/// A map point widened to the exact `i128` space of the distance tests.
pub(super) type End = (i128, i128);

/// The blocking radius of thing type `kind`: its definition's radius, or
/// `unknown_radius` when the definitions do not list the type.
#[must_use]
#[inline]
pub fn thing_radius(definitions: &Definitions, kind: u32, unknown_radius: i64) -> i64 {
    definitions
        .thing(kind)
        .map_or(unknown_radius, |row| i64::from(row.radius))
}

/// The wall clearance of thing type `kind`: [`thing_radius`] with
/// `unknown_radius` as the fallback, plus [`CLEARANCE`] map units.
#[must_use]
#[inline]
pub fn thing_clearance(definitions: &Definitions, kind: u32, unknown_radius: i64) -> i64 {
    thing_radius(definitions, kind, unknown_radius).saturating_add(CLEARANCE)
}

/// Whether `point` keeps at least `min` distance from the segment
/// `from`-`to`, exact in `i128`.
///
/// [`crate::geom`]'s point-to-segment fraction against `min * min`, the
/// perpendicular case working out to `cross^2 >= min^2 * len^2`.
#[must_use]
#[inline]
pub const fn keeps_distance(from: End, to: End, point: End, min: i128) -> bool {
    let (num, den) = distance_point_segment(point, from, to);
    num >= min.saturating_mul(min).saturating_mul(den)
}

/// Both endpoints of `linedef` widened to `i128`, when they resolve.
fn segment_endpoints(map: &Map, linedef: usize) -> Option<(End, End)> {
    let ((ax, ay), (bx, by)) = map.line_endpoints(map.linedefs.get(linedef)?)?;
    Some((
        (i128::from(ax), i128::from(ay)),
        (i128::from(bx), i128::from(by)),
    ))
}

/// Whether the line `linedef` keeps at least `clearance` from `at`.
#[must_use]
#[inline]
pub fn line_clear(map: &Map, linedef: usize, at: Pt, clearance: i64) -> bool {
    let Some((from, to)) = segment_endpoints(map, linedef) else {
        return true;
    };
    keeps_distance(from, to, wide(at), i128::from(clearance))
}

/// Whether every boundary line of `sector` keeps at least `clearance`
/// map units from `at`; lines with unresolvable endpoints never bound
/// anything and stay clear.
///
/// The single-answer helper: it indexes the map for this one check. A
/// run that measures many points holds one [`SectorIndex`] and reads its
/// [`SectorIndex::boundary`] once per check.
#[must_use]
#[inline]
pub fn boundary_clear(map: &Map, sector: usize, at: Pt, clearance: i64) -> bool {
    Read::new(map).boundary_clear(sector, at, clearance)
}

/// Whether a thing with this `clearance` may stand at `at` in `sector`:
/// the sector contains the point and every boundary line of the sector
/// keeps at least `clearance` map units of distance from it.
///
/// The single-answer helper: one [`SectorIndex`] built here carries both
/// reads; a run that measures many points indexes the map once for the
/// whole operation.
#[must_use]
#[inline]
pub fn fits(map: &Map, sector: usize, at: Pt, clearance: i64) -> bool {
    Read::new(map).fits(sector, at, clearance)
}

/// One placement operation's map reads: the map with its sector geometry
/// indexed once. Every containment, box and boundary check of a whole
/// run answers from the stored [`SectorIndex`] instead of rescanning the
/// linedefs per point; the free [`fits`] and [`boundary_clear`] helpers
/// wrap one [`Read`] for their single answer.
pub(super) struct Read<'map> {
    /// The map being read.
    map: &'map Map,
    /// Its per-sector geometry, built once for the operation.
    index: SectorIndex,
}

impl<'map> Read<'map> {
    /// Index `map` for one placement operation.
    pub(super) fn new(map: &'map Map) -> Self {
        Self {
            map,
            index: SectorIndex::new(map),
        }
    }

    /// Whether every boundary line of `sector` keeps at least
    /// `clearance` map units from `at`, over the stored index's
    /// boundary rows.
    #[must_use]
    pub(super) fn boundary_clear(&self, sector: usize, at: Pt, clearance: i64) -> bool {
        self.index
            .boundary(sector)
            .iter()
            .all(|&(line, _)| line_clear(self.map, line, at, clearance))
    }

    /// [`fits`]' answer from the stored index: the containment read and
    /// the boundary read share the one pass over the linedefs.
    #[must_use]
    pub(super) fn fits(&self, sector: usize, at: Pt, clearance: i64) -> bool {
        let (x, y) = at;
        self.index.point_in_sector(x, y) == Some(sector)
            && self.boundary_clear(sector, at, clearance)
    }
}

/// The grid values from `start` to `last` inclusive, `step` apart,
/// capped at 4 096 points per axis.
#[must_use]
#[inline]
pub fn axis_points(start: i64, last: i64, step: i64) -> Vec<i64> {
    let mut points = Vec::new();
    let mut value = start;
    while value <= last && points.len() < MAX_AXIS_POINTS {
        points.push(value);
        value = value.saturating_add(step);
    }
    points
}

/// One target area [`distribute`](fn@distribute) fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// A sector of the map: points the sector itself contains.
    Sector(
        /// The sector index.
        usize,
    ),
    /// An axis-aligned box in map units (inclusive edges).
    Rect {
        /// The lower-left corner `X`.
        min_x: i64,
        /// The lower-left corner `Y`.
        min_y: i64,
        /// The upper-right corner `X`.
        max_x: i64,
        /// The upper-right corner `Y`.
        max_y: i64,
    },
    /// A circle in map units.
    Circle {
        /// The centre point.
        centre: Pt,
        /// The radius, map units.
        radius: i64,
    },
}

impl Area {
    /// The area's bounding box `(min_x, min_y, max_x, max_y)` in map
    /// units, `None` when it bounds nothing (a sector with no resolvable
    /// boundary).
    ///
    /// The single-answer helper: a sector's box comes from a
    /// [`SectorIndex`] built here. A run measures its areas against one
    /// index, whose stored rows answer every box read.
    #[must_use]
    #[inline]
    pub fn bbox(&self, map: &Map) -> Option<(i64, i64, i64, i64)> {
        self.bbox_in(&SectorIndex::new(map))
    }

    /// [`Self::bbox`] over a prepared `index`: a sector answers from its
    /// stored box, the open shapes from their own coordinates.
    #[must_use]
    pub(super) fn bbox_in(&self, index: &SectorIndex) -> Option<(i64, i64, i64, i64)> {
        match *self {
            Self::Sector(sector) => index.bbox(sector).map(widen_box),
            Self::Rect {
                min_x,
                min_y,
                max_x,
                max_y,
            } => Some((min_x, min_y, max_x, max_y)),
            Self::Circle { centre, radius } => Some((
                centre.0.saturating_sub(radius),
                centre.1.saturating_sub(radius),
                centre.0.saturating_add(radius),
                centre.1.saturating_add(radius),
            )),
        }
    }

    /// The area's centre: the circle's own, every other area the bounding
    /// box midpoint; `None` when the area bounds nothing.
    #[must_use]
    #[inline]
    pub fn centre(&self, map: &Map) -> Option<Pt> {
        self.centre_in(&SectorIndex::new(map))
    }

    /// [`Self::centre`] over a prepared `index`.
    #[must_use]
    pub(super) fn centre_in(&self, index: &SectorIndex) -> Option<Pt> {
        match *self {
            Self::Circle { centre, .. } => Some(centre),
            Self::Sector(_) | Self::Rect { .. } => self.bbox_in(index).map(box_centre),
        }
    }

    /// Whether the point lies inside the area. A sector tests its own
    /// containment; the open shapes additionally require the point to sit
    /// in *some* sector of the map, so nothing lands in the void.
    ///
    /// The single-answer helper: the containment comes from a
    /// [`SectorIndex`] built here. A run measures its candidates against
    /// one index, whose stored rows answer every membership test.
    #[must_use]
    #[inline]
    pub fn contains(&self, map: &Map, at: Pt) -> bool {
        self.contains_in(&SectorIndex::new(map), at)
    }

    /// [`Self::contains`] over a prepared `index`: every sector
    /// containment - the own-sector arm and the open shapes'
    /// sits-in-some-sector arm alike - hits
    /// [`SectorIndex::point_in_sector`], never a fresh scan.
    #[must_use]
    pub(super) fn contains_in(&self, index: &SectorIndex, at: Pt) -> bool {
        match *self {
            Self::Sector(sector) => index.point_in_sector(at.0, at.1) == Some(sector),
            Self::Rect {
                min_x,
                min_y,
                max_x,
                max_y,
            } => {
                let inside = at.0 >= min_x && at.0 <= max_x && at.1 >= min_y && at.1 <= max_y;
                inside && index.point_in_sector(at.0, at.1).is_some()
            }
            Self::Circle { centre, radius } => {
                within_circle(centre, radius, at) && index.point_in_sector(at.0, at.1).is_some()
            }
        }
    }

    /// What warnings call the area.
    #[must_use]
    #[inline]
    pub fn label(&self) -> String {
        match *self {
            Self::Sector(sector) => format!("sector {sector}"),
            Self::Rect { .. } => "the bounding box".to_owned(),
            Self::Circle { .. } => "the circle".to_owned(),
        }
    }
}

/// The midpoint of a `(min_x, min_y, max_x, max_y)` bounding box, each
/// axis rounded through [`midpoint`].
#[must_use]
#[inline]
fn box_centre((min_x, min_y, max_x, max_y): (i64, i64, i64, i64)) -> Pt {
    (midpoint(min_x, max_x), midpoint(min_y, max_y))
}

/// A sector's stored `i32` bounding box, widened to the `i64` space the
/// areas measure in.
#[must_use]
fn widen_box((x1, y1, x2, y2): (i32, i32, i32, i32)) -> (i64, i64, i64, i64) {
    (i64::from(x1), i64::from(y1), i64::from(x2), i64::from(y2))
}

/// Whether `at` sits within `radius` of `centre`, exact in `i128`.
fn within_circle(centre: Pt, radius: i64, at: Pt) -> bool {
    let dx = i128::from(at.0).saturating_sub(i128::from(centre.0));
    let dy = i128::from(at.1).saturating_sub(i128::from(centre.1));
    let wide = i128::from(radius);
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) <= wide.saturating_mul(wide)
}

/// The squared distance from `at` to `centre`, for the ordering.
pub(super) fn distance_squared(centre: Pt, at: Pt) -> i128 {
    let dx = i128::from(at.0).saturating_sub(i128::from(centre.0));
    let dy = i128::from(at.1).saturating_sub(i128::from(centre.1));
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

/// The `point` widened into the exact `i128` test space.
pub(super) fn wide(point: Pt) -> End {
    (i128::from(point.0), i128::from(point.1))
}

/// Whether `line` is a wall to placement: one-sided, or carrying a bit
/// that stops bodies in the map's own flag family.
const fn is_wall(line: &Linedef, family: FlagFamily) -> bool {
    line.back.is_none() || blocks_players(line.flags, family)
}

/// The segments placement candidates keep clear of: every wall line of
/// the map whose endpoints resolve.
pub(super) fn wall_segments(map: &Map) -> Vec<(End, End)> {
    let family = map.flag_family();
    map.linedefs
        .iter()
        .enumerate()
        .filter(|entry| is_wall(entry.1, family))
        .filter_map(|entry| segment_endpoints(map, entry.0))
        .collect()
}

/// A thing already standing where candidates are measured against.
#[derive(Clone, Copy, Debug)]
pub struct Occupant {
    /// Where it stands.
    pub at: Pt,
    /// Its blocking radius, map units.
    pub radius: i64,
}

/// The existing things of `map` as obstacles: each row's position and
/// blocking radius (the definitions' for its type, `unknown_radius` for
/// types the tables do not list).
#[must_use]
#[inline]
pub fn occupants(map: &Map, definitions: &Definitions, unknown_radius: i64) -> Vec<Occupant> {
    map.things
        .iter()
        .map(|row| Occupant {
            at: (i64::from(row.x), i64::from(row.y)),
            radius: thing_radius(
                definitions,
                u32::try_from(row.kind).unwrap_or(u32::MAX),
                unknown_radius,
            ),
        })
        .collect()
}

/// The player 1 start of `map`: the position of its first `type 1` thing.
#[must_use]
#[inline]
pub fn player_start(map: &Map) -> Option<Pt> {
    map.things
        .iter()
        .find(|row| row.kind == PLAYER1_START)
        .map(|row| (i64::from(row.x), i64::from(row.y)))
}

#[cfg(test)]
mod tests;
