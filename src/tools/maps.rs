//! Map tools: [`Cyberdemon::list_maps`], [`Cyberdemon::get_map_lumps`],
//! [`Cyberdemon::get_map_summary`], [`Cyberdemon::assemble_map`] and
//! [`Cyberdemon::set_active_map`], built on the shared resolvers in
//! [`crate::tools::maps::common`] that every later map tool reuses.

mod assemble;
pub(in crate::tools) mod common;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session, Source};
use crate::state::AppState;
use crate::tools::maps::common::Directory;
use crate::tools::{fail, ok};
use crate::wad::archive::Lump;
use crate::wad::classify::{LumpKind, MapBlock, MapFormat};
use crate::wad::map::read_map;
use crate::wad::map::stats::{MapSummary, summarize};
use crate::wad::name::LumpName;

/// Asset file extension for published raw map lumps.
const LUMP_EXTENSION: &str = "lmp";

/// Arguments for [`Cyberdemon::list_maps`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListMapsParams {
    /// Which archive to list: `"merged"` (the default: the directory an
    /// engine sees), `"working"` (the editable archive alone) or
    /// `"layer:N"` (read-only layer `N`, 0 is the IWAD).
    #[serde(default)]
    scope: Option<String>,
}

/// Arguments for [`Cyberdemon::get_map_lumps`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetMapLumpsParams {
    /// The map to read, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// Publish every non-empty lump's raw bytes as an asset and attach an
    /// `"asset"` to its row (default `false`).
    #[serde(default)]
    urls: bool,
}

/// Arguments for [`Cyberdemon::get_map_summary`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetMapSummaryParams {
    /// The map to summarize, e.g. `"MAP01"`. Defaults to the session's
    /// active map; with none active, the merged view's only map when it
    /// holds exactly one. Without either the call fails with
    /// `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
}

/// Arguments for [`Cyberdemon::assemble_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AssembleMapParams {
    /// The map to assemble, e.g. `"MAP01"`. Defaults to the session's
    /// active map; with none active, the merged view's only map when it
    /// holds exactly one. Without either the call fails with
    /// `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// Which element arrays to return: a subset of `"vertices"`,
    /// `"linedefs"`, `"sidedefs"`, `"sectors"`, `"things"`. Defaults to
    /// all five; an unknown name fails with `invalid_argument`.
    #[serde(default)]
    include: Option<Vec<String>>,
    /// Inline the whole graph when its total element count is at most this
    /// (default 2000, maximum 20000); above it the JSON is published as an
    /// asset and only a summary is returned. `0` or a value above the
    /// maximum fail with `invalid_argument`.
    #[serde(default)]
    inline_limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::set_active_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetActiveMapParams {
    /// The map to make active (1 to 8 printable ASCII characters,
    /// case-insensitive); `null` - or omitted - clears the selection.
    #[serde(default)]
    map: Option<String>,
}

/// The accepted `scope` text, the merged view being the default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
}

/// One map block as [`Cyberdemon::list_maps`] reports it.
#[derive(Serialize)]
struct MapRow {
    /// The block's header name.
    name: LumpName,
    /// The layout its member lumps use.
    format: MapFormat,
    /// The namespace a UDMF `TEXTMAP` declares; `null` for binary blocks
    /// and for blocks that do not decode.
    udmf_namespace: Option<String>,
    /// Where the scope took the block's header from; set for every listed
    /// block.
    source: Option<Source>,
    /// Member lump names in directory order, the GL node run included,
    /// without the map header itself.
    lumps: Vec<String>,
    /// Whether the block carries node data: a non-empty `NODES` lump or a
    /// `ZNODES` lump.
    has_nodes: bool,
    /// Whether a `GL_` node block is attached.
    has_gl_nodes: bool,
}

/// Structured result of [`Cyberdemon::list_maps`].
#[derive(Serialize)]
struct ListMapsReport {
    /// The scope's map blocks, in directory order.
    maps: Vec<MapRow>,
}

