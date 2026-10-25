//! The pure `align_textures` engine: continuous wall textures.
//!
//! [`apply`] walks chains of connected walls — consecutive linedefs
//! sharing a vertex in drawing order (`previous.v2 == next.v1`) whose
//! front side carries the same wall texture — and rewrites the front
//! sidedef's offsets: `x_offset` becomes the chain distance before the
//! line modulo the texture width (so the texture runs unbroken around
//! corners), and `y_offset` is set so the texture's top aligns with the
//! highest ceiling in the chain (vanilla pegs an unset top to the front
//! ceiling and shifts it down by `y_offset`). Texture widths come from
//! the caller: this module never reads the archive.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::wad::classify::MapFormat;
use crate::wad::map::consts::normalized_picture;
use crate::wad::map::geometry::linedef_length;
use crate::wad::map::{Map, Sidedef};

/// The lowest offset an `i16` binary `SIDEDEFS` column holds (`i16::MIN`).
const OFFSET_MIN: i32 = -0x8000_i32;
/// The highest offset an `i16` binary `SIDEDEFS` column holds (`i16::MAX`).
const OFFSET_MAX: i32 = 0x7FFF_i32;
/// The most linedefs one chain may absorb (a guard against pathological
/// cycles in hand-built maps).
const CHAIN_BUDGET: usize = 100_000;

/// Everything one `align_textures` pass needs besides the map.
#[derive(Debug)]
pub struct AlignPlan<'plan> {
    /// The linedef indices to consider, in any order.
    pub targets: BTreeSet<usize>,
    /// Wall texture name to width in pixels, from the merged definitions.
    pub widths: &'plan BTreeMap<String, u32>,
    /// Whether to rewrite `x_offset`.
    pub do_x: bool,
    /// Whether to rewrite `y_offset`.
    pub do_y: bool,
}

/// What one alignment pass changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlignChange {
    /// Sidedefs whose offsets actually changed.
    pub aligned: usize,
    /// Lines the pass refused to move (and why), for the report.
    pub warnings: Vec<String>,
}

/// The front-side wall texture of a line: the middle of a one-sided line,
/// the upper of a two-sided one; `None` when that slot is empty or the
/// references do not resolve.
fn slot_texture(map: &Map, line: usize) -> Option<String> {
    let def = map.linedefs.get(line)?;
    let sidedef = def.front?;
    let side = map.sidedefs.get(sidedef)?;
    let name = if def.back.is_none() {
        &side.middle
    } else {
        &side.upper
    };
    normalized_picture(name)
}

/// Adjacency of the target lines: by start vertex and by end vertex.
struct Graph {
    /// Lines by their `v1` (drawing-order start).
    from: BTreeMap<usize, Vec<usize>>,
    /// Lines by their `v2` (drawing-order end).
    to: BTreeMap<usize, Vec<usize>>,
}

/// Index the target lines by start and end vertex.
fn build_graph(map: &Map, targets: &BTreeSet<usize>) -> Graph {
    let mut graph = Graph {
        from: BTreeMap::new(),
        to: BTreeMap::new(),
    };
    for line in targets {
        if let Some(def) = map.linedefs.get(*line) {
            graph.from.entry(def.v1).or_default().push(*line);
            graph.to.entry(def.v2).or_default().push(*line);
        }
    }
    graph
}

/// Apply `plan` to `map`: every maximal chain, from its head. Lines
/// already inside one chain are never realigned by another.
#[inline]
pub fn apply(map: &mut Map, plan: &AlignPlan<'_>) -> AlignChange {
    let graph = build_graph(map, &plan.targets);
    let mut change = AlignChange::default();
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    for start in &plan.targets {
        if visited.contains(start) || slot_texture(map, *start).is_none() {
            continue;
        }
        let head = chain_head(map, &graph, *start, &visited);
        let chain = walk_chain(map, &graph, head, &mut visited);
        align_chain(map, plan, &chain, &mut change);
    }
    change
}

