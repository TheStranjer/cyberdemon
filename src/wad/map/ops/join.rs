//! The join pass: how two linedefs meet, what a weld loses and how the
//! removal engine runs it, behind [`join_linedefs`](super::join_linedefs).
//!
//! [`join_order`] and [`join_ends`] settle which end of which linedef is
//! the shared one, [`require_same_direction`], [`require_matching_sectors`]
//! and [`require_vertex_is_theirs_alone`] refuse a pair that is not one
//! straight run of wall with the same faces and no third line hanging on
//! the joint, and [`join_differences`] names what a plain weld would lose.
//! [`plan_join`] folds those checks and the [`shifted_side`] of the
//! survivor into a [`JoinPlan`](super::JoinPlan), and [`apply_join`] writes
//! it: retarget the survivor, shift its moved side and hand the second
//! linedef, its orphaned sides and the absorbed vertex to the removal
//! engine.

use alloc::collections::BTreeSet;

use crate::error::CyberError;

use super::split::piece_length;
use super::{
    JoinPlan, JoinResult, Linedef, Map, Order, Point, SideShift, check_offset, delta, endpoint,
    linedef_row, side_sector, sidedef_of, wide,
};
use crate::wad::map::remove::{CascadeOptions, Removal, apply_removal, expand_cascade};

/// The order the two linedefs meet in: which end of which is shared.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when neither pair of facing endpoints
/// meets (no shared head-to-tail vertex, or both endpoints shared).
pub(super) fn join_order(first: &Linedef, second: &Linedef) -> Result<Order, CyberError> {
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
pub(super) const fn join_ends(order: Order, first: &Linedef, second: &Linedef) -> (usize, usize) {
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
pub(super) fn require_same_direction(
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
pub(super) fn require_matching_sectors(
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
pub(super) fn require_vertex_is_theirs_alone(
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
pub(super) fn join_differences(
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
pub(super) fn side_pair_differs(
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
pub(super) fn shifted_side(
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

/// Everything [`join_linedefs`](super::join_linedefs) will do, checked against `map` first.
///
/// # Errors
///
/// Every refusal [`join_linedefs`](super::join_linedefs) reports.
pub(super) fn plan_join(
    map: &Map,
    first: usize,
    second: usize,
    force: bool,
) -> Result<JoinPlan, CyberError> {
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
pub(super) fn apply_join(map: &mut Map, plan: JoinPlan) -> Result<JoinResult, CyberError> {
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
