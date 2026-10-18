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

use alloc::collections::BTreeSet;
use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::{hypot, mul_div_round};
use crate::wad::classify::MapFormat;

use super::model::{Linedef, Map, Sidedef, Vertex};
use super::remove::{CascadeOptions, Removal, RemovalReport, apply_removal, expand_cascade};

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
type SideShift = Option<(usize, i32)>;

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
enum Order {
    /// `first` is the head: its `v2` is the shared vertex.
    FirstHead,
    /// `second` is the head: `first` is the tail, its `v1` is the shared
    /// vertex.
    SecondHead,
}

/// A validated [`split_linedef`]: the split point, its integer piece lengths
/// and the side copies and shift it implies.
struct SplitPlan {
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
struct JoinPlan {
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
fn linedef_row(map: &Map, index: usize) -> Result<Linedef, CyberError> {
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
fn endpoint(map: &Map, index: usize) -> Result<Point, CyberError> {
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
    CyberError::NotFound(format!(
        "{kind} {index} does not exist (map has {count} {kind}s)"
    ))
}

/// The `InvalidArgument` telling the caller to pass a point on the segment
/// between the endpoints.
fn off_segment(detail: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "not a split point on this linedef: {detail}; pass a point on the segment between \
         the endpoints, within one map unit of the line, or leave `at` out for the midpoint"
    ))
}

/// The sidedef `index`, or a `NotFound` naming the side that points at it.
fn sidedef_of<'model>(
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

/// The sector behind an optional side reference, `None` for an absent side
/// (and, defensively, for a reference into a missing sidedef).
fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    side.and_then(|index| map.sidedefs.get(index))
        .map(|sidedef| sidedef.sector)
}

/// The vector from `from` to `to` as `i64` deltas; `i32` coordinates never
/// wrap the wider difference.
fn delta(from: Point, to: Point) -> (i64, i64) {
    (
        i64::from(to.x).wrapping_sub(i64::from(from.x)),
        i64::from(to.y).wrapping_sub(i64::from(from.y)),
    )
}

/// The `i128` product of two `i64` deltas (map coordinates square far
/// inside `i128`; saturation is defensive).
fn wide(first: i64, second: i64) -> i128 {
    i128::from(first).saturating_mul(i128::from(second))
}

/// The squared length of the `from` to `to` segment, in `i128`.
fn length_squared(from: Point, to: Point) -> i128 {
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
fn midpoint(from: Point, to: Point) -> Point {
    Point {
        x: half_sum(from.x, to.x),
        y: half_sum(from.y, to.y),
    }
}

/// The split point: `at` as given, else the midpoint (each coordinate
/// rounded to nearest). Either way [`require_on_segment`] checks it.
fn split_point(at: Option<Point>, from: Point, to: Point) -> Point {
    at.unwrap_or_else(|| midpoint(from, to))
}

/// Refuses a point more than one map unit off the `from` to `to` line, or
/// not strictly between its endpoints.
///
/// The perpendicular distance test is the integer one: with
/// `cross = dx * py - dy * px` the distance is `|cross| / length`, so
/// `distance <= 1` is `cross * cross <= length * length`. The strict
/// interior test is the projection: `dot = dx * px + dy * py` must sit
/// inside `0 .. length * length`. It also refuses a zero-length segment
/// (nothing between its endpoints to cut at) and a midpoint that rounds
/// onto an endpoint of a one-unit line.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a zero-length segment, a point off
/// the line, or a point whose projection falls on or past an endpoint.
fn require_on_segment(point: Point, from: Point, to: Point) -> Result<(), CyberError> {
    let squared = length_squared(from, to);
    if squared == 0_i128 {
        return Err(off_segment("the linedef is zero-length"));
    }
    let (dx, dy) = delta(from, to);
    let (px, py) = delta(from, point);
    let cross = wide(dx, py).saturating_sub(wide(dy, px));
    if cross.saturating_mul(cross) > squared {
        return Err(off_segment(&format!(
            "the point {}, {} is more than one map unit off the line",
            point.x, point.y
        )));
    }
    let dot = wide(dx, px).saturating_add(wide(dy, py));
    if dot <= 0_i128 || dot >= squared {
        return Err(off_segment(&format!(
            "the point {}, {} is not strictly between the endpoints",
            point.x, point.y
        )));
    }
    Ok(())
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
fn check_offset(format: MapFormat, which: &str, value: i32) -> Result<(), CyberError> {
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

/// The integer length of the `from` to `to` piece (`hypot`, floored),
/// refused when no `i32` offset can be shifted by it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the piece is longer than
/// `i32::MAX` map units.
fn piece_length(from: Point, to: Point) -> Result<i32, CyberError> {
    let (dx, dy) = delta(from, to);
    let length = hypot(dx, dy);
    i32::try_from(length).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "the split piece is {length} units long, past what an x_offset can shift by"
        ))
    })
}

