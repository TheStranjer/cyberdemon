//! Connector placement: where each [`Connection`] cuts the wall.
//!
//! A [`Connection`] between two rooms is placed against the
//! [`shared_boundaries`](crate::plan::walls::shared_boundaries) pieces
//! [`build_walls`](crate::plan::walls::build_walls) found for that room
//! pair: the nearest piece to [`Connection::at`], or the longest piece
//! when no point is given. Around the placement point [`plan_connectors`]
//! paints one [`Label::Region`] band (openings, doors, windows) or a
//! stack of step bands (stairs) that [`final_layers`] puts above the wall
//! voids, so the next [`arrange`](fn@crate::geom::arrange) cuts the
//! opening clean through the wall. The region labels it allocates (past
//! the room labels) are how the map emitter of the next slice recognises
//! connector faces. Nothing here touches archives, sessions or `MCP`
//! types; it is integer geometry and measurement only.

use crate::error::CyberError;
use crate::fixed::{isqrt, mul_div_round};
use crate::geom::label::Label;
use crate::geom::{Layer, Piece, Pt};
use crate::plan::model::{Connection, ConnectionKind, Plan, Room};
use crate::plan::walls::{Walls, room_layers};

/// Band depth used for an opening when [`Plan::wall_thickness`] is not
/// positive: the rectangle then overlaps both rooms by half its depth on
/// each side of the shared line, and
/// [`arrange`](fn@crate::geom::arrange)'s cuts turn its edges into the
/// proper shared linedefs there.
const ZERO_THICKNESS_DEPTH: i64 = 16_i64;

/// Smallest usable window glass height, map units.
const MIN_GLASS_HEIGHT: i64 = 16_i64;

/// Largest accepted [`ConnectionKind::Stairs::step_height`], map units;
/// taller requests clamp down to this.
const MAX_STEP_HEIGHT: i32 = 24_i32;

/// Upper bound on the steps of one stair run; keeps the emitted layer
/// count sane for absurd floor differences.
const MAX_STEPS: usize = 4_096_usize;

/// Everything the map emitter needs to know about one placed connection.
///
/// [`ConnectorGeometry::kind`] rides along unchanged: a
/// [`ConnectionKind::Door`] has exactly the same geometry as a plain
/// [`ConnectionKind::Opening`] (one band through the wall); which door
/// special to put on the linedef is
/// [`DoorStyle`](crate::plan::model::DoorStyle)'s business, not this
/// module's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorGeometry {
    /// Index of the connection in [`Plan::connections`].
    pub connection_index: usize,
    /// The connection's id, or `from->to` when it is unnamed.
    pub id: String,
    /// Plan-order index of the room the connection starts from.
    pub from: usize,
    /// Plan-order index of the room the connection leads to.
    pub to: usize,
    /// The connection's kind, copied from the model.
    pub kind: ConnectionKind,
    /// The band layers to paint above the room and wall layers, bottom
    /// step first for stairs, one band otherwise.
    pub layers: Vec<Layer>,
    /// Rounded unit direction along the chosen boundary piece, from its
    /// `a` towards its `b`.
    pub band: Pt,
    /// Rounded unit normal from the `from` room's side of the boundary
    /// towards the `to` room's side.
    pub normal: Pt,
    /// For stairs, the plan-order index of the lower room; `None` for
    /// every other kind.
    pub lower_room: Option<usize>,
    /// For stairs, the floor of each step, ordered from the lower room
    /// upwards; empty for every other kind.
    pub step_floors: Vec<i32>,
    /// The clear vertical span: `floor = max(room floors)` and
    /// `ceiling = min(room ceilings)` for openings and doors;
    /// `floor = max(room floors) + sill` and
    /// `ceiling = min(room ceilings) - lintel` for windows. Stairs carry
    /// their floors in [`ConnectorGeometry::step_floors`] instead and
    /// leave this `None`.
    pub height: Option<(i32, i32)>,
}

impl ConnectorGeometry {
    /// The region label of this connection's `step`-th layer: stair
    /// steps count from the lower room upwards, and every other kind has
    /// the single band at `0`. `None` when there is no such layer.
    ///
    /// [`final_layers`] and [`arrange`](fn@crate::geom::arrange) put this
    /// number in [`Piece::left`] / [`Piece::right`] of every piece the
    /// connector cuts, which is how the map emitter finds a connector's
    /// faces again.
    #[must_use]
    #[inline]
    pub fn label_of(&self, step: usize) -> Option<u32> {
        self.layers
            .get(step)
            .map(|layer| layer.label)
            .and_then(region_id)
    }
}

/// The `u32` of a [`Label::Region`], `None` for [`Label::Void`].
const fn region_id(label: Label) -> Option<u32> {
    match label {
        Label::Region(id) => Some(id),
        Label::Void => None,
    }
}

