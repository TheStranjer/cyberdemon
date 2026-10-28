//! The 400-sector work-count acceptance test.
//!
//! A deterministic grid of 400 rooms joined by two-sided step lines, with
//! 300 things stood at room centres: the [`Describe`] read and one
//! [`distribute`] area over that map answer their point lookups from one
//! [`SectorIndex`]. The work both operations actually do is counted here
//! per lookup as the boundary segments of exactly the sectors whose
//! bounding box covers the point, [`SectorIndex::point_candidates`]
//! summed over [`SectorIndex::degree`], and asserted under a stated
//! small multiple of the map's total boundary-segment count. The naive
//! full-scan shape would pay every sector's segments per lookup, and
//! that count is asserted to break the same bound: the old shape fails
//! by counting alone, without this file ever running it.
//!
//! No wall time is measured. The results side is pinned in the same
//! pass: every counted lookup answers the same sector through the free
//! [`crate::wad::map::geometry`] wrappers as through the held index, the
//! real [`Describe`] rows agree with wrapper-folded per-sector thing
//! counts, boxes and neighbours, and the real [`distribute`] run places
//! its things on counted grid points whose answers the wrappers agree
//! with.
//!
//! [`Describe`]: crate::wad::map::describe::Describe
//! [`distribute`]: crate::wad::map::placement::distribute

use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::wad::game::Game;
use crate::wad::map::common::fixtures::{edge, spawn};
use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::describe::Describe;
use crate::wad::map::geometry::{self, SectorIndex};
use crate::wad::map::model::Map;
use crate::wad::map::placement::{self, Area, DistributeSpec, Facing, distribute};

use super::fixtures::grid_map;

/// Rooms per grid side: 20 x 20 sectors.
const GRID_SIDE: usize = 20_usize;
/// Every fourth room stays empty: 300 things in 400 rooms.
const EMPTY_EVERY: usize = 4_usize;
/// The distribution area's lower corner, on a room boundary.
const AREA_MIN: i64 = 1_024_i64;
/// The distribution area's upper corner, eight rooms over.
const AREA_MAX: i64 = 3_072_i64;
/// The clearance of the one distribution run: the grid spaces 48 apart.
const CLEARANCE: i64 = 24_i64;
/// How many things the distribution area is asked to hold.
const NEED: usize = 60_usize;
/// The stated bound: the counted point work of either operation may not
/// exceed this multiple of the map's total boundary-segment count.
const WORK_BOUND_MULTIPLE: usize = 16_usize;
/// The boundary segments every room of the grid carries.
const ROOM_SEGMENTS: usize = 4_usize;
/// The distribution engine's per-axis grid cap, mirrored by [`axis`].
const MAX_AXIS: usize = 4_096_usize;
/// The engine's smallest grid spacing, mirrored by [`grid_points`].
const MIN_SPACING: i64 = 16_i64;

/// The DOOM II tables the describe and place reads name things with.
fn definitions() -> Definitions {
    crate::defs::definitions_for(Game::Doom2).expect("the embedded tables are valid")
}

/// The acceptance map: [`grid_map`]'s joined rooms with a type-3001
/// stood at the centre of every room but every fourth.
fn grid_with_things() -> Map {
    let mut map = grid_map(GRID_SIDE, GRID_SIDE);
    for cell in 0..map.sectors.len() {
        if cell.checked_rem(EMPTY_EVERY) == Some(0_usize) {
            continue;
        }
        let Some(row) = cell.checked_div(GRID_SIDE) else {
            continue;
        };
        let Some(column) = cell.checked_rem(GRID_SIDE) else {
            continue;
        };
        let (x, y) = (
            edge(column).saturating_add(128),
            edge(row).saturating_add(128),
        );
        map.things.push(spawn(3001, x, y, 0));
    }
    map
}

/// The lookup work of one point: the boundary segments the even-odd test
/// walks, summed over exactly the sectors whose box covers it.
fn point_work(index: &SectorIndex, x: i64, y: i64) -> usize {
    index
        .point_candidates(x, y)
        .map(|sector| index.degree(sector))
        .sum()
}

/// The lookup work of a whole operation: [`point_work`] over every
/// point the operation tests.
fn total_work(index: &SectorIndex, points: &[(i64, i64)]) -> usize {
    points.iter().map(|&(x, y)| point_work(index, x, y)).sum()
}

