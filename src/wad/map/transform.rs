//! Pure selection transform: the integer scale/rotate/translate math and
//! the whole-map step behind `transform_selection`.
//!
//! Everything here is a function of the [`Map`] model and plain integers:
//! no sessions, no MCP. Positions move through
//! `p' = origin + rotate(scale(p - origin)) + translate`, applied in that
//! order: the scale is the exact fraction `num / den` folded onto the
//! origin-relative offset with [`mul_div_round`], the rotation is
//! [`rotate_point`] (counter-clockwise, `BAM` angles), and every stage
//! rounds to nearest, so the result is a whole map unit. Thing facings take
//! the rotation as whole degrees normalised into `0..=359`. [`expand_selection`]
//! turns a request over any mix of vertices, linedefs, sectors and things
//! into the vertex and thing sets the transform moves, using
//! [`crate::wad::map::geometry`].
//!
//! The safety rule mirrors [`super::ops`]: a refused call leaves the map
//! untouched. Every resulting coordinate is checked against what the map's
//! format can store (`i16` for the binary formats) and every touched
//! linedef is checked against collapsing to zero length before a single
//! row is written. Vertices that land on an unselected vertex are allowed
//! and merely reported as coincident; nothing is merged.
//!
//! This file keeps the shapes one call is phrased in, the maths they carry
//! and [`apply_transform`] itself; reading a request against a map is the
//! private `select` module and planning, checking and writing the moves the
//! private `apply` module. Both are implementation details of the two entry
//! points the edit tool calls.

mod apply;
mod select;

use alloc::collections::BTreeSet;

use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::{Angle, div_round, mul_div_round, narrow_i64, rotate_point};

use super::model::Map;
use apply::{
    check_collapse, coincident_pairs, default_origin, move_things, move_vertices, moved_points,
    write_moves,
};

pub use select::expand_selection;

/// Whole degrees in one turn, for thing-facing normalisation.
pub(super) const DEGREES: i64 = 360;
/// Millidegrees in one degree.
const MILLIDEGREES: i64 = 1_000;

/// A point in map units, widened to `i64` so no transform intermediate
/// can overflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Point {
    /// `X` coordinate in map units.
    pub x: i64,
    /// `Y` coordinate in map units.
    pub y: i64,
}

impl Point {
    /// The point at a model element's `i32` coordinates.
    #[must_use]
    #[inline]
    pub fn from_model(x: i32, y: i32) -> Self {
        Self {
            x: i64::from(x),
            y: i64::from(y),
        }
    }
}

/// An exact positive rational scale factor `num / den`
/// (e.g. `3 / 2` grows to 150%, `1 / 2` shrinks to 50%).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// Numerator; above zero.
    pub num: i64,
    /// Denominator; above zero.
    pub den: i64,
}

impl Scale {
    /// Builds `num / den`, refusing either part at or below zero.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a part is not positive.
    #[inline]
    pub fn new(num: i32, den: i32) -> Result<Self, CyberError> {
        if num <= 0_i32 || den <= 0_i32 {
            return Err(CyberError::InvalidArgument(format!(
                "scale {num}/{den} is not a positive factor: pass a numerator and a \
                 denominator both above 0 (3/2 grows to 150%, 1/2 shrinks to 50%)"
            )));
        }
        Ok(Self {
            num: i64::from(num),
            den: i64::from(den),
        })
    }
}

/// A counter-clockwise rotation: exact in `BAM` for positions, whole
/// degrees for thing facings, which the formats store as integers.
#[derive(Clone, Copy, Debug)]
pub struct Rotation {
    /// The `BAM` angle positions are turned by.
    pub angle: Angle,
    /// Whole degrees added to the selected things' `angle`.
    pub degrees: i64,
}

impl Rotation {
    /// A rotation of whole `degrees` (any integer, counter-clockwise).
    #[must_use]
    #[inline]
    pub fn from_degrees(degrees: i64) -> Self {
        Self {
            angle: Angle::from_degrees(degrees),
            degrees,
        }
    }

    /// A rotation of `millidegrees` thousandths of a degree; thing facings
    /// take the nearest whole degree.
    #[must_use]
    #[inline]
    pub fn from_millidegrees(millidegrees: i64) -> Self {
        Self {
            angle: Angle::from_millidegrees(millidegrees),
            degrees: div_round(i128::from(millidegrees), i128::from(MILLIDEGREES))
                .map_or(millidegrees, narrow_i64),
        }
    }
}

/// The operations of one call, applied in the order scale, rotate,
/// translate.
#[derive(Clone, Copy, Debug, Default)]
pub struct Operations {
    /// Exact-fraction scaling about the origin, applied first.
    pub scale: Option<Scale>,
    /// Counter-clockwise rotation about the origin, applied second.
    pub rotation: Option<Rotation>,
    /// Whole-unit shift, applied last.
    pub translate: Option<Point>,
}