/// Place every connection of `plan` against `walls`, in plan order.
///
/// Rooms own the labels `Region(0..rooms.len())`; a `LabelAllocator`
/// starting just past them hands consecutive labels to each connection's
/// layers, so a stair run with several steps gets one label per step
/// ([`ConnectorGeometry::label_of`] reads them back). `plan` should have
/// passed [`Plan::validate`] first; this pass still refuses connections
/// that name unknown rooms and boundaries too short for the connection's
/// [`Connection::width`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when:
/// - a connection names a room that is not in the plan;
/// - the two rooms share no boundary piece ("do not touch");
/// - the chosen boundary is shorter than the connection's width;
/// - a window's computed span is under `16` units of glass;
/// - stairs join floors that are level ("use an opening").
///
/// [`CyberError::LimitExceeded`] when a stair run would need more than
/// `4096` steps.
#[inline]
pub fn plan_connectors(plan: &Plan, walls: &Walls) -> Result<Vec<ConnectorGeometry>, CyberError> {
    let mut pass = Pass {
        plan,
        walls,
        labels: LabelAllocator::new(plan.rooms.len()),
    };
    let mut placed = Vec::with_capacity(plan.connections.len());
    for (index, connection) in plan.connections.iter().enumerate() {
        placed.push(pass.place(connection, index)?);
    }
    Ok(placed)
}

/// The complete painter's-algorithm stack: room layers, then the wall
/// void layers, then every connector layer, bottom first.
///
/// Connectors sit above the wall voids so their regions paint through
/// them; [`arrange`](fn@crate::geom::arrange) consumes the stack in this
/// bottom-first order with a
/// [`LayerLabeler`](crate::geom::LayerLabeler) over it.
#[must_use]
#[inline]
pub fn final_layers(plan: &Plan, walls: &Walls, connectors: &[ConnectorGeometry]) -> Vec<Layer> {
    let mut stack = room_layers(plan);
    stack.extend(walls.layers.iter().cloned());
    for connector in connectors {
        stack.extend(connector.layers.iter().cloned());
    }
    stack
}

/// The next-free-region-label counter, threaded through one
/// [`plan_connectors`] pass.
#[derive(Clone, Copy, Debug)]
struct LabelAllocator {
    /// The label the next layer will get.
    next: u32,
}

impl LabelAllocator {
    /// An allocator handing out labels starting just past `room_count`
    /// region labels.
    #[inline]
    fn new(room_count: usize) -> Self {
        Self {
            next: u32::try_from(room_count).unwrap_or(u32::MAX),
        }
    }

    /// Takes `count` consecutive labels and returns the first one.
    #[inline]
    fn take(&mut self, count: usize) -> u32 {
        let base = self.next;
        let width = u32::try_from(count).unwrap_or(u32::MAX);
        self.next = base.saturating_add(width);
        base
    }
}

/// The measured piece a connector is placed on.
struct Boundary<'walls> {
    /// The shared piece itself.
    piece: &'walls Piece,
    /// `b - a`, clamped to `i64`.
    dx: i64,
    /// `b - a`, clamped to `i64`.
    dy: i64,
    /// [`isqrt`] of `length_squared`, the working length.
    length: i64,
    /// Squared length in `i128`, exact for the comparisons.
    length_squared: i128,
}

/// Measure one shared piece.
fn measure(piece: &Piece) -> Boundary<'_> {
    let dx = clamp_i64(delta(piece.a.0, piece.b.0));
    let dy = clamp_i64(delta(piece.a.1, piece.b.1));
    let length_squared = i128::from(dx)
        .saturating_mul(i128::from(dx))
        .saturating_add(i128::from(dy).saturating_mul(i128::from(dy)));
    let root = isqrt(u64::try_from(length_squared).unwrap_or(u64::MAX));
    Boundary {
        piece,
        dx,
        dy,
        length: i64::try_from(root).unwrap_or(i64::MAX),
        length_squared,
    }
}

/// The best boundary of `pieces` for a connector placed at `at` (or at
/// no particular point); `None` for an empty slice.
fn best_boundary(pieces: &[Piece], at: Option<Pt>) -> Option<Boundary<'_>> {
    pieces.iter().map(measure).reduce(|best, candidate| {
        if preferred(&candidate, &best, at) {
            candidate
        } else {
            best
        }
    })
}

/// One connection with its endpoints resolved to plan rooms.
struct Target<'plan> {
    /// The connection being placed.
    connection: &'plan Connection,
    /// Plan-order index of the `from` room.
    from: usize,
    /// Plan-order index of the `to` room.
    to: usize,
    /// The `from` room.
    from_room: &'plan Room,
    /// The `to` room.
    to_room: &'plan Room,
    /// The connection's id, or `from->to` when unnamed.
    label: String,
}

