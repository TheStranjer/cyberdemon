//! `get_nodes`: the parsed node run of one map, reported and published.
//!
//! [`nodes_run`] reads the block under the session lock and hands back the
//! report with the `full` asset's body held back for [`super::deliver`].
//! [`read_node_data`] sorts a block's node lumps into the three shapes a
//! map can carry ([`NodeData`]): a parsed vanilla or `ZDoom` extended run,
//! a `GL`-family magic that is reported but never parsed, or nothing at
//! all; [`gl_info`] measures a `GL` run attached right after the block,
//! counting whole `glBSP` v2 rows through [`whole_records`] and leaving
//! every other magic uncounted. [`nodes_outcome`] folds the two readings
//! into [`NodesReport`], [`nodes_asset_body`] the parsed records into the
//! published [`NodesAsset`].

use alloc::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common::Directory;
use crate::wad::classify::MapBlock;
use crate::wad::map::nodes::{self, Node, NodesFormat, Seg, Subsector};
use crate::wad::map::{Map, NodeSet};
use crate::wad::name::LumpName;

use super::{
    Detail, JSON_EXTENSION, MAGIC_LEN, Publish, Read, Request, find_lump, json_bytes, lump_bytes,
    open_map,
};

/// Arguments for [`Cyberdemon::get_nodes`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetNodesParams {
    /// The map to read, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    pub(super) scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes the parsed segs, subsectors and nodes as a `JSON` asset.
    #[serde(default)]
    pub(super) detail: Option<String>,
}

/// What a block's node lumps hold.
#[derive(Debug)]
pub(super) enum NodeData {
    /// No node bytes in the block.
    Empty,
    /// A parsed vanilla or extended node run, with the size of the lump
    /// it came from.
    Parsed {
        /// The parsed tree.
        set: NodeSet,
        /// Bytes of the lump that held the data.
        bytes: usize,
    },
    /// A `GL`-family magic: reported, never parsed.
    GlMagic {
        /// The lowercase magic name.
        format: &'static str,
        /// Bytes of the lump that carried the magic.
        bytes: usize,
    },
}

/// Reads the block's node run: the vanilla trio, an extended `NODES`
/// lump, or the `ZNODES` run of a block whose `NODES` is empty.
///
/// # Errors
///
/// [`nodes::parse_vanilla_nodes`] and [`nodes::parse_extended_nodes`]'s
/// [`CyberError::Format`] results, and the same for a `ZNODES` lump that
/// starts without an extended magic.
pub(super) fn read_node_data(map: &Map) -> Result<NodeData, CyberError> {
    let nodes_bytes = lump_bytes(&map.other, b"NODES");
    if !nodes_bytes.is_empty() {
        return Ok(match nodes::lump_format(nodes_bytes) {
            None => NodeData::Parsed {
                set: nodes::parse_vanilla_nodes(
                    lump_bytes(&map.other, b"SEGS"),
                    lump_bytes(&map.other, b"SSECTORS"),
                    nodes_bytes,
                )?,
                bytes: nodes_bytes.len(),
            },
            Some("xnod" | "znod") => NodeData::Parsed {
                set: nodes::parse_extended_nodes(nodes_bytes)?,
                bytes: nodes_bytes.len(),
            },
            Some(other) => NodeData::GlMagic {
                format: other,
                bytes: nodes_bytes.len(),
            },
        });
    }
    read_znodes(map)
}

/// The `ZNODES` node run of a block whose `NODES` lump holds no bytes.
fn read_znodes(map: &Map) -> Result<NodeData, CyberError> {
    let Some(znodes) = find_lump(&map.other, b"ZNODES").filter(|lump| !lump.data.is_empty()) else {
        return Ok(NodeData::Empty);
    };
    match nodes::lump_format(&znodes.data) {
        Some("xnod" | "znod") => Ok(NodeData::Parsed {
            set: nodes::parse_extended_nodes(&znodes.data)?,
            bytes: znodes.size(),
        }),
        Some(other) => Ok(NodeData::GlMagic {
            format: other,
            bytes: znodes.size(),
        }),
        None => Err(CyberError::Format(
            "the ZNODES lump starts without an XNOD or ZNOD magic: ZNODES \
             always holds extended node data; rebuild the nodes"
                .to_owned(),
        )),
    }
}

