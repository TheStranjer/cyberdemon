//! Pure statistics over a decoded [`Map`] plus the node lumps of its block.
//!
//! [`summarize`] folds a map into the compact [`MapSummary`] the
//! `get_map_summary` tool reports: element counts, vertex bounds, line and
//! height ranges, thing/texture/flat tallies, and the generated-node
//! picture read off the block's lump sizes. Nothing here knows about
//! sessions or MCP: the caller hands over the decoded map and the member
//! lumps of its block (an attached `GL` node run included, when the scope
//! shows one).

use alloc::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::model::Map;
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::consts::{NO_TEXTURE, PLAYER_START_TYPES};
use crate::wad::name::LumpName;

/// Bytes in one vanilla `SEGS` record.
const SEG_RECORD_BYTES: usize = 12;
/// Bytes in one vanilla `SSECTORS` record.
const SSECTOR_RECORD_BYTES: usize = 4;
/// Bytes in one vanilla `NODES` record.
const NODE_RECORD_BYTES: usize = 28;
/// Length of the magic number an extended node lump starts with.
const MAGIC_LEN: usize = 4;
/// Longest `things_by_type` list a summary carries.
const THING_TYPE_ROWS: usize = 50;

/// How many of each editable element a map holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ElementCounts {
    /// Rows of [`Map::things`].
    pub things: usize,
    /// Rows of [`Map::vertices`].
    pub vertices: usize,
    /// Rows of [`Map::linedefs`].
    pub linedefs: usize,
    /// Rows of [`Map::sidedefs`].
    pub sidedefs: usize,
    /// Rows of [`Map::sectors`].
    pub sectors: usize,
}

/// The bounding box of a map's vertices, with its derived size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MapBounds {
    /// Smallest vertex `X`.
    pub min_x: i32,
    /// Smallest vertex `Y`.
    pub min_y: i32,
    /// Largest vertex `X`.
    pub max_x: i32,
    /// Largest vertex `Y`.
    pub max_y: i32,
    /// `max_x - min_x`, in map units.
    pub width: i64,
    /// `max_y - min_y`, in map units.
    pub height: i64,
}

/// How the linedefs are sided and specialled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LineStats {
    /// Lines without both sides set.
    pub one_sided: usize,
    /// Lines with both sides set.
    pub two_sided: usize,
    /// Lines whose special is not zero.
    pub with_special: usize,
}

/// The floor and ceiling height extremes across the sectors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HeightRange {
    /// Lowest floor height, `null` with no sectors.
    pub floor_min: Option<i32>,
    /// Highest floor height, `null` with no sectors.
    pub floor_max: Option<i32>,
    /// Lowest ceiling height, `null` with no sectors.
    pub ceiling_min: Option<i32>,
    /// Highest ceiling height, `null` with no sectors.
    pub ceiling_max: Option<i32>,
}

/// How many things carry one type number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ThingTypeCount {
    /// The thing type, reported as the `type` key.
    #[serde(rename = "type")]
    pub kind: i32,
    /// How many things carry it.
    pub count: usize,
}

/// Which node formats the block's lumps show, serialized flattened into
/// [`NodeSummary`]'s object so the two presence flags can stay within the
/// struct-bools budget beside the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NodeKinds {
    /// A `ZNODES` lump exists (the `UDMF` node run).
    pub znodes: bool,
    /// A lump named `GL_` something exists (an attached `GL` node run).
    pub gl: bool,
}

/// The generated node data a map block carries, read from its lump sizes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeSummary {
    /// Whether the block carries node data: bytes in `NODES` or in `ZNODES`.
    pub present: bool,
    /// `SEGS` records counted from the lump size; absent when the `NODES`
    /// lump starts with a magic (see [`NodeSummary::nodes_format`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segs: Option<usize>,
    /// `SSECTORS` records, like [`NodeSummary::segs`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsectors: Option<usize>,
    /// `NODES` records, like [`NodeSummary::segs`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<usize>,
    /// The four uppercase letters the `NODES` lump starts with (`XNOD`,
    /// `ZNOD`, `XGLN`, `ZGLN`, ...) when it holds extended rather than
    /// vanilla node data; the three record counts are absent then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes_format: Option<String>,
    /// `REJECT` size in bytes.
    pub reject_bytes: usize,
    /// `BLOCKMAP` size in bytes.
    pub blockmap_bytes: usize,
    /// The `ZNODES` and `GL_` presence flags, flattened into this object.
    #[serde(flatten)]
    pub kinds: NodeKinds,
}

