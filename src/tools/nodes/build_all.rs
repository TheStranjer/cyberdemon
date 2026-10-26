//! `build_all_nodes`: tree, `REJECT` and `BLOCKMAP` in one undoable edit.
//!
//! [`plan_all`] resolves one call's parameters against the map's format,
//! starting from [`AllPlan::for_model`] — the format's whole-set defaults,
//! which is also exactly what [`Cyberdemon::build_map_from_plan`]'s
//! `finalize` step asks for. [`apply_all`] is the combined edit closure:
//! the tree through [`apply_build_nodes`], then [`apply_build_reject`] and
//! [`apply_build_blockmap`] on the same model, so one refusal rolls
//! everything back; [`all_edit_step`] adds [`splice_gl_run`] when the plan
//! built a `gl` run and [`all_nodes_run`] turns one call into a single
//! [`Session::edit`], reported as [`AllNodesReport`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::map_edit::{self, MapEditReport};
use crate::session::{MergedLump, Scope, Session};
use crate::tools::maps::params::edit_result_fragment;
use crate::wad::archive::Wad;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::build_reject::RejectMode;
use crate::wad::name::LumpName;

use super::build_gl::splice_gl_run;
use super::build_nodes::{
    DEFAULT_SPLIT_COST, NodeBuildPlan, NodesBuildBody, NodesBuilt, Variant, apply_build_nodes,
};
use super::build_tables::{
    BlockmapBuildBody, RejectBuildBody, apply_build_blockmap, apply_build_reject, blockmap_body,
    reject_body,
};
use super::{Detail, Request, open_map};

/// Arguments for [`Cyberdemon::build_all_nodes`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildAllNodesParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The node layout, exactly as in [`BuildNodesParams::variant`]:
    /// `"classic"`, `"extended"` or `"gl"`. Left out, a binary map takes
    /// `"classic"` and a `UDMF` one `"extended"`.
    #[serde(default)]
    pub(super) variant: Option<String>,
    /// The reject matrix mode, as in [`BuildRejectParams::mode`]: `"zero"`
    /// (the default) or `"components"`. A binary map always gets a fresh
    /// `REJECT`; a `UDMF` map carries none (`ZDoom` builds the matrix
    /// itself), so naming a mode on one is refused.
    #[serde(default)]
    pub(super) reject_mode: Option<String>,
    /// Compress what can be compressed: an extended lump behind the `ZNOD`
    /// magic and identical `BLOCKMAP` blocklists (the default, `true`).
    #[serde(default)]
    pub(super) compress: Option<bool>,
}

/// Everything one [`Cyberdemon::build_all_nodes`] call writes: the node
/// plan plus which of the two generated side lumps join the tree. Also
/// what [`Cyberdemon::build_map_from_plan`]'s `finalize` step runs:
/// [`AllPlan::for_model`] holds the format's defaults there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tools) struct AllPlan {
    /// The tree build: layout, cost and compression.
    pub(super) nodes: NodeBuildPlan,
    /// The reject matrix to write, when the map gets one.
    pub(super) reject: Option<RejectMode>,
    /// Whether the map also gets a fresh `BLOCKMAP`.
    pub(super) blockmap: bool,
}

impl AllPlan {
    /// The whole-set defaults for `model`'s format: `classic` for a
    /// binary map and `extended` for a `UDMF` one, the `zero` reject and
    /// a fresh `BLOCKMAP` for binary maps only (`ZDoom` builds both
    /// itself for `UDMF`), `compress` on and [`DEFAULT_SPLIT_COST`].
    /// `build_map_from_plan`'s `finalize` builds exactly this;
    /// [`plan_all`] starts here and lets the wire parameters override
    /// the knobs.
    pub(in crate::tools) fn for_model(model: &Map) -> Self {
        let udmf = model.format == MapFormat::Udmf;
        Self {
            nodes: NodeBuildPlan {
                variant: Variant::for_format(model.format),
                split_cost: DEFAULT_SPLIT_COST,
                compress: true,
            },
            reject: (!udmf).then_some(RejectMode::Zero),
            blockmap: !udmf,
        }
    }
}

/// What one [`Cyberdemon::build_all_nodes`] edit wrote.
pub(in crate::tools) struct AllBuilt {
    /// The node run, in the layout the plan chose.
    nodes: NodesBuilt,
    /// The reject matrix body, when the plan wrote one.
    reject: Option<RejectBuildBody>,
    /// The blockmap body, when the plan wrote one.
    blockmap: Option<BlockmapBuildBody>,
}

impl AllBuilt {
    /// The node build's counts as the `NodesBuildBody`-shaped `JSON`
    /// `build_map_from_plan`'s `finalize` hands back; the reject and
    /// blockmap bodies go unused, their lumps are in the map either way.
    pub(in crate::tools) fn into_nodes_json(self) -> Result<Value, CyberError> {
        serde_json::to_value(self.nodes.into_body()).map_err(|error| {
            CyberError::Internal(format!(
                "the node build summary does not serialize: {error}"
            ))
        })
    }
}

