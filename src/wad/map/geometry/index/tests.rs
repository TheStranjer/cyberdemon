//! Identity tests: the one-pass index must answer exactly what the old
//! full-scan free functions answered, plus the direct reads the index
//! adds (degrees, candidates) and the point primitives it owns.

mod fixtures;
mod perf;
mod reference;

use super::{SectorIndex, bbox_area, crosses_ray, point_on_segment};
use crate::wad::map::MapFormat;
use crate::wad::map::common::fixtures::{name, two_room_map};
use crate::wad::map::flags::TWO_SIDED;
use crate::wad::map::geometry::{Segment, widen};
use crate::wad::map::model::{Linedef, Map};
use fixtures::{dangling_map, grid_map, nested_map, sample_points};

/// The identity pass: every stored read of the index equals the
/// reference scan's answer, sector by sector and over the sampled
/// points.
fn assert_index_matches_the_scans(map: &Map) {
    let index = SectorIndex::new(map);
    for sector in 0..map.sectors.len() {
        assert_eq!(
            index.boundary(sector),
            reference::boundary(map, sector),
            "sector {sector}'s boundary pairs",
        );
        assert_eq!(
            index.bbox(sector),
            reference::bbox(map, sector),
            "sector {sector}'s box",
        );
        assert_eq!(
            index.neighbours(sector),
            reference::neighbours(map, sector),
            "sector {sector}'s neighbours",
        );
        assert_eq!(
            index.degree(sector),
            reference::segments(map, sector).len(),
            "sector {sector}'s segment count",
        );
    }
    for &(probe_x, probe_y) in &sample_points(map) {
        for sector in 0..map.sectors.len() {
            assert_eq!(
                index.contains_point(sector, probe_x, probe_y),
                reference::sector_contains(map, sector, probe_x, probe_y),
                "sector {sector} at {probe_x}/{probe_y}",
            );
        }
        assert_eq!(
            index.point_in_sector(probe_x, probe_y),
            reference::point_in_sector(map, probe_x, probe_y),
            "the winner at {probe_x}/{probe_y}",
        );
    }
}

#[test]
fn the_index_matches_the_scans_on_the_two_room_map() {
    assert_index_matches_the_scans(&two_room_map());
}

#[test]
fn the_index_matches_the_scans_on_a_nested_map() {
    assert_index_matches_the_scans(&nested_map());
}

#[test]
fn the_index_matches_the_scans_on_a_dangling_map() {
    assert_index_matches_the_scans(&dangling_map());
}

#[test]
fn the_index_matches_the_scans_on_room_grids() {
    assert_index_matches_the_scans(&grid_map(3, 2));
    assert_index_matches_the_scans(&grid_map(2, 3));
    assert_index_matches_the_scans(&grid_map(1, 1));
}

#[test]
fn a_missing_sector_answers_everything_empty() {
    let map = two_room_map();
    let index = SectorIndex::new(&map);
    assert!(index.boundary(9).is_empty(), "no sector 9 bounds nothing");
    assert!(index.neighbours(9).is_empty(), "and neighbours nothing");
    assert_eq!(index.bbox(9), None, "and has no box");
    assert_eq!(index.degree(9), 0, "and carries no segments");
    assert!(
        !index.contains_point(9, 64, 128),
        "and holds no point at all"
    );
    let bare = Map::new_empty(name("EMPTY"), MapFormat::Doom);
    let empty = SectorIndex::new(&bare);
    assert_eq!(
        empty.point_in_sector(0, 0),
        None,
        "an empty map holds nothing"
    );
}

#[test]
fn degrees_count_the_carrying_segments() {
    let map = two_room_map();
    let index = SectorIndex::new(&map);
    assert_eq!(index.degree(0), 4, "the hall owns four carrying lines");
    assert_eq!(index.degree(1), 4, "the back room owns four carrying lines");
}

#[test]
fn point_candidates_are_exactly_the_covering_boxes() {
    let map = two_room_map();
    let index = SectorIndex::new(&map);
    assert_eq!(
        index.point_candidates(256, 128).collect::<Vec<usize>>(),
        vec![0_usize, 1],
        "the shared step line sits inside both boxes",
    );
    assert_eq!(
        index.point_candidates(64, 128).collect::<Vec<usize>>(),
        vec![0_usize],
        "inside the hall only the hall's box covers",
    );
    assert!(
        index
            .point_candidates(600, 128)
            .collect::<Vec<usize>>()
            .is_empty(),
        "right of both rooms nothing covers",
    );
}