/// Everything a cheap overview of one map needs to say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MapSummary {
    /// Map name, from the block header.
    pub name: LumpName,
    /// Layout the block's lumps use.
    pub format: MapFormat,
    /// The namespace a `UDMF` `TEXTMAP` declares; `None` for binary maps.
    pub udmf_namespace: Option<String>,
    /// How many of each element the map holds.
    pub counts: ElementCounts,
    /// The vertex bounding box; `None` when the map holds no vertices.
    pub bounds: Option<MapBounds>,
    /// How the linedefs are sided and specialled.
    pub lines: LineStats,
    /// The sector height extremes.
    pub heights: HeightRange,
    /// The most numerous thing types, at most fifty rows.
    pub things_by_type: Vec<ThingTypeCount>,
    /// Which of the player start types 1, 2, 3 and 4 are present.
    pub player_starts: Vec<i32>,
    /// The sidedef texture names, sorted and deduplicated, without `"-"`.
    pub textures_used: Vec<String>,
    /// The sector flat names, sorted and deduplicated, without `"-"`.
    pub flats_used: Vec<String>,
    /// The block's generated node data.
    pub nodes: NodeSummary,
}

/// Summarizes `map` alongside the member lumps `node_lumps` of its block.
///
/// Vanilla node records are counted from their sizes, an extended `NODES`
/// lump reports its magic instead, and `ZNODES` and `GL_` lumps report
/// presence. A block that repeats a member name is read last-match-wins,
/// the way an engine reads one.
#[must_use]
#[inline]
pub fn summarize(map: &Map, node_lumps: &[Lump]) -> MapSummary {
    MapSummary {
        name: map.name,
        format: map.format,
        udmf_namespace: map.udmf_namespace.clone(),
        counts: element_counts(map),
        bounds: map_bounds(map),
        lines: line_stats(map),
        heights: height_range(map),
        things_by_type: thing_rows(map),
        player_starts: player_starts(map),
        textures_used: textures_used(map),
        flats_used: flats_used(map),
        nodes: node_summary(node_lumps),
    }
}

/// The five element lengths.
fn element_counts(map: &Map) -> ElementCounts {
    ElementCounts {
        things: map.things.len(),
        vertices: map.vertices.len(),
        linedefs: map.linedefs.len(),
        sidedefs: map.sidedefs.len(),
        sectors: map.sectors.len(),
    }
}

/// The vertex bounding box with its size, or `None` without vertices.
fn map_bounds(map: &Map) -> Option<MapBounds> {
    let (min_x, min_y, max_x, max_y) = map.bounds()?;
    Some(MapBounds {
        min_x,
        min_y,
        max_x,
        max_y,
        width: span(min_x, max_x),
        height: span(min_y, max_y),
    })
}

/// `high - low` in `i64`: always exact, because the `i32` span fits.
fn span(low: i32, high: i32) -> i64 {
    i64::from(high).saturating_sub(i64::from(low))
}

/// The sided and special tallies over the linedefs.
fn line_stats(map: &Map) -> LineStats {
    let mut stats = LineStats::default();
    for line in &map.linedefs {
        if line.front.is_some() && line.back.is_some() {
            stats.two_sided = stats.two_sided.saturating_add(1);
        } else {
            stats.one_sided = stats.one_sided.saturating_add(1);
        }
        if line.special != 0_i32 {
            stats.with_special = stats.with_special.saturating_add(1);
        }
    }
    stats
}