impl Operations {
    /// Whether no operation is set (a call must carry at least one).
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.scale.is_none() && self.rotation.is_none() && self.translate.is_none()
    }

    /// `origin + rotate(scale(p - origin)) + translate`, each stage rounded
    /// to nearest. `None` when any intermediate or the result leaves `i64`.
    fn apply_point(&self, point: Point, origin: Point) -> Option<Point> {
        let mut dx = point.x.checked_sub(origin.x)?;
        let mut dy = point.y.checked_sub(origin.y)?;
        if let Some(scale) = self.scale {
            dx = mul_div_round(dx, scale.num, scale.den)?;
            dy = mul_div_round(dy, scale.num, scale.den)?;
        }
        if let Some(rotation) = self.rotation {
            (dx, dy) = rotate_point(dx, dy, rotation.angle);
        }
        if let Some(delta) = self.translate {
            dx = dx.checked_add(delta.x)?;
            dy = dy.checked_add(delta.y)?;
        }
        Some(Point {
            x: origin.x.checked_add(dx)?,
            y: origin.y.checked_add(dy)?,
        })
    }
}

/// The caller's selection request, before it is expanded against a map.
#[derive(Clone, Debug, Default)]
pub struct SelectionRequest {
    /// Vertex indices to move.
    pub vertices: Vec<usize>,
    /// Linedef indices: both their endpoints join the selection.
    pub linedefs: Vec<usize>,
    /// Sector indices: their whole boundary joins the selection, and their
    /// things may too.
    pub sectors: Vec<usize>,
    /// Thing indices to move.
    pub things: Vec<usize>,
    /// Whether every vertex and thing of the map is selected.
    pub all: bool,
    /// Whether things standing in a selected sector join the selection.
    pub include_things_in_sectors: bool,
}

/// An expanded selection: exactly which vertices and things move.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Indices into [`Map::vertices`].
    pub vertices: BTreeSet<usize>,
    /// Indices into [`Map::things`].
    pub things: BTreeSet<usize>,
}

/// What one successful [`apply_transform`] did.
#[derive(Clone, Debug, Serialize)]
pub struct TransformReport {
    /// The point positions were scaled and rotated about.
    pub origin: Point,
    /// The expanded selection's vertex indices, ascending.
    pub vertices_selected: Vec<usize>,
    /// The expanded selection's thing indices, ascending.
    pub things_selected: Vec<usize>,
    /// How many vertices were written at new coordinates.
    pub vertices_moved: usize,
    /// How many things were written at new coordinates.
    pub things_moved: usize,
    /// `[moved, unmoved]` index pairs that now share one position.
    pub coincident_vertices: Vec<[usize; 2]>,
}

/// One vertex's planned destination, already checked to fit the format.
#[derive(Clone, Copy, Debug)]
struct Move {
    /// Index into [`Map::vertices`].
    index: usize,
    /// The stored `X` to write.
    x: i32,
    /// The stored `Y` to write.
    y: i32,
}

impl Move {
    /// The planned position as a [`Point`].
    fn point(self) -> Point {
        Point::from_model(self.x, self.y)
    }
}

/// A thing's planned destination and facing.
#[derive(Clone, Copy, Debug)]
struct ThingMove {
    /// The position to write.
    position: Move,
    /// The facing after a rotation; `None` keeps the current angle.
    angle: Option<i32>,
}

/// Scale, rotate and translate the selected vertices and things of `map`
/// about `origin`, in place, and report what moved.
///
/// `origin` defaults to the centre (rounded to nearest) of the selection's
/// bounding box. The refusal rule: a coordinate the map's format cannot
/// store fails `invalid_argument` and a linedef that becomes zero-length
/// fails `conflict` naming it, both before any row is written, so a failed
/// call leaves `map` untouched. Vertices landing on unselected vertices are
/// allowed and listed in the report's `coincident_vertices`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty selection, a coordinate
/// that overflows or leaves the format's range; [`CyberError::Conflict`]
/// for a linedef collapsing to zero length.
#[inline]
pub fn apply_transform(
    map: &mut Map,
    selection: &Selection,
    ops: &Operations,
    pivot: Option<Point>,
) -> Result<TransformReport, CyberError> {
    if selection.vertices.is_empty() && selection.things.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the selection holds no vertex and no thing: expand a non-empty request first"
                .to_owned(),
        ));
    }
    let origin = pivot.unwrap_or_else(|| default_origin(map, selection));
    let vertices = move_vertices(map, selection, ops, origin)?;
    let moved = moved_points(&vertices);
    let things = move_things(map, selection, ops, origin)?;
    check_collapse(map, &moved)?;
    let coincident_vertices = coincident_pairs(map, &moved);
    write_moves(map, &vertices, &things);
    Ok(TransformReport {
        origin,
        vertices_selected: vertices.iter().map(|vertex| vertex.index).collect(),
        things_selected: things.iter().map(|item| item.position.index).collect(),
        vertices_moved: vertices.len(),
        things_moved: things.len(),
        coincident_vertices,
    })
}

#[cfg(test)]
mod tests;
