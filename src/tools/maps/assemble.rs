//! The typed whole-map graph behind the `assemble_map` tool: every
//! element of the requested kinds as one row with its references already
//! resolved (vertices looked up, sidedefs mapped to sectors, things placed
//! into sectors), so a client can reason about the map without chasing
//! indices. Dangling references never fail the call: they are named in the
//! graph's `problems` list and the unresolvable part becomes `null`.
//!
//! The row builders here are pure model folds; the geometry (sector
//! boundaries, adjacency, containment, boxes, lengths) comes from
//! [`crate::wad::map::geometry`].

use serde::Serialize;

use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::state::AppState;
use crate::tools::{json_bytes, parse_limit};
use crate::wad::map::flags::to_names;
use crate::wad::map::geometry;
use crate::wad::map::{FlagKind, Map};
use crate::wad::name::LumpName;

/// File extension of the published graph asset.
const JSON_EXTENSION: &str = "json";
/// Inline limit when the caller gives none.
const DEFAULT_INLINE_LIMIT: usize = 2000;
/// Largest accepted inline limit.
const MAX_INLINE_LIMIT: usize = 20000;

/// One of the five element kinds a graph can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The vertex array.
    Vertices,
    /// The linedef array.
    Linedefs,
    /// The sidedef array.
    Sidedefs,
    /// The sector array.
    Sectors,
    /// The thing array.
    Things,
}

/// The five kinds in report order.
const ALL_KINDS: [Kind; 5] = [
    Kind::Vertices,
    Kind::Linedefs,
    Kind::Sidedefs,
    Kind::Sectors,
    Kind::Things,
];

impl Kind {
    /// The include word and array key of this kind.
    const fn name(self) -> &'static str {
        match self {
            Self::Vertices => "vertices",
            Self::Linedefs => "linedefs",
            Self::Sidedefs => "sidedefs",
            Self::Sectors => "sectors",
            Self::Things => "things",
        }
    }

    /// The kind an include word names, case-insensitively.
    fn parse(text: &str) -> Option<Self> {
        let lowered = text.to_ascii_lowercase();
        ALL_KINDS
            .into_iter()
            .find(|kind| kind.name() == lowered.as_str())
    }
}

/// Resolve `include` into the kinds to carry: absent means all five;
/// duplicates drop; an unknown name refuses with the valid list.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the unknown word.
pub(super) fn parse_include(requested: Option<Vec<String>>) -> Result<Vec<Kind>, CyberError> {
    let Some(texts) = requested else {
        return Ok(ALL_KINDS.to_vec());
    };
    let mut picked: Vec<Kind> = Vec::new();
    for text in &texts {
        let kind = Kind::parse(text).ok_or_else(|| unknown_include(text))?;
        if !picked.contains(&kind) {
            picked.push(kind);
        }
    }
    Ok(picked)
}

/// The refusal for an include word that names no kind.
fn unknown_include(text: &str) -> CyberError {
    let valid: Vec<&str> = ALL_KINDS.iter().map(|kind| kind.name()).collect();
    CyberError::InvalidArgument(format!(
        "include {text:?} names no element kind: valid names are {}; \
         pass a subset of those or leave the parameter out",
        valid.join(", ")
    ))
}

/// The element-count threshold under which the graph is inlined, with the
/// default and maximum applied.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or any value above
/// [`MAX_INLINE_LIMIT`].
pub(super) fn parse_inline_limit(given: Option<usize>) -> Result<usize, CyberError> {
    parse_limit(
        given,
        DEFAULT_INLINE_LIMIT,
        MAX_INLINE_LIMIT,
        "inline_limit",
    )
    .map_err(|_ignored| {
        CyberError::InvalidArgument(format!(
            "inline_limit {} is outside 1..={MAX_INLINE_LIMIT}; pass the \
             largest element count to inline, or leave the parameter out for \
             {DEFAULT_INLINE_LIMIT}",
            given.unwrap_or_default()
        ))
    })
}

/// A vertex row.
#[derive(Serialize)]
struct VertexNode {
    /// Position in [`Map::vertices`].
    index: usize,
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
    /// Indices of the linedefs using this vertex.
    linedefs: Vec<usize>,
}

