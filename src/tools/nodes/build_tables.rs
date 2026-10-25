//! `build_blockmap` and `build_reject`: the two generated side lumps.
//!
//! [`install_lump`] is the one write step every build of the group shares,
//! the tree builders included: a block's own lump is replaced in place, an
//! absent one lands after the first canonical neighbour the block carries.
//! [`apply_build_blockmap`] and [`apply_build_reject`] are the edit
//! closures on top of it, computing the grid and the matrix from the map's
//! current geometry and writing their own lump only, so no node data is
//! invalidated; [`blockmap_body`] and [`reject_body`] fold what they built
//! into the report rows. [`blockmap_build_run`] and [`reject_build_run`]
//! are the two tools' work, each one [`Session::edit_map`] step.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::wad::archive::Lump;
use crate::wad::lumps;
use crate::wad::map::Map;
use crate::wad::map::build_blockmap::{self, BlockmapBuild};
use crate::wad::map::build_reject::{self, RejectBuild, RejectMode};
use crate::wad::name::LumpName;

use super::get_tables::BlockOrigin;

/// Arguments for [`Cyberdemon::build_blockmap`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildBlockmapParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Share one copy of identical blocklists (default `true`, the smaller
    /// lump; `false` writes every block its own list).
    #[serde(default)]
    pub(super) compress: Option<bool>,
}

/// Arguments for [`Cyberdemon::build_reject`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildRejectParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// `"zero"` (the default: clear the matrix, every sector may see
    /// every other) or `"components"` (reject sector pairs in different
    /// connected components of the sector graph).
    #[serde(default)]
    pub(super) mode: Option<String>,
}

/// The canonical neighbours of the written builder lumps: an absent lump
/// is inserted after the first of these the block carries (the binary
/// writers normalise the member order anyway).
pub(super) const BLOCKMAP_AFTER: [LumpName; 4] =
    [lumps::REJECT, lumps::NODES, lumps::SSECTORS, lumps::SEGS];
/// The canonical neighbours of `REJECT`.
pub(super) const REJECT_AFTER: [LumpName; 3] = [lumps::NODES, lumps::SSECTORS, lumps::SEGS];
/// The canonical neighbours of `SEGS` inside [`Map::other`]: nothing in
/// the side-bag precedes it.
pub(super) const SEGS_AFTER: [LumpName; 0] = [];
/// The canonical neighbour of `SSECTORS`.
pub(super) const SSECTORS_AFTER: [LumpName; 1] = [lumps::SEGS];
/// The canonical neighbours of `NODES`.
pub(super) const NODES_AFTER: [LumpName; 2] = [lumps::SSECTORS, lumps::SEGS];
/// The position of `ZNODES` inside [`Map::other`]: an absent one appends,
/// which the `UDMF` writer still places between `TEXTMAP` and `ENDMAP`.
pub(super) const ZNODES_AFTER: [LumpName; 0] = [];

/// The accepted `mode` text, `zero` being the default.
///
/// # Errors
///
/// [`RejectMode::parse`]'s [`CyberError::InvalidArgument`] for any other
/// name.
pub(super) fn parse_mode(text: Option<&str>) -> Result<RejectMode, CyberError> {
    text.map_or(Ok(RejectMode::Zero), RejectMode::parse)
}

/// Replaces the first lump named `name` in `map.other` with `data`, or
/// inserts it after the last copy of the first canonical neighbour from
/// `after` the block carries (at the end when it carries none).
pub(super) fn install_lump(map: &mut Map, name: LumpName, data: &[u8], after: &[LumpName]) {
    let bytes = data.to_vec();
    if let Some(at) = map.other.iter().position(|lump| lump.name == name) {
        if let Some(slot) = map.other.get_mut(at) {
            *slot = Lump::new(name, bytes);
        }
        return;
    }
    let at = after
        .iter()
        .find_map(|wanted| map.other.iter().rposition(|lump| lump.name == *wanted))
        .and_then(|seen| seen.checked_add(1))
        .unwrap_or(map.other.len());
    map.other.insert(at, Lump::new(name, bytes));
}