/// The floor and ceiling extremes across the sectors.
fn height_range(map: &Map) -> HeightRange {
    let mut range = HeightRange::default();
    for sector in &map.sectors {
        range.floor_min = Some(fold_height(range.floor_min, sector.floor_height, i32::min));
        range.floor_max = Some(fold_height(range.floor_max, sector.floor_height, i32::max));
        range.ceiling_min = Some(fold_height(
            range.ceiling_min,
            sector.ceiling_height,
            i32::min,
        ));
        range.ceiling_max = Some(fold_height(
            range.ceiling_max,
            sector.ceiling_height,
            i32::max,
        ));
    }
    range
}

/// Folds `value` into a running extreme `pick`s, replacing the empty one.
fn fold_height(held: Option<i32>, value: i32, pick: fn(i32, i32) -> i32) -> i32 {
    held.map_or(value, |current| pick(current, value))
}

/// The thing-type tally: most numerous first, ties by ascending type, at
/// most [`THING_TYPE_ROWS`] rows.
fn thing_rows(map: &Map) -> Vec<ThingTypeCount> {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for spawned in &map.things {
        counts
            .entry(spawned.kind)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    }
    let mut rows: Vec<ThingTypeCount> = counts
        .into_iter()
        .map(|(kind, count)| ThingTypeCount { kind, count })
        .collect();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.kind.cmp(&right.kind))
    });
    rows.into_iter().take(THING_TYPE_ROWS).collect()
}

/// The player start types the map actually carries, in 1 to 4 order.
fn player_starts(map: &Map) -> Vec<i32> {
    let kinds: BTreeSet<i32> = map.things.iter().map(|spawned| spawned.kind).collect();
    PLAYER_START_TYPES
        .iter()
        .copied()
        .filter(|start| kinds.contains(start))
        .collect()
}

/// The sidedef texture names, sorted and deduplicated, without the none
/// name.
fn textures_used(map: &Map) -> Vec<String> {
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for side in &map.sidedefs {
        names.extend([
            side.upper.as_str(),
            side.lower.as_str(),
            side.middle.as_str(),
        ]);
    }
    names.remove(NO_TEXTURE);
    names.into_iter().map(str::to_owned).collect()
}

/// The sector flat names, sorted and deduplicated, without the none name.
fn flats_used(map: &Map) -> Vec<String> {
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for sector in &map.sectors {
        names.extend([sector.floor_flat.as_str(), sector.ceiling_flat.as_str()]);
    }
    names.remove(NO_TEXTURE);
    names.into_iter().map(str::to_owned).collect()
}

/// The node picture of a block from its member lumps.
fn node_summary(node_lumps: &[Lump]) -> NodeSummary {
    let nodes_lump = lump_named(node_lumps, lumps::NODES);
    let nodes_bytes = nodes_lump.map_or(0, Lump::size);
    let znodes_lump = lump_named(node_lumps, lumps::ZNODES);
    let magic = nodes_lump.and_then(|lump| lump_magic(&lump.data));
    let (segs, subsectors, nodes) = if magic.is_none() {
        let segs_bytes = lump_named(node_lumps, lumps::SEGS).map_or(0, Lump::size);
        let subsector_bytes = lump_named(node_lumps, lumps::SSECTORS).map_or(0, Lump::size);
        (
            Some(record_count(segs_bytes, SEG_RECORD_BYTES)),
            Some(record_count(subsector_bytes, SSECTOR_RECORD_BYTES)),
            Some(record_count(nodes_bytes, NODE_RECORD_BYTES)),
        )
    } else {
        (None, None, None)
    };
    NodeSummary {
        present: nodes_bytes > 0 || znodes_lump.is_some_and(|lump| lump.size() > 0),
        segs,
        subsectors,
        nodes,
        nodes_format: magic,
        reject_bytes: lump_named(node_lumps, lumps::REJECT).map_or(0, Lump::size),
        blockmap_bytes: lump_named(node_lumps, lumps::BLOCKMAP).map_or(0, Lump::size),
        kinds: NodeKinds {
            znodes: znodes_lump.is_some(),
            gl: node_lumps
                .iter()
                .any(|lump| lump.name.as_bytes().starts_with(b"GL_")),
        },
    }
}

/// The last lump named `wanted`, engines reading the last copy of a run.
fn lump_named(lumps: &[Lump], wanted: LumpName) -> Option<&Lump> {
    lumps.iter().rev().find(|lump| lump.name == wanted)
}

