//! The Doom ↔ ZDoom linedef-special translation tables. No MCP concerns.
//!
//! `ZDoom` keeps the vanilla `Doom` linedef specials working by translating
//! each one into a generalized action number plus a fixed argument template:
//! the table pairs every legacy special with its `zspecial`, the activation
//! conditions the trigger keeps, and the arguments to write, with the
//! line's tag dropped into the single slot the table marks `tag`.
//! [`parse_xlat_table`] reads the tab-separated rows of
//! `data/xlat/doom_to_zdoom.tsv`; [`xlat_rows`], [`rows_for_doom`] and
//! [`rows_for_zspecial`] hand out the embedded rows for the cross-family
//! conversion built on top of this module. The embedded table is parsed
//! once on first use behind a `LazyLock`; a parse failure means the shipped
//! data is broken, so every accessor reports it as
//! [`crate::error::CyberError::Internal`], never a
//! panic, exactly like [`crate::defs::registry`].
//!
//! The module also holds the thing-flag, sector-type and linedef stages
//! of the cross-family `Doom` ↔ `Hexen` conversion: [`things_to_hexen`]
//! and [`things_from_hexen`] translate [`Thing`] presence flags,
//! [`sectors_to_hexen`] and [`sectors_from_hexen`] the Boom-style sector
//! effects, each returning a [`FieldReport`] of the elements it changed
//! and the values with no equivalent it dropped, and
//! [`linedefs_to_hexen`] and [`linedefs_from_hexen`] run the table over
//! the [`Linedef`] specials in either direction, returning a
//! [`LinesReport`] of the specials that made the trip and the ids, flags
//! and values that could not. They are pure in-place stages over the
//! format-neutral [`Map`], run by the cross-family directions of
//! [`super::convert`].
//!
//! This root holds the report shapes every stage answers with and the
//! helpers they share: `move_field`, `count_if`, `count_bit` and
//! `show_indices` with `GROUP_LIST_LIMIT`, the `HexenDrops` bit tally and
//! the two field masks `FIELD_ONE` and `FIELD_TWO`. Five private modules
//! carry the rest, all re-exported below: `table` is the embedded TSV table
//! with its rows and accessors, `things` and `sectors` are the thing-flag
//! and sector-type stages, and `lines_to` and `lines_from` are the two
//! directions of the linedef stage. The stages' tests are the private
//! `tests` module, its builders shared, the table, the thing and sector
//! stages and the linedef round trips, loss warnings and tag rules one
//! topic file apiece.
//!
//! [`Linedef`]: crate::wad::map::Linedef
//! [`Map`]: crate::wad::map::Map
//! [`Thing`]: crate::wad::map::Thing

mod lines_from;
mod lines_to;
mod sectors;
mod table;
mod things;

#[cfg(test)]
mod tests;

pub use lines_from::linedefs_from_hexen;
pub use lines_to::linedefs_to_hexen;
pub use sectors::{sectors_from_hexen, sectors_to_hexen};
pub use table::{
    XlatFlags, XlatRow, parse_xlat_table, rows_for_doom, rows_for_zspecial, xlat_rows,
};
pub use things::{things_from_hexen, things_to_hexen};

use serde::Serialize;

use crate::wad::map::xlat::things::{
    THING_HEXEN_DORMANT, THING_HEXEN_INVISIBLE, THING_HEXEN_STANDING, THING_HEXEN_TRANSLUCENT,
};

/// What one cross-family field-translation stage changed and noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FieldReport {
    /// Elements whose translated fields differ from the originals.
    pub changed: usize,
    /// Lossy steps the caller should know about.
    pub warnings: Vec<String>,
}

/// One linedef whose special the translation stage could not carry
/// across the family boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Untranslated {
    /// The index of the line in [`Map::linedefs`](crate::wad::map::Map::linedefs).
    pub linedef: usize,
    /// The special the line carried when the stage gave up on it.
    pub special: i32,
}