/// The first unvisited predecessor of `start` (its chain head).
fn chain_head(map: &Map, graph: &Graph, start: usize, visited: &BTreeSet<usize>) -> usize {
    let mut head = start;
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    seen.insert(start);
    for _step in 0..CHAIN_BUDGET {
        let Some(name) = slot_texture(map, head) else {
            return head;
        };
        let Some(def) = map.linedefs.get(head) else {
            return head;
        };
        let Some(previous) =
            graph
                .to
                .get(&def.v1)
                .into_iter()
                .flatten()
                .copied()
                .find(|&candidate| {
                    !seen.contains(&candidate)
                        && !visited.contains(&candidate)
                        && slot_texture(map, candidate).as_deref() == Some(name.as_str())
                })
        else {
            return head;
        };
        seen.insert(previous);
        head = previous;
    }
    head
}

/// The chain from `head` forward: every connected, same-texture line in
/// drawing order. Marks every member visited.
fn walk_chain(map: &Map, graph: &Graph, head: usize, visited: &mut BTreeSet<usize>) -> Vec<usize> {
    let Some(name) = slot_texture(map, head) else {
        return Vec::new();
    };
    let mut chain = vec![head];
    visited.insert(head);
    let mut cursor = map.linedefs.get(head).map_or(usize::MAX, |def| def.v2);
    for _step in 0..CHAIN_BUDGET.saturating_sub(chain.len()) {
        let Some(next) =
            graph
                .from
                .get(&cursor)
                .into_iter()
                .flatten()
                .copied()
                .find(|&candidate| {
                    !visited.contains(&candidate)
                        && slot_texture(map, candidate).as_deref() == Some(name.as_str())
                })
        else {
            break;
        };
        chain.push(next);
        visited.insert(next);
        cursor = map.linedefs.get(next).map_or(usize::MAX, |def| def.v2);
    }
    chain
}

/// One member's pending offsets, keyed by its front sidedef.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Offsets {
    /// The new `x_offset`, when `x` alignment ran and the width was known.
    x: Option<i32>,
    /// The new `y_offset`, when `y` alignment ran.
    y: Option<i32>,
}

/// Compute and write one chain's offsets, counting changed sidedefs.
fn align_chain(map: &mut Map, plan: &AlignPlan<'_>, chain: &[usize], change: &mut AlignChange) {
    if chain.is_empty() {
        return;
    }
    let mut edits: BTreeMap<usize, Offsets> = BTreeMap::new();
    let mut notes = Vec::new();
    if plan.do_x {
        notes.extend(x_offsets(map, plan, chain, &mut edits));
    }
    if plan.do_y {
        y_offsets(map, chain, &mut edits);
    }
    change.warnings.extend(notes);
    let mut sink = Sink {
        binary: map.format != MapFormat::Udmf,
        warnings: &mut change.warnings,
    };
    for (line, offset) in edits {
        if write_offsets(map, line, offset, &mut sink) {
            change.aligned = change.aligned.saturating_add(1);
        }
    }
}

/// Where one offset lands: the format cap and the refusal notes.
struct Sink<'report> {
    /// Whether the map stores offsets as `i16`.
    binary: bool,
    /// Refusals collected for the report.
    warnings: &'report mut Vec<String>,
}

/// Which offset column of a sidedef.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The along-line shift.
    X,
    /// The across-line shift.
    Y,
}

impl Field {
    /// The column's wire name, for warnings.
    const fn as_str(self) -> &'static str {
        match self {
            Self::X => "x_offset",
            Self::Y => "y_offset",
        }
    }
}

/// Move one chain member's front sidedef; whether anything changed.
fn write_offsets(map: &mut Map, line: usize, offset: Offsets, sink: &mut Sink<'_>) -> bool {
    let Some(index) = map.linedefs.get(line).and_then(|def| def.front) else {
        return false;
    };
    let Some(side) = map.sidedefs.get_mut(index) else {
        return false;
    };
    let x = sink.store(side, (line, Field::X), offset.x);
    let y = sink.store(side, (line, Field::Y), offset.y);
    x || y
}

impl Sink<'_> {
    /// Write one candidate offset to its column unless the format cannot
    /// store it; whether the value landed on a sidedef that actually moved.
    fn store(&mut self, side: &mut Sidedef, slot: (usize, Field), value: Option<i32>) -> bool {
        let (line, field) = slot;
        let Some(stored) = value else {
            return false;
        };
        if self.binary && !fits(stored) {
            self.warnings.push(format!(
                "linedef {line} {} {stored} does not fit the binary i16 range \
                 (-32768..=32767): left unchanged",
                field.as_str()
            ));
            return false;
        }
        let current = match field {
            Field::X => &mut side.x_offset,
            Field::Y => &mut side.y_offset,
        };
        if *current == stored {
            return false;
        }
        *current = stored;
        true
    }
}

