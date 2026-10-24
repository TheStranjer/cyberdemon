//! The void carve strip: a whole shared boundary thickened into one
//! polygon.
//!
//! [`void_strip`] orders the boundary lines into a connected vertex
//! chain with [`super::cut::span`], re-orients each line to follow its
//! neighbour, offsets every vertex by the bevelled mean of its
//! segments' half-thickness normals and walks one side forward and the
//! other back, so an L-shaped boundary carves as one clean hexagon and
//! a straight multi-line run merges into one full-length band.

use alloc::collections::VecDeque;

use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::geom::segment::{Delta, delta};
use crate::wad::map::model::Map;
use crate::wad::map::ops::Point;

use super::cut::span;
use super::{BoundaryStrip, ORIGIN, half_vector, invalid, length_of, moved, scale_shift, wide};

/// The shared boundary `boundaries` thickened into one carve strip:
/// every line offset half of `thickness` toward each side, joined into
/// one polygon for [`crate::wad::map::overlay::apply_overlay`].
///
/// The lines are ordered into a connected run (each re-oriented to
/// follow its neighbours); corners join by averaging the two segments'
/// offsets, so an L-shaped boundary carves cleanly and a straight run
/// of several lines merges into one full-length band. The polygon
/// spans the whole thickness.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `boundaries` is empty, when
/// `thickness` is not positive, when a linedef index or one of its
/// vertices does not resolve, when a line is zero-length, or when the
/// lines do not form one connected run: carve each disconnected
/// boundary run with its own call then.
#[inline]
pub fn void_strip(
    map: &Map,
    boundaries: &[usize],
    thickness: i32,
) -> Result<BoundaryStrip, CyberError> {
    let half = checked_half_thickness(thickness)?;
    let ordered = boundary_chain(map, boundaries)?;
    Ok(BoundaryStrip {
        polygon: strip_polygon(&ordered, half),
    })
}

/// Half of `thickness`, rounded to nearest (halves away from zero), so
/// the strip spans the full requested thickness across.
fn checked_half_thickness(thickness: i32) -> Result<i32, CyberError> {
    if thickness <= 0_i32 {
        return Err(invalid(
            "the void strip thickness must be a positive number of map units; the door \
             tools carve with 16",
        ));
    }
    let half =
        mul_div_round(i64::from(thickness), 1_i64, 2_i64).unwrap_or_else(|| i64::from(thickness));
    Ok(i32::try_from(half).unwrap_or(i32::MAX))
}

/// The boundary lines ordered into one connected vertex chain, each
/// re-oriented to follow its neighbour.
fn boundary_chain(map: &Map, boundaries: &[usize]) -> Result<VecDeque<Point>, CyberError> {
    if boundaries.is_empty() {
        return Err(invalid(
            "no boundary linedefs were given: pass the lines longest_boundary reports",
        ));
    }
    let mut open: Vec<(usize, Point, Point)> = Vec::with_capacity(boundaries.len());
    for position in boundaries {
        let (from, to) = span(map, *position, "boundary")?;
        if from == to {
            return Err(invalid(&format!(
                "boundary linedef {position} is zero-length: it offsets to nothing; \
                 leave it out of the boundary"
            )));
        }
        open.push((*position, from, to));
    }
    let (first, head, tail) = open.remove(0);
    let mut ordered: VecDeque<Point> = VecDeque::new();
    ordered.push_back(head);
    ordered.push_back(tail);
    let mut ends = (head, tail);
    while chain_step(&mut open, &mut ordered, &mut ends) {}
    if let Some(&(index, _, _)) = open.first() {
        return Err(invalid(&format!(
            "boundary linedef {index} does not touch the run started by linedef {first}: \
             the shared boundary is not one connected run; carve each run with its own \
             void_strip call"
        )));
    }
    Ok(ordered)
}

/// Extend the chain by any open line touching an end, preferring the
/// tail (a line may attach to either end in either orientation); false
/// when nothing touches the run.
fn chain_step(
    open: &mut Vec<(usize, Point, Point)>,
    ordered: &mut VecDeque<Point>,
    ends: &mut (Point, Point),
) -> bool {
    let (head, tail) = *ends;
    let position = open
        .iter()
        .position(|&(_, from, to)| from == tail || to == tail || from == head || to == head);
    let Some(found) = position else {
        return false;
    };
    let (_, from, to) = open.swap_remove(found);
    if from == tail {
        ordered.push_back(to);
        ends.1 = to;
    } else if to == tail {
        ordered.push_back(from);
        ends.1 = from;
    } else if from == head {
        ordered.push_front(to);
        ends.0 = to;
    } else {
        ordered.push_front(from);
        ends.0 = from;
    }
    true
}

/// The strip polygon of the ordered chain: every vertex offset by the
/// bevelled mean of its segments' half-thickness normals, one side
/// walked forward and the other back.
fn strip_polygon(ordered: &VecDeque<Point>, half: i32) -> Vec<Point> {
    let vertices: Vec<Point> = ordered.iter().copied().collect();
    let offsets: Vec<Delta> = vertices
        .iter()
        .zip(vertices.iter().skip(1))
        .map(|(from, to)| edge_offset(*from, *to, half))
        .collect();
    let bevels = bevelled(&offsets);
    let mut polygon: Vec<Point> = Vec::with_capacity(vertices.len().saturating_mul(2));
    for (vertex, bevel) in vertices.iter().zip(bevels.iter()) {
        polygon.push(moved(*vertex, *bevel));
    }
    for (vertex, bevel) in vertices.iter().zip(bevels.iter()).rev() {
        polygon.push(moved(
            *vertex,
            (bevel.0.saturating_neg(), bevel.1.saturating_neg()),
        ));
    }
    polygon
}

/// The chain vertex offsets: each interior vertex gets the mean of its
/// two segments' offsets, the ends keep their own.
fn bevelled(offsets: &[Delta]) -> Vec<Delta> {
    let Some(first) = offsets.first().copied() else {
        return Vec::new();
    };
    let mut bevels: Vec<Delta> = Vec::with_capacity(offsets.len().saturating_add(1));
    bevels.push(first);
    let mut previous = first;
    for current in offsets.iter().skip(1).copied() {
        bevels.push(half_vector(previous, current));
        previous = current;
    }
    bevels.push(offsets.last().copied().unwrap_or(first));
    bevels
}

/// The half-thickness offset of one chain segment: its front-side
/// perpendicular scaled to `half` map units, rounded.
fn edge_offset(from: Point, to: Point, half: i32) -> Delta {
    let run = delta(wide(from), wide(to));
    let perpendicular = (run.1, run.0.saturating_neg());
    let length = i128::from(length_of(run)).max(1_i128);
    let point = scale_shift(ORIGIN, perpendicular, i128::from(half), length);
    (i128::from(point.x), i128::from(point.y))
}
