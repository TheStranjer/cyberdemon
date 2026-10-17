//! The `BLOCKMAP` builder: the map's spatial index, computed from geometry.
//!
//! [`build`] derives the vanilla grid from the vertex bounds — origins 8
//! map units below the lowest vertex, 128-unit cells counted up to the
//! highest — and sorts every linedef into the blocks whose square it
//! touches. The touch test is the conservative one engines accept: a line
//! is skipped only when all four block corners lie strictly on the same
//! side of its infinite line (`i128` cross products, so no float appears
//! and no product overflows), and the blocks offered the test are the
//! linedef's bounding-box block span, widened by one where the box edge
//! sits exactly on a block edge: a line on a shared block edge is listed
//! in both neighbours, and one crossing through a block corner lands in
//! every block meeting at that corner. Blocklists hold the linedef
//! indices ascending; [`Blockmap::to_bytes`] finishes the lump, sharing
//! one copy of identical lists when `compress` is set. A linedef whose
//! vertex references do not resolve contributes to no block. Nothing here
//! knows about sessions or MCP.

use crate::error::CyberError;
use crate::wad::map::blockmap::BLOCK_SIZE;
use crate::wad::map::{Blockmap, Linedef, Map};

/// Words in the blockmap header (two origins and the grid size).
const HEADER_WORDS: usize = 4;
/// The largest `BLOCKMAP` lump its `u16` word offsets can address.
const MAX_LUMP_WORDS: usize = 0xFFFF_usize;
/// The largest lump vanilla `Doom` reads whole (it signs the offsets).
const VANILLA_MAX_WORDS: usize = 0x7FFF_usize;
/// The margin the grid keeps below the lowest vertex, in map units.
const MARGIN: i32 = 8_i32;
/// The highest linedef index a blocklist can carry (`0xFFFF` is the list
/// terminator, never an entry).
const MAX_LISTED_INDEX: u16 = 0xFFFE_u16;
/// Bytes in one blocklist word.
const WORD_BYTES: usize = 2;

/// What one [`build`] produced: the finished lump and the grid facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockmapBuild {
    /// The `BLOCKMAP` lump bytes.
    pub bytes: Vec<u8>,
    /// `X` of block `(0, 0)`'s left edge.
    pub origin_x: i16,
    /// `Y` of block `(0, 0)`'s bottom edge.
    pub origin_y: i16,
    /// Blocks per row.
    pub columns: u16,
    /// Rows in the grid.
    pub rows: u16,
    /// Whether the lump stays inside the 32767 words vanilla reads.
    pub vanilla_safe: bool,
}

/// A linedef with both endpoints resolved.
#[derive(Clone, Copy, Debug)]
struct Ends {
    /// Start vertex `X`.
    x1: i32,
    /// Start vertex `Y`.
    y1: i32,
    /// End vertex `X`.
    x2: i32,
    /// End vertex `Y`.
    y2: i32,
}

/// The grid derived from the map's vertex bounds.
#[derive(Clone, Copy, Debug)]
struct Grid {
    /// `X` of block `(0, 0)`'s left edge.
    origin_x: i16,
    /// `Y` of block `(0, 0)`'s bottom edge.
    origin_y: i16,
    /// Blocks per row.
    columns: u16,
    /// Rows in the grid.
    rows: u16,
}

/// One linedef's placement task: the grid, its endpoints, the block
/// ranges its bounding box spans and the entry to list.
#[derive(Clone, Copy, Debug)]
struct Placement {
    /// The grid the line is sorted into.
    grid: Grid,
    /// The resolved endpoints.
    ends: Ends,
    /// The first and last column the box touches.
    cols: (usize, usize),
    /// The first and last row the box touches.
    rows: (usize, usize),
    /// The linedef index as the blocklists store it.
    number: u16,
}

