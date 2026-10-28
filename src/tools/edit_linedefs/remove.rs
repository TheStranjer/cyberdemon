//! The three remaining linedef tools: `remove_linedef`, `split_linedef` and
//! `join_linedefs`, each one [`crate::session::Session::edit_map`] step.
//!
//! [`remove_run`] hands the call's pruning flags to the shared scaffold in
//! [`crate::tools::maps::remove`] as an always-cascading [`remove::RemovePlan`]
//! — a linedef is referenced by nothing, so its removal only ever grows the
//! set through the cascade; [`split_run`] and [`join_run`] are thin wrappers
//! over the pure [`crate::wad::map::ops`] engine, the split taking its
//! optional cut point as a [`PointParam`]. Every result is its engine's
//! report ([`remove::RemoveToolReport`], [`SplitToolReport`] and
//! [`JoinToolReport`]) beside the standard edit fragment.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, IndexSelector, PointParam, edit_result_fragment};
use crate::tools::maps::remove::{self, RemoveToolReport, cascade_options};
use crate::wad::map::ops;

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

/// `remove_linedef`'s work: resolve the map and run the shared removal
/// step as the always-cascading plan the call's pruning flags describe.
pub(super) fn remove_run(
    session: &mut Session,
    args: &RemoveLinedefParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.indices()?;
    remove::remove_run(
        session,
        &remove::RemovePlan {
            kind: ElementKind::Linedefs,
            map: args.map.as_deref(),
            indices: &indices,
            cascade: Some(cascade_options(args.prune_sidedefs, args.prune_vertices)),
        },
    )
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