/// Resolve one connection's room ids to plan rooms, or refuse it.
fn resolve_target<'plan>(
    plan: &'plan Plan,
    connection: &'plan Connection,
) -> Result<Target<'plan>, CyberError> {
    let label = connection_label(connection);
    let (from, from_room) = find_room(plan, &connection.from, &label)?;
    let (to, to_room) = find_room(plan, &connection.to, &label)?;
    Ok(Target {
        connection,
        from,
        to,
        from_room,
        to_room,
        label,
    })
}

/// The room with `id` and its plan index, or an error naming the
/// connection.
fn find_room<'plan>(
    plan: &'plan Plan,
    id: &str,
    label: &str,
) -> Result<(usize, &'plan Room), CyberError> {
    plan.rooms
        .iter()
        .enumerate()
        .find(|&(_, room)| room.id == id)
        .ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "connection '{label}' names unknown room '{id}'; add the room or fix the connection"
            ))
        })
}

/// The display name of a connection: its id, or `from->to`.
fn connection_label(connection: &Connection) -> String {
    connection
        .id
        .clone()
        .unwrap_or_else(|| format!("{}->{}", connection.from, connection.to))
}

/// The computed vertical span of an opening or door: the higher floor
/// to the lower ceiling, saturating into the `i32` range.
fn span_height(target: &Target<'_>) -> (i32, i32) {
    let floor = i64::from(target.from_room.floor).max(i64::from(target.to_room.floor));
    let ceiling = i64::from(target.from_room.ceiling).min(i64::from(target.to_room.ceiling));
    (clamp_i32(floor), clamp_i32(ceiling))
}

/// The glass span of a window: `max(floor) + sill` to
/// `min(ceiling) - lintel`; an error when it is under
/// [`MIN_GLASS_HEIGHT`] tall.
fn window_span(target: &Target<'_>, sill: i32, lintel: i32) -> Result<(i32, i32), CyberError> {
    let (base_floor, base_ceiling) = span_height(target);
    let floor = i64::from(base_floor).saturating_add(i64::from(sill));
    let ceiling = i64::from(base_ceiling).saturating_sub(i64::from(lintel));
    if ceiling.saturating_sub(floor) >= MIN_GLASS_HEIGHT {
        return Ok((clamp_i32(floor), clamp_i32(ceiling)));
    }
    Err(CyberError::InvalidArgument(format!(
        "connection '{}' has window floor {floor} and ceiling {ceiling}; that leaves under {MIN_GLASS_HEIGHT} units of glass; lower the sill or the lintel",
        target.label
    )))
}

/// The plan of one stair run: which room is lower and what its steps are.
struct StairPlan {
    /// Plan-order index of the lower room.
    lower_room: usize,
    /// The step count.
    steps: usize,
    /// The step depth, map units; positive.
    step_depth: i64,
    /// Each step's floor, ordered from the lower room upwards, clamped
    /// to the higher room's floor.
    step_floors: Vec<i32>,
}

/// Work out one stair run. `step_height` clamps to `1..=24`: `0` and
/// negative ask for one-unit steps, taller for [`MAX_STEP_HEIGHT`]; the
/// last step's floor never passes the higher room's floor.
fn plan_stairs(
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
            clamp_i32(
                lower
                    .saturating_add(height.saturating_mul(rise))
                    .min(higher),
            )
        })
        .collect()
}

/// The layers, step floors and heights one connection contributes.
struct Body {
    /// The band layers, bottom first.
    layers: Vec<Layer>,
    /// Stairs only: the lower room.
    lower_room: Option<usize>,
    /// Stairs only: each step's floor.
    step_floors: Vec<i32>,
    /// Openings, doors and windows only: the clear span.
    height: Option<(i32, i32)>,
}

/// The frame a connector band is built in: the centre point on the
/// boundary line and the two integer numerators, the piece direction and
/// the `from -> to` perpendicular, both of magnitude `length`.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// The placement point, rounded to integers.
    centre: Pt,
    /// Piece direction components.
    dx: i64,
    /// Piece direction components.
    dy: i64,
    /// The piece's working length.
    length: i64,
    /// Perpendicular components, from the `from` side to the `to` side.
    nx: i64,
    /// Perpendicular components, from the `from` side to the `to` side.
    ny: i64,
}

