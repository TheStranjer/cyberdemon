//! The maps the identity tests compare against, and the deterministic
//! probe points they walk.

use crate::wad::map::MapFormat;
use crate::wad::map::common::fixtures::{edge, name, room, side, solid_line, two_room_map, vertex};
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::model::{Linedef, Map, Sidedef, Vertex};

/// A sector nested in a bigger one, the `geometry.rs` tests' map: an
/// outer square `0..1000` (sector 0) and an inner square `400..600`
/// (sector 1), all lines one-sided.
pub(super) fn nested_map() -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(1000, 0),
        vertex(1000, 1000),
        vertex(0, 1000),
        vertex(400, 400),
        vertex(600, 400),
        vertex(600, 600),
        vertex(400, 600),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
        solid_line(3, 0, 3),
        solid_line(4, 5, 4),
        solid_line(5, 6, 5),
        solid_line(6, 7, 6),
        solid_line(7, 4, 7),
    ];
    map.sidedefs = (0_usize..8)
        .map(|index| side("-", "-", "WALL1", usize::from(index >= 4)))
        .collect();
    map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 1)];
    map
}

/// A map whose lines dangle in every way the reads must tolerate: a
/// sidedef into a missing sector, a vertex past `VERTEXES`, a line with a
/// dangling vertex and side, and a line with both sides dangling.
pub(super) fn dangling_map() -> Map {
    let mut map = two_room_map();
    map.sidedefs.get_mut(4).expect("sidedef 4").sector = 99;
    map.linedefs.get_mut(3).expect("linedef 3").v1 = 999;
    map.linedefs.push(Linedef {
        v1: 2,
        v2: 998,
        flags: BLOCKING,
        front: Some(97),
        ..Default::default()
    });
    map.linedefs.push(Linedef {
        v1: 997,
        v2: 998,
        flags: TWO_SIDED,
        front: Some(0),
        back: Some(98),
        ..Default::default()
    });
    map
}

/// The accumulating walls of [`grid_map`], with the sidedef bag they
/// wear.
struct Grid {
    /// The number of columns of cells.
    columns: usize,
    /// The grid corner vertices, row-major.
    vertices: Vec<Vertex>,
    /// The walls grown so far, in build order.
    linedefs: Vec<Linedef>,
    /// The sidedefs the walls append as they grow.
    sidedefs: Vec<Sidedef>,
}

impl Grid {
    /// The vertex index of grid corner `column`/`row`.
    const fn point(&self, column: usize, row: usize) -> usize {
        row.saturating_mul(self.columns.saturating_add(1))
            .saturating_add(column)
    }

    /// The sector owning cell `column`/`row`.
    const fn owner(&self, column: usize, row: usize) -> usize {
        row.saturating_mul(self.columns).saturating_add(column)
    }

    /// The index of a fresh sidedef opening into `sector`, appended to
    /// the bag.
    fn new_side(&mut self, sector: usize) -> usize {
        self.sidedefs.push(side("-", "-", "-", sector));
        self.sidedefs.len().saturating_sub(1)
    }

    /// A blocking one-sided border wall from `v1` to `v2` fronting
    /// `sector`.
    fn one_sided(&mut self, v1: usize, v2: usize, sector: usize) -> Linedef {
        let front = self.new_side(sector);
        Linedef {
            v1,
            v2,
            flags: BLOCKING,
            front: Some(front),
            ..Default::default()
        }
    }

    /// The wall along one grid edge between the owners `first` and
    /// `second`: two-sided between two cells, one-sided fronting the
    /// lone cell.
    fn wall(
        &mut self,
        (start, end): (usize, usize),
        (first, second): (Option<usize>, Option<usize>),
    ) {
        let line = match (first, second) {
            (Some(front), Some(back)) => {
                let front_side = self.new_side(front);
                let back_side = self.new_side(back);
                Linedef {
                    v1: start,
                    v2: end,
                    flags: TWO_SIDED,
                    front: Some(front_side),
                    back: Some(back_side),
                    ..Default::default()
                }
            }
            (Some(sector), None) | (None, Some(sector)) => self.one_sided(start, end, sector),
            (None, None) => self.one_sided(start, end, 0),
        };
        self.linedefs.push(line);
    }
}

