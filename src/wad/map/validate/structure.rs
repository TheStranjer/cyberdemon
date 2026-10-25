//! Structural checks over a decoded [`Map`]: line shape (collapsed and
//! duplicated lines, two-sided flag mismatches) and element use (unused
//! vertices, sidedefs and sectors, sidedefs shared across linedefs).
//!
//! Every check ignores out-of-range references rather than reporting them
//! twice: [`super::references`] owns that, and a dangling index cannot be
//! used, shared or measured.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::vec::Vec;

use super::{Issue, Severity, listed};
use crate::wad::map::flags::TWO_SIDED;
use crate::wad::map::model::{Linedef, Map, Vertex};

/// Both endpoint vertices when the map holds the indices `line` names.
fn resolved_ends<'map>(map: &'map Map, line: &Linedef) -> Option<(&'map Vertex, &'map Vertex)> {
    Some((map.vertices.get(line.v1)?, map.vertices.get(line.v2)?))
}

/// The vertex indices every in-range linedef end references.
fn used_vertices(map: &Map) -> BTreeSet<usize> {
    map.linedefs
        .iter()
        .flat_map(|line| [line.v1, line.v2])
        .filter(|index| map.vertices.get(*index).is_some())
        .collect()
}

/// Which in-range linedefs reference each sidedef (key: sidedef index,
/// value: linedef indices in block order, one index twice when that line
/// uses the side on both of its faces).
fn side_users(map: &Map) -> BTreeMap<usize, Vec<usize>> {
    let mut users: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        record_side(map, &mut users, line.front, index);
        record_side(map, &mut users, line.back, index);
    }
    users
}

/// Notes that linedef `index` uses `side` when it names an in-range sidedef.
fn record_side(
    map: &Map,
    users: &mut BTreeMap<usize, Vec<usize>>,
    side: Option<usize>,
    index: usize,
) {
    let Some(known) = side.filter(|wanted| map.sidedefs.get(*wanted).is_some()) else {
        return;
    };
    users.entry(known).or_default().push(index);
}

/// Linedefs whose two ends sit at the same coordinates.
pub(super) fn zero_length_line(map: &Map) -> Vec<Issue> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let (start, end) = resolved_ends(map, line)?;
            if start.x != end.x || start.y != end.y {
                return None;
            }
            let single = [line.v1];
            let both = [line.v1, line.v2];
            let ends: &[usize] = if line.v1 == line.v2 { &single } else { &both };
            Some(
                Issue::new(
                    Severity::Error,
                    format!(
                        "linedef {index} has both ends at x {}, y {}: a zero-length line has \
                         no side to draw or block on",
                        start.x, start.y,
                    ),
                )
                .linedef(&[index])
                .vertex(ends)
                .at(start.x, start.y),
            )
        })
        .collect()
}

/// Groups of linedefs connecting the same two vertices, either direction.
pub(super) fn duplicate_line(map: &Map) -> Vec<Issue> {
    let mut groups: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        if resolved_ends(map, line).is_none() {
            continue;
        }
        let ends = if line.v1 <= line.v2 {
            (line.v1, line.v2)
        } else {
            (line.v2, line.v1)
        };
        groups.entry(ends).or_default().push(index);
    }
    groups
        .iter()
        .filter(|&(_, members)| members.len() > 1)
        .map(|(&(a, b), members)| {
            Issue::new(
                Severity::Error,
                format!(
                    "linedefs {} connect the same vertices {a} and {b}",
                    listed(members),
                ),
            )
            .linedef(members)
            .vertex(&[a, b])
        })
        .collect()
}

/// Linedefs whose two-sided flag disagrees with the presence of a back
/// side.
pub(super) fn two_sided_flag_mismatch(map: &Map) -> Vec<Issue> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let flagged = line.flags & TWO_SIDED != 0;
            if line.back.is_some() && !flagged {
                return Some(
                    Issue::new(
                        Severity::Warning,
                        format!(
                            "linedef {index} has a back side but the two-sided flag \
                             ({TWO_SIDED}) is not set",
                        ),
                    )
                    .linedef(&[index]),
                );
            }
            if line.back.is_none() && flagged {
                return Some(
                    Issue::new(
                        Severity::Warning,
                        format!(
                            "linedef {index} sets the two-sided flag ({TWO_SIDED}) but has no \
                             back side",
                        ),
                    )
                    .linedef(&[index]),
                );
            }
            None
        })
        .collect()
}

/// Vertices no in-range linedef end references.
pub(super) fn unused_vertex(map: &Map) -> Vec<Issue> {
    let used = used_vertices(map);
    map.vertices
        .iter()
        .enumerate()
        .filter(|&(index, _)| !used.contains(&index))
        .map(|(index, point)| {
            Issue::new(
                Severity::Info,
                format!(
                    "vertex {index} at x {}, y {} is referenced by no linedef",
                    point.x, point.y
                ),
            )
            .vertex(&[index])
            .at(point.x, point.y)
        })
        .collect()
}

