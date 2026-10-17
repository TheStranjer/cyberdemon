//! Vertex editing tools: `add_vertex`, `move_vertex` and `remove_vertex`
//! (see the `edit_vertices` router methods on [`crate::server::Cyberdemon`]).
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The one exception is `add_vertex`
//! reusing a point that is already there (the default): it reports the
//! existing index without touching the archive, so it leaves no undo entry
//! and copies nothing in.
//!
//! The parameter shapes ([`IndexSelector`]) and validators ([`check_index`],
//! [`edit_result_fragment`]) come from [`crate::tools::maps::params`];
//! removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a vertex the linedefs still use unless `cascade` is set.
//!
//! The geometry rule: a vertex is geometry for the node lumps while a
//! linedef ends at it. Editing a referenced vertex changes geometry — the
//! generated node lumps are emptied (and the trailing unreferenced ones,
//! split points a `build_nodes` left behind, are stripped with them), the
//! block's `GL` run dropped, and the result reports `nodes_invalidated:
//! true`. Run `build_nodes` before playing such a map in vanilla `Doom`;
//! `ZDoom`-family ports rebuild the missing node data themselves. Trailing
//! vertices no linedef references are invisible to the node builder: an
//! `add_vertex` append keeps the nodes trusted until a line ends at the
//! new point.
//!
//! The format rules: the binary formats store each coordinate as `i16`, so
//! `Doom` and `Hexen` maps keep `x`/`y` inside `-32768..=32767` and a value
//! outside that is `invalid_argument`; `UDMF` stores full integers and has
//! no such cap. A move that would put both ends of one linedef on the same
//! coordinates — a zero-length line, which no node builder can use — is
//! refused with `conflict` naming the linedef.

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::map_edit::MapEditReport;
use crate::session::{Scope, Session};
use crate::tools::maps::common;
use crate::tools::maps::params::{IndexSelector, edit_result_fragment};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};
use crate::wad::map::{Linedef, Map, Vertex};
use crate::wad::name::LumpName;

/// The lowest coordinate an `i16` binary `VERTEXES` column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest coordinate an `i16` binary `VERTEXES` column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;

/// Where a `move_vertex` call lands its targets: each selected vertex index
/// with the point it gets.
type Moves = BTreeMap<usize, (i32, i32)>;

/// Arguments for [`Cyberdemon::add_vertex`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddVertexParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
    /// Return an existing vertex at exactly these coordinates instead of
    /// appending a second one (default `true`).
    #[serde(default)]
    reuse_existing: Option<bool>,
}

/// Arguments for [`Cyberdemon::move_vertex`].
#[derive(Debug, Deserialize, JsonSchema)]
struct MoveVertexParams {
    /// The map to edit; defaults like [`AddVertexParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The vertex to move: one index or a list of them (duplicates
    /// collapse). Absolute `x`/`y` need a single index; `dx`/`dy` shift a
    /// whole list by the same amount.
    index: IndexSelector,
    /// Absolute `X` (single index only; use `dx` for a list).
    #[serde(default)]
    x: Option<i32>,
    /// Absolute `Y` (single index only; use `dy` for a list).
    #[serde(default)]
    y: Option<i32>,
    /// Move every selected vertex by this much in `X`.
    #[serde(default)]
    dx: Option<i32>,
    /// Move every selected vertex by this much in `Y`.
    #[serde(default)]
    dy: Option<i32>,
}

/// Arguments for [`Cyberdemon::remove_vertex`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveVertexParams {
    /// The map to edit; defaults like [`AddVertexParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The vertex to remove: one index or a list of them.
    index: IndexSelector,
    /// Remove the linedefs that use these vertices with them (default
    /// `false`: a used vertex is refused with `conflict`).
    #[serde(default)]
    cascade: bool,
    /// Within a cascade, also remove sidedefs no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_sidedefs: Option<bool>,
    /// Within a cascade, also remove vertices no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_vertices: Option<bool>,
}

/// One vertex as the tools report it: its position and its point.
#[derive(Clone, Copy, Debug, Serialize)]
struct VertexRow {
    /// Position in [`Map::vertices`].
    index: usize,
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
}

