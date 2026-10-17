//! Nodes tools: [`Cyberdemon::get_nodes`], [`Cyberdemon::get_reject`] and
//! [`Cyberdemon::get_blockmap`], the inspection side of the node-builder
//! output lumps parsed by [`crate::wad::map::nodes`],
//! [`crate::wad::map::reject`] and [`crate::wad::map::blockmap`], plus the
//! builders [`Cyberdemon::build_blockmap`] and
//! [`Cyberdemon::build_reject`], which recompute the `BLOCKMAP` and
//! `REJECT` lumps from the map's current geometry as single undoable
//! edits (see [`crate::wad::map::build_blockmap`] and
//! [`crate::wad::map::build_reject`]).
//!
//! All three readers take the usual `map` and `scope` parameters and a
//! `detail` knob: `summary` (the default) reports measurable numbers only,
//! `full` additionally publishes the parsed structure as a `JSON` asset,
//! held back until the session lock is released. Missing or empty lumps
//! are never errors, they report as absent data; malformed lumps fail with
//! `format_error`. The two builders take the usual `map` parameter plus
//! their judgment knob (`compress` / `mode`) and rewrite only their own
//! lump in the block, so no node data is invalidated.

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::maps::common::{self, Directory};
use crate::tools::maps::params::edit_result_fragment;
use crate::tools::{fail, ok};
use crate::wad::archive::Lump;
use crate::wad::classify::MapBlock;
use crate::wad::map::build_blockmap::{self, BlockmapBuild};
use crate::wad::map::build_reject::{self, RejectBuild, RejectMode};
use crate::wad::map::nodes::{self, Node, NodesFormat, Seg, Subsector};
use crate::wad::map::{Blockmap, Map, NodeSet, Reject, read_map};
use crate::wad::name::LumpName;

/// File extension of the published `JSON` assets.
const JSON_EXTENSION: &str = "json";
/// Bytes in a `GL` lump magic.
const MAGIC_LEN: usize = 4;

/// Arguments for [`Cyberdemon::get_nodes`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetNodesParams {
    /// The map to read, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes the parsed segs, subsectors and nodes as a `JSON` asset.
    #[serde(default)]
    detail: Option<String>,
}

/// Arguments for [`Cyberdemon::get_reject`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetRejectParams {
    /// The map to read; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes one bit-string row per sector as a `JSON` asset.
    #[serde(default)]
    detail: Option<String>,
}

/// Arguments for [`Cyberdemon::get_blockmap`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetBlockmapParams {
    /// The map to read; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// `"summary"` (the default) reports counts only; `"full"` additionally
    /// publishes every block's linedef list as a `JSON` asset.
    #[serde(default)]
    detail: Option<String>,
}

/// Arguments for [`Cyberdemon::build_blockmap`].
#[derive(Debug, Deserialize, JsonSchema)]
struct BuildBlockmapParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// Share one copy of identical blocklists (default `true`, the smaller
    /// lump; `false` writes every block its own list).
    #[serde(default)]
    compress: Option<bool>,
}

/// Arguments for [`Cyberdemon::build_reject`].
#[derive(Debug, Deserialize, JsonSchema)]
struct BuildRejectParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// `"zero"` (the default: clear the matrix, every sector may see
    /// every other) or `"components"` (reject sector pairs in different
    /// connected components of the sector graph).
    #[serde(default)]
    mode: Option<String>,
}

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

/// The accepted `scope` text, the merged view being the default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
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
/// [`parse_scope`]'s and [`parse_detail`]'s refusals.
fn resolve_request(
    map: Option<String>,
    scope: Option<&str>,
    detail: Option<&str>,
) -> Result<Request, CyberError> {
    Ok(Request {
        map,
        scope: parse_scope(scope)?,
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
fn find_lump<'lumps>(lumps: &'lumps [Lump], wanted: &[u8]) -> Option<&'lumps Lump> {
    lumps
        .iter()
        .rev()
        .find(|lump| lump.name.as_bytes() == wanted)
}

/// The payload of the last lump named `wanted`; empty when absent.
fn lump_bytes<'lumps>(lumps: &'lumps [Lump], wanted: &[u8]) -> &'lumps [u8] {
    find_lump(lumps, wanted).map_or(b"", |lump| lump.data.as_ref())
}

