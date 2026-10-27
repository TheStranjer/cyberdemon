//! The one index-based union-find the crate shares.
//!
//! `UnionFind` tracks classes over dense `usize` ids: the arrangement's
//! point merge (`geom::segment::PointMerge`) classes candidate points,
//! and the `REJECT` builder classes sector graphs. Links stay shallow:
//! `UnionFind::unite` attaches the smaller class under the bigger, and
//! `UnionFind::unite_keeping` (for callers that must nominate the
//! representative) attaches the other root under the nomination directly.

use alloc::vec;
use alloc::vec::Vec;

/// Union-find over `0..len` dense ids.
pub(crate) struct UnionFind {
    /// Parent link per id; a root points at itself.
    parent: Vec<usize>,
    /// Class size, meaningful at roots only.
    size: Vec<usize>,
}

impl UnionFind {
    /// `len` singleton classes.
    pub(crate) fn with_len(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            size: vec![1_usize; len],
        }
    }

    /// Appends one singleton class and answers its id.
    pub(crate) fn push(&mut self) -> usize {
        let id = self.parent.len();
        self.parent.push(id);
        self.size.push(1_usize);
        id
    }

    /// The class root of `node`; an id past the end is its own root.
    pub(crate) fn find(&self, node: usize) -> usize {
        let mut current = node;
        while self.parent.get(current).copied().unwrap_or(current) != current {
            current = self.parent.get(current).copied().unwrap_or(current);
        }
        current
    }

    /// Joins the classes of `a` and `b`, attaching the smaller under the
    /// bigger so every walk stays logarithmic.
    pub(crate) fn unite(&mut self, a: usize, b: usize) {
        let (root_a, root_b) = (self.find(a), self.find(b));
        if root_a == root_b {
            return;
        }
        let size_a = self.size_of(root_a);
        let size_b = self.size_of(root_b);
        let (child, winner) = if size_a <= size_b {
            (root_a, root_b)
        } else {
            (root_b, root_a)
        };
        self.link(child, winner);
    }

    /// Joins the classes of `a` and `b`, keeping `winner`'s root as the
    /// merged class's representative. `winner` should be one of those two
    /// roots (as the point merge hands out its lexicographic choice);
    /// anything else falls back to the balanced [`UnionFind::unite`].
    pub(crate) fn unite_keeping(&mut self, a: usize, b: usize, winner: usize) {
        let (root_a, root_b) = (self.find(a), self.find(b));
        if root_a == root_b {
            return;
        }
        let (child, champion) = if winner == root_a {
            (root_b, root_a)
        } else if winner == root_b {
            (root_a, root_b)
        } else {
            self.unite(a, b);
            return;
        };
        self.link(child, champion);
    }

    /// The ids that are their own class roots, ascending.
    pub(crate) fn roots(&self) -> Vec<usize> {
        (0..self.parent.len())
            .filter(|id| self.find(*id) == *id)
            .collect()
    }

    /// The class size at a root (`1` for an out-of-range id).
    fn size_of(&self, root: usize) -> usize {
        self.size.get(root).copied().unwrap_or(1_usize)
    }

    /// Attaches one root under another and folds the sizes.
    fn link(&mut self, child: usize, winner: usize) {
        if let Some(slot) = self.parent.get_mut(child) {
            *slot = winner;
        }
        let total = self.size_of(child).saturating_add(self.size_of(winner));
        if let Some(slot) = self.size.get_mut(winner) {
            *slot = total;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UnionFind;

    #[test]
    fn singletons_start_as_their_own_roots() {
        let union = UnionFind::with_len(4);
        for id in 0..4 {
            assert_eq!(union.find(id), id, "{id} is its own root");
        }
        assert_eq!(union.roots(), vec![0_usize, 1, 2, 3], "all singleton");
    }

    #[test]
    fn unite_merges_both_directions_and_finds_walk_chains() {
        let mut union = UnionFind::with_len(5);
        union.unite(0, 1);
        union.unite(1, 2);
        assert_eq!(union.find(2), union.find(0), "transitive class");
        assert_eq!(union.find(3), 3, "untouched stays alone");
        union.unite(4, 2);
        assert_eq!(union.find(4), union.find(0), "the far member joins too");
        let roots = union.roots();
        assert_eq!(roots.len(), 2, "two classes: 0..2 and 3");
        assert!(roots.contains(&union.find(0)), "the class root is listed");
    }

    #[test]
    fn unite_is_idempotent_on_one_class() {
        let mut union = UnionFind::with_len(3);
        union.unite(0, 1);
        let after = union.find(1);
        union.unite(0, 1);
        union.unite(1, 0);
        assert_eq!(union.find(1), after, "re-merging changes nothing");
        assert_eq!(union.find(2), 2, "the outsider stays out");
    }

    #[test]
    fn unite_keeping_nominates_the_representative() {
        let mut union = UnionFind::with_len(4);
        union.unite(0, 1);
        union.unite_keeping(2, 0, 2);
        assert_eq!(union.find(0), 2, "the nominated root wins");
        assert_eq!(union.find(1), 2, "and carries its whole class");
        union.unite_keeping(3, 3, 3);
        assert_eq!(union.find(3), 3, "self-merge changes nothing");
    }

    #[test]
    fn unite_keeping_falls_back_when_the_winner_is_in_no_class() {
        let mut union = UnionFind::with_len(3);
        union.unite_keeping(0, 1, 99);
        assert_eq!(union.find(0), union.find(1), "the classes still merge");
        assert_ne!(union.find(0), 99, "a foreign id never becomes a root");
    }

    #[test]
    fn push_appends_singletons_past_the_initial_len() {
        let mut union = UnionFind::with_len(1);
        let second = union.push();
        assert_eq!(second, 1, "the new id continues the range");
        assert_eq!(union.find(second), second, "a fresh push is a root");
        union.unite(0, second);
        assert_eq!(union.find(second), union.find(0), "and joins like any id");
    }

    #[test]
    fn out_of_range_ids_are_their_own_roots() {
        let union = UnionFind::with_len(2);
        assert_eq!(union.find(77), 77, "a forgotten id answers itself");
    }
}