/// The namespace a UDMF block declares, read by decoding it through
/// [`common::load_map`]; a block that does not decode reports `null`
/// instead of failing the listing.
///
/// Binary blocks are never decoded: they have no namespace.
fn udmf_namespace(session: &Session, block: &MapBlock, scope: Scope) -> Option<String> {
    if block.format != MapFormat::Udmf {
        return None;
    }
    common::load_map(session, block.name, scope)
        .ok()
        .and_then(|map| map.udmf_namespace)
}

/// One block as a row: the classified listing plus the decoded namespace
/// and node presence.
fn map_row(session: &Session, dir: &Directory, block: &MapBlock, scope: Scope) -> MapRow {
    MapRow {
        name: block.name,
        format: block.format,
        udmf_namespace: udmf_namespace(session, block, scope),
        source: dir.entries.get(block.header).map(|entry| entry.source),
        lumps: dir.block_lump_names(block),
        has_nodes: dir.block_has_nodes(block),
        has_gl_nodes: block.gl.is_some(),
    }
}

/// `list_maps`' work: classify the scope and report every map block.
fn list_run(session: &Session, scope: Scope) -> Result<ListMapsReport, CyberError> {
    let dir = common::directory(session, scope)?;
    let maps = dir
        .analysis
        .maps
        .iter()
        .map(|block| map_row(session, &dir, block, scope))
        .collect();
    Ok(ListMapsReport { maps })
}

/// One raw lump of a map block, as [`Cyberdemon::get_map_lumps`] reports
/// it.
#[derive(Serialize)]
struct MapLumpRow {
    /// Lump name.
    name: LumpName,
    /// Payload size in bytes.
    size: usize,
    /// Type hint, the same values as `list_lumps` rows.
    kind: LumpKind,
    /// The published raw bytes; only in `urls` mode, and absent for
    /// zero-size lumps.
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<AssetRef>,
}

/// Structured result of [`Cyberdemon::get_map_lumps`].
#[derive(Serialize)]
struct MapLumpsReport {
    /// The map that was read.
    map: LumpName,
    /// The layout its lumps use.
    format: MapFormat,
    /// The block's lumps in directory order, the GL run included.
    lumps: Vec<MapLumpRow>,
}

/// One publish job held back until the session lock is released.
struct Publish {
    /// Index of the row this asset belongs to.
    at: usize,
    /// File name to publish under: `<MAP>_<LUMP>.lmp`.
    filename: String,
    /// The raw payload.
    data: Vec<u8>,
}

/// The rows of one `get_map_lumps` call plus the publishes they may still
/// need.
struct Plan {
    /// The map read.
    map: LumpName,
    /// Its format.
    format: MapFormat,
    /// The rows, in directory order.
    rows: Vec<MapLumpRow>,
    /// The held-back publish jobs, in row order.
    publishes: Vec<Publish>,
}

/// `get_map_lumps`' work under the session lock: read the scope, find the
/// block named `name` and build its raw lump rows (plus the publish jobs
/// `urls` asks for).
fn get_run(
    session: &Session,
    name: LumpName,
    scope: Scope,
    urls: bool,
) -> Result<Plan, CyberError> {
    let dir = common::directory(session, scope)?;
    let block = dir
        .find_block(name)
        .ok_or_else(|| common::missing_map(name, scope))?;
    let mut rows: Vec<MapLumpRow> = Vec::new();
    let mut publishes: Vec<Publish> = Vec::new();
    for position in Directory::block_positions(block) {
        let (Some(lump), Some(info)) = (dir.lumps.get(position), dir.analysis.lumps.get(position))
        else {
            continue;
        };
        if urls && info.size > 0 {
            let lump_name = lump.name;
            publishes.push(Publish {
                at: rows.len(),
                filename: format!("{name}_{lump_name}.{LUMP_EXTENSION}"),
                data: lump.data.to_vec(),
            });
        }
        rows.push(MapLumpRow {
            name: lump.name,
            size: info.size,
            kind: info.kind,
            asset: None,
        });
    }
    Ok(Plan {
        map: name,
        format: block.format,
        rows,
        publishes,
    })
}