/// What a block's node lumps hold.
#[derive(Debug)]
enum NodeData {
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
fn read_node_data(map: &Map) -> Result<NodeData, CyberError> {
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
struct GlInfo {
    /// Whether a `GL_` node run is attached to the block.
    present: bool,
    /// The `GL_VERT` lump's magic (`gNd2`, `gNd5`, ...), `None` without a
    /// readable one.
    magic: Option<String>,
    /// The run's lump sizes by name.
    lumps: BTreeMap<String, usize>,
}

/// Reports `block`'s attached `GL` run: magic and sizes only, glBSP data
/// is never parsed here.
fn gl_info(dir: &Directory, block: &MapBlock) -> GlInfo {
    let Some((start, end)) = block.gl else {
        return GlInfo {
            present: false,
            magic: None,
            lumps: BTreeMap::new(),
        };
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
    GlInfo {
        present: true,
        magic,
        lumps,
    }
}

/// The four printable-`ASCII` letters a lump starts with, when it does.
fn lump_magic(data: &[u8]) -> Option<String> {
    let head = data.get(..MAGIC_LEN)?;
    head.iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| String::from_utf8_lossy(head).into_owned())
}

/// Structured result of [`Cyberdemon::get_nodes`].
#[derive(Debug, Serialize)]
struct NodesReport {
    /// The map that was read.
    map: LumpName,
    /// `"none"`, `"vanilla"`, `"xnod"`, `"znod"` or a lowercase `GL`
    /// magic (`xgln`, `zgln`, `xgl2`, `zgl2`, `xgl3`, `zgl3`).
    format: String,
    /// Size of the lump the format was read from; `0` when absent.
    bytes: usize,
    /// Parsed seg count; `null` when nothing parsed.
    segs: Option<usize>,
    /// Parsed subsector count; `null` when nothing parsed.
    subsectors: Option<usize>,
    /// Parsed node count; `null` when nothing parsed.
    nodes: Option<usize>,
    /// Vertices the extended format adds to `VERTEXES`; `null` when
    /// nothing parsed.
    extra_vertices: Option<usize>,
    /// The root node's index, the last node; `null` without a tree.
    root: Option<usize>,
    /// Node levels on the longest root-to-leaf path; `null` when nothing
    /// parsed.
    depth: Option<usize>,
    /// The attached `GL` node run.
    gl: GlInfo,
    /// The published full `JSON`; only in `full` detail with a parsed run.
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<AssetRef>,
}

/// Builds the `get_nodes` report from what the block's lumps held, also
/// handing back the parsed set for the `full` asset when there is one.
fn nodes_outcome(name: LumpName, data: NodeData, gl: GlInfo) -> (NodesReport, Option<NodeSet>) {
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
struct NodesAsset {
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
fn nodes_asset_body(set: &NodeSet) -> NodesAsset {
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

/// Serializes any asset body to pretty `JSON`.
///
/// # Errors
///
/// [`CyberError::Internal`] when serialization fails (a server bug).
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CyberError> {
    serde_json::to_vec_pretty(value)
        .map_err(|error| CyberError::Internal(format!("the asset does not serialize: {error}")))
}

/// A `JSON` body held back until the session lock is released.
#[derive(Debug)]
struct Publish {
    /// File name to publish under.
    filename: String,
    /// The serialized body.
    data: Vec<u8>,
}

/// `get_nodes`' work under the session lock: the report plus the held-back
/// publish job (`full` detail of a parsed run only).
fn nodes_run(session: &Session, request: &Request) -> Read<NodesReport> {
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

/// Structured result of [`Cyberdemon::get_reject`].
#[derive(Debug, Serialize)]
struct RejectReport {
    /// The map that was read.
    map: LumpName,
    /// Size of the `REJECT` lump in bytes; `0` when absent.
    bytes: usize,
    /// `ceil(sectors² / 8)`, the size the lump should have.
    expected_bytes: usize,
    /// The map's sector count.
    sectors: usize,
    /// Whether `bytes` equals [`RejectReport::expected_bytes`].
    size_ok: bool,
    /// Whether no pair is rejected inside the logical matrix.
    all_zero: bool,
    /// Set bits inside the `sectors²` logical bits.
    rejected_pairs: usize,
    /// The published full `JSON`; only in `full` detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<AssetRef>,
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
fn reject_run(session: &Session, request: &Request) -> Read<RejectReport> {
    let (name, _dir, _block, map) = open_map(session, request)?;
    let bits = lump_bytes(&map.other, b"REJECT");
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
struct BlockOrigin {
    /// `X` in map units.
    x: i16,
    /// `Y` in map units.
    y: i16,
}

/// Structured result of [`Cyberdemon::get_blockmap`].
#[derive(Debug, Serialize)]
struct BlockmapReport {
    /// The map that was read.
    map: LumpName,
    /// Size of the `BLOCKMAP` lump in bytes; `0` when absent.
    bytes: usize,
    /// `X`/`Y` of block `(0, 0)`'s bottom-left corner.
    origin: BlockOrigin,
    /// Blocks per row.
    columns: u16,
    /// Rows in the grid.
    rows: u16,
    /// The parsed blocklists.
    blocks: usize,
    /// Linedef references across all blocks.
    total_entries: usize,
    /// The largest single block list.
    max_lines_per_block: usize,
    /// Blocks holding no linedef.
    empty_blocks: usize,
    /// The published full `JSON`; only in `full` detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<AssetRef>,
}

/// Folds one parsed blockmap into its summary report.
fn blockmap_summary(name: LumpName, bytes: usize, parsed: &Blockmap) -> BlockmapReport {
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
fn empty_blockmap_report(name: LumpName) -> BlockmapReport {
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
fn blockmap_run(session: &Session, request: &Request) -> Read<BlockmapReport> {
    let (name, _dir, _block, map) = open_map(session, request)?;
    let data = lump_bytes(&map.other, b"BLOCKMAP");
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

/// Attach a published asset to a nodes report.
fn attach_nodes(report: &mut NodesReport, asset: AssetRef) {
    report.asset = Some(asset);
}

/// Attach a published asset to a reject report.
fn attach_reject(report: &mut RejectReport, asset: AssetRef) {
    report.asset = Some(asset);
}

/// Attach a published asset to a blockmap report.
fn attach_blockmap(report: &mut BlockmapReport, asset: AssetRef) {
    report.asset = Some(asset);
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
    let outcome = request.and_then(|resolved| {
        let (report, publish) = state.with_session(|session| work(session, &resolved))?;
        deliver(state, report, publish, attach)
    });
    match outcome {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The canonical neighbours of the written builder lumps: an absent lump
/// is inserted after the first of these the block carries (the binary
/// writers normalise the member order anyway).
const BLOCKMAP_AFTER: [&[u8]; 4] = [b"REJECT", b"NODES", b"SSECTORS", b"SEGS"];
/// The canonical neighbours of `REJECT`.
const REJECT_AFTER: [&[u8]; 3] = [b"NODES", b"SSECTORS", b"SEGS"];

/// The accepted `mode` text, `zero` being the default.
///
/// # Errors
///
/// [`RejectMode::parse`]'s [`CyberError::InvalidArgument`] for any other
/// name.
fn parse_mode(text: Option<&str>) -> Result<RejectMode, CyberError> {
    text.map_or(Ok(RejectMode::Zero), RejectMode::parse)
}

/// Replaces the first lump named `name` in `map.other` with `data`, or
/// inserts it after the last copy of the first canonical neighbour from
/// `after` the block carries (at the end when it carries none).
fn install_lump(map: &mut Map, name: LumpName, data: &[u8], after: &[&[u8]]) {
    let bytes = data.to_vec();
    if let Some(at) = map.other.iter().position(|lump| lump.name == name) {
        if let Some(slot) = map.other.get_mut(at) {
            *slot = Lump::new(name, bytes);
        }
        return;
    }
    let at = after
        .iter()
        .find_map(|wanted| {
            map.other
                .iter()
                .rposition(|lump| lump.name.as_bytes() == *wanted)
        })
        .and_then(|seen| seen.checked_add(1))
        .unwrap_or(map.other.len());
    map.other.insert(at, Lump::new(name, bytes));
}

/// `build_blockmap`'s edit step: compute the grid and write its lump.
fn install_blockmap(map: &mut Map, compress: bool) -> Result<BlockmapBuild, CyberError> {
    let built = build_blockmap::build(map, compress)?;
    install_lump(
        map,
        LumpName::from_raw(*b"BLOCKMAP"),
        &built.bytes,
        &BLOCKMAP_AFTER,
    );
    Ok(built)
}

/// `build_reject`'s edit step: compute the matrix and write its lump.
fn install_reject(map: &mut Map, mode: RejectMode) -> Result<RejectBuild, CyberError> {
    let built = build_reject::build(map, mode)?;
    install_lump(
        map,
        LumpName::from_raw(*b"REJECT\0\0"),
        &built.bytes,
        &REJECT_AFTER,
    );
    Ok(built)
}

/// Structured result of [`Cyberdemon::build_blockmap`].
#[derive(Debug, Serialize)]
struct BlockmapBuildReport {
    /// Size of the written `BLOCKMAP` lump in bytes.
    bytes: usize,
    /// Blocks per row.
    columns: u16,
    /// Rows in the grid.
    rows: u16,
    /// `X`/`Y` of block `(0, 0)`'s bottom-left corner.
    origin: BlockOrigin,
    /// Whether the lump stays inside the 32767 words vanilla reads.
    vanilla_safe: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::build_reject`].
#[derive(Debug, Serialize)]
struct RejectBuildReport {
    /// Size of the written `REJECT` lump in bytes.
    bytes: usize,
    /// The map's sector count.
    sectors: usize,
    /// Rejected pairs in the written matrix.
    rejected_pairs: usize,
    /// The mode that was built.
    mode: &'static str,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// `build_blockmap`'s work: resolve the map and run the one edit step.
fn blockmap_build_run(
    session: &mut Session,
    args: &BuildBlockmapParams,
) -> Result<BlockmapBuildReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let compress = args.compress.unwrap_or(true);
    let (built, report) = session.edit_map(name, "build blockmap", |map| {
        install_blockmap(map, compress)
    })?;
    Ok(BlockmapBuildReport {
        bytes: built.bytes.len(),
        columns: built.columns,
        rows: built.rows,
        origin: BlockOrigin {
            x: built.origin_x,
            y: built.origin_y,
        },
        vanilla_safe: built.vanilla_safe,
        edit: edit_result_fragment(&report),
    })
}

/// `build_reject`'s work: resolve the mode and the map, run the edit step.
fn reject_build_run(
    session: &mut Session,
    args: &BuildRejectParams,
) -> Result<RejectBuildReport, CyberError> {
    let mode = parse_mode(args.mode.as_deref())?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let (built, report) =
        session.edit_map(name, "build reject", |map| install_reject(map, mode))?;
    Ok(RejectBuildReport {
        bytes: built.bytes.len(),
        sectors: built.sectors,
        rejected_pairs: built.rejected_pairs,
        mode: mode.as_str(),
        edit: edit_result_fragment(&report),
    })
}

/// Hand a builder's outcome to the client as the documented result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = nodes_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports the node-builder output of one map.
    #[tool(
        description = "Reads a map's node-builder output: the BSP tree built from its geometry. map defaults to the active map (set_active_map, else the merged view's only map); scope picks the archive (\"merged\" default, \"working\", \"layer:N\"). Returns {map, format, bytes (size of the lump the format was read from), segs, subsectors, nodes, extra_vertices (fraction-precise vertices an extended run adds to VERTEXES), root (index of the root node, the last one), depth (node levels on the longest root-to-leaf path), gl {present, magic (GL_VERT's four-byte magic, e.g. \"gNd2\"), lumps {name: size}}}. format: \"none\" (no node bytes; counts 0), \"vanilla\" (the SEGS/SSECTORS/NODES record lumps), \"xnod\"/\"znod\" (ZDoom extended data in NODES or ZNODES; znod is zlib-compressed after the magic), or a GL magic (\"xgln\", \"zgln\", \"xgl2\", \"zgl2\", \"xgl3\", \"zgl3\"): GL data and GL magics inside NODES are reported with their size and never parsed, so their counts are null. Missing or empty lumps are not errors. detail \"full\" additionally publishes {format, extra_vertices, segs, subsectors, nodes} as a JSON asset named <MAP>_nodes.json and attaches \"asset\" {url, filename, mime, size}; nothing-parsed publishes nothing. A malformed node lump is format_error."
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
        description = "Reads a map's REJECT lump: the sectors-by-sectors visibility matrix where bit i*sectors+j set means sector j is never visible from sector i (row-major, least significant bit first). map and scope work as in get_nodes. Returns {map, bytes, expected_bytes (ceil(sectors^2/8)), sectors, size_ok (bytes equal expected_bytes), all_zero, rejected_pairs (set bits inside the logical matrix)}; a missing or empty lump reports bytes 0 and is not an error, and a wrong size is surfaced through size_ok, not a failure. detail \"full\" additionally publishes {sectors, rows} - one '0'/'1' string per sector, first character the row's first bit - as a JSON asset named <MAP>_reject.json with \"asset\" {url, filename, mime, size} attached."
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
        description = "Reads a map's BLOCKMAP lump: the 128x128-map-unit grid whose per-block lists hold the linedef indices touching each block. map and scope work as in get_nodes. Returns {map, bytes, origin {x, y} (block (0,0)'s bottom-left corner), columns, rows, blocks (the parsed list count), total_entries (linedef references across all blocks), max_lines_per_block, empty_blocks}; a missing or empty lump reports all zeros and is not an error. An offset leaving the lump or a blocklist without its 0xFFFF terminator is format_error. detail \"full\" additionally publishes the whole parsed {origin_x, origin_y, columns, rows, blocks} (blocks in row-major order, each the block's linedef indices) as a JSON asset named <MAP>_blockmap.json with \"asset\" {url, filename, mime, size} attached."
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
        description = "Recomputes a map's BLOCKMAP lump from its current geometry, as a single undoable edit. map defaults to the active map (set_active_map, else the merged view's only map). The grid is the vanilla one: origins 8 map units below the lowest vertex, 128x128 map-unit cells covering every vertex (columns = (max_x-origin_x)/128+1, rows likewise). Every linedef is listed in each block whose closed square it touches - it is skipped only when all four block corners lie strictly on the same side of its infinite line - so a line exactly on a shared block edge is listed in both neighbours and one crossing a block corner lands in every block meeting there; untouched blocks get empty lists. compress (default true) shares one copy of identical blocklists, shrinking the lump. Only the BLOCKMAP lump changes (replaced, or inserted after REJECT when the block has none); the BSP lumps are left alone, so nodes_invalidated is false however fresh the nodes are. Returns {bytes (the written lump's size), columns, rows, origin {x, y}, vanilla_safe (the lump is at most 32767 words, what vanilla Doom reads with signed offsets), map, nodes_invalidated, copied_to_working}. A map with no vertices is invalid_argument; a grid, linedef index or lump past the BLOCKMAP format's shape is limit_exceeded (blockmap too large). Linedefs whose vertex references do not resolve are listed in no block. Unknown map is not_found. Run get_blockmap to inspect the result; see skill/reference/nodes.md."
    )]
    fn build_blockmap(
        &self,
        Parameters(args): Parameters<BuildBlockmapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| blockmap_build_run(session, &args)),
        )
    }

    /// Rebuilds a map's REJECT lump.
    #[tool(
        description = "Recomputes a map's REJECT lump as a single undoable edit. map defaults to the active map (set_active_map, else the merged view's only map). mode \"zero\" (the default) clears the matrix: every sector may see every other, which is always correct and what ZDBSP writes by default. mode \"components\" builds the sector graph (an edge joins the two different sectors on the sides of every two-sided linedef; lines whose sides or sectors do not resolve join nothing), finds its connected components, and sets the bit for exactly the ordered pairs in different components: sectors with no opening between them can never see each other, whatever the heights; same-component pairs stay clear. The lump is ceil(sectors^2/8) bytes, bit i*sectors+j (row-major, least significant bit first); a map with no sectors writes an empty lump. Only the REJECT lump changes (replaced, or inserted after NODES when the block has none); the BSP lumps are left alone (nodes_invalidated false). Returns {bytes, sectors, rejected_pairs (set bits in the written matrix), mode, map, nodes_invalidated, copied_to_working}. An unknown mode is invalid_argument; unknown map is not_found. Run get_reject to inspect the result; see skill/reference/nodes.md."
    )]
    fn build_reject(
        &self,
        Parameters(args): Parameters<BuildRejectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| reject_build_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        BuildBlockmapParams, BuildRejectParams, Detail, GlInfo, NodeData, RejectMode, Request,
        blockmap_build_run, blockmap_summary, empty_blockmap_report, gl_info, install_lump,
        json_bytes, lump_magic, nodes_asset_body, nodes_outcome, parse_detail, parse_mode,
        parse_scope, read_node_data, reject_build_run,
    };
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::bytes::Writer;
    use crate::wad::classify::analyze;
    use crate::wad::map::Blockmap;
    use crate::wad::map::Map;
    use crate::wad::name::LumpName;
    use serde_json::json;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` holding `payload`.
    fn lump(text: &str, payload: &[u8]) -> Lump {
        Lump::new(name(text), payload.to_vec())
    }

    /// One 12-byte vanilla seg record.
    fn seg_bytes() -> Vec<u8> {
        let mut w = Writer::new();
        w.put_u16(0);
        w.put_u16(1);
        w.put_i16(0);
        w.put_u16(0);
        w.put_i16(0);
        w.put_i16(0);
        w.into_inner()
    }

    /// One 28-byte vanilla node record with both children on subsector 0.
    fn node_bytes() -> Vec<u8> {
        let mut w = Writer::new();
        w.put_i16(0);
        w.put_i16(0);
        w.put_i16(1);
        w.put_i16(0);
        for _field in 0_usize..8_usize {
            w.put_i16(0);
        }
        w.put_u16(0x8000);
        w.put_u16(0x8000);
        w.into_inner()
    }

    /// The smallest complete vanilla run: one seg, one subsector, one node.
    fn vanilla_map() -> Map {
        Map {
            other: vec![
                lump("SEGS", &seg_bytes()),
                lump("SSECTORS", &[1, 0, 0, 0]),
                lump("NODES", &node_bytes()),
            ],
            ..Map::default()
        }
    }

    /// An `XNOD` extended run: no added vertices, one subsector of one
    /// seg, one node with subsector children.
    fn xnod_bytes() -> Vec<u8> {
        let mut w = Writer::new();
        w.put_bytes(b"XNOD");
        w.put_u32(2);
        w.put_u32(0);
        w.put_u32(1);
        w.put_u32(1);
        w.put_u32(1);
        w.put_u32(0);
        w.put_u32(1);
        w.put_u16(0);
        w.put_u8(1);
        w.put_u32(1);
        for _field in 0_u32..4_u32 {
            w.put_i16(0);
        }
        for _field in 0_usize..8_usize {
            w.put_i16(0);
        }
        w.put_u32(0x8000_0000);
        w.put_u32(0x8000_0000);
        w.into_inner()
    }

    #[test]
    fn parse_scope_defaults_to_the_merged_view() {
        assert_eq!(parse_scope(None).expect("absent"), Scope::Merged);
        assert_eq!(
            parse_scope(Some("WORKING")).expect("working"),
            Scope::Working
        );
        let error = parse_scope(Some("nope")).expect_err("not a scope");
        assert_eq!(error.code(), "invalid_argument", "bad scope code");
    }

    #[test]
    fn parse_detail_accepts_the_two_names_and_defaults_to_summary() {
        assert_eq!(parse_detail(None).expect("absent"), Detail::Summary);
        assert_eq!(parse_detail(Some("Full")).expect("full"), Detail::Full);
        assert_eq!(
            parse_detail(Some("SUMMARY")).expect("summary"),
            Detail::Summary
        );
        let error = parse_detail(Some("everything")).expect_err("not a detail level");
        assert_eq!(error.code(), "invalid_argument", "bad detail code");
        let message = error.to_string();
        assert!(
            message.contains("summary") && message.contains("full"),
            "the message names both values: {message}"
        );
    }

    #[test]
    fn lump_magic_needs_four_printable_letters() {
        assert_eq!(
            lump_magic(b"gNd2 rest").as_deref(),
            Some("gNd2"),
            "a glBSP magic"
        );
        assert_eq!(lump_magic(b"..!").as_deref(), None, "too short");
        assert_eq!(
            lump_magic(b"\x01\x02\x03\x04").as_deref(),
            None,
            "not printable"
        );
    }

    #[test]
    fn read_node_data_sorts_the_block_into_its_three_shapes() {
        let empty = Map::default();
        assert!(matches!(
            read_node_data(&empty).expect("no lumps"),
            NodeData::Empty
        ));
        let vanilla = read_node_data(&vanilla_map()).expect("the vanilla fixture parses");
        assert!(matches!(vanilla, NodeData::Parsed { .. }), "records parse");
        let xnodes = Map {
            other: vec![lump("NODES", &xnod_bytes())],
            ..Map::default()
        };
        let extended = read_node_data(&xnodes).expect("the XNOD fixture parses");
        assert!(
            matches!(extended, NodeData::Parsed { .. }),
            "extended parses"
        );
        let gl = Map {
            other: vec![lump("NODES", b"XGLN extended gl nodes")],
            ..Map::default()
        };
        let magic = read_node_data(&gl).expect("a GL magic is reported, not parsed");
        match magic {
            NodeData::GlMagic { format, bytes } => {
                assert_eq!(format, "xgln", "the lowercase magic");
                assert_eq!(bytes, 22, "the lump size travels with it");
            }
            NodeData::Empty | NodeData::Parsed { .. } => panic!("expected a GL magic"),
        }
    }

    #[test]
    fn read_node_data_falls_back_to_znodes_and_refuses_magicless_ones() {
        let znodes = Map {
            other: vec![marker("NODES"), lump("ZNODES", &xnod_bytes())],
            ..Map::default()
        };
        assert!(
            matches!(
                read_node_data(&znodes).expect("the ZNODES parses"),
                NodeData::Parsed { .. }
            ),
            "an empty NODES defers to ZNODES",
        );
        let junk = Map {
            other: vec![lump("ZNODES", b"no magic here")],
            ..Map::default()
        };
        let error = read_node_data(&junk).expect_err("ZNODES needs a magic");
        assert_eq!(error.code(), "format_error", "magicless ZNODES code");
        assert!(
            error.to_string().contains("ZNODES"),
            "the lump is named: {error}"
        );
    }

    #[test]
    fn reports_answer_absent_gl_and_parsed_shapes() {
        let gl = GlInfo {
            present: false,
            magic: None,
            lumps: alloc::collections::BTreeMap::new(),
        };
        let (none, none_set) = nodes_outcome(name("MAP01"), NodeData::Empty, gl.clone());
        assert!(none_set.is_none(), "absent data has nothing to publish");
        assert_eq!(none.format, "none", "absent data reports none");
        assert_eq!(none.bytes, 0, "and zero bytes");
        assert_eq!(none.segs, Some(0), "with zero counts");
        assert_eq!(none.root, None, "no root to name");
        let magic = NodeData::GlMagic {
            format: "zgl3",
            bytes: 40,
        };
        let (gl_report, gl_set) = nodes_outcome(name("MAP01"), magic, gl.clone());
        assert!(gl_set.is_none(), "an unparsed magic has no set");
        assert_eq!(gl_report.format, "zgl3", "the magic travels");
        assert_eq!(gl_report.bytes, 40, "with its size");
        assert_eq!(gl_report.nodes, None, "unparsed counts are null");
        assert_eq!(gl_report.depth, None, "and no depth");
        let parsed = read_node_data(&vanilla_map()).expect("parses");
        let (parsed_report, parsed_set) = nodes_outcome(name("MAP01"), parsed, gl);
        assert!(parsed_set.is_some(), "the parsed set travels back");
        assert_eq!(parsed_report.format, "vanilla", "the parsed layout");
        assert_eq!(parsed_report.segs, Some(1), "one seg");
        assert_eq!(parsed_report.subsectors, Some(1), "one subsector");
        assert_eq!(parsed_report.nodes, Some(1), "one node");
        assert_eq!(
            parsed_report.extra_vertices,
            Some(0),
            "vanilla adds no vertices"
        );
        assert_eq!(parsed_report.root, Some(0), "the only node is the root");
        assert_eq!(parsed_report.depth, Some(1), "one level deep");
    }

    #[test]
    fn the_full_nodes_asset_carries_every_parsed_record() {
        let parsed = read_node_data(&vanilla_map()).expect("parses");
        let NodeData::Parsed { set, .. } = parsed else {
            panic!("the vanilla fixture parses");
        };
        let json = serde_json::to_value(nodes_asset_body(&set)).expect("the asset serializes");
        assert_eq!(json["format"], serde_json::json!("vanilla"), "the layout");
        assert_eq!(
            json["segs"].as_array().expect("segs").len(),
            1,
            "one seg row"
        );
        assert_eq!(
            json["nodes"][0]["left_child"],
            serde_json::json!({ "subsector": 0_u64 }),
            "tagged children",
        );
        assert_eq!(json["extra_vertices"], serde_json::json!([]), "none here");
    }

    #[test]
    fn gl_info_reports_the_attached_run_and_its_magic() {
        let lumps = vec![
            marker("MAP01"),
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            marker("SECTORS"),
            marker("NODES"),
            marker("GL_MAP01"),
            lump("GL_VERT", b"gNd5 payload"),
            lump("GL_NODES", b"gl nodes"),
        ];
        let block = analyze(&lumps).maps.first().expect("a block").clone();
        let dir = super::Directory {
            entries: Vec::new(),
            analysis: analyze(&lumps),
            lumps,
        };
        let info = gl_info(&dir, &block);
        assert!(info.present, "a GL run is attached");
        assert_eq!(info.magic.as_deref(), Some("gNd5"), "the GL_VERT magic");
        assert_eq!(info.lumps.len(), 3, "header and two members");
        assert_eq!(info.lumps.get("GL_VERT").copied(), Some(12), "GL_VERT size");
        let block_without_gl = super::MapBlock { gl: None, ..block };
        assert!(
            !gl_info(&dir, &block_without_gl).present,
            "no run, no report"
        );
    }

    /// Two empty `SECTORS` records for a decodable fixture block.
    fn two_sectors() -> Vec<u8> {
        let mut sectors = Writer::new();
        for _sector in 0_u32..2_u32 {
            sectors.put_i16(0);
            sectors.put_i16(128);
            sectors.put_bytes(b"FLOOR4\0\0");
            sectors.put_bytes(b"FLOOR4\0\0");
            sectors.put_i16(160);
            sectors.put_u16(0);
            sectors.put_u16(0);
        }
        sectors.into_inner()
    }

    /// A merged-scope request for `map` at `detail`.
    fn request(map: Option<&str>, scope: Scope, detail: Detail) -> Request {
        Request {
            map: map.map(str::to_owned),
            scope,
            detail,
        }
    }

    /// A decodable `MAP01` block carrying `other` as its node lumps.
    fn block_session(other: Vec<Lump>) -> Session {
        let mut lumps = vec![
            marker("MAP01"),
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            lump("SECTORS", &two_sectors()),
        ];
        lumps.extend(other);
        session_over(lumps)
    }

    #[test]
    fn nodes_run_reports_a_decoded_block_and_holds_back_the_asset() {
        let session = block_session(vec![
            lump("SEGS", &seg_bytes()),
            lump("SSECTORS", &[1, 0, 0, 0]),
            lump("NODES", &node_bytes()),
        ]);
        let (report, publish) =
            super::nodes_run(&session, &request(None, Scope::Merged, Detail::Summary))
                .expect("runs");
        assert_eq!(report.map, name("MAP01"), "the map travels");
        assert_eq!(report.format, "vanilla", "the parsed layout");
        assert_eq!(report.segs, Some(1), "one seg");
        assert!(publish.is_none(), "summary publishes nothing");
        let (full, held) = super::nodes_run(
            &session,
            &request(Some("map01"), Scope::Merged, Detail::Full),
        )
        .expect("full detail runs");
        assert!(full.asset.is_none(), "the slot fills only after delivery");
        let job = held.expect("full detail holds a body back");
        assert_eq!(job.filename, "MAP01_nodes.json", "the asset file name");
        assert!(job.data.starts_with(b"{"), "a JSON body");
    }

    #[test]
    fn nodes_run_refuses_an_undecodable_block() {
        let session = block_session(vec![lump("NODES", b"123")]);
        let broken = super::nodes_run(&session, &request(None, Scope::Merged, Detail::Summary))
            .expect_err("3 bytes are no NODES record");
        assert_eq!(broken.code(), "format_error", "malformed nodes code");
        let missing = super::nodes_run(
            &session,
            &request(Some("MAP09"), Scope::Merged, Detail::Summary),
        )
        .expect_err("MAP09 is in no archive");
        assert_eq!(missing.code(), "not_found", "unknown map code");
        let bad_scope =
            super::nodes_run(&session, &request(None, Scope::Layer(4), Detail::Summary))
                .expect_err("layer 4 is not loaded");
        assert_eq!(bad_scope.code(), "not_found", "unloaded layer code");
    }

    #[test]
    fn reject_run_measures_the_matrix_without_failing_on_sizes() {
        let session = block_session(vec![lump("REJECT", &[0b0000_0010_u8])]);
        let (report, publish) =
            super::reject_run(&session, &request(None, Scope::Merged, Detail::Summary))
                .expect("runs");
        assert_eq!(report.bytes, 1, "one byte stored");
        assert_eq!(report.expected_bytes, 1, "two sectors need one byte");
        assert_eq!(report.sectors, 2, "the decoded sector count");
        assert!(report.size_ok, "the sizes agree");
        assert!(!report.all_zero, "one pair is rejected");
        assert_eq!(report.rejected_pairs, 1, "and it counts");
        assert!(publish.is_none(), "summary publishes nothing");
        let (full, held) = super::reject_run(&session, &request(None, Scope::Merged, Detail::Full))
            .expect("full runs");
        assert!(full.asset.is_none(), "the slot fills only after delivery");
        let job = held.expect("full detail holds a body back");
        assert_eq!(job.filename, "MAP01_reject.json", "the asset file name");
        let text = String::from_utf8(job.data).expect("json is utf-8");
        assert!(
            text.contains("\"01\"") && text.contains("\"00\""),
            "the rows: {text}"
        );
        let empty = block_session(vec![marker("REJECT")]);
        let (zero, _) = super::reject_run(&empty, &request(None, Scope::Merged, Detail::Summary))
            .expect("empty is data");
        assert_eq!(zero.bytes, 0, "an empty lump reports zero bytes");
        assert!(zero.all_zero, "and nothing rejected");
    }

    #[test]
    fn blockmap_run_summarizes_or_zeros_and_refuses_malformed_lists() {
        let mut w = Writer::new();
        w.put_i16(0);
        w.put_i16(0);
        w.put_u16(1);
        w.put_u16(2);
        w.put_u16(6);
        w.put_u16(9);
        w.put_u16(0);
        w.put_u16(3);
        w.put_u16(0xFFFF);
        w.put_u16(0);
        w.put_u16(0xFFFF);
        let good = w.into_inner();
        let bytes = good.len();
        let session = block_session(vec![lump("BLOCKMAP", &good)]);
        let (report, held) =
            super::blockmap_run(&session, &request(None, Scope::Merged, Detail::Full))
                .expect("runs");
        assert_eq!(report.bytes, bytes, "the lump size");
        assert_eq!((report.columns, report.rows), (1, 2), "the grid");
        assert_eq!(report.blocks, 2, "two cells");
        assert_eq!(report.total_entries, 1, "one reference");
        assert_eq!(report.max_lines_per_block, 1, "in one list");
        assert_eq!(report.empty_blocks, 1, "beside an empty one");
        let job = held.expect("full detail holds a body back");
        assert_eq!(job.filename, "MAP01_blockmap.json", "the asset file name");
        let empty = block_session(vec![marker("BLOCKMAP")]);
        let (zero, _) = super::blockmap_run(&empty, &request(None, Scope::Merged, Detail::Full))
            .expect("empty is data");
        assert_eq!(zero.bytes, 0, "an empty lump reports zero bytes");
        assert_eq!(zero.blocks, 0, "and no cells");
        let broken = block_session(vec![lump("BLOCKMAP", b"12345678")]);
        let err = super::blockmap_run(&broken, &request(None, Scope::Merged, Detail::Summary))
            .expect_err("an empty grid with no lists is malformed");
        assert_eq!(err.code(), "format_error", "malformed blockmap code");
    }

    #[test]
    fn blockmap_reports_summarize_share_and_empty_blocks() {
        let empty = empty_blockmap_report(name("MAP01"));
        assert_eq!(empty.bytes, 0, "absent data reports zero bytes");
        assert_eq!(empty.columns, 0, "and no grid");
        assert_eq!(empty.max_lines_per_block, 0, "no lists");

        let mut w = Writer::new();
        w.put_i16(-128);
        w.put_i16(-256);
        w.put_u16(1);
        w.put_u16(2);
        // Two cells: one list of two linedefs at word 6, one empty list
        // at word 10 (the table holds two words at 4 and 5).
        w.put_u16(6);
        w.put_u16(10);
        w.put_u16(0);
        w.put_u16(5);
        w.put_u16(9);
        w.put_u16(0xFFFF);
        w.put_u16(0);
        w.put_u16(0xFFFF);
        let parsed = Blockmap::parse(&w.into_inner()).expect("the fixture parses");
        let report = blockmap_summary(name("MAP01"), 24, &parsed);
        assert_eq!(report.blocks, 2, "two cells");
        assert_eq!(report.total_entries, 2, "two references");
        assert_eq!(report.max_lines_per_block, 2, "the wider list");
        assert_eq!(report.empty_blocks, 1, "the empty cell");
        assert_eq!(report.origin.x, -128, "the origin object");
        assert_eq!(report.origin.y, -256, "on both axes");
        let json = json_bytes(&parsed).expect("the asset serializes");
        assert!(
            String::from_utf8_lossy(&json).contains("\"blocks\""),
            "the full body carries the lists",
        );
    }

    /// A decodable `MAP01` with real geometry: vertices `(0, 0)` and
    /// `(200, 0)`, one one-sided line between them (its sidedef
    /// reference dangles), two sectors, and a `REJECT` placeholder but
    /// no `BLOCKMAP` lump.
    fn build_session() -> Session {
        let mut verts = Writer::new();
        for (x, y) in [(0_i16, 0_i16), (200_i16, 0_i16)] {
            verts.put_i16(x);
            verts.put_i16(y);
        }
        let mut line = Writer::new();
        line.put_u16(0);
        line.put_u16(1);
        line.put_u16(1);
        line.put_u16(0);
        line.put_u16(0);
        line.put_u16(0);
        line.put_u16(0xFFFF);
        session_over(vec![
            marker("MAP01"),
            marker("THINGS"),
            lump("LINEDEFS", &line.into_inner()),
            marker("SIDEDEFS"),
            lump("VERTEXES", &verts.into_inner()),
            lump("SECTORS", &two_sectors()),
            marker("SEGS"),
            marker("SSECTORS"),
            marker("NODES"),
            marker("REJECT"),
        ])
    }

    /// The first working-archive lump named `text`.
    fn written<'lump>(session: &'lump Session, text: &[u8]) -> &'lump Lump {
        session
            .working()
            .lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == text)
            .unwrap_or_else(|| panic!("no {text:?} lump was written"))
    }

    #[test]
    fn install_lump_replaces_in_place_or_inserts_after_its_neighbour() {
        let mut map = Map {
            other: vec![
                marker("SEGS"),
                marker("SSECTORS"),
                lump("NODES", b"nd"),
                marker("REJECT"),
                lump("BLOCKMAP", b"old"),
                marker("BEHAVIOR"),
            ],
            ..Map::default()
        };
        install_lump(
            &mut map,
            LumpName::from_raw(*b"BLOCKMAP"),
            b"new",
            &super::BLOCKMAP_AFTER,
        );
        assert_eq!(map.other.len(), 6, "the replacement did not grow the block");
        let at = map
            .other
            .iter()
            .position(|lump| lump.name.as_bytes() == b"BLOCKMAP")
            .expect("BLOCKMAP is present");
        assert_eq!(at, 4, "it kept its position");
        assert_eq!(
            map.other.get(at).map(|lump| lump.data.as_ref()),
            Some(&b"new"[..]),
            "and took the new bytes"
        );
        let mut bare = Map {
            other: vec![marker("SEGS"), marker("NODES"), marker("BEHAVIOR")],
            ..Map::default()
        };
        install_lump(
            &mut bare,
            LumpName::from_raw(*b"REJECT\0\0"),
            b"r",
            &super::REJECT_AFTER,
        );
        let names: Vec<String> = bare
            .other
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(
            names,
            ["SEGS", "NODES", "REJECT", "BEHAVIOR"],
            "an absent lump lands after its first canonical neighbour"
        );
        let mut empty = Map::default();
        install_lump(
            &mut empty,
            LumpName::from_raw(*b"BLOCKMAP"),
            b"b",
            &super::BLOCKMAP_AFTER,
        );
        assert_eq!(empty.other.len(), 1, "with no neighbour it appends");
    }

    #[test]
    fn parse_mode_defaults_to_zero_and_refuses_anything_else() {
        assert_eq!(parse_mode(None).expect("absent"), RejectMode::Zero);
        assert_eq!(
            parse_mode(Some("Components")).expect("components"),
            RejectMode::Components
        );
        let error = parse_mode(Some("sightlines")).expect_err("not a mode");
        assert_eq!(error.code(), "invalid_argument", "bad mode code");
        let message = error.to_string();
        assert!(
            message.contains("zero") && message.contains("components"),
            "the message names both values: {message}"
        );
    }

    #[test]
    fn blockmap_build_run_writes_the_grid_into_the_working_archive() {
        let mut session = build_session();
        let report = blockmap_build_run(
            &mut session,
            &BuildBlockmapParams {
                map: Some("map01".to_owned()),
                compress: None,
            },
        )
        .expect("the fixture builds a blockmap");
        assert_eq!(report.columns, 2, "x 0..200 from -8 spans two cells");
        assert_eq!(report.rows, 1, "y 0..0 fits one row");
        assert_eq!(
            (report.origin.x, report.origin.y),
            (-8_i16, -8_i16),
            "origins"
        );
        assert_eq!(report.bytes, 18, "the two identical lists share one copy");
        assert!(report.vanilla_safe, "a toy grid is tiny");
        assert_eq!(
            report.edit.get("map"),
            Some(&json!("MAP01")),
            "the fragment"
        );
        assert_eq!(
            report.edit.get("nodes_invalidated"),
            Some(&json!(false)),
            "a lump rewrite is not a geometry edit"
        );
        assert_eq!(report.edit.get("copied_to_working"), Some(&json!(true)));
        assert_eq!(
            session.undo_labels(),
            vec!["build blockmap".to_owned()],
            "one undo step"
        );
        let parsed = Blockmap::parse(written(&session, b"BLOCKMAP").data.as_ref())
            .expect("the written lump parses");
        assert_eq!(
            parsed.blocks,
            vec![vec![0_u16], vec![0]],
            "the line is listed in both cells"
        );
        let plain = blockmap_build_run(
            &mut session,
            &BuildBlockmapParams {
                map: None,
                compress: Some(false),
            },
        )
        .expect("the uncompressed build runs too");
        assert_eq!(plain.bytes, 24, "every cell writes its own list");
        assert_eq!(plain.edit.get("copied_to_working"), Some(&json!(false)));
    }

    #[test]
    fn reject_build_run_writes_zero_then_components() {
        let mut session = build_session();
        let zero = reject_build_run(
            &mut session,
            &BuildRejectParams {
                map: None,
                mode: None,
            },
        )
        .expect("the default mode builds");
        assert_eq!(zero.bytes, 1, "two sectors need one byte");
        assert_eq!(zero.sectors, 2, "the decoded sector count");
        assert_eq!(zero.rejected_pairs, 0, "zero rejects nothing");
        assert_eq!(zero.mode, "zero", "the echo");
        let parts = reject_build_run(
            &mut session,
            &BuildRejectParams {
                map: None,
                mode: Some("COMPONENTS".to_owned()),
            },
        )
        .expect("components builds");
        assert_eq!(parts.rejected_pairs, 2, "the dangling side joins nothing");
        assert_eq!(parts.mode, "components", "the echo");
        assert_eq!(
            written(&session, b"REJECT").data.as_ref(),
            &[0b0000_0110_u8],
            "bits (0,1) and (1,0) only"
        );
        assert_eq!(
            session.undo_labels().len(),
            2,
            "the two builds are two steps"
        );
    }

    #[test]
    fn build_runs_refuse_without_writing_anything() {
        let mut session = block_session(Vec::new());
        let error = blockmap_build_run(
            &mut session,
            &BuildBlockmapParams {
                map: None,
                compress: None,
            },
        )
        .expect_err("the fixture has no vertices");
        assert_eq!(error.code(), "invalid_argument", "empty map code");
        assert!(session.working().is_empty(), "the copy rolled back");
        assert!(session.undo_labels().is_empty(), "and no history entry");
        let bad = reject_build_run(
            &mut session,
            &BuildRejectParams {
                map: None,
                mode: Some("opaque".to_owned()),
            },
        )
        .expect_err("not a mode");
        assert_eq!(bad.code(), "invalid_argument", "bad mode code");
        assert!(session.working().is_empty(), "still nothing written");
    }

    /// A base layer holding `lumps`.
    fn base(lumps: Vec<Lump>) -> Layer {
        Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        )
    }

    /// A session over a base layer of `lumps`.
    fn session_over(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(base(lumps));
        session
    }
}