/// The running chain distance modulo the texture width, per line; the
/// refusals (an undefined texture has no width) as notes.
fn x_offsets(
    map: &Map,
    plan: &AlignPlan<'_>,
    chain: &[usize],
    edits: &mut BTreeMap<usize, Offsets>,
) -> Vec<String> {
    let Some(name) = chain.first().and_then(|line| slot_texture(map, *line)) else {
        return Vec::new();
    };
    let Some(&width) = plan.widths.get(&name) else {
        return vec![format!(
            "texture {name} has no TEXTURE1/TEXTURE2 definition: x offsets left unchanged (its \
             width is unknown)"
        )];
    };
    if width == 0_u32 {
        return Vec::new();
    }
    let mut distance = 0_u64;
    for line in chain {
        let offset = u32::try_from(distance.checked_rem(u64::from(width)).unwrap_or(0_u64))
            .ok()
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0_i32);
        edits.entry(*line).or_default().x = Some(offset);
        distance = distance
            .checked_add(linedef_length(map, *line).unwrap_or(0))
            .unwrap_or(distance);
    }
    Vec::new()
}

/// The highest front ceiling in the chain, applied per line.
fn y_offsets(map: &Map, chain: &[usize], edits: &mut BTreeMap<usize, Offsets>) {
    let Some(highest) = chain
        .iter()
        .filter_map(|line| front_ceiling(map, *line))
        .max()
    else {
        return;
    };
    for line in chain {
        if let Some(ceiling) = front_ceiling(map, *line) {
            edits.entry(*line).or_default().y = ceiling.checked_sub(highest);
        }
    }
}

/// The front sidedef's sector's ceiling height.
fn front_ceiling(map: &Map, line: usize) -> Option<i32> {
    let sidedef = map.linedefs.get(line)?.front?;
    let index = map.sidedefs.get(sidedef)?.sector;
    map.sectors.get(index).map(|placed| placed.ceiling_height)
}

