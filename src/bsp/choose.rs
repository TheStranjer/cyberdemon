//! Partition candidate lines and the cost that picks one.
//!
//! The candidates for a region are the distinct partition lines of its
//! segs: collinear lines with the same direction count once, in order of
//! first appearance. More than [`max_candidates`](crate::bsp::BuildOptions::max_candidates)
//! candidates are sampled evenly. A candidate's cost is `splits * split_cost + |right - left|`
//! with split segs counted on both sides; a candidate that would leave
//! one child empty is invalid. The lowest cost wins, ties go to the
//! earliest candidate.

use crate::bsp::BuildOptions;
use crate::bsp::Partition;
use crate::bsp::seg::WorkSeg;
use crate::bsp::side::{FixedLine, SegPlace, classify_seg};

/// Whether two partitions are the same line, same direction: parallel
/// equal-direction vectors through a common point.
fn same_line(a: Partition, b: Partition) -> bool {
    let adx = i128::from(a.dx);
    let ady = i128::from(a.dy);
    let bdx = i128::from(b.dx);
    let bdy = i128::from(b.dy);
    let parallel = adx
        .saturating_mul(bdy)
        .saturating_sub(ady.saturating_mul(bdx));
    let same_way = adx
        .saturating_mul(bdx)
        .saturating_add(ady.saturating_mul(bdy));
    let on_line = adx
        .saturating_mul(i128::from(b.y).saturating_sub(i128::from(a.y)))
        .saturating_sub(ady.saturating_mul(i128::from(b.x).saturating_sub(i128::from(a.x))));
    parallel == 0_i128 && same_way > 0_i128 && on_line == 0_i128
}

/// The distinct partition lines of `set`, first-appearance order.
fn distinct_partitions(set: &[WorkSeg]) -> Vec<Partition> {
    let mut out: Vec<Partition> = Vec::new();
    for work in set {
        if !out.iter().any(|seen| same_line(*seen, work.partition)) {
            out.push(work.partition);
        }
    }
    out
}

/// `values` truncated to `max` entries at even spacing, first and last
/// reachable index preserved. `max == 0` yields nothing.
fn even_sample(values: &[Partition], max: usize) -> Vec<Partition> {
    let len = values.len();
    if len <= max {
        return values.to_vec();
    }
    (0..max)
        .filter_map(|slot| {
            let index = slot.saturating_mul(len).checked_div(max)?;
            values.get(index).copied()
        })
        .collect()
}

/// The cost of scoring `line` over `set`, or `None` when a child would
/// be empty. Split segs are counted on both sides.
fn region_cost(line: &FixedLine, set: &[WorkSeg], split_cost: i64) -> Option<i128> {
    let mut right = 0_i64;
    let mut left = 0_i64;
    let mut splits = 0_i64;
    for work in set {
        match classify_seg(line, &work.seg) {
            SegPlace::Right => right = right.saturating_add(1),
            SegPlace::Left => left = left.saturating_add(1),
            SegPlace::Crossing => {
                splits = splits.saturating_add(1);
                right = right.saturating_add(1);
                left = left.saturating_add(1);
            }
        }
    }
    if right == 0_i64 || left == 0_i64 {
        return None;
    }
    let imbalance = i128::from(right.saturating_sub(left).saturating_abs());
    Some(
        i128::from(splits)
            .saturating_mul(i128::from(split_cost))
            .saturating_add(imbalance),
    )
}

/// The best partition line for `set` under `opts`, or `None` when no
/// candidate scores on both sides.
pub(crate) fn choose_partition(set: &[WorkSeg], opts: BuildOptions) -> Option<Partition> {
    let mut best: Option<(i128, Partition)> = None;
    for partition in even_sample(&distinct_partitions(set), opts.max_candidates) {
        let line = FixedLine::from_partition(partition);
        let Some(cost) = region_cost(&line, set, opts.split_cost) else {
            continue;
        };
        if best.is_none_or(|(lowest, _)| cost < lowest) {
            best = Some((cost, partition));
        }
    }
    best.map(|(_, partition)| partition)
}

#[cfg(test)]
mod tests {
    use super::{choose_partition, distinct_partitions, even_sample, same_line};
    use crate::bsp::BuildOptions;
    use crate::bsp::Partition;
    use crate::bsp::seg::WorkSeg;
    use crate::bsp::side::FixedLine;
    use crate::wad::map::Vertex;

    /// A seg between whole-unit endpoints on the given partition line.
    fn work(from: (i64, i64), to: (i64, i64), partition: Partition) -> WorkSeg {
        let scale = |value: i64| value.checked_mul(0x1_0000).unwrap();
        let (x1, y1) = from;
        let (x2, y2) = to;
        WorkSeg {
            seg: crate::bsp::BuildSeg {
                start: crate::bsp::Point {
                    x: scale(x1),
                    y: scale(y1),
                },
                end: crate::bsp::Point {
                    x: scale(x2),
                    y: scale(y2),
                },
                linedef: 0,
                side: 0,
                offset: 0,
                sector: 0,
            },
            partition,
        }
    }

    /// The partition of `from` to `to` on `side`.
    fn whole(value: i64) -> i32 {
        i32::try_from(value).unwrap()
    }

