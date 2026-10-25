//! The pure linedef geometry engine: [`split_linedef`] cuts one linedef
//! into two at a point on it and [`join_linedefs`] welds two collinear
//! ones back together, the algorithms behind the two eponymous tools.
//!
//! Everything here is a function of the [`Map`] model and plain integers:
//! no sessions, no MCP. The texture-offset rule drives both operations: a
//! side's `x_offset` is measured from its linedef's `v1` on the front and
//! from its `v2` on the back, so a side keeps its offset when its anchor
//! vertex stays put and shifts by the length of the gained or lost piece
//! when the anchor moves. Lengths are [`crate::fixed::hypot`] of the
//! vertex deltas (floored to whole map units); on-segment and collinearity
//! tests use `i128` cross and dot products. Like
//! [`crate::wad::map::ops::split_linedef`], a refused call
//! leaves the map untouched: every check runs before any mutation.
//!
//! The split pass, from the split point through its plan to the writes,
//! lives in the private `split` module; the join pass, from the order the
//! two linedefs meet in through its plan to the removal engine, in the
//! private `join` module. The two entry points, the shapes they report and
//! every check and geometry helper both passes share are this file.

mod join;
mod split;

use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::wad::classify::MapFormat;

use super::model::{Linedef, Map, Sidedef, Vertex};
use super::remove::RemovalReport;
use join::{apply_join, plan_join};
use split::{apply_split, plan_split, require_on_segment, split_point};

/// The lowest coordinate an `i16` binary `VERTEXES` column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest coordinate an `i16` binary `VERTEXES` column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;
/// The lowest offset an `i16` binary `SIDEDEFS` column holds (`i16::MIN`).
const OFFSET_MIN: i32 = -0x8000_i32;
/// The highest offset an `i16` binary `SIDEDEFS` column holds (`i16::MAX`).
const OFFSET_MAX: i32 = 0x7FFF_i32;
/// Divisor of the two-endpoint sum that gives a midpoint.
const MIDPOINT_DIVISOR: i64 = 2_i64;

/// A point in map units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    /// `X` coordinate in map units.
    pub x: i32,
    /// `Y` coordinate in map units.
    pub y: i32,
}

/// A side whose anchor vertex moved: its index and new `x_offset`.
pub(super) type SideShift = Option<(usize, i32)>;

/// What [`split_linedef`] did, as the tool reports it.
#[derive(Clone, Debug, Serialize)]
pub struct SplitResult {
    /// Index in [`Map::vertices`] of the split point (an existing vertex
    /// reused, or a new one created).
    pub vertex: usize,
    /// Index in [`Map::linedefs`] of the appended second half.
    pub new_linedef: usize,
    /// Indices in [`Map::sidedefs`] of the copied sides, in creation order
    /// (front first, then back when the line had one).
    pub new_sidedefs: Vec<usize>,
}

/// What [`join_linedefs`] did, as the tool reports it.
#[derive(Debug, Serialize)]
pub struct JoinResult {
    /// Index of the surviving linedef after the removal renumbered the
    /// block.
    pub linedef: usize,
    /// What the removal engine dropped: the second linedef, its orphaned
    /// sidedefs and the absorbed vertex (original indices, sorted).
    pub removed: RemovalReport,
}

/// The two linedefs' relative direction, decided by which pair of endpoints
/// meets head to tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Order {
    /// `first` is the head: its `v2` is the shared vertex.
    FirstHead,
    /// `second` is the head: `first` is the tail, its `v1` is the shared
    /// vertex.
    SecondHead,
}

/// A validated [`split_linedef`]: the split point, its integer piece lengths
/// and the side copies and shift it implies.
pub(super) struct SplitPlan {
    /// The split point coordinates.
    point: Point,
    /// The front copy for the new linedef, `None` without a front side.
    front: Option<Sidedef>,
    /// The back copy for the new linedef, `None` without a back side.
    back: Option<Sidedef>,
    /// The original back sidedef to shift, with its new `x_offset`.
    shift: SideShift,
}

/// A validated [`join_linedefs`]: the survivor's retargeting, the absorbed
/// vertex and the side shift the moved anchor implies.
#[derive(Clone, Copy)]
pub(super) struct JoinPlan {
    /// Index of the surviving (first) linedef.
    first: usize,
    /// Index of the removed (second) linedef.
    second: usize,
    /// Index of the absorbed shared vertex.
    shared: usize,
    /// The survivor's new far endpoint (the second linedef's free end).
    far: usize,
    /// Whether the survivor keeps its `v1` (`first` is the head) or its
    /// `v2`.
    head_is_first: bool,
    /// The survivor's side whose anchor moved, with its new `x_offset`.
    shift: SideShift,
}