/// The map's whole boundary-segment count: the sum of every sector's
/// degree, the work one naive full-scan lookup would pay.
fn total_segments(index: &SectorIndex, map: &Map) -> usize {
    (0..map.sectors.len())
        .map(|sector| index.degree(sector))
        .sum()
}

/// The things' spawn points: every home lookup the [`Describe`] read
/// resolves.
///
/// [`Describe`]: crate::wad::map::describe::Describe
fn thing_points(map: &Map) -> Vec<(i64, i64)> {
    map.things
        .iter()
        .map(|thing| (i64::from(thing.x), i64::from(thing.y)))
        .collect()
}

/// One axis of the distribution engine's centred grid, mirroring its
/// documented rule: the whole span walked at `step`, the leftover slack
/// split evenly below and above.
fn axis(min: i64, max: i64, step: i64) -> Vec<i64> {
    let span = max.saturating_sub(min);
    let steps = usize::try_from(span.checked_div(step).unwrap_or_default())
        .unwrap_or(MAX_AXIS)
        .min(MAX_AXIS);
    let used = i64::try_from(steps)
        .unwrap_or(i64::MAX)
        .saturating_mul(step);
    let start = min.saturating_add(
        span.saturating_sub(used)
            .checked_div(2_i64)
            .unwrap_or_default(),
    );
    placement::axis_points(start, start.saturating_add(used), step)
}

/// Every candidate point the one [`distribute`] area tests: the engine
/// squares its spacing to `max(2 * clearance, 16)` and lays the centred
/// grid over the area's box; the doubling escape for grids past its
/// candidate cap never triggers here, which the checked assert inside
/// keeps honest.
fn grid_points() -> Vec<(i64, i64)> {
    let spacing = CLEARANCE.saturating_mul(2_i64).max(MIN_SPACING);
    let columns = axis(AREA_MIN, AREA_MAX, spacing);
    let rows = axis(AREA_MIN, AREA_MAX, spacing);
    assert!(
        columns.len().saturating_mul(rows.len()) <= 0x1_0000_usize,
        "the mirror grid stays under the engine's candidate cap"
    );
    rows.iter()
        .flat_map(|&y| columns.iter().map(move |&x| (x, y)))
        .collect()
}

/// The spec of the one distribution area the acceptance run fills.
fn area_spec() -> DistributeSpec {
    DistributeSpec {
        areas: alloc::vec![Area::Rect {
            min_x: AREA_MIN,
            min_y: AREA_MIN,
            max_x: AREA_MAX,
            max_y: AREA_MAX,
        }],
        needs: alloc::vec![NEED],
        clearance: CLEARANCE,
        facing: Facing::Fixed(90),
        unknown_radius: 16,
    }
}

/// The wrapper-vs-index agreement every counted lookup must keep: the
/// free [`geometry::point_in_sector`] of CD-290a answers exactly what
/// the held index answers, for every point of both operations.
fn assert_wrappers_agree_with_the_index(map: &Map, index: &SectorIndex, points: &[(i64, i64)]) {
    for &(x, y) in points {
        assert_eq!(
            index.point_in_sector(x, y),
            geometry::point_in_sector(map, x, y),
            "the held index and the free wrapper agree at {x}/{y}",
        );
    }
}

/// The thing count each sector earns from the wrapper-resolved homes.
fn wrapper_homes(map: &Map, points: &[(i64, i64)]) -> Vec<usize> {
    let mut expected: Vec<usize> = vec![0_usize; map.sectors.len()];
    for point in points {
        let Some(sector) = geometry::point_in_sector(map, point.0, point.1) else {
            continue;
        };
        let Some(slot) = expected.get_mut(sector) else {
            continue;
        };
        *slot = slot.saturating_add(1);
    }
    expected
}

