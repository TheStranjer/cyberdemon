//! `get_reject` and `get_blockmap`: the two generated side lumps, measured.
//!
//! [`reject_run`] reads the `REJECT` matrix's size against what the map's
//! sector count asks for and counts the rejected pairs without failing on
//! a truncated lump; [`blockmap_run`] parses the `BLOCKMAP` grid and folds
//! it into [`BlockmapReport`] through [`blockmap_summary`], answering the
//! zeroed [`empty_blockmap_report`] for a block that carries no bytes.
//! Both publish their `full` body as a held-back asset exactly like
//! [`super::get_nodes`] does, and both read the same [`super::Request`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::session::Session;
use crate::wad::lumps;
use crate::wad::map::{Blockmap, Reject};
use crate::wad::name::LumpName;

use super::{Detail, JSON_EXTENSION, Publish, Read, Request, json_bytes, lump_bytes, open_map};

/// Arguments for [`Cyberdemon::get_reject`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetRejectParams {
    /// The map to read; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    pub(super) scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes one bit-string row per sector as a `JSON` asset.
    #[serde(default)]
    pub(super) detail: Option<String>,
}

/// Arguments for [`Cyberdemon::get_blockmap`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetBlockmapParams {
    /// The map to read; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    pub(super) scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes every block's linedef list as a `JSON` asset.
    #[serde(default)]
    pub(super) detail: Option<String>,
}

/// Structured result of [`Cyberdemon::get_reject`].
#[derive(Debug, Serialize)]
pub(super) struct RejectReport {
    /// The map that was read.
    pub(super) map: LumpName,
    /// Size of the `REJECT` lump in bytes; `0` when absent.
    pub(super) bytes: usize,
    /// `ceil(sectors² / 8)`, the size the lump should have.
    pub(super) expected_bytes: usize,
    /// The map's sector count.
    pub(super) sectors: usize,
    /// Whether `bytes` equals [`RejectReport::expected_bytes`].
    pub(super) size_ok: bool,
    /// Whether no pair is rejected inside the logical matrix.
    pub(super) all_zero: bool,
    /// Set bits inside the `sectors²` logical bits.
    pub(super) rejected_pairs: usize,
    /// The published full `JSON`; only in `full` detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) asset: Option<AssetRef>,
}

/// The `full` asset body of [`Cyberdemon::get_reject`]: one bit-string
/// row per sector, first character the row's first (least significant)
/// bit.
#[derive(Serialize)]
struct RejectAsset {
    /// The matrix width and height.
    sectors: usize,
    /// Row `i` as a `'0'`/`'1'` string, `sectors` characters.
    rows: Vec<String>,
}

/// `get_reject`'s work under the session lock.
pub(super) fn reject_run(session: &Session, request: &Request) -> Read<RejectReport> {
    let (name, _dir, _block, map) = open_map(session, request)?;
    let bits = lump_bytes(&map.other, lumps::REJECT);
    let reject = Reject::new(map.sectors.len(), bits);
    let report = RejectReport {
        map: name,
        bytes: bits.len(),
        expected_bytes: Reject::expected_bytes(reject.sectors),
        sectors: reject.sectors,
        size_ok: reject.size_ok(),
        all_zero: reject.all_zero(),
        rejected_pairs: reject.rejected_pairs(),
        asset: None,
    };
    let publish = (request.detail == Detail::Full)
        .then(|| {
            let rows: Vec<String> = (0..reject.sectors).map(|index| reject.row(index)).collect();
            json_bytes(&RejectAsset {
                sectors: reject.sectors,
                rows,
            })
            .map(|data| Publish {
                filename: format!("{name}_reject.{JSON_EXTENSION}"),
                data,
            })
        })
        .transpose()?;
    Ok((report, publish))
}

/// The blockmap origin as a nested object.
#[derive(Debug, Serialize, Default)]
pub(super) struct BlockOrigin {
    /// `X` in map units.
    pub(super) x: i16,
    /// `Y` in map units.
    pub(super) y: i16,
}

/// Structured result of [`Cyberdemon::get_blockmap`].
#[derive(Debug, Serialize)]
pub(super) struct BlockmapReport {
    /// The map that was read.
    pub(super) map: LumpName,
    /// Size of the `BLOCKMAP` lump in bytes; `0` when absent.
    pub(super) bytes: usize,
    /// `X`/`Y` of block `(0, 0)`'s bottom-left corner.
    pub(super) origin: BlockOrigin,
    /// Blocks per row.
    pub(super) columns: u16,
    /// Rows in the grid.
    pub(super) rows: u16,
    /// The parsed blocklists.
    pub(super) blocks: usize,
    /// Linedef references across all blocks.
    pub(super) total_entries: usize,
    /// The largest single block list.
    pub(super) max_lines_per_block: usize,
    /// Blocks holding no linedef.
    pub(super) empty_blocks: usize,
    /// The published full `JSON`; only in `full` detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) asset: Option<AssetRef>,
}

/// Folds one parsed blockmap into its summary report.
pub(super) fn blockmap_summary(name: LumpName, bytes: usize, parsed: &Blockmap) -> BlockmapReport {
    BlockmapReport {
        map: name,
        bytes,
        origin: BlockOrigin {
            x: parsed.origin_x,
            y: parsed.origin_y,
        },
        columns: parsed.columns,
        rows: parsed.rows,
        blocks: parsed.blocks.len(),
        total_entries: parsed.blocks.iter().map(Vec::len).sum(),
        max_lines_per_block: parsed.blocks.iter().map(Vec::len).max().unwrap_or(0),
        empty_blocks: parsed
            .blocks
            .iter()
            .filter(|lines| lines.is_empty())
            .count(),
        asset: None,
    }
}

/// The zeroed report an absent or empty `BLOCKMAP` gets.
pub(super) fn empty_blockmap_report(name: LumpName) -> BlockmapReport {
    BlockmapReport {
        map: name,
        bytes: 0,
        origin: BlockOrigin::default(),
        columns: 0,
        rows: 0,
        blocks: 0,
        total_entries: 0,
        max_lines_per_block: 0,
        empty_blocks: 0,
        asset: None,
    }
}

/// `get_blockmap`'s work under the session lock.
pub(super) fn blockmap_run(session: &Session, request: &Request) -> Read<BlockmapReport> {
    let (name, _dir, _block, map) = open_map(session, request)?;
    let data = lump_bytes(&map.other, lumps::BLOCKMAP);
    if data.is_empty() {
        return Ok((empty_blockmap_report(name), None));
    }
    let parsed = Blockmap::parse(data)?;
    let report = blockmap_summary(name, data.len(), &parsed);
    let publish = (request.detail == Detail::Full)
        .then(|| {
            json_bytes(&parsed).map(|body| Publish {
                filename: format!("{name}_blockmap.{JSON_EXTENSION}"),
                data: body,
            })
        })
        .transpose()?;
    Ok((report, publish))
}

/// Attach a published asset to a reject report.
pub(super) fn attach_reject(report: &mut RejectReport, asset: AssetRef) {
    report.asset = Some(asset);
}

/// Attach a published asset to a blockmap report.
pub(super) fn attach_blockmap(report: &mut BlockmapReport, asset: AssetRef) {
    report.asset = Some(asset);
}