/// A vertex as referenced from a linedef row.
#[derive(Serialize)]
struct VertexRef {
    /// Position in [`Map::vertices`].
    index: usize,
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
}

/// One side of a linedef, resolved through its sidedef.
#[derive(Serialize)]
struct SideNode {
    /// Position in [`Map::sidedefs`].
    sidedef: usize,
    /// The sector this side opens into, `null` when the reference dangles.
    sector: Option<usize>,
    /// Upper texture name.
    upper: String,
    /// Middle texture name.
    middle: String,
    /// Lower texture name.
    lower: String,
    /// Texture shift along the line.
    x_offset: i32,
    /// Texture shift across the line.
    y_offset: i32,
}

/// A linedef row with both ends and both sides resolved.
#[derive(Serialize)]
struct LinedefNode {
    /// Position in [`Map::linedefs`].
    index: usize,
    /// The start vertex, `null` when the reference dangles.
    v1: Option<VertexRef>,
    /// The end vertex, `null` when the reference dangles.
    v2: Option<VertexRef>,
    /// Floor-rounded line length in map units, `null` when unresolved.
    length: Option<u64>,
    /// Raw flag bits as the map's format stores them.
    flags: u32,
    /// The flag names from the map's flag family.
    flag_names: Vec<&'static str>,
    /// Line special number.
    special: i32,
    /// Trigger tag.
    tag: i32,
    /// The five script arguments (`Hexen`/`UDMF`).
    args: [i32; 5],
    /// The front side, `null` when absent or dangling.
    front: Option<SideNode>,
    /// The back side, `null` when absent or dangling.
    back: Option<SideNode>,
}

/// A sidedef row, with the linedef side that references it.
#[derive(Serialize)]
struct SidedefNode {
    /// Position in [`Map::sidedefs`].
    index: usize,
    /// The sector this side opens into, `null` when the reference dangles.
    sector: Option<usize>,
    /// The first linedef referencing this sidedef, `null` for orphans.
    linedef: Option<usize>,
    /// Which side the first referencing linedef uses it as.
    side: Option<&'static str>,
    /// Upper texture name.
    upper: String,
    /// Middle texture name.
    middle: String,
    /// Lower texture name.
    lower: String,
    /// Texture shift along the line.
    x_offset: i32,
    /// Texture shift across the line.
    y_offset: i32,
}

/// A sector row with its whole neighbourhood resolved.
#[derive(Serialize)]
struct SectorNode {
    /// Position in [`Map::sectors`].
    index: usize,
    /// Floor height in map units.
    floor_height: i32,
    /// Ceiling height in map units.
    ceiling_height: i32,
    /// Floor flat name.
    floor_flat: String,
    /// Ceiling flat name.
    ceiling_flat: String,
    /// Light level.
    light: i32,
    /// Sector effect number.
    special: i32,
    /// Tag matched by linedef triggers.
    tag: i32,
    /// Indices of the boundary linedefs.
    linedefs: Vec<usize>,
    /// Indices of the sidedefs opening into this sector.
    sidedefs: Vec<usize>,
    /// Indices of the sectors across two-sided boundary lines.
    neighbors: Vec<usize>,
    /// Indices of the things standing in this sector.
    things: Vec<usize>,
    /// Boundary box `(min_x, min_y, max_x, max_y)`, `null` when unbounded.
    bbox: Option<(i32, i32, i32, i32)>,
}

/// A thing row placed into its sector.
#[derive(Serialize)]
struct ThingNode {
    /// Position in [`Map::things`].
    index: usize,
    /// Thing number.
    #[serde(rename = "type")]
    kind: i32,
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
    /// Spawn height in map units.
    z: i32,
    /// Facing direction in degrees.
    angle: i32,
    /// Raw flag bits as the map's format stores them.
    flags: u32,
    /// The flag names from the map's flag family.
    flag_names: Vec<&'static str>,
    /// Thing id (`Hexen`/`UDMF`).
    tid: i32,
    /// Thing special (`Hexen`/`UDMF`).
    special: i32,
    /// The five script arguments (`Hexen`/`UDMF`).
    args: [i32; 5],
    /// The sector containing the thing, `null` when it stands outside all.
    sector: Option<usize>,
}

