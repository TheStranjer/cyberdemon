//! Query tools: `find_elements`, the set-valued read behind every
//! plain-English "all the X" request, and CD-108's bulk `replace_*` swaps.
//!
//! The name globs of `replace_texture` and `replace_flat` and the
//! doomednum-or-name sides of `replace_thing_type` fold their per-map
//! counts across whole maps in one undoable step. The filtering is the pure
//! [`crate::wad::map::query`] engine's; this file
//! resolves the flattened query parameters, reads the map and the session's
//! definitions (custom `DECORATE`/`DEHACKED` things included) under one
//! lock exactly like the playability group does, runs the query and pages
//! the result: `count` totals every hit, `indices` and `rows` carry one
//! ascending page of the hits with a compact summary per kind, and
//! `next_offset` names the `offset` where the rest continues. The bulk
//! edits act on exactly the elements `find_elements` reports; the
//! session-facing machinery they share (the `maps` selector, the
//! one-undo-step-over-many-maps runner, the texture stock check and the
//! folded [`replace::ReplaceReport`]) lives in the public `replace`
//! submodule; the pure per-map passes are
//! [`crate::wad::map::bulk::replace_textures`] and its siblings.

pub mod replace;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::query::replace::{
    MapsSelector, ReplaceReport, Replacement, Stock, ThingRef, parse_terms, require_defined_thing,
    require_target, resolve_thing_ref, run_replacement, thing_definitions,
};
use crate::tools::{fail, ok};
use crate::wad::map::Map;
use crate::wad::map::bulk::{Face, Surface, replace_flats, replace_textures, replace_thing_types};
use crate::wad::map::query::{
    ElementKind, Filter, Query, QueryContext, Within, run_query, side_sector,
};
/// Default row cap for the `indices` and `rows` lists.
const DEFAULT_LIMIT: usize = 200;
/// Largest row cap the lists accept.
const MAX_LIMIT: usize = 1000;

/// Arguments for [`Cyberdemon::find_elements`]: the [`Query`] fields
/// flattened beside the paging.
#[derive(Debug, Deserialize, JsonSchema)]
struct FindElementsParams {
    /// The element block to run over: "sectors", "linedefs", "sidedefs",
    /// "things" or "vertices".
    kind: ElementKind,
    /// The filters to AND together, each `{field, op, value}`; the valid
    /// fields depend on the kind and the ops on the field's type. An empty
    /// list keeps every bounded element.
    #[serde(default, rename = "where")]
    r#where: Vec<Filter>,
    /// Bound the hits by region: `{"bbox": [min_x, min_y, max_x, max_y]}`
    /// or `{"sectors": [index, ...]}`.
    #[serde(default)]
    within: Option<Within>,
    /// The map to query, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Skip this many hits before returning (default 0). An offset past the
    /// end is never an error: the page simply comes back empty.
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum hits returned (default 200, at most 1000); anything else
    /// fails with `invalid_argument`. `count` always totals everything.
    #[serde(default)]
    limit: Option<usize>,
}

/// The row cap `limit` asks for, [`DEFAULT_LIMIT`] when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above [`MAX_LIMIT`].
fn parse_limit(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(limit) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    if limit == 0 || limit > MAX_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "limit {limit} is out of range: pass a value between 1 and {MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

/// The map and definitions one query reads, out of the session lock.
struct Prepared {
    /// The map to filter, from the merged view.
    map: Map,
    /// The session's definitions: custom `DECORATE`/`DEHACKED` things
    /// included, exactly what `describe_map` names rows through.
    defs: Definitions,
}

/// Read the query's inputs under the session lock.
///
/// # Errors
///
/// [`common::resolve_map_name`] and [`common::load_map`]'s errors, and the
/// definition-collection errors of [`session_definitions`].
fn load(state: &AppState, requested: Option<&str>) -> Result<Prepared, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let (sources, _notes) = custom_sources(session)?;
        let defs = session_definitions(session_game(session), &sources)?;
        Ok(Prepared { map, defs })
    })
}

/// One compact sector summary beside its index.
#[derive(Debug, Serialize)]
struct SectorRow {
    /// The index into `SECTORS`.
    index: usize,
    /// The floor height in map units.
    floor: i32,
    /// The ceiling height in map units.
    ceiling: i32,
    /// The light level as stored.
    light: i32,
    /// The floor flat name.
    floor_flat: String,
    /// The ceiling flat name.
    ceiling_flat: String,
    /// The sector effect number.
    special: i32,
    /// The sector tag.
    tag: i32,
}

/// One compact linedef summary beside its index.
#[derive(Debug, Serialize)]
struct LinedefRow {
    /// The index into `LINEDEFS`.
    index: usize,
    /// The start vertex index.
    v1: usize,
    /// The end vertex index.
    v2: usize,
    /// The line special number.
    special: i32,
    /// The trigger tag (`Hexen` maps also report their `arg0`-carried tag).
    tag: i32,
    /// The sector the front side opens into; `null` when unresolvable.
    front_sector: Option<usize>,
    /// The sector the back side opens into; `null` without a back side.
    back_sector: Option<usize>,
}

/// One compact sidedef summary beside its index.
#[derive(Debug, Serialize)]
struct SidedefRow {
    /// The index into `SIDEDEFS`.
    index: usize,
    /// The sector this side opens into, as stored.
    sector: usize,
    /// The upper texture name, `"-"` for none.
    upper: String,
    /// The middle texture name, `"-"` for none.
    middle: String,
    /// The lower texture name, `"-"` for none.
    lower: String,
}

/// One compact thing summary beside its index.
#[derive(Debug, Serialize)]
struct ThingRow {
    /// The index into `THINGS`.
    index: usize,
    /// The thing type number.
    #[serde(rename = "type")]
    kind: i32,
    /// The definition display name, `Unknown (n)` when undefined.
    name: String,
    /// The `X` coordinate in map units.
    x: i32,
    /// The `Y` coordinate in map units.
    y: i32,
    /// The facing direction in degrees.
    angle: i32,
}

