//! Query tools: `find_elements`, the set-valued read behind every
//! plain-English "all the X" request, CD-108's bulk `replace_*` swaps and
//! the `bulk_edit` field editor those reads drive.
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
//! `next_offset` names the `offset` where the rest continues. `bulk_edit`
//! takes the very same selection and hands its indices to the pure
//! [`crate::wad::map::bulk::edits`] engine as one `Session::edit_map`
//! step, with a `dry_run` preview and a `max` guard over the matched set.
//! The bulk
//! edits act on exactly the elements `find_elements` reports; the
//! session-facing machinery they share (the `maps` selector, the
//! one-undo-step-over-many-maps runner, the texture stock check and the
//! folded [`replace::ReplaceReport`]) lives in the public `replace`
//! submodule; the pure per-map passes are
//! [`crate::wad::map::bulk::replace_textures`] and its siblings.

pub mod replace;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::plan::things::resolve_thing_kind;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::tools::query::replace::{
    MapsSelector, ReplaceReport, Replacement, Stock, ThingRef, parse_terms, require_defined_thing,
    require_target, resolve_thing_ref, run_replacement, thing_definitions,
};
use crate::tools::{fail, ok};
use crate::wad::map::Map;
use crate::wad::map::bulk::edits::{Applied, Change, EditScope, FieldEdits, apply};
use crate::wad::map::bulk::{Face, Surface, replace_flats, replace_textures, replace_thing_types};
use crate::wad::map::query::{
    ElementKind, Filter, Query, QueryContext, Within, run_query, side_sector,
};
use crate::wad::name::LumpName;
/// Default row cap for the `indices` and `rows` lists.
const DEFAULT_LIMIT: usize = 200;
/// Largest row cap the lists accept.
const MAX_LIMIT: usize = 1000;
/// Default `max` for [`Cyberdemon::bulk_edit`]: more matches refuse.
const DEFAULT_MAX: usize = 5000;
/// Longest `indices` list a `bulk_edit` report carries.
const INDEX_PAGE: usize = 500;
/// Longest `preview` list a `bulk_edit` report carries.
const PREVIEW_PAGE: usize = 20;

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
        Err(error) if error.code() == "invalid_argument" => {
            Err(error.map_message(|problem| format!("{parameter}: {problem}")))
        }
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

/// Arguments for [`Cyberdemon::bulk_edit`]: the [`Query`] fields beside
/// one [`FieldEdits`] request and the run's knobs.
#[derive(Debug, Deserialize, JsonSchema)]
struct BulkEditParams {
    /// The map to edit, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The element block to edit: "sectors", "linedefs", "sidedefs",
    /// "things" or "vertices".
    kind: ElementKind,
    /// The filters selecting the elements, exactly as `find_elements`'
    /// `where`: an empty list edits everything of the kind.
    #[serde(default, rename = "where")]
    r#where: Vec<Filter>,
    /// Bound the selection by region, exactly as `find_elements`'
    /// `within`.
    #[serde(default)]
    within: Option<Within>,
    /// New values by field name: integers, texture/flat names, and for
    /// things a doomednum integer or a definition name.
    #[serde(default)]
    set: Option<BTreeMap<String, Value>>,
    /// Integer amounts shifted onto each matched element's current value.
    #[serde(default)]
    add: Option<BTreeMap<String, i64>>,
    /// Flag names to set on each matched linedef or thing.
    #[serde(default)]
    flags_set: Option<Vec<String>>,
    /// Flag names to clear on each matched linedef or thing; a bit named
    /// on both ends ends up set.
    #[serde(default)]
    flags_clear: Option<Vec<String>>,
    /// Write texture/flat names the merged view lacks without the
    /// existence check (default `false`). Numbers are never clamped
    /// either way.
    #[serde(default)]
    allow_missing: Option<bool>,
    /// Run the whole check and report the same numbers over a copy of the
    /// map, writing nothing and recording no undo step (default `false`).
    #[serde(default)]
    dry_run: Option<bool>,
    /// Refuse (and write nothing) when more than this many elements match
    /// (default 5000); `max: 0` therefore refuses any non-empty match.
    #[serde(default)]
    max: Option<usize>,
}

