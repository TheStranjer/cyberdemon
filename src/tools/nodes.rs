//! Nodes tools: [`Cyberdemon::get_nodes`], [`Cyberdemon::get_reject`] and
//! [`Cyberdemon::get_blockmap`], the inspection side of the node-builder
//! output lumps parsed by [`crate::wad::map::nodes`],
//! [`crate::wad::map::reject`] and [`crate::wad::map::blockmap`], plus the
//! builders [`Cyberdemon::build_blockmap`],
//! [`Cyberdemon::build_reject`] and [`Cyberdemon::build_nodes`], which
//! recompute the `BLOCKMAP`, the `REJECT` and the `SEGS`/`SSECTORS`/`NODES`
//! tree from the map's current geometry as single undoable edits (see
//! [`crate::wad::map::build_blockmap`], [`crate::wad::map::build_reject`]
//! and [`crate::bsp`]).
//!
//! All three readers take the usual `map` and `scope` parameters and a
//! `detail` knob: `summary` (the default) reports measurable numbers only,
//! `full` additionally publishes the parsed structure as a `JSON` asset,
//! held back until the session lock is released. Missing or empty lumps
//! are never errors, they report as absent data; malformed lumps fail with
//! `format_error`. The two lump builders take the usual `map` parameter
//! plus their judgment knob (`compress` / `mode`) and rewrite only their
//! own lump in the block, so no node data is invalidated.
//! [`Cyberdemon::build_nodes`] writes the whole vanilla run through
//! [`crate::bsp::to_vanilla`] (its split vertices are appended to
//! `VERTEXES` and cleaned up again by the next geometry edit), one
//! `ZDoom` extended lump through [`crate::bsp::to_extended`] (stored in
//! `NODES` of a binary block or in `ZNODES` of a `UDMF` one, with the
//! split vertices kept inside the lump and `VERTEXES` untouched), or the
//! `glBSP` v2 run through [`crate::bsp::to_gl`] — the classic lumps plus
//! a `GL_*` block attached right after the map block, written as one
//! undoable [`Session::edit`]. [`Cyberdemon::build_all_nodes`] runs the
//! whole set at once — the tree, the `REJECT` and the `BLOCKMAP` (a
//! `UDMF` block gets `ZNODES` alone unless a reject mode is asked for) —
//! as a single [`Session::edit`], sharing the `apply_build_nodes`,
//! [`build_tables::apply_build_reject`] and
//! [`build_tables::apply_build_blockmap`] steps with the three
//! single-lump builders. The [`build_all::apply_all`] step and the
//! [`build_all::AllPlan::for_model`] default of that combined build are
//! also what [`Cyberdemon::build_map_from_plan`]'s `finalize` parameter
//! runs on its compiled model.
//!
//! The work is split by tool: [`get_nodes`] reads the node run and
//! [`get_tables`] the `REJECT` and `BLOCKMAP` lumps, [`build_tables`]
//! writes the two side lumps, [`build_nodes`] the tree in its three
//! layouts, [`build_gl`] the `GL_*` block a `gl` build splices around the
//! map and [`build_all`] the combined whole-set run. This root keeps what
//! the group shares - the asset-extension and lump-magic constants, the
//! three parameters every reader resolves, the map-open and lump lookups
//! behind them, the held-back publish plumbing and the router - and the
//! group's unit tests live in `nodes/tests.rs`.

mod build_all;
mod build_gl;
mod build_nodes;
mod build_tables;
mod get_nodes;
mod get_tables;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use serde::Serialize;

use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::maps::common::{self, Directory};
use crate::tools::nodes::build_all::{BuildAllNodesParams, all_nodes_run};
use crate::tools::nodes::build_nodes::{BuildNodesParams, nodes_build_run};
use crate::tools::nodes::build_tables::{
    BuildBlockmapParams, BuildRejectParams, blockmap_build_run, reject_build_run,
};
use crate::tools::nodes::get_nodes::{GetNodesParams, attach_nodes, nodes_run};
use crate::tools::nodes::get_tables::{
    GetBlockmapParams, GetRejectParams, attach_blockmap, attach_reject, blockmap_run, reject_run,
};
use crate::tools::{json_bytes, reply};
use crate::wad::archive::Lump;
use crate::wad::classify::MapBlock;
use crate::wad::map::{Map, read_map};
use crate::wad::name::LumpName;

// The whole-set build behind `build_map_from_plan`'s `finalize`, kept at
// the path the `plan` group already imports; both live in `build_all`.
pub(in crate::tools) use crate::tools::nodes::build_all::{AllPlan, apply_all};

