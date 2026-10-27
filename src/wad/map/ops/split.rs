//! The split pass: where the cut lands, what it does to the two sides and
//! how the second half is written, behind
//! [`split_linedef`](super::split_linedef).
//!
//! [`split_point`] picks the point (`at` as given, else the rounded
//! midpoint) and [`require_on_segment`] refuses everything more than one
//! map unit off the line or not strictly between its endpoints.
//! [`plan_split`] then measures both pieces with [`piece_length`] and
//! builds the side copies [`front_copy`] and [`back_pieces`] imply, so
//! [`apply_split`] only writes: place the vertex, push the copies, shift
//! the surviving back and append the second linedef. Every refusal happens
//! in the plan, never in the writes.

use crate::error::CyberError;
use crate::geom::segment::{delta, distance_squared, dot, grid_hypot, orient};

use super::{
    Linedef, Map, Point, SideShift, Sidedef, SplitPlan, SplitResult, check_offset, endpoint,
    midpoint, off_segment, place_vertex, push_sidedef, sidedef_of,
};

/// The split point: `at` as given, else the midpoint (each coordinate
/// rounded to nearest). Either way [`require_on_segment`] checks it.
pub(super) fn split_point(at: Option<Point>, from: Point, to: Point) -> Point {
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
pub(super) fn require_on_segment(point: Point, from: Point, to: Point) -> Result<(), CyberError> {
    let squared = distance_squared(from, to);
    if squared == 0_i128 {
        return Err(off_segment("the linedef is zero-length"));
    }
    let perpendicular = orient(from, to, point);
    if perpendicular.saturating_mul(perpendicular) > squared {
        return Err(off_segment(&format!(
            "the point {}, {} is more than one map unit off the line",
            point.x, point.y
        )));
    }
    let along = dot(delta(from, to), delta(from, point));
    if along <= 0_i128 || along >= squared {
        return Err(off_segment(&format!(
            "the point {}, {} is not strictly between the endpoints",
            point.x, point.y
        )));
    }
    Ok(())
}

/// The integer length of the `from` to `to` piece (`hypot`, floored),
/// refused when no `i32` offset can be shifted by it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the piece is longer than
/// `i32::MAX` map units.
pub(super) fn piece_length(from: Point, to: Point) -> Result<i32, CyberError> {
    let length = grid_hypot(from, to);
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
pub(super) fn plan_split(map: &Map, line: &Linedef, point: Point) -> Result<SplitPlan, CyberError> {
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
pub(super) fn apply_split(
    map: &mut Map,
    index: usize,
    line: &Linedef,
    plan: SplitPlan,
) -> SplitResult {
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