impl Frame {
    /// The frame of `boundary` at `centre`, with the perpendicular
    /// oriented from the room `from` (a plan index) towards the other
    /// side.
    fn new(boundary: &Boundary<'_>, centre: Pt, from: u32) -> Self {
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
    fn band(&self, width: i64, low: i64, high: i64) -> Vec<Pt> {
        if self.length == 0 {
            return vec![self.centre; 4];
        }
        let denominator = self.length.saturating_mul(2_i64);
        let along = scale_offset(width, (self.dx, self.dy), denominator);
        let near = scale_offset(low, (self.nx, self.ny), denominator);
        let far = scale_offset(high, (self.nx, self.ny), denominator);
        let back = negated(along);
        vec![
            corner(self.centre, along, near),
            corner(self.centre, back, near),
            corner(self.centre, back, far),
            corner(self.centre, along, far),
        ]
    }
}

/// `value * numerator / denominator` per component, rounded through
/// [`mul_div_round`].
fn scale_offset(value: i64, numerator: Pt, denominator: i64) -> Pt {
    (
        rounded(value, numerator.0, denominator),
        rounded(value, numerator.1, denominator),
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
fn stair_edge(back: i64, side: i64, step: usize, step_depth: i64) -> i64 {
    let count = i64::try_from(step).unwrap_or(i64::MAX);
    let gone = i128::from(count).saturating_mul(i128::from(step_depth).saturating_mul(2_i128));
    clamp_i64(
        i128::from(back)
            .saturating_sub(gone)
            .saturating_mul(i128::from(side)),
    )
}

/// The band depth of an opening through a wall of `thickness`: the
/// thickness when positive, [`ZERO_THICKNESS_DEPTH`] otherwise.
const fn gap_depth(thickness: i64) -> i64 {
    if thickness > 0 {
        thickness
    } else {
        ZERO_THICKNESS_DEPTH
    }
}

/// The rounded unit vector of a numerator, or `(0, 0)` when there is no
/// length to divide by.
fn rounded_unit(x: i64, y: i64, length: i64) -> Pt {
    if length == 0 {
        return (0_i64, 0_i64);
    }
    (rounded(1_i64, x, length), rounded(1_i64, y, length))
}

/// `value * numerator / denominator` through [`mul_div_round`],
/// saturating to the `i64` extreme in the product's direction.
fn rounded(value: i64, numerator: i64, denominator: i64) -> i64 {
    let fallback = if (value >= 0) == (numerator >= 0) {
        i64::MAX
    } else {
        i64::MIN
    };
    mul_div_round(value, numerator, denominator).unwrap_or(fallback)
}

/// The `i64` nearest `value`, saturating at the extremes.
fn clamp_i64(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// The `i32` nearest `value`, saturating at the extremes.
fn clamp_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

/// `to - from` in `i128`.
fn delta(from: i64, to: i64) -> i128 {
    i128::from(to).saturating_sub(i128::from(from))
}

/// `product / denominator` rounded half away from zero, saturating to
/// the `i64` range.
fn round_div(product: i128, denominator: i128) -> i64 {
    let Some(quotient) = product.checked_div(denominator) else {
        return if (product >= 0) == (denominator >= 0) {
            i64::MAX
        } else {
            i64::MIN
        };
    };
    let remainder = product.checked_rem(denominator).unwrap_or(0_i128);
    let doubled = remainder.checked_abs().and_then(|a| a.checked_mul(2));
    let magnitude = denominator.checked_abs().unwrap_or(i128::MAX);
    let step = if doubled.is_some_and(|twice| twice >= magnitude) {
        if (remainder >= 0) == (denominator >= 0) {
            1_i128
        } else {
            -1_i128
        }
    } else {
        0_i128
    };
    clamp_i64(quotient.saturating_add(step))
}

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
fn preferred(candidate: &Boundary<'_>, best: &Boundary<'_>, at: Option<Pt>) -> bool {
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
fn distance_squared(boundary: &Boundary<'_>, point: Pt) -> i128 {
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
/// `at`, or the piece's [`mul_div_round`]-rounded midpoint.
fn placement_point(boundary: &Boundary<'_>, at: Option<Pt>) -> Pt {
    at.map_or_else(|| midpoint(boundary), |point| projected(boundary, point))
}

/// The piece's midpoint, each coordinate rounded through
/// [`mul_div_round`].
fn midpoint(boundary: &Boundary<'_>) -> Pt {
    let piece = boundary.piece;
    (
        rounded_half(piece.a.0, piece.b.0),
        rounded_half(piece.a.1, piece.b.1),
    )
}

/// `(first + second) / 2` through [`mul_div_round`], the sum saturating
/// at the `i64` extremes.
fn rounded_half(first: i64, second: i64) -> i64 {
    let sum = clamp_i64(i128::from(first).saturating_add(i128::from(second)));
    rounded(sum, 1_i64, 2_i64)
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
        piece
            .a
            .0
            .saturating_add(round_div(along.saturating_mul(dx), boundary.length_squared)),
        piece
            .a
            .1
            .saturating_add(round_div(along.saturating_mul(dy), boundary.length_squared)),
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

/// One connection being placed by a [`Pass`].
struct Pass<'plan, 'walls> {
    /// The plan being compiled.
    plan: &'plan Plan,
    /// The walls its rooms produced.
    walls: &'walls Walls,
    /// The label counter, threaded through the whole pass.
    labels: LabelAllocator,
}

impl<'plan, 'walls> Pass<'plan, 'walls> {
    /// Place one connection: pick the boundary piece, check the width,
    /// build its layers and hand it back as a [`ConnectorGeometry`].
    fn place(
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
        let body = self.body_for(&target, &frame)?;
        Ok(ConnectorGeometry {
            connection_index: index,
            id: target.label,
            from: target.from,
            to: target.to,
            kind: connection.kind,
            layers: body.layers,
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

    /// The layers and heights of one placed connection, per kind.
    fn body_for(&mut self, target: &Target<'plan>, frame: &Frame) -> Result<Body, CyberError> {
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

#[cfg(test)]
mod tests {
    use super::{
        ConnectorGeometry, Frame, LabelAllocator, clamp_i32, clamp_i64, connection_label,
        distance_squared, final_layers, measure, midpoint, plan_connectors, rounded_unit,
    };
    use crate::error::CyberError;
    use crate::geom::label::{Label, LayerLabeler};
    use crate::geom::{Piece, Pt, arrange};
    use crate::plan::model::{Connection, ConnectionKind, DoorStyle, KeyColor, Plan, Room, Shape};
    use crate::plan::walls::{Walls, build_walls};

    /// A rectangular room with default sector properties.
    fn rect_room(id: &str, left_bottom: Pt, width_height: Pt) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: left_bottom.0,
                y: left_bottom.1,
                w: width_height.0,
                h: width_height.1,
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A polygon room with default sector properties.
    fn polygon_room(id: &str, points: Vec<Pt>) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Polygon { points },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A default-width opening between two rooms by id.
    fn opening(from: &str, to: &str) -> Connection {
        Connection {
            id: None,
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width: 64,
        }
    }

    /// A default stair run between two rooms by id.
    fn stairs(from: &str, to: &str) -> Connection {
        let mut climb = opening(from, to);
        climb.kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32,
        };
        climb
    }

    /// Two 128 x 128 rooms side by side, sharing the line x = 128.
    fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
            ],
            connections,
            features: Vec::new(),
            wall_thickness: thickness,
        }
    }

    /// Rooms sharing two disjoint runs on x = 128: a tall room `a` and
    /// a `C`-shaped room `b` whose notch separates the runs.
    fn notch_plan(connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 256)),
                polygon_room(
                    "b",
                    vec![
                        (128, 0),
                        (256, 0),
                        (256, 128),
                        (128, 128),
                        (128, 64),
                        (192, 64),
                        (192, 32),
                        (128, 32),
                    ],
                ),
            ],
            connections,
            features: Vec::new(),
            wall_thickness: 16,
        }
    }

    /// Raise room `b` (the plan's second room) to `floor`.
    fn raise_b(plan: &mut Plan, floor: i32) {
        if let Some(room) = plan.rooms.get_mut(1) {
            room.floor = floor;
        }
    }

    /// Build walls and connectors for a plan expected to succeed.
    #[track_caller]
    fn compile(plan: &Plan) -> (Walls, Vec<ConnectorGeometry>) {
        let walls = build_walls(plan).expect("the fixture rooms arrange");
        let connectors = plan_connectors(plan, &walls).expect("the connectors place");
        (walls, connectors)
    }

    /// The first connector of a plan that compiles.
    #[track_caller]
    fn first(plan: &Plan) -> ConnectorGeometry {
        compile(plan)
            .1
            .into_iter()
            .next()
            .expect("the plan places one connector")
    }

    /// Build walls and expect the connector pass to fail.
    #[track_caller]
    fn error_of(plan: &Plan) -> CyberError {
        let walls = build_walls(plan).expect("the fixture rooms arrange");
        plan_connectors(plan, &walls).expect_err("the connector pass must fail")
    }

    /// The piece between two endpoints, either direction.
    fn find_piece(pieces: &[Piece], one: Pt, two: Pt) -> Option<&Piece> {
        pieces
            .iter()
            .find(|piece| (piece.a == one && piece.b == two) || (piece.a == two && piece.b == one))
    }

    /// Assert the band rectangle centred on `centre` with half extent
    /// `half` (across, along) matching `Frame::band`'s corner order.
    #[track_caller]
    fn assert_band(polygon: &[Pt], centre: Pt, half: Pt, note: &str) {
        let (west, east) = (
            centre.0.saturating_sub(half.0),
            centre.0.saturating_add(half.0),
        );
        let (low, high) = (
            centre.1.saturating_sub(half.1),
            centre.1.saturating_add(half.1),
        );
        assert_eq!(
            polygon.to_vec(),
            vec![(west, high), (west, low), (east, low), (east, high)],
            "{note}"
        );
    }

    #[test]
    fn an_opening_cuts_one_centred_band_and_gaps_the_wall() {
        let plan = two_rooms(16, vec![opening("a", "b")]);
        plan.validate().expect("the plan validates");
        let (walls, connectors) = compile(&plan);
        assert_eq!(connectors.len(), 1, "one connection, one geometry");
        let connector = connectors.first().expect("the opening");
        assert_eq!(connector.connection_index, 0, "plan order");
        assert_eq!(connector.id, "a->b", "unnamed shows from->to");
        assert_eq!((connector.from, connector.to), (0, 1), "resolved indices");
        assert_eq!(connector.kind, ConnectionKind::Opening, "kind copied");
        assert_eq!(connector.band, (0, 1), "unit direction along the boundary");
        assert_eq!(connector.normal, (1, 0), "unit normal from a towards b");
        assert_eq!(connector.lower_room, None, "not stairs");
        assert!(connector.step_floors.is_empty(), "not stairs");
        assert_eq!(connector.height, Some((0_i32, 128_i32)), "opening span");
        assert_eq!(
            connector.label_of(0),
            Some(2),
            "labels start past the rooms"
        );
        assert_eq!(connector.label_of(1), None, "a single band has one label");
        assert_eq!(connector.layers.len(), 1, "exactly one connector layer");
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 64),
            (8, 32),
            "64 along y by 16 across x, centred on the boundary",
        );
        let stack = final_layers(&plan, &walls, &connectors);
        assert_eq!(stack.len(), 4, "two rooms, one wall, one band");
        assert_eq!(
            stack.last().expect("the connector paints topmost").label,
            Label::Region(2),
            "connectors sit above the wall voids"
        );
        let pieces = arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("the stack arranges");
        let west = find_piece(&pieces, (120, 32), (120, 96)).expect("west wall face in the gap");
        assert_eq!(west.left, Label::Region(0), "room a faces the opening");
        assert_eq!(
            west.right,
            Label::Region(2),
            "the connector label shows on both sides"
        );
        let east = find_piece(&pieces, (136, 32), (136, 96)).expect("east wall face in the gap");
        assert_eq!(
            east.left,
            Label::Region(2),
            "the connector label shows on both sides"
        );
        assert_eq!(east.right, Label::Region(1), "room b faces the opening");
        assert!(
            find_piece(&pieces, (128, 32), (128, 96)).is_none(),
            "the wall line has a gap where the opening is"
        );
    }

    #[test]
    fn a_door_gets_the_same_band_as_an_opening() {
        let mut door = opening("a", "b");
        door.kind = ConnectionKind::Door {
            style: DoorStyle::Locked(KeyColor::Red),
        };
        let connector = first(&two_rooms(16, vec![door]));
        assert_eq!(
            connector.layers.len(),
            1,
            "geometry identical to an opening"
        );
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 64),
            (8, 32),
            "the door band is the opening band",
        );
        assert_eq!(
            connector.height,
            Some((0_i32, 128_i32)),
            "the span rides along"
        );
        assert!(matches!(
            connector.kind,
            ConnectionKind::Door {
                style: DoorStyle::Locked(KeyColor::Red)
            }
        ));
    }

