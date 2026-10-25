//! Placing one connection: the boundary it goes on, the refusals and
//! the layers it contributes.
//!
//! [`Pass::place`] is the whole step for one [`Connection`]: it resolves
//! the rooms through [`resolve_target`], picks the shared piece with
//! [`best_boundary`] (the nearest to [`Connection::at`], or the longest,
//! ties by `(a, b)`), checks the width, centres the [`Frame`] on the
//! placement point and asks [`body_for`](Pass::body_for) for the layers
//! per kind, handing the result back as a [`ConnectorGeometry`]. A plain
//! opening whose band spans the run end to end owns no band at all:
//! [`joined_opening`] records its [`WallKey`] and
//! [`final_layers`](super::final_layers) drops the wall there.

use super::frame::{
    Body, Frame, StairPlan, delta, gap_depth, plan_stairs, rounded_unit, stair_edge,
};
use super::{
    Boundary, ConnectorGeometry, LabelAllocator, Target, best_boundary, resolve_target,
    span_height, window_span,
};
use crate::error::CyberError;
use crate::fixed;
use crate::geom::Layer;
use crate::geom::Pt;
use crate::geom::label::Label;
use crate::plan::model::{Connection, ConnectionKind, Plan};
use crate::plan::walls::{WallKey, Walls};

/// The room pair key into [`Walls::shared`]: plan indices, lower first.
fn pair_key(one: usize, two: usize) -> (u32, u32) {
    let (first, second) = if one < two { (one, two) } else { (two, one) };
    (
        u32::try_from(first).unwrap_or(u32::MAX),
        u32::try_from(second).unwrap_or(u32::MAX),
    )
}

/// The error for rooms with no shared boundary piece.
fn not_touching(target: &Target<'_>) -> CyberError {
    CyberError::InvalidArgument(format!(
        "connection '{}' cannot be placed: rooms {} and {} do not touch; move them together or add a corridor room",
        target.label, target.connection.from, target.connection.to
    ))
    .with_hint(PLAN_ROOMS_HINT)
}

/// The hint attached to the plan refusals about rooms that cannot meet.
const PLAN_ROOMS_HINT: &str = "move the rooms so they share an edge, or add a corridor room";

/// Whether `candidate` beats `best`: nearer to `at`, or longer when `at`
/// is `None`; ties break by the piece's `(a, b)` order.
pub(super) fn preferred(candidate: &Boundary<'_>, best: &Boundary<'_>, at: Option<Pt>) -> bool {
    at.map_or_else(
        || longer(candidate, best),
        |point| nearer(candidate, best, point),
    )
}

/// Whether the candidate boundary is longer, or equal and sorts first.
fn longer(candidate: &Boundary<'_>, best: &Boundary<'_>) -> bool {
    let (mine, theirs) = (candidate.length_squared, best.length_squared);
    mine > theirs || (mine == theirs && sorts_first(candidate, best))
}

/// Whether the candidate boundary is nearer to `point`, or equal and
/// sorts first.
fn nearer(candidate: &Boundary<'_>, best: &Boundary<'_>, point: Pt) -> bool {
    let mine = distance_squared(candidate, point);
    let theirs = distance_squared(best, point);
    mine < theirs || (mine == theirs && sorts_first(candidate, best))
}

/// Whether the candidate piece has the smaller `(a, b)`.
fn sorts_first(candidate: &Boundary<'_>, best: &Boundary<'_>) -> bool {
    (candidate.piece.a, candidate.piece.b) < (best.piece.a, best.piece.b)
}

/// Squared `i128` distance from `point` to the piece, projection clamped
/// to the segment; no floats, saturating at the `i128` extremes.
pub(super) fn distance_squared(boundary: &Boundary<'_>, point: Pt) -> i128 {
    let dx = i128::from(boundary.dx);
    let dy = i128::from(boundary.dy);
    let fx = delta(boundary.piece.a.0, point.0);
    let fy = delta(boundary.piece.a.1, point.1);
    let along = fx.saturating_mul(dx).saturating_add(fy.saturating_mul(dy));
    if boundary.length_squared == 0 || along <= 0 {
        return fx.saturating_mul(fx).saturating_add(fy.saturating_mul(fy));
    }
    if along >= boundary.length_squared {
        let gx = fx.saturating_sub(dx);
        let gy = fy.saturating_sub(dy);
        return gx.saturating_mul(gx).saturating_add(gy.saturating_mul(gy));
    }
    let cross = dx.saturating_mul(fy).saturating_sub(dy.saturating_mul(fx));
    cross
        .saturating_mul(cross)
        .checked_div(boundary.length_squared)
        .unwrap_or(i128::MAX)
}