/// The `BLOCKMAP` edit step every blockmap build shares: compute the
/// grid from the map's current geometry and write its lump.
pub(super) fn apply_build_blockmap(
    map: &mut Map,
    compress: bool,
) -> Result<BlockmapBuild, CyberError> {
    let built = build_blockmap::build(map, compress)?;
    install_lump(map, lumps::BLOCKMAP, &built.bytes, &BLOCKMAP_AFTER);
    Ok(built)
}

/// The `REJECT` edit step every reject build shares: compute the matrix
/// in `mode` and write its lump.
pub(super) fn apply_build_reject(
    map: &mut Map,
    mode: RejectMode,
) -> Result<RejectBuild, CyberError> {
    let built = build_reject::build(map, mode)?;
    install_lump(map, lumps::REJECT, &built.bytes, &REJECT_AFTER);
    Ok(built)
}

/// What one blockmap build wrote, before the edit fragment joins it.
#[derive(Debug, Serialize)]
pub(super) struct BlockmapBuildBody {
    /// Size of the written `BLOCKMAP` lump in bytes.
    pub(super) bytes: usize,
    /// Blocks per row.
    pub(super) columns: u16,
    /// Rows in the grid.
    pub(super) rows: u16,
    /// `X`/`Y` of block `(0, 0)`'s bottom-left corner.
    pub(super) origin: BlockOrigin,
    /// Whether the lump stays inside the 32767 words vanilla reads.
    pub(super) vanilla_safe: bool,
}

/// Folds a finished blockmap build into its report body.
pub(super) fn blockmap_body(built: &BlockmapBuild) -> BlockmapBuildBody {
    BlockmapBuildBody {
        bytes: built.bytes.len(),
        columns: built.columns,
        rows: built.rows,
        origin: BlockOrigin {
            x: built.origin_x,
            y: built.origin_y,
        },
        vanilla_safe: built.vanilla_safe,
    }
}

/// Structured result of [`Cyberdemon::build_blockmap`].
#[derive(Debug, Serialize)]
pub(super) struct BlockmapBuildReport {
    /// What the build wrote.
    #[serde(flatten)]
    pub(super) body: BlockmapBuildBody,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    pub(super) edit: JsonMap<String, Value>,
}

/// What one reject build wrote, before the edit fragment joins it.
#[derive(Debug, Serialize)]
pub(super) struct RejectBuildBody {
    /// Size of the written `REJECT` lump in bytes.
    pub(super) bytes: usize,
    /// The map's sector count.
    pub(super) sectors: usize,
    /// Rejected pairs in the written matrix.
    pub(super) rejected_pairs: usize,
    /// The mode that was built.
    pub(super) mode: &'static str,
}

/// Folds a finished reject build and the mode it ran in into its body.
pub(super) fn reject_body(built: &RejectBuild, mode: RejectMode) -> RejectBuildBody {
    RejectBuildBody {
        bytes: built.bytes.len(),
        sectors: built.sectors,
        rejected_pairs: built.rejected_pairs,
        mode: mode.as_str(),
    }
}

/// Structured result of [`Cyberdemon::build_reject`].
#[derive(Debug, Serialize)]
pub(super) struct RejectBuildReport {
    /// What the build wrote.
    #[serde(flatten)]
    pub(super) body: RejectBuildBody,
    /// The standard edit fragment.
    #[serde(flatten)]
    pub(super) edit: JsonMap<String, Value>,
}

/// `build_blockmap`'s work: resolve the map and run the one edit step.
pub(super) fn blockmap_build_run(
    session: &mut Session,
    args: &BuildBlockmapParams,
) -> Result<BlockmapBuildReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let compress = args.compress.unwrap_or(true);
    let (built, report) = session.edit_map(name, "build blockmap", |map| {
        apply_build_blockmap(map, compress)
    })?;
    Ok(BlockmapBuildReport {
        body: blockmap_body(&built),
        edit: edit_result_fragment(&report),
    })
}

/// `build_reject`'s work: resolve the mode and the map, run the edit step.
pub(super) fn reject_build_run(
    session: &mut Session,
    args: &BuildRejectParams,
) -> Result<RejectBuildReport, CyberError> {
    let mode = parse_mode(args.mode.as_deref())?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let (built, report) =
        session.edit_map(name, "build reject", |map| apply_build_reject(map, mode))?;
    Ok(RejectBuildReport {
        body: reject_body(&built, mode),
        edit: edit_result_fragment(&report),
    })
}