/// The whole [`SplitPlan`] for `line` cut at `point`: checked piece lengths,
/// checked side copies and the checked back-side shift.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a dangling side reference, and
/// [`CyberError::InvalidArgument`] through [`piece_length`] and
/// [`check_offset`].
fn plan_split(map: &Map, line: &Linedef, point: Point) -> Result<SplitPlan, CyberError> {
    let from = endpoint(map, line.v1)?;
    let to = endpoint(map, line.v2)?;
    let first_len = piece_length(from, point)?;
    let second_len = piece_length(point, to)?;
    let front = line
        .front
        .map(|index| front_copy(map, index, first_len))
        .transpose()?;
    let moved_back = line
        .back
        .map(|index| back_pieces(map, index, second_len))
        .transpose()?;
    let (back, shift) = match moved_back {
        Some((copy, moved)) => (Some(copy), moved),
        None => (None, None),
    };
    Ok(SplitPlan {
        point,
        front,
        back,
        shift,
    })
}

/// The back side's copy for the new linedef (the original's old offset) and
/// the shift the surviving original needs, refusing an offset the binary
/// format cannot store.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a dangling reference, and
/// [`CyberError::InvalidArgument`] when the shifted offset leaves the
/// binary range.
fn back_pieces(
    map: &Map,
    index: usize,
    second_len: i32,
) -> Result<(Sidedef, SideShift), CyberError> {
    let side = sidedef_of(map, index, "back")?;
    let shifted = side.x_offset.wrapping_add(second_len);
    check_offset(map.format, "back", shifted)?;
    Ok((side.clone(), Some((index, shifted))))
}

/// The front side's copy for the new linedef: `x_offset` shifted by the
/// length of the first piece, everything else identical.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a dangling reference, and
/// [`CyberError::InvalidArgument`] when the shifted offset leaves the
/// binary range.
fn front_copy(map: &Map, index: usize, first_len: i32) -> Result<Sidedef, CyberError> {
    let side = sidedef_of(map, index, "front")?;
    let mut copy = side.clone();
    copy.x_offset = side.x_offset.wrapping_add(first_len);
    check_offset(map.format, "front", copy.x_offset)?;
    Ok(copy)
}

/// Runs a fully validated [`plan_split`]: places the vertex, copies the
/// sides, shifts the back, retargets the original and appends the second
/// half. Every reference the plan names was checked, so the lookups here
/// cannot fail.
fn apply_split(map: &mut Map, index: usize, line: &Linedef, plan: SplitPlan) -> SplitResult {
    let mid = place_vertex(map, plan.point);
    let mut created = Vec::new();
    let front = plan.front.map(|side| push_sidedef(map, side, &mut created));
    let back = plan.back.map(|side| push_sidedef(map, side, &mut created));
    if let Some((old, offset)) = plan.shift {
        if let Some(side) = map.sidedefs.get_mut(old) {
            side.x_offset = offset;
        }
    }
    if let Some(survivor) = map.linedefs.get_mut(index) {
        survivor.v2 = mid;
    }
    let new_linedef = map.linedefs.len();
    map.linedefs.push(Linedef {
        v1: mid,
        v2: line.v2,
        flags: line.flags,
        special: line.special,
        tag: line.tag,
        args: line.args,
        front,
        back,
        extra: line.extra.clone(),
    });
    SplitResult {
        vertex: mid,
        new_linedef,
        new_sidedefs: created,
    }
}