/// The attached `GL` node block of one map, as reported.
#[derive(Clone, Debug, Serialize)]
pub(super) struct GlInfo {
    /// Whether a `GL_` node run is attached to the block.
    pub(super) present: bool,
    /// The `GL_VERT` lump's magic (`gNd2`, `gNd5`, ...), `None` without a
    /// readable one.
    pub(super) magic: Option<String>,
    /// `GL_SEGS` records; `null` unless the run is a readable `glBSP` v2.
    pub(super) segs: Option<usize>,
    /// `GL_SSECT` records; `null` unless the run is a readable `glBSP` v2.
    pub(super) subsectors: Option<usize>,
    /// `GL_NODES` records; `null` unless the run is a readable `glBSP` v2.
    pub(super) nodes: Option<usize>,
    /// `GL_VERT` vertices; `null` unless the run is a readable `glBSP` v2.
    pub(super) vertices: Option<usize>,
    /// The run's lump sizes by name.
    pub(super) lumps: BTreeMap<String, usize>,
}

/// Bytes in one `GL_SEGS` record.
pub(super) const GL_SEGS_RECORD: usize = 10;
/// Bytes in one `GL_SSECT` record.
const GL_SSECT_RECORD: usize = 4;
/// Bytes in one `GL_NODES` record.
pub(super) const GL_NODES_RECORD: usize = 28;
/// Bytes in one `GL_VERT` row.
const GL_VERT_ROW: usize = 8;
/// Bytes in the `GL_VERT` magic that precede the rows.
const GL_VERT_MAGIC: usize = 4;
/// The `GL_VERT` magic whose records this reader counts.
const GL_MAGIC_V2: &str = "gNd2";

/// The record count of a GL lump holding whole `record`-sized rows only.
pub(super) fn whole_records(bytes: usize, record: usize) -> Option<usize> {
    let count = bytes.checked_div(record)?;
    let used = record.checked_mul(count)?;
    (used == bytes).then_some(count)
}

/// The record counts of a `glBSP` v2 run's lumps.
#[derive(Clone, Copy, Default)]
struct GlCounts {
    /// `GL_SEGS` rows.
    segs: Option<usize>,
    /// `GL_SSECT` rows.
    subsectors: Option<usize>,
    /// `GL_NODES` rows.
    nodes: Option<usize>,
    /// `GL_VERT` rows.
    vertices: Option<usize>,
}

/// The v2 record counts of the run's lumps, all `None` for any other
/// magic (only `gNd2` has a defined layout here).
fn gl_counts(magic: Option<&str>, lumps: &BTreeMap<String, usize>) -> GlCounts {
    if magic != Some(GL_MAGIC_V2) {
        return GlCounts::default();
    }
    let segs = lumps
        .get("GL_SEGS")
        .and_then(|size| whole_records(*size, GL_SEGS_RECORD));
    let subsectors = lumps
        .get("GL_SSECT")
        .and_then(|size| whole_records(*size, GL_SSECT_RECORD));
    let nodes = lumps
        .get("GL_NODES")
        .and_then(|size| whole_records(*size, GL_NODES_RECORD));
    let vertices = lumps
        .get("GL_VERT")
        .and_then(|size| size.checked_sub(GL_VERT_MAGIC))
        .and_then(|body| whole_records(body, GL_VERT_ROW));
    GlCounts {
        segs,
        subsectors,
        nodes,
        vertices,
    }
}

/// Reports `block`'s attached `GL` run: magic, `glBSP` v2 counts and
/// sizes; the record bytes themselves are never parsed here.
pub(super) fn gl_info(dir: &Directory, block: &MapBlock) -> GlInfo {
    let absent = || GlInfo {
        present: false,
        magic: None,
        segs: None,
        subsectors: None,
        nodes: None,
        vertices: None,
        lumps: BTreeMap::new(),
    };
    let Some((start, end)) = block.gl else {
        return absent();
    };
    let mut lumps = BTreeMap::new();
    let mut magic = None;
    for position in start..end {
        let Some(lump) = dir.lumps.get(position) else {
            continue;
        };
        lumps.insert(lump.name.to_string(), lump.size());
        if lump.name.as_bytes() == b"GL_VERT" {
            magic = magic.or_else(|| lump_magic(&lump.data));
        }
    }
    let counts = gl_counts(magic.as_deref(), &lumps);
    GlInfo {
        present: true,
        magic,
        segs: counts.segs,
        subsectors: counts.subsectors,
        nodes: counts.nodes,
        vertices: counts.vertices,
        lumps,
    }
}

/// The four printable-`ASCII` letters a lump starts with, when it does.
pub(super) fn lump_magic(data: &[u8]) -> Option<String> {
    let head = data.get(..MAGIC_LEN)?;
    head.iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| String::from_utf8_lossy(head).into_owned())
}

