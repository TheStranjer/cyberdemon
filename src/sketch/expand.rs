//! Growing labelled regions into the closed walls, and reading the region
//! adjacency that growth creates.
//!
//! [`expand_regions`] runs a deterministic multi-source breadth-first flood
//! from every labelled pixel into the wall raster: after `steps` rings a
//! region has eaten up to `steps` pixels of wall, and two regions on the
//! ends of one wall of at most `2 * steps` pixels meet inside it. Ties in
//! the same ring go to the lower region id, so the split is reproducible.
//! Paper that belongs to no region is never claimed: growth only walks
//! pixels set in the closed wall mask. [`adjacency`] then counts, per pair
//! of region ids, the 4-neighbour pixel pairs that share a border, which is
//! how the plan builder learns which rooms touch.

use alloc::collections::BTreeMap;

use crate::sketch::mask::Mask;

use super::components::Labels;

/// One touching pair of regions from [`adjacency`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adjacent {
    /// Lower region id of the pair.
    pub a: u32,
    /// Higher region id of the pair.
    pub b: u32,
    /// 4-neighbour pixel pairs on the shared border.
    pub shared_px: u32,
}

/// Grows every labelled region into the closed walls, ring by ring.
///
/// A 4-connected, multi-source breadth-first flood walks from all labelled
/// pixels at most `steps` rings outward into `walls_closed`. A free wall
/// pixel reached by several regions in the same ring goes to the lowest
/// id; unlabelled paper is never claimed. Queue order is raster scan
/// order, so the result is deterministic.
///
/// The [`vectorize`](super::vectorize::vectorize) default `steps` is
/// `(stroke + 1) / 2 + close_radius`, enough rings to reach the centre of
/// a stroke from the region behind it.
#[must_use]
#[inline]
pub fn expand_regions(regions: &Labels, walls_closed: &Mask, steps: u32) -> Labels {
    let mut ids = regions.ids.clone();
    let mut frontier: Vec<usize> = (0..ids.len())
        .filter(|index| ids.get(*index).copied().unwrap_or(0) != 0)
        .collect();
    let Some(width) = usize::try_from(regions.width).ok().filter(|&w| w > 0) else {
        return Labels {
            width: regions.width,
            height: regions.height,
            ids,
        };
    };
    for _ in 0..steps {
        let claims = ring_claims(&ids, walls_closed, &frontier, width);
        if claims.is_empty() {
            break;
        }
        write_claims(&mut ids, &claims);
        frontier = claims.keys().copied().collect();
    }
    Labels {
        width: regions.width,
        height: regions.height,
        ids,
    }
}

/// Writes every claim of a finished ring into the id raster.
fn write_claims(ids: &mut [u32], claims: &BTreeMap<usize, u32>) {
    for (&index, &id) in claims {
        if let Some(slot) = ids.get_mut(index) {
            *slot = id;
        }
    }
}

/// Mutable state of one expansion ring, kept out of the loop's argument
/// list.
struct Ring<'grow> {
    /// Ids assigned so far; `0` means free.
    ids: &'grow [u32],
    /// The closed wall raster growth may claim from.
    walls: &'grow Mask,
    /// Best claim on each pixel this ring.
    claims: BTreeMap<usize, u32>,
}

impl Ring<'_> {
    /// Offers one wall pixel to `owner`; the lowest id wins a same-ring
    /// tie.
    fn offer(&mut self, neighbour: usize, owner: u32) {
        let free = self.ids.get(neighbour).copied().unwrap_or(1) == 0;
        let wall = self.walls.bits.get(neighbour).copied().unwrap_or(false);
        if !free || !wall {
            return;
        }
        let entry = self.claims.entry(neighbour).or_insert(u32::MAX);
        *entry = (*entry).min(owner);
    }
}

/// The id claims for one ring: every free, wall-set 4-neighbour of the
/// frontier pixels mapped to the lowest region id that reaches it.
fn ring_claims(
    ids: &[u32],
    walls_closed: &Mask,
    frontier: &[usize],
    width: usize,
) -> BTreeMap<usize, u32> {
    let mut ring = Ring {
        ids,
        walls: walls_closed,
        claims: BTreeMap::new(),
    };
    for &index in frontier {
        let Some(owner) = ids.get(index).copied().filter(|&id| id != 0) else {
            continue;
        };
        for neighbour in neighbours(index, width, ids.len()) {
            ring.offer(neighbour, owner);
        }
    }
    ring.claims
}

/// The in-bounds 4-neighbours of a row-major index in a raster of `width`
/// columns and `len` pixels.
fn neighbours(index: usize, width: usize, len: usize) -> impl Iterator<Item = usize> {
    let x = index.checked_rem(width).unwrap_or(0);
    let last_in_row = x.checked_add(1) == Some(width);
    [
        index.checked_sub(width),
        if x == 0 { None } else { index.checked_sub(1) },
        if last_in_row {
            None
        } else {
            index.checked_add(1)
        },
        index.checked_add(width),
    ]
    .into_iter()
    .flatten()
    .filter(move |&other| other < len)
}

/// Every pair of regions that share at least one border of 4-neighbour
/// pixels, sorted by `(a, b)`. Each touching pixel pair is counted once
/// (`shared_px`), scanning each pixel's right and down neighbours.
#[must_use]
#[inline]
pub fn adjacency(labels: &Labels) -> Vec<Adjacent> {
    let Some(width) = usize::try_from(labels.width).ok().filter(|&w| w > 0) else {
        return Vec::new();
    };
    let mut counts: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let filled = labels
        .ids
        .iter()
        .enumerate()
        .filter(|&(_, &pixel)| pixel != 0);
    for (index, &id) in filled {
        for other in right_and_below(labels, index, width) {
            record_pair(&mut counts, id, other);
        }
    }
    counts
        .into_iter()
        .map(|((a, b), shared_px)| Adjacent { a, b, shared_px })
        .collect()
}

