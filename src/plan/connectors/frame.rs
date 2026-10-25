//! The stair run and the band frame: the shapes a connector's layers are
//! built in, with the integer helpers they measure through.
//!
//! [`plan_stairs`] turns a [`Target`]'s two floors into a [`StairPlan`]:
//! the lower room, the step count and each step's floor. [`Frame`] is the
//! rectangle every band is drawn in: the placement point on the chosen
//! [`Boundary`], the piece direction and the `from -> to` perpendicular,
//! both of magnitude `length`, so [`Frame::band`] can round a `width`
//! wide, `depth` deep band to four integer corners with no float in
//! sight and [`Frame::spans`] can tell whether that band reaches past
//! both ends of the run. [`Body`] carries the layers, step floors and
//! span of one placed connection back to the [`Pass`](super::Pass).

use super::{Boundary, MAX_STEP_HEIGHT, MAX_STEPS, Target, ZERO_THICKNESS_DEPTH};
use crate::error::CyberError;
use crate::fixed::{mul_div_round_sat, narrow_i32, narrow_i64};
use crate::geom::Layer;
use crate::geom::Pt;
use crate::geom::label::Label;

/// The plan of one stair run: which room is lower and what its steps are.
pub(super) struct StairPlan {
    /// Plan-order index of the lower room.
    pub(super) lower_room: usize,
    /// The step count.
    pub(super) steps: usize,
    /// The step depth, map units; positive.
    pub(super) step_depth: i64,
    /// Each step's floor, ordered from the lower room upwards, clamped
    /// to the higher room's floor.
    pub(super) step_floors: Vec<i32>,
}

/// Work out one stair run. `step_height` clamps to `1..=24`: `0` and
/// negative ask for one-unit steps, taller for [`MAX_STEP_HEIGHT`]; the
/// last step's floor never passes the higher room's floor.
pub(super) fn plan_stairs(
    target: &Target<'_>,
    step_height: i32,
    step_depth: i32,
) -> Result<StairPlan, CyberError> {
    let from_floor = i64::from(target.from_room.floor);
    let to_floor = i64::from(target.to_room.floor);
    let difference = to_floor.saturating_sub(from_floor);
    if difference == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "connection '{}' has stairs between rooms whose floors are level; floors are level; use an opening",
            target.label
        )));
    }
    let lower_room = if difference > 0 {
        target.from
    } else {
        target.to
    };
    let lower = from_floor.min(to_floor);
    let higher = from_floor.max(to_floor);
    let height = i64::from(i32::clamp(step_height, 1_i32, MAX_STEP_HEIGHT));
    let steps = count_steps(higher.saturating_sub(lower), height, &target.label)?;
    Ok(StairPlan {
        lower_room,
        steps,
        step_depth: i64::from(step_depth.max(1_i32)),
        step_floors: step_floors(lower, higher, height, steps),
    })
}

/// `ceil(rise / height)` as a step count, refusing more than
/// [`MAX_STEPS`] steps.
fn count_steps(rise: i64, height: i64, label: &str) -> Result<usize, CyberError> {
    let raw = rise
        .saturating_add(height)
        .saturating_sub(1_i64)
        .checked_div(height)
        .unwrap_or(i64::MAX);
    let steps = usize::try_from(raw).unwrap_or(usize::MAX);
    if steps > MAX_STEPS {
        return Err(CyberError::LimitExceeded(format!(
            "connection '{label}' needs {raw} steps of height {height}; at most {MAX_STEPS} are supported; raise the step height or shrink the floor difference"
        )));
    }
    Ok(steps)
}

/// Each step's floor: one `step_height` higher than the last, counted
/// from the lower room's floor, clamped so the top step never passes the
/// higher room's floor.
fn step_floors(lower: i64, higher: i64, height: i64, steps: usize) -> Vec<i32> {
    (0..steps)
        .map(|step| {
            let rise = i64::try_from(step.saturating_add(1)).unwrap_or(i64::MAX);
            narrow_i32(i128::from(
                lower
                    .saturating_add(height.saturating_mul(rise))
                    .min(higher),
            ))
        })
        .collect()
}

/// The layers, step floors and heights one connection contributes.
pub(super) struct Body {
    /// The band layers, bottom first.
    pub(super) layers: Vec<Layer>,
    /// Stairs only: the lower room.
    pub(super) lower_room: Option<usize>,
    /// Stairs only: each step's floor.
    pub(super) step_floors: Vec<i32>,
    /// Openings, doors and windows only: the clear span.
    pub(super) height: Option<(i32, i32)>,
}

/// The frame a connector band is built in: the centre point on the
/// boundary line and the two integer numerators, the piece direction and
/// the `from -> to` perpendicular, both of magnitude `length`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    /// The placement point, rounded to integers.
    pub(super) centre: Pt,
    /// Piece direction components.
    pub(super) dx: i64,
    /// Piece direction components.
    pub(super) dy: i64,
    /// The piece's working length.
    pub(super) length: i64,
    /// Perpendicular components, from the `from` side to the `to` side.
    pub(super) nx: i64,
    /// Perpendicular components, from the `from` side to the `to` side.
    pub(super) ny: i64,
}