/// One compact vertex summary beside its index.
#[derive(Debug, Serialize)]
struct VertexRow {
    /// The index into `VERTEXES`.
    index: usize,
    /// The `X` coordinate in map units.
    x: i32,
    /// The `Y` coordinate in map units.
    y: i32,
}

/// One hit's compact summary, in the shape its kind documents.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Row {
    /// A sector hit's summary.
    Sector(SectorRow),
    /// A linedef hit's summary.
    Linedef(LinedefRow),
    /// A sidedef hit's summary.
    Sidedef(SidedefRow),
    /// A thing hit's summary.
    Thing(ThingRow),
    /// A vertex hit's summary.
    Vertex(VertexRow),
}

/// The `rows` for one kind's page of hits, in index order.
fn rows(kind: ElementKind, page: &[usize], map: &Map, ctx: &QueryContext<'_>) -> Vec<Row> {
    match kind {
        ElementKind::Sectors => page.iter().filter_map(|i| sector_row(*i, map)).collect(),
        ElementKind::Linedefs => page.iter().filter_map(|i| linedef_row(*i, map)).collect(),
        ElementKind::Sidedefs => page.iter().filter_map(|i| sidedef_row(*i, map)).collect(),
        ElementKind::Things => page
            .iter()
            .filter_map(|i| thing_row(*i, map, ctx))
            .collect(),
        ElementKind::Vertices => page.iter().filter_map(|i| vertex_row(*i, map)).collect(),
    }
}

/// The sector summary row, `None` when the index left the block.
fn sector_row(index: usize, map: &Map) -> Option<Row> {
    let row = map.sectors.get(index)?;
    Some(Row::Sector(SectorRow {
        index,
        floor: row.floor_height,
        ceiling: row.ceiling_height,
        light: row.light,
        floor_flat: row.floor_flat.clone(),
        ceiling_flat: row.ceiling_flat.clone(),
        special: row.special,
        tag: row.tag,
    }))
}

/// The linedef summary row, `None` when the index left the block.
fn linedef_row(index: usize, map: &Map) -> Option<Row> {
    let row = map.linedefs.get(index)?;
    Some(Row::Linedef(LinedefRow {
        index,
        v1: row.v1,
        v2: row.v2,
        special: row.special,
        tag: row.tag,
        front_sector: side_sector(map, row.front),
        back_sector: side_sector(map, row.back),
    }))
}

/// The sidedef summary row, `None` when the index left the block.
fn sidedef_row(index: usize, map: &Map) -> Option<Row> {
    let row = map.sidedefs.get(index)?;
    Some(Row::Sidedef(SidedefRow {
        index,
        sector: row.sector,
        upper: row.upper.clone(),
        middle: row.middle.clone(),
        lower: row.lower.clone(),
    }))
}

/// The thing summary row, `None` when the index left the block.
fn thing_row(index: usize, map: &Map, ctx: &QueryContext<'_>) -> Option<Row> {
    let row = map.things.get(index)?;
    Some(Row::Thing(ThingRow {
        index,
        kind: row.kind,
        name: ctx.thing_name(row.kind),
        x: row.x,
        y: row.y,
        angle: row.angle,
    }))
}

/// The vertex summary row, `None` when the index left the block.
fn vertex_row(index: usize, map: &Map) -> Option<Row> {
    let row = map.vertices.get(index)?;
    Some(Row::Vertex(VertexRow {
        index,
        x: row.x,
        y: row.y,
    }))
}

/// Structured result of [`Cyberdemon::find_elements`].
#[derive(Debug, Serialize)]
struct FindReport {
    /// The queried kind, as the wire name.
    kind: &'static str,
    /// How many elements matched in total, before the paging.
    count: usize,
    /// This page of matching indices, ascending.
    indices: Vec<usize>,
    /// The compact summaries of [`Self::indices`], in the same order.
    rows: Vec<Row>,
    /// The `offset` that returns the hits this page left behind, `null`
    /// when the page took everything.
    next_offset: Option<usize>,
}

/// The `offset`/`limit` window over the ascending hit list: this page and
/// the `next_offset` that continues it (`None` when nothing stayed behind).
fn page_hits(hits: &[usize], offset: usize, limit: usize) -> (Vec<usize>, Option<usize>) {
    let start = offset.min(hits.len());
    let stop = start.saturating_add(limit).min(hits.len());
    let page = hits.get(start..stop).unwrap_or_default().to_vec();
    let next = (stop < hits.len()).then(|| offset.saturating_add(page.len()));
    (page, next)
}

/// `find_elements`' work: check the limit before the session is touched,
/// read the map and the definitions under one lock, run the pure query and
/// page its hits.
///
/// # Errors
///
/// [`parse_limit`]'s [`CyberError::InvalidArgument`], [`load`]'s errors and
/// [`run_query`]'s refusals for a bad field, op or value.
fn find_run(state: &AppState, args: &FindElementsParams) -> Result<FindReport, CyberError> {
    let limit = parse_limit(args.limit)?;
    let prepared = load(state, args.map.as_deref())?;
    let query = Query {
        kind: args.kind,
        r#where: args.r#where.clone(),
        within: args.within.clone(),
    };
    let ctx = QueryContext::new(prepared.map.flag_family(), &prepared.defs);
    let hits = run_query(&prepared.map, &ctx, &query)?;
    let (page, next_offset) = page_hits(&hits, args.offset.unwrap_or(0), limit);
    Ok(FindReport {
        kind: args.kind.as_str(),
        count: hits.len(),
        indices: page.clone(),
        rows: rows(args.kind, &page, &prepared.map, &ctx),
        next_offset,
    })
}