/// The whole typed graph: one array per included kind plus the dangling
/// references met on the way.
#[derive(Serialize)]
pub(super) struct Graph {
    /// The map this graph describes.
    map: LumpName,
    /// Vertex rows; `None` when the kind was not included.
    #[serde(skip_serializing_if = "Option::is_none")]
    vertices: Option<Vec<VertexNode>>,
    /// Linedef rows; `None` when the kind was not included.
    #[serde(skip_serializing_if = "Option::is_none")]
    linedefs: Option<Vec<LinedefNode>>,
    /// Sidedef rows; `None` when the kind was not included.
    #[serde(skip_serializing_if = "Option::is_none")]
    sidedefs: Option<Vec<SidedefNode>>,
    /// Sector rows; `None` when the kind was not included.
    #[serde(skip_serializing_if = "Option::is_none")]
    sectors: Option<Vec<SectorNode>>,
    /// Thing rows; `None` when the kind was not included.
    #[serde(skip_serializing_if = "Option::is_none")]
    things: Option<Vec<ThingNode>>,
    /// Dangling references found while resolving, never a failure.
    problems: Vec<String>,
}

impl Graph {
    /// The per-kind row counts plus the problem count; the inline-limit
    /// total is then read off these once-measured lengths (see
    /// [`Counts::total`]).
    fn counts(&self) -> Counts {
        Counts {
            vertices: self.vertices.as_ref().map_or(0, Vec::len),
            linedefs: self.linedefs.as_ref().map_or(0, Vec::len),
            sidedefs: self.sidedefs.as_ref().map_or(0, Vec::len),
            sectors: self.sectors.as_ref().map_or(0, Vec::len),
            things: self.things.as_ref().map_or(0, Vec::len),
            problems: self.problems.len(),
        }
    }
}

/// Row counts of a graph, reported alongside a published asset.
#[derive(Serialize)]
pub(super) struct Counts {
    /// Vertex rows.
    vertices: usize,
    /// Linedef rows.
    linedefs: usize,
    /// Sidedef rows.
    sidedefs: usize,
    /// Sector rows.
    sectors: usize,
    /// Thing rows.
    things: usize,
    /// Dangling references found.
    problems: usize,
}

impl Counts {
    /// Elements across the five kind arrays; the problem count rides
    /// along in the report but never decides the inline limit.
    fn total(&self) -> usize {
        [
            self.vertices,
            self.linedefs,
            self.sidedefs,
            self.sectors,
            self.things,
        ]
        .into_iter()
        .fold(0_usize, usize::saturating_add)
    }
}

/// What the `assemble_map` tool hands the client: the graph itself when it
/// fits the inline limit, else a summary of the published asset.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Outcome {
    /// The full graph, inlined.
    Inline(Graph),
    /// The graph published as `<MAP>_graph.json`.
    Asset(Summary),
}

/// The asset-mode result: the map, its counts and the published JSON.
#[derive(Serialize)]
pub(super) struct Summary {
    /// The map the graph describes.
    map: LumpName,
    /// Rows per kind and the problem count.
    counts: Counts,
    /// The published full JSON.
    asset: AssetRef,
}

/// A built graph, awaiting its delivery decision.
pub(super) struct Plan {
    /// The typed graph.
    graph: Graph,
}

impl Plan {
    /// Deliver the graph: inline when it holds at most `limit` elements,
    /// else serialize it, publish `<MAP>_graph.json` and summarize.
    ///
    /// # Errors
    ///
    /// [`CyberError::Internal`] when the graph does not serialize (a
    /// server bug) and [`AppState::publish`]'s errors for the asset store.
    pub(super) fn finish(self, state: &AppState, limit: usize) -> Result<Outcome, CyberError> {
        let counts = self.graph.counts();
        if counts.total() <= limit {
            return Ok(Outcome::Inline(self.graph));
        }
        let data = json_bytes(&self.graph, "graph")?;
        let filename = format!("{}_graph.{JSON_EXTENSION}", self.graph.map);
        let asset = state.publish(&filename, mime_for_extension(JSON_EXTENSION), data)?;
        Ok(Outcome::Asset(Summary {
            map: self.graph.map,
            counts,
            asset,
        }))
    }
}