/// File extension of the published `JSON` assets.
const JSON_EXTENSION: &str = "json";
/// Bytes in a `GL` lump magic.
const MAGIC_LEN: usize = 4;

/// How much of a result to publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detail {
    /// Measurable numbers only.
    Summary,
    /// Also publish the parsed structure as a `JSON` asset.
    Full,
}

/// One nodes tool call's resolved arguments.
#[derive(Debug)]
struct Request {
    /// The requested map name, if any.
    map: Option<String>,
    /// The archive to read.
    scope: Scope,
    /// How much to publish.
    detail: Detail,
}

/// The accepted `detail` text, `summary` being the default.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the two accepted values.
fn parse_detail(text: Option<&str>) -> Result<Detail, CyberError> {
    let lowered = match text {
        None => return Ok(Detail::Summary),
        Some(given) => given.to_ascii_lowercase(),
    };
    match lowered.as_str() {
        "" | "summary" => Ok(Detail::Summary),
        "full" => Ok(Detail::Full),
        other => Err(CyberError::InvalidArgument(format!(
            "detail {other:?} is neither \"summary\" nor \"full\"; pass one \
             of those two or leave the parameter out"
        ))),
    }
}

/// Resolve the three shared parameters of a nodes tool into a request,
/// map kept owned so the request borrows nothing.
///
/// # Errors
///
/// [`Scope::parse_or_merged`]'s and [`parse_detail`]'s refusals.
fn resolve_request(
    map: Option<String>,
    scope: Option<&str>,
    detail: Option<&str>,
) -> Result<Request, CyberError> {
    Ok(Request {
        map,
        scope: Scope::parse_or_merged(scope)?,
        detail: parse_detail(detail)?,
    })
}

/// One map block, decoded model and classified directory plus the name a
/// nodes tool resolved, in that order.
type Opened = (LumpName, Directory, MapBlock, Map);

/// The map block, decoded model and classified directory one nodes tool
/// reads, resolving `map` the way every map tool does.
///
/// # Errors
///
/// [`common::resolve_map_name`], [`common::directory`] and
/// [`read_map`]'s errors.
fn open_map(session: &Session, request: &Request) -> Result<Opened, CyberError> {
    let name = common::resolve_map_name(session, request.map.as_deref())?;
    let dir = common::directory(session, request.scope)?;
    let block = dir
        .find_block(name)
        .ok_or_else(|| common::missing_map(name, request.scope))?
        .clone();
    let map = read_map(&dir.lumps, &block)?;
    Ok((name, dir, block, map))
}

/// The last lump of `lumps` named `wanted`, engines reading the last
/// copy of a run.
fn find_lump(lumps: &[Lump], wanted: LumpName) -> Option<&Lump> {
    lumps.iter().rev().find(|lump| lump.name == wanted)
}

/// The payload of the last lump named `wanted`; empty when absent.
fn lump_bytes(lumps: &[Lump], wanted: LumpName) -> &[u8] {
    find_lump(lumps, wanted).map_or(b"", |lump| lump.data.as_ref())
}

/// A `JSON` body held back until the session lock is released.
#[derive(Debug)]
struct Publish {
    /// File name to publish under.
    filename: String,
    /// The serialized body.
    data: Vec<u8>,
}

/// Publishes a held-back asset into the report's `asset` slot.
///
/// # Errors
///
/// [`AppState::publish`]'s errors.
fn deliver<T, F>(
    state: &AppState,
    mut report: T,
    publish: Option<Publish>,
    attach: F,
) -> Result<T, CyberError>
where
    F: FnOnce(&mut T, AssetRef),
{
    if let Some(job) = publish {
        let asset = state.publish(&job.filename, mime_for_extension(JSON_EXTENSION), job.data)?;
        attach(&mut report, asset);
    }
    Ok(report)
}

/// The session-locked result of one nodes tool: report plus held-back job.
type Planned<T> = (T, Option<Publish>);

/// The locked read of one nodes tool.
type Read<T> = Result<Planned<T>, CyberError>;

/// The session-lock work of one nodes tool: read, report, hold back.
type ToolWork<T> = fn(&Session, &Request) -> Read<T>;