/// The point the band is centred on: the nearest point of the piece to
/// `at`, or the piece's [`fixed::midpoint`]-rounded midpoint.
fn placement_point(boundary: &Boundary<'_>, at: Option<Pt>) -> Pt {
    at.map_or_else(|| midpoint(boundary), |point| projected(boundary, point))
}

/// The piece's midpoint, each coordinate rounded through
/// [`fixed::midpoint`].
pub(super) fn midpoint(boundary: &Boundary<'_>) -> Pt {
    let piece = boundary.piece;
    (
        fixed::midpoint(piece.a.0, piece.b.0),
        fixed::midpoint(piece.a.1, piece.b.1),
    )
}

/// The clamped projection of `point` onto the piece, rounded to integers
/// in `i128`.
fn projected(boundary: &Boundary<'_>, point: Pt) -> Pt {
    let piece = boundary.piece;
    if boundary.length_squared == 0 {
        return piece.a;
    }
    let dx = i128::from(boundary.dx);
    let dy = i128::from(boundary.dy);
    let fx = delta(piece.a.0, point.0);
    let fy = delta(piece.a.1, point.1);
    let along = fx.saturating_mul(dx).saturating_add(fy.saturating_mul(dy));
    if along <= 0 {
        return piece.a;
    }
    if along >= boundary.length_squared {
        return piece.b;
    }
    (
        piece.a.0.saturating_add(fixed::div_round_sat(
            along.saturating_mul(dx),
            boundary.length_squared,
        )),
        piece.a.1.saturating_add(fixed::div_round_sat(
            along.saturating_mul(dy),
            boundary.length_squared,
        )),
    )
}

/// The error when a connection is wider than the boundary under it.
fn fit_width(boundary: &Boundary<'_>, target: &Target<'_>) -> Result<(), CyberError> {
    let width = target.connection.width;
    let wanted = i128::from(width).saturating_mul(i128::from(width));
    if boundary.length_squared >= wanted {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "connection '{}' is {width} units wide but the boundary between rooms {} and {} is {} units long; move the rooms further apart or narrow the connection",
        target.label, target.connection.from, target.connection.to, boundary.length
    ))
    .with_hint(PLAN_ROOMS_HINT))
}

/// The wall one connection opens end to end: `Some` only for a plain
/// [`ConnectionKind::Opening`] whose band spans the whole shared run.
/// Such an opening is not an intervention in a wall, it is the absence of
/// one: the band, the wall it would cut and the rooms' own boundary all
/// coincide, so [`final_layers`](super::final_layers) drops the wall and
/// the rooms join with a two-sided line of their own, with no band sector and
/// no jamb faces.
/// A [`ConnectionKind::Door`], [`ConnectionKind::Window`] or
/// [`ConnectionKind::Stairs`] always keeps its bands, however wide: each
/// needs a sector of its own to hold the leaf, the glass or the steps.
fn joined_opening(target: &Target<'_>, frame: &Frame, boundary: &Boundary<'_>) -> Option<WallKey> {
    if !matches!(target.connection.kind, ConnectionKind::Opening) {
        return None;
    }
    if !frame.spans(boundary, target.connection.width) {
        return None;
    }
    Some(WallKey {
        pair: pair_key(target.from, target.to),
        piece: (boundary.piece.a, boundary.piece.b),
    })
}

/// One connection being placed by a [`Pass`].
pub(super) struct Pass<'plan, 'walls> {
    /// The plan being compiled.
    pub(super) plan: &'plan Plan,
    /// The walls its rooms produced.
    pub(super) walls: &'walls Walls,
    /// The label counter, threaded through the whole pass.
    pub(super) labels: LabelAllocator,
}