#[test]
fn a_line_whose_sides_share_its_sector_is_front_once_and_carries_nothing() {
    let mut map = two_room_map();
    map.linedefs.push(Linedef {
        v1: 0,
        v2: 2,
        flags: TWO_SIDED,
        front: Some(0),
        back: Some(1),
        ..Default::default()
    });
    let line = map.linedefs.len().saturating_sub(1);
    let index = SectorIndex::new(&map);
    assert_eq!(
        index
            .boundary(0)
            .iter()
            .filter(|pair| pair.0 == line)
            .copied()
            .collect::<Vec<(usize, bool)>>(),
        vec![(line, true)],
        "the line appears once, flagged front",
    );
    assert_eq!(index.degree(0), 4, "but it carries no containment segment");
    assert_eq!(
        index.bbox(0),
        Some((0_i32, 0_i32, 512_i32, 256_i32)),
        "while its resolvable vertices do count into the box",
    );
    assert_eq!(
        index.neighbours(0),
        vec![1_usize],
        "and it neighbours nothing new"
    );
    assert_eq!(
        index.point_in_sector(64, 128),
        Some(0),
        "and containment keeps its answer",
    );
}

#[test]
fn point_on_segment_includes_endpoints_and_stays_in_the_box() {
    let segment = Segment {
        x1: 0_i64,
        y1: 0,
        x2: 100,
        y2: 40,
    };
    assert!(
        point_on_segment(0, 0, &segment),
        "the start vertex is on it"
    );
    assert!(point_on_segment(100, 40, &segment), "and the end vertex");
    assert!(
        point_on_segment(50, 20, &segment),
        "and the collinear middle"
    );
    assert!(
        !point_on_segment(150, 60, &segment),
        "collinear but outside the box is off it"
    );
    assert!(
        !point_on_segment(50, 21, &segment),
        "inside the box but off the line is off it"
    );
    let stretched = Segment {
        x1: i64::MIN,
        y1: 0,
        x2: i64::MAX,
        y2: 0,
    };
    assert!(
        !point_on_segment(0, 0, &stretched),
        "a delta that overflows answers false, never a wrapped true"
    );
}

#[test]
fn crosses_ray_counts_only_a_strict_rightward_crossing() {
    let up = Segment {
        x1: 0_i64,
        y1: 0,
        x2: 10,
        y2: 10,
    };
    assert!(
        crosses_ray(0, 5, &up),
        "the ray from 0/5 meets the climb at x = 5"
    );
    assert!(
        !crosses_ray(6, 5, &up),
        "meeting left of the ray start does not count"
    );
    assert!(
        !crosses_ray(0, 15, &up),
        "a ray above both endpoints is clear"
    );
    let down = Segment {
        x1: 10_i64,
        y1: 10,
        x2: 0,
        y2: 0,
    };
    assert!(crosses_ray(0, 5, &down), "a downward segment counts too");
    let flat = Segment {
        x1: 0_i64,
        y1: 5,
        x2: 10,
        y2: 5,
    };
    assert!(
        !crosses_ray(-1, 5, &flat),
        "a horizontal segment is never crossed"
    );
    let stretched = Segment {
        x1: i64::MIN,
        y1: -1,
        x2: i64::MAX,
        y2: 1,
    };
    assert!(
        !crosses_ray(0, 0, &stretched),
        "a delta that overflows answers false, never a wrapped true"
    );
}

#[test]
fn bbox_area_is_exact_in_i128() {
    assert_eq!(
        bbox_area((0_i32, 0_i32, 10_i32, 20_i32)),
        200_i128,
        "a plain box"
    );
    let widest = i128::from(u32::MAX);
    assert_eq!(
        bbox_area((i32::MIN, i32::MIN, i32::MAX, i32::MAX)),
        widest
            .checked_mul(widest)
            .expect("u32::MAX squared fits i128"),
        "the full i32 box squares exactly, without saturation",
    );
}

#[test]
fn the_widen_and_index_reads_share_the_same_box_rule() {
    let map = two_room_map();
    let index = SectorIndex::new(&map);
    let grown = widen(index.bbox(0), ((512_i32, -256_i32), (i32::MIN, i32::MAX)));
    assert_eq!(
        grown,
        (i32::MIN, -256_i32, 512_i32, i32::MAX),
        "the shared widen grows the stored box across every coordinate",
    );
}