impl Frame {
    /// The frame of `boundary` at `centre`, with the perpendicular
    /// oriented from the room `from` (a plan index) towards the other
    /// side.
    pub(super) fn new(boundary: &Boundary<'_>, centre: Pt, from: u32) -> Self {
        let flipped = boundary.piece.left != Label::Region(from);
        Self {
            centre,
            dx: boundary.dx,
            dy: boundary.dy,
            length: boundary.length,
            nx: if flipped {
                boundary.dy.saturating_neg()
            } else {
                boundary.dy
            },
            ny: if flipped {
                boundary.dx
            } else {
                boundary.dx.saturating_neg()
            },
        }
    }

    /// The four corners of a band `width` along the boundary, centred on
    /// the frame point, across it between the doubled coordinates `low`
    /// and `high` (map units times two, signed along the `from -> to`
    /// perpendicular). A zero-length boundary collapses to the centre
    /// point, which [`arrange`](fn@crate::geom::arrange) refuses as
    /// degenerate.
    pub(super) fn band(&self, width: i64, low: i64, high: i64) -> Vec<Pt> {
        if self.length == 0 {
            return vec![self.centre; 4];
        }
        let along = self.along(width);
        let near = scale_offset(low, (self.nx, self.ny), self.denominator());
        let far = scale_offset(high, (self.nx, self.ny), self.denominator());
        let back = negated(along);
        vec![
            corner(self.centre, along, near),
            corner(self.centre, back, near),
            corner(self.centre, back, far),
            corner(self.centre, along, far),
        ]
    }

    /// Whether a band of `width` centred on the frame point spans
    /// `boundary` end to end: the rounded along-extent [`Frame::band`]
    /// gives its corners reaches past both of the piece's ends.
    pub(super) fn spans(&self, boundary: &Boundary<'_>, width: i64) -> bool {
        if self.length == 0 {
            return false;
        }
        let along = self.along(width);
        let start = corner(self.centre, negated(along), (0_i64, 0_i64));
        let stop = corner(self.centre, along, (0_i64, 0_i64));
        let dx = i128::from(self.dx);
        let dy = i128::from(self.dy);
        along_at(start, boundary.piece.a, dx, dy) <= 0
            && along_at(stop, boundary.piece.a, dx, dy) >= boundary.length_squared
    }

    /// Twice the piece's length: the denominator every band offset
    /// divides by.
    const fn denominator(&self) -> i64 {
        self.length.saturating_mul(2_i64)
    }

    /// The band's half-extent vector along the piece for a `width` wide
    /// opening.
    fn along(&self, width: i64) -> Pt {
        scale_offset(width, (self.dx, self.dy), self.denominator())
    }
}

/// `(point - from)` dotted with the piece direction: `0` at the piece's
/// `a` end, its squared length at the `b` end and clamped nowhere.
fn along_at(point: Pt, from: Pt, dx: i128, dy: i128) -> i128 {
    delta(from.0, point.0)
        .saturating_mul(dx)
        .saturating_add(delta(from.1, point.1).saturating_mul(dy))
}

/// `value * numerator / denominator` per component, rounded through
/// [`mul_div_round_sat`].
fn scale_offset(value: i64, numerator: Pt, denominator: i64) -> Pt {
    (
        mul_div_round_sat(value, numerator.0, denominator),
        mul_div_round_sat(value, numerator.1, denominator),
    )
}

/// `point` plus both offsets.
const fn corner(point: Pt, along: Pt, across: Pt) -> Pt {
    (
        point.0.saturating_add(along.0).saturating_add(across.0),
        point.1.saturating_add(along.1).saturating_add(across.1),
    )
}

/// Both components negated, saturating at the `i64` extremes.
const fn negated(offset: Pt) -> Pt {
    (offset.0.saturating_neg(), offset.1.saturating_neg())
}

/// The doubled across-coordinate of stair edge `step`: the run's back
/// edge sits `back` (doubled) units on the lower room's side of the
/// line (`side` is `-1` when the lower room is the `from` room, `1`
/// when it is the `to` room) and edges towards the line by twice the
/// step depth per step.
pub(super) fn stair_edge(back: i64, side: i64, step: usize, step_depth: i64) -> i64 {
    let count = i64::try_from(step).unwrap_or(i64::MAX);
    let gone = i128::from(count).saturating_mul(i128::from(step_depth).saturating_mul(2_i128));
    narrow_i64(
        i128::from(back)
            .saturating_sub(gone)
            .saturating_mul(i128::from(side)),
    )
}

/// The band depth of an opening through a wall of `thickness`: the
/// thickness when positive, [`ZERO_THICKNESS_DEPTH`] otherwise.
pub(super) const fn gap_depth(thickness: i64) -> i64 {
    if thickness > 0 {
        thickness
    } else {
        ZERO_THICKNESS_DEPTH
    }
}

/// The rounded unit vector of a numerator, or `(0, 0)` when there is no
/// length to divide by.
pub(super) fn rounded_unit(x: i64, y: i64, length: i64) -> Pt {
    if length == 0 {
        return (0_i64, 0_i64);
    }
    (
        mul_div_round_sat(1_i64, x, length),
        mul_div_round_sat(1_i64, y, length),
    )
}

/// `to - from` in `i128`.
pub(super) fn delta(from: i64, to: i64) -> i128 {
    i128::from(to).saturating_sub(i128::from(from))
}