impl<'plan, 'walls> Pass<'plan, 'walls> {
    /// Place one connection: pick the boundary piece, check the width,
    /// build its layers and hand it back as a [`ConnectorGeometry`].
    pub(super) fn place(
        &mut self,
        connection: &'plan Connection,
        index: usize,
    ) -> Result<ConnectorGeometry, CyberError> {
        let target = resolve_target(self.plan, connection)?;
        let boundary = self.boundary_for(&target)?;
        fit_width(&boundary, &target)?;
        let centre = placement_point(&boundary, connection.at);
        let frame = Frame::new(
            &boundary,
            centre,
            u32::try_from(target.from).unwrap_or(u32::MAX),
        );
        let joined = joined_opening(&target, &frame, &boundary);
        let body = self.body_for(&target, &frame, joined)?;
        Ok(ConnectorGeometry {
            connection_index: index,
            id: target.label,
            from: target.from,
            to: target.to,
            kind: connection.kind,
            layers: body.layers,
            joined,
            band: rounded_unit(frame.dx, frame.dy, frame.length),
            normal: rounded_unit(frame.nx, frame.ny, frame.length),
            lower_room: body.lower_room,
            step_floors: body.step_floors,
            height: body.height,
        })
    }

    /// The shared piece the connection goes on: the nearest to
    /// [`Connection::at`], or the longest when no point is given; ties
    /// break by the piece's `(a, b)` order.
    fn boundary_for(&self, target: &Target<'plan>) -> Result<Boundary<'walls>, CyberError> {
        self.walls
            .shared
            .get(&pair_key(target.from, target.to))
            .and_then(|pieces| best_boundary(pieces, target.connection.at))
            .ok_or_else(|| not_touching(target))
    }

    /// The layers and heights of one placed connection, per kind. A
    /// `joined` opening contributes no layer: its band would be the wall
    /// it replaces, so [`final_layers`](super::final_layers) leaves the
    /// wall out and the two rooms meet on their own boundary.
    fn body_for(
        &mut self,
        target: &Target<'plan>,
        frame: &Frame,
        joined: Option<WallKey>,
    ) -> Result<Body, CyberError> {
        if joined.is_some() {
            return Ok(Body {
                layers: Vec::new(),
                lower_room: None,
                step_floors: Vec::new(),
                height: Some(span_height(target)),
            });
        }
        match target.connection.kind {
            ConnectionKind::Opening | ConnectionKind::Door { .. } => {
                let height = span_height(target);
                Ok(self.band_body(target, frame, height))
            }
            ConnectionKind::Window { sill, lintel } => {
                let height = window_span(target, sill, lintel)?;
                Ok(self.band_body(target, frame, height))
            }
            ConnectionKind::Stairs {
                step_height,
                step_depth,
            } => {
                let run = plan_stairs(target, step_height, step_depth)?;
                Ok(self.stairs_layers(target, frame, &run))
            }
        }
    }

    /// The one-band body: a region rectangle `width` along the boundary
    /// centred on the placement point, `depth` across it centred on the
    /// boundary line.
    fn band_body(&mut self, target: &Target<'plan>, frame: &Frame, height: (i32, i32)) -> Body {
        let base = self.labels.take(1_usize);
        let depth = gap_depth(self.plan.wall_thickness);
        Body {
            layers: vec![Layer {
                label: Label::Region(base),
                polygon: frame.band(target.connection.width, depth.saturating_neg(), depth),
            }],
            lower_room: None,
            step_floors: Vec::new(),
            height: Some(height),
        }
    }

    /// The stacked-step body: `run.steps` bands of `step_depth` each,
    /// ordered from the lower room upwards, the whole run centred on the
    /// boundary line except for the extra depth beyond the wall void,
    /// pushed to the lower room's side.
    fn stairs_layers(&mut self, target: &Target<'plan>, frame: &Frame, run: &StairPlan) -> Body {
        let base = self.labels.take(run.steps);
        let void = self.plan.wall_thickness.max(0_i64);
        let run_depth = i64::try_from(run.steps)
            .unwrap_or(i64::MAX)
            .saturating_mul(run.step_depth);
        let extra = run_depth.saturating_sub(void).max(0_i64);
        let back = void.saturating_add(extra.saturating_mul(2_i64));
        let side: i64 = if run.lower_room == target.from {
            -1_i64
        } else {
            1_i64
        };
        let width = target.connection.width;
        let layers = (0..run.steps)
            .map(|step| Layer {
                label: Label::Region(base.saturating_add(u32::try_from(step).unwrap_or(u32::MAX))),
                polygon: frame.band(
                    width,
                    stair_edge(back, side, step, run.step_depth),
                    stair_edge(back, side, step.saturating_add(1), run.step_depth),
                ),
            })
            .collect();
        Body {
            layers,
            lower_room: Some(run.lower_room),
            step_floors: run.step_floors.clone(),
            height: None,
        }
    }
}
