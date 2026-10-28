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
//! [`edit_result_fragment`]) and the shared `join`, `find_vertex` and
//! `untouched_fragment` helpers come from [`crate::tools::maps::params`];
//! the working-then-merged map read from [`crate::tools::maps::common`].
//! Removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a vertex the linedefs still use unless `cascade` is set.
//!
//! The geometry rule: a vertex is geometry for the node lumps while a
//! linedef ends at it. Editing a referenced vertex changes geometry — the
//! generated node lumps are emptied, the block's `GL` run is dropped and the
//! result reports `nodes_invalidated: true`. The edit takes the shed run's
//! own split points with it (the trailing vertices its `SEGS` records name,
//! which is where `build_nodes` keeps them) and no other: a trailing point
//! the run never named is the caller's, an `add_vertex` append waiting for a
//! line to end at it, and it survives whatever the nodes do. Run
//! `build_nodes` before playing such a map in vanilla `Doom`; `ZDoom`-family
//! ports rebuild the missing node data themselves. Trailing vertices no
//! linedef references are invisible to the node builder: an `add_vertex`
//! append keeps the nodes trusted until a line ends at the new point.
//!
//! The format rules: the binary formats store each coordinate as `i16`, so
//! `Doom` and `Hexen` maps keep `x`/`y` inside `-32768..=32767` and a value
//! outside that is `invalid_argument`; `UDMF` stores full integers and has
//! no such cap. A move that would put both ends of one linedef on the same
//! coordinates — a zero-length line, which no node builder can use — is
//! refused with `conflict` naming the linedef.
//!
//! This file keeps the parameter and report shapes, the append and removal
//! paths and the three router methods; reading a `move_vertex` request onto
//! the map's points and refusing the linedefs it would collapse is the
//! private `moves` module. Both are implementation details of the tools'
//! one-step edits.
//!
//! [`check_index`]: crate::tools::maps::params::check_index

mod moves;

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::tools::edit_vertices::moves::{check_vertices, move_run};
use crate::tools::maps::common;
use crate::tools::maps::params::{
    IndexSelector, edit_result_fragment, find_vertex, fits_coord, join, untouched_fragment,
};
use crate::tools::reply;
use crate::wad::classify::MapFormat;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};
use crate::wad::map::{Map, Vertex};

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
    check_vertices(indices, map)?;
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
        find_vertex(&common::current_map(session, name)?, args.x, args.y)
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

/// `remove_vertex`'s work: resolve the map and run the removal.
fn remove_run(
    session: &mut Session,
    args: &RemoveVertexParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} vertices", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_vertices_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one vertex to a map, or returns the point already there.
    #[tool(
        description = "Adds one vertex (a point of the map's geometry) to a map in a single undoable edit: x and y in map units, and reuse_existing (default true) answers the index of a vertex already sitting exactly there without writing. Prefer create_sector_from_loop to draw a room and its wall vertices in one call, or build_map_from_plan for a whole map; a lone vertex does nothing until a linedef ends at it."
    )]
    fn add_vertex(
        &self,
        Parameters(args): Parameters<AddVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Moves one vertex, or a whole group of them, in one undoable edit.
    #[tool(
        description = "Moves one vertex, or several at once by the same delta, of a map in a single undoable edit: absolute x/y for a single index or dx/dy for any list; every linedef ending there follows, and a move collapsing a line to zero length fails conflict. Geometry change: run build_all_nodes before vanilla play. Prefer transform_selection to scale, rotate or move a whole group coherently."
    )]
    fn move_vertex(
        &self,
        Parameters(args): Parameters<MoveVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| move_run(session, &args)),
        )
    }

    /// Removes one or several vertices in a single undoable edit.
    #[tool(
        description = "Removes one vertex or a list of them from a map in a single undoable edit; the remaining vertices renumber down. Without cascade a vertex a linedef still uses is refused with conflict naming the lines; cascade: true removes those lines too, with the flip and one-sided normalisation of the removal engine. Geometry change - rebuild nodes before vanilla play; for mixed selections prefer delete_geometry."
    )]
    fn remove_vertex(
        &self,
        Parameters(args): Parameters<RemoveVertexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
