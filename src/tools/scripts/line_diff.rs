//! Minimal deterministic line diffs: the counts behind `edit_dehacked`'s
//! `added`/`removed`/`changed` report.
//!
//! [`line_diff`] compares two texts line by line the way a minimal `LCS`
//! diff does: the shared prefix and suffix are matched first, the middle is
//! walked with a longest-common-subsequence dynamic program, and every run
//! of unpaired lines between two matches forms one block. A block of `o`
//! old and `n` new lines pairs `min(o, n)` of them off as `changed` (one
//! old line rewritten into one new line) and reports the rest as `removed`
//! or `added`. So inserting one line anywhere reports `added: 1` with the
//! other two counts untouched, however long the file is.
//!
//! The exact walk needs `(old middle + 1) * (new middle + 1)` table cells.
//! Above [`MAX_DIFF_CELLS`] it is skipped and the whole middle counts as
//! one block; that is exact when the two middles share no line (the
//! wholesale-replacement case, checked up front) and a conservative
//! pair-off otherwise.

use std::collections::HashSet;

/// The largest `LCS` table the exact walk builds: two million `usize`
/// cells, about 16 MiB.
const MAX_DIFF_CELLS: usize = 2_000_000;

/// The line counts of one [`line_diff`] answer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct LineDiff {
    /// Lines only the new text has.
    pub(super) added: usize,
    /// Lines only the old text had.
    pub(super) removed: usize,
    /// Lines the diff pairs off as rewritten in place.
    pub(super) changed: usize,
}

/// The minimal line diff of two texts (an absent base is the empty text).
pub(super) fn line_diff(old: &str, new: &str) -> LineDiff {
    let old_lines = old.lines().collect::<Vec<&str>>();
    let new_lines = new.lines().collect::<Vec<&str>>();
    counts(&old_lines, &new_lines)
}

/// The counts for two line lists: shared prefix and suffix cut, the middle
/// walked exactly or counted as one block.
fn counts(old: &[&str], new: &[&str]) -> LineDiff {
    let prefix = common_prefix(old, new);
    let old_rest = tail_from(old, prefix);
    let new_rest = tail_from(new, prefix);
    let suffix = common_suffix(old_rest, new_rest);
    let old_mid = head_before_end(old_rest, suffix);
    let new_mid = head_before_end(new_rest, suffix);
    if old_mid.is_empty()
        || new_mid.is_empty()
        || share_no_line(old_mid, new_mid)
        || too_wide(old_mid, new_mid)
    {
        return block(old_mid.len(), new_mid.len());
    }
    walk(old_mid, new_mid)
}

/// The number of equal lines both lists share from the front.
fn common_prefix(old: &[&str], new: &[&str]) -> usize {
    old.iter()
        .zip(new.iter())
        .take_while(|pair| pair.0 == pair.1)
        .count()
}

/// The number of equal lines both lists share from the back, over lists
/// already cut to a common prefix.
fn common_suffix(old: &[&str], new: &[&str]) -> usize {
    old.iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|pair| pair.0 == pair.1)
        .count()
}

/// The lines of `lines` from index `at` on.
fn tail_from<'list, 'line>(lines: &'list [&'line str], at: usize) -> &'list [&'line str] {
    lines
        .split_at_checked(at)
        .map_or(lines, |(_head, tail)| tail)
}

/// The lines of `lines` before its last `suffix` ones.
fn head_before_end<'list, 'line>(lines: &'list [&'line str], suffix: usize) -> &'list [&'line str] {
    let stop = lines.len().saturating_sub(suffix);
    lines
        .split_at_checked(stop)
        .map_or(lines, |(head, _tail)| head)
}

/// Whether the two middles hold no line in common, so the whole middle is
/// one replacement block and no walk can find a match.
fn share_no_line(old: &[&str], new: &[&str]) -> bool {
    let (small, large) = if old.len() <= new.len() {
        (old, new)
    } else {
        (new, old)
    };
    let held: HashSet<&str> = small.iter().copied().collect();
    !large.iter().any(|line| held.contains(line))
}

/// Whether the exact walk would need more than [`MAX_DIFF_CELLS`] cells.
fn too_wide(old: &[&str], new: &[&str]) -> bool {
    old.len()
        .checked_mul(new.len())
        .is_none_or(|cells| cells > MAX_DIFF_CELLS)
}