/// Structured result of [`Cyberdemon::bulk_edit`]: the engine's [`Applied`]
/// paged to the documented windows, beside the standard edit fragment.
#[derive(Debug, Serialize)]
struct BulkReport {
    /// Elements the query selected.
    matched: usize,
    /// Elements whose values actually changed.
    changed: usize,
    /// Ascending indices of the changed elements, at most [`INDEX_PAGE`]
    /// of them.
    indices: Vec<usize>,
    /// Whether [`Self::indices`] left changed indices out.
    truncated: bool,
    /// Whether the run only previewed.
    dry_run: bool,
    /// Up to [`PREVIEW_PAGE`] before/after change rows.
    preview: Vec<Change>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
    /// What to do or look at next.
    next_steps: Vec<&'static str>,
}

/// The `next_steps` every written `bulk_edit` report carries.
const BULK_NEXT_STEPS: [&str; 3] = [
    "undo takes the whole edit back as one step",
    "find_elements over the same query reads the new values",
    "render_topdown looks at the result",
];

/// The `next_steps` a previewed `bulk_edit` report carries.
const BULK_DRY_NEXT_STEPS: [&str; 2] = [
    "nothing was written: run again with dry_run false to apply the edit",
    "the preview rows list every element that would change, before and after",
];

/// The extra step a geometry edit's report carries.
const BULK_NODES_STEP: &str =
    "nodes_invalidated is true: run build_nodes before playing the map in vanilla Doom";

/// The tool's parameters as one engine [`FieldEdits`].
fn requested_edits(args: &BulkEditParams) -> FieldEdits {
    FieldEdits {
        set: args.set.clone().unwrap_or_default(),
        add: args.add.clone().unwrap_or_default(),
        flags_set: args.flags_set.clone().unwrap_or_default(),
        flags_clear: args.flags_clear.clone().unwrap_or_default(),
    }
}

/// The refusal for a call that asks for no field change at all.
fn nothing_to_change() -> CyberError {
    CyberError::InvalidArgument(
        "nothing to change: pass at least one of set, add, flags_set or flags_clear".to_owned(),
    )
}

/// The query engine's selection for this call, behind the `max` guard.
///
/// # Errors
///
/// [`run_query`]'s refusals for a bad field, op or value, and
/// [`CyberError::LimitExceeded`] when the hits outnumber `max`.
fn selected(
    map: &Map,
    defs: &Definitions,
    args: &BulkEditParams,
) -> Result<Vec<usize>, CyberError> {
    let ctx = QueryContext::new(map.flag_family(), defs);
    let query = Query {
        kind: args.kind,
        r#where: args.r#where.clone(),
        within: args.within.clone(),
    };
    let hits = run_query(map, &ctx, &query)?;
    let max = args.max.unwrap_or(DEFAULT_MAX);
    if hits.len() > max {
        return Err(CyberError::LimitExceeded(format!(
            "{} {} match this query, above max {max}: nothing was written, \
             dry_run included. Narrow the selection with where/within, or \
             raise max to at least {}",
            hits.len(),
            args.kind.as_str(),
            hits.len()
        )));
    }
    Ok(hits)
}

/// Fold a `type` name on a `things` request into the doomednum the engine
/// writes; integers and every other kind pass through untouched.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the candidates when the
/// session's definitions hold no thing with that name, or when one
/// substring matches several distinct types.
fn resolve_thing_type(
    mut edits: FieldEdits,
    kind: ElementKind,
    defs: &Definitions,
) -> Result<FieldEdits, CyberError> {
    if kind != ElementKind::Things {
        return Ok(edits);
    }
    let Some(text) = edits.set.get("type").and_then(Value::as_str) else {
        return Ok(edits);
    };
    let resolved = resolve_thing_kind(defs, text).map_err(CyberError::InvalidArgument)?;
    edits
        .set
        .insert("type".to_owned(), Value::from(u64::from(resolved)));
    Ok(edits)
}

/// The edit fragment a `dry_run` reports: the named map, nothing copied,
/// nothing invalidated.
fn dry_fragment(name: LumpName) -> JsonMap<String, Value> {
    [
        ("map".to_owned(), Value::String(name.to_string())),
        ("nodes_invalidated".to_owned(), Value::Bool(false)),
        ("copied_to_working".to_owned(), Value::Bool(false)),
    ]
    .into_iter()
    .collect()
}