/// Runs one of the three nodes tools end to end: resolve the shared
/// parameters, read under the session lock, then publish outside it.
fn nodes_call<T, F>(
    state: &AppState,
    request: Result<Request, CyberError>,
    attach: F,
    work: ToolWork<T>,
) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
    F: FnOnce(&mut T, AssetRef),
{
    reply(request.and_then(|resolved| {
        let (report, publish) = state.with_session(|session| work(session, &resolved))?;
        deliver(state, report, publish, attach)
    }))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = nodes_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports the node-builder output of one map.
    #[tool(
        description = "Reads a map's node-builder output: the BSP tree's seg, subsector and node counts, the layout it is stored in (classic, extended XNOD/ZNOD, plus a GL run when present), depth and extra vertices - parsed, never judged. map and scope work as in the other map readers. Use it to check whether node data exists or looks stale before running build_all_nodes."
    )]
    fn get_nodes(
        &self,
        Parameters(args): Parameters<GetNodesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        nodes_call(
            self.state(),
            resolve_request(args.map, args.scope.as_deref(), args.detail.as_deref()),
            attach_nodes,
            nodes_run,
        )
    }

    /// Reports the reject matrix of one map.
    #[tool(
        description = "Reads a map's REJECT lump: the sector visibility matrix's size, expected size, all_zero flag and rejected-pair count; a missing or empty lump is reported, not an error. detail \"full\" also publishes the per-row bits. Use it to inspect culling data; build_reject recomputes it and build_all_nodes rebuilds the whole set."
    )]
    fn get_reject(
        &self,
        Parameters(args): Parameters<GetRejectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        nodes_call(
            self.state(),
            resolve_request(args.map, args.scope.as_deref(), args.detail.as_deref()),
            attach_reject,
            reject_run,
        )
    }

    /// Reports the blockmap of one map.
    #[tool(
        description = "Reads a map's BLOCKMAP lump: the grid's origin, columns, rows, block count, total linedef entries and empty blocks; a missing or empty lump reports zeros and is not an error. detail \"full\" publishes the per-block lists. Use it to inspect collision data; build_blockmap recomputes it."
    )]
    fn get_blockmap(
        &self,
        Parameters(args): Parameters<GetBlockmapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        nodes_call(
            self.state(),
            resolve_request(args.map, args.scope.as_deref(), args.detail.as_deref()),
            attach_blockmap,
            blockmap_run,
        )
    }

    /// Rebuilds a map's BLOCKMAP lump from its geometry.
    #[tool(
        description = "Recomputes a map's BLOCKMAP lump from its current geometry as a single undoable edit: the vanilla grid with 128-unit cells covering every vertex, each linedef listed in every block it touches, and only the BLOCKMAP lump changing. Prefer build_all_nodes after geometry changes, which rebuilds the BSP, REJECT and BLOCKMAP together in one undo step."
    )]
    fn build_blockmap(
        &self,
        Parameters(args): Parameters<BuildBlockmapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| blockmap_build_run(session, &args)),
        )
    }

    /// Rebuilds a map's REJECT lump.
    #[tool(
        description = "Recomputes a map's REJECT lump as a single undoable edit: mode zero (default) clears the visibility matrix - always correct - and mode components sets exactly the sector pairs with no two-sided opening between them. Only the REJECT lump changes. Prefer build_all_nodes after geometry changes, which rebuilds the BSP, BLOCKMAP and REJECT together."
    )]
    fn build_reject(
        &self,
        Parameters(args): Parameters<BuildRejectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| reject_build_run(session, &args)),
        )
    }

    /// Rebuilds a map's BSP node tree (vanilla or ZDoom extended lumps).
    #[tool(
        description = "Rebuilds a map's BSP node tree from its current geometry as a single undoable edit: variant classic (the vanilla SEGS/SSECTORS/NODES every port plays), extended (ZDoom's fraction-precise single lump) or gl (classic plus a glBSP v2 GL_* run); split_cost tunes how dearly the builder buys cutting segs. Prefer build_all_nodes after finishing geometry - it rebuilds the REJECT and BLOCKMAP in the same undo step."
    )]
    fn build_nodes(
        &self,
        Parameters(args): Parameters<BuildNodesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| nodes_build_run(session, &args)),
        )
    }

    /// Rebuilds a map's whole node data (tree, reject and blockmap) in one step.
    #[tool(
        description = "Rebuilds a map's complete node data - the BSP tree, the REJECT matrix and the BLOCKMAP grid - in ONE undoable edit: the step to run after geometry changes instead of separate build_nodes, build_reject and build_blockmap calls. variant picks the node layout as build_nodes does and reject_mode zero or components clears or computes visibility. Returns each build's summary."
    )]
    fn build_all_nodes(
        &self,
        Parameters(args): Parameters<BuildAllNodesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| all_nodes_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