/// The counts of one replacement block: `dropped` old lines became
/// `inserted` new lines, the shared part of the two counts as rewritten
/// and the rest as removed or added.
const fn block(dropped: usize, inserted: usize) -> LineDiff {
    let changed = if dropped < inserted {
        dropped
    } else {
        inserted
    };
    LineDiff {
        added: inserted.saturating_sub(changed),
        removed: dropped.saturating_sub(changed),
        changed,
    }
}

/// Adds one block's counts into the running tally.
const fn fold(tally: &mut LineDiff, dropped: usize, inserted: usize) {
    let step = block(dropped, inserted);
    tally.added = tally.added.saturating_add(step.added);
    tally.removed = tally.removed.saturating_add(step.removed);
    tally.changed = tally.changed.saturating_add(step.changed);
}

/// The suffix-`LCS` dynamic-programming table over two middle parts: the
/// cell at `(row, column)` holds the length of the longest common
/// subsequence of `old[row..]` and `new[column..]`, kept flat in row-major
/// order.
struct Table<'slice, 'line> {
    /// Cells per row: the new side's line count plus the zero column.
    columns: usize,
    /// The cells, filled bottom-up.
    cells: Vec<usize>,
    /// The old middle.
    old: &'slice [&'line str],
    /// The new middle.
    new: &'slice [&'line str],
}

impl<'slice, 'line> Table<'slice, 'line> {
    /// The table over the two middles, every cell filled.
    fn build(old: &'slice [&'line str], new: &'slice [&'line str]) -> Self {
        let columns = new.len().saturating_add(1);
        let rows = old.len().saturating_add(1);
        let mut table = Self {
            columns,
            cells: vec![0_usize; rows.saturating_mul(columns)],
            old,
            new,
        };
        for row in (0..old.len()).rev() {
            table.fill_row(row);
        }
        table
    }

    /// The `LCS` length to store at `(row, column)`, read from the cells
    /// below and right of it.
    fn value_at(&self, row: usize, column: usize) -> usize {
        let below = row.saturating_add(1);
        let right = column.saturating_add(1);
        if self.old.get(row) == self.new.get(column) {
            self.cell(below, right).saturating_add(1)
        } else {
            self.cell(below, column).max(self.cell(row, right))
        }
    }

    /// Fills one row, right to left.
    fn fill_row(&mut self, row: usize) {
        let width = self.new.len();
        for column in (0..width).rev() {
            let value = self.value_at(row, column);
            self.set(row, column, value);
        }
    }

    /// The flat index of the cell at `(row, column)`.
    const fn index(&self, row: usize, column: usize) -> usize {
        self.columns.saturating_mul(row).saturating_add(column)
    }

    /// The cell at `(row, column)`; every index the walk forms is in
    /// range, and the padded base row and column read zero.
    fn cell(&self, row: usize, column: usize) -> usize {
        let at = self.index(row, column);
        self.cells.get(at).copied().unwrap_or_default()
    }

    /// Writes the cell at `(row, column)`.
    fn set(&mut self, row: usize, column: usize, value: usize) {
        let at = self.index(row, column);
        if let Some(slot) = self.cells.get_mut(at) {
            *slot = value;
        }
    }
}

/// The exact `LCS` walk over the two middles, folding one block into the
/// tally at every match and once more for the trailing run.
fn walk(old: &[&str], new: &[&str]) -> LineDiff {
    let table = Table::build(old, new);
    let mut tally = LineDiff::default();
    let mut dropped = 0_usize;
    let mut inserted = 0_usize;
    let mut row = 0_usize;
    let mut column = 0_usize;
    while row < old.len() && column < new.len() {
        if old.get(row) == new.get(column) {
            fold(&mut tally, dropped, inserted);
            dropped = 0_usize;
            inserted = 0_usize;
            row = row.saturating_add(1);
            column = column.saturating_add(1);
        } else if table.cell(row.saturating_add(1), column)
            >= table.cell(row, column.saturating_add(1))
        {
            dropped = dropped.saturating_add(1);
            row = row.saturating_add(1);
        } else {
            inserted = inserted.saturating_add(1);
            column = column.saturating_add(1);
        }
    }
    dropped = dropped.saturating_add(old.len().saturating_sub(row));
    inserted = inserted.saturating_add(new.len().saturating_sub(column));
    fold(&mut tally, dropped, inserted);
    tally
}

#[cfg(test)]
mod tests {
    use super::{LineDiff, MAX_DIFF_CELLS, line_diff};

    /// The counts as the `(added, removed, changed)` triple.
    fn triple(old: &str, new: &str) -> (usize, usize, usize) {
        let found = line_diff(old, new);
        (found.added, found.removed, found.changed)
    }