/// The report's `next_steps`, by run mode and whether the step changed
/// geometry.
fn bulk_next_steps(dry_run: bool, edit: &JsonMap<String, Value>) -> Vec<&'static str> {
    let nodes_invalidated = edit
        .get("nodes_invalidated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut steps: Vec<&'static str> = if dry_run {
        BULK_DRY_NEXT_STEPS.to_vec()
    } else {
        BULK_NEXT_STEPS.to_vec()
    };
    if nodes_invalidated {
        steps.push(BULK_NODES_STEP);
    }
    steps
}

/// [`Applied`] folded into the tool's report, paged to the documented
/// `indices` and `preview` windows, beside the edit fragment.
fn bulk_report(applied: Applied, dry_run: bool, edit: JsonMap<String, Value>) -> BulkReport {
    let truncated = applied.indices.len() > INDEX_PAGE;
    let next_steps = bulk_next_steps(dry_run, &edit);
    BulkReport {
        matched: applied.matched,
        changed: applied.changed,
        indices: applied.indices.iter().copied().take(INDEX_PAGE).collect(),
        truncated,
        dry_run,
        preview: applied.preview.into_iter().take(PREVIEW_PAGE).collect(),
        edit,
        next_steps,
    }
}

/// `bulk_edit`'s work inside the session lock: parse the request, select
/// with the query engine behind the `max` guard, resolve thing-type names,
/// and run the pure engine either as a preview over the merged copy or as
/// one undoable `Session::edit_map` step.
///
/// # Errors
///
/// [`nothing_to_change`]'s refusal, the map resolvers' errors,
/// [`selected`]'s and [`resolve_thing_type`]'s refusals, the engine's
/// (nothing is written behind any of them, and [`Session::edit_map`]
/// rolls the whole step back on any error inside it) and [`Stock::load`]'s
/// errors.
fn bulk_in_session(session: &mut Session, args: &BulkEditParams) -> Result<BulkReport, CyberError> {
    let requested = requested_edits(args);
    if requested.is_empty() {
        return Err(nothing_to_change());
    }
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let mut map = common::load_map(session, name, Scope::Merged)?;
    let (sources, _notes) = custom_sources(session)?;
    let defs = session_definitions(session_game(session), &sources)?;
    let hits = selected(&map, &defs, args)?;
    let edits = resolve_thing_type(requested, args.kind, &defs)?;
    let stock = Stock::load(session)?;
    let scope = EditScope {
        format: map.format,
        family: map.flag_family(),
        walls: &stock.walls,
        flats: &stock.flats,
        allow_missing: args.allow_missing.unwrap_or(false),
    };
    if args.dry_run.unwrap_or(false) {
        let applied = apply(&mut map, args.kind, &hits, &edits, &scope)?;
        return Ok(bulk_report(applied, true, dry_fragment(name)));
    }
    let label = format!("bulk_edit {} ({})", args.kind.as_str(), hits.len());
    let (applied, report) = session.edit_map(name, &label, |target| {
        apply(target, args.kind, &hits, &edits, &scope)
    })?;
    Ok(bulk_report(applied, false, edit_result_fragment(&report)))
}