/// Assemble the typed graph of `map` over the `include` kinds.
pub(super) fn assemble(name: LumpName, map: &Map, include: &[Kind]) -> Plan {
    let mut problems: Vec<String> = Vec::new();
    let wants = |kind| include.contains(&kind);
    let graph = Graph {
        map: name,
        vertices: wants(Kind::Vertices).then(|| vertex_nodes(map)),
        linedefs: wants(Kind::Linedefs).then(|| linedef_nodes(map, &mut problems)),
        sidedefs: wants(Kind::Sidedefs).then(|| sidedef_nodes(map, &mut problems)),
        sectors: wants(Kind::Sectors).then(|| sector_nodes(map)),
        things: wants(Kind::Things).then(|| thing_nodes(map)),
        problems,
    };
    Plan { graph }
}

/// The vertex rows, each listing the linedefs that use it.
fn vertex_nodes(map: &Map) -> Vec<VertexNode> {
    map.vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| VertexNode {
            index,
            x: vertex.x,
            y: vertex.y,
            linedefs: map
                .linedefs
                .iter()
                .enumerate()
                .filter_map(|(at, line)| (line.v1 == index || line.v2 == index).then_some(at))
                .collect(),
        })
        .collect()
}

/// The linedef `index` as a reference row, or a problem naming `owner`.
fn vertex_ref(
    map: &Map,
    index: usize,
    owner: &str,
    problems: &mut Vec<String>,
) -> Option<VertexRef> {
    map.vertices.get(index).map_or_else(
        || {
            problems.push(format!("{owner} vertex {index} is out of range"));
            None
        },
        |vertex| {
            Some(VertexRef {
                index,
                x: vertex.x,
                y: vertex.y,
            })
        },
    )
}

/// The side of a linedef stored at `index`, resolved through its sidedef.
/// An absent side is quietly `null`; a dangling sidedef or sector index
/// records a problem naming `owner`.
fn side_node(
    map: &Map,
    index: Option<usize>,
    owner: &str,
    problems: &mut Vec<String>,
) -> Option<SideNode> {
    let given = index?;
    let sidedef = map.sidedefs.get(given).map_or_else(
        || {
            problems.push(format!("{owner} sidedef {given} is out of range"));
            None
        },
        Some,
    )?;
    let sector = resolve_sector(
        map,
        sidedef.sector,
        &format!("{owner} sidedef {given}"),
        problems,
    );
    Some(SideNode {
        sidedef: given,
        sector,
        upper: sidedef.upper.clone(),
        middle: sidedef.middle.clone(),
        lower: sidedef.lower.clone(),
        x_offset: sidedef.x_offset,
        y_offset: sidedef.y_offset,
    })
}

/// A sidedef's sector index as a live reference, or `null` with a problem
/// naming `owner` when it dangles.
fn resolve_sector(
    map: &Map,
    index: usize,
    owner: &str,
    problems: &mut Vec<String>,
) -> Option<usize> {
    if map.sectors.get(index).is_some() {
        return Some(index);
    }
    problems.push(format!("{owner} sector {index} is out of range"));
    None
}

/// The linedef rows, with flags named in the map's family.
fn linedef_nodes(map: &Map, problems: &mut Vec<String>) -> Vec<LinedefNode> {
    let family = map.flag_family();
    map.linedefs
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let owner = format!("linedef {index}");
            LinedefNode {
                index,
                v1: vertex_ref(map, line.v1, &owner, problems),
                v2: vertex_ref(map, line.v2, &owner, problems),
                length: geometry::linedef_length(map, index),
                flags: line.flags,
                flag_names: to_names(line.flags, FlagKind::Linedef, family),
                special: line.special,
                tag: line.tag,
                args: line.args,
                front: side_node(map, line.front, &format!("{owner} front"), problems),
                back: side_node(map, line.back, &format!("{owner} back"), problems),
            }
        })
        .collect()
}

