//! The one-pass per-sector geometry index [`super`]'s free functions
//! answer from.
//!
//! [`SectorIndex::new`] walks `map.linedefs` exactly once, resolving every
//! line's two sides and its endpoints through the [`Map`] reference checks,
//! and stores per sector exactly what the free functions used to recompute
//! scan by scan: the boundary line pairs, the boundary segments that carry
//! geometry, the bounding box, the neighbour sectors and the containment
//! answer. The edge-case semantics are the scans' own: references that do
//! not resolve contribute nothing, and a line whose two resolved sides are
//! the same sector appears in the boundary once, flagged front, excluded
//! from the containment segments.
//!
//! Point lookup runs through [`SectorIndex::point_candidates`]: a sector
//! cannot contain a point outside its bounding box, so the box is the
//! exact pre-filter and only covering sectors get the even-odd test.
//!
//! [`Map`]: crate::wad::map::Map

use super::{Segment, same_sector_sides, widen};
use crate::wad::map::model::{Endpoints, Linedef, Map};

/// One linedef read once: both resolved sides and both reads of its
/// geometry, shared by every sector the line bounds.
struct LineSides {
    /// The sector behind the front sidedef, `None` when the side or its
    /// references dangle.
    front: Option<usize>,
    /// The sector behind the back sidedef, `None` when the side or its
    /// references dangle.
    back: Option<usize>,
    /// The line's endpoints, `None` when either vertex dangles; counted
    /// into the boundary box even when both sides share a sector.
    endpoints: Option<Endpoints>,
    /// The boundary segment: the endpoints when the line carries geometry,
    /// `None` when a vertex dangles or the two sides share a sector.
    segment: Option<Segment>,
}

impl LineSides {
    /// The sides that bound a sector, front first: the front side whenever
    /// it resolves, and the back side only when it resolves to a *different*
    /// sector - a line whose two sides share one sector bounds it once,
    /// flagged front.
    fn touching(&self, line: usize) -> (Option<Side>, Option<Side>) {
        let front = self.front.map(|sector| Side {
            sector,
            line,
            is_front: true,
        });
        let back = self
            .back
            .filter(|sector| Some(*sector) != self.front)
            .map(|sector| Side {
                sector,
                line,
                is_front: false,
            });
        (front, back)
    }
}

/// One side of one linedef, as it is taken into a sector's row.
struct Side {
    /// The sector this side of the line opens into.
    sector: usize,
    /// The line's index in `map.linedefs`.
    line: usize,
    /// Whether this is the line's front side.
    is_front: bool,
}

/// What the one pass collected for one sector.
#[derive(Clone, Debug, Default)]
struct SectorRow {
    /// The lines with a side in this sector as `(linedef index, is_front)`
    /// pairs, in linedef order.
    boundary: Vec<(usize, bool)>,
    /// The boundary lines that carry geometry, each with its `is_front`
    /// flag and its endpoints widened to `i64`.
    segments: Vec<(Segment, bool)>,
    /// The box covering the boundary vertices, `None` while nothing
    /// resolvable has been counted.
    bbox: Option<(i32, i32, i32, i32)>,
    /// The sectors across this sector's two-sided lines, grown unsorted
    /// and settled by [`SectorRow::finish`].
    neighbours: Vec<usize>,
}

impl SectorRow {
    /// Take one boundary side into the row.
    fn take(&mut self, side: &Side, sides: &LineSides) {
        self.boundary.push((side.line, side.is_front));
        if let Some(endpoints) = sides.endpoints {
            self.bbox = Some(widen(self.bbox, endpoints));
        }
        if let Some(segment) = sides.segment {
            self.segments.push((segment, side.is_front));
        }
        let across = if side.is_front {
            sides.back
        } else {
            sides.front
        };
        if let Some(neighbour) = across.filter(|other| *other != side.sector) {
            self.neighbours.push(neighbour);
        }
    }

    /// Sort and deduplicate the neighbour list the pass grew unsorted.
    fn finish(&mut self) {
        self.neighbours.sort_unstable();
        self.neighbours.dedup();
    }
}

/// Read one line's both sides and both geometry resolutions, once.
fn read_sides(map: &Map, line: &Linedef) -> LineSides {
    let (front, back) = map.line_sectors(line);
    let endpoints = map.line_endpoints(line);
    let segment = endpoints
        .filter(|_| !same_sector_sides(map, line))
        .map(Segment::from_endpoints);
    LineSides {
        front,
        back,
        endpoints,
        segment,
    }
}

