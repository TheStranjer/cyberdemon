//! Shared builders for the `glBSP` v2 writer's tests: the `GL` run wrapper,
//! the re-parsers of the `GL_SEGS`, `GL_SSECT` and `GL_VERT` lumps, and the
//! loop, direction and partner assertions every closed-run claim checks
//! against. The maps, points and segs these ride are the shared
//! [`crate::bsp::fixtures`] the builder tests use too.
//!
//! [`loops`] traces the region loops themselves, from one convex room to
//! the miniseg-filled doorway and the hairline snap; [`records`] reads the
//! record lump and every limit and `internal` refusal.

mod loops;
mod records;

use crate::bsp::GlNodes;
use crate::bsp::fixtures::tree_of;
use crate::bsp::gl::{GL_VERTEX_FLAG, MAGIC_GND2, NO_PARTNER, SEG_RECORD_LEN, to_gl};
use crate::bsp::records::scaled;
use crate::wad::map::Map;

/// The GL run of `map`'s tree.
pub(super) fn run_of(map: &Map) -> GlNodes {
    let tree = tree_of(map);
    to_gl(map, &tree).expect("the fixture serializes")
}

/// One parsed `GL_SEGS` row.
#[derive(Clone, Copy, Debug)]
pub(super) struct Row {
    /// First endpoint word, flag included.
    v1: u16,
    /// Second endpoint word, flag included.
    v2: u16,
    /// Linedef index or the miniseg sentinel.
    linedef: u16,
    /// Side word.
    side: u16,
    /// Partner index or the sentinel.
    partner: u16,
}

/// Read one `GL_SEGS` record off a cursor.
pub(super) fn read_row(reader: &mut crate::wad::bytes::Reader<'_>) -> Row {
    Row {
        v1: reader.read_u16().unwrap(),
        v2: reader.read_u16().unwrap(),
        linedef: reader.read_u16().unwrap(),
        side: reader.read_u16().unwrap(),
        partner: reader.read_u16().unwrap(),
    }
}

/// Parse the `GL_SEGS` lump back into rows.
pub(super) fn rows(run: &GlNodes) -> Vec<Row> {
    let mut reader = crate::wad::bytes::Reader::new(&run.segs);
    let mut out = Vec::new();
    while reader.remaining() >= SEG_RECORD_LEN {
        out.push(read_row(&mut reader));
    }
    out
}

/// The subsector ranges `(first, count)` of a written run.
pub(super) fn subsector_ranges(run: &GlNodes) -> Vec<(usize, usize)> {
    let mut reader = crate::wad::bytes::Reader::new(&run.ssectors);
    let mut out = Vec::new();
    while reader.remaining() >= 4 {
        let count = usize::from(reader.read_u16().unwrap());
        let first = usize::from(reader.read_u16().unwrap());
        out.push((first, count));
    }
    out
}

/// The `16.16` point a vertex word names.
pub(super) fn endpoint(map: &Map, run: &GlNodes, word: u16) -> (i64, i64) {
    if word & GL_VERTEX_FLAG == 0 {
        let v = &map.vertices[usize::from(word)];
        return (scaled(v.x), scaled(v.y));
    }
    let row = usize::from(word & !GL_VERTEX_FLAG);
    let offset = row.checked_mul(8).unwrap();
    let mut reader = crate::wad::bytes::Reader::at(&run.vertices, MAGIC_GND2.len());
    reader
        .seek(offset.checked_add(MAGIC_GND2.len()).unwrap())
        .unwrap();
    let x = i64::from(reader.read_i32().unwrap());
    let y = i64::from(reader.read_i32().unwrap());
    (x, y)
}

/// Require every subsector's seg loop to close and the leaves to
/// cover the whole pool.
pub(super) fn assert_loops_close(run: &GlNodes) {
    let all = rows(run);
    let ranges = subsector_ranges(run);
    let total: usize = ranges.iter().map(|entry| entry.1).sum();
    assert_eq!(total, all.len(), "the leaves cover every seg");
    for (first, count) in ranges {
        assert_cycle_closes(&all, first, count);
    }
}

/// One closed cycle: rows share endpoint words, end to start.
pub(super) fn assert_cycle_closes(all: &[Row], first: usize, count: usize) {
    assert!(count >= 3, "a region needs at least three segs");
    let window = &all[first..first.checked_add(count).unwrap()];
    let ends: Vec<u16> = window.iter().map(|row| row.v2).collect();
    let starts: Vec<u16> = window
        .iter()
        .cycle()
        .skip(1)
        .take(count)
        .map(|row| row.v1)
        .collect();
    assert_eq!(ends, starts, "subsector at seg {first} closes exactly");
}

/// The signed `16.16` shoelace of one loop, via its endpoints.
pub(super) fn loop_area(map: &Map, run: &GlNodes, first: usize, count: usize) -> i128 {
    let all = rows(run);
    let window = &all[first..first.checked_add(count).unwrap()];
    let corners: Vec<(i64, i64)> = window
        .iter()
        .map(|row| endpoint(map, run, row.v1))
        .collect();
    let mut sum = 0_i128;
    for pair in corners
        .iter()
        .zip(corners.iter().cycle().skip(1).take(count))
    {
        let (here, next) = pair;
        let hx = i128::from(here.0);
        let hy = i128::from(here.1);
        let nx = i128::from(next.0);
        let ny = i128::from(next.1);
        sum = sum.saturating_add(hx.saturating_mul(ny).saturating_sub(nx.saturating_mul(hy)));
    }
    sum
}

/// Require every loop to run clockwise (negative shoelace, y up).
pub(super) fn assert_loops_clockwise(map: &Map, run: &GlNodes) {
    for (first, count) in subsector_ranges(run) {
        assert!(
            loop_area(map, run, first, count) < 0_i128,
            "subsector at seg {first} runs clockwise"
        );
    }
}

/// Require the partner words to be symmetric and self-consistent.
pub(super) fn assert_partner_symmetry(rows: &[Row]) {
    let matched: Vec<(usize, usize)> = rows
        .iter()
        .enumerate()
        .filter(|pair| pair.1.partner != NO_PARTNER)
        .map(|pair| (pair.0, usize::from(pair.1.partner)))
        .collect();
    for (mine, theirs) in matched {
        let here = &rows[mine];
        let other = &rows[theirs];
        assert_eq!(
            other.partner,
            u16::try_from(mine).unwrap(),
            "partners face back"
        );
        assert_eq!(here.v1, other.v2, "partners reverse v1");
        assert_eq!(here.v2, other.v1, "partners reverse v2");
    }
}
