//! `move_vertex`'s working steps: reading a request onto the map's points
//! and refusing the linedefs it would collapse.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`MoveVertexParams`] to [`move_run`]. That validates the request up
//! front ([`validate_move`]), resolves the map and runs [`move_in_place`]
//! as one [`crate::session::Session::edit_map`] step: [`new_positions`]
//! computes where every selected vertex lands, [`check_vertices`] refuses an
//! index the map does not hold and [`collapsed_linedefs`] refuses the move
//! that would put both ends of one line on the same point, so a refused
//! call never writes.

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, edit_result_fragment};
use crate::wad::classify::MapFormat;
use crate::wad::map::query::check_indices;
use crate::wad::map::{Linedef, Map, Vertex};

use super::{MoveReport, MoveVertexParams, Moves, VertexRow, check_binary_coords};

/// The coordinate after the move: `absolute` wins, `delta` shifts
/// `current`, neither keeps it.
pub(super) fn new_coord(
    name: &str,
    current: i32,
    absolute: Option<i32>,
    delta: Option<i32>,
) -> Result<i32, CyberError> {
    if let Some(value) = absolute {
        return Ok(value);
    }
    delta.map_or(Ok(current), |shift| {
        current.checked_add(shift).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "{name} {current} plus {shift} leaves the range of map coordinates"
            ))
        })
    })
}

/// The point vertex `current` lands on under `args`, range-checked against
/// the binary `VERTEXES` columns of `format`.
pub(super) fn moved_to(
    args: &MoveVertexParams,
    format: MapFormat,
    current: &Vertex,
) -> Result<(i32, i32), CyberError> {
    let x = new_coord("x", current.x, args.x, args.dx)?;
    let y = new_coord("y", current.y, args.y, args.dy)?;
    check_binary_coords(format, x, y)?;
    Ok((x, y))
}

/// Validates a `move_vertex` request up front and returns its sorted,
/// deduplicated target indices.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty index list, an absolute and
/// a relative parameter together, neither at all, or absolute `x`/`y` asked
/// for more than one vertex.
pub(super) fn validate_move(args: &MoveVertexParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.clone().indices()?;
    let absolute = args.x.is_some() || args.y.is_some();
    let relative = args.dx.is_some() || args.dy.is_some();
    if absolute && relative {
        return Err(CyberError::InvalidArgument(
            "an absolute coordinate conflicts with a delta: pass x and/or y to place one vertex, \
             or dx and/or dy to shift a vertex or a whole list"
                .to_owned(),
        ));
    }
    if !absolute && !relative {
        return Err(CyberError::InvalidArgument(
            "nothing to move: pass x and/or y for one vertex, or dx and/or dy for one or many"
                .to_owned(),
        ));
    }
    if absolute && indices.len() > 1 {
        return Err(CyberError::InvalidArgument(
            "absolute x/y apply to a single vertex: pass one index, or move a list with dx/dy"
                .to_owned(),
        ));
    }
    Ok(indices)
}

/// The new points of `indices` under `args`, keyed by vertex index.
fn new_positions(
    map: &Map,
    args: &MoveVertexParams,
    indices: &[usize],
) -> Result<Moves, CyberError> {
    let mut moved = Moves::new();
    for index in indices {
        if let Some(vertex) = map.vertices.get(*index) {
            moved.insert(*index, moved_to(args, map.format, vertex)?);
        }
    }
    Ok(moved)
}

/// Refuses unless every index in `indices` is a vertex of `map`, through
/// the shared [`check_indices`] guard.
///
/// # Errors
///
/// [`CyberError::NotFound`] for the first out-of-range index.
///
/// [`check_indices`]: crate::wad::map::query::check_indices
pub(super) fn check_vertices(indices: &[usize], map: &Map) -> Result<(), CyberError> {
    check_indices(ElementKind::Vertices, indices, map)
}

/// The `(x, y)` of vertex `index` as `map` stores it.
pub(super) fn stored(map: &Map, index: usize) -> Option<(i32, i32)> {
    map.vertices.get(index).map(|vertex| (vertex.x, vertex.y))
}

/// The `(x, y)` of vertex `index` after the move `moved` describes.
fn placed(map: &Map, moved: &Moves, index: usize) -> Option<(i32, i32)> {
    moved.get(&index).copied().or_else(|| stored(map, index))
}

/// Whether two known points are the same place; an unknown point (a
/// dangling reference) never matches one.
pub(super) fn same_point(one: Option<(i32, i32)>, other: Option<(i32, i32)>) -> bool {
    one.is_some_and(|first| other == Some(first))
}

/// Whether `line` spans two distinct points now and a single point after
/// the move `moved`: the zero-length line the edit must refuse.
pub(super) fn made_zero_length(map: &Map, moved: &Moves, line: &Linedef) -> bool {
    let apart_now = !same_point(stored(map, line.v1), stored(map, line.v2));
    let together_after = same_point(placed(map, moved, line.v1), placed(map, moved, line.v2));
    apart_now && together_after
}

/// The indices of the linedefs `moved` would collapse to a point.
fn collapsed_linedefs(map: &Map, moved: &Moves) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| made_zero_length(map, moved, line))
        .map(|(index, _)| index)
        .collect()
}

/// The indices as one comma-separated list.
pub(super) fn join(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The `conflict` naming every linedef a move would collapse to a point.
pub(super) fn collapse_refusal(lines: &[usize]) -> CyberError {
    CyberError::Conflict(format!(
        "the move makes linedef(s) {} zero-length: a line needs two distinct endpoints; move the \
         other endpoint too, or leave this vertex where it is",
        join(lines)
    ))
}

/// `move_vertex`'s step: check the targets, refuse a collapsed linedef and
/// rewrite the selected vertices.
pub(super) fn move_in_place(
    map: &mut Map,
    args: &MoveVertexParams,
    indices: &[usize],
) -> Result<Vec<VertexRow>, CyberError> {
    check_vertices(indices, map)?;
    let moved = new_positions(map, args, indices)?;
    let collapsed = collapsed_linedefs(map, &moved);
    if !collapsed.is_empty() {
        return Err(collapse_refusal(&collapsed));
    }
    let mut rows = Vec::new();
    for (index, slot) in map.vertices.iter_mut().enumerate() {
        let Some(&(x, y)) = moved.get(&index) else {
            continue;
        };
        slot.x = x;
        slot.y = y;
        rows.push(VertexRow { index, x, y });
    }
    Ok(rows)
}

/// `move_vertex`'s work: validate, resolve the map, run the one-step move.
pub(super) fn move_run(
    session: &mut Session,
    args: &MoveVertexParams,
) -> Result<MoveReport, CyberError> {
    let indices = validate_move(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("move {} vertices", indices.len());
    let (rows, report) =
        session.edit_map(name, &label, |map| move_in_place(map, args, &indices))?;
    Ok(MoveReport {
        moved: rows,
        edit: edit_result_fragment(&report),
    })
}