/// Publish the held-back bytes outside the session lock and assemble the
/// report.
///
/// # Errors
///
/// [`AppState::publish`]'s errors when an asset cannot be stored.
fn deliver(state: &AppState, mut plan: Plan) -> Result<MapLumpsReport, CyberError> {
    let mime = mime_for_extension(LUMP_EXTENSION);
    for publish in plan.publishes {
        let asset = state.publish(&publish.filename, mime, publish.data)?;
        if let Some(row) = plan.rows.get_mut(publish.at) {
            row.asset = Some(asset);
        }
    }
    Ok(MapLumpsReport {
        map: plan.map,
        format: plan.format,
        lumps: plan.rows,
    })
}

/// Structured result of [`Cyberdemon::get_map_summary`]: the pure summary
/// of the decoded map with the block's origin added.
#[derive(Serialize)]
struct MapSummaryReport {
    /// The map's statistics, serialized into this object.
    #[serde(flatten)]
    summary: MapSummary,
    /// Where the scope took the block's header from; set for every found
    /// map.
    source: Option<Source>,
}

/// `get_map_summary`' work: resolve the map, decode its block out of
/// `scope` and summarize it against the block's member lumps (the `GL`
/// node run included).
fn summary_run(
    session: &Session,
    requested: Option<&str>,
    scope: Scope,
) -> Result<MapSummaryReport, CyberError> {
    let name = common::resolve_map_name(session, requested)?;
    let dir = common::directory(session, scope)?;
    let block = dir
        .find_block(name)
        .ok_or_else(|| common::missing_map(name, scope))?;
    let map = read_map(&dir.lumps, block)?;
    let block_lumps: Vec<Lump> = Directory::block_positions(block)
        .into_iter()
        .filter_map(|position| dir.lumps.get(position))
        .cloned()
        .collect();
    Ok(MapSummaryReport {
        summary: summarize(&map, &block_lumps),
        source: dir.entries.get(block.header).map(|entry| entry.source),
    })
}

/// Structured result of [`Cyberdemon::set_active_map`].
#[derive(Debug, Serialize)]
struct ActiveMapReport {
    /// The map that is active now, `null` after a clear.
    active_map: Option<LumpName>,
}