    /// The partition of `from` to `to` on `side`.
    fn partition(from: (i64, i64), to: (i64, i64), side: u8) -> Partition {
        let (x1, y1) = from;
        let (x2, y2) = to;
        Partition::from_vertices(
            Vertex {
                x: whole(x1),
                y: whole(y1),
            },
            Vertex {
                x: whole(x2),
                y: whole(y2),
            },
            side,
        )
    }

    #[test]
    fn same_line_dedupes_parallel_collinear_lines_only_when_equal_way() {
        let east = partition((0, 0), (10, 0), 0);
        let east_again = partition((10, 0), (20, 0), 0);
        let west = partition((20, 0), (10, 0), 0);
        let north = partition((0, 0), (0, 10), 0);
        let offset = partition((0, 5), (10, 5), 0);
        assert!(same_line(east, east_again), "same line, same way");
        assert!(!same_line(east, west), "same line, opposite way");
        assert!(!same_line(east, north), "different direction");
        assert!(!same_line(east, offset), "parallel, not collinear");
    }

    #[test]
    fn distinct_keeps_first_appearance_order() {
        let east = partition((0, 0), (10, 0), 0);
        let west = partition((10, 0), (0, 0), 0);
        let north = partition((0, 0), (0, 10), 0);
        let set = [
            work((0, 0), (10, 0), east),
            work((0, 0), (0, 10), north),
            work((10, 0), (0, 0), west),
            work((20, 0), (30, 0), partition((20, 0), (30, 0), 0)),
        ];
        assert_eq!(
            distinct_partitions(&set),
            vec![east, north, west],
            "the fourth seg duplicates the first line"
        );
    }

    #[test]
    fn even_sample_shortens_long_lists_at_even_spacing() {
        let values: Vec<Partition> = (0_i64..10_i64)
            .map(|slot| partition((slot, 0), (slot.checked_add(1).unwrap(), 1), 0))
            .collect();
        let sampled = even_sample(&values, 4);
        assert_eq!(sampled.len(), 4, "capped at the request");
        assert_eq!(sampled[0], values[0], "first kept");
        assert_eq!(sampled[3], values[7], "slot 3 of 10 over 4 is 7");
        assert_eq!(
            even_sample(&values, 10).len(),
            10,
            "exact fit passes through"
        );
        assert_eq!(
            even_sample(&values, 32).len(),
            10,
            "short lists pass through"
        );
        assert!(even_sample(&values, 0).is_empty(), "zero takes nothing");
    }

    #[test]
    fn region_cost_counts_splits_on_both_sides_and_rejects_empty_children() {
        let east_line = FixedLine {
            x: 0,
            y: 0,
            dx: 0x1_0000,
            dy: 0,
        };
        // One south seg (right of an east line), one north seg (left),
        // one seg crossing y = 0.
        let set = [
            work((10, -10), (10, -5), partition((10, -10), (10, -5), 0)),
            work((20, 5), (20, 10), partition((20, 5), (20, 10), 0)),
            work((-5, -5), (5, 5), partition((-5, -5), (5, 5), 0)),
        ];
        // right = 1 + 1 split, left = 1 + 1 split, splits = 1.
        assert_eq!(
            super::region_cost(&east_line, &set, 8_i64),
            Some(8_i128),
            "8 per split plus a balanced count"
        );
        assert_eq!(
            super::region_cost(&east_line, &set, 2_i64),
            Some(2_i128),
            "the parameter scales the split term"
        );
        // A line with nothing strictly left of it: only the seg on the
        // line itself, counted right, so the left child would be empty.
        let alone = [work((0, 0), (10, 0), partition((0, 0), (10, 0), 0))];
        assert_eq!(
            super::region_cost(&east_line, &alone, 8_i64),
            None,
            "no left segs"
        );
    }

    #[test]
    fn ties_go_to_the_earliest_candidate_and_invalid_ones_are_skipped() {
        // Two opposite segs on one line: both directions score splits
        // none, balance perfect, cost zero; the first must win.
        let east = partition((0, 0), (10, 0), 0);
        let west = partition((10, 0), (0, 0), 0);
        let pair = [work((0, 0), (10, 0), east), work((10, 0), (0, 0), west)];
        assert_eq!(
            choose_partition(&pair, BuildOptions::default()),
            Some(east),
            "earliest at the tie"
        );
        assert_eq!(
            choose_partition(
                &pair,
                BuildOptions {
                    max_candidates: 1,
                    ..BuildOptions::default()
                }
            ),
            Some(east),
            "sampling keeps the first"
        );
        // Zero candidates can never score.
        assert!(
            choose_partition(
                &pair,
                BuildOptions {
                    max_candidates: 0,
                    ..BuildOptions::default()
                }
            )
            .is_none(),
            "zero candidates, no choice"
        );
    }

    #[test]
    fn collinear_same_way_segs_have_no_valid_split() {
        // Everything on one line running its own direction: the only
        // distinct candidate is the line itself, which counts them all
        // right and leaves no left child.
        let east = partition((0, 0), (64, 0), 0);
        let same = partition((64, 0), (128, 0), 0);
        let set = [work((0, 0), (64, 0), east), work((64, 0), (128, 0), same)];
        assert_eq!(
            choose_partition(&set, BuildOptions::default()),
            None,
            "one line, nothing on its left"
        );
    }
}
