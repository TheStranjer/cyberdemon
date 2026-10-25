//! The top-level plan compiler: a [`Plan`] and a theme in, a dressed
//! [`CompiledPlan`] out.
//!
//! [`compile`] runs the pipeline in this order, over the pure modules of
//! this crate's `plan` tree:
//!
//! 1. [`Plan::validate`] — structural checks on rooms and connections
//!    ([`plan::model`](crate::plan::model)).
//! 2. [`build_walls`] — arrange the rooms and grow the thick shared-wall
//!    void layers between them ([`plan::walls`](crate::plan::walls)).
//! 3. [`plan_connectors`] — place every connection on a shared boundary
//!    and build its band layers; an opening that spans a whole shared run
//!    takes its wall out of the stack instead of painting a band, so the
//!    two rooms join directly ([`plan::connectors`](crate::plan::connectors)).
//! 4. [`emit`] — one final arrangement of rooms, standing walls and bands,
//!    cut into the fresh [`Map`](crate::wad::map::Map)
//!    ([`plan::emit`](crate::plan::emit)).
//! 5. [`apply_textures`] — dress the emission in the theme's
//!    [`TextureRole`](crate::plan::textures::TextureRole) names
//!    ([`plan::textures`](crate::plan::textures)).
//! 6. The reachability, painted-over-room and floor-gap warnings of
//!    this module.
//! 7. Assemble the [`CompiledPlan`]: map, bookkeeping and warnings.
//!
//! The arrangement of step 4 is the only one: steps 5 to 7 read its
//! [`Emitted::pieces`](crate::plan::emit::Emitted::pieces) and label
//! index, never re-arrange. Nothing here touches archives, sessions or
//! `MCP` types.
//!
//! The compiler's own tests live in `plan/compile/tests.rs` with its
//! `openings`, `dressing` and `warnings` topics.

use alloc::collections::VecDeque;

use crate::error::CyberError;
use crate::plan::connectors::{ConnectorGeometry, plan_connectors};
use crate::plan::emit::{CompiledPlan, MapIdentity, compiled_plan, emit};
use crate::plan::model::{ConnectionKind, Plan};
use crate::plan::textures::{Scene, TextureRoles, apply_textures};
use crate::plan::walls::build_walls;
use crate::wad::classify::MapFormat;
use crate::wad::map::consts::MAX_STEP_HEIGHT;
use crate::wad::name::LumpName;

/// Compile `plan` into a dressed [`CompiledPlan`].
///
/// Runs the seven pipeline steps of this module's documentation:
/// [`Plan::validate`], [`build_walls`], [`plan_connectors`], [`emit`]
/// into a map named `name` in layout `format`, [`apply_textures`] with
/// `roles`, the reachability / coverage / floor-gap warnings of this
/// module, and the [`CompiledPlan`] bookkeeping. Every failure aborts
/// the whole compile: nothing outside the returned value is touched, so
/// a refused plan leaves no half-dressed map behind.
///
/// # Errors
///
/// [`Plan::validate`]'s refusals first, then
/// [`arrange`](fn@crate::geom::arrange)'s [`CyberError::InvalidArgument`] /
/// [`CyberError::LimitExceeded`] from the wall and final arrangements,
/// [`plan_connectors`]' refusals for unplaceable connections (unknown
/// rooms, rooms that "do not touch", boundaries too short, window spans
/// with too little glass, level stair runs), [`emit`]'s limit refusals
/// for out-of-range coordinates, and [`apply_textures`]' internal
/// contract refusals.
#[inline]
pub fn compile(
    plan: &Plan,
    name: LumpName,
    format: MapFormat,
    roles: &dyn TextureRoles,
) -> Result<CompiledPlan, CyberError> {
    plan.validate()?;
    let walls = build_walls(plan)?;
    let connectors = plan_connectors(plan, &walls)?;
    let identity = MapIdentity { name, format };
    let mut emitted = emit(plan, &walls, &connectors, identity)?;
    let mut map = core::mem::take(&mut emitted.map);
    let scene = Scene {
        plan,
        connectors: &connectors,
        emitted: &emitted,
    };
    apply_textures(&mut map, &scene, roles)?;
    emitted.map = map;
    let mut compiled = compiled_plan(plan, &connectors, emitted);
    compiled.warnings = warnings(plan, &connectors, &compiled);
    Ok(compiled)
}