/// `set_active_map`'s work: clear on `null`, else parse the name, require
/// the merged view to hold that block and store the selection. This is a
/// session setting, not an undoable edit.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unparsable name and
/// [`CyberError::NotFound`] when the merged view holds no such block; the
/// previous selection stands in both cases.
fn set_active_run(
    session: &mut Session,
    requested: Option<&str>,
) -> Result<ActiveMapReport, CyberError> {
    let Some(text) = requested else {
        session.set_active_map(None);
        return Ok(ActiveMapReport { active_map: None });
    };
    let name = LumpName::parse(text)?;
    let dir = common::directory(session, Scope::Merged)?;
    if dir.find_block(name).is_none() {
        return Err(common::missing_map(name, Scope::Merged));
    }
    session.set_active_map(Some(name));
    Ok(ActiveMapReport {
        active_map: Some(name),
    })
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = maps_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the map blocks of a scope with formats and sources.
    #[tool(
        description = "Lists every map block of a scope (\"merged\" default: the directory an engine sees, \"working\", \"layer:N\") in directory order. Returns {maps: [{name, format: \"doom\"|\"hexen\"|\"udmf\", udmf_namespace (the namespace a UDMF TEXTMAP declares; null for binary blocks and undecodable ones), source (\"working\" or {\"layer\": n}: where a merged listing took the block from), lumps (member lump names in directory order, an attached GL node run included, without the map header), has_nodes (a non-empty NODES or a ZNODES lump is present), has_gl_nodes (a GL_ node block is attached)}]}. Unknown scope names are invalid_argument; an unloaded layer is not_found. See skill/reference/maps.md for how maps are stored."
    )]
    fn list_maps(
        &self,
        Parameters(args): Parameters<ListMapsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match parse_scope(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        reported(
            self.state()
                .with_session(|session| list_run(session, scope)),
        )
    }

    /// Reports one map block's raw lump set, optionally publishing bytes.
    #[tool(
        description = "Reports the raw lump set of one map block: THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SECTORS and whatever else the block carries (SEGS, SSECTORS, NODES, REJECT, BLOCKMAP, BEHAVIOR, TEXTMAP, ZNODES, ENDMAP and an attached GL node run), in directory order. map defaults to the active map (set_active_map); with none active it resolves only when the merged view holds exactly one map, else invalid_argument. scope (\"merged\" default, \"working\", \"layer:N\") picks the archive to read; the map default always looks at the merged view. Returns {map, format, lumps: [{name, size, kind}]} with list_lumps' kind values. urls true additionally publishes every non-empty lump's raw bytes as an asset named <MAP>_<LUMP>.lmp and those rows carry \"asset\": {url, filename, mime, size}; zero-size lumps get none. An unknown map is not_found, an unparsable name or scope is invalid_argument."
    )]
    fn get_map_lumps(
        &self,
        Parameters(args): Parameters<GetMapLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match parse_scope(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        let plan = match self.state().with_session(|session| {
            let name = common::resolve_map_name(session, args.map.as_deref())?;
            get_run(session, name, scope, args.urls)
        }) {
            Ok(plan) => plan,
            Err(error) => return fail(&error),
        };
        reported(deliver(self.state(), plan))
    }

    /// Summarizes one map cheaply: counts, bounds, heights and node state.
    #[tool(
        description = "A cheap, compact overview of one map, decoded but never measured in detail. map defaults to the active map (set_active_map, else the merged view's only map); scope picks the archive (\"merged\" default, \"working\", \"layer:N\"). Returns {name, format, udmf_namespace, source (\"working\" or {\"layer\": n}: where the scope took the block's header from), counts {things, vertices, linedefs, sidedefs, sectors}, bounds {min_x, min_y, max_x, max_y, width, height} or null with no vertices, lines {one_sided, two_sided (both sides set), with_special (nonzero special)}, heights {floor_min, floor_max, ceiling_min, ceiling_max across the sectors; nulls with no sectors}, things_by_type (at most 50 {type, count} rows, most numerous first, ties by ascending type), player_starts (which of types 1, 2, 3, 4 are present), textures_used and flats_used (sorted, deduplicated, without \"-\"), nodes {present (bytes in NODES or ZNODES), segs, subsectors, nodes (record counts from SEGS/12, SSECTORS/4, NODES/28 while NODES is vanilla; replaced by nodes_format, e.g. \"XNOD\" or \"ZGLN\", when it starts with such a magic), reject_bytes, blockmap_bytes, znodes (a ZNODES lump exists), gl (a GL_ lump exists)}}. Unknown map is not_found, unparsable map or scope or no resolvable default is invalid_argument, an undecodable block is format_error."
    )]
    fn get_map_summary(
        &self,
        Parameters(args): Parameters<GetMapSummaryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match parse_scope(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        reported(
            self.state()
                .with_session(|session| summary_run(session, args.map.as_deref(), scope)),
        )
    }

    /// Returns the whole map as one typed graph with resolved references.
    #[tool(
        description = "Returns a whole map as one typed graph whose references are already resolved, so the map can be reasoned about without chasing indices. map defaults to the active map (set_active_map, else the merged view's only map); scope picks the archive (\"merged\" default, \"working\", \"layer:N\"). include selects the arrays to return - any subset of \"vertices\", \"linedefs\", \"sidedefs\", \"sectors\", \"things\" (default all five); an unknown name is invalid_argument. Inline rows: vertices {index, x, y, linedefs}; linedefs {index, v1: {index,x,y}|null, v2: ..., length (floor-rounded map units)|null, flags, flag_names (named by the map's flag family), special, tag, args, front|back: {sidedef, sector|null, upper, middle, lower, x_offset, y_offset}|null}; sidedefs {index, sector|null, linedef|null, side: \"front\"|\"back\"|null (the first line using it), upper, middle, lower, x_offset, y_offset}; sectors {index, floor_height, ceiling_height, floor_flat, ceiling_flat, light, special, tag, linedefs (boundary line indices), sidedefs, neighbors (sectors across two-sided lines), things (indices of things standing in it, point-in-sector), bbox [min_x,min_y,max_x,max_y]|null}; things {index, type, x, y, z, angle, flags, flag_names, tid, special, args, sector|null}. problems lists every dangling vertex/sidedef/sector reference found; broken references never fail the call, the unresolvable part is null. While the included arrays hold at most inline_limit elements (default 2000, maximum 20000; 0 or above the maximum are invalid_argument) the whole graph is returned; above it the result is instead {map, counts, asset} with the full JSON published as <MAP>_graph.json (application/json). Unknown map is not_found, unparsable map or scope or no resolvable default is invalid_argument, an undecodable block is format_error."
    )]
    fn assemble_map(
        &self,
        Parameters(args): Parameters<AssembleMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match parse_scope(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        let include = match assemble::parse_include(args.include) {
            Ok(include) => include,
            Err(error) => return fail(&error),
        };
        let limit = match assemble::parse_inline_limit(args.inline_limit) {
            Ok(limit) => limit,
            Err(error) => return fail(&error),
        };
        let plan = match self.state().with_session(|session| {
            let name = common::resolve_map_name(session, args.map.as_deref())?;
            let map = common::load_map(session, name, scope)?;
            Ok(assemble::assemble(name, &map, &include))
        }) {
            Ok(plan) => plan,
            Err(error) => return fail(&error),
        };
        reported(plan.finish(self.state(), limit))
    }

    /// Selects or clears the map the map tools default to.
    #[tool(
        description = "Chooses the map the map tools use when their optional map parameter is omitted (see skill/reference/maps.md). map must name a map block of the merged view (not_found otherwise, the previous selection standing); pass null or omit it to clear the selection. This is a session setting, not an undoable edit: the undo stack never holds it. Returns {active_map: name|null}."
    )]
    fn set_active_map(
        &self,
        Parameters(args): Parameters<SetActiveMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| set_active_run(session, args.map.as_deref())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_scope, set_active_run};
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;
    use std::path::PathBuf;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A base layer whose directory is a `MAP01` block.
    fn session_with_map() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![marker("MAP01"), marker("THINGS")],
            },
        ));
        session
    }

    #[test]
    fn parse_scope_defaults_to_the_merged_view() {
        assert_eq!(parse_scope(None).expect("absent"), Scope::Merged);
        assert_eq!(
            parse_scope(Some("working")).expect("working"),
            Scope::Working
        );
        let error = parse_scope(Some("nope")).expect_err("not a scope");
        assert_eq!(error.code(), "invalid_argument", "bad scope code");
    }

    #[test]
    fn set_active_run_selects_validates_and_clears() {
        let mut session = session_with_map();
        let set = set_active_run(&mut session, Some("map01")).expect("MAP01 exists");
        assert_eq!(set.active_map, Some(name("MAP01")), "parsed uppercased");
        let missing =
            set_active_run(&mut session, Some("MAP09")).expect_err("MAP09 is in no archive");
        assert_eq!(missing.code(), "not_found", "unknown map code");
        assert_eq!(
            session.active_map(),
            Some(name("MAP01")),
            "the refusal leaves the selection standing",
        );
        let bad = set_active_run(&mut session, Some("too long name"))
            .expect_err("spaces and length fail the parse");
        assert_eq!(bad.code(), "invalid_argument", "bad name code");
        let cleared = set_active_run(&mut session, None).expect("null clears");
        assert_eq!(cleared.active_map, None, "reported cleared");
        assert_eq!(session.active_map(), None, "and stored cleared");
        assert!(session.undo_labels().is_empty(), "no history entry appears");
    }
}
