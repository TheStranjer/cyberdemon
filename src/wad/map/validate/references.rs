//! Reference-integrity checks: every index a [`Map`] carries must land
//! inside the block it names, and no line may lose its front side.
//!
//! One row per offending element. A binary map usually clamps its
//! `0xFFFF` "no side" sentinel to `None` while reading, so a side that
//! points past the sidedef block, a sector that points past the sector
//! block, or a linedef without any front side at all is exactly the kind
//! of damage hand-edited or half-overwritten maps carry.

use alloc::format;
use alloc::vec::Vec;

use super::{Issue, Severity, listed};
use crate::wad::map::model::Map;

/// Linedefs whose `v1`/`v2` name a vertex the map does not have.
pub(super) fn vertex_out_of_range(map: &Map) -> Vec<Issue> {
    let count = map.vertices.len();
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let bad: Vec<usize> = [line.v1, line.v2]
                .into_iter()
                .filter(|vertex| *vertex >= count)
                .collect();
            (!bad.is_empty()).then(|| {
                Issue::new(
                    Severity::Error,
                    format!(
                        "linedef {index} references nonexistent vertex {} but the map has \
                         {count} vertices",
                        listed(&bad),
                    ),
                )
                .linedef(&[index])
            })
        })
        .collect()
}

/// Linedefs whose front or back side names a sidedef the map does not
/// have.
pub(super) fn sidedef_out_of_range(map: &Map) -> Vec<Issue> {
    let count = map.sidedefs.len();
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let bad: Vec<usize> = [line.front, line.back]
                .into_iter()
                .flatten()
                .filter(|side| *side >= count)
                .collect();
            (!bad.is_empty()).then(|| {
                Issue::new(
                    Severity::Error,
                    format!(
                        "linedef {index} references nonexistent sidedef {} but the map has \
                         {count} sidedefs",
                        listed(&bad),
                    ),
                )
                .linedef(&[index])
            })
        })
        .collect()
}

/// Sidedefs whose sector index names a sector the map does not have.
pub(super) fn sector_out_of_range(map: &Map) -> Vec<Issue> {
    let count = map.sectors.len();
    map.sidedefs
        .iter()
        .enumerate()
        .filter(|&(_, side)| side.sector >= count)
        .map(|(index, side)| {
            Issue::new(
                Severity::Error,
                format!(
                    "sidedef {index} references nonexistent sector {} but the map has \
                     {count} sectors",
                    side.sector
                ),
            )
            .sidedef(&[index])
        })
        .collect()
}

/// Linedefs without a front side: vanilla rendering and blockmap need one.
pub(super) fn missing_front_side(map: &Map) -> Vec<Issue> {
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| line.front.is_none())
        .map(|(index, _)| {
            Issue::new(
                Severity::Error,
                format!("linedef {index} has no front side"),
            )
            .linedef(&[index])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        missing_front_side, sector_out_of_range, sidedef_out_of_range, vertex_out_of_range,
    };
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

    /// A vertex at `x`/`y`.
    const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A line from `v1` to `v2` with the given sides.
    const fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            special: 0,
            tag: 0,
            args: [0; 5],
            front,
            back,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A sidedef opening into `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// A map of one vertex pair, one line, one side and one sector.
    fn minimal() -> Map {
        Map {
            vertices: vec![vertex(0, 0), vertex(64, 0)],
            linedefs: vec![line(0, 1, Some(0), None)],
            sidedefs: vec![side(0)],
            sectors: vec![Sector::default()],
            ..Default::default()
        }
    }

    #[test]
    fn vertex_out_of_range_names_the_dangling_ends() {
        let clean = vertex_out_of_range(&minimal());
        assert!(clean.is_empty(), "in-range ends stay silent");
        let mut map = minimal();
        {
            let bad = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            bad.v2 = 900;
        }
        let issues = vertex_out_of_range(&map);
        let issue = issues.first().expect("the dangling end reports");
        assert_eq!(issue.linedefs, vec![0], "the bad linedef is attached");
        assert!(
            issue.message.contains("900") && issue.message.contains("2 vertices"),
            "the message names the index and the block size: {}",
            issue.message
        );
    }

    #[test]
    fn vertex_out_of_range_lists_both_bad_ends_once() {
        let mut map = minimal();
        {
            let line = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            line.v1 = 40;
            line.v2 = 900;
        }
        let issues = vertex_out_of_range(&map);
        assert_eq!(issues.len(), 1, "one row per linedef");
        let issue = issues.first().expect("both ends are bad");
        assert!(
            issue.message.contains("40, 900"),
            "both bad indices are listed: {}",
            issue.message
        );
    }

    #[test]
    fn sidedef_out_of_range_names_both_sides() {
        let clean = sidedef_out_of_range(&minimal());
        assert!(
            clean.is_empty(),
            "Some indices inside the block stay silent"
        );
        let mut map = minimal();
        {
            let line = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            line.front = Some(7);
            line.back = Some(8);
        }
        let issues = sidedef_out_of_range(&map);
        let issue = issues.first().expect("both sides dangle");
        assert_eq!(issues.len(), 1, "one row per linedef");
        assert!(
            issue.message.contains("7, 8") && issue.message.contains("1 sidedefs"),
            "the message names the sides and the block size: {}",
            issue.message
        );
    }

    #[test]
    fn sector_out_of_range_names_the_sidedef() {
        let clean = sector_out_of_range(&minimal());
        assert!(clean.is_empty(), "sector 0 of 1 is fine");
        let mut map = minimal();
        {
            let side = map
                .sidedefs
                .first_mut()
                .expect("the minimal map has a side");
            side.sector = 42;
        }
        let issues = sector_out_of_range(&map);
        let issue = issues.first().expect("the dangling sector reports");
        assert_eq!(issue.sidedefs, vec![0], "the bad sidedef is attached");
        assert!(
            issue.message.contains("42") && issue.message.contains("1 sectors"),
            "the message names the index and the block size: {}",
            issue.message
        );
    }

    #[test]
    fn missing_front_side_reports_lines_without_any_front() {
        let clean = missing_front_side(&minimal());
        assert!(clean.is_empty(), "the minimal line has a front side");
        let mut map = minimal();
        {
            let line = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            line.front = None;
            line.back = Some(0);
        }
        let issues = missing_front_side(&map);
        let issue = issues.first().expect("a back-only line is broken");
        assert_eq!(issue.linedefs, vec![0], "the linedef is attached");
        assert!(
            issue.message.contains("no front side"),
            "the message names the problem: {}",
            issue.message
        );
    }
}