/// The linedef side that first references each sidedef.
#[derive(Clone, Copy)]
struct Owner {
    /// Position in [`Map::linedefs`] of the referencing line.
    linedef: usize,
    /// Which side references it: `"front"` or `"back"`.
    side: &'static str,
}

/// One linedef side's claim on the sidedef array; dangling indices are
/// skipped (the linedef rows already report them).
fn owners_of(map: &Map) -> Vec<Option<Owner>> {
    let mut owners: Vec<Option<Owner>> = map.sidedefs.iter().map(|_| None).collect();
    for (linedef, line) in map.linedefs.iter().enumerate() {
        claim(
            &mut owners,
            line.front,
            Owner {
                linedef,
                side: "front",
            },
        );
        claim(
            &mut owners,
            line.back,
            Owner {
                linedef,
                side: "back",
            },
        );
    }
    owners
}

/// Record `owner` for sidedef `index` unless an earlier line claimed it.
fn claim(owners: &mut [Option<Owner>], index: Option<usize>, owner: Owner) {
    let Some(slot) = index.and_then(|at| owners.get_mut(at)) else {
        return;
    };
    if slot.is_none() {
        *slot = Some(owner);
    }
}

/// The sidedef rows, each back-referenced to its first linedef side.
fn sidedef_nodes(map: &Map, problems: &mut Vec<String>) -> Vec<SidedefNode> {
    let owners = owners_of(map);
    map.sidedefs
        .iter()
        .enumerate()
        .map(|(index, sidedef)| {
            let sector = resolve_sector(map, sidedef.sector, &format!("sidedef {index}"), problems);
            let owner = owners.get(index).copied().flatten();
            SidedefNode {
                index,
                sector,
                linedef: owner.map(|found| found.linedef),
                side: owner.map(|found| found.side),
                upper: sidedef.upper.clone(),
                middle: sidedef.middle.clone(),
                lower: sidedef.lower.clone(),
                x_offset: sidedef.x_offset,
                y_offset: sidedef.y_offset,
            }
        })
        .collect()
}

/// The sector each thing stands in.
fn thing_homes(map: &Map) -> Vec<Option<usize>> {
    map.things
        .iter()
        .map(|spawn| geometry::point_in_sector(map, i64::from(spawn.x), i64::from(spawn.y)))
        .collect()
}

/// The sector rows with boundary, neighbours and occupants resolved.
fn sector_nodes(map: &Map) -> Vec<SectorNode> {
    let homes = thing_homes(map);
    map.sectors
        .iter()
        .enumerate()
        .map(|(index, sector)| SectorNode {
            index,
            floor_height: sector.floor_height,
            ceiling_height: sector.ceiling_height,
            floor_flat: sector.floor_flat.clone(),
            ceiling_flat: sector.ceiling_flat.clone(),
            light: sector.light,
            special: sector.special,
            tag: sector.tag,
            linedefs: geometry::sector_boundary(map, index)
                .into_iter()
                .map(|(at, _)| at)
                .collect(),
            sidedefs: sidedefs_of(map, index),
            neighbors: geometry::neighbors(map, index),
            things: homes
                .iter()
                .enumerate()
                .filter_map(|(at, home)| (*home == Some(index)).then_some(at))
                .collect(),
            bbox: geometry::sector_bbox(map, index),
        })
        .collect()
}

/// The sidedef indices opening into `sector`, in sidedef order.
fn sidedefs_of(map: &Map, sector: usize) -> Vec<usize> {
    map.sidedefs
        .iter()
        .enumerate()
        .filter_map(|(index, sidedef)| (sidedef.sector == sector).then_some(index))
        .collect()
}