/// What one cross-family linedef translation stage changed and noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LinesReport {
    /// Lines whose special moved through the translation table.
    pub translated: usize,
    /// Lines carrying a special this stage could not translate.
    pub untranslated: Vec<Untranslated>,
    /// Lossy steps the caller should know about.
    pub warnings: Vec<String>,
}

/// The two-bit field mask.
pub(super) const FIELD_TWO: u32 = 0b11;
/// The one-bit field mask.
pub(super) const FIELD_ONE: u32 = 1_u32;
/// The number of problem indices one grouped sector warning lists.
pub(super) const GROUP_LIST_LIMIT: usize = 20;

/// The field of `value` taken from bit `from`, widened by `mask`, and put
/// at bit `to`; every cross-family sector field move is one call.
pub(super) fn move_field(value: u32, from: u32, mask: u32, to: u32) -> u32 {
    let taken = value.checked_shr(from).unwrap_or(0_u32) & mask;
    taken.checked_shl(to).unwrap_or(0_u32)
}

/// `tally` plus one when `differs`.
#[inline]
pub(super) fn count_if(differs: bool, tally: usize) -> usize {
    tally.saturating_add(usize::from(differs))
}

/// The per-bit counts of the `Hexen` thing flags that have no `Doom`
/// equivalent, gathered while [`things_from_hexen`] rewrites a map.
#[derive(Clone, Copy, Default)]
pub(super) struct HexenDrops {
    /// Things that carried at least one of the bits.
    touched: usize,
    /// The count of `dormant` bits.
    dormant: usize,
    /// The count of `translucent` bits.
    translucent: usize,
    /// The count of `invisible` bits.
    invisible: usize,
    /// The count of `standing` bits.
    standing: usize,
}

impl HexenDrops {
    /// Counts the dropped bits `flags` carries.
    pub(super) fn note(&mut self, flags: u32) {
        let mut hit = false;
        hit |= count_bit(flags, THING_HEXEN_DORMANT, &mut self.dormant);
        hit |= count_bit(flags, THING_HEXEN_TRANSLUCENT, &mut self.translucent);
        hit |= count_bit(flags, THING_HEXEN_INVISIBLE, &mut self.invisible);
        hit |= count_bit(flags, THING_HEXEN_STANDING, &mut self.standing);
        self.touched = count_if(hit, self.touched);
    }

    /// The single grouped warning naming the totals, or `None` when every
    /// thing translated without a drop.
    pub(super) fn warning(&self) -> Option<String> {
        if self.touched == 0_usize {
            return None;
        }
        Some(format!(
            "{} things had Hexen-only flags with no Doom equivalent ({}): they were dropped",
            self.touched,
            self.parts()
        ))
    }

    /// The named per-bit counts, only the nonzero ones, comma-joined.
    fn parts(&self) -> String {
        let named = [
            ("dormant", self.dormant),
            ("translucent", self.translucent),
            ("invisible", self.invisible),
            ("standing", self.standing),
        ];
        named
            .iter()
            .filter(|entry| entry.1 > 0_usize)
            .map(|entry| format!("{} {}", entry.0, entry.1))
            .collect::<Vec<String>>()
            .join(", ")
    }
}

/// Adds one to `counter` when `flags` carries `bit`, answering whether it
/// did.
pub(super) const fn count_bit(flags: u32, bit: u32, counter: &mut usize) -> bool {
    if flags & bit == 0_u32 {
        return false;
    }
    *counter = counter.saturating_add(1_usize);
    true
}

/// The first [`GROUP_LIST_LIMIT`] indices comma-joined, the rest counted.
pub(super) fn show_indices(indices: &[usize]) -> String {
    let names: Vec<String> = indices
        .iter()
        .take(GROUP_LIST_LIMIT)
        .map(usize::to_string)
        .collect();
    if indices.len() <= GROUP_LIST_LIMIT {
        return names.join(", ");
    }
    let rest = indices.len().saturating_sub(GROUP_LIST_LIMIT);
    format!("{}, and {rest} more", names.join(", "))
}