/// Arguments for [`Cyberdemon::replace_texture`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ReplaceTextureParams {
    /// The case-insensitive glob (`*` any run, `?` one character) of
    /// texture names to replace; `"WALL1"` names one texture, `"WALL*"`
    /// sweeps its family and `"*"` takes every face, `"-"` and the empty
    /// ones included.
    from: String,
    /// The texture every matched face gets: a wall texture the merged
    /// view holds (`list_textures` shows them; uppercased) or `"-"`,
    /// which clears the face. An unknown name fails `not_found`.
    to: String,
    /// The sidedef faces to rewrite: any of `"upper"`, `"middle"`,
    /// `"lower"` (case-insensitive). Absent or empty means all three.
    #[serde(default)]
    parts: Option<Vec<String>>,
    /// The map blocks to edit: absent takes the session's default map
    /// (the active map, else the merged view's only map), `"all"` every
    /// distinct block of the merged view, a list exactly those names.
    /// A map a read-only layer holds is copied into the working archive
    /// first, inside the same undo step.
    #[serde(default)]
    maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    dry_run: Option<bool>,
}

/// Arguments for [`Cyberdemon::replace_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ReplaceFlatParams {
    /// The case-insensitive glob of flat names to replace, matched
    /// against every targeted sector surface.
    from: String,
    /// The flat every matched surface gets: a flat the merged view holds
    /// (`list_flats` shows them, `F_SKY1` included; uppercased). An
    /// unknown name fails `not_found`; unlike a wall swap there is no
    /// clearing `"-"` for a surface.
    to: String,
    /// The sector surfaces to rewrite: any of `"floor"`, `"ceiling"`
    /// (case-insensitive). Absent or empty means both.
    #[serde(default)]
    surfaces: Option<Vec<String>>,
    /// The map blocks to edit, exactly as in [`ReplaceTextureParams::maps`].
    #[serde(default)]
    maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    dry_run: Option<bool>,
}

/// `replace_texture`'s work: parse the face list, check the target
/// against the merged view's wall stock (the clearing `"-"` excepted)
/// and run [`replace_textures`] over the selected maps as one step.
///
/// # Errors
///
/// [`parse_terms`]'s and [`require_target`]'s refusals for a stray face
/// or target name, [`Stock`]'s `not_found` for a texture that does not
/// exist and [`run_replacement`]'s for a map no scope holds.
fn texture_run(
    session: &mut Session,
    args: &ReplaceTextureParams,
) -> Result<ReplaceReport, CyberError> {
    let faces = parse_terms(args.parts.as_deref(), Face::parse)?;
    let to = require_target(session, &args.to, true, Stock::require_wall)?;
    let label = format!("replace_texture {} -> {to}", args.from.to_ascii_uppercase());
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let from = args.from.as_str();
    run_replacement(session, &replacement, |map| {
        replace_textures(map, from, &to, &faces)
    })
}

/// `replace_flat`'s work: parse the surface list, check the target
/// against the merged view's flats and run [`replace_flats`] over the
/// selected maps as one step.
///
/// # Errors
///
/// [`parse_terms`]'s and [`require_target`]'s refusals for a stray
/// surface or flat name and [`run_replacement`]'s for a map no scope
/// holds.
fn flat_run(session: &mut Session, args: &ReplaceFlatParams) -> Result<ReplaceReport, CyberError> {
    let surfaces = parse_terms(args.surfaces.as_deref(), Surface::parse)?;
    let to = require_target(session, &args.to, false, Stock::require_flat)?;
    let label = format!("replace_flat {} -> {to}", args.from.to_ascii_uppercase());
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let from = args.from.as_str();
    run_replacement(session, &replacement, |map| {
        replace_flats(map, from, &to, &surfaces)
    })
}

/// Arguments for [`Cyberdemon::replace_thing_type`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ReplaceThingTypeParams {
    /// The things to rewrite: the doomednum as stored, or a definition
    /// name (`get_thing_types` shows them; an exact case-insensitive
    /// name wins, else a unique case-insensitive substring). A name
    /// nothing holds or several types share fails with `invalid_argument`
    /// naming `from`.
    from: ThingRef,
    /// What every matched thing gets: a doomednum or a definition name,
    /// resolved exactly as [`Self::from`]. The resolved number must
    /// exist in the session's definitions unless `allow_unknown` is set.
    to: ThingRef,
    /// The map blocks to edit, exactly as in [`ReplaceTextureParams::maps`].
    #[serde(default)]
    maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    dry_run: Option<bool>,
    /// Write a `to` number the session does not define without the
    /// existence check, and report it as-is (default `false`). The
    /// number must still fit each targeted map's format.
    #[serde(default)]
    allow_unknown: Option<bool>,
}

/// Structured result of [`Cyberdemon::replace_thing_type`]: the shared
/// [`ReplaceReport`] fold beside the two resolved numbers.
#[derive(Debug, Serialize)]
struct ThingSwapReport {
    /// The resolved thing type the call rewrote.
    from: i32,
    /// The resolved thing type the call wrote.
    to: i32,
    /// The glob swaps' shared fields: `maps`, `total`, `copied_maps`,
    /// `dry_run` and `next_steps`.
    #[serde(flatten)]
    swap: ReplaceReport,
}

/// [`resolve_thing_ref`]'s answer with an `invalid_argument` refusal
/// attributed to the `parameter` that carried the bad reference.
fn resolve_side(session: &Session, value: &ThingRef, parameter: &str) -> Result<i32, CyberError> {
    match resolve_thing_ref(session, value) {
        Err(CyberError::InvalidArgument(problem)) => Err(CyberError::InvalidArgument(format!(
            "{parameter}: {problem}"
        ))),
        answered => answered,
    }
}