/// Write one line into every sector's row it bounds.
fn observe(rows: &mut [SectorRow], map: &Map, line: usize, definition: &Linedef) {
    let sides = read_sides(map, definition);
    let (front, back) = sides.touching(line);
    for side in front.into_iter().chain(back) {
        if let Some(row) = rows.get_mut(side.sector) {
            row.take(&side, &sides);
        }
    }
}

/// The exact `i128` product of two `i64` differences (map coordinates
/// make the true products far smaller than `i64::MAX` squared).
fn wide_product(a: i64, b: i64) -> i128 {
    i128::from(a)
        .checked_mul(i128::from(b))
        .unwrap_or(i128::MAX)
}

/// Whether `x`/`y` lies on `segment`, endpoints included: within the
/// segment's box and collinear through the cross product.
fn point_on_segment(x: i64, y: i64, segment: &Segment) -> bool {
    let within_x = x >= segment.x1.min(segment.x2) && x <= segment.x1.max(segment.x2);
    let within_y = y >= segment.y1.min(segment.y2) && y <= segment.y1.max(segment.y2);
    let (Some(dx), Some(dy), Some(px), Some(py)) = (
        segment.x2.checked_sub(segment.x1),
        segment.y2.checked_sub(segment.y1),
        x.checked_sub(segment.x1),
        y.checked_sub(segment.y1),
    ) else {
        return false;
    };
    within_x && within_y && wide_product(px, dy) == wide_product(py, dx)
}

/// Whether the ray from `x`/`y` straight right crosses `segment` at a
/// point strictly to the right of `x` (the even-odd crossing test).
fn crosses_ray(x: i64, y: i64, segment: &Segment) -> bool {
    if (segment.y1 > y) == (segment.y2 > y) {
        return false;
    }
    let (Some(dx), Some(dy), Some(px), Some(py)) = (
        segment.x2.checked_sub(segment.x1),
        segment.y2.checked_sub(segment.y1),
        x.checked_sub(segment.x1),
        y.checked_sub(segment.y1),
    ) else {
        return false;
    };
    // Intersection x > x iff (x - x1) * dy is below (y - y1) * dx for an
    // upward segment and above it for a downward one.
    if dy > 0 {
        wide_product(px, dy) < wide_product(py, dx)
    } else {
        wide_product(px, dy) > wide_product(py, dx)
    }
}

/// Whether `segments` hold `x`/`y`: an even-odd ray-crossing count; when
/// the point sits exactly on the boundary the front-side rule decides -
/// the holder is the sector that fronts a line the point is on.
fn contains_over(segments: &[(Segment, bool)], x: i64, y: i64) -> bool {
    let mut inside = false;
    let mut on_boundary = false;
    let mut on_front = false;
    for &(segment, is_front) in segments {
        if point_on_segment(x, y, &segment) {
            on_boundary = true;
            on_front |= is_front;
            continue;
        }
        if crosses_ray(x, y, &segment) {
            inside = !inside;
        }
    }
    if on_boundary {
        return on_front;
    }
    inside
}

/// The area of `sector`'s bounding box when it holds `x`/`y`, `None`
/// when it does not hold the point.
fn held_area(index: &SectorIndex, sector: usize, x: i64, y: i64) -> Option<i128> {
    if !index.contains_point(sector, x, y) {
        return None;
    }
    index.bbox(sector).map(bbox_area)
}

/// The winner between the current best and a new candidate: the smaller
/// box area takes, a tie keeps the earlier sector.
fn better(current: Option<(i128, usize)>, candidate: (i128, usize)) -> Option<(i128, usize)> {
    if current.is_none_or(|(best_area, _)| candidate.0 < best_area) {
        return Some(candidate);
    }
    current
}

/// Sweep `index`'s covering candidates for `x`/`y` and answer the holder
/// with the smallest bounding-box area, ties by the lower sector index.
fn sweep(index: &SectorIndex, x: i64, y: i64) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for sector in index.point_candidates(x, y) {
        let Some(area) = held_area(index, sector, x, y) else {
            continue;
        };
        best = better(best, (area, sector));
    }
    best.map(|(_, sector)| sector)
}

/// The area of a bounding box in square map units, exact in `i128`.
fn bbox_area((min_x, min_y, max_x, max_y): (i32, i32, i32, i32)) -> i128 {
    let width = i128::from(max_x)
        .checked_sub(i128::from(min_x))
        .unwrap_or(i128::MAX);
    let height = i128::from(max_y)
        .checked_sub(i128::from(min_y))
        .unwrap_or(i128::MAX);
    width.checked_mul(height).unwrap_or(i128::MAX)
}