/// The vertex sitting exactly on `point`, created when none exists.
fn place_vertex(map: &mut Map, point: Point) -> usize {
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
fn push_sidedef(map: &mut Map, side: Sidedef, created: &mut Vec<usize>) -> usize {
    map.sidedefs.push(side);
    let index = map.sidedefs.len().saturating_sub(1);
    created.push(index);
    index
}

/// The order the two linedefs meet in: which end of which is shared.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when neither pair of facing endpoints
/// meets (no shared head-to-tail vertex, or both endpoints shared).
fn join_order(first: &Linedef, second: &Linedef) -> Result<Order, CyberError> {
    match (first.v2 == second.v1, second.v2 == first.v1) {
        (true, true) => Err(CyberError::InvalidArgument(
            "the two linedefs share both endpoints: joining them would fold one back onto \
             the other; remove one instead"
                .to_owned(),
        )),
        (true, false) => Ok(Order::FirstHead),
        (false, true) => Ok(Order::SecondHead),
        (false, false) => Err(CyberError::InvalidArgument(
            "the two linedefs do not meet head to tail: the end of one must be the other's \
             start (one shared vertex); flip a line with edit_linedef (flip: true) or pick \
             the two lines along the joint"
                .to_owned(),
        )),
    }
}

/// The `(shared, far)` vertex indices of a join in `order`.
const fn join_ends(order: Order, first: &Linedef, second: &Linedef) -> (usize, usize) {
    match order {
        Order::FirstHead => (first.v2, second.v2),
        Order::SecondHead => (first.v1, second.v1),
    }
}

/// Refuses two linedefs that are not collinear or run in opposite
/// directions, comparing their `v1` to `v2` direction vectors.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the cross product is not zero
/// (different lines) or the dot product is not positive (opposite or
/// zero-length directions).
fn require_same_direction(
    (start, stop): (Point, Point),
    (next_start, next_stop): (Point, Point),
) -> Result<(), CyberError> {
    let (fx, fy) = delta(start, stop);
    let (sx, sy) = delta(next_start, next_stop);
    if wide(fx, sy) != wide(fy, sx) {
        return Err(CyberError::InvalidArgument(
            "the two linedefs are not collinear: join only lines along one straight run, \
             check their coordinates with assemble_map"
                .to_owned(),
        ));
    }
    if wide(fx, sx).saturating_add(wide(fy, sy)) <= 0_i128 {
        return Err(CyberError::InvalidArgument(
            "the two linedefs run in opposite directions: flip one with edit_linedef \
             (flip: true) so they point the same way along the join"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Refuses sides that do not open onto the same sectors: the same front
/// sector, and the same back sector or no back on both.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming which side disagrees.
fn require_matching_sectors(
    map: &Map,
    first: &Linedef,
    second: &Linedef,
) -> Result<(), CyberError> {
    if side_sector(map, first.front) != side_sector(map, second.front) {
        return Err(CyberError::InvalidArgument(
            "the two linedefs' front sides open onto different sectors (or one has no \
             front side): only a line whose faces match can be joined"
                .to_owned(),
        ));
    }
    if side_sector(map, first.back) != side_sector(map, second.back) {
        return Err(CyberError::InvalidArgument(
            "the two linedefs' back sides differ (different sectors, or one line is \
             two-sided and the other is not): close the extra side with edit_linedef \
             (back: null) or keep the lines apart"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Refuses a join whose shared vertex is used by any third linedef.
///
/// # Errors
///
/// [`CyberError::Conflict`] naming the third linedef.
fn require_vertex_is_theirs_alone(
    map: &Map,
    shared: usize,
    first: usize,
    second: usize,
) -> Result<(), CyberError> {
    let blocker = map.linedefs.iter().enumerate().find(|&(index, line)| {
        index != first && index != second && (line.v1 == shared || line.v2 == shared)
    });
    if let Some((index, _)) = blocker {
        return Err(CyberError::Conflict(format!(
            "the shared vertex {shared} is also an endpoint of linedef {index}: a join would \
             dangle it; remove or move that line first"
        )));
    }
    Ok(())
}

/// The named differences between the two linedefs and their sides, empty
/// when a plain join loses nothing.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a side reference into a missing sidedef.
fn join_differences(
    map: &Map,
    first: &Linedef,
    second: &Linedef,
) -> Result<Vec<String>, CyberError> {
    let mut differences = Vec::new();
    if first.flags != second.flags {
        differences.push("flags".to_owned());
    }
    if first.special != second.special {
        differences.push("special".to_owned());
    }
    if first.tag != second.tag {
        differences.push("tag".to_owned());
    }
    if first.args != second.args {
        differences.push("args".to_owned());
    }
    for (which, a, b) in [
        ("front", first.front, second.front),
        ("back", first.back, second.back),
    ] {
        if side_pair_differs(map, a, b, which)? {
            differences.push(format!("{which} textures"));
        }
    }
    Ok(differences)
}

/// Whether the two optional sides of one `which` carry different textures.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a reference into a missing sidedef.
fn side_pair_differs(
    map: &Map,
    a: Option<usize>,
    b: Option<usize>,
    which: &'static str,
) -> Result<bool, CyberError> {
    let (Some(first), Some(second)) = (a, b) else {
        return Ok(false);
    };
    let one = sidedef_of(map, first, which)?;
    let other = sidedef_of(map, second, which)?;
    Ok(one.upper != other.upper || one.lower != other.lower || one.middle != other.middle)
}

/// The survivor side whose anchor is the absorbed vertex (its back when it
/// is the head, its front when it is the tail), shifted off by the removed
/// line's length, and range-checked like every binary offset.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a dangling side reference, and
/// [`CyberError::InvalidArgument`] when the shifted offset leaves the
/// binary range.
fn shifted_side(
    map: &Map,
    first: &Linedef,
    order: Order,
    removed_length: i32,
) -> Result<SideShift, CyberError> {
    let reference = match order {
        Order::FirstHead => first.back,
        Order::SecondHead => first.front,
    };
    let Some(index) = reference else {
        return Ok(None);
    };
    let side = sidedef_of(
        map,
        index,
        if order == Order::FirstHead {
            "back"
        } else {
            "front"
        },
    )?;
    let shifted = side.x_offset.wrapping_sub(removed_length);
    check_offset(map.format, "joined side", shifted)?;
    Ok(Some((index, shifted)))
}

/// Everything [`join_linedefs`] will do, checked against `map` first.
///
/// # Errors
///
/// Every refusal [`join_linedefs`] reports.
fn plan_join(map: &Map, first: usize, second: usize, force: bool) -> Result<JoinPlan, CyberError> {
    let first_line = linedef_row(map, first)?;
    let second_line = linedef_row(map, second)?;
    let order = join_order(&first_line, &second_line)?;
    let (shared, far) = join_ends(order, &first_line, &second_line);
    let ends = (endpoint(map, first_line.v1)?, endpoint(map, first_line.v2)?);
    let next_ends = (
        endpoint(map, second_line.v1)?,
        endpoint(map, second_line.v2)?,
    );
    require_same_direction(ends, next_ends)?;
    require_matching_sectors(map, &first_line, &second_line)?;
    require_vertex_is_theirs_alone(map, shared, first, second)?;
    let differences = join_differences(map, &first_line, &second_line)?;
    if !differences.is_empty() && !force {
        return Err(CyberError::Conflict(format!(
            "the two linedefs differ in {}: a join keeps the first linedef's properties and \
             loses the second's; pass force: true to accept that, or even the lines out \
             with edit_linedef first",
            differences.join(", ")
        )));
    }
    let removed_length = piece_length(next_ends.0, next_ends.1)?;
    let shift = shifted_side(map, &first_line, order, removed_length)?;
    Ok(JoinPlan {
        first,
        second,
        shared,
        far,
        head_is_first: order == Order::FirstHead,
        shift,
    })
}

/// Runs a validated [`plan_join`]: retargets the survivor, shifts its moved
/// side, then drops the second linedef, its orphaned sides and the absorbed
/// vertex through the removal engine.
///
/// # Errors
///
/// [`CyberError::Internal`] from [`apply_removal`] when the map already
/// dangles a reference into the removal set.
fn apply_join(map: &mut Map, plan: JoinPlan) -> Result<JoinResult, CyberError> {
    if let Some(line) = map.linedefs.get_mut(plan.first) {
        if plan.head_is_first {
            line.v2 = plan.far;
        } else {
            line.v1 = plan.far;
        }
    }
    if let Some((index, offset)) = plan.shift {
        if let Some(side) = map.sidedefs.get_mut(index) {
            side.x_offset = offset;
        }
    }
    let seed = Removal {
        linedefs: BTreeSet::from([plan.second]),
        vertices: BTreeSet::from([plan.shared]),
        ..Default::default()
    };
    let grown = expand_cascade(map, &seed, CascadeOptions::default());
    let removed = apply_removal(map, &grown)?;
    let shifted_off = removed
        .linedefs
        .iter()
        .filter(|index| **index < plan.first)
        .count();
    Ok(JoinResult {
        linedef: plan.first.saturating_sub(shifted_off),
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::{OFFSET_MIN, Point, find_vertex, half_sum, join_linedefs, midpoint, split_linedef};
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{name, vertex};
    use crate::wad::map::{Linedef, Map, Sector, Sidedef};

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A point at `x`/`y`.
    const fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// A sidedef with `x_offset`, a `middle` texture and a sector.
    fn side(x_offset: i32, middle: &str, sector: usize) -> Sidedef {
        Sidedef {
            x_offset,
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: middle.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A line from `v1` to `v2` with the given sides and flags.
    fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1_u32,
            front,
            back,
            ..Default::default()
        }
    }

    /// The split fixture: one line `(0,0)` to `(256,0)`, front sidedef 0
    /// (`x_offset` 10, sector 0) and back sidedef 1 (`x_offset` 5,
    /// sector 1), with a tag and a special to copy.
    fn split_fixture(format: MapFormat) -> Map {
        Map {
            name: name("MAP01"),
            format,
            vertices: vec![vertex(0, 0), vertex(256, 0)],
            linedefs: vec![Linedef {
                special: 9,
                tag: 3,
                flags: 0x104,
                ..line(0, 1, Some(0), Some(1))
            }],
            sidedefs: vec![side(10, "WALL1", 0), side(5, "-", 1)],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    /// A map for joining: vertices `(0,0)`, `(128,0)`, `(256,0)` and lines
    /// head-to-tail along the run, each fronted by its own sector-0 side.
    fn join_fixture(format: MapFormat) -> Map {
        Map {
            name: name("MAP01"),
            format,
            vertices: vec![vertex(0, 0), vertex(128, 0), vertex(256, 0)],
            linedefs: vec![line(0, 1, Some(0), None), line(1, 2, Some(1), None)],
            sidedefs: vec![side(0, "WALL1", 0), side(40, "WALL1", 0)],
            sectors: vec![Sector::default()],
            ..Default::default()
        }
    }

    #[test]
    fn half_sum_rounds_halves_away_from_zero() {
        assert_eq!(half_sum(0, 256), 128_i32, "an even sum halves exactly");
        assert_eq!(half_sum(0, 255), 128_i32, "255/2 rounds up to nearest");
        assert_eq!(
            half_sum(0, -255),
            -128_i32,
            "and a negative half rounds away from zero"
        );
        assert_eq!(half_sum(-3, -2), -3_i32, "(-5)/2 is -2.5, away from zero");
    }

    #[test]
    fn midpoint_is_the_rounded_average_of_both_axes() {
        assert_eq!(
            midpoint(point(0, 0), point(256, 256)),
            point(128, 128),
            "even"
        );
        assert_eq!(
            midpoint(point(-256, -1), point(0, 2)),
            point(-128, 1),
            "odd, negative"
        );
    }

    #[test]
    fn split_horizontal_at_midpoint_cuts_the_line_and_shifts_the_sides() {
        let mut map = split_fixture(MapFormat::Doom);
        let result = split_linedef(&mut map, 0, None).expect("the cut lands");
        assert_eq!(result.vertex, 2, "a new vertex at the midpoint");
        assert_eq!(result.new_linedef, 1, "the second half is appended");
        assert_eq!(
            result.new_sidedefs,
            vec![2_usize, 3],
            "front copy, then back copy"
        );
        let first = map.linedefs.first().expect("the survivor");
        assert_eq!((first.v1, first.v2), (0_usize, 2), "v1 keeps the start");
        let second = map.linedefs.get(1).expect("the second half");
        assert_eq!(
            (second.v1, second.v2),
            (2_usize, 1),
            "it runs midpoint to old end"
        );
        assert_eq!(
            (second.flags, second.special, second.tag),
            (0x104_u32, 9_i32, 3_i32),
            "properties copy"
        );
        assert_eq!(second.front, Some(2), "and it fronts with its copy");
        assert_eq!(second.back, Some(3), "and backs with its copy");
        let mid = map.vertices.get(2).expect("the new vertex");
        assert_eq!((mid.x, mid.y), (128_i32, 0_i32), "at the midpoint");
        assert_eq!(
            map.sidedefs.first().expect("side 0").x_offset,
            10_i32,
            "the original front stays"
        );
        assert_eq!(
            map.sidedefs.get(2).expect("front copy").x_offset,
            138_i32,
            "shifted by 128"
        );
        assert_eq!(
            map.sidedefs.get(2).expect("front copy").middle,
            "WALL1",
            "and dressed alike"
        );
        assert_eq!(
            map.sidedefs.get(1).expect("back original").x_offset,
            133_i32,
            "5 + 128"
        );
        assert_eq!(
            map.sidedefs.get(3).expect("back copy").x_offset,
            5_i32,
            "the copy takes the old offset"
        );
        assert_eq!(
            map.sidedefs.get(3).expect("back copy").sector,
            1,
            "over the back's sector"
        );
    }

    #[test]
    fn split_diagonal_at_midpoint_shifts_by_the_floored_hypot() {
        let mut map = split_fixture(MapFormat::Doom);
        map.vertices = vec![vertex(0, 0), vertex(256, 256)];
        let result = split_linedef(&mut map, 0, None).expect("the cut lands");
        let mid = map.vertices.get(result.vertex).expect("the midpoint");
        assert_eq!((mid.x, mid.y), (128_i32, 128_i32), "each axis halved");
        assert_eq!(
            map.sidedefs.get(2).expect("front copy").x_offset,
            10_i32 + 181_i32,
            "hypot(128,128) floors to 181, so the front shifts by 181"
        );
        assert_eq!(
            map.sidedefs.get(1).expect("back original").x_offset,
            5_i32 + 181_i32,
            "and the back shifts by the other 181-unit piece"
        );
        assert_eq!(
            map.sidedefs.get(3).expect("back copy").x_offset,
            5_i32,
            "the copy keeps the old offset at the untouched end"
        );
    }

    #[test]
    fn split_at_a_given_point_uses_it_and_reuses_the_vertex_there() {
        let mut map = split_fixture(MapFormat::Doom);
        map.vertices.push(vertex(64, 0));
        let result = split_linedef(&mut map, 0, Some(point(64, 0))).expect("an off-centre cut");
        assert_eq!(
            result.vertex, 2,
            "the existing vertex at exactly that point"
        );
        assert_eq!(map.vertices.len(), 3, "and nothing new came in");
        assert_eq!(
            map.sidedefs.get(2).expect("front copy").x_offset,
            10_i32 + 64_i32,
            "shifted by the first piece only"
        );
        assert_eq!(
            map.sidedefs.get(1).expect("back original").x_offset,
            5_i32 + 192_i32,
            "the back shifts by the second piece"
        );
    }

    #[test]
    fn split_tolerates_a_point_exactly_one_unit_off_the_line() {
        let mut map = split_fixture(MapFormat::Doom);
        let result = split_linedef(&mut map, 0, Some(point(128, 1))).expect("cross^2 == length^2");
        let mid = map.vertices.get(result.vertex).expect("the split vertex");
        assert_eq!(
            (mid.x, mid.y),
            (128_i32, 1_i32),
            "the point stands as given"
        );
    }

    #[test]
    fn split_refuses_points_off_beyond_and_on_the_endpoints() {
        let refusals = [
            point(128, 5),
            point(0, 0),
            point(256, 0),
            point(300, 0),
            point(-4, 0),
        ];
        for given in refusals {
            let mut map = split_fixture(MapFormat::Doom);
            let error = split_linedef(&mut map, 0, Some(given)).expect_err("must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal for {given:?}");
            assert_eq!(
                map,
                split_fixture(MapFormat::Doom),
                "the refusal changed nothing"
            );
        }
    }

    #[test]
    fn split_refuses_a_zero_length_line_and_its_rounded_onto_endpoint() {
        let mut flat = split_fixture(MapFormat::Doom);
        flat.vertices = vec![vertex(0, 0), vertex(0, 0)];
        let error = split_linedef(&mut flat, 0, None).expect_err("nothing to cut");
        assert_eq!(code(&error), "invalid_argument", "zero-length code");
        assert!(
            error.to_string().contains("zero-length"),
            "the message names the rule: {error}"
        );
        let mut tiny = split_fixture(MapFormat::Doom);
        tiny.vertices = vec![vertex(0, 0), vertex(1, 0)];
        let short = split_linedef(&mut tiny, 0, None).expect_err("the midpoint rounds onto v2");
        assert_eq!(code(&short), "invalid_argument", "one-unit line code");
    }

    #[test]
    fn split_refuses_offsets_a_binary_map_cannot_store() {
        let mut map = split_fixture(MapFormat::Doom);
        map.sidedefs.get_mut(0).expect("the front").x_offset = 32_700_i32;
        let error = split_linedef(&mut map, 0, None).expect_err("32828 is over i16");
        assert_eq!(code(&error), "invalid_argument", "front offset code");
        assert!(
            error.to_string().contains("front"),
            "the message names the side: {error}"
        );
        let mut back_map = split_fixture(MapFormat::Doom);
        back_map.sidedefs.get_mut(0).expect("the front").x_offset = 0_i32;
        back_map.sidedefs.get_mut(1).expect("the back").x_offset = 32_700_i32;
        let back_error =
            split_linedef(&mut back_map, 0, None).expect_err("the back shifts too far");
        assert_eq!(code(&back_error), "invalid_argument", "back offset code");
        assert!(
            back_error.to_string().contains("back"),
            "the message names the side: {back_error}"
        );
        assert_eq!(back_map, split_fixture_back_only(), "nothing shifted");
    }

    /// The split fixture with the front unshifted but the back at 32700.
    fn split_fixture_back_only() -> Map {
        let mut map = split_fixture(MapFormat::Doom);
        map.sidedefs.get_mut(0).expect("the front").x_offset = 0_i32;
        map.sidedefs.get_mut(1).expect("the back").x_offset = 32_700_i32;
        map
    }

    #[test]
    fn split_wraps_offsets_past_i32_on_udmf_without_range_checks() {
        let mut map = split_fixture(MapFormat::Udmf);
        map.sidedefs.get_mut(0).expect("the front").x_offset = 0x7FFF_FFF8_i32;
        let result = split_linedef(&mut map, 0, None).expect("UDMF does not range-check offsets");
        let copy = map
            .sidedefs
            .get(result.new_sidedefs.first().copied().expect("a front copy"))
            .expect("the copy");
        assert_eq!(
            copy.x_offset,
            0x7FFF_FFF8_i32.wrapping_add(128_i32),
            "the offset wraps as the ticket prescribes"
        );
    }

    #[test]
    fn split_refuses_a_point_a_binary_map_cannot_store() {
        let mut map = split_fixture(MapFormat::Doom);
        map.vertices = vec![vertex(40_000, 0), vertex(60_000, 0)];
        let error = split_linedef(&mut map, 0, None).expect_err("the midpoint is over i16");
        assert_eq!(code(&error), "invalid_argument", "coordinate code");
        assert!(
            error.to_string().contains("vertices hold"),
            "the message names the range: {error}"
        );
    }

    #[test]
    fn split_refuses_pieces_too_long_to_shift_an_offset() {
        let mut map = split_fixture(MapFormat::Udmf);
        map.vertices = vec![vertex(i32::MIN, 0), vertex(i32::MAX, 0)];
        let error = split_linedef(&mut map, 0, Some(point(i32::MIN + 2, 0)))
            .expect_err("the second piece is past i32::MAX");
        assert_eq!(code(&error), "invalid_argument", "length code");
        assert!(
            error.to_string().contains("past what an x_offset"),
            "the message names the rule: {error}"
        );
    }

    #[test]
    fn split_one_sided_lines_copy_only_the_front() {
        let mut map = split_fixture(MapFormat::Doom);
        map.linedefs.get_mut(0).expect("the line").back = None;
        map.sidedefs.pop();
        let result = split_linedef(&mut map, 0, None).expect("the cut lands");
        assert_eq!(result.new_sidedefs, vec![1_usize], "one copy for one side");
        let second = map.linedefs.get(1).expect("the second half");
        assert_eq!(second.front, Some(1), "it keeps a front");
        assert_eq!(second.back, None, "and no back");
    }

    #[test]
    fn split_reports_missing_lines_vertices_and_side_references() {
        let mut map = split_fixture(MapFormat::Doom);
        let gone = split_linedef(&mut map, 9, None).expect_err("linedef 9 is not there");
        assert_eq!(code(&gone), "not_found", "missing line code");
        map.linedefs.get_mut(0).expect("the line").v2 = 99;
        let dangling = split_linedef(&mut map, 0, None).expect_err("vertex 99 is not there");
        assert_eq!(code(&dangling), "not_found", "missing vertex code");
        let mut sided = split_fixture(MapFormat::Doom);
        sided.linedefs.get_mut(0).expect("the line").back = Some(9);
        let sideless = split_linedef(&mut sided, 0, None).expect_err("sidedef 9 is not there");
        assert_eq!(code(&sideless), "not_found", "missing sidedef code");
        assert!(
            sideless.to_string().contains("back"),
            "the message names the side: {sideless}"
        );
    }

    #[test]
    fn split_copies_the_udmf_extra_bag_onto_the_new_line() {
        let mut map = split_fixture(MapFormat::Udmf);
        map.linedefs
            .get_mut(0)
            .expect("the line")
            .extra
            .insert("autoflow".to_owned(), "1".to_owned());
        let result = split_linedef(&mut map, 0, None).expect("the cut lands");
        let second = map.linedefs.get(result.new_linedef).expect("the half");
        assert_eq!(
            second.extra.get("autoflow").map(String::as_str),
            Some("1"),
            "the extra bag rides along"
        );
    }

    #[test]
    fn join_of_the_two_halves_of_a_split_restores_the_map_exactly() {
        let original = split_fixture(MapFormat::Doom);
        let mut map = original.clone();
        let split = split_linedef(&mut map, 0, None).expect("the cut lands");
        let joined = join_linedefs(&mut map, 0, split.new_linedef, false).expect("the halves weld");
        assert_eq!(joined.linedef, 0, "the survivor stays first");
        assert_eq!(
            joined.removed.linedefs,
            vec![1_usize],
            "the second half went"
        );
        assert_eq!(
            joined.removed.sidedefs,
            vec![2_usize, 3],
            "its copies went with it"
        );
        assert_eq!(
            joined.removed.vertices,
            vec![2_usize],
            "and so did the cut point"
        );
        assert_eq!(map, original, "endpoints, sides and every offset are back");
    }

    #[test]
    fn join_of_a_reversed_pair_survives_first_and_shifts_its_front() {
        let mut map = join_fixture(MapFormat::Doom);
        let joined = join_linedefs(&mut map, 1, 0, false).expect("the tail survives the weld");
        assert_eq!(joined.linedef, 0, "the survivor renumbered down");
        let survivor = map.linedefs.first().expect("one line left");
        assert_eq!(
            (survivor.v1, survivor.v2),
            (0_usize, 1),
            "it spans 0,0 to 256,0"
        );
        assert_eq!(
            map.sidedefs.len(),
            1,
            "the removed head's orphaned side went"
        );
        assert_eq!(
            map.sidedefs.first().expect("the survivor's front").x_offset,
            40_i32 - 128_i32,
            "its front re-anchors 128 units back"
        );
    }

    #[test]
    fn join_keeps_the_shared_vertex_when_a_third_line_uses_it() {
        let mut map = join_fixture(MapFormat::Doom);
        map.linedefs.push(line(1, 2, Some(0), None));
        let error = join_linedefs(&mut map, 0, 1, false).expect_err("vertex 1 has a third user");
        assert_eq!(code(&error), "conflict", "third line code");
        assert!(
            error.to_string().contains("linedef 2"),
            "the message names the blocker: {error}"
        );
    }

    #[test]
    fn join_refuses_pairs_that_meet_nowhere_backwards_or_off_line() {
        let mut apart = join_fixture(MapFormat::Doom);
        apart.vertices.push(vertex(512, 0));
        apart.linedefs.get_mut(1).expect("the tail").v1 = 3;
        apart.linedefs.get_mut(1).expect("the tail").v2 = 2;
        let nowhere = join_linedefs(&mut apart, 0, 1, false).expect_err("no head-to-tail meet");
        assert_eq!(code(&nowhere), "invalid_argument", "no shared vertex");
        assert!(
            nowhere.to_string().contains("head to tail"),
            "the message names the rule: {nowhere}"
        );

        let mut both_ends = join_fixture(MapFormat::Doom);
        both_ends.linedefs.get_mut(1).expect("the tail").v2 = 0;
        let folded = join_linedefs(&mut both_ends, 0, 1, false).expect_err("both ends shared");
        assert_eq!(code(&folded), "invalid_argument", "both-endpoints refusal");
        assert!(
            folded.to_string().contains("both endpoints"),
            "the message names the rule: {folded}"
        );

        let mut angle = join_fixture(MapFormat::Doom);
        angle.vertices.get_mut(2).expect("the far vertex").y = 128_i32;
        let crooked = join_linedefs(&mut angle, 0, 1, false).expect_err("a corner, not a run");
        assert_eq!(code(&crooked), "invalid_argument", "collinearity refusal");
        assert!(
            crooked.to_string().contains("collinear"),
            "the message names the rule: {crooked}"
        );

        let mut backwards = join_fixture(MapFormat::Doom);
        backwards.vertices = vec![vertex(0, 0), vertex(128, 0), vertex(64, 0)];
        let against = join_linedefs(&mut backwards, 0, 1, false).expect_err("the tail runs back");
        assert_eq!(code(&against), "invalid_argument", "direction refusal");
        assert!(
            against.to_string().contains("opposite"),
            "the message names the rule and the fix: {against}"
        );
    }

    #[test]
    fn join_refuses_sides_that_open_onto_different_sectors() {
        let mut map = join_fixture(MapFormat::Doom);
        map.sidedefs.get_mut(1).expect("the tail's front").sector = 1;
        map.sectors.push(Sector::default());
        let error = join_linedefs(&mut map, 0, 1, false).expect_err("front sectors disagree");
        assert_eq!(code(&error), "invalid_argument", "front sector code");
        assert!(
            error.to_string().contains("front sides"),
            "the message names the side: {error}"
        );
        let mut closed = join_fixture(MapFormat::Doom);
        closed.linedefs.get_mut(1).expect("the tail").back = Some(2);
        closed.sidedefs.push(side(0, "-", 0));
        let back = join_linedefs(&mut closed, 0, 1, false).expect_err("one line is two-sided");
        assert_eq!(code(&back), "invalid_argument", "back side code");
        assert!(
            back.to_string().contains("back sides"),
            "the message names the side: {back}"
        );
    }

    #[test]
    fn join_conflicts_naming_every_difference_and_force_lets_the_first_win() {
        let mut map = join_fixture(MapFormat::Doom);
        map.linedefs.get_mut(1).expect("the tail").flags = 4_u32;
        map.linedefs.get_mut(1).expect("the tail").special = 7_i32;
        map.sidedefs.get_mut(1).expect("the tail's front").middle = "BRICK".to_owned();
        let error = join_linedefs(&mut map, 0, 1, false).expect_err("the pair differs");
        assert_eq!(code(&error), "conflict", "differences code");
        let message = error.to_string();
        assert!(
            message.contains("flags")
                && message.contains("special")
                && message.contains("front textures"),
            "the message lists every difference: {message}"
        );
        assert!(
            message.contains("force: true"),
            "and names the fix: {message}"
        );
        let joined = join_linedefs(&mut map, 0, 1, true).expect("force welds them");
        assert_eq!(joined.linedef, 0_usize, "the first linedef survived");
        let survivor = map.linedefs.first().expect("one line");
        assert_eq!(survivor.flags, 1_u32, "its flags won");
        assert_eq!(survivor.special, 0_i32, "as did its special");
        assert_eq!(
            map.sidedefs.first().expect("its front"),
            &side(0, "WALL1", 0),
            "and its textures"
        );
    }

    #[test]
    fn force_join_welds_a_split_pair_losing_only_the_second_tags_value() {
        let mut map = split_fixture(MapFormat::Doom);
        let split = split_linedef(&mut map, 0, None).expect("the cut lands");
        map.linedefs
            .get_mut(split.new_linedef)
            .expect("the half")
            .tag = 9_i32;
        let joined =
            join_linedefs(&mut map, 0, split.new_linedef, true).expect("force over the tag");
        assert_eq!(
            joined.removed.linedefs,
            vec![split.new_linedef],
            "the half went"
        );
        assert_eq!(
            map,
            split_fixture(MapFormat::Doom),
            "the tag conflict cost the second half's tag"
        );
    }

    #[test]
    fn join_refuses_offsets_a_binary_map_cannot_store() {
        let mut map = join_fixture(MapFormat::Doom);
        map.linedefs.get_mut(0).expect("the head").back = Some(2);
        map.linedefs.get_mut(1).expect("the tail").back = Some(3);
        map.sidedefs.push(side(OFFSET_MIN, "-", 0));
        map.sidedefs.push(side(0, "-", 0));
        let error = join_linedefs(&mut map, 0, 1, false).expect_err("-32768 - 128 is out");
        assert_eq!(code(&error), "invalid_argument", "joined offset code");
        assert!(
            error.to_string().contains("joined side"),
            "the message names the shift: {error}"
        );
    }

    #[test]
    fn join_reports_missing_lines_and_vertices_as_not_found() {
        let mut map = join_fixture(MapFormat::Doom);
        let error = join_linedefs(&mut map, 0, 9, false).expect_err("linedef 9 is not there");
        assert_eq!(code(&error), "not_found", "missing line code");
        map.linedefs.get_mut(1).expect("the tail").v2 = 99;
        let gone = join_linedefs(&mut map, 0, 1, false).expect_err("vertex 99 is not there");
        assert_eq!(code(&gone), "not_found", "missing vertex code");
    }

    #[test]
    fn join_of_one_sided_and_missing_side_pairs_passes_quietly() {
        let mut map = join_fixture(MapFormat::Doom);
        let joined = join_linedefs(&mut map, 0, 1, false).expect("heads: no backs to disagree");
        assert_eq!(joined.linedef, 0, "the head survived unchanged in place");
        assert_eq!(map.linedefs.len(), 1, "one line covers the run");
    }

    #[test]
    fn find_vertex_finds_only_an_exact_coordinate_match() {
        let map = split_fixture(MapFormat::Doom);
        assert_eq!(find_vertex(&map, point(256, 0)), Some(1), "an exact hit");
        assert_eq!(find_vertex(&map, point(256, 1)), None, "one off is a miss");
    }
}