/// Structured result of [`Cyberdemon::add_vertex`].
#[derive(Serialize)]
struct AddReport {
    /// Index of the vertex: the reused one, else the appended one.
    index: usize,
    /// `X` as stored.
    x: i32,
    /// `Y` as stored.
    y: i32,
    /// Whether an existing vertex at these coordinates was returned.
    reused: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::move_vertex`].
#[derive(Serialize)]
struct MoveReport {
    /// The moved vertices, ascending by index, at their new points.
    moved: Vec<VertexRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_vertex`].
#[derive(Serialize)]
struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Whether the request accepts an existing point at the same coordinates.
fn reuses(args: &AddVertexParams) -> bool {
    args.reuse_existing.unwrap_or(true)
}

/// The index of the first vertex sitting exactly on `x`/`y`.
fn find_vertex(map: &Map, x: i32, y: i32) -> Option<usize> {
    map.vertices
        .iter()
        .position(|vertex| vertex.x == x && vertex.y == y)
}

/// The map as the next edit sees it: the working archive's block when it
/// holds one, else the merged view's (which is what the edit copies in).
///
/// # Errors
///
/// [`common::load_map`]'s errors for the merged scope when the working
/// archive holds no such block.
fn current_map(session: &Session, name: LumpName) -> Result<Map, CyberError> {
    match common::load_map(session, name, Scope::Working) {
        Ok(map) => Ok(map),
        Err(CyberError::NotFound(_)) => common::load_map(session, name, Scope::Merged),
        Err(error) => Err(error),
    }
}

/// The edit fragment of a call that changed nothing: the map's name with
/// both flags `false`.
fn untouched_fragment(name: LumpName) -> JsonMap<String, Value> {
    edit_result_fragment(&MapEditReport {
        map: name,
        copied_to_working: false,
        nodes_invalidated: false,
    })
}

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `VERTEXES` column.
fn fits_coord(name: &str, value: i32) -> Result<(), CyberError> {
    if (COORD_MIN..=COORD_MAX).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds \
         {COORD_MIN}..={COORD_MAX}"
    )))
}

/// Range-checks one point against the binary `VERTEXES` record; a `UDMF`
/// map stores full integers and needs no check.
fn check_binary_coords(format: MapFormat, x: i32, y: i32) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_coord("x", x)?;
            fits_coord("y", y)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// `add_vertex`'s step: append the described point and report its row.
fn add_in_place(map: &mut Map, args: &AddVertexParams) -> Result<VertexRow, CyberError> {
    check_binary_coords(map.format, args.x, args.y)?;
    let index = map.vertices.len();
    map.vertices.push(Vertex {
        x: args.x,
        y: args.y,
        extra: BTreeMap::new(),
    });
    Ok(VertexRow {
        index,
        x: args.x,
        y: args.y,
    })
}