/// Rebuilds `map`'s `BLOCKMAP`: every linedef into every block whose
/// square it touches. `compress` shares one copy of identical blocklists,
/// the vanilla-friendly default that shrinks the lump.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the map holds no vertices;
/// [`CyberError::LimitExceeded`] ("blockmap too large") when the grid, a
/// linedef index or the finished lump leaves the vanilla `BLOCKMAP`
/// shape.
#[inline]
pub fn build(map: &Map, compress: bool) -> Result<BlockmapBuild, CyberError> {
    let grid = Grid::of(map)?;
    let parsed = Blockmap {
        origin_x: grid.origin_x,
        origin_y: grid.origin_y,
        columns: grid.columns,
        rows: grid.rows,
        blocks: block_lists(map, grid)?,
    };
    let bytes = parsed.to_bytes(compress)?;
    let words = bytes.len().checked_div(WORD_BYTES).unwrap_or(usize::MAX);
    Ok(BlockmapBuild {
        origin_x: parsed.origin_x,
        origin_y: parsed.origin_y,
        columns: parsed.columns,
        rows: parsed.rows,
        vanilla_safe: words <= VANILLA_MAX_WORDS,
        bytes,
    })
}

impl Grid {
    /// The vanilla grid over `map`'s vertex bounds.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] on a map without vertices;
    /// [`CyberError::LimitExceeded`] when an origin leaves `i16`, a cell
    /// count leaves `u16`, or the offset table alone leaves the lump.
    fn of(map: &Map) -> Result<Self, CyberError> {
        let (min_x, min_y, max_x, max_y) = map.bounds().ok_or_else(|| {
            CyberError::InvalidArgument(
                "the map holds no vertices: a BLOCKMAP indexes the map's linedefs, \
                 and there is no geometry to index yet"
                    .to_owned(),
            )
        })?;
        let origin_x = origin_of(min_x)?;
        let origin_y = origin_of(min_y)?;
        let columns = extent(max_x, origin_x)?;
        let rows = extent(max_y, origin_y)?;
        let cells = usize::from(columns)
            .checked_mul(usize::from(rows))
            .ok_or_else(too_large)?;
        HEADER_WORDS
            .checked_add(cells)
            .filter(|end| *end <= MAX_LUMP_WORDS)
            .ok_or_else(too_large)?;
        Ok(Self {
            origin_x,
            origin_y,
            columns,
            rows,
        })
    }

    /// `columns × rows`, always in range because [`Grid::of`] checked the
    /// offset table.
    #[must_use]
    fn cells(self) -> usize {
        usize::from(self.columns).saturating_mul(usize::from(self.rows))
    }
}

/// One axis's grid origin: the lowest vertex minus the 8-unit margin.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the shift leaves `i16`.
fn origin_of(min: i32) -> Result<i16, CyberError> {
    let raw = min.checked_sub(MARGIN).ok_or_else(too_large)?;
    i16::try_from(raw).map_err(|_error| too_large())
}

/// The number of 128-unit cells from the origin `base` up to `max`, at
/// least one.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the count leaves `u16`.
fn extent(max: i32, base: i16) -> Result<u16, CyberError> {
    let span = i64::from(max)
        .checked_sub(i64::from(base))
        .ok_or_else(too_large)?;
    let cells = span
        .checked_div(i64::from(BLOCK_SIZE))
        .ok_or_else(too_large)?
        .checked_add(1)
        .ok_or_else(too_large)?;
    u16::try_from(cells).map_err(|_error| too_large())
}

/// The world-space edge of block `index` on an axis, or `None` when the
/// index or the arithmetic leaves `i32`.
fn edge(base: i16, index: usize) -> Option<i32> {
    let units = i32::try_from(index).ok()?.checked_mul(BLOCK_SIZE)?;
    i32::from(base).checked_add(units)
}

/// The first and last block index the closed interval `lo..=hi` touches
/// on one axis of a grid of `count` cells. The low end counts the cell
/// whose far edge the interval touches exactly, which is what puts a line
/// on a shared block edge into both neighbours; the high end is the plain
/// containing block.
fn span(lo: i32, hi: i32, base: i16, count: u16) -> Option<(usize, usize)> {
    let from = i64::from(lo).checked_sub(i64::from(base))?.checked_sub(1)?;
    let to = i64::from(hi).checked_sub(i64::from(base))?;
    let last = i64::from(count.saturating_sub(1_u16));
    let first = from.checked_div(i64::from(BLOCK_SIZE))?.max(0);
    let final_block = to.checked_div(i64::from(BLOCK_SIZE))?.min(last);
    if first > final_block {
        return None;
    }
    Some((
        usize::try_from(first).ok()?,
        usize::try_from(final_block).ok()?,
    ))
}

