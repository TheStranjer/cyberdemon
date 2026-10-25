//! Map tools: reading, selection and management of map blocks (see the
//! `maps` router methods on [`crate::server::Cyberdemon`]).
//!
//! The readers are `list_maps`, `get_map_lumps`, `get_map_summary` and
//! `assemble_map`; `set_active_map` picks the map the other tools default
//! to; `create_map`, `copy_map`, `rename_map` and `remove_map` manage
//! whole blocks of the working archive. Everything reads through the
//! shared resolvers in `common` that every
//! later map tool reuses, and the element-editing tools will reuse the
//! parameter types and validators in [`crate::tools::maps::params`]. The
//! management machinery (block append, copy, rename and removal over the
//! working archive) lives in the private `manage` module.

mod assemble;
pub mod common;
mod manage;
pub mod params;

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
use crate::tools::{fail, reply};
use crate::wad::archive::{Lump, Wad};
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

/// Arguments for [`Cyberdemon::create_map`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateMapParams {
    /// The new map's header name (1 to 8 printable ASCII characters,
    /// case-insensitive). Anything but `ExMy`/`MAPxx` succeeds with a
    /// warning: engines only load those two forms unless MAPINFO defines
    /// the name.
    pub(crate) name: String,
    /// The layout of the empty block: `"doom"` (the default), `"hexen"`
    /// or `"udmf"`.
    #[serde(default)]
    pub(crate) format: Option<String>,
    /// The namespace the `UDMF` `TEXTMAP` declares (default `"zdoom"`);
    /// only valid with `format` `"udmf"`.
    #[serde(default)]
    pub(crate) namespace: Option<String>,
    /// Discard the working archive's block of this name (and its GL run)
    /// before creating (default `false`; a map only lower layers hold is
    /// always overridable and needs no replace).
    #[serde(default)]
    pub(crate) replace: bool,
}

/// Arguments for [`Cyberdemon::copy_map`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CopyMapParams {
    /// The map to read, e.g. `"MAP01"`.
    pub(crate) from: String,
    /// The name the copy gets in the working archive.
    pub(crate) to: String,
    /// Which archive to read the source from: `"merged"` (the default),
    /// `"working"` or `"layer:N"`.
    #[serde(default)]
    pub(crate) scope: Option<String>,
    /// Discard the working archive's block named `to` (and its GL run)
    /// before copying (default `false`).
    #[serde(default)]
    pub(crate) replace: bool,
}

/// Arguments for [`Cyberdemon::rename_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RenameMapParams {
    /// The working archive's map to rename (its copy-on-write copy included).
    pub(crate) from: String,
    /// The new name (1 to 8 printable ASCII characters, case-insensitive).
    pub(crate) to: String,
}

/// Arguments for [`Cyberdemon::remove_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveMapParams {
    /// The working archive's map block to remove, GL node run included.
    pub(crate) name: String,
}

/// The `UDMF` namespace `create_map` declares when the caller gives none.
const DEFAULT_NAMESPACE: &str = "zdoom";
/// Longest accepted `UDMF` namespace name, in characters.
const MAX_NAMESPACE_LEN: usize = 32;

/// The `format` parameter: `doom` (absent means this), `hexen` or
/// `udmf`, case-insensitively. Shared by `create_map` and
/// `build_map_from_plan`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
pub(crate) fn parse_format(text: Option<&str>) -> Result<MapFormat, CyberError> {
    text.map_or(Ok(MapFormat::Doom), |raw| {
        MapFormat::parse(raw).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "unknown map format {raw:?}: use \"doom\", \"hexen\" or \"udmf\""
            ))
        })
    })
}