/// Whether `value` survives in a binary map's offsets column.
#[must_use]
fn fits(value: i32) -> bool {
    (OFFSET_MIN..=OFFSET_MAX).contains(&value)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::collections::BTreeSet;

    use super::{AlignChange, AlignPlan, apply};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

    /// A vertex at `x`.
    fn vertex(x: i32) -> Vertex {
        Vertex {
            x,
            ..Default::default()
        }
    }

    /// A one-sided wall line from `v1` to `v2` with sidedef `side`.
    fn wall(v1: usize, v2: usize, side: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(side),
            ..Default::default()
        }
    }

    /// A sidedef at the given offsets carrying texture `name`.
    fn side(name: &str, x: i32, y: i32, sector: usize) -> Sidedef {
        Sidedef {
            x_offset: x,
            y_offset: y,
            middle: name.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// An L: a 100-unit east wall from the origin, then a 64-unit north
    /// wall; both carry `BRICK` (width 64) over sectors with ceilings
    /// 128 and 96. The stray third line carries a different texture.
    fn l_shape() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![vertex(0), vertex(100), vertex(100)],
            linedefs: vec![
                wall(0, 1, 0),
                wall(1, 2, 1),
                Linedef {
                    v1: 2,
                    v2: 0,
                    flags: 1,
                    front: Some(2),
                    ..Default::default()
                },
            ],
            sidedefs: vec![
                side("BRICK", 7, 5, 0),
                side("BRICK", 7, 5, 1),
                side("OTHER", 0, 0, 0),
            ],
            sectors: vec![
                Sector {
                    ceiling_height: 128,
                    ..Default::default()
                },
                Sector {
                    ceiling_height: 96,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// The widths table for the tests.
    fn widths() -> BTreeMap<String, u32> {
        [("BRICK".to_owned(), 64_u32), ("OTHER".to_owned(), 128_u32)]
            .into_iter()
            .collect()
    }

    /// Run the engine with both axes over every line.
    fn run(map: &mut Map) -> AlignChange {
        let table = widths();
        let plan = AlignPlan {
            targets: BTreeSet::from([0_usize, 1, 2]),
            widths: &table,
            do_x: true,
            do_y: true,
        };
        apply(map, &plan)
    }

    #[test]
    fn the_l_chain_accumulates_the_distance_modulo_the_width() {
        let mut map = l_shape();
        let change = run(&mut map);
        assert_eq!(
            map.sidedefs[0].x_offset, 0_i32,
            "the chain starts at distance 0"
        );
        assert_eq!(map.sidedefs[1].x_offset, 36_i32, "100 units in, 100 % 64");
        assert_eq!(
            map.sidedefs[2].x_offset, 0_i32,
            "the other texture is its own chain"
        );
        assert_eq!(
            change.aligned, 2,
            "the two brick sidedefs moved: {change:?}"
        );
    }

    #[test]
    fn y_alignment_pegs_the_texture_top_to_the_highest_ceiling() {
        let mut map = l_shape();
        run(&mut map);
        assert_eq!(
            map.sidedefs[0].y_offset, 0_i32,
            "the high ceiling is the reference"
        );
        assert_eq!(
            map.sidedefs[1].y_offset, -32_i32,
            "its top must rise 32 units"
        );
        assert_eq!(
            map.sidedefs[2].y_offset, 0_i32,
            "its chain reaches 128 anyway"
        );
    }

    #[test]
    fn a_chain_only_runs_in_drawing_order() {
        let mut map = l_shape();
        map.linedefs[1].v1 = 0;
        map.linedefs[1].v2 = 1;
        let change = run(&mut map);
        assert_eq!(change.aligned, 2, "both brick lines start their own chains");
        assert_eq!(
            map.sidedefs[1].x_offset, 0_i32,
            "a reversed line restarts the distance"
        );
    }

    #[test]
    fn an_unknown_texture_width_warns_and_leaves_x() {
        let mut map = l_shape();
        map.sidedefs[0].middle = "GHOST".to_owned();
        map.sidedefs[1].middle = "GHOST".to_owned();
        let table = widths();
        let plan = AlignPlan {
            targets: BTreeSet::from([0_usize, 1, 2]),
            widths: &table,
            do_x: true,
            do_y: false,
        };
        let change = apply(&mut map, &plan);
        assert_eq!(
            map.sidedefs[0].x_offset, 7_i32,
            "x untouched without a width"
        );
        assert_eq!(change.warnings.len(), 1, "one warning for the chain");
        assert_eq!(change.aligned, 0, "nothing moved");
    }

    #[test]
    fn offsets_beyond_the_binary_i16_range_warn_and_stay() {
        let mut map = l_shape();
        map.sectors[0].ceiling_height = 40_000_i32;
        map.sectors[1].ceiling_height = -40_000_i32;
        let change = run(&mut map);
        assert_eq!(map.sidedefs[1].y_offset, 5_i32, "out-of-range y stays put");
        assert!(
            change.warnings.iter().any(|w| w.contains("i16")),
            "the refusal is reported: {:?}",
            change.warnings
        );
    }

    #[test]
    fn udmf_maps_have_no_i16_cap() {
        let mut map = l_shape();
        map.format = MapFormat::Udmf;
        map.sectors[0].ceiling_height = 40_000_i32;
        map.sectors[1].ceiling_height = -40_000_i32;
        run(&mut map);
        assert_eq!(
            map.sidedefs[1].y_offset, -80_000_i32,
            "the full integer range"
        );
    }

    #[test]
    fn empty_slots_are_never_aligned() {
        let mut map = l_shape();
        map.sidedefs[0].middle = "-".to_owned();
        let change = run(&mut map);
        assert_eq!(map.sidedefs[0].x_offset, 7_i32, "no texture, no alignment");
        assert_eq!(change.aligned, 1, "only the surviving brick chain moved");
    }

    #[test]
    fn an_unset_slot_starts_no_chain_and_draws_no_warning() {
        let mut map = l_shape();
        map.sidedefs[0].middle.clear();
        map.sidedefs[1].middle.clear();
        let change = run(&mut map);
        assert_eq!(
            map.sidedefs[0].x_offset, 7_i32,
            "the unset face is left alone"
        );
        assert_eq!(
            change.aligned, 0,
            "the untouched OTHER chain already rests at 0"
        );
        assert!(
            change.warnings.is_empty(),
            "an unset face is skipped, not warned about: {:?}",
            change.warnings
        );
    }
}