/// The per-block lists of `grid`, every resolvable linedef of `map` in
/// the blocks it touches, indices ascending, in row-major order.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] on a linedef index past [`MAX_LISTED_INDEX`].
fn block_lists(map: &Map, grid: Grid) -> Result<Vec<Vec<u16>>, CyberError> {
    let mut blocks = vec![Vec::new(); grid.cells()];
    for (index, line) in map.linedefs.iter().enumerate() {
        add_line(grid, &mut blocks, index, ends_of(map, line))?;
    }
    Ok(blocks)
}

/// The linedef's endpoints, when both vertex references resolve.
fn ends_of(map: &Map, line: &Linedef) -> Option<Ends> {
    let start = map.vertices.get(line.v1)?;
    let stop = map.vertices.get(line.v2)?;
    Some(Ends {
        x1: start.x,
        y1: start.y,
        x2: stop.x,
        y2: stop.y,
    })
}

/// Sorts one linedef into its blocks. An unresolvable line contributes to
/// no block; an index past the list field's range is the one refusal.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] on a linedef index past [`MAX_LISTED_INDEX`].
fn add_line(
    grid: Grid,
    blocks: &mut [Vec<u16>],
    index: usize,
    ends: Option<Ends>,
) -> Result<(), CyberError> {
    let number = u16::try_from(index).map_err(|_error| oversized(index))?;
    if number > MAX_LISTED_INDEX {
        return Err(oversized(index));
    }
    let task = ends.and_then(|pts| Placement::new(grid, pts, number));
    if let Some(job) = task {
        spread(&job, blocks);
    }
    Ok(())
}

impl Placement {
    /// The block ranges of one line's bounding box, or `None` when an
    /// axis falls outside the grid (which the vertex bounds rule out).
    fn new(grid: Grid, ends: Ends, number: u16) -> Option<Self> {
        let lo_x = ends.x1.min(ends.x2);
        let hi_x = ends.x1.max(ends.x2);
        let lo_y = ends.y1.min(ends.y2);
        let hi_y = ends.y1.max(ends.y2);
        let cols = span(lo_x, hi_x, grid.origin_x, grid.columns)?;
        let rows = span(lo_y, hi_y, grid.origin_y, grid.rows)?;
        Some(Self {
            grid,
            ends,
            cols,
            rows,
            number,
        })
    }
}

/// Offers the linedef to every block its bounding-box range covers.
fn spread(placement: &Placement, blocks: &mut [Vec<u16>]) {
    for row in placement.rows.0..=placement.rows.1 {
        for col in placement.cols.0..=placement.cols.1 {
            record(placement, blocks, col, row);
        }
    }
}

/// Lists the linedef in cell `(col, row)` when its square is touched.
fn record(placement: &Placement, blocks: &mut [Vec<u16>], col: usize, row: usize) {
    let Some(bx) = edge(placement.grid.origin_x, col) else {
        return;
    };
    let Some(by) = edge(placement.grid.origin_y, row) else {
        return;
    };
    if !touches(placement.ends, bx, by) {
        return;
    }
    let at = row
        .saturating_mul(usize::from(placement.grid.columns))
        .saturating_add(col);
    if let Some(cell) = blocks.get_mut(at) {
        cell.push(placement.number);
    }
}

/// Whether the segment touches the closed block square whose
/// bottom-left corner is `(bx, by)`: `false` only when all four corners
/// lie strictly on the same side of the segment's infinite line, so a
/// corner sitting on that line (the shared-edge and corner-crossing
/// cases) accepts. Arithmetic that leaves every addressable range
/// accepts, the conservative direction.
fn touches(ends: Ends, bx: i32, by: i32) -> bool {
    let Some(line) = Line::of(ends) else {
        return true;
    };
    let Some(right) = bx.checked_add(BLOCK_SIZE) else {
        return true;
    };
    let Some(top) = by.checked_add(BLOCK_SIZE) else {
        return true;
    };
    let mut positive = false;
    let mut negative = false;
    for corner in [(bx, by), (right, by), (bx, top), (right, top)] {
        if touches_at(&line, corner, &mut positive, &mut negative) {
            return true;
        }
    }
    positive && negative
}