/// Resolve one `build_all_nodes` call's arguments against the map's
/// format: the node layout (`classic` for a binary map, `extended` for a
/// `UDMF` one when the caller does not choose), the reject mode (binary
/// maps always write one, a `UDMF` map carrying none refuses any named
/// mode) and the blockmap (binary maps only, which `ZDoom` builds itself
/// for `UDMF`).
///
/// # Errors
///
/// [`Variant::resolve`]'s and [`RejectMode::parse`]'s refusals, and the
/// `UDMF` reject refusal below.
pub(super) fn plan_all(model: &Map, args: &BuildAllNodesParams) -> Result<AllPlan, CyberError> {
    let mut plan = AllPlan::for_model(model);
    plan.nodes.variant = Variant::resolve(args.variant.as_deref(), model.format)?;
    if let Some(text) = args.reject_mode.as_deref() {
        if model.format == MapFormat::Udmf {
            return Err(udmf_reject_refusal());
        }
        plan.reject = Some(RejectMode::parse(text)?);
    }
    plan.nodes.compress = args.compress.unwrap_or(true);
    Ok(plan)
}

/// The refusal of a `reject_mode` named on a `UDMF` map, whose block has
/// no place for a `REJECT` lump and whose engine builds the matrix itself.
fn udmf_reject_refusal() -> CyberError {
    CyberError::InvalidArgument(
        "UDMF maps carry no REJECT: ZDoom builds the visibility matrix \
         itself; leave reject_mode out for a UDMF map"
            .to_owned(),
    )
}

/// The combined edit closure of [`Cyberdemon::build_all_nodes`]: the
/// planned tree, then the reject and the blockmap, all on the one model
/// so one refusal rolls everything back. `build_map_from_plan`'s
/// `finalize` runs the same step on its compiled model.
///
/// # Errors
///
/// [`apply_build_nodes`], [`apply_build_reject`] and
/// [`apply_build_blockmap`]' refusals.
pub(in crate::tools) fn apply_all(map: &mut Map, plan: &AllPlan) -> Result<AllBuilt, CyberError> {
    let nodes = apply_build_nodes(map, plan.nodes)?;
    let reject = plan
        .reject
        .map(|mode| -> Result<RejectBuildBody, CyberError> {
            let built = apply_build_reject(map, mode)?;
            Ok(reject_body(&built, mode))
        })
        .transpose()?;
    let blockmap = plan
        .blockmap
        .then(|| -> Result<BlockmapBuildBody, CyberError> {
            let built = apply_build_blockmap(map, plan.nodes.compress)?;
            Ok(blockmap_body(&built))
        })
        .transpose()?;
    Ok(AllBuilt {
        nodes,
        reject,
        blockmap,
    })
}

/// The one edit step of a `build_all_nodes` call: the tree-plus-reject-
/// plus-blockmap rewrite through [`map_edit::edit_in_place`], then the
/// `GL_*` block spliced in behind the map block when the plan built a
/// `gl` run — one undo entry carrying everything.
///
/// # Errors
///
/// [`map_edit::edit_in_place`] and [`install_gl_run`]'s refusals.
fn all_edit_step(
    wad: &mut Wad,
    merged: &[MergedLump],
    name: LumpName,
    plan: &AllPlan,
) -> Result<(AllBuilt, MapEditReport), CyberError> {
    let (mut built, report) =
        map_edit::edit_in_place(wad, merged, name, |map| apply_all(map, plan))?;
    built.nodes = splice_gl_run(wad, name, built.nodes)?;
    Ok((built, report))
}

/// Structured result of [`Cyberdemon::build_all_nodes`].
#[derive(Debug, Serialize)]
pub(super) struct AllNodesReport {
    /// The node layout that was built: `"classic"`, `"extended"` or `"gl"`.
    pub(super) variant: &'static str,
    /// What the tree build wrote, exactly the `build_nodes` keys.
    pub(super) nodes: NodesBuildBody,
    /// What the reject build wrote; `null` when the plan wrote none.
    pub(super) reject: Option<RejectBuildBody>,
    /// What the blockmap build wrote; `null` when the plan wrote none.
    pub(super) blockmap: Option<BlockmapBuildBody>,
    /// The standard edit fragment.
    #[serde(flatten)]
    pub(super) edit: JsonMap<String, Value>,
}

/// The `build_all_nodes` result: the three bodies and the fragment.
fn all_built(built: AllBuilt, edit: JsonMap<String, Value>) -> AllNodesReport {
    let nodes = built.nodes.into_body();
    AllNodesReport {
        variant: nodes.variant,
        nodes,
        reject: built.reject,
        blockmap: built.blockmap,
        edit,
    }
}

/// `build_all_nodes`' work: read the map's format, plan the whole build
/// against it and run everything as one undoable edit.
pub(super) fn all_nodes_run(
    session: &mut Session,
    args: &BuildAllNodesParams,
) -> Result<AllNodesReport, CyberError> {
    let request = Request {
        map: args.map.clone(),
        scope: Scope::Merged,
        detail: Detail::Summary,
    };
    let (name, _dir, _block, model) = open_map(session, &request)?;
    let plan = plan_all(&model, args)?;
    let merged = session.merged();
    let (built, report) = session.edit("build all nodes", |wad| {
        all_edit_step(wad, &merged, name, &plan)
    })?;
    Ok(all_built(built, edit_result_fragment(&report)))
}