    /// A text of `count` distinct numbered lines, optionally reversed.
    fn numbered(count: usize, reversed: bool) -> String {
        let mut rows: Vec<String> = (0..count).map(|row| format!("line {row}")).collect();
        if reversed {
            rows.reverse();
        }
        let mut text = rows.join("\n");
        text.push('\n');
        text
    }

    /// The block counts a full middle replacement answers with.
    const fn all_changed(count: usize) -> LineDiff {
        LineDiff {
            added: 0,
            removed: 0,
            changed: count,
        }
    }

    #[test]
    fn an_identical_text_counts_nothing() {
        assert_eq!(triple("a\nb\n", "a\nb\n"), (0, 0, 0), "no change at all");
        assert_eq!(triple("", ""), (0, 0, 0), "two empty texts");
    }

    #[test]
    fn a_line_inserted_at_the_top_counts_as_one_added() {
        let base = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let shifted = format!("0\n{base}");
        assert_eq!(
            triple(base, &shifted),
            (1, 0, 0),
            "the positional count would say every line differs"
        );
    }

    #[test]
    fn a_line_inserted_in_the_middle_counts_as_one_added() {
        assert_eq!(
            triple("1\n2\n4\n", "1\n2\n3\n4\n"),
            (1, 0, 0),
            "the insert alone"
        );
    }

    #[test]
    fn one_rewritten_line_counts_as_one_changed() {
        assert_eq!(
            triple("a\nb\nc\n", "a\nx\nc\n"),
            (0, 0, 1),
            "a rewrite is not an add and a remove"
        );
    }

    #[test]
    fn one_dropped_line_counts_as_one_removed() {
        assert_eq!(triple("a\nb\nc\n", "a\nc\n"), (0, 1, 0), "one line went");
    }

    #[test]
    fn an_absent_base_counts_every_line_as_added() {
        assert_eq!(triple("", "a\nb\n"), (2, 0, 0), "a created patch");
        assert_eq!(triple("a\nb\n", ""), (0, 2, 0), "an emptied patch");
    }

    #[test]
    fn a_two_line_rewrite_counts_as_two_changed() {
        assert_eq!(
            triple("a\nb\nc\nd\n", "a\nx\ny\nd\n"),
            (0, 0, 2),
            "the block pairs the two off"
        );
    }

    #[test]
    fn a_disjoint_replacement_pairs_the_shorter_side() {
        assert_eq!(
            triple("a\nb\n", "x\ny\nz\n"),
            (1, 0, 2),
            "two rewrites and one true insert"
        );
    }

    #[test]
    fn a_moved_block_reports_removed_and_added_lines() {
        assert_eq!(
            triple("a\nb\nc\nd\n", "b\nc\nd\na\n"),
            (1, 1, 0),
            "the minimum is one line moved down"
        );
    }

    #[test]
    fn interleaved_matches_pair_only_their_own_blocks() {
        assert_eq!(
            triple(
                "k1\nold1\nk2\nold2\nk3\n",
                "k1\nnew1\nnew2\nnew3\nk2\nnew4\nk3\n"
            ),
            (2, 0, 2),
            "the two old lines pair off as rewrites, two new lines are true adds"
        );
    }

    #[test]
    fn trailing_newlines_do_not_add_a_line_to_the_counts() {
        assert_eq!(triple("a\n", "a\nb\n"), (1, 0, 0), "one new line");
        assert_eq!(
            triple("a\nb", "a\nb\n"),
            (0, 0, 0),
            "a terminator, not a line"
        );
    }

    /// A line count whose middle is wider than [`MAX_DIFF_CELLS`] cells.
    const WIDE: usize = 1_500;

    #[test]
    fn an_enormous_shared_middle_falls_back_to_block_counts() {
        assert!(
            WIDE.saturating_mul(WIDE) > MAX_DIFF_CELLS,
            "WIDE really is past the table cap"
        );
        let old = numbered(WIDE, false);
        let new = numbered(WIDE, true);
        let found = line_diff(&old, &new);
        assert_eq!(
            found,
            all_changed(WIDE),
            "the reversal is counted as one full rewrite of the middle"
        );
    }

    #[test]
    fn an_enormous_disjoint_middle_needs_no_table() {
        let old = numbered(WIDE, false);
        let mut new: String = (0..WIDE)
            .map(|row| format!("other {row}"))
            .collect::<Vec<String>>()
            .join("\n");
        new.push('\n');
        let found = line_diff(&old, &new);
        assert_eq!(found, all_changed(WIDE), "exact: nothing could match");
    }
}