/// The thing rows, flags named and sectors assigned.
fn thing_nodes(map: &Map) -> Vec<ThingNode> {
    let family = map.flag_family();
    let homes = thing_homes(map);
    map.things
        .iter()
        .enumerate()
        .map(|(index, spawn)| ThingNode {
            index,
            kind: spawn.kind,
            x: spawn.x,
            y: spawn.y,
            z: spawn.z,
            angle: spawn.angle,
            flags: spawn.flags,
            flag_names: to_names(spawn.flags, FlagKind::Thing, family),
            tid: spawn.tid,
            special: spawn.special,
            args: spawn.args,
            sector: homes.get(index).copied().flatten(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::{Kind, MAX_INLINE_LIMIT, assemble, parse_include, parse_inline_limit};
    use crate::state::AppState;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: BTreeMap::new(),
        }
    }

    /// A one-sided line from `v1` to `v2` over front sidedef `front`.
    fn line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
    fn line_both(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sidedef over `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// A spawn of `kind` at `x`/`y`.
    fn spawn(kind: i32, x: i32, y: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            angle: 0,
            flags: 7,
            ..Default::default()
        }
    }

    /// The unit-test two-room map: a hall (`0..256`) and a back room
    /// (`256..512`) split by the two-sided line `2`.
    fn two_rooms() -> Map {
        Map {
            name: name("MAP01"),
            things: vec![spawn(1, 64, 128), spawn(3001, 384, 128)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                line(0, 5, 0),
                line(5, 4, 1),
                line_both(4, 1, 2, 3),
                line(1, 0, 4),
                line(4, 3, 5),
                line(3, 2, 6),
                line(2, 1, 7),
            ],
            sidedefs: vec![
                side(0),
                side(0),
                side(0),
                side(1),
                side(0),
                side(1),
                side(1),
                side(1),
            ],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    /// The full graph of `map` as a JSON value.
    fn graph_json(map: &Map) -> Value {
        let include = parse_include(None).expect("all kinds");
        let plan = assemble(name("MAP01"), map, &include);
        let outcome = plan
            .finish(&AppState::default(), MAX_INLINE_LIMIT)
            .expect("inlining needs no asset server");
        serde_json::to_value(&outcome).expect("outcomes serialize")
    }

    #[test]
    fn parse_include_defaults_deduplicates_and_refuses_unknowns() {
        assert_eq!(
            parse_include(None).expect("absent means all"),
            vec![
                Kind::Vertices,
                Kind::Linedefs,
                Kind::Sidedefs,
                Kind::Sectors,
                Kind::Things
            ],
            "all five, in report order",
        );
        let given = vec![
            "Things".to_owned(),
            "linedefs".to_owned(),
            "things".to_owned(),
        ];
        assert_eq!(
            parse_include(Some(given)).expect("known names"),
            vec![Kind::Things, Kind::Linedefs],
            "case-insensitive, deduplicated, request order",
        );
        let empty = parse_include(Some(Vec::new())).expect("nothing is valid");
        assert!(empty.is_empty(), "an empty include carries no arrays");
        let error = parse_include(Some(vec!["nope".to_owned()])).expect_err("not a kind");
        assert_eq!(error.code(), "invalid_argument", "bad kind code");
        let message = error.to_string();
        assert!(
            message.contains("\"nope\"")
                && message.contains("vertices, linedefs, sidedefs, sectors, things"),
            "the refusal names the word and the valid list: {message}"
        );
    }

    #[test]
    fn parse_inline_limit_applies_the_default_and_the_range() {
        assert_eq!(
            parse_inline_limit(None).expect("absent"),
            2000,
            "the default"
        );
        assert_eq!(
            parse_inline_limit(Some(MAX_INLINE_LIMIT)).expect("the boundary"),
            MAX_INLINE_LIMIT,
            "the maximum is inclusive"
        );
        let zero = parse_inline_limit(Some(0)).expect_err("zero refuses");
        assert_eq!(zero.code(), "invalid_argument", "zero code");
        let over = parse_inline_limit(Some(20_001)).expect_err("above the maximum");
        assert_eq!(over.code(), "invalid_argument", "over-max code");
        assert!(
            zero.to_string().contains("1..=20000"),
            "the message names the range: {zero}"
        );
    }

    #[test]
    fn the_graph_resolves_linedefs_sidedefs_and_vertices() {
        let report = graph_json(&two_rooms());
        assert_eq!(report["map"], json!("MAP01"), "the map echoes");
        assert_eq!(report["problems"], json!([]), "the fixture map is sound");
        let step = &report["linedefs"][2];
        assert_eq!(
            step["front"]["sector"],
            json!(0_i64),
            "the step line's front room"
        );
        assert_eq!(step["back"]["sector"], json!(1_i64), "and its back room");
        assert_eq!(step["flag_names"], json!(["twosided"]), "flags named");
        assert_eq!(step["length"], json!(256_u64), "the shared wall's length");
        assert_eq!(
            step["v1"]["index"],
            json!(4_i64),
            "ends resolved to coordinates"
        );
        assert_eq!(step["v1"]["y"], json!(256_i64), "vertex 4 is at y 256");
        assert_eq!(
            report["sidedefs"][3]["side"],
            json!("back"),
            "sidedef 3 is a back side"
        );
        assert_eq!(
            report["sidedefs"][3]["linedef"],
            json!(2_i64),
            "of the step line"
        );
        assert_eq!(
            report["vertices"][1]["linedefs"],
            json!([2_i64, 3_i64, 6_i64]),
            "shared corner"
        );
    }

    #[test]
    fn the_graph_places_sectors_and_things() {
        let report = graph_json(&two_rooms());
        assert_eq!(
            report["sectors"][0]["neighbors"],
            json!([1_i64]),
            "hall neighbours"
        );
        assert_eq!(
            report["sectors"][0]["things"],
            json!([0_i64]),
            "the player start stands in the hall"
        );
        assert_eq!(
            report["sectors"][1]["things"],
            json!([1_i64]),
            "the imp stands in the back room"
        );
        assert_eq!(
            report["sectors"][0]["bbox"],
            json!([0_i64, 0_i64, 256_i64, 256_i64]),
            "the hall box"
        );
        assert_eq!(
            report["things"][1]["sector"],
            json!(1_i64),
            "and back from the thing side"
        );
        assert_eq!(
            report["things"][0]["flag_names"],
            json!(["easy", "medium", "hard"]),
            "thing flags named",
        );
        assert_eq!(
            report["sectors"][0]["sidedefs"],
            json!([0_i64, 1_i64, 2_i64, 4_i64]),
            "the hall's sides"
        );
    }

    #[test]
    fn include_narrows_the_arrays_and_the_counts() {
        let include = parse_include(Some(vec!["things".to_owned()])).expect("one kind");
        let plan = assemble(name("MAP01"), &two_rooms(), &include);
        let outcome = plan
            .finish(&AppState::default(), MAX_INLINE_LIMIT)
            .expect("two things inline");
        let report = serde_json::to_value(&outcome).expect("outcomes serialize");
        assert!(
            report.get("things").is_some(),
            "the requested array is there"
        );
        for key in ["vertices", "linedefs", "sidedefs", "sectors"] {
            assert!(report.get(key).is_none(), "{key} was not requested");
        }
        assert_eq!(
            report["things"].as_array().expect("array").len(),
            2,
            "both things"
        );
        assert_eq!(report["problems"], json!([]), "problems always ride along");
    }

    #[test]
    fn dangling_references_become_problems_and_nulls() {
        let mut broken = two_rooms();
        broken.sidedefs.get_mut(4).expect("sidedef 4").sector = 99;
        let report = graph_json(&broken);
        let problems = report["problems"].as_array().expect("array");
        assert!(!problems.is_empty(), "the graph reports the break");
        assert!(
            problems.iter().any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("sector 99"))),
            "the dangling sector is named: {problems:?}"
        );
        assert_eq!(
            report["sidedefs"][4]["sector"],
            Value::Null,
            "the row nulls it"
        );
        assert_eq!(
            report["linedefs"][3]["front"]["sector"],
            Value::Null,
            "and through the line"
        );
        assert_eq!(
            report["sectors"][0]["sidedefs"],
            json!([0_i64, 1_i64, 2_i64]),
            "the hall lost side 4"
        );
    }
}