/// Sidedefs no in-range linedef side references.
pub(super) fn unused_sidedef(map: &Map) -> Vec<Issue> {
    let used = side_users(map);
    map.sidedefs
        .iter()
        .enumerate()
        .filter(|&(index, _)| !used.contains_key(&index))
        .map(|(index, _)| {
            Issue::new(
                Severity::Info,
                format!("sidedef {index} is referenced by no linedef"),
            )
            .sidedef(&[index])
        })
        .collect()
}

/// Sectors no in-range sidedef references.
pub(super) fn unused_sector(map: &Map) -> Vec<Issue> {
    let used: BTreeSet<usize> = map
        .sidedefs
        .iter()
        .map(|side| side.sector)
        .filter(|index| map.sectors.get(*index).is_some())
        .collect();
    map.sectors
        .iter()
        .enumerate()
        .filter(|&(index, _)| !used.contains(&index))
        .map(|(index, _)| {
            Issue::new(
                Severity::Warning,
                format!("sector {index} is referenced by no sidedef: it borders no geometry"),
            )
            .sector(&[index])
        })
        .collect()
}

/// Sidedefs referenced by several linedefs (legal in vanilla, but many
/// editors assume one line per side).
pub(super) fn shared_sidedef(map: &Map) -> Vec<Issue> {
    side_users(map)
        .iter()
        .filter_map(|(side, lines)| {
            let mut distinct: Vec<usize> = lines.clone();
            distinct.dedup();
            (distinct.len() > 1).then(|| {
                Issue::new(
                    Severity::Info,
                    format!(
                        "sidedef {side} is used by linedefs {}: legal in vanilla, but some \
                         editors refuse to edit shared sides",
                        listed(&distinct),
                    ),
                )
                .sidedef(&[*side])
                .linedef(&distinct)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::{
        duplicate_line, shared_sidedef, two_sided_flag_mismatch, unused_sector, unused_sidedef,
        unused_vertex, zero_length_line,
    };
    use crate::wad::map::flags::TWO_SIDED;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

    /// A vertex at `x`/`y`.
    const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A one-sided line from `v1` to `v2` over sidedef `front`.
    const fn line(v1: usize, v2: usize, front: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            special: 0,
            tag: 0,
            args: [0; 5],
            front,
            back: None,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A two-sided line from `v1` to `v2` over the sides `front`/`back`.
    const fn flagged(v1: usize, v2: usize, sides: (usize, usize)) -> Linedef {
        let (front, back) = sides;
        Linedef {
            v1,
            v2,
            flags: TWO_SIDED,
            special: 0,
            tag: 0,
            args: [0; 5],
            front: Some(front),
            back: Some(back),
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A sidedef over `sector`.
    const fn side(sector: usize) -> Sidedef {
        Sidedef {
            x_offset: 0,
            y_offset: 0,
            upper: String::new(),
            lower: String::new(),
            middle: String::new(),
            sector,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A two-vertex, one-line, one-side, one-sector base every structure
    /// test edits into the shape it needs.
    fn minimal() -> Map {
        Map {
            vertices: vec![vertex(0, 0), vertex(64, 0)],
            linedefs: vec![line(0, 1, Some(0))],
            sidedefs: vec![side(0)],
            sectors: vec![Sector::default()],
            ..Default::default()
        }
    }

    #[test]
    fn zero_length_line_collapses_to_a_point() {
        let clean = zero_length_line(&minimal());
        assert!(clean.is_empty(), "distinct ends measure fine");
        let mut map = minimal();
        map.vertices = vec![vertex(10, 20), vertex(10, 20)];
        let issues = zero_length_line(&map);
        let issue = issues.first().expect("the same coordinates collapse");
        assert_eq!(issue.linedefs, vec![0], "the collapsed linedef is attached");
        assert_eq!(issue.vertices, vec![0, 1], "both vertex slots are named");
        assert_eq!(issue.x, Some(10_i32), "the point is reported");
        assert_eq!(issue.y, Some(20_i32), "in map units");
    }

    #[test]
    fn zero_length_line_names_one_vertex_when_both_ends_share_it() {
        let mut map = minimal();
        {
            let bad = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            bad.v2 = 0;
        }
        let issues = zero_length_line(&map);
        let issue = issues.first().expect("v1 == v2 is zero-length");
        assert_eq!(issue.vertices, vec![0], "the shared index appears once");
    }

    #[test]
    fn duplicate_line_catches_the_reversed_twin() {
        let mut map = minimal();
        map.linedefs.push(line(1, 0, Some(0)));
        let issues = duplicate_line(&map);
        let issue = issues.first().expect("0->1 and 1->0 are duplicates");
        assert_eq!(issue.linedefs, vec![0, 1], "both linedefs are attached");
        assert_eq!(
            issue.vertices,
            vec![0, 1],
            "the shared endpoints are attached"
        );
        assert!(
            issue.message.contains("linedefs 0, 1"),
            "the message lists the pair: {}",
            issue.message
        );
    }

    #[test]
    fn duplicate_line_groups_three_linedefs_into_one_row() {
        let mut map = minimal();
        map.linedefs.push(line(0, 1, Some(0)));
        map.linedefs.push(line(1, 0, Some(0)));
        let issues = duplicate_line(&map);
        assert_eq!(issues.len(), 1, "one row per vertex pair");
        assert_eq!(
            issues.first().expect("the trio is a group").linedefs,
            vec![0, 1, 2],
            "all three linedefs appear in the row"
        );
    }

    #[test]
    fn duplicate_line_ignores_dangling_ends() {
        let mut map = minimal();
        map.linedefs.push(line(0, 900, Some(0)));
        map.linedefs.push(line(900, 0, Some(0)));
        assert!(
            duplicate_line(&map).is_empty(),
            "out-of-range ends are the reference checks' business"
        );
    }

    #[test]
    fn consistent_two_sided_shapes_stay_silent() {
        let mut map = minimal();
        map.sidedefs.push(side(0));
        map.linedefs.push(flagged(1, 0, (0, 1)));
        assert!(
            two_sided_flag_mismatch(&map).is_empty(),
            "a one-sided line and a flagged two-sided line are both fine"
        );
    }

    #[test]
    fn two_sided_flag_mismatch_names_a_back_side_without_the_flag() {
        let mut map = minimal();
        map.sidedefs.push(side(0));
        map.linedefs.push(line(1, 0, Some(0)));
        {
            let twin = map.linedefs.get_mut(1).expect("the second line");
            twin.back = Some(1);
        }
        let issues = two_sided_flag_mismatch(&map);
        assert_eq!(issues.len(), 1, "only the unflagged two-sided line reports");
        let issue = issues.first().expect("the mismatch row");
        assert_eq!(issue.linedefs, vec![1], "the offender is attached");
        assert!(
            issue.message.contains("two-sided flag (4) is not set"),
            "the message names the flag: {}",
            issue.message
        );
    }

    #[test]
    fn two_sided_flag_mismatch_names_the_flag_without_a_back_side() {
        let mut map = minimal();
        {
            let bad = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            bad.flags = TWO_SIDED;
        }
        let issues = two_sided_flag_mismatch(&map);
        let issue = issues
            .first()
            .expect("the flag promises a side that is not there");
        assert!(
            issue.message.contains("but has no back side"),
            "the other direction reads the same way: {}",
            issue.message
        );
    }

    #[test]
    fn unused_vertex_finds_the_orphan_with_its_point() {
        let clean = unused_vertex(&minimal());
        assert!(clean.is_empty(), "both base vertices are used");
        let mut map = minimal();
        map.vertices.push(vertex(32, 32));
        let issues = unused_vertex(&map);
        let issue = issues.first().expect("the third vertex is alone");
        assert_eq!(issue.vertices, vec![2], "the orphan index is attached");
        assert_eq!(issue.x, Some(32_i32), "and its coordinates");
        assert_eq!(issue.y, Some(32_i32), "so the editor can jump there");
    }

    #[test]
    fn unused_sidedef_finds_the_side_no_line_uses() {
        let mut map = minimal();
        map.sidedefs.push(side(0));
        let issues = unused_sidedef(&map);
        let issue = issues.first().expect("sidedef 1 was never referenced");
        assert_eq!(issue.sidedefs, vec![1], "the unused sidedef is attached");
    }

    #[test]
    fn unused_sector_warns_for_the_sector_without_borders() {
        let clean = unused_sector(&minimal());
        assert!(clean.is_empty(), "sector 0 borders sidedef 0");
        let mut map = minimal();
        map.sectors.push(Sector::default());
        let issues = unused_sector(&map);
        let issue = issues.first().expect("sector 1 borders nothing");
        assert_eq!(issue.sectors, vec![1], "the orphan sector is attached");
    }

    #[test]
    fn shared_sidedef_lists_the_lines_that_share_one_side() {
        let clean = shared_sidedef(&minimal());
        assert!(clean.is_empty(), "one line per side is the normal shape");
        let mut map = minimal();
        map.linedefs.push(line(1, 0, Some(0)));
        let issues = shared_sidedef(&map);
        let issue = issues.first().expect("both lines use sidedef 0");
        assert_eq!(issue.sidedefs, vec![0], "the shared sidedef is attached");
        assert_eq!(issue.linedefs, vec![0, 1], "and both users");
    }

    #[test]
    fn shared_sidedef_does_not_trip_on_one_line_using_both_sides() {
        let mut map = minimal();
        {
            let bad = map
                .linedefs
                .first_mut()
                .expect("the minimal map has a line");
            bad.back = Some(0);
        }
        let issues = shared_sidedef(&map);
        assert!(
            issues.is_empty(),
            "one linedef is not 'several linedefs': {issues:?}"
        );
    }
}