/// Cuts linedef `index` of `map` into two at `at`, or at its midpoint
/// (rounded to nearest) when `at` names no point.
///
/// The original linedef keeps its `v1` and ends at the split point; the
/// appended linedef runs from the split point to the original `v2` and
/// copies its `flags`, `special`, `tag`, `args` and `extra`. Each existing
/// side gets a copy for the new line and the offset shift that keeps its
/// texture at the same place along the wall: the front copy shifts by the
/// length of the first piece, the surviving back shifts by the length of
/// the second. A vertex already sitting exactly on the split point is
/// reused, else one is created.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a linedef or vertex index the map does not
/// hold, or a side reference into a missing sidedef.
/// [`CyberError::InvalidArgument`] when the given point is off the segment
/// (perpendicular distance above one map unit) or not strictly between the
/// endpoints, when the split point would coincide with an endpoint (a
/// zero-length line, or a midpoint rounded onto an endpoint of a one-unit
/// line), when a binary map cannot store the split point's coordinates or
/// a shifted offset, or when a piece is too long to shift an offset by.
#[inline]
pub fn split_linedef(
    map: &mut Map,
    index: usize,
    at: Option<Point>,
) -> Result<SplitResult, CyberError> {
    let line = linedef_row(map, index)?;
    let from = endpoint(map, line.v1)?;
    let to = endpoint(map, line.v2)?;
    let point = split_point(at, from, to);
    require_on_segment(point, from, to)?;
    check_point_format(map.format, point)?;
    let plan = plan_split(map, &line, point)?;
    Ok(apply_split(map, index, &line, plan))
}

/// Welds linedefs `first` and `second` into the first of them, in `map`.
///
/// The two must share exactly one vertex, meet head to tail (`first.v2`
/// equals `second.v1`, or `second.v2` equals `first.v1`), run in the same
/// direction along one line, and carry the same front and back sector (or
/// the same absence of a side). The survivor spans from its free end
/// through the shared vertex to the other free end, keeps its own sides
/// with the front or back `x_offset` shifted by the length of the removed
/// line, and the second linedef, its orphaned sidedefs and the absorbed
/// vertex leave through the removal engine.
///
/// Without `force`, differing `flags`, `special`, `tag`, `args` or side
/// textures are refused; with it, the first linedef's properties win.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a linedef or vertex index the map does not
/// hold, or a side reference into a missing sidedef.
/// [`CyberError::InvalidArgument`] when the pair shares no vertex or both,
/// does not meet head to tail, is not collinear, runs opposite, opens onto
/// different sectors, or leaves a binary map's offset range.
/// [`CyberError::Conflict`] when a third linedef uses the shared vertex,
/// or the two differ without `force`.
#[inline]
pub fn join_linedefs(
    map: &mut Map,
    first: usize,
    second: usize,
    force: bool,
) -> Result<JoinResult, CyberError> {
    let plan = plan_join(map, first, second, force)?;
    apply_join(map, plan)
}

/// The linedef `index`, cloned so the edit can hand the map around freely.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the index is past [`Map::linedefs`].
pub(super) fn linedef_row(map: &Map, index: usize) -> Result<Linedef, CyberError> {
    map.linedefs
        .get(index)
        .cloned()
        .ok_or_else(|| missing("linedef", index, map.linedefs.len()))
}

/// The coordinates of vertex `index`.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the index is past [`Map::vertices`].
pub(super) fn endpoint(map: &Map, index: usize) -> Result<Point, CyberError> {
    map.vertices
        .get(index)
        .map(|vertex| Point {
            x: vertex.x,
            y: vertex.y,
        })
        .ok_or_else(|| missing("vertex", index, map.vertices.len()))
}

/// The `NotFound` saying `kind` `index` is past a block of `count`.
fn missing(kind: &str, index: usize, count: usize) -> CyberError {
    let range = count.checked_sub(1).map_or_else(
        || format!("the map has no {kind}s"),
        |last| format!("the map has {count} {kind}s, valid indices are 0..={last}"),
    );
    CyberError::NotFound(format!("{kind} {index} does not exist ({range})"))
        .with_hint("describe_map or find_elements returns indices")
}

/// The `InvalidArgument` telling the caller to pass a point on the segment
/// between the endpoints.
pub(super) fn off_segment(detail: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "not a split point on this linedef: {detail}; pass a point on the segment between \
         the endpoints, within one map unit of the line, or leave `at` out for the midpoint"
    ))
}

