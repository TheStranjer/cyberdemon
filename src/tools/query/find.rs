//! `find_elements`: the flattened [`crate::wad::map::query`] parameters,
//! the map-and-definitions read under one session lock, the five compact
//! summary row shapes and the run that pages the engine's hits into
//! `{count, indices, rows, next_offset}`.
//!
//! [`find_run`] checks the row cap with [`parse_limit`] before the session
//! is touched, reads the block and the session's definitions (custom
//! `DECORATE`/`DEHACKED` things included) with [`common::load`], runs
//! [`run_query`] and windows the ascending hits with [`Page::window`].
//! [`rows`] summarises one page in the shape its kind documents:
//! [`SectorRow`], [`LinedefRow`], [`SidedefRow`], [`ThingRow`] or
//! [`VertexRow`] folded into [`Row`]; an index that left its own block
//! contributes nothing.
//!
//! The `bulk_edit` selection this query drives is the `bulk` module's; the
//! three `replace_*` swaps and the machinery they share across maps are the
//! `replace_tools` and public `replace` modules'.

use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::state::AppState;
use crate::tools::maps::common;
use crate::tools::{Page, parse_limit};
use crate::wad::map::Map;
use crate::wad::map::query::{ElementKind, Filter, Query, QueryContext, Within, run_query};

use super::{DEFAULT_LIMIT, MAX_LIMIT};

/// Arguments for [`Cyberdemon::find_elements`]: the [`Query`] fields
/// flattened beside the paging.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct FindElementsParams {
    /// The element block to run over: "sectors", "linedefs", "sidedefs",
    /// "things" or "vertices".
    pub(super) kind: ElementKind,
    /// The filters to AND together, each `{field, op, value}`; the valid
    /// fields depend on the kind and the ops on the field's type. An empty
    /// list keeps every bounded element.
    #[serde(default, rename = "where")]
    pub(super) r#where: Vec<Filter>,
    /// Bound the hits by region: `{"bbox": [min_x, min_y, max_x, max_y]}`
    /// or `{"sectors": [index, ...]}`.
    #[serde(default)]
    pub(super) within: Option<Within>,
    /// The map to query, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Skip this many hits before returning (default 0). An offset past the
    /// end is never an error: the page simply comes back empty.
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum hits returned (default 200, at most 1000); anything else
    /// fails with `invalid_argument`. `count` always totals everything.
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// One compact sector summary beside its index.
#[derive(Debug, Serialize)]
pub(super) struct SectorRow {
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
pub(super) struct LinedefRow {
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
pub(super) struct SidedefRow {
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
pub(super) struct ThingRow {
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
pub(super) struct VertexRow {
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
pub(super) enum Row {
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
pub(super) fn rows(
    kind: ElementKind,
    page: &[usize],
    map: &Map,
    ctx: &QueryContext<'_>,
) -> Vec<Row> {
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
        front_sector: map.side_sector(row.front),
        back_sector: map.side_sector(row.back),
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
pub(super) struct FindReport {
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

/// `find_elements`' work: check the limit before the session is touched,
/// read the map and the definitions under one lock, run the pure query and
/// page its hits.
///
/// # Errors
///
/// [`parse_limit`]'s [`CyberError::InvalidArgument`], [`common::load`]'s errors and
/// [`run_query`]'s refusals for a bad field, op or value.
pub(super) fn find_run(
    state: &AppState,
    args: FindElementsParams,
) -> Result<FindReport, CyberError> {
    let limit = parse_limit(args.limit, DEFAULT_LIMIT, MAX_LIMIT, "limit")?;
    let prepared = common::load(state, args.map.as_deref())?;
    let query = Query {
        kind: args.kind,
        r#where: args.r#where,
        within: args.within,
    };
    let ctx = QueryContext::new(prepared.map.flag_family(), &prepared.defs);
    let hits = run_query(&prepared.map, &ctx, &query)?;
    let (page, next_offset) = Page::new(args.offset.unwrap_or(0), limit).window(&hits);
    let summary = rows(args.kind, &page, &prepared.map, &ctx);
    Ok(FindReport {
        kind: args.kind.as_str(),
        count: hits.len(),
        indices: page,
        rows: summary,
        next_offset,
    })
}