/// Structured result of [`Cyberdemon::get_nodes`].
#[derive(Debug, Serialize)]
pub(super) struct NodesReport {
    /// The map that was read.
    pub(super) map: LumpName,
    /// `"none"`, `"vanilla"`, `"xnod"`, `"znod"` or a lowercase `GL`
    /// magic (`xgln`, `zgln`, `xgl2`, `zgl2`, `xgl3`, `zgl3`).
    pub(super) format: String,
    /// Size of the lump the format was read from; `0` when absent.
    pub(super) bytes: usize,
    /// Parsed seg count; `null` when nothing parsed.
    pub(super) segs: Option<usize>,
    /// Parsed subsector count; `null` when nothing parsed.
    pub(super) subsectors: Option<usize>,
    /// Parsed node count; `null` when nothing parsed.
    pub(super) nodes: Option<usize>,
    /// Vertices the extended format adds to `VERTEXES`; `null` when
    /// nothing parsed.
    pub(super) extra_vertices: Option<usize>,
    /// The root node's index, the last node; `null` without a tree.
    pub(super) root: Option<usize>,
    /// Node levels on the longest root-to-leaf path; `null` when nothing
    /// parsed.
    pub(super) depth: Option<usize>,
    /// The attached `GL` node run.
    pub(super) gl: GlInfo,
    /// The published full `JSON`; only in `full` detail with a parsed run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) asset: Option<AssetRef>,
}

/// Builds the `get_nodes` report from what the block's lumps held, also
/// handing back the parsed set for the `full` asset when there is one.
pub(super) fn nodes_outcome(
    name: LumpName,
    data: NodeData,
    gl: GlInfo,
) -> (NodesReport, Option<NodeSet>) {
    match data {
        NodeData::Empty => (
            NodesReport {
                map: name,
                format: "none".to_owned(),
                bytes: 0,
                segs: Some(0),
                subsectors: Some(0),
                nodes: Some(0),
                extra_vertices: Some(0),
                root: None,
                depth: Some(0),
                gl,
                asset: None,
            },
            None,
        ),
        NodeData::GlMagic { format, bytes } => (
            NodesReport {
                map: name,
                format: format.to_owned(),
                bytes,
                segs: None,
                subsectors: None,
                nodes: None,
                extra_vertices: None,
                root: None,
                depth: None,
                gl,
                asset: None,
            },
            None,
        ),
        NodeData::Parsed { set, bytes } => {
            let report = NodesReport {
                map: name,
                format: set.format.as_str().to_owned(),
                bytes,
                segs: Some(set.segs.len()),
                subsectors: Some(set.subsectors.len()),
                nodes: Some(set.nodes.len()),
                extra_vertices: Some(set.vertices_extra.len()),
                root: set.root(),
                depth: set.depth(),
                gl,
                asset: None,
            };
            (report, Some(set))
        }
    }
}

/// One `16.16` vertex of a published asset.
#[derive(Serialize)]
struct FixedVertex {
    /// Raw `16.16` fixed-point `X`.
    x: i32,
    /// Raw `16.16` fixed-point `Y`.
    y: i32,
}

/// The `full` asset body of [`Cyberdemon::get_nodes`].
#[derive(Serialize)]
pub(super) struct NodesAsset {
    /// The layout the records came from.
    format: NodesFormat,
    /// The added fraction-precise vertices, raw `16.16` values.
    extra_vertices: Vec<FixedVertex>,
    /// Every parsed seg.
    segs: Vec<Seg>,
    /// Every parsed subsector.
    subsectors: Vec<Subsector>,
    /// Every parsed node, children tagged `{"node": i}` or
    /// `{"subsector": i}`.
    nodes: Vec<Node>,
}

/// Serializes a parsed node set for the `full` asset.
pub(super) fn nodes_asset_body(set: &NodeSet) -> NodesAsset {
    NodesAsset {
        format: set.format,
        extra_vertices: set
            .vertices_extra
            .iter()
            .map(|&(x, y)| FixedVertex { x: x.0, y: y.0 })
            .collect(),
        segs: set.segs.clone(),
        subsectors: set.subsectors.clone(),
        nodes: set.nodes.clone(),
    }
}

/// `get_nodes`' work under the session lock: the report plus the held-back
/// publish job (`full` detail of a parsed run only).
pub(super) fn nodes_run(session: &Session, request: &Request) -> Read<NodesReport> {
    let (name, dir, block, map) = open_map(session, request)?;
    let (report, parsed) = nodes_outcome(name, read_node_data(&map)?, gl_info(&dir, &block));
    let publish = match parsed {
        Some(set) if request.detail == Detail::Full => Some(Publish {
            filename: format!("{name}_nodes.{JSON_EXTENSION}"),
            data: json_bytes(&nodes_asset_body(&set))?,
        }),
        Some(_) | None => None,
    };
    Ok((report, publish))
}

/// Attach a published asset to a nodes report.
pub(super) fn attach_nodes(report: &mut NodesReport, asset: AssetRef) {
    report.asset = Some(asset);
}