    #[test]
    fn at_selects_the_nearer_of_two_shared_pieces() {
        let mut near = opening("a", "b");
        near.at = Some((128, 16));
        near.width = 24;
        let connector = first(&notch_plan(vec![near]));
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 16),
            (8, 12),
            "the lower, nearer run wins",
        );
    }

    #[test]
    fn at_selects_the_farther_piece_from_the_other_end() {
        let mut far = opening("a", "b");
        far.at = Some((128, 110));
        far.width = 24;
        let connector = first(&notch_plan(vec![far]));
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 110),
            (8, 12),
            "the upper run wins for a point near it",
        );
    }

    #[test]
    fn without_at_the_longest_shared_piece_and_its_midpoint_win() {
        let connector = first(&notch_plan(vec![opening("a", "b")]));
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 96),
            (8, 32),
            "the 64-unit run, centred on its rounded midpoint",
        );
    }

    #[test]
    fn non_touching_rooms_are_refused_with_a_hint() {
        let plan = Plan {
            rooms: vec![
                rect_room("a", (0, 0), (64, 64)),
                rect_room("b", (200, 0), (64, 64)),
            ],
            connections: vec![opening("a", "b")],
            features: Vec::new(),
            wall_thickness: 16,
        };
        let error = error_of(&plan);
        assert_eq!(error.code(), "invalid_argument", "code");
        assert!(
            error
                .to_string()
                .contains("rooms a and b do not touch; move them together or add a corridor room"),
            "message carries the hint: {error}"
        );
    }

    #[test]
    fn a_connection_naming_an_unknown_room_is_refused() {
        let error = error_of(&two_rooms(16, vec![opening("a", "ghost")]));
        assert!(
            error.to_string().contains("unknown room 'ghost'"),
            "names the missing room: {error}"
        );
    }

    #[test]
    fn a_wider_than_the_boundary_connection_is_refused() {
        let mut wide = opening("a", "b");
        wide.id = Some("gate".to_owned());
        wide.width = 200;
        let error = error_of(&two_rooms(16, vec![wide]));
        assert_eq!(error.code(), "invalid_argument", "code");
        let message = error.to_string();
        assert!(
            message.contains("'gate'"),
            "names the connection: {message}"
        );
        assert!(message.contains("200"), "names the width: {message}");
        assert!(
            message.contains("128"),
            "names the boundary length: {message}"
        );
    }

    #[test]
    fn a_window_stores_its_glass_span() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 32);
        if let Some(room) = plan.rooms.get_mut(1) {
            room.ceiling = 200_i32;
        }
        let mut view = opening("a", "b");
        view.id = Some("w1".to_owned());
        view.kind = ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32,
        };
        plan.connections = vec![view];
        let connector = first(&plan);
        assert_eq!(
            connector.height,
            Some((64_i32, 104_i32)),
            "max floor 32 + sill 32, min ceiling 128 - lintel 24"
        );
        assert_eq!(connector.layers.len(), 1, "one region rectangle");
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 64),
            (8, 32),
            "the window band is placed like an opening",
        );
    }

    #[test]
    fn a_window_thin_under_the_sill_is_refused() {
        let mut low = opening("a", "b");
        low.id = Some("slit".to_owned());
        low.kind = ConnectionKind::Window {
            sill: 120_i32,
            lintel: 24_i32,
        };
        let error = error_of(&two_rooms(16, vec![low]));
        assert_eq!(error.code(), "invalid_argument", "code");
        let message = error.to_string();
        assert!(
            message.contains("'slit'"),
            "names the connection: {message}"
        );
        assert!(message.contains("120"), "names the floor: {message}");
        assert!(message.contains("104"), "names the ceiling: {message}");
    }

    #[test]
    fn stairs_stack_steps_into_the_lower_room() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 64);
        plan.connections = vec![stairs("a", "b")];
        let connector = first(&plan);
        assert_eq!(connector.lower_room, Some(0), "room a is the lower one");
        assert_eq!(
            connector.step_floors,
            vec![16_i32, 32_i32, 48_i32, 64_i32],
            "floors increase from the lower room"
        );
        assert_eq!(
            connector.height, None,
            "stairs carry step floors, not a span"
        );
        assert_eq!(connector.layers.len(), 4, "one region per step");
        assert_eq!(connector.label_of(0), Some(2), "first step, first label");
        assert_eq!(connector.label_of(3), Some(5), "labels run consecutively");
        assert_eq!(connector.label_of(4), None, "past the stack is nothing");
        assert_band(
            &connector.layers.first().expect("step 0").polygon,
            (52, 64),
            (12, 32),
            "step 0 spans x 40..64: the run reaches 88 units into room a",
        );
        assert_band(
            &connector.layers.last().expect("step 3").polygon,
            (124, 64),
            (12, 32),
            "step 3 spans x 112..136: out of the 96-unit run, 80 units lie inside the lower room",
        );
    }

    #[test]
    fn stair_step_floors_clamp_at_the_higher_floor() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 60);
        plan.connections = vec![stairs("a", "b")];
        let connector = first(&plan);
        assert_eq!(
            connector.step_floors,
            vec![16_i32, 32_i32, 48_i32, 60_i32],
            "the fourth step caps at room b's floor 60"
        );
    }

    #[test]
    fn stairs_downhill_run_into_the_from_room_side() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 64);
        plan.connections = vec![stairs("b", "a")];
        let connector = first(&plan);
        assert_eq!(
            connector.lower_room,
            Some(0),
            "room a is still the lower one"
        );
        assert_eq!(connector.normal, (-1, 0), "b to a faces west");
        assert_eq!(
            connector.step_floors,
            vec![16_i32, 32_i32, 48_i32, 64_i32],
            "still up from a"
        );
        assert_band(
            &connector.layers.first().expect("step 0").polygon,
            (52, 64),
            (12, 32),
            "the run recedes into the lower room a, on the -normal side",
        );
    }

    #[test]
    fn level_stairs_are_refused_and_a_steep_height_clamps() {
        let error = error_of(&two_rooms(16, vec![stairs("a", "b")]));
        assert!(
            error.to_string().contains("'a->b' has stairs")
                && error
                    .to_string()
                    .contains("floors are level; use an opening"),
            "level floors get the hint: {error}"
        );
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 64);
        let mut steep = stairs("a", "b");
        steep.kind = ConnectionKind::Stairs {
            step_height: 40_i32,
            step_depth: 24_i32,
        };
        plan.connections = vec![steep];
        let connector = first(&plan);
        assert_eq!(
            connector.step_floors,
            vec![24_i32, 48_i32, 64_i32],
            "step height clamps to 24"
        );
    }

    #[test]
    fn an_absurd_step_count_is_refused() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 100_000_000);
        let mut endless = stairs("a", "b");
        endless.kind = ConnectionKind::Stairs {
            step_height: 0_i32,
            step_depth: 24_i32,
        };
        plan.connections = vec![endless];
        let error = error_of(&plan);
        assert_eq!(error.code(), "limit_exceeded", "code");
        assert!(
            error.to_string().contains("at most 4096"),
            "names the budget: {error}"
        );
    }

    #[test]
    fn a_zero_thickness_opening_overlaps_both_rooms() {
        let plan = two_rooms(0, vec![opening("a", "b")]);
        let (walls, connectors) = compile(&plan);
        assert!(walls.layers.is_empty(), "no wall layers at thickness 0");
        let connector = connectors.first().expect("the opening");
        assert_band(
            &connector.layers.first().expect("band").polygon,
            (128, 64),
            (8, 32),
            "the depth-16 band straddles the shared line",
        );
        let stack = final_layers(&plan, &walls, &connectors);
        let pieces = arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("arranges");
        let west = find_piece(&pieces, (120, 32), (120, 96)).expect("cut into room a");
        assert_eq!(
            (west.left, west.right),
            (Label::Region(0), Label::Region(2)),
            "a's side"
        );
        let east = find_piece(&pieces, (136, 32), (136, 96)).expect("cut into room b");
        assert_eq!(
            (east.left, east.right),
            (Label::Region(2), Label::Region(1)),
            "b's side"
        );
    }

    #[test]
    fn the_pass_is_deterministic_and_labels_follow_the_rooms() {
        let mut plan = two_rooms(16, Vec::new());
        raise_b(&mut plan, 64);
        let mut view = opening("b", "a");
        view.id = Some("v".to_owned());
        view.kind = ConnectionKind::Window {
            sill: 20_i32,
            lintel: 8_i32,
        };
        view.at = Some((128, 16));
        view.width = 32;
        plan.connections = vec![stairs("a", "b"), view];
        let walls = build_walls(&plan).expect("walls build");
        let once = plan_connectors(&plan, &walls).expect("first pass");
        let twice = plan_connectors(&plan, &walls).expect("second pass");
        assert_eq!(once, twice, "same plan, identical layer stacks and labels");
        let climb = once.first().expect("stairs");
        let glass = once.get(1).expect("window");
        assert_eq!(climb.label_of(3), Some(5), "the stairs took labels 2..=5");
        assert_eq!(glass.label_of(0), Some(6), "the window follows the steps");
        assert_eq!(
            glass.height,
            Some((84_i32, 120_i32)),
            "max floor 64 + sill 20, min ceiling 128 - lintel 8"
        );
    }

    #[test]
    fn helpers_round_measure_and_clamp() {
        assert_eq!(clamp_i64(i128::from(i64::MAX)), i64::MAX, "in range");
        assert_eq!(clamp_i32(i64::from(i32::MAX)), i32::MAX, "in range");
        assert_eq!(
            clamp_i32(-i64::from(i32::MAX)),
            i32::MIN.saturating_add(1),
            "negative"
        );
        assert_eq!(rounded_unit(0, 128, 128), (0, 1), "exact unit");
        assert_eq!(
            rounded_unit(64, 64, 90),
            (1, 1),
            "a diagonal rounds to ones"
        );
        assert_eq!(
            rounded_unit(3, 0, 128),
            (0, 0),
            "a tiny numerator rounds to zero"
        );
        let mut allocator = LabelAllocator::new(3_usize);
        assert_eq!(allocator.take(2_usize), 3, "first pair of labels");
        assert_eq!(allocator.take(1_usize), 5, "continues past them");
        assert_eq!(connection_label(&opening("a", "b")), "a->b", "unnamed");
        let piece = Piece {
            a: (128, 0),
            b: (128, 128),
            left: Label::Region(0),
            right: Label::Region(1),
            origin: Vec::new(),
        };
        let boundary = measure(&piece);
        assert_eq!(
            (boundary.length, boundary.length_squared),
            (128, 0x4000),
            "measured"
        );
        assert_eq!(distance_squared(&boundary, (128, 64)), 0, "on the line");
        assert_eq!(distance_squared(&boundary, (124, 64)), 16, "beside it");
        assert_eq!(
            distance_squared(&boundary, (128, 200)),
            5184,
            "past the end"
        );
        assert_eq!(midpoint(&boundary), (128, 64), "rounded midpoint");
        let flat = Frame {
            centre: (0, 0),
            dx: 0,
            dy: 0,
            length: 0,
            nx: 0,
            ny: 0,
        };
        assert_eq!(
            flat.band(64, -16, 16),
            vec![(0, 0); 4],
            "a degenerate frame collapses"
        );
    }
}