/// `bulk_edit`'s work: the whole request under one session lock.
///
/// # Errors
///
/// [`bulk_in_session`]'s errors and the poisoned-lock
/// [`CyberError::Internal`].
fn bulk_run(state: &AppState, args: &BulkEditParams) -> Result<BulkReport, CyberError> {
    state.with_session(|session| bulk_in_session(session, args))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = query_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Runs a query over one map and reports the matching element indices.
    #[tool(
        description = "Answers \"which elements?\" as a set instead of a scan: runs a {kind, where, within} query over one map - sectors, linedefs, sidedefs, things or vertices, ANDed {field, op, value} filters over stored and derived values (globs, flag names, sector roles), optionally bounded by bbox or sector list. Returns {count, indices, rows, next_offset}. Use it to pick indices for the edit tools, and as the selection for bulk_edit."
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

    /// Applies one field-edit set to every element of a query selection.
    #[tool(
        description = "Query-driven bulk editing: selects exactly like find_elements ({kind, where, within}) and applies set/add amounts and named flag edits to every matched element as ONE undoable step - sector heights, light, flats, special and tag; sidedef textures and offsets; linedef special, tag and args; thing type, position and angle; vertex x/y add-only. Values are range-checked before anything writes; dry_run previews and max caps the set. Prefer it over looping the edit_* tools."
    )]
    fn bulk_edit(
        &self,
        Parameters(args): Parameters<BulkEditParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match bulk_run(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Swaps wall texture names across whole maps in one undoable step.
    #[tool(
        description = "Swaps wall textures across maps in ONE undoable step: rewrites every sidedef face whose texture the case-insensitive glob from matches to the name to. maps selects the blocks (default the working archive), dry_run previews, and the report totals changes per map and names the blocks copied in. to should exist - list_textures shows names or create_texture_from_image makes one; undo takes the whole swap back."
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
        description = "Renames floor and ceiling flats across maps in ONE undoable step: every sector surface whose flat the case-insensitive glob from matches becomes to. maps selects the blocks (default the working archive), dry_run previews and the report counts per-map changes and copied blocks. list_flats shows the names; prefer it over looping edit_sector's floor_flat and ceiling_flat."
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
        description = "Changes thing types across maps in ONE undoable step: rewrites every thing whose type equals from to the type to - each given as a doomednum or a definition name resolved through get_thing_types (an unknown or ambiguous name is refused listing the candidates). maps selects the blocks, max guards the count, dry_run previews and allow_unknown accepts an undefined target."
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

    use serde_json::{Value, json};

    use super::{
        BULK_NODES_STEP, BulkEditParams, DEFAULT_LIMIT, FindElementsParams, INDEX_PAGE, MAX_LIMIT,
        PREVIEW_PAGE, ReplaceFlatParams, ReplaceTextureParams, ReplaceThingTypeParams, Row,
        ThingRef, bulk_in_session, bulk_report, bulk_run, common, find_run, flat_run, page_hits,
        parse_limit, requested_edits, rows, texture_run, thing_type_run,
    };
    use crate::defs::definitions_for;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::game::Game;
    use crate::wad::map::bulk::edits::{Applied, Change};
    use crate::wad::map::query::{ElementKind, Filter, QueryContext};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Thing, Vertex, write_map};
    use crate::wad::name::LumpName;
    use serde_json::Map as JsonMap;
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

    /// Parse `bulk_edit` arguments from wire `JSON`, defaulting `map`.
    fn bulk_args(wire: Value) -> BulkEditParams {
        let mut full = wire;
        full.as_object_mut()
            .expect("bulk arguments are an object")
            .entry("map")
            .or_insert_with(|| json!("MAP01"));
        serde_json::from_value(full).expect("test bulk arguments parse")
    }

    /// One `{field, op, value}` filter as wire `JSON`.
    fn one_filter(field: &str, op: &str, value: &Value) -> Value {
        json!({"field": field, "op": op, "value": value})
    }

    #[test]
    fn requested_edits_folds_the_wire_parts_into_the_engine_request() {
        let args = bulk_args(json!({
            "kind": "things",
            "set": {"angle": 90_i64},
            "add": {"x": 4_i64},
            "flags_set": ["ambush"],
            "flags_clear": [],
        }));
        let edits = requested_edits(&args);
        assert_eq!(
            edits.set.get("angle"),
            Some(&json!(90_i64)),
            "the set map rides"
        );
        assert_eq!(edits.add.get("x"), Some(&4_i64), "the amounts ride");
        assert_eq!(edits.flags_set, vec!["ambush".to_owned()], "the names ride");
        assert!(edits.flags_clear.is_empty(), "an empty list stays empty");
        assert!(!edits.is_empty(), "a full request is not empty");
    }

    #[test]
    fn bulk_in_session_edits_every_match_in_one_labeled_step() {
        let mut session = bulk_session();
        let report = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "sectors", "set": {"light": 96_i64}, "add": {"tag": 2_i64}})),
        )
        .expect("the edit runs");
        let shown = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(shown["matched"], json!(2_u64), "both sectors matched");
        assert_eq!(shown["changed"], json!(2_u64), "both rows moved");
        assert_eq!(shown["indices"], json!([0_u64, 1_u64]), "ascending");
        assert_eq!(shown["truncated"], json!(false), "both fit the page");
        assert_eq!(shown["dry_run"], json!(false), "the edit wrote");
        assert_eq!(shown["map"], json!("MAP01"), "the fragment names the block");
        assert_eq!(
            shown["nodes_invalidated"],
            json!(false),
            "lights and tags are properties"
        );
        assert_eq!(
            shown["copied_to_working"],
            json!(true),
            "the map came from the layer"
        );
        let row = shown["preview"].get(0).expect("a preview row");
        assert_eq!(
            row["after"],
            json!({"light": 96_i64, "tag": 2_i64}),
            "set and add ride one pass: {row}"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["bulk_edit sectors (2)"],
            "one labeled step"
        );
        assert!(
            !report.next_steps.contains(&BULK_NODES_STEP),
            "a property step names no node build: {report:?}"
        );
    }

    #[test]
    fn bulk_in_session_previews_without_writing_or_recording() {
        let mut session = bulk_session();
        let report = bulk_in_session(
            &mut session,
            &bulk_args(json!({
                "kind": "sectors",
                "where": [one_filter("index", "eq", &json!(1_i64))],
                "add": {"light": 8_i64},
                "dry_run": true,
            })),
        )
        .expect("the preview runs");
        let shown = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(
            shown["matched"],
            json!(1_u64),
            "the index filter selected one"
        );
        assert_eq!(shown["changed"], json!(1_u64), "its light would move");
        assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(
            shown["preview"][0]["before"],
            json!({"light": 0_i64}),
            "the past previews"
        );
        assert_eq!(
            shown["preview"][0]["after"],
            json!({"light": 8_i64}),
            "and the future"
        );
        assert_eq!(
            shown["copied_to_working"],
            json!(false),
            "a preview copies nothing"
        );
        assert_eq!(
            shown["nodes_invalidated"],
            json!(false),
            "and invalidates nothing"
        );
        assert!(session.working().is_empty(), "the working archive stands");
        assert!(session.undo_labels().is_empty(), "no history entry");
        assert_eq!(
            report.next_steps.len(),
            2_usize,
            "the preview carries its steps"
        );
    }

    #[test]
    fn bulk_in_session_refuses_an_empty_request_before_any_read() {
        let mut session = bulk_session();
        let error = bulk_in_session(&mut session, &bulk_args(json!({"kind": "sectors"})))
            .expect_err("no change was asked");
        assert_eq!(error.code(), "invalid_argument", "empty request code");
        assert!(
            error.to_string().contains("nothing to change"),
            "the message names the parameters: {error}"
        );
        assert!(session.undo_labels().is_empty(), "and nothing was recorded");
    }

    #[test]
    fn bulk_in_session_guards_the_matched_set_with_max() {
        let mut session = bulk_session();
        let error = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "vertices", "max": 1_i64, "add": {"x": 4_i64}})),
        )
        .expect_err("two vertices match, max is one");
        assert_eq!(error.code(), "limit_exceeded", "over max code");
        let message = error.to_string();
        assert!(
            message.contains("max") && message.contains("dry_run") && message.contains("where"),
            "the refusal names max, dry_run and the narrowing: {message}"
        );
        assert!(
            session.undo_labels().is_empty(),
            "the guard recorded nothing"
        );
        assert!(session.working().is_empty(), "and wrote nothing");
    }

    #[test]
    fn bulk_in_session_slides_vertices_as_geometry_and_things_as_property() {
        let mut session = bulk_session();
        let slid = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "vertices", "add": {"x": 8_i64}})),
        )
        .expect("the slide runs");
        let shown = serde_json::to_value(&slid).expect("the report serializes");
        assert_eq!(
            shown["changed"],
            json!(2_u64),
            "both referenced vertices moved"
        );
        assert_eq!(
            shown["nodes_invalidated"],
            json!(true),
            "moving referenced vertices is geometry"
        );
        assert!(
            slid.next_steps.contains(&BULK_NODES_STEP),
            "the refusal names the node build: {slid:?}"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["bulk_edit vertices (2)"],
            "one labeled step"
        );
        let moved = bulk_in_session(
            &mut session,
            &bulk_args(json!({
                "kind": "things",
                "where": [one_filter("type", "eq", &json!(3001_i64))],
                "add": {"x": 16_i64},
            })),
        )
        .expect("the thing move runs");
        let written = serde_json::to_value(&moved).expect("the report serializes");
        assert_eq!(written["changed"], json!(1_u64), "the imp slid");
        assert_eq!(
            written["nodes_invalidated"],
            json!(false),
            "thing moves never touch the nodes"
        );
        assert_eq!(
            written["copied_to_working"],
            json!(false),
            "the block was already editable"
        );
    }

    #[test]
    fn bulk_in_session_resolves_the_thing_type_name_and_refuses_the_stray() {
        let mut session = bulk_session();
        let swapped = bulk_in_session(
            &mut session,
            &bulk_args(json!({
                "kind": "things",
                "where": [one_filter("name", "glob", &json!("imp*"))],
                "set": {"type": "demon"},
            })),
        )
        .expect("the name resolves against the session definitions");
        let shown = serde_json::to_value(&swapped).expect("the report serializes");
        assert_eq!(shown["matched"], json!(1_u64), "the glob found the imp");
        assert_eq!(
            shown["preview"][0]["after"],
            json!({"type": 3002_i64}),
            "the name landed as the doomednum"
        );
        let stray = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "things", "set": {"type": "platypus"}})),
        )
        .expect_err("nothing is named platypus");
        assert_eq!(stray.code(), "invalid_argument", "unknown name code");
        assert!(
            stray.to_string().contains("get_thing_types"),
            "the reference is named: {stray}"
        );
        assert_eq!(
            session.undo_labels().len(),
            1_usize,
            "the refusal recorded nothing"
        );
    }

    #[test]
    fn bulk_in_session_sets_linedef_flags_by_name() {
        let mut session = bulk_session();
        let gated = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "linedefs", "flags_set": ["blockmonsters"]})),
        )
        .expect("the flag edit runs");
        let shown = serde_json::to_value(&gated).expect("the report serializes");
        assert_eq!(shown["changed"], json!(1_u64), "the fixture line moved");
        assert_eq!(
            shown["preview"][0]["after"],
            json!({"flags": 3_i64}),
            "blocking stands, blockmonsters goes on"
        );
        let map = common::load_map(&session, parsed("MAP01"), Scope::Merged)
            .expect("the merged block reads");
        assert_eq!(
            map.linedefs.first().expect("the fixture line").flags,
            3_u32,
            "the bits landed"
        );
        let stray = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "linedefs", "flags_set": ["nope"]})),
        )
        .expect_err("nope is no flag");
        assert_eq!(stray.code(), "invalid_argument", "unknown flag code");
        assert_eq!(
            session.undo_labels().len(),
            1_usize,
            "the refusal recorded nothing"
        );
    }

    #[test]
    fn bulk_in_session_checks_the_stock_and_allow_missing_escapes_it() {
        let mut session = bulk_session();
        let missing = bulk_in_session(
            &mut session,
            &bulk_args(json!({"kind": "sectors", "set": {"floor_flat": "slime9"}})),
        )
        .expect_err("the name is not part of the stock");
        assert_eq!(missing.code(), "not_found", "missing stock code");
        assert!(
            missing.to_string().contains("allow_missing"),
            "the escape hatch is named: {missing}"
        );
        let written = bulk_in_session(
            &mut session,
            &bulk_args(json!({
                "kind": "sectors",
                "set": {"floor_flat": "slime9"},
                "allow_missing": true,
            })),
        )
        .expect("the escape admits the name");
        let shown = serde_json::to_value(&written).expect("the report serializes");
        assert_eq!(shown["changed"], json!(2_u64), "both sectors took it");
        assert_eq!(
            session.undo_labels(),
            vec!["bulk_edit sectors (2)"],
            "one labeled step wrote the unstocked names"
        );
    }

    #[test]
    fn bulk_run_reports_the_no_map_refusal_through_the_state() {
        let state = AppState::default();
        let error = bulk_run(
            &state,
            &bulk_args(json!({"kind": "sectors", "map": "MAP09", "set": {"light": 1_i64}})),
        )
        .expect_err("nothing is open");
        assert_eq!(
            error.code(),
            "not_found",
            "an explicit unknown map is missing"
        );
    }

    #[test]
    fn bulk_report_pages_the_index_and_preview_windows() {
        let wide = INDEX_PAGE.checked_add(2_usize).expect("the window fits");
        let applied = Applied {
            matched: wide,
            changed: wide,
            indices: (0..wide).collect(),
            preview: (0..wide)
                .take(PREVIEW_PAGE.checked_add(5_usize).expect("window"))
                .map(|index| Change {
                    index,
                    before: json!({"light": 0_i64}),
                    after: json!({"light": 1_i64}),
                })
                .collect(),
        };
        let report = bulk_report(applied, false, JsonMap::new());
        assert_eq!(report.matched, wide, "the counts travel whole");
        assert_eq!(report.indices.len(), INDEX_PAGE, "the index list is capped");
        assert!(report.truncated, "and says so");
        assert_eq!(report.preview.len(), PREVIEW_PAGE, "the preview is capped");
        assert_eq!(
            report.next_steps.len(),
            3_usize,
            "a plain written run carries the three steps"
        );
    }
}