/// The sidedef `index`, or a `NotFound` naming the side that points at it.
pub(super) fn sidedef_of<'model>(
    map: &'model Map,
    index: usize,
    which: &'static str,
) -> Result<&'model Sidedef, CyberError> {
    map.sidedefs.get(index).ok_or_else(|| {
        CyberError::NotFound(format!(
            "the linedef's {which} side references sidedef {index}, which the map does not \
             hold: fix the reference with edit_linedef first"
        ))
    })
}

/// The vector from `from` to `to` as `i64` deltas; `i32` coordinates never
/// wrap the wider difference.
pub(super) fn delta(from: Point, to: Point) -> (i64, i64) {
    (
        i64::from(to.x).wrapping_sub(i64::from(from.x)),
        i64::from(to.y).wrapping_sub(i64::from(from.y)),
    )
}

/// The `i128` product of two `i64` deltas (map coordinates square far
/// inside `i128`; saturation is defensive).
pub(super) fn wide(first: i64, second: i64) -> i128 {
    i128::from(first).saturating_mul(i128::from(second))
}

/// The squared length of the `from` to `to` segment, in `i128`.
pub(super) fn length_squared(from: Point, to: Point) -> i128 {
    let (dx, dy) = delta(from, to);
    wide(dx, dx).saturating_add(wide(dy, dy))
}

/// `(a + b) / 2` rounded to nearest, halves away from zero: one coordinate
/// of a midpoint. The midpoint of two `i32`s always fits `i32`, so the
/// fallback is defensive.
fn half_sum(a: i32, b: i32) -> i32 {
    let sum = i64::from(a).saturating_add(i64::from(b));
    let half = mul_div_round(sum, 1_i64, MIDPOINT_DIVISOR).unwrap_or(sum);
    i32::try_from(half).unwrap_or(i32::MAX)
}

/// The midpoint of `from` and `to`, each coordinate rounded to nearest.
pub(super) fn midpoint(from: Point, to: Point) -> Point {
    Point {
        x: half_sum(from.x, to.x),
        y: half_sum(from.y, to.y),
    }
}

/// Refuses a split point a binary `VERTEXES` record cannot store; `UDMF`
/// holds the full `i32` range.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a coordinate of `point` leaves the
/// `i16` range on a binary map.
fn check_point_format(format: MapFormat, point: Point) -> Result<(), CyberError> {
    let inside =
        (COORD_MIN..=COORD_MAX).contains(&point.x) && (COORD_MIN..=COORD_MAX).contains(&point.y);
    match format {
        MapFormat::Doom | MapFormat::Hexen if !inside => Err(CyberError::InvalidArgument(format!(
            "the split point {}, {} is out of range for the binary map format: vertices \
                 hold {COORD_MIN}..={COORD_MAX}",
            point.x, point.y
        ))),
        MapFormat::Doom | MapFormat::Hexen | MapFormat::Udmf => Ok(()),
    }
}

/// Refuses an `x_offset` a binary `SIDEDEFS` record cannot store.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `value` leaves the `i16` range on
/// a binary map.
pub(super) fn check_offset(format: MapFormat, which: &str, value: i32) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen if !(OFFSET_MIN..=OFFSET_MAX).contains(&value) => {
            Err(CyberError::InvalidArgument(format!(
                "the edit leaves the {which} side an x_offset of {value}, outside the \
                 {OFFSET_MIN}..={OFFSET_MAX} the binary format stores"
            )))
        }
        MapFormat::Doom | MapFormat::Hexen | MapFormat::Udmf => Ok(()),
    }
}

/// The vertex sitting exactly on `point`, created when none exists.
pub(super) fn place_vertex(map: &mut Map, point: Point) -> usize {
    if let Some(existing) = find_vertex(map, point) {
        return existing;
    }
    map.vertices.push(Vertex {
        x: point.x,
        y: point.y,
        ..Default::default()
    });
    map.vertices.len().saturating_sub(1)
}

/// The index of the first vertex sitting exactly on `point`.
fn find_vertex(map: &Map, point: Point) -> Option<usize> {
    map.vertices
        .iter()
        .position(|vertex| vertex.x == point.x && vertex.y == point.y)
}

/// Appends `side` to [`Map::sidedefs`], records its index in `created` and
/// returns it.
pub(super) fn push_sidedef(map: &mut Map, side: Sidedef, created: &mut Vec<usize>) -> usize {
    map.sidedefs.push(side);
    let index = map.sidedefs.len().saturating_sub(1);
    created.push(index);
    index
}

#[cfg(test)]
mod tests;