/// How many whole `record_bytes`-sized records a `bytes`-sized lump holds.
const fn record_count(bytes: usize, record_bytes: usize) -> usize {
    match bytes.checked_div(record_bytes) {
        Some(count) => count,
        None => 0,
    }
}

/// The four uppercase letters a node lump starts with, or `None` for
/// vanilla record data.
fn lump_magic(data: &[u8]) -> Option<String> {
    let head = data.get(..MAGIC_LEN)?;
    if head.iter().all(u8::is_ascii_uppercase) {
        String::from_utf8(head.to_vec()).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::summarize;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{lump, marker, name, spawn, two_room_map};
    use crate::wad::map::model::{Map, Thing};

    /// Serialize the summary of `map` over its own block lumps.
    fn value_of(map: &Map) -> Value {
        serde_json::to_value(summarize(map, &map.other)).expect("a summary serializes")
    }

    /// The complete expected summary of [`two_room_map`].
    fn two_room_json() -> Value {
        json!({
            "name": "MAP01", "format": "doom", "udmf_namespace": null,
            "counts": {"things": 2_u64, "vertices": 6_u64, "linedefs": 7_u64, "sidedefs": 8_u64, "sectors": 2_u64},
            "bounds": {"min_x": 0_i64, "min_y": 0_i64, "max_x": 512_i64, "max_y": 256_i64, "width": 512_i64, "height": 256_i64},
            "lines": {"one_sided": 6_u64, "two_sided": 1_u64, "with_special": 0_u64},
            "heights": {"floor_min": 0_i64, "floor_max": 32_i64, "ceiling_min": 128_i64, "ceiling_max": 128_i64},
            "things_by_type": [{"type": 1_i64, "count": 1_u64}, {"type": 3001_i64, "count": 1_u64}],
            "player_starts": [1_i64],
            "textures_used": ["STEP1", "WALL1"], "flats_used": ["FLOOR1"],
            "nodes": {
                "present": false, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
                "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
            },
        })
    }

    /// A map holding one thing of every kind in `kinds`, plus an extra
    /// thing of `doubled`.
    fn kinds_map(mut kinds: Vec<i32>, doubled: i32) -> Map {
        kinds.push(doubled);
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        for kind in kinds {
            map.things.push(Thing {
                kind,
                ..Default::default()
            });
        }
        map
    }

    /// The `type` numbers of a `things_by_type` list, in report order.
    fn reported_types(summary: &Value) -> Vec<i64> {
        summary["things_by_type"]
            .as_array()
            .expect("things_by_type is an array")
            .iter()
            .map(|row| row["type"].as_i64().expect("types are integers"))
            .collect()
    }

    #[test]
    fn summarize_reports_the_two_room_map_exactly() {
        assert_eq!(value_of(&two_room_map()), two_room_json(), "the whole JSON");
    }

    #[test]
    fn summarize_of_an_empty_map_has_no_bounds_and_nothing_to_tally() {
        let empty = Map::new_empty(name("MAP01"), MapFormat::Doom);
        assert_eq!(
            value_of(&empty),
            json!({
                "name": "MAP01", "format": "doom", "udmf_namespace": null,
                "counts": {"things": 0_u64, "vertices": 0_u64, "linedefs": 0_u64, "sidedefs": 0_u64, "sectors": 0_u64},
                "bounds": null,
                "lines": {"one_sided": 0_u64, "two_sided": 0_u64, "with_special": 0_u64},
                "heights": {
                    "floor_min": null, "floor_max": null,
                    "ceiling_min": null, "ceiling_max": null,
                },
                "things_by_type": [], "player_starts": [],
                "textures_used": [], "flats_used": [],
                "nodes": {
                    "present": false, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
                    "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
                },
            }),
            "an empty map summarizes empty",
        );
    }

    #[test]
    fn summarize_counts_player_starts_specials_and_shifted_bounds() {
        let mut map = two_room_map();
        map.things.push(spawn(2, 128, 128, 90));
        map.linedefs
            .first_mut()
            .expect("the fixture map has lines")
            .special = 1_i32;
        for vertex in &mut map.vertices {
            vertex.x = vertex.x.saturating_sub(64);
        }
        let summary = value_of(&map);
        assert_eq!(
            summary["player_starts"],
            json!([1_i64, 2_i64]),
            "players one and two",
        );
        assert_eq!(
            summary["lines"]["with_special"],
            json!(1_u64),
            "the tagged line",
        );
        assert_eq!(
            summary["lines"]["one_sided"],
            json!(6_u64),
            "siding did not change",
        );
        assert_eq!(
            summary["bounds"],
            json!({
                "min_x": -64_i64, "min_y": 0_i64, "max_x": 448_i64, "max_y": 256_i64,
                "width": 512_i64, "height": 256_i64,
            }),
            "shifted bounds keep the span",
        );
    }

    #[test]
    fn things_by_type_orders_by_count_then_type_and_truncates() {
        let summary = value_of(&kinds_map((10_i32..70_i32).collect(), 42));
        let rows = summary["things_by_type"]
            .as_array()
            .expect("things_by_type is an array");
        assert_eq!(rows.len(), 50, "sixty distinct kinds truncate");
        assert_eq!(
            rows.first().expect("a first row"),
            &json!({ "type": 42_i64, "count": 2_u64 }),
            "the doubled kind leads",
        );
        let types = reported_types(&summary);
        assert_eq!(
            types.get(1).copied(),
            Some(10),
            "then types ascend in a count tie"
        );
        assert_eq!(types.last().copied(), Some(59), "cut after fifty rows");
        assert!(!types.contains(&60), "the kind after 59 was truncated away");
    }

    #[test]
    fn vanilla_node_lumps_count_their_records_and_sizes() {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.other = vec![
            lump("SEGS", &[0_u8; 24]),
            lump("SSECTORS", &[0_u8; 8]),
            lump("NODES", &[1_u8; 84]),
            lump("REJECT", &[0_u8; 4]),
            lump("BLOCKMAP", &[0_u8; 10]),
        ];
        assert_eq!(
            value_of(&map)["nodes"],
            json!({
                "present": true, "segs": 2_u64, "subsectors": 2_u64, "nodes": 3_u64,
                "reject_bytes": 4_u64, "blockmap_bytes": 10_u64, "znodes": false, "gl": false,
            }),
            "records and raw sizes of a built block",
        );
    }

    #[test]
    fn an_extended_nodes_lump_reports_its_magic_instead_of_counts() {
        let mut extended = Map::new_empty(name("MAP01"), MapFormat::Doom);
        extended.other = vec![
            marker("SEGS"),
            lump("NODES", b"XNOD rest of an extended lump"),
        ];
        let summary = value_of(&extended);
        assert_eq!(summary["nodes"]["nodes_format"], json!("XNOD"), "the magic");
        assert_eq!(summary["nodes"]["present"], json!(true), "it carries bytes");
        for key in ["segs", "subsectors", "nodes"] {
            assert!(
                summary["nodes"].get(key).is_none(),
                "the counts are replaced: {key} must be absent",
            );
        }
        let mut lowercase = Map::new_empty(name("MAP01"), MapFormat::Doom);
        lowercase.other = vec![lump("NODES", b"xnod but lowercase is plain data")];
        let plain = value_of(&lowercase)["nodes"].clone();
        assert_eq!(
            plain["nodes"],
            json!(1_u64),
            "no magic: 32 bytes hold one record",
        );
        assert!(plain.get("nodes_format").is_none(), "and no format key");
    }

    #[test]
    fn znodes_and_gl_lumps_report_presence() {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.other = vec![
            marker("NODES"),
            marker("ZNODES"),
            marker("GL_MAP01"),
            marker("GL_VERT"),
        ];
        let summary = value_of(&map)["nodes"].clone();
        assert_eq!(summary["znodes"], json!(true), "the ZNODES lump exists");
        assert_eq!(summary["gl"], json!(true), "a GL_ run is attached");
        assert_eq!(
            summary["present"],
            json!(false),
            "but neither carries bytes"
        );
    }
}
