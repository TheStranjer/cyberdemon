//! The three remaining linedef tools: `remove_linedef`, `split_linedef` and
//! `join_linedefs`, each one [`crate::session::Session::edit_map`] step.
//!
//! [`remove_run`] checks the indices and hands the cascade-grown set to
//! [`remove_in_place`], with [`cascade_options`] reading the call's pruning
//! flags; [`split_run`] and [`join_run`] are thin wrappers over the pure
//! [`crate::wad::map::ops`] engine, the split taking its optional cut point
//! as a [`PointParam`]. Every result is its engine's report
//! ([`RemoveToolReport`], [`SplitToolReport`] and [`JoinToolReport`]) beside
//! the standard edit fragment.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{IndexSelector, PointParam, check_index, edit_result_fragment};
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, expand_cascade,
};
use crate::wad::map::{Map, ops};

/// Arguments for [`Cyberdemon::remove_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RemoveLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The linedef to remove: one index or a list of them.
    pub(super) index: IndexSelector,
    /// Within the cascade, also remove sidedefs no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    pub(super) prune_sidedefs: Option<bool>,
    /// Within the cascade, also remove vertices no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    pub(super) prune_vertices: Option<bool>,
}

/// Arguments for [`Cyberdemon::split_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SplitLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef to cut.
    index: usize,
    /// The cut point `{x, y}`: on the segment (within one map unit of the
    /// line, strictly between the endpoints). Defaults to the midpoint,
    /// rounded to nearest.
    #[serde(default)]
    at: Option<PointParam>,
}

/// Arguments for [`Cyberdemon::join_linedefs`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct JoinLinedefsParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef that survives the join (its properties win under `force`).
    first: usize,
    /// The linedef merged into the first and removed.
    second: usize,
    /// Join even though the two lines' properties or side textures differ,
    /// keeping the first linedef's (default `false`).
    #[serde(default)]
    force: Option<bool>,
}

/// Structured result of [`Cyberdemon::remove_linedef`].
#[derive(Serialize)]
pub(super) struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::split_linedef`].
#[derive(Serialize)]
pub(super) struct SplitToolReport {
    /// What the split engine built, inlined next to the edit fragment.
    #[serde(flatten)]
    result: ops::SplitResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::join_linedefs`].
#[derive(Serialize)]
pub(super) struct JoinToolReport {
    /// What the join engine welded and removed, inlined next to the edit
    /// fragment.
    #[serde(flatten)]
    result: ops::JoinResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The pruning flags of a `remove_linedef` call, the cascading default
/// when either is absent.
fn cascade_options(args: &RemoveLinedefParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// `remove_linedef`'s step: check the indices and run the cascade-grown
/// removal.
pub(super) fn remove_in_place(
    map: &mut Map,
    args: &RemoveLinedefParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    for index in indices {
        check_index("linedef", *index, map.linedefs.len())?;
    }
    let removal = Removal {
        linedefs: indices.iter().copied().collect(),
        ..Default::default()
    };
    let grown = expand_cascade(map, &removal, cascade_options(args));
    apply_removal(map, &grown)
}

/// `remove_linedef`'s work: resolve the map and run the removal.
pub(super) fn remove_run(
    session: &mut Session,
    args: &RemoveLinedefParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} linedefs", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

/// `split_linedef`'s work: resolve the map and run the one-step split.
pub(super) fn split_run(
    session: &mut Session,
    args: &SplitLinedefParams,
) -> Result<SplitToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("split linedef {}", args.index);
    let (result, report) = session.edit_map(name, &label, |map| {
        ops::split_linedef(
            map,
            args.index,
            args.at.map(|at| ops::Point { x: at.x, y: at.y }),
        )
    })?;
    Ok(SplitToolReport {
        result,
        edit: edit_result_fragment(&report),
    })
}

/// `join_linedefs`' work: resolve the map and run the one-step join.
pub(super) fn join_run(
    session: &mut Session,
    args: &JoinLinedefsParams,
) -> Result<JoinToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("join linedefs {} and {}", args.first, args.second);
    let (result, report) = session.edit_map(name, &label, |map| {
        ops::join_linedefs(map, args.first, args.second, args.force.unwrap_or(false))
    })?;
    Ok(JoinToolReport {
        result,
        edit: edit_result_fragment(&report),
    })
}