/// A deterministic `columns` x `rows` grid of 256-unit rooms sharing
/// every wall: cell `(column, row)` owns sector `row * columns + column`
/// and spans `x = 256*column..256*(column + 1)` and likewise in `y`.
/// Interior edges are two-sided; border edges are one-sided fronts.
pub(super) fn grid_map(columns: usize, rows: usize) -> Map {
    let mut grid = Grid {
        columns,
        vertices: Vec::new(),
        linedefs: Vec::new(),
        sidedefs: Vec::new(),
    };
    for row in 0..=rows {
        for column in 0..=columns {
            grid.vertices.push(vertex(edge(column), edge(row)));
        }
    }
    for column in 0..=columns {
        for row in 0..rows {
            let below = row.saturating_add(1);
            let points = (grid.point(column, row), grid.point(column, below));
            let owners = (
                column.checked_sub(1).map(|left| grid.owner(left, row)),
                (column < columns).then(|| grid.owner(column, row)),
            );
            grid.wall(points, owners);
        }
    }
    for row in 0..=rows {
        for column in 0..columns {
            let next = column.saturating_add(1);
            let points = (grid.point(column, row), grid.point(next, row));
            let owners = (
                row.checked_sub(1).map(|below| grid.owner(column, below)),
                (row < rows).then(|| grid.owner(column, row)),
            );
            grid.wall(points, owners);
        }
    }
    let mut map = Map::new_empty(name("GRID01"), MapFormat::Doom);
    map.vertices = grid.vertices;
    map.linedefs = grid.linedefs;
    map.sidedefs = grid.sidedefs;
    map.sectors = vec![room(0, 128, 160, 0); columns.saturating_mul(rows)];
    map
}

/// The box covering every vertex, `None` for a map without vertices.
fn vertex_box(map: &Map) -> Option<(i64, i64, i64, i64)> {
    map.vertices.iter().fold(None, |found, point| {
        let (low_x, low_y) = (i64::from(point.x), i64::from(point.y));
        Some(match found {
            None => (low_x, low_y, low_x, low_y),
            Some((min_x, min_y, max_x, max_y)) => (
                min_x.min(low_x),
                min_y.min(low_y),
                max_x.max(low_x),
                max_y.max(low_y),
            ),
        })
    })
}

/// The endpoints, the midpoint and the midpoint's four unit neighbours
/// of one resolved line: the on-boundary and off-boundary probes.
fn push_line_points(points: &mut Vec<(i64, i64)>, line: &Linedef, map: &Map) {
    let Some(((x1, y1), (x2, y2))) = map.line_endpoints(line) else {
        return;
    };
    let (first, second) = (
        (i64::from(x1), i64::from(y1)),
        (i64::from(x2), i64::from(y2)),
    );
    points.push(first);
    points.push(second);
    let half = (
        first
            .0
            .checked_add(second.0)
            .and_then(|sum| sum.checked_div(2_i64)),
        first
            .1
            .checked_add(second.1)
            .and_then(|sum| sum.checked_div(2_i64)),
    );
    let (Some(mid_x), Some(mid_y)) = half else {
        return;
    };
    points.push((mid_x, mid_y));
    for (off_x, off_y) in [(1_i64, 0_i64), (-1, 0), (0, 1), (0, -1)] {
        if let (Some(shift_x), Some(shift_y)) = (mid_x.checked_add(off_x), mid_y.checked_add(off_y))
        {
            points.push((shift_x, shift_y));
        }
    }
}

/// The number of `step`-wide cells that span `min..max`, counted so a
/// walk of `0..=grid_count` from `min - step` reaches `max + step`.
fn grid_count(min: i64, max: i64, step: i64) -> usize {
    max.checked_sub(min)
        .and_then(|span| span.checked_div(step))
        .and_then(|cells| usize::try_from(cells.saturating_add(1)).ok())
        .unwrap_or(0)
}

/// The coordinate `cells` steps of `step` past `start`.
fn stepped(start: i64, cells: usize, step: i64) -> i64 {
    let past = i64::try_from(cells).unwrap_or(i64::MAX);
    start.saturating_add(past.saturating_mul(step))
}

/// Appends a `64`-unit grid over the vertex box, padded one step each
/// way.
fn push_grid_points(map: &Map, points: &mut Vec<(i64, i64)>) {
    let Some((min_x, min_y, max_x, max_y)) = vertex_box(map) else {
        return;
    };
    let step = 64_i64;
    let (left, top) = (min_x.saturating_sub(step), min_y.saturating_sub(step));
    for row in 0..=grid_count(min_y, max_y, step) {
        for column in 0..=grid_count(min_x, max_x, step) {
            points.push((stepped(left, column, step), stepped(top, row, step)));
        }
    }
}

/// Deterministic probe points for `map`: every vertex, the line probes
/// [`push_line_points`] gives for every resolving line, and the grid
/// [`push_grid_points`] lays over the whole map. The fixtures'
/// coordinates are multiples of `64`, so the grid hits walls, vertices
/// and interiors exactly; the line probes add every midpoint and its
/// unit neighbours for the other fixtures.
pub(super) fn sample_points(map: &Map) -> Vec<(i64, i64)> {
    let mut points: Vec<(i64, i64)> = map
        .vertices
        .iter()
        .map(|point| (i64::from(point.x), i64::from(point.y)))
        .collect();
    for line in &map.linedefs {
        push_line_points(&mut points, line, map);
    }
    push_grid_points(map, &mut points);
    points
}