/// The advisory notes of a compiled plan: quirks worth telling the
/// caller about that are not errors, in the order unreachable rooms,
/// painted-over rooms, too-tall floor gaps.
fn warnings(plan: &Plan, connectors: &[ConnectorGeometry], compiled: &CompiledPlan) -> Vec<String> {
    let mut notes = Vec::new();
    unreachable_rooms(plan, connectors, &mut notes);
    covered_rooms(plan, compiled, &mut notes);
    tall_steps(plan, connectors, &mut notes);
    notes
}

/// Warn about every room the connection graph cannot carry you to from
/// the plan's first room.
fn unreachable_rooms(plan: &Plan, connectors: &[ConnectorGeometry], notes: &mut Vec<String>) {
    let reached = reachable_from_first(plan, connectors);
    let Some(root) = plan.rooms.first() else {
        return;
    };
    for (index, room) in plan.rooms.iter().enumerate() {
        if !reached.get(index).copied().unwrap_or(false) {
            notes.push(format!(
                "room '{}' cannot be reached from room '{}' through the plan's connections; connect it or drop it",
                room.id, root.id
            ));
        }
    }
}

/// The rooms reachable from room `0` over the placed connectors'
/// undirected room pairs.
fn reachable_from_first(plan: &Plan, connectors: &[ConnectorGeometry]) -> Vec<bool> {
    let adjacency = adjacency(plan, connectors);
    let mut reached = vec![false; plan.rooms.len()];
    let Some((first, _)) = reached.split_first_mut() else {
        return reached;
    };
    *first = true;
    let mut queue = VecDeque::from([0_usize]);
    while let Some(index) = queue.pop_front() {
        for neighbour in adjacency.get(index).into_iter().flatten() {
            visit_room(*neighbour, &mut reached, &mut queue);
        }
    }
    reached
}

/// Mark `neighbour` reached and queue it when it is new.
fn visit_room(neighbour: usize, reached: &mut [bool], queue: &mut VecDeque<usize>) {
    let Some(slot) = reached.get_mut(neighbour) else {
        return;
    };
    if !*slot {
        *slot = true;
        queue.push_back(neighbour);
    }
}

/// The undirected room-pair adjacency of the placed connectors.
fn adjacency(plan: &Plan, connectors: &[ConnectorGeometry]) -> Vec<Vec<usize>> {
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); plan.rooms.len()];
    for connector in connectors {
        if let Some(from) = neighbours.get_mut(connector.to) {
            from.push(connector.from);
        }
        if let Some(to) = neighbours.get_mut(connector.from) {
            to.push(connector.to);
        }
    }
    neighbours
}

/// Warn about every room a later room painted over completely: its
/// region label survives on no piece, so it owns no sector.
fn covered_rooms(plan: &Plan, compiled: &CompiledPlan, notes: &mut Vec<String>) {
    for room in &plan.rooms {
        if !compiled.room_sectors.contains_key(&room.id) {
            notes.push(format!(
                "room '{}' is fully painted over by later rooms and owns no sector",
                room.id
            ));
        }
    }
}

/// Warn about every opening, door or window joining floors more than
/// [`MAX_STEP_HEIGHT`] apart: the player cannot step over that.
fn tall_steps(plan: &Plan, connectors: &[ConnectorGeometry], notes: &mut Vec<String>) {
    for connector in connectors {
        if !matches!(
            connector.kind,
            ConnectionKind::Opening | ConnectionKind::Door { .. } | ConnectionKind::Window { .. }
        ) {
            continue;
        }
        let (Some(from), Some(to)) = (plan.rooms.get(connector.from), plan.rooms.get(connector.to))
        else {
            continue;
        };
        let gap = i64::from(from.floor)
            .saturating_sub(i64::from(to.floor))
            .abs();
        if gap > i64::from(MAX_STEP_HEIGHT) {
            notes.push(format!(
                "step taller than {MAX_STEP_HEIGHT} between {} and {}; use stairs",
                from.id, to.id
            ));
        }
    }
}

#[cfg(test)]
mod tests;