/// `replace_thing_type`'s work: resolve both sides (each failure naming
/// its parameter), refuse an undefined `to` unless `allow_unknown` lets
/// it through, and run [`replace_thing_types`] over the selected maps
/// as one step.
///
/// # Errors
///
/// [`resolve_side`]'s and [`require_defined_thing`]'s
/// [`CyberError::InvalidArgument`] refusals and [`run_replacement`]'s
/// for a map no scope holds.
fn thing_type_run(
    session: &mut Session,
    args: &ReplaceThingTypeParams,
) -> Result<ThingSwapReport, CyberError> {
    let from = resolve_side(session, &args.from, "from")?;
    let to = resolve_side(session, &args.to, "to")?;
    if !args.allow_unknown.unwrap_or(false) {
        require_defined_thing(&thing_definitions(session)?, to)?;
    }
    let label = format!("replace_thing_type {from} -> {to}");
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let swap = run_replacement(session, &replacement, |map| {
        replace_thing_types(map, from, to)
    })?;
    Ok(ThingSwapReport { from, to, swap })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = query_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Runs a query over one map and reports the matching element indices.
    #[tool(
        description = "Answers \"which elements?\" as a set instead of a scan: runs a {kind, where, within} query over one map and returns the ascending indices of every match. kind is \"sectors\", \"linedefs\", \"sidedefs\", \"things\" or \"vertices\"; where is a list of {field, op, value} filters combined with AND (empty keeps everything); within is an optional {\"bbox\": [min_x, min_y, max_x, max_y]} (inclusive) or {\"sectors\": [index, ...]} bound. Fields: sectors index, floor, ceiling, height, light, floor_flat, ceiling_flat, special, tag, area, roles; linedefs index, v1, v2, flags, special, tag (Doom family) or arg0..arg4 (Hexen family), front_sector, back_sector, two_sided, length, front_upper/front_middle/front_lower, back_upper/back_middle/back_lower; sidedefs index, sector, upper, middle, lower, x_offset, y_offset, linedef; things index, type, x, y, angle, flags, name, category, sector, plus tid and special in the Hexen family; vertices index, x, y. Ops: numbers take eq ne lt le gt ge in not_in with integer values (in/not_in with arrays); names and flats take eq ne in not_in and glob (case-insensitive * and ?); two_sided takes eq ne in not_in with booleans; flags takes has/has_not with a flag name from the map's family tables (blocking, blockmonsters, twosided, secret, mapped, ambush, easy, playeruse, ...); sector roles take has/has_not with a describe_map role (door, lift, teleport_destination, secret, damaging, light_effect, exit_room, stair_step, sky). Derived fields (height, area, roles, sides, two_sided, length, face textures, sidedef linedef, thing name/category/sector) that do not resolve match nothing, ne and has_not included. within: bbox keeps things and vertices at points inside the box, sectors whose label centre is inside it, and linedefs with both vertices inside it (sidedefs follow their owning line); sectors keeps sectors by index, things and vertices standing in one, and linedefs and sidedefs with a side in one. map works as in the other map tools (merged view); offset (default 0) and limit (default 200, 1..=1000) page the hits. Returns {kind, count, indices, rows, next_offset}: count totals every hit, rows carries a compact summary per returned index (sectors {index, floor, ceiling, light, floor_flat, ceiling_flat, special, tag}; linedefs {index, v1, v2, special, tag, front_sector, back_sector}; sidedefs {index, sector, upper, middle, lower}; things {index, type, name, x, y, angle}; vertices {index, x, y}); next_offset names where the rest continues, null when the page took everything. Unknown map is not_found; a bad field, op or value is invalid_argument naming the valid options. Feed the indices to the edit tools, or read the picture first with describe_map."
    )]
    fn find_elements(
        &self,
        Parameters(args): Parameters<FindElementsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match find_run(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Swaps wall texture names across whole maps in one undoable step.
    #[tool(
        description = "Renames wall textures across maps in ONE undoable step: rewrites every sidedef face whose texture the case-insensitive glob from matches to to; faces that already hold to are left alone and stay out of the counts. parts selects the faces to touch, any of \"upper\", \"middle\", \"lower\" (case-insensitive; default all three; a stray name is invalid_argument listing the three). to is validated as a texture name (uppercased) and must exist in the merged view - list_textures shows the composites, build_texture and the import tools add more - or be \"-\", which clears the matched faces; an unknown to is not_found. maps defaults to the active map (set_active_map, else the merged view's only map), \"all\" takes every distinct map block of the merged view, and a list of names takes exactly those (an unknown name is not_found). A map a read-only layer (IWAD or resource) holds is copied into the working archive before it is rewritten (copy-on-write): the copies, the rewrites and the label sit in ONE undo entry, and any refusal leaves the session exactly as it was. dry_run: true (default false) reports the same counts while writing nothing and recording nothing - run it first for a big or wildcard swap. Returns {maps: {block name: changed face fields}, total (the sum over the maps), copied_maps (the maps that had to be copied in; empty for a dry run), dry_run, next_steps}; one undo takes the whole swap across every map back. Find the faces first with find_elements (kind \"sidedefs\", fields upper, middle, lower); see skill/reference/query.md."
    )]
    fn replace_texture(
        &self,
        Parameters(args): Parameters<ReplaceTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .state()
            .with_session(|session| texture_run(session, &args))
        {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Swaps flat names across whole maps in one undoable step.
    #[tool(
        description = "Renames floor and ceiling flats across maps in ONE undoable step: rewrites every sector surface whose flat the case-insensitive glob from matches to to; surfaces that already hold to are left alone and stay out of the counts. surfaces selects the faces to touch, any of \"floor\", \"ceiling\" (case-insensitive; default both; a stray name is invalid_argument listing the two). to is validated as a flat name (uppercased) and must exist in the merged view's flats namespace - list_flats shows the flats, import_image_as_flat adds one, F_SKY1 is always available; an unknown to is not_found, and there is no clearing \"-\" for a surface. maps defaults to the active map (set_active_map, else the merged view's only map), \"all\" takes every distinct map block of the merged view, and a list of names takes exactly those (an unknown name is not_found). A map a read-only layer (IWAD or resource) holds is copied into the working archive before it is rewritten (copy-on-write): the copies, the rewrites and the label sit in ONE undo entry, and any refusal leaves the session exactly as it was. dry_run: true (default false) reports the same counts while writing nothing and recording nothing - run it first for a big or wildcard swap. Returns {maps: {block name: changed flat fields}, total (the sum over the maps), copied_maps (the maps that had to be copied in; empty for a dry run), dry_run, next_steps}; heights, lights, specials and tags stay exactly as they were, and one undo takes the whole swap back. Find the sectors first with find_elements (kind \"sectors\", fields floor_flat, ceiling_flat) and read the result back with describe_map; see skill/reference/query.md."
    )]
    fn replace_flat(
        &self,
        Parameters(args): Parameters<ReplaceFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .state()
            .with_session(|session| flat_run(session, &args))
        {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Rewrites thing type numbers across whole maps in one undoable step.
    #[tool(
        description = "Changes thing types across maps in ONE undoable step: rewrites every thing whose type number equals the resolved from to the resolved to; things that already hold to stay out of the counts. Both sides take a number (the doomednum as stored in THINGS) or a definition name (e.g. \"imp\"): names resolve through the session's definitions (custom DEHACKED/DECORATE/ZScript included) - an exact case-insensitive name wins, else a unique case-insensitive substring; a name nothing holds or several distinct types share is invalid_argument listing the candidates and naming which parameter (from or to) failed. The resolved to must exist in the session's definitions (get_thing_types lists the numbers and names) or the call refuses with invalid_argument, naming get_thing_types and the escape hatch: allow_unknown: true skips the check and reports the written number as-is. An undefined from needs no escape - it simply matches nothing, and an allow_unknown to must still fit the target map's format (binary Doom stores thing types as u16; a larger number refuses with limit_exceeded, nothing written). Positions, facings, presence flags and the Hexen-family tid, special and args come out byte-identical: only the type number moves. maps defaults to the active map (set_active_map, else the merged view's only map), \"all\" takes every distinct map block of the merged view, and a list of names takes exactly those (an unknown name is not_found naming list_maps). A map a read-only layer (IWAD or resource) holds is copied into the working archive before it is rewritten (copy-on-write): the copies, the rewrites and the label replace_thing_type <FROM> -> <TO> sit in ONE undo entry, and any refusal leaves the session exactly as it was. dry_run: true (default false) reports the same counts while writing nothing and recording nothing - run it first for a wide swap. Returns {maps: {block name: changed things}, total (the sum over the maps), copied_maps (the maps that had to be copied in; empty for a dry run), from (the resolved doomednum), to (the resolved doomednum), dry_run, next_steps}; one undo restores every type in one step. Count the targets first with find_elements (kind \"things\", field type or a name glob) and read positions back through assemble_map; see skill/reference/query.md."
    )]
    fn replace_thing_type(
        &self,
        Parameters(args): Parameters<ReplaceThingTypeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .state()
            .with_session(|session| thing_type_run(session, &args))
        {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        DEFAULT_LIMIT, FindElementsParams, MAX_LIMIT, ReplaceFlatParams, ReplaceTextureParams,
        ReplaceThingTypeParams, Row, ThingRef, find_run, flat_run, page_hits, parse_limit, rows,
        texture_run, thing_type_run,
    };
    use crate::defs::definitions_for;
    use crate::session::{Layer, LayerRole, Session};
    use crate::state::AppState;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::game::Game;
    use crate::wad::map::query::{ElementKind, Filter, QueryContext};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Thing, Vertex, write_map};
    use crate::wad::name::LumpName;
    /// A parsed test map name.
    fn parsed(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `find_elements` call over the fixture map with this query.
    fn params(kind: ElementKind, filters: Vec<Filter>) -> FindElementsParams {
        FindElementsParams {
            kind,
            r#where: filters,
            within: None,
            map: Some("MAP01".to_owned()),
            offset: None,
            limit: None,
        }
    }

    /// A one-line, one-side, one-sector map with a start and two vertices.
    fn mini_map() -> Map {
        Map {
            name: parsed("MAP01"),
            format: MapFormat::Doom,
            other: Vec::new(),
            things: vec![Thing {
                kind: 1_i32,
                x: 64_i32,
                y: 128_i32,
                angle: 90_i32,
                flags: 7_u32,
                ..Default::default()
            }],
            vertices: vec![
                Vertex {
                    x: 0_i32,
                    y: 0_i32,
                    ..Default::default()
                },
                Vertex {
                    x: 128_i32,
                    y: 0_i32,
                    ..Default::default()
                },
            ],
            linedefs: vec![Linedef {
                v1: 0_usize,
                v2: 1_usize,
                flags: 1_u32,
                special: 10_i32,
                tag: 3_i32,
                front: Some(0_usize),
                ..Default::default()
            }],
            sidedefs: vec![Sidedef {
                middle: "STARTAN".to_owned(),
                sector: 0_usize,
                ..Default::default()
            }],
            sectors: vec![Sector {
                floor_height: 0_i32,
                ceiling_height: 128_i32,
                floor_flat: "FLOOR1".to_owned(),
                ceiling_flat: "F_SKY1".to_owned(),
                light: 170_i32,
                special: 9_i32,
                tag: 3_i32,
                ..Default::default()
            }],
            ..Map::default()
        }
    }

    #[test]
    fn parse_limit_pages_between_one_and_a_thousand() {
        assert_eq!(parse_limit(None).expect("absent"), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some(1)).expect("one"), 1);
        assert_eq!(parse_limit(Some(MAX_LIMIT)).expect("max"), MAX_LIMIT);
        for bad in [0, MAX_LIMIT.checked_add(1).expect("small")] {
            let error = parse_limit(Some(bad)).expect_err("out of range");
            assert_eq!(error.code(), "invalid_argument", "bad limit code");
            assert!(
                error.to_string().contains("between 1 and 1000"),
                "the message names the range: {error}"
            );
        }
    }

    #[test]
    fn page_hits_windows_the_list_and_names_the_rest() {
        let hits: Vec<usize> = (0..6).collect();
        let (first, continues) = page_hits(&hits, 0, 2);
        assert_eq!(first, vec![0_usize, 1], "the leading page");
        assert_eq!(continues, Some(2), "the rest continues at 2");
        let (middle, more) = page_hits(&hits, 2, 2);
        assert_eq!(middle, vec![2_usize, 3], "the middle page");
        assert_eq!(more, Some(4), "and one page more");
        let (last, rest) = page_hits(&hits, 4, 2);
        assert_eq!(last, vec![4_usize, 5], "the final page");
        assert_eq!(rest, None, "nothing stayed behind");
        let (past, beyond) = page_hits(&hits, 9, 2);
        assert!(
            past.is_empty(),
            "an offset past the end is empty, not an error"
        );
        assert_eq!(beyond, None, "and has no continuation");
        let (all, tail) = page_hits(&hits, 1, 100);
        assert_eq!(
            all,
            vec![1_usize, 2, 3, 4, 5],
            "a generous page takes the rest"
        );
        assert_eq!(tail, None, "leaving nothing to continue at");
    }

    #[test]
    fn rows_summarize_each_kind_in_its_documented_shape() {
        let map = mini_map();
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        let ctx = QueryContext::new(map.flag_family(), &defs);
        let sector = rows(ElementKind::Sectors, &[0_usize], &map, &ctx)
            .into_iter()
            .next()
            .expect("one row");
        assert_eq!(
            serde_json::to_value(&sector).expect("serializable"),
            json!({
                "index": 0_i64, "floor": 0_i64, "ceiling": 128_i64, "light": 170_i64,
                "floor_flat": "FLOOR1", "ceiling_flat": "F_SKY1",
                "special": 9_i64, "tag": 3_i64,
            }),
            "the sector shape"
        );
        let line = rows(ElementKind::Linedefs, &[0_usize], &map, &ctx)
            .into_iter()
            .next()
            .expect("one row");
        assert!(matches!(line, Row::Linedef(..)), "the linedef variant");
        assert_eq!(
            serde_json::to_value(&line).expect("serializable"),
            json!({
                "index": 0_i64, "v1": 0_i64, "v2": 1_i64, "special": 10_i64, "tag": 3_i64,
                "front_sector": 0_i64, "back_sector": null,
            }),
            "the linedef shape with a resolved front and no back"
        );
        let thing = rows(ElementKind::Things, &[0_usize], &map, &ctx)
            .into_iter()
            .next()
            .expect("one row");
        assert_eq!(
            serde_json::to_value(&thing).expect("serializable"),
            json!({
                "index": 0_i64, "type": 1_i64, "name": "Player 1 Start",
                "x": 64_i64, "y": 128_i64, "angle": 90_i64,
            }),
            "the thing shape, named through the definitions"
        );
        let side = rows(ElementKind::Sidedefs, &[0_usize], &map, &ctx)
            .into_iter()
            .next()
            .expect("one row");
        assert_eq!(
            serde_json::to_value(&side).expect("serializable"),
            json!({"index": 0_i64, "sector": 0_i64, "upper": "", "middle": "STARTAN", "lower": ""}),
            "the sidedef shape"
        );
        let vertex = rows(ElementKind::Vertices, &[0_usize, 1_usize], &map, &ctx);
        assert_eq!(vertex.len(), 2, "both vertices");
        assert_eq!(
            serde_json::to_value(&vertex[1_usize]).expect("serializable"),
            json!({"index": 1_i64, "x": 128_i64, "y": 0_i64}),
            "the vertex shape"
        );
    }

    #[test]
    fn rows_drop_indices_that_left_their_block() {
        let map = mini_map();
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        let ctx = QueryContext::new(map.flag_family(), &defs);
        assert!(
            rows(ElementKind::Sectors, &[0_usize, 42_usize], &map, &ctx).len() == 1,
            "the stale index contributes nothing"
        );
    }

    #[test]
    fn find_run_reports_parameter_and_map_errors() {
        let state = AppState::default();
        let mut bad_limit = params(ElementKind::Sectors, vec![]);
        bad_limit.limit = Some(0);
        assert_eq!(
            find_run(&state, &bad_limit).expect_err("zero rows").code(),
            "invalid_argument",
            "the limit is checked before the session is touched"
        );
        let no_map = params(ElementKind::Things, vec![]);
        assert_eq!(
            find_run(&state, &no_map)
                .expect_err("nothing is open")
                .code(),
            "not_found",
            "an explicit unknown map over an empty session is not_found"
        );
        let mut anonymous = no_map;
        anonymous.map = None;
        assert_eq!(
            find_run(&state, &anonymous)
                .expect_err("no map default exists")
                .code(),
            "invalid_argument",
            "the shared resolver names the no-map problem"
        );
    }

    /// An eight-byte dummy lump named `text`.
    fn dummy(text: &str) -> Lump {
        Lump::new(parsed(text), vec![0xA5_u8; 8])
    }

    /// A fully visible spawn of `kind` at `x`/`y` facing `angle`.
    fn loaded(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            angle,
            flags: 7_u32,
            ..Default::default()
        }
    }

    /// The bulk-edit fixture map: three sidedef faces named `WALL1`
    /// (two middles, one upper), three sector surfaces named `FLOOR1`
    /// (both of sector 0, sector 1's floor), a type-1 player start and
    /// one imp (3001) for the thing-type swap.
    fn bulk_map() -> Map {
        Map {
            name: parsed("MAP01"),
            format: MapFormat::Doom,
            things: vec![
                loaded(1_i32, 64_i32, 128_i32, 0_i32),
                loaded(3001_i32, -64_i32, 64_i32, 90_i32),
            ],
            vertices: vec![
                Vertex {
                    x: 0_i32,
                    y: 0_i32,
                    ..Default::default()
                },
                Vertex {
                    x: 128_i32,
                    y: 0_i32,
                    ..Default::default()
                },
            ],
            linedefs: vec![Linedef {
                v1: 0_usize,
                v2: 1_usize,
                flags: 1_u32,
                ..Default::default()
            }],
            sidedefs: vec![
                Sidedef {
                    upper: "-".to_owned(),
                    middle: "WALL1".to_owned(),
                    lower: "-".to_owned(),
                    sector: 0_usize,
                    ..Default::default()
                },
                Sidedef {
                    upper: "WALL1".to_owned(),
                    middle: "WALL1".to_owned(),
                    lower: "-".to_owned(),
                    sector: 1_usize,
                    ..Default::default()
                },
            ],
            sectors: vec![
                Sector {
                    floor_flat: "FLOOR1".to_owned(),
                    ceiling_flat: "FLOOR1".to_owned(),
                    ..Default::default()
                },
                Sector {
                    floor_flat: "FLOOR1".to_owned(),
                    ceiling_flat: "F_SKY1".to_owned(),
                    ..Default::default()
                },
            ],
            ..Map::default()
        }
    }

    /// A session whose only layer holds [`bulk_map`] plus the stock the
    /// bulk tools check: flats `FLOOR1` and `NUKAGE1`, wall `STEP1`
    /// (named by its `TX_` lump).
    fn bulk_session() -> Session {
        let mut lumps = write_map(&bulk_map()).expect("the fixture map encodes");
        lumps.extend(vec![
            Lump::marker(parsed("F_START")),
            dummy("FLOOR1"),
            dummy("NUKAGE1"),
            Lump::marker(parsed("F_END")),
            Lump::marker(parsed("TX_START")),
            dummy("TX_STEP1"),
            Lump::marker(parsed("TX_END")),
        ]);
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("bulk.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ));
        session
    }

    /// `replace_texture` arguments over the fixture's default map.
    fn texture_params(from: &str, to: &str) -> ReplaceTextureParams {
        ReplaceTextureParams {
            from: from.to_owned(),
            to: to.to_owned(),
            parts: None,
            maps: None,
            dry_run: None,
        }
    }

    /// `replace_flat` arguments over the fixture's default map.
    fn flat_params(from: &str, to: &str) -> ReplaceFlatParams {
        ReplaceFlatParams {
            from: from.to_owned(),
            to: to.to_owned(),
            surfaces: None,
            maps: None,
            dry_run: None,
        }
    }

    #[test]
    fn texture_run_swaps_faces_as_one_labeled_step() {
        let mut session = bulk_session();
        let mut args = texture_params("wall1", "step1");
        args.dry_run = Some(true);
        let preview = texture_run(&mut session, &args).expect("the preview runs");
        assert_eq!(preview.total, 3_usize, "two middles and one upper");
        assert!(
            session.undo_labels().is_empty(),
            "a preview records nothing"
        );
        args.dry_run = None;
        let report = texture_run(&mut session, &args).expect("the swap runs");
        let shown = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(
            shown["maps"],
            json!({"MAP01": 3_u64}),
            "the three faces folded onto the block name"
        );
        assert_eq!(shown["total"], json!(3_u64), "the sum");
        assert_eq!(
            shown["copied_maps"],
            json!(["MAP01"]),
            "the map came from the IWAD layer"
        );
        assert_eq!(shown["dry_run"], json!(false), "the swap wrote");
        assert!(
            !report.next_steps.is_empty(),
            "the report names a next step"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_texture WALL1 -> STEP1"],
            "one step, uppercase label from the folded names"
        );
    }

    #[test]
    fn texture_run_narrows_to_the_listed_faces_and_clears_with_the_dash() {
        let mut middles = texture_params("WALL1", "STEP1");
        middles.parts = Some(vec!["middle".to_owned()]);
        let mut session = bulk_session();
        let report = texture_run(&mut session, &middles).expect("one face listed");
        assert_eq!(report.total, 2_usize, "only the two middles");
        let mut clear = texture_params("WALL1", "-");
        clear.parts = Some(vec!["UPPER".to_owned()]);
        let mut blank = bulk_session();
        let cleared = texture_run(&mut blank, &clear).expect("\"-\" clears without a stock check");
        assert_eq!(cleared.total, 1_usize, "only the upper, emptied");
    }

    #[test]
    fn texture_run_refuses_stray_faces_and_unknown_targets() {
        let mut session = bulk_session();
        let mut stray = texture_params("WALL1", "STEP1");
        stray.parts = Some(vec!["roof".to_owned()]);
        let bad_face = texture_run(&mut session, &stray).expect_err("roof is no face");
        assert_eq!(bad_face.code(), "invalid_argument", "stray face code");
        let empty = texture_run(&mut session, &texture_params("WALL1", ""))
            .expect_err("an empty target is refused");
        assert_eq!(empty.code(), "invalid_argument", "empty target code");
        let missing = texture_run(&mut session, &texture_params("WALL1", "NOSUCH"))
            .expect_err("no such wall");
        assert_eq!(missing.code(), "not_found", "missing target code");
        assert!(
            missing.to_string().contains("list_textures"),
            "the fix is named: {missing}"
        );
        assert!(
            session.undo_labels().is_empty(),
            "refusals write nothing and record nothing"
        );
    }

    #[test]
    fn flat_run_swaps_surfaces_and_refuses_the_documented_strays() {
        let mut session = bulk_session();
        let mut floors = flat_params("floor1", "nukage1");
        floors.surfaces = Some(vec!["floor".to_owned()]);
        let report = flat_run(&mut session, &floors).expect("the swap runs");
        assert_eq!(report.total, 2_usize, "the two floors, ceilings untouched");
        assert_eq!(
            session.undo_labels(),
            vec!["replace_flat FLOOR1 -> NUKAGE1"],
            "one labeled step"
        );
        let mut blank = bulk_session();
        let all = flat_run(&mut blank, &flat_params("FLOOR1", "NUKAGE1")).expect("both surfaces");
        assert_eq!(all.total, 3_usize, "F_SKY1 is not FLOOR1 and stays");
        let stray = flat_run(&mut blank, &flat_params("FLOOR1", "NUKAGE1"))
            .expect("the swap runs again, changing nothing");
        assert_eq!(stray.total, 0_usize, "already swapped");
        let mut bad_surface = flat_params("FLOOR1", "NUKAGE1");
        bad_surface.surfaces = Some(vec!["roof".to_owned()]);
        let error = flat_run(&mut blank, &bad_surface).expect_err("roof is no surface");
        assert_eq!(error.code(), "invalid_argument", "stray surface code");
        assert!(
            error.to_string().contains("ceiling"),
            "the message names the two: {error}"
        );
        let dash = flat_run(&mut blank, &flat_params("FLOOR1", "-")).expect_err("no dash flat");
        assert_eq!(
            dash.code(),
            "not_found",
            "a flat \"-\" must exist and none does"
        );
        let missing =
            flat_run(&mut blank, &flat_params("FLOOR1", "NOSUCH")).expect_err("no such flat");
        assert_eq!(missing.code(), "not_found", "missing flat code");
    }

    #[test]
    fn flat_run_previews_without_writing_or_recording() {
        let mut session = bulk_session();
        let mut preview = flat_params("FLOOR1", "NUKAGE1");
        preview.dry_run = Some(true);
        let report = flat_run(&mut session, &preview).expect("the preview runs");
        let shown = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(shown["maps"], json!({"MAP01": 3_u64}), "counts reported");
        assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(shown["copied_maps"], json!([]), "nothing copied");
        assert!(session.working().is_empty(), "the working archive stands");
        assert!(session.undo_labels().is_empty(), "no history entry");
    }

    /// `replace_thing_type` arguments over the fixture's default map.
    fn thing_params(from: ThingRef, to: ThingRef) -> ReplaceThingTypeParams {
        ReplaceThingTypeParams {
            from,
            to,
            maps: None,
            dry_run: None,
            allow_unknown: None,
        }
    }

    /// The doomednum variant of the test [`ThingRef`].
    fn number(kind: u32) -> ThingRef {
        ThingRef::Number(kind)
    }

    /// The name variant of the test [`ThingRef`].
    fn name(text: &str) -> ThingRef {
        ThingRef::Name(text.to_owned())
    }

    #[test]
    fn thing_type_run_labels_the_step_with_the_resolved_numbers() {
        let mut session = bulk_session();
        let mut preview = thing_params(name("imp"), name("demon"));
        preview.dry_run = Some(true);
        let previewed = thing_type_run(&mut session, &preview).expect("the preview runs");
        let shown = serde_json::to_value(&previewed).expect("the report serializes");
        assert_eq!(shown["maps"], json!({"MAP01": 1_u64}), "counts reported");
        assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(shown["copied_maps"], json!([]), "a preview copies nothing");
        assert_eq!(shown["from"], json!(3001_u64), "the resolved doomednum");
        assert_eq!(shown["to"], json!(3002_u64), "and the resolved target");
        assert!(
            session.undo_labels().is_empty(),
            "the preview records nothing"
        );
        let report = thing_type_run(&mut session, &thing_params(name("Imp"), number(3002_u32)))
            .expect("the swap runs");
        let written = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(written["total"], json!(1_u64), "the fixture imp moved");
        assert_eq!(
            report.swap.copied_maps,
            vec!["MAP01".to_owned()],
            "copied in"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_thing_type 3001 -> 3002"],
            "one labeled step with the resolved numbers"
        );
    }

    #[test]
    fn thing_type_run_names_the_failing_parameter_on_both_sides() {
        let mut session = bulk_session();
        let bad_from = thing_type_run(&mut session, &thing_params(name("platypus"), name("imp")))
            .expect_err("no thing named platypus");
        assert_eq!(bad_from.code(), "invalid_argument", "unknown from code");
        assert!(
            bad_from.to_string().starts_with("from:"),
            "the refusal names from: {bad_from}"
        );
        let bad_to = thing_type_run(&mut session, &thing_params(name("imp"), name("emon")))
            .expect_err("several types share that substring");
        assert_eq!(bad_to.code(), "invalid_argument", "ambiguous to code");
        assert!(
            bad_to.to_string().starts_with("to:"),
            "the refusal names to: {bad_to}"
        );
        assert!(
            session.undo_labels().is_empty(),
            "refusals write nothing and record nothing"
        );
    }

    /// An undefined doomednum the binary `Doom` records can still store.
    const UNDEFINED_KIND: u32 = 9999_u32;

    #[test]
    fn thing_type_run_checks_the_target_and_allow_unknown_escapes_it() {
        let mut session = bulk_session();
        let refused = thing_type_run(
            &mut session,
            &thing_params(number(3001_u32), number(UNDEFINED_KIND)),
        )
        .expect_err("9999 is undefined");
        assert_eq!(refused.code(), "invalid_argument", "undefined target code");
        let message = refused.to_string();
        assert!(
            message.contains("get_thing_types") && message.contains("allow_unknown"),
            "the reference and the escape hatch are named: {message}"
        );
        assert!(
            session.undo_labels().is_empty(),
            "the refusal records nothing"
        );
        let mut allowed = thing_params(number(3001_u32), number(UNDEFINED_KIND));
        allowed.allow_unknown = Some(true);
        let report = thing_type_run(&mut session, &allowed).expect("the escape admits it");
        let shown = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(
            shown["to"],
            json!(i64::from(UNDEFINED_KIND)),
            "the written number is reported as-is"
        );
        assert_eq!(
            shown["total"],
            json!(1_u64),
            "the imp took the undefined type"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_thing_type 3001 -> 9999"],
            "the escape hatch writes like any other run"
        );
    }
}