/// One corner's contribution: `true` when it sits exactly on the line
/// (which accepts the block), otherwise the side flags grow.
fn touches_at(line: &Line, corner: (i32, i32), positive: &mut bool, negative: &mut bool) -> bool {
    let cross = line
        .side_at(i64::from(corner.0), i64::from(corner.1))
        .unwrap_or(0_i128);
    if cross == 0_i128 {
        return true;
    }
    *positive |= cross > 0_i128;
    *negative |= cross < 0_i128;
    false
}

/// The segment as a start point and direction widened to `i64`.
#[derive(Clone, Copy, Debug)]
struct Line {
    /// Start `X`.
    px: i64,
    /// Start `Y`.
    py: i64,
    /// End `X` minus start `X`.
    dx: i64,
    /// End `Y` minus start `Y`.
    dy: i64,
}

impl Line {
    /// The segment widened, or `None` when a difference overflows.
    fn of(ends: Ends) -> Option<Self> {
        let px = i64::from(ends.x1);
        let py = i64::from(ends.y1);
        Some(Self {
            px,
            py,
            dx: i64::from(ends.x2).checked_sub(px)?,
            dy: i64::from(ends.y2).checked_sub(py)?,
        })
    }

    /// The exact cross product of the direction with the offset of
    /// `(x, y)` from the start: positive left of the line, negative
    /// right, zero on it.
    fn side_at(&self, x: i64, y: i64) -> Option<i128> {
        let rise = i128::from(self.dx).checked_mul(i128::from(y.checked_sub(self.py)?))?;
        let run = i128::from(self.dy).checked_mul(i128::from(x.checked_sub(self.px)?))?;
        rise.checked_sub(run)
    }
}

/// The documented refusal of a grid or lump the format cannot hold.
fn too_large() -> CyberError {
    CyberError::LimitExceeded(
        "blockmap too large: the grid or its blocklists leave the 65535 words a \
         BLOCKMAP lump can hold"
            .to_owned(),
    )
}

/// The refusal of a linedef index no blocklist can carry.
fn oversized(index: usize) -> CyberError {
    CyberError::LimitExceeded(format!(
        "blockmap too large: linedef {index} cannot be listed (blocklist entries \
         end at {MAX_LISTED_INDEX}); the map holds more linedefs than a BLOCKMAP can index"
    ))
}

#[cfg(test)]
mod tests {
    use super::{MAX_LUMP_WORDS, VANILLA_MAX_WORDS, build};
    use crate::wad::map::common::fixtures::{name, two_room_map, vertex};
    use crate::wad::map::{Blockmap, Linedef, Map};

    /// The list of cells whose block holds `number`.
    fn cells_holding(parsed: &Blockmap, number: u16) -> Vec<usize> {
        parsed
            .blocks
            .iter()
            .enumerate()
            .filter(|&(_, lines)| lines.contains(&number))
            .map(|(cell, _)| cell)
            .collect()
    }

    /// A plain `Doom` map over `points` with one line per `(v1, v2)` pair.
    fn map_with(points: &[(i32, i32)], lines: &[(usize, usize)]) -> Map {
        Map {
            name: name("MAP01"),
            linedefs: lines
                .iter()
                .map(|&(v1, v2)| Linedef {
                    v1,
                    v2,
                    ..Default::default()
                })
                .collect(),
            vertices: points.iter().map(|&(x, y)| vertex(x, y)).collect(),
            ..Map::default()
        }
    }

    /// A map of `count` zero-length linedefs at the single vertex (64, 64).
    fn many_lines(count: usize) -> Map {
        Map {
            name: name("MAP01"),
            linedefs: (0..count).map(|_| Linedef::default()).collect(),
            vertices: vec![vertex(64, 64)],
            ..Map::default()
        }
    }