/// The ids of the right and down neighbours of `index`, `0` included so
/// the caller sees every contact; diagonals never touch.
fn right_and_below(labels: &Labels, index: usize, width: usize) -> Vec<u32> {
    let x = index.checked_rem(width).unwrap_or(0);
    let last_in_row = x.checked_add(1) == Some(width);
    let len = labels.ids.len();
    [
        if last_in_row {
            None
        } else {
            index.checked_add(1)
        },
        index.checked_add(width).filter(|&below| below < len),
    ]
    .into_iter()
    .flatten()
    .filter_map(|other| labels.ids.get(other).copied())
    .collect::<Vec<u32>>()
}

/// Counts one contact between two different non-zero region ids.
fn record_pair(counts: &mut BTreeMap<(u32, u32), u32>, one: u32, other: u32) {
    if other == 0 || other == one {
        return;
    }
    let key = (one.min(other), one.max(other));
    let entry = counts.entry(key).or_insert(0_u32);
    *entry = entry.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use super::{Adjacent, adjacency, expand_regions};
    use crate::sketch::components::Labels;
    use crate::sketch::mask::Mask;

    /// An id with the inclusive box to paint under it.
    type Painted = (u32, (u32, u32, u32, u32));

    /// A `width` x `height` label raster with inclusive boxes painted.
    fn boxes(width: u32, height: u32, painted: &[Painted]) -> Labels {
        let mut labels = Labels::blank(width, height);
        for &(id, box_) in painted {
            fill_labels(&mut labels, id, box_);
        }
        labels
    }

    /// Paints the inclusive rectangle into `labels` under `id`.
    fn fill_labels(labels: &mut Labels, id: u32, (x0, y0, x1, y1): (u32, u32, u32, u32)) {
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            assert!(labels.set(x, y, id), "fixture in bounds");
        }
    }

    /// A `width` x `height` mask with the inclusive box set.
    fn wall(width: u32, height: u32, box_: (u32, u32, u32, u32)) -> Mask {
        let (x0, y0, x1, y1) = box_;
        let mut mask = Mask::blank(width, height);
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            assert!(mask.set(x, y, true), "fixture in bounds");
        }
        mask
    }

    #[test]
    fn two_regions_meet_in_the_middle_of_a_three_pixel_wall() {
        let labels = boxes(50, 20, &[(1, (0, 5, 9, 15)), (2, (13, 5, 22, 15))]);
        let walls = wall(50, 20, (10, 5, 12, 15));
        let grown = expand_regions(&labels, &walls, 8);
        assert_eq!(
            grown.get(10, 10),
            Some(1),
            "left wall pixel goes to region 1"
        );
        assert_eq!(
            grown.get(11, 10),
            Some(1),
            "the middle pixel is a tie: lower id"
        );
        assert_eq!(
            grown.get(12, 10),
            Some(2),
            "right wall pixel goes to region 2"
        );
        assert_eq!(
            grown.get(0, 0),
            Some(0),
            "paper outside the wall stays free"
        );
        let pairs = adjacency(&grown);
        assert_eq!(
            pairs,
            vec![Adjacent {
                a: 1,
                b: 2,
                shared_px: 11,
            }],
            "one pair meeting along the wall height"
        );
    }

    #[test]
    fn growth_never_claims_unlabelled_paper() {
        let labels = boxes(10, 10, &[(1, (0, 0, 2, 9))]);
        let grown = expand_regions(&labels, &Mask::blank(10, 10), 9);
        assert_eq!(grown.get(3, 3), Some(0), "paper beside the region stays 0");
        assert_eq!(grown.get(0, 0), Some(1), "the region itself is kept");
        assert!(adjacency(&grown).is_empty(), "one region touches nothing");
    }

    #[test]
    fn steps_cap_the_rings_and_diagonals_never_touch() {
        let labels = boxes(10, 10, &[(1, (0, 0, 0, 9)), (2, (4, 0, 9, 9))]);
        let walls = wall(10, 10, (1, 0, 3, 9));
        let capped = expand_regions(&labels, &walls, 1);
        assert_eq!(
            capped.get(1, 0),
            Some(1),
            "one ring reaches the first column"
        );
        assert_eq!(capped.get(2, 0), Some(0), "the second ring was not run");
        let mut diagonal = Labels::blank(3, 3);
        assert!(diagonal.set(0, 0, 1), "in bounds");
        assert!(diagonal.set(2, 2, 2), "in bounds");
        assert!(
            adjacency(&diagonal).is_empty(),
            "a diagonal corner is no contact"
        );
    }

    #[test]
    fn adjacency_pairs_are_sorted_and_counted_once_per_pixel_pair() {
        let labels = boxes(6, 3, &[(3, (2, 0, 5, 2)), (2, (0, 0, 1, 2))]);
        let pairs = adjacency(&labels);
        assert_eq!(
            pairs,
            vec![Adjacent {
                a: 2,
                b: 3,
                shared_px: 3,
            }],
            "keys normalise to a < b, one count per row"
        );
    }

    #[test]
    fn empty_shapes_expand_and_meet_nothing() {
        let grown = expand_regions(&Labels::blank(0, 0), &Mask::blank(0, 0), 4);
        assert!(grown.ids.is_empty(), "an empty raster stays empty");
        assert!(
            adjacency(&grown).is_empty(),
            "nothing is adjacent to nothing"
        );
    }
}
