//! Query tools: [`Cyberdemon::find_elements`], the set-valued read behind
//! every plain-English "all the X" request.
//!
//! The filtering is the pure [`crate::wad::map::query`] engine's; this file
//! resolves the flattened query parameters, reads the map and the session's
//! definitions (custom `DECORATE`/`DEHACKED` things included) under one
//! lock exactly like the playability group does, runs the query and pages
//! the result: `count` totals every hit, `indices` and `rows` carry one
//! ascending page of the hits with a compact summary per kind, and
//! `next_offset` names the `offset` where the rest continues. CD-108's bulk
//! edits take the same `Query` and act on exactly these indices.

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
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::{fail, ok};
use crate::wad::map::Map;
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        DEFAULT_LIMIT, FindElementsParams, MAX_LIMIT, Row, find_run, page_hits, parse_limit, rows,
    };
    use crate::defs::definitions_for;
    use crate::state::AppState;
    use crate::wad::game::Game;
    use crate::wad::map::query::{ElementKind, Filter, QueryContext};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Thing, Vertex};
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
}