/// Per-sector geometry of one [`Map`], read in a single pass over its
/// linedefs.
///
/// Every answer is a stored fact of that pass, identical to what the
/// free functions in [`super`] compute by scanning: see
/// [`SectorIndex::boundary`], [`SectorIndex::bbox`],
/// [`SectorIndex::neighbours`], [`SectorIndex::contains_point`] and
/// [`SectorIndex::point_in_sector`]. A sector index the map does not have
/// simply bounds nothing: every read answers empty.
///
/// [`Map`]: crate::wad::map::Map
#[derive(Clone, Debug)]
pub struct SectorIndex {
    /// One row per sector of the indexed map, in sector order.
    rows: Vec<SectorRow>,
}

impl SectorIndex {
    /// Index `map` in one pass over `map.linedefs`: every line is read
    /// once and written into the row of each sector it bounds.
    #[must_use]
    #[inline]
    pub fn new(map: &Map) -> Self {
        let mut rows: Vec<SectorRow> = vec![SectorRow::default(); map.sectors.len()];
        for (line, definition) in map.linedefs.iter().enumerate() {
            observe(&mut rows, map, line, definition);
        }
        for row in &mut rows {
            row.finish();
        }
        Self { rows }
    }

    /// The row of `sector`, `None` when the map holds no such sector.
    fn row(&self, sector: usize) -> Option<&SectorRow> {
        self.rows.get(sector)
    }

    /// The linedefs having a side in `sector`, as `(linedef index,
    /// is_front)` pairs in linedef order; a line whose two sides both sit
    /// in `sector` appears once, flagged front.
    #[must_use]
    #[inline]
    pub fn boundary(&self, sector: usize) -> &[(usize, bool)] {
        self.row(sector).map_or(&[], |row| &row.boundary)
    }

    /// The boundary segments of `sector` that carry geometry, each with
    /// its `is_front` flag: unresolvable lines and lines whose two sides
    /// share the sector are skipped, exactly as the containment tests
    /// skip them.
    #[must_use]
    #[inline]
    pub(super) fn segments(&self, sector: usize) -> &[(Segment, bool)] {
        self.row(sector).map_or(&[], |row| &row.segments)
    }

    /// The bounding box `(min_x, min_y, max_x, max_y)` of the vertices on
    /// `sector`'s boundary, or `None` when the sector bounds nothing with
    /// a resolvable vertex.
    #[must_use]
    #[inline]
    pub fn bbox(&self, sector: usize) -> Option<(i32, i32, i32, i32)> {
        self.row(sector).and_then(|row| row.bbox)
    }

    /// The sectors across `sector`'s two-sided boundary lines, sorted and
    /// deduplicated; lines whose two sides share a sector contribute
    /// nothing.
    #[must_use]
    #[inline]
    pub fn neighbours(&self, sector: usize) -> &[usize] {
        self.row(sector).map_or(&[], |row| &row.neighbours)
    }

    /// The number of boundary segments `sector` carries: the work the
    /// containment test walks, for callers that bound it.
    #[must_use]
    #[inline]
    pub fn degree(&self, sector: usize) -> usize {
        self.segments(sector).len()
    }

    /// Whether `sector` contains `x`/`y`: an even-odd ray-crossing count
    /// against its boundary segments; when the point sits exactly on the
    /// boundary the front-side rule decides - the sector holds the point
    /// only if it fronts a line the point is on.
    #[must_use]
    #[inline]
    pub fn contains_point(&self, sector: usize, x: i64, y: i64) -> bool {
        contains_over(self.segments(sector), x, y)
    }

    /// The sectors whose bounding box covers `x`/`y`, in sector order:
    /// the exact pre-filter for [`SectorIndex::contains_point`], since a
    /// sector cannot contain a point outside its box.
    #[inline]
    pub fn point_candidates(&self, x: i64, y: i64) -> impl Iterator<Item = usize> + '_ {
        (0..self.rows.len()).filter(move |sector| self.covers(*sector, x, y))
    }

    /// The sector containing the point `x`/`y`, or `None` when no sector
    /// holds it: [`SectorIndex::contains_point`] over
    /// [`SectorIndex::point_candidates`] only, settled by the current
    /// winner rule - of the nested holders the one with the smallest
    /// bounding-box area wins, ties by the lower sector index.
    #[must_use]
    #[inline]
    pub fn point_in_sector(&self, x: i64, y: i64) -> Option<usize> {
        sweep(self, x, y)
    }

    /// Whether `sector`'s box covers `x`/`y`; a sector with no box covers
    /// nothing.
    fn covers(&self, sector: usize, x: i64, y: i64) -> bool {
        self.bbox(sector)
            .is_some_and(|(min_x, min_y, max_x, max_y)| {
                i64::from(min_x) <= x
                    && x <= i64::from(max_x)
                    && i64::from(min_y) <= y
                    && y <= i64::from(max_y)
            })
    }
}

#[cfg(test)]
mod tests;
