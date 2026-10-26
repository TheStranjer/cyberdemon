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
//! opening clean through the wall. A plain [`ConnectionKind::Opening`]
//! wide enough to span the whole shared run needs no band of its own: it
//! records that run's [`WallKey`] in [`ConnectorGeometry::joined`]
//! instead and [`final_layers`] drops the wall there, so the two rooms
//! simply share their boundary line. The region labels it allocates
//! (past the room labels) are how the map emitter of the next slice
//! recognises connector faces. Nothing here touches archives, sessions
//! or `MCP` types; it is integer geometry and measurement only.
//!
//! The shared constants, the placed [`ConnectorGeometry`], its label
//! allocator and the room and boundary lookups stay here beside
//! [`plan_connectors`]; the band frame, the stair run and the integer
//! helpers they measure through are the private `frame` module, the
//! boundary choice, the refusals and the one-connection pass the entry
//! point drives are the private `pass` module. Both are implementation
//! details of this module's two public entry points.

mod frame;
mod pass;

use alloc::collections::BTreeSet;

use crate::error::{CyberError, PlanRef};
use crate::fixed::{isqrt, narrow_i32, narrow_i64};
use crate::geom::label::Label;
use crate::geom::{Layer, Piece, Pt};
use crate::plan::model::{Connection, ConnectionKind, Plan, Room};
use crate::plan::walls::{WallKey, Walls, room_layers};
use crate::wad::map::consts::MAX_STEP_HEIGHT;
use frame::delta;
use pass::{Pass, preferred};

/// Band depth used for an opening when [`Plan::wall_thickness`] is not
/// positive: the rectangle then overlaps both rooms by half its depth on
/// each side of the shared line, and
/// [`arrange`](fn@crate::geom::arrange)'s cuts turn its edges into the
/// proper shared linedefs there.
const ZERO_THICKNESS_DEPTH: i64 = 16_i64;

/// Smallest usable window glass height, map units.
const MIN_GLASS_HEIGHT: i64 = 16_i64;

/// Upper bound on the steps of one stair run; keeps the emitted layer
/// count sane for absurd floor differences.
const MAX_STEPS: usize = 4_096_usize;

/// Everything the map emitter needs to know about one placed connection.
///
/// [`ConnectorGeometry::kind`] rides along unchanged: a
/// [`ConnectionKind::Door`] has exactly the same geometry as a plain
/// [`ConnectionKind::Opening`] (one band through the wall) unless the
/// opening is wide enough to span the whole shared run, in which case it
/// carries no band at all.
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
    /// step first for stairs, one band otherwise; empty for a
    /// [`ConnectorGeometry::joined`] opening, which needs no band because
    /// no wall stands there.
    pub layers: Vec<Layer>,
    /// The wall this connection opens end to end: `Some` only for a plain
    /// [`ConnectionKind::Opening`] whose band reaches past both ends of
    /// the shared run it sits on. [`final_layers`] drops that wall and
    /// the two rooms share their boundary line directly, so the opening
    /// owns no band layer, no sector and no jamb faces.
    pub joined: Option<WallKey>,
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
/// ([`ConnectorGeometry::label_of`] reads them back). A plain opening
/// that spans the whole shared run takes no label and paints no band: it
/// only records the [`ConnectorGeometry::joined`] wall. `plan` should
/// have passed [`Plan::validate`] first; this pass still refuses
/// connections that name unknown rooms and boundaries too short for the
/// connection's [`Connection::width`].
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
///
/// Every refusal is attributed to the [`PlanRef::connection`] being placed,
/// named by the id (or `from->to` label) the tool edge resolves to its
/// wire row.
#[inline]
pub fn plan_connectors(plan: &Plan, walls: &Walls) -> Result<Vec<ConnectorGeometry>, CyberError> {
    let mut pass = Pass {
        plan,
        walls,
        labels: LabelAllocator::new(plan.rooms.len()),
    };
    let mut placed = Vec::with_capacity(plan.connections.len());
    for (index, connection) in plan.connections.iter().enumerate() {
        placed.push(pass.place(connection, index).map_err(|problem| {
            problem.at_plan(PlanRef::connection(&connection_label(connection)))
        })?);
    }
    Ok(placed)
}

/// The complete painter's-algorithm stack: room layers, then the wall
/// void layers a [`Walls`] build did not have to drop, then every
/// connector layer, bottom first.
///
/// Connectors sit above the wall voids so their regions paint through
/// them; [`arrange`](fn@crate::geom::arrange) consumes the stack in this
/// bottom-first order with a
/// [`LayerLabeler`](crate::geom::LayerLabeler) over it. A wall whose key
/// some connector [`joined`](ConnectorGeometry::joined) is left out of the
/// stack entirely: that opening spans the whole shared run, so nothing
/// stands between the rooms there and they meet on their own boundary.
#[must_use]
#[inline]
pub fn final_layers(plan: &Plan, walls: &Walls, connectors: &[ConnectorGeometry]) -> Vec<Layer> {
    let joined = joined_walls(connectors);
    let mut stack = room_layers(plan);
    stack.extend(
        walls
            .layers
            .iter()
            .filter(|wall| !joined.contains(&wall.key))
            .map(|wall| wall.layer.clone()),
    );
    for connector in connectors {
        stack.extend(connector.layers.iter().cloned());
    }
    stack
}

/// The walls the placed connections open end to end.
fn joined_walls(connectors: &[ConnectorGeometry]) -> BTreeSet<WallKey> {
    connectors
        .iter()
        .filter_map(|connector| connector.joined)
        .collect()
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
    let dx = narrow_i64(delta(piece.a.0, piece.b.0));
    let dy = narrow_i64(delta(piece.a.1, piece.b.1));
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
    (
        narrow_i32(i128::from(floor)),
        narrow_i32(i128::from(ceiling)),
    )
}

/// The glass span of a window: `max(floor) + sill` to
/// `min(ceiling) - lintel`; an error when it is under
/// [`MIN_GLASS_HEIGHT`] tall.
fn window_span(target: &Target<'_>, sill: i32, lintel: i32) -> Result<(i32, i32), CyberError> {
    let (base_floor, base_ceiling) = span_height(target);
    let floor = i64::from(base_floor).saturating_add(i64::from(sill));
    let ceiling = i64::from(base_ceiling).saturating_sub(i64::from(lintel));
    if ceiling.saturating_sub(floor) >= MIN_GLASS_HEIGHT {
        return Ok((
            narrow_i32(i128::from(floor)),
            narrow_i32(i128::from(ceiling)),
        ));
    }
    Err(CyberError::InvalidArgument(format!(
        "connection '{}' has window floor {floor} and ceiling {ceiling}; that leaves under {MIN_GLASS_HEIGHT} units of glass; lower the sill or the lintel",
        target.label
    )))
}

#[cfg(test)]
mod tests;