/// The coordinate after the move: `absolute` wins, `delta` shifts
/// `current`, neither keeps it.
fn new_coord(
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
fn moved_to(
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
fn validate_move(args: &MoveVertexParams) -> Result<Vec<usize>, CyberError> {
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

/// Refuses unless every index in `indices` is a vertex of a `map` that
/// holds `count` of them.
///
/// [`check_index`] spells its plural with a plain `s`, which "vertex" does
/// not take, so the wording lives here and matches the shared shape:
/// `"vertex 9 does not exist (map has 6 vertices)"`.
///
/// # Errors
///
/// [`CyberError::NotFound`] for the first out-of-range index.
///
/// [`check_index`]: crate::tools::maps::params::check_index
fn check_vertices(indices: &[usize], count: usize) -> Result<(), CyberError> {
    for index in indices {
        if *index >= count {
            return Err(CyberError::NotFound(format!(
                "vertex {index} does not exist (map has {count} vertices)"
            )));
        }
    }
    Ok(())
}

/// The `(x, y)` of vertex `index` as `map` stores it.
fn stored(map: &Map, index: usize) -> Option<(i32, i32)> {
    map.vertices.get(index).map(|vertex| (vertex.x, vertex.y))
}

/// The `(x, y)` of vertex `index` after the move `moved` describes.
fn placed(map: &Map, moved: &Moves, index: usize) -> Option<(i32, i32)> {
    moved.get(&index).copied().or_else(|| stored(map, index))
}

/// Whether two known points are the same place; an unknown point (a
/// dangling reference) never matches one.
fn same_point(one: Option<(i32, i32)>, other: Option<(i32, i32)>) -> bool {
    one.is_some_and(|first| other == Some(first))
}

/// Whether `line` spans two distinct points now and a single point after
/// the move `moved`: the zero-length line the edit must refuse.
fn made_zero_length(map: &Map, moved: &Moves, line: &Linedef) -> bool {
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
fn join(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The `conflict` naming every linedef a move would collapse to a point.
fn collapse_refusal(lines: &[usize]) -> CyberError {
    CyberError::Conflict(format!(
        "the move makes linedef(s) {} zero-length: a line needs two distinct endpoints; move the \
         other endpoint too, or leave this vertex where it is",
        join(lines)
    ))
}

/// `move_vertex`'s step: check the targets, refuse a collapsed linedef and
/// rewrite the selected vertices.
fn move_in_place(
    map: &mut Map,
    args: &MoveVertexParams,
    indices: &[usize],
) -> Result<Vec<VertexRow>, CyberError> {
    check_vertices(indices, map.vertices.len())?;
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

/// The pruning flags of a `remove_vertex` call, the cascading default when
/// either is absent.
fn cascade_options(args: &RemoveVertexParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// The seed removal of exactly the selected vertices.
fn vertex_removal(indices: &[usize]) -> Removal {
    Removal {
        vertices: indices.iter().copied().collect::<BTreeSet<usize>>(),
        ..Default::default()
    }
}

/// The `conflict` naming every linedef that still uses a selected vertex.
fn dependent_refusal(uses: &BTreeMap<usize, Vec<usize>>) -> CyberError {
    let parts: Vec<String> = uses
        .iter()
        .map(|(vertex, lines)| format!("vertex {vertex} is used by linedef(s) {}", join(lines)))
        .collect();
    CyberError::Conflict(format!(
        "{}; pass cascade: true to remove the referencing linedefs with them",
        parts.join("; ")
    ))
}

/// `remove_vertex`'s step: check the indices, refuse a used vertex without
/// `cascade`, then run the removal (grown by the cascade when set).
fn remove_in_place(
    map: &mut Map,
    args: &RemoveVertexParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    check_vertices(indices, map.vertices.len())?;
    let removal = vertex_removal(indices);
    if !args.cascade {
        let uses = dependents(map, &removal).linedefs_using_vertices;
        if !uses.is_empty() {
            return Err(dependent_refusal(&uses));
        }
        return apply_removal(map, &removal);
    }
    let grown = expand_cascade(map, &removal, cascade_options(args));
    apply_removal(map, &grown)
}

/// `add_vertex`'s work: resolve the map, return a point that is already
/// there, else run the one-step append.
fn add_run(session: &mut Session, args: &AddVertexParams) -> Result<AddReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let existing = if reuses(args) {
        find_vertex(&current_map(session, name)?, args.x, args.y)
    } else {
        None
    };
    if let Some(index) = existing {
        return Ok(AddReport {
            index,
            x: args.x,
            y: args.y,
            reused: true,
            edit: untouched_fragment(name),
        });
    }
    let label = format!("add vertex at {}, {}", args.x, args.y);
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args))?;
    Ok(AddReport {
        index: row.index,
        x: row.x,
        y: row.y,
        reused: false,
        edit: edit_result_fragment(&report),
    })
}

/// `move_vertex`'s work: validate, resolve the map, run the one-step move.
fn move_run(session: &mut Session, args: &MoveVertexParams) -> Result<MoveReport, CyberError> {
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

/// `remove_vertex`'s work: resolve the map and run the removal.
fn remove_run(
    session: &mut Session,
    args: &RemoveVertexParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} vertices", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_vertices_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one vertex to a map, or returns the point already there.
    #[tool(
        description = "Adds one vertex (a point of the map's geometry) to a map in a single undoable edit. map defaults to the active map (set_active_map, else the merged view's only map). Required: x and y in map units. Optional: reuse_existing (default true) - when a vertex already sits at exactly these coordinates its index comes back with reused true, nothing is written and no undo entry is made; pass false to store a second vertex on the same point anyway. Binary maps (Doom, Hexen) store coordinates as i16, so x or y outside -32768..=32767 fails invalid_argument; UDMF maps store full integers and take any value. A vertex nothing references is invisible in the game: a linedef has to end at it (add_linedef, or move_vertex to bring one endpoint of an existing line onto it). A vertex is geometry to the node builder only while a linedef ends at it: an append past the last referenced index keeps the node lumps trusted (nodes_invalidated false) - note that a later edit that does invalidate the nodes also strips trailing vertices no linedef references, this one included. Returns {index, x, y, reused, map, nodes_invalidated, copied_to_working}. Unknown map is not_found, an undecodable map is format_error. See skill/reference/vertices.md."
    )]
    fn add_vertex(
        &self,
        Parameters(args): Parameters<AddVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Moves one vertex, or a whole group of them, in one undoable edit.
    #[tool(
        description = "Moves one vertex, or several at once, of a map in a single undoable edit. index is one vertex index or a list of them (duplicates collapse, sorted first). Two ways to say where, and only one of them per call: absolute x and/or y - allowed with a single index only - or relative dx and/or dy, which shift a single vertex or a whole list by the same amount. An axis you leave out stays where it is. Mixing an absolute parameter with a delta, giving none of the four, or giving absolute x/y with a list of indices all fail invalid_argument. On binary maps the resulting x/y must still fit i16 -32768..=32767 (invalid_argument), and a dx/dy that leaves the range of map coordinates is refused; UDMF maps are unbounded. A move that would put both endpoints of one linedef on the same coordinates (a zero-length line no node builder can use) fails conflict naming that linedef and nothing changes; two vertices may share a point as long as no single line spans them. Moving a vertex a linedef ends at is geometry: the node lumps are invalidated (nodes_invalidated true; run build_nodes before playing the map in vanilla Doom, which also strips trailing vertices no linedef references, split points a previous build left). Moving only vertices nothing references keeps the nodes. Returns {moved: [{index, x, y}], map, nodes_invalidated, copied_to_working}. An index the map does not hold is not_found and nothing changes. See skill/reference/vertices.md."
    )]
    fn move_vertex(
        &self,
        Parameters(args): Parameters<MoveVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| move_run(session, &args)),
        )
    }

    /// Removes one or several vertices in a single undoable edit.
    #[tool(
        description = "Removes one vertex or a list of them (index: one integer or an array) from a map in a single undoable edit; the remaining vertices renumber down and every surviving linedef's endpoints follow. Without cascade, a vertex any linedef still uses is refused with conflict naming those linedefs and nothing changes; a vertex nothing references (e.g. one just added, or an orphan left by an earlier removal) removes fine. With cascade: true the removal takes the geometry with it: every linedef using a removed vertex goes, a linedef that loses its front side is flipped (its back becomes the front), one that loses its back becomes one-sided (two-sided bit cleared, blocking bit set, an otherwise invisible lower or upper texture of the surviving side moved to the middle), and a linedef that loses both sides goes with them; prune_sidedefs (default true) also drops sidedefs no surviving linedef references and prune_vertices (default true) also drops vertices no surviving linedef references, so a cascade leaves no orphans behind unless you switch a pruning off. Removing vertices is geometry: the node lumps are invalidated (nodes_invalidated true; run build_nodes before playing the map in vanilla Doom). Returns {removed: {vertices, linedefs, sidedefs, sectors, things (removed original indices, sorted), flipped_linedefs, one_sided_linedefs (new indices of the normalised linedefs)}, map, nodes_invalidated, copied_to_working}. An index the map does not hold is not_found; an empty index list is invalid_argument. See skill/reference/vertices.md."
    )]
    fn remove_vertex(
        &self,
        Parameters(args): Parameters<RemoveVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, from_str};

    use super::{
        AddVertexParams, COORD_MAX, COORD_MIN, MoveVertexParams, Moves, add_in_place,
        cascade_options, check_binary_coords, collapse_refusal, current_map, find_vertex,
        fits_coord, made_zero_length, move_in_place, moved_to, new_coord, remove_in_place, reuses,
        same_point, stored, untouched_fragment, validate_move,
    };
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Session};
    use crate::wad::archive::{Wad, WadKind};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;
    use std::path::PathBuf;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An add request parsed from `json`.
    fn add_args(json: &str) -> AddVertexParams {
        from_str(json).expect("test add requests parse")
    }

    /// A move request parsed from `json`.
    fn move_args(json: &str) -> MoveVertexParams {
        from_str(json).expect("test move requests parse")
    }

    /// A remove request parsed from `json`.
    fn remove_args(json: &str) -> super::RemoveVertexParams {
        from_str(json).expect("test remove requests parse")
    }

    /// A valid parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A blocking one-sided line from `v1` to `v2` over sidedef `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided step line from `v1` to `v2` over sidedefs `front`/`back`.
    fn step_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sidedef opening into `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// The two-room map the test fixtures build: a `0,0` to `512,256` hall
    /// and raised back room, six vertices, seven lines (`2` the two-sided
    /// step), eight sides over two sectors.
    fn two_room() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                solid_line(0, 5, 0),
                solid_line(5, 4, 1),
                step_line(4, 1, 2, 3),
                solid_line(1, 0, 4),
                solid_line(4, 3, 5),
                solid_line(3, 2, 6),
                solid_line(2, 1, 7),
            ],
            sidedefs: vec![
                side(0),
                side(0),
                side(0),
                side(1),
                side(0),
                side(1),
                side(1),
                side(1),
            ],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    /// The two-room map in `format`.
    fn fixture(format: MapFormat) -> Map {
        let mut map = two_room();
        map.format = format;
        map
    }

    /// A session whose only layer is an empty archive (no map blocks).
    fn empty_session() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: Vec::new(),
            },
        ));
        session
    }

    #[test]
    fn reuse_is_the_default_and_finds_an_exact_point() {
        let plain = add_args(r#"{"x": 8, "y": -8}"#);
        assert!(reuses(&plain), "reuse_existing defaults to true");
        let fresh = add_args(r#"{"x": 8, "y": -8, "reuse_existing": false}"#);
        assert!(!reuses(&fresh), "and can be switched off");
        let map = fixture(MapFormat::Doom);
        assert_eq!(
            find_vertex(&map, 256_i32, 256_i32),
            Some(4_usize),
            "the fixture's back-left corner"
        );
        assert_eq!(
            find_vertex(&map, 0_i32, 0_i32),
            Some(0_usize),
            "the first match wins"
        );
        assert_eq!(
            find_vertex(&map, 256_i32, 255_i32),
            None,
            "one unit off is another point"
        );
    }

    #[test]
    fn an_added_vertex_appends_and_reports_its_row() {
        let mut map = fixture(MapFormat::Doom);
        let args = add_args(r#"{"x": -16, "y": 128}"#);
        let row = add_in_place(&mut map, &args).expect("a plain point appends");
        assert_eq!((row.index, row.x, row.y), (6_usize, -16_i32, 128_i32));
        assert_eq!(map.vertices.len(), 7, "the list grew by one");
        let last = map.vertices.last().expect("the appended vertex");
        assert!(last.extra.is_empty(), "and it carries no format extras");
    }

    #[test]
    fn binary_maps_cap_coordinates_and_udmf_does_not() {
        assert!(
            fits_coord("x", COORD_MIN).is_ok() && fits_coord("y", COORD_MAX).is_ok(),
            "the i16 edges fit"
        );
        let over = COORD_MAX.checked_add(1).expect("the edge plus one");
        let error = fits_coord("x", over).expect_err("above i16 refuses");
        assert_eq!(code(&error), "invalid_argument", "range code");
        assert!(
            error.to_string().contains(
                "x 32768 is out of range for the binary map format: the field holds -32768..=32767"
            ),
            "the message names the field and the range: {error}"
        );
        assert!(
            check_binary_coords(MapFormat::Hexen, 0_i32, COORD_MIN).is_ok(),
            "the Hexen record is i16 too"
        );
        assert!(
            check_binary_coords(MapFormat::Doom, over, 0_i32).is_err(),
            "x above i16 refuses on binary Doom"
        );
        assert!(
            check_binary_coords(MapFormat::Udmf, i32::MAX, i32::MIN).is_ok(),
            "UDMF stores full integers"
        );
        let mut wide = fixture(MapFormat::Udmf);
        let args = add_args(r#"{"x": 40000, "y": -40000}"#);
        let row = add_in_place(&mut wide, &args).expect("UDMF takes the wide point");
        assert_eq!(row.x, 40_000_i32, "and stores it");
        let mut binary = fixture(MapFormat::Doom);
        let refusal = add_in_place(&mut binary, &args).expect_err("the same point refuses");
        assert_eq!(code(&refusal), "invalid_argument", "binary code");
        assert_eq!(binary.vertices.len(), 6, "and nothing appended");
    }

    #[test]
    fn a_reused_point_reports_an_untouched_fragment() {
        let fragment = untouched_fragment(name("MAP01"));
        assert_eq!(fragment["map"], Value::String("MAP01".to_owned()), "named");
        assert_eq!(
            fragment["nodes_invalidated"],
            Value::Bool(false),
            "nothing was invalidated"
        );
        assert_eq!(
            fragment["copied_to_working"],
            Value::Bool(false),
            "nothing was copied"
        );
    }

    #[test]
    fn current_map_falls_back_to_the_merged_view() {
        let session = empty_session();
        let error = current_map(&session, name("MAP01")).expect_err("no map anywhere");
        assert_eq!(code(&error), "not_found", "the merged refusal stands");
        assert!(
            error.to_string().contains("merged"),
            "the message names the scope it checked: {error}"
        );
    }

    #[test]
    fn move_validation_reads_absolute_relative_and_lists() {
        let single = validate_move(&move_args(r#"{"index": 3, "x": 8, "y": 16}"#))
            .expect("absolute on one index");
        assert_eq!(single, vec![3_usize], "the target");
        let list = validate_move(&move_args(r#"{"index": [4, 1, 4], "dx": 8, "dy": -8}"#))
            .expect("a list with deltas");
        assert_eq!(list, vec![1_usize, 4], "sorted and deduplicated");
        let refusals = [
            r#"{"index": 0, "x": 1, "dx": 2}"#,
            r#"{"index": 0, "y": 1, "dy": 2}"#,
            r#"{"index": 0, "x": 1, "dy": 2}"#,
            r#"{"index": [0, 1], "x": 4}"#,
            r#"{"index": [0, 1], "y": 4}"#,
            r#"{"index": 0}"#,
            r#"{"index": []}"#,
        ];
        for json in refusals {
            let error = validate_move(&move_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
        let nothing = validate_move(&move_args(r#"{"index": 0}"#))
            .expect_err("nothing to move")
            .to_string();
        assert!(
            nothing.contains("x and/or y") && nothing.contains("dx and/or dy"),
            "the message spells out both forms: {nothing}"
        );
    }

    #[test]
    fn moved_points_shift_or_replace_and_stay_in_range() {
        let vertex = Vertex {
            x: 100_i32,
            y: -100_i32,
            ..Default::default()
        };
        let absolute = move_args(r#"{"index": 0, "x": -5}"#);
        assert_eq!(
            moved_to(&absolute, MapFormat::Doom, &vertex).expect("x moves, y stands"),
            (-5_i32, -100_i32),
            "an axis left out stays"
        );
        let relative = move_args(r#"{"index": 0, "dx": 8, "dy": 8}"#);
        assert_eq!(
            moved_to(&relative, MapFormat::Doom, &vertex).expect("both axes shift"),
            (108_i32, -92_i32),
            "the block moves"
        );
        let far = move_args(r#"{"index": 0, "dx": 40000}"#);
        let error = moved_to(&far, MapFormat::Doom, &vertex).expect_err("above i16 refuses");
        assert_eq!(code(&error), "invalid_argument", "the binary cap applies");
        assert!(
            moved_to(&far, MapFormat::Udmf, &vertex).is_ok(),
            "UDMF has no cap"
        );
        assert_eq!(
            new_coord("x", COORD_MAX, Some(0_i32), None).expect("absolute wins"),
            0_i32,
            "a replacement ignores the old edge"
        );
        let overflow = new_coord("y", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
        assert_eq!(
            code(&overflow),
            "invalid_argument",
            "an overflowing delta refuses"
        );
        assert!(
            overflow
                .to_string()
                .contains("y 2147483647 plus 1 leaves the range of map coordinates"),
            "the message names the axis: {overflow}"
        );
    }

    #[test]
    fn a_move_rewrites_the_targets_and_reports_them() {
        let mut map = fixture(MapFormat::Doom);
        let args = move_args(r#"{"index": [0, 1], "dy": 32}"#);
        let rows = move_in_place(&mut map, &args, &[0_usize, 1])
            .expect("the shared wall's bottom corners rise");
        assert_eq!(
            rows.iter()
                .map(|row| (row.index, row.x, row.y))
                .collect::<Vec<_>>(),
            vec![(0_usize, 0_i32, 32_i32), (1_usize, 256_i32, 32_i32)],
            "ascending, at the new points"
        );
        assert_eq!(
            (map.vertices[2].x, map.vertices[2].y),
            (512_i32, 0_i32),
            "an unselected vertex stands"
        );
    }

    #[test]
    fn a_move_refuses_a_missing_index_and_changes_nothing() {
        let mut map = fixture(MapFormat::Doom);
        let before = map.clone();
        let error = move_in_place(&mut map, &move_args(r#"{"index": 0, "dx": 8}"#), &[9_usize])
            .expect_err("the map holds six vertices");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert!(
            error
                .to_string()
                .contains("vertex 9 does not exist (map has 6 vertices)"),
            "the message counts the vertices: {error}"
        );
        assert_eq!(map, before, "and the map stands");
    }

    #[test]
    fn a_move_onto_another_endpoint_is_a_conflict_naming_the_line() {
        let mut map = fixture(MapFormat::Doom);
        let args = move_args(r#"{"index": 5, "x": 0, "y": 0}"#);
        let error = move_in_place(&mut map, &args, &[5_usize]).expect_err("line 0 collapses");
        assert_eq!(code(&error), "conflict", "zero-length code");
        assert!(
            error
                .to_string()
                .contains("the move makes linedef(s) 0 zero-length")
                && error.to_string().contains("other endpoint"),
            "the message names the line and the fix: {error}"
        );
        assert_eq!(map, fixture(MapFormat::Doom), "nothing moved");
        let refusal = collapse_refusal(&[3_usize, 6]).to_string();
        assert!(
            refusal.contains("linedef(s) 3, 6"),
            "several lines are listed: {refusal}"
        );
    }

    #[test]
    fn a_group_move_collapsing_two_lines_names_them_both() {
        let mut map = fixture(MapFormat::Doom);
        let args = move_args(r#"{"index": [1, 3], "dx": -256}"#);
        let error = move_in_place(&mut map, &args, &[1_usize, 3]).expect_err("lines 3 and 4 go");
        assert_eq!(code(&error), "conflict", "zero-length code");
        assert!(
            error.to_string().contains("linedef(s) 3, 4"),
            "both collapsed lines are named: {error}"
        );
        assert_eq!(map, fixture(MapFormat::Doom), "nothing moved");
    }

    #[test]
    fn a_move_onto_a_shared_point_without_a_line_spanning_it_is_fine() {
        let mut map = fixture(MapFormat::Doom);
        let args = move_args(r#"{"index": 5, "x": 512, "y": 0}"#);
        let rows =
            move_in_place(&mut map, &args, &[5_usize]).expect("no single line spans 5 and 2");
        assert_eq!(rows.len(), 1, "the far corner moved");
        assert_eq!(
            (map.vertices[5].x, map.vertices[5].y),
            (512_i32, 0_i32),
            "onto vertex 2's point"
        );
        assert_eq!(
            (map.vertices[2].x, map.vertices[2].y),
            (512_i32, 0_i32),
            "which stands where it was"
        );
    }

    #[test]
    fn zero_length_detection_reads_the_map_not_the_request() {
        let mut map = fixture(MapFormat::Doom);
        map.linedefs.push(Linedef {
            v1: 5,
            v2: 99,
            ..Default::default()
        });
        map.linedefs.push(solid_line(4, 4, 0));
        let moved = Moves::from([(5_usize, (0_i32, 0_i32))]);
        let spanning = map.linedefs.first().expect("line 0 (0,0 to 0,256)");
        assert!(
            made_zero_length(&map, &moved, spanning),
            "line 0 collapses when vertex 5 lands on (0, 0)"
        );
        let dangling = map.linedefs.get(7).expect("the dangling line");
        assert!(
            !made_zero_length(&map, &moved, dangling),
            "a dangling reference is not a collapsed line"
        );
        let self_spanning = map.linedefs.get(8).expect("the self-referencing line");
        assert!(
            !made_zero_length(&map, &moved, self_spanning),
            "a line that is already zero-length stays outside the refusal"
        );
        assert!(
            same_point(Some((1_i32, 2_i32)), Some((1_i32, 2_i32))),
            "equal points match"
        );
        assert!(!same_point(None, None), "two unknowns are not one point");
        assert_eq!(
            stored(&map, 0_usize),
            Some((0_i32, 0_i32)),
            "the stored point reads back"
        );
    }

    #[test]
    fn an_unreferenced_vertex_removes_without_a_cascade() {
        let mut map = fixture(MapFormat::Doom);
        let added = add_in_place(&mut map, &add_args(r#"{"x": 32, "y": 32}"#)).expect("one point");
        let report = remove_in_place(&mut map, &remove_args(r#"{"index": 6}"#), &[6_usize])
            .expect("the loose point goes");
        assert_eq!(added.index, 6, "the fixture has six vertices");
        assert_eq!(report.vertices, vec![6], "the removed original index");
        assert!(
            report.linedefs.is_empty() && report.sidedefs.is_empty(),
            "nothing else went: {report:?}"
        );
        assert_eq!(map.vertices.len(), 6, "and the list shrank back");
    }

    #[test]
    fn a_used_vertex_refuses_without_cascade_and_lists_its_lines() {
        let mut map = fixture(MapFormat::Doom);
        let before = map.clone();
        let error = remove_in_place(&mut map, &remove_args(r#"{"index": 4}"#), &[4_usize])
            .expect_err("three lines end at vertex 4");
        assert_eq!(code(&error), "conflict", "dependents code");
        assert!(
            error
                .to_string()
                .contains("vertex 4 is used by linedef(s) 1, 2, 4")
                && error.to_string().contains("cascade: true"),
            "the message lists the lines and the fix: {error}"
        );
        assert_eq!(map, before, "nothing was removed");
    }

    #[test]
    fn a_cascaded_vertex_removal_takes_its_lines_and_orphaned_sides() {
        let mut map = fixture(MapFormat::Doom);
        let report = remove_in_place(
            &mut map,
            &remove_args(r#"{"index": 4, "cascade": true}"#),
            &[4_usize],
        )
        .expect("the cascade lifts the conflict");
        assert_eq!(report.vertices, vec![4], "the seeded point");
        assert_eq!(
            report.linedefs,
            vec![1, 2, 4],
            "the three lines that used it"
        );
        assert_eq!(
            report.sidedefs,
            vec![1, 2, 3, 5],
            "the sides only those lines had"
        );
        assert!(
            report.sectors.is_empty() && report.things.is_empty(),
            "nothing else: {report:?}"
        );
        assert_eq!(map.vertices.len(), 5, "one vertex dropped");
        assert_eq!(map.linedefs.len(), 4, "three lines dropped");
        let corner_gone = !map
            .vertices
            .iter()
            .any(|vertex| vertex.x == 256_i32 && vertex.y == 256_i32);
        assert!(corner_gone, "the removed point left the vertex list");
    }

    #[test]
    fn the_pruning_flags_reach_the_cascade() {
        let defaults = cascade_options(&remove_args(r#"{"index": 4, "cascade": true}"#));
        assert!(
            defaults.prune_sidedefs && defaults.prune_vertices,
            "pruning is on by default"
        );
        let let_alone = remove_args(
            r#"{"index": 4, "cascade": true, "prune_sidedefs": false, "prune_vertices": false}"#,
        );
        let off = cascade_options(&let_alone);
        assert!(
            !off.prune_sidedefs && !off.prune_vertices,
            "both switches reach the options"
        );
        let mut map = fixture(MapFormat::Doom);
        let report = remove_in_place(&mut map, &let_alone, &[4_usize])
            .expect("the unpruned cascade still applies");
        assert_eq!(report.linedefs, vec![1, 2, 4], "the seeded rule still runs");
        assert!(report.sidedefs.is_empty(), "the sides stay orphaned");
        assert_eq!(map.vertices.len(), 5, "only the selected vertex went");
        assert_eq!(map.sidedefs.len(), 8, "and no side followed it");
    }

    #[test]
    fn removing_a_missing_index_reports_not_found() {
        let mut map = fixture(MapFormat::Doom);
        let error = remove_in_place(&mut map, &remove_args(r#"{"index": 40}"#), &[40_usize])
            .expect_err("the map holds six vertices");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert_eq!(map, fixture(MapFormat::Doom), "nothing removed");
    }
}