/// Whether `text` is an acceptable `UDMF` namespace: 1 to
/// [`MAX_NAMESPACE_LEN`] `ASCII` letters, digits or underscores.
fn namespace_valid(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NAMESPACE_LEN
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The namespace the new `UDMF` map declares: the lowercased parameter,
/// `zdoom` when absent, and `None` for the binary formats. Shared by
/// `create_map` and `build_map_from_plan` (which only consults it for
/// `UDMF` and ignores a namespace beside a binary layout).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a namespace is given for a binary
/// format or holds characters a namespace name excludes.
pub(crate) fn resolve_namespace(
    text: Option<&str>,
    format: MapFormat,
) -> Result<Option<String>, CyberError> {
    match (text, format) {
        (None, MapFormat::Udmf) => Ok(Some(DEFAULT_NAMESPACE.to_owned())),
        (None, MapFormat::Doom | MapFormat::Hexen) => Ok(None),
        (Some(raw), MapFormat::Doom | MapFormat::Hexen) => Err(CyberError::InvalidArgument(
            format!("namespace {raw:?} only applies to UDMF maps: drop it for the binary format"),
        )),
        (Some(raw), MapFormat::Udmf) => {
            let lowered = raw.to_ascii_lowercase();
            if namespace_valid(&lowered) {
                return Ok(Some(lowered));
            }
            Err(CyberError::InvalidArgument(format!(
                "namespace {raw:?} is not a UDMF namespace name: use 1 to {MAX_NAMESPACE_LEN} \
                 ASCII letters, digits or underscores, or omit it for \"{DEFAULT_NAMESPACE}\""
            )))
        }
    }
}

/// Appends the freshly `write_map`-encoded `lumps` as the working
/// archive's new block for `name`, first discarding the old block of
/// that name when `replace` allows it: `build_map_from_plan`'s write
/// path, sharing `create_map`'s append rules (see
/// [`manage::append_block`]).
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] when the working archive holds a block
/// named `name` and `replace` is `false`.
pub(crate) fn write_block(
    wad: &mut Wad,
    name: LumpName,
    replace: bool,
    lumps: Vec<Lump>,
) -> Result<(), CyberError> {
    manage::append_block(wad, name, replace, lumps)
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = maps_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the map blocks of a scope with formats and sources.
    #[tool(
        description = "Lists every map block of a scope (merged default, working or layer:N) in directory order with its format, member lump names and element counts. Use it to find the map names every map tool takes; a not_found on a map parameter means the name is not in this list."
    )]
    fn list_maps(
        &self,
        Parameters(args): Parameters<ListMapsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match Scope::parse_or_merged(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        reply(
            self.state()
                .with_session(|session| list_run(session, scope)),
        )
    }

    /// Reports one map block's raw lump set, optionally publishing bytes.
    #[tool(
        description = "Reports the raw lump set of one map block: the header position, its member lumps with sizes, and the attached GL run when present - the directory view, nothing decoded. Use it to see what a block carries before lump-level surgery; get_map_summary reads the elements themselves."
    )]
    fn get_map_lumps(
        &self,
        Parameters(args): Parameters<GetMapLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match Scope::parse_or_merged(args.scope.as_deref()) {
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
        reply(deliver(self.state(), plan))
    }

    /// Summarizes one map cheaply: counts, bounds, heights and node state.
    #[tool(
        description = "A cheap, compact overview of one map, decoded but never measured: element counts by kind, format, bounds and the block's lump layout. Use it to decide where to look next; describe_map reads sector facts and check_playability walks the whole map."
    )]
    fn get_map_summary(
        &self,
        Parameters(args): Parameters<GetMapSummaryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match Scope::parse_or_merged(args.scope.as_deref()) {
            Ok(scope) => scope,
            Err(error) => return fail(&error),
        };
        reply(
            self.state()
                .with_session(|session| summary_run(session, args.map.as_deref(), scope)),
        )
    }

    /// Returns the whole map as one typed graph with resolved references.
    #[tool(
        description = "Returns one whole map as a typed graph with every reference resolved - things, vertices, linedefs with their sides, sectors - dangling references reported as problems; past the inline size the graph comes back as a JSON asset URL. Use it to reason about a map without chasing indices; for sector-level facts prefer describe_map."
    )]
    fn assemble_map(
        &self,
        Parameters(args): Parameters<AssembleMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = match Scope::parse_or_merged(args.scope.as_deref()) {
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
        reply(plan.finish(self.state(), limit))
    }

    /// Selects or clears the map the map tools default to.
    #[tool(
        description = "Chooses the map the map tools use when their optional map parameter is omitted: pass a map name from the merged view (list_maps shows them) or null to clear the choice. Without a selection, tools default to the merged view's only map when there is exactly one. Use it once per session to stop repeating map everywhere."
    )]
    fn set_active_map(
        &self,
        Parameters(args): Parameters<SetActiveMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| set_active_run(session, args.map.as_deref())),
        )
    }

    /// Appends an empty map block to the working archive.
    #[tool(
        description = "Creates a level: appends an empty map block to the working archive as one undoable edit - the first step of hand-building a map with the element tools. name is 1 to 8 printable ASCII characters; a working name already held is already_exists unless replace: true. Prefer build_map_from_plan to get a playable map in one call."
    )]
    fn create_map(
        &self,
        Parameters(args): Parameters<CreateMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let plan = match manage::plan_create(&args) {
            Ok(plan) => plan,
            Err(error) => return fail(&error),
        };
        let outcome: Result<manage::CreateReport, CyberError> = self
            .state()
            .with_session(|session| manage::create_run(session, plan));
        reply(outcome)
    }

    /// Copies a scope's map block into the working archive under a new name.
    #[tool(
        description = "Copies a whole map block (and its GL node run) from any scope into the working archive under a new header name as one undoable edit; the destination name must be free unless replace: true. The copy carries the block's GL run along, re-naming its marker to GL_<name> or the GL_LEVEL fallback; the result's warnings name anything it could not carry. Use it to clone a level or to bring a read-only map in for editing; undo removes the copy again."
    )]
    fn copy_map(
        &self,
        Parameters(args): Parameters<CopyMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let plan = match manage::plan_copy(&args) {
            Ok(plan) => plan,
            Err(error) => return fail(&error),
        };
        let outcome: Result<manage::CopyReport, CyberError> = self
            .state()
            .with_session(|session| manage::copy_run(session, &plan));
        reply(outcome)
    }

    /// Renames a working-archive map block in place.
    #[tool(
        description = "Renames a map block of the working archive as one undoable edit: the header lump and its GL node run follow the new name - the marker becomes GL_<name>, or the GL_LEVEL fallback when the name leaves no room behind the prefix - and the result's warnings name any node data it could not carry. The new name must be 1 to 8 printable ASCII characters and free. A block only a read-only layer holds cannot be renamed; copy_map brings one in first."
    )]
    fn rename_map(
        &self,
        Parameters(args): Parameters<RenameMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outcome: Result<manage::RenameReport, CyberError> =
            self.state().with_session(|session| {
                let from = LumpName::parse(&args.from)?;
                let to = LumpName::parse(&args.to)?;
                manage::rename_run(session, from, to)
            });
        reply(outcome)
    }

    /// Removes a map block (and its GL run) from the working archive.
    #[tool(
        description = "Removes a map block and its GL node run from the working archive as one undoable edit: name must be a block the working archive holds - a map only a layer owns is not_found. Use it to drop a level you created; undo restores the whole block."
    )]
    fn remove_map(
        &self,
        Parameters(args): Parameters<RemoveMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outcome: Result<manage::RemoveReport, CyberError> =
            self.state().with_session(|session| {
                let name = LumpName::parse(&args.name)?;
                manage::remove_run(session, name)
            });
        reply(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::set_active_run;
    use crate::session::{Layer, LayerRole, Session};
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