    /// Build `map` and parse the lump back.
    fn round_trip(map: &Map, compress: bool) -> Blockmap {
        let built = build(map, compress).expect("the test map builds");
        Blockmap::parse(&built.bytes).expect("the built lump parses")
    }

    #[test]
    fn the_two_room_grid_and_lists_match_the_hand_computation() {
        let built = build(&two_room_map(), true).expect("the fixture builds");
        assert_eq!(
            (built.origin_x, built.origin_y),
            (-8_i16, -8_i16),
            "origins are 8 below the lowest vertex"
        );
        assert_eq!(
            (built.columns, built.rows),
            (5_u16, 3_u16),
            "512x256 spans 5x3 blocks from -8"
        );
        assert!(built.vanilla_safe, "a toy map stays tiny");
        let parsed = Blockmap::parse(&built.bytes).expect("the built lump parses");
        let expect: Vec<Vec<u16>> = vec![
            vec![0, 3],
            vec![3],
            vec![2, 3, 6],
            vec![6],
            vec![5, 6],
            vec![0],
            vec![],
            vec![2],
            vec![],
            vec![5],
            vec![0, 1],
            vec![1],
            vec![1, 2, 4],
            vec![4],
            vec![4, 5],
        ];
        assert_eq!(parsed.blocks, expect, "the row-major block lists");
        // The step line (index 2) runs x = 256, y 256..0: 256 - (-8) = 264
        // sits inside column 2 (248..376), not on an edge, so it is listed
        // in exactly that column's three rows: cells 2, 7 and 12.
        assert_eq!(cells_holding(&parsed, 2), vec![2_usize, 7, 12], "linedef 2");
    }

    #[test]
    fn compression_shares_identical_lists_and_both_round_trip() {
        let plain = round_trip(&two_room_map(), false);
        let packed = round_trip(&two_room_map(), true);
        assert_eq!(packed, plain, "the two lumps say the same thing");
        let plain_bytes = build(&two_room_map(), false)
            .expect("plain build")
            .bytes
            .len();
        let packed_bytes = build(&two_room_map(), true)
            .expect("packed build")
            .bytes
            .len();
        assert!(
            packed_bytes < plain_bytes,
            "the three identical empty lists (and no other twin) share one copy: \
             {packed_bytes} < {plain_bytes}"
        );
        assert_eq!(plain.blocks.len(), 15, "5x3 cells");
        assert_eq!(
            plain.blocks.iter().filter(|lines| lines.is_empty()).count(),
            2,
            "the two interior corners are empty"
        );
    }

    #[test]
    fn a_line_on_a_shared_block_edge_is_listed_in_both_neighbours() {
        // x = 248 is exactly the edge between columns 1 and 2 of the
        // -8-origin grid; the rectangle walls keep it inside a 5x3 grid.
        let map = map_with(
            &[
                (0_i32, 0_i32),
                (512_i32, 0_i32),
                (512_i32, 256_i32),
                (0_i32, 256_i32),
                (248_i32, 0_i32),
                (248_i32, 256_i32),
            ],
            &[
                (0_usize, 1_usize),
                (1_usize, 2_usize),
                (2_usize, 3_usize),
                (3_usize, 0_usize),
                (4_usize, 5_usize),
            ],
        );
        let parsed = round_trip(&map, true);
        let expect: Vec<Vec<u16>> = vec![
            vec![0, 3],
            vec![0, 4],
            vec![0, 4],
            vec![0],
            vec![0, 1],
            vec![3],
            vec![4],
            vec![4],
            vec![],
            vec![1],
            vec![2, 3],
            vec![2, 4],
            vec![2, 4],
            vec![2],
            vec![1, 2],
        ];
        assert_eq!(parsed.blocks, expect, "edge line 4 in columns 1 and 2");
        assert_eq!(
            cells_holding(&parsed, 4),
            vec![1_usize, 2, 6, 7, 11, 12],
            "both neighbours of the shared edge, all three rows"
        );
    }

