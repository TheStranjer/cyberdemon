//! `delete_geometry`'s arguments, report and working steps: the mixed
//! selection folded into one [`crate::wad::map::remove`] call.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`DeleteGeometryParams`] to [`delete_run`]. That validates the request
//! ([`has_selection`], [`seed_removal`], [`check_selection`]), turns the
//! pruning switches into [`cascade_options`] and then either runs
//! [`delete_in_place`] as one [`crate::session::Session::edit_map`] step or,
//! for `dry_run`, computes the same thing on a copy through [`dry_delete`].

use alloc::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, edit_result_fragment, untouched_fragment};
use crate::wad::map::Map;
use crate::wad::map::query::check_indices;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, expand_cascade,
};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::delete_geometry`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct DeleteGeometryParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Indices into the map's vertex block to delete.
    #[serde(default)]
    pub(super) vertices: Option<Vec<usize>>,
    /// Indices into the map's linedef block to delete.
    #[serde(default)]
    pub(super) linedefs: Option<Vec<usize>>,
    /// Indices into the map's sidedef block to delete.
    #[serde(default)]
    pub(super) sidedefs: Option<Vec<usize>>,
    /// Indices into the map's sector block to delete.
    #[serde(default)]
    pub(super) sectors: Option<Vec<usize>>,
    /// Indices into the map's thing block to delete.
    #[serde(default)]
    pub(super) things: Option<Vec<usize>>,
    /// Within the cascade, also remove sidedefs no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    pub(super) prune_sidedefs: Option<bool>,
    /// Within the cascade, also remove vertices no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    pub(super) prune_vertices: Option<bool>,
    /// Compute the whole cascade on a copy and report it, touching neither
    /// the working archive nor the undo stack (default `false`).
    #[serde(default)]
    pub(super) dry_run: bool,
}

/// Structured result of
/// [`Cyberdemon::delete_geometry`](crate::server::Cyberdemon).
#[derive(Serialize)]
pub(super) struct DeleteReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// Whether nothing changed because the call only reported.
    dry_run: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The request's five index lists as slices, absent ones empty, in the
/// order the blocks are checked.
pub(super) fn selected_lists(args: &DeleteGeometryParams) -> [&[usize]; 5] {
    [
        args.vertices.as_deref().unwrap_or_default(),
        args.linedefs.as_deref().unwrap_or_default(),
        args.sidedefs.as_deref().unwrap_or_default(),
        args.sectors.as_deref().unwrap_or_default(),
        args.things.as_deref().unwrap_or_default(),
    ]
}

/// Whether the request names at least one index in any of its lists.
pub(super) fn has_selection(args: &DeleteGeometryParams) -> bool {
    selected_lists(args)
        .into_iter()
        .any(|list| !list.is_empty())
}

/// The requested indices of one list as a set; duplicates collapse.
fn index_set(indices: Option<&Vec<usize>>) -> BTreeSet<usize> {
    indices.map_or_else(BTreeSet::new, |list| list.iter().copied().collect())
}

/// The seed removal of exactly the selected indices.
pub(super) fn seed_removal(args: &DeleteGeometryParams) -> Removal {
    Removal {
        vertices: index_set(args.vertices.as_ref()),
        linedefs: index_set(args.linedefs.as_ref()),
        sidedefs: index_set(args.sidedefs.as_ref()),
        sectors: index_set(args.sectors.as_ref()),
        things: index_set(args.things.as_ref()),
    }
}

/// The number of seeded indices across all five kinds.
pub(super) fn removal_size(removal: &Removal) -> usize {
    removal
        .vertices
        .len()
        .saturating_add(removal.linedefs.len())
        .saturating_add(removal.sidedefs.len())
        .saturating_add(removal.sectors.len())
        .saturating_add(removal.things.len())
}

/// The pruning flags of a call, the cascading default when either is
/// absent.
pub(super) fn cascade_options(args: &DeleteGeometryParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// Refuses unless every selected index exists in its block of `map`,
/// kinds in vertex, linedef, sidedef, sector, thing order, through the
/// shared [`check_indices`] guard.
///
/// [`check_indices`]: crate::wad::map::query::check_indices
pub(super) fn check_selection(map: &Map, removal: &Removal) -> Result<(), CyberError> {
    check_indices(ElementKind::Vertices, &removal.vertices, map)?;
    check_indices(ElementKind::Linedefs, &removal.linedefs, map)?;
    check_indices(ElementKind::Sidedefs, &removal.sidedefs, map)?;
    check_indices(ElementKind::Sectors, &removal.sectors, map)?;
    check_indices(ElementKind::Things, &removal.things, map)
}

/// The deletion step: check the selection, grow it with the always-on
/// cascade and run the removal, returning what the engine did.
pub(super) fn delete_in_place(
    map: &mut Map,
    opts: CascadeOptions,
    removal: &Removal,
) -> Result<RemovalReport, CyberError> {
    check_selection(map, removal)?;
    let grown = expand_cascade(map, removal, opts);
    apply_removal(map, &grown)
}

/// The `invalid_argument` when no list carries an index.
fn empty_selection() -> CyberError {
    CyberError::InvalidArgument(
        "nothing selected: pass at least one non-empty list among vertices, linedefs, \
         sidedefs, sectors and things"
            .to_owned(),
    )
}

/// `delete_geometry`'s work: validate the selection, then run the cascade
/// as one undoable step, or compute it on a copy for a dry run.
pub(super) fn delete_run(
    session: &mut Session,
    args: &DeleteGeometryParams,
) -> Result<DeleteReport, CyberError> {
    if !has_selection(args) {
        return Err(empty_selection());
    }
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let removal = seed_removal(args);
    let opts = cascade_options(args);
    if args.dry_run {
        return dry_delete(session, name, opts, &removal);
    }
    let label = format!("delete {} map elements", removal_size(&removal));
    let (removed, report) =
        session.edit_map(name, &label, |map| delete_in_place(map, opts, &removal))?;
    Ok(DeleteReport {
        removed,
        dry_run: false,
        edit: edit_result_fragment(&report),
    })
}

/// The dry run: the same step computed on a loaded copy, reported with an
/// untouched fragment and no write.
fn dry_delete(
    session: &Session,
    name: LumpName,
    opts: CascadeOptions,
    removal: &Removal,
) -> Result<DeleteReport, CyberError> {
    let mut copy = common::current_map(session, name)?;
    let removed = delete_in_place(&mut copy, opts, removal)?;
    Ok(DeleteReport {
        removed,
        dry_run: true,
        edit: untouched_fragment(name),
    })
}
