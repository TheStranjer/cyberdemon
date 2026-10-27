//! The pre-index full-scan bodies, kept verbatim as the identity oracle:
//! the one-pass index must agree with these on every map the suite builds.
//! Each fn here is the original of the same-named read in the module
//! history, line for line except for its name and visibility.

use super::super::{bbox_area, crosses_ray, point_on_segment};
use crate::wad::map::geometry::{Segment, same_sector_sides, segment_of, widen};
use crate::wad::map::model::Map;

/// The original `sector_boundary` scan.
pub(super) fn boundary(map: &Map, sector: usize) -> Vec<(usize, bool)> {
    let mut found = Vec::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        let (front, back) = map.line_sectors(line);
        match (front == Some(sector), back == Some(sector)) {
            (true, _) => found.push((index, true)),
            (false, true) => found.push((index, false)),
            (false, false) => {}
        }
    }
    found
}

/// The original `sector_bbox` scan.
pub(super) fn bbox(map: &Map, sector: usize) -> Option<(i32, i32, i32, i32)> {
    let mut box_ = None;
    for &(index, _) in &boundary(map, sector) {
        let Some(line) = map.linedefs.get(index) else {
            continue;
        };
        if let Some(pair) = map.line_endpoints(line) {
            box_ = Some(widen(box_, pair));
        }
    }
    box_
}

/// The original `boundary_segments` scan.
pub(super) fn segments(map: &Map, sector: usize) -> Vec<(Segment, bool)> {
    boundary(map, sector)
        .into_iter()
        .filter_map(|(index, is_front)| {
            let line = map.linedefs.get(index)?;
            if same_sector_sides(map, line) {
                return None;
            }
            Some((segment_of(map, line)?, is_front))
        })
        .collect()
}

/// The original `sector_contains` walk.
pub(super) fn sector_contains(map: &Map, sector: usize, x: i64, y: i64) -> bool {
    let segments = segments(map, sector);
    let mut inside = false;
    let mut on_boundary = false;
    let mut on_front = false;
    for &(segment, is_front) in &segments {
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

/// The original `neighbors` scan.
pub(super) fn neighbours(map: &Map, sector: usize) -> Vec<usize> {
    let mut out: Vec<usize> = boundary(map, sector)
        .into_iter()
        .filter_map(|(index, is_front)| {
            let line = map.linedefs.get(index)?;
            let other = if is_front { line.back } else { line.front };
            map.side_sector(other).filter(|across| *across != sector)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The original `point_in_sector` full sweep.
pub(super) fn point_in_sector(map: &Map, x: i64, y: i64) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for sector in 0..map.sectors.len() {
        if !sector_contains(map, sector, x, y) {
            continue;
        }
        let Some(area) = bbox(map, sector).map(bbox_area) else {
            continue;
        };
        if best.is_none_or(|(best_area, _)| area < best_area) {
            best = Some((area, sector));
        }
    }
    best.map(|(_, sector)| sector)
}