    #[test]
    fn a_diagonal_through_block_corners_lands_in_every_corner_cell() {
        // (0,0) to (256,256) passes through the corners (120,120) and
        // (248,248) of the -8-origin grid: accepted cells are the six the
        // strip runs through plus the corner touches, rejected only the
        // far corners (2,0) and (0,2).
        let map = map_with(&[(0_i32, 0_i32), (256_i32, 256_i32)], &[(0_usize, 1_usize)]);
        let parsed = round_trip(&map, true);
        assert_eq!((parsed.columns, parsed.rows), (3, 3), "the grid");
        let expect: Vec<Vec<u16>> = vec![
            vec![0],
            vec![0],
            vec![],
            vec![0],
            vec![0],
            vec![0],
            vec![],
            vec![0],
            vec![0],
        ];
        assert_eq!(parsed.blocks, expect, "cells 2 and 6 miss the line");
    }

    #[test]
    fn empty_maps_refuse_with_a_hint() {
        let error = build(&Map::default(), true).expect_err("no vertices to frame a grid");
        assert_eq!(error.code(), "invalid_argument", "empty map code");
        assert!(
            error.to_string().contains("vertices"),
            "the missing piece is named: {error}"
        );
    }

    #[test]
    fn dangling_vertex_references_contribute_to_no_block() {
        let map = map_with(&[(0_i32, 0_i32)], &[(0_usize, 99_usize)]);
        let parsed = round_trip(&map, true);
        assert_eq!((parsed.columns, parsed.rows), (1, 1), "the lone vertex");
        let expect: Vec<Vec<u16>> = vec![Vec::new()];
        assert_eq!(parsed.blocks, expect, "the unresolvable line is skipped");
    }

    #[test]
    fn a_linedef_past_the_list_field_is_refused() {
        let error =
            build(&many_lines(0x0001_0000), true).expect_err("index 65535 cannot be listed");
        assert_eq!(error.code(), "limit_exceeded", "wild index code");
        assert!(
            error.to_string().contains("linedef 65535"),
            "the offender is named: {error}"
        );
    }

    #[test]
    fn a_lump_beyond_the_addressable_size_is_refused() {
        let error = build(&many_lines(MAX_LUMP_WORDS.saturating_sub(4)), true)
            .expect_err("one list of 65531 entries leaves the lump");
        assert_eq!(error.code(), "limit_exceeded", "oversized lump code");
        assert!(
            error.to_string().contains("blockmap too large"),
            "the ticket phrase is kept: {error}"
        );
    }

    #[test]
    fn vanilla_safe_reports_the_signed_offset_boundary() {
        // One cell, one list: 4 header + 1 offset + 2 terminator words
        // plus the entries, so VANILLA_MAX_WORDS-6 entries is 32768
        // words, one past vanilla's signed reach, and one fewer entry is
        // exactly the last safe size.
        let past = build(&many_lines(VANILLA_MAX_WORDS.saturating_sub(6)), true)
            .expect("one word past vanilla builds");
        assert_eq!(
            past.bytes.len().checked_div(2_usize).expect("even"),
            VANILLA_MAX_WORDS.saturating_add(1),
            "one word past vanilla"
        );
        assert!(!past.vanilla_safe, "vanilla signs the offsets");
        let edge = build(&many_lines(VANILLA_MAX_WORDS.saturating_sub(7)), true)
            .expect("the vanilla edge builds");
        assert!(edge.vanilla_safe, "the last size vanilla reads whole");
    }

    #[test]
    fn grids_beyond_the_stored_shape_are_refused() {
        // 65536+ columns: the cell count leaves u16.
        let wide = map_with(
            &[(0_i32, 0_i32), (0x0080_0000_i32, 0_i32)],
            &[(0_usize, 1_usize)],
        );
        let wide_error = build(&wide, true).expect_err("65537 columns do not fit");
        assert_eq!(wide_error.code(), "limit_exceeded", "wide grid code");
        // An origin below i16::MIN cannot be stored either.
        let low = map_with(
            &[(i32::from(i16::MIN), 0_i32), (0_i32, 0_i32)],
            &[(0_usize, 1_usize)],
        );
        let low_error = build(&low, true).expect_err("one margin below i16::MIN leaves i16");
        assert_eq!(low_error.code(), "limit_exceeded", "sunken origin code");
        assert!(
            low_error.to_string().contains("too large"),
            "the fault is named: {low_error}"
        );
    }
}