/// The `Describe` half of the identity check: every row's box, neighbour
/// sectors and thing total equal the answers folded from the free
/// wrappers, and the whole-map summary walks every element block.
fn assert_describe_rows_match_the_wrappers(map: &Map, defs: &Definitions, points: &[(i64, i64)]) {
    let describe = Describe::new(map, defs, SKY_FLAT);
    let expected_things = wrapper_homes(map, points);
    for sector in 0..map.sectors.len() {
        let row = describe
            .sector(sector)
            .expect("every grid sector is describable");
        assert_eq!(
            row.bbox,
            geometry::sector_bbox(map, sector).map(<[i32; 4]>::from),
            "sector {sector}'s box comes through unchanged",
        );
        let neighbours: Vec<usize> = row.neighbours.iter().map(|entry| entry.sector).collect();
        assert_eq!(
            neighbours,
            geometry::neighbors(map, sector),
            "sector {sector}'s neighbours come through unchanged",
        );
        let counted: u32 = row.things.iter().map(|entry| entry.count).sum();
        assert_eq!(
            usize::try_from(counted).expect("the count fits"),
            *expected_things
                .get(sector)
                .expect("the acceptance map has that sector"),
            "sector {sector}'s things are the wrapper-resolved homes",
        );
    }
    let overview = describe.summary();
    assert_eq!(
        overview.counts.sectors,
        map.sectors.len(),
        "the summary walked the whole sector block",
    );
    assert_eq!(
        overview.counts.things,
        map.things.len(),
        "and counted every thing",
    );
}

/// The shape of the acceptance map: 400 joined rooms, 300 centre
/// things, every room carrying exactly four boundary segments.
fn assert_the_map_shape(map: &Map, index: &SectorIndex) {
    assert_eq!(
        map.sectors.len(),
        GRID_SIDE.saturating_mul(GRID_SIDE),
        "400 sectors"
    );
    let expected_things = map
        .sectors
        .len()
        .saturating_sub(map.sectors.len().saturating_div(EMPTY_EVERY));
    assert_eq!(map.things.len(), expected_things, "300 things");
    assert_eq!(
        index.degree(0),
        ROOM_SEGMENTS,
        "a grid room carries four segments"
    );
    for sector in 1..map.sectors.len() {
        assert_eq!(index.degree(sector), ROOM_SEGMENTS, "sector {sector} too");
    }
}

#[test]
fn a_400_sector_grid_counts_point_work_at_the_covered_degrees() {
    let map = grid_with_things();
    let defs = definitions();
    let index = SectorIndex::new(&map);
    assert_the_map_shape(&map, &index);
    let segments = total_segments(&index, &map);
    let bound = WORK_BOUND_MULTIPLE.saturating_mul(segments);

    let describe_points = thing_points(&map);
    let place_points = grid_points();
    let describe_work = total_work(&index, &describe_points);
    let place_work = total_work(&index, &place_points);

    assert_eq!(
        describe_work,
        describe_points.len().saturating_mul(ROOM_SEGMENTS),
        "a room-centre point is covered by exactly its own room's box",
    );
    assert!(
        describe_work <= bound,
        "the describe read's {describe_work} segment tests exceeded the {bound} bound",
    );
    assert!(
        place_work <= bound,
        "the distribution's {place_work} segment tests stayed under the {bound}-segment bound \
         ({WORK_BOUND_MULTIPLE} x {segments} for {} lookups)",
        place_points.len(),
    );

    let naive_lookups = describe_points.len().saturating_add(place_points.len());
    let naive_work = naive_lookups.saturating_mul(segments);
    assert!(
        naive_work > bound,
        "the old full-scan shape would pay {naive_work} segment tests, above the {bound} bound: \
         {naive_lookups} lookups x {segments} segments per map, not per covering box",
    );

    let counted: Vec<(i64, i64)> = describe_points
        .iter()
        .chain(place_points.iter())
        .copied()
        .collect();
    assert_wrappers_agree_with_the_index(&map, &index, &counted);
    assert_describe_rows_match_the_wrappers(&map, &defs, &describe_points);

    let distribution = distribute(&map, &defs, &area_spec());
    assert_eq!(
        distribution.placed.len(),
        NEED,
        "the area holds the whole need"
    );
    for spot in &distribution.placed {
        assert!(
            place_points.contains(&spot.at),
            "the engine placed at {}, {} outside the counted grid",
            spot.at.0,
            spot.at.1,
        );
        assert!(
            geometry::point_in_sector(&map, spot.at.0, spot.at.1).is_some(),
            "every placed thing stands in a sector the wrappers resolve",
        );
    }
}
