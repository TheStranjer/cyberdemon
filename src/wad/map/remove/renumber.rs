//! The renumbering pass [`apply_removal`](super::apply_removal) runs once a
//! [`Removal`] has been decided: the new index of every surviving element,
//! the surviving sidedefs and linedefs with their references remapped
//! through [`Numbering`], and the flip, flagging and texture reveal that
//! normalise a linedef left with one side.
//!
//! [`KeptLines`] carries the renumbered linedef block beside the two lists
//! the tool reports. A surviving reference that names a removed element, or
//! one the map never held, is refused through [`refusal`]: the caller has
//! applied an unexpanded set, which is an internal error, not user input.

use alloc::collections::BTreeSet;

use super::{NO_TEXTURE, Removal};
use crate::error::CyberError;
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::model::{Linedef, Map, Sidedef};

/// The old-to-new indices a surviving linedef's fields renumber through.
pub(super) struct Numbering {
    /// Numbering of [`Map::vertices`]: `None` at removed and dropped slots.
    pub(super) vertices: Vec<Option<usize>>,
    /// Numbering of [`Map::sidedefs`]: `None` at removed and dropped slots.
    pub(super) sidedefs: Vec<Option<usize>>,
}

/// The renumbered linedef block and the side-normalisation records.
#[derive(Default)]
pub(super) struct KeptLines {
    /// The surviving linedefs, in order, with remapped references.
    pub(super) linedefs: Vec<Linedef>,
    /// New indices of the linedefs that were flipped.
    pub(super) flipped: Vec<usize>,
    /// New indices of the linedefs that lost their back side.
    pub(super) one_sided: Vec<usize>,
}

/// The new index of old index `old` among the elements outside `removal`.
pub(super) fn renumber(count: usize, removal: &BTreeSet<usize>) -> Vec<Option<usize>> {
    let mut next = 0_usize;
    (0..count)
        .map(|old| {
            if removal.contains(&old) {
                None
            } else {
                let current = Some(next);
                next = next.saturating_add(1);
                current
            }
        })
        .collect()
}

/// The new index `target` renumbers to, or `None` when it was removed.
fn live(numbering: &[Option<usize>], target: usize) -> Option<usize> {
    numbering.get(target).copied().flatten()
}

/// The surviving sidedefs with their sector references renumbered.
pub(super) fn keep_sidedefs(
    map: &Map,
    removal: &Removal,
    sector_map: &[Option<usize>],
) -> Result<Vec<Sidedef>, CyberError> {
    let mut kept = Vec::new();
    for (old, side) in map.sidedefs.iter().enumerate() {
        if removal.sidedefs.contains(&old) {
            continue;
        }
        let mut moved = side.clone();
        moved.sector = live(sector_map, side.sector)
            .ok_or_else(|| refusal("sector", side.sector, "sidedef", old))?;
        kept.push(moved);
    }
    Ok(kept)
}

/// The surviving linedefs with renumbered references and one-sided lines
/// flipped and flagged; `new_sides` is indexed by the new sidedef numbers.
pub(super) fn keep_linedefs(
    map: &Map,
    removal: &Removal,
    numbering: &Numbering,
    new_sides: &mut [Sidedef],
) -> Result<KeptLines, CyberError> {
    let mut kept = KeptLines::default();
    for (old, line) in map.linedefs.iter().enumerate() {
        if removal.linedefs.contains(&old) {
            continue;
        }
        let had_back = line.back.is_some();
        let (v1, v2) = remap_ends(line, old, &numbering.vertices)?;
        let mut moved = line.clone();
        moved.v1 = v1;
        moved.v2 = v2;
        moved.front = line.front.and_then(|side| live(&numbering.sidedefs, side));
        moved.back = line.back.and_then(|side| live(&numbering.sidedefs, side));
        normalise_side(&mut kept, new_sides, &mut moved, had_back);
        kept.linedefs.push(moved);
    }
    Ok(kept)
}

/// The renumbered `v1`/`v2` of surviving linedef `holder`.
fn remap_ends(
    line: &Linedef,
    holder: usize,
    numbering: &[Option<usize>],
) -> Result<(usize, usize), CyberError> {
    Ok((
        live(numbering, line.v1).ok_or_else(|| refusal("vertex", line.v1, "linedef", holder))?,
        live(numbering, line.v2).ok_or_else(|| refusal("vertex", line.v2, "linedef", holder))?,
    ))
}

/// Flip a side-less-front line and flag a line whose back side went.
fn normalise_side(
    kept: &mut KeptLines,
    new_sides: &mut [Sidedef],
    moved: &mut Linedef,
    had_back: bool,
) {
    let new_index = kept.linedefs.len();
    if moved.front.is_none() && moved.back.is_some() {
        core::mem::swap(&mut moved.v1, &mut moved.v2);
        moved.front = moved.back.take();
        kept.flipped.push(new_index);
    }
    if had_back && moved.back.is_none() {
        moved.flags = (moved.flags & !TWO_SIDED) | BLOCKING;
        kept.one_sided.push(new_index);
        reveal_front_texture(new_sides, moved.front);
    }
}

/// Make the new one-sided wall visible from the front side, if any.
fn reveal_front_texture(new_sides: &mut [Sidedef], front: Option<usize>) {
    if let Some(side) = front.and_then(|index| new_sides.get_mut(index)) {
        move_texture_to_middle(side);
    }
}

/// Move a lone lower (preferred) or upper texture of a now one-sided
/// front side to the middle, where a wall without a back side renders it.
fn move_texture_to_middle(side: &mut Sidedef) {
    if side.middle != NO_TEXTURE {
        return;
    }
    if side.lower != NO_TEXTURE {
        side.middle.clone_from(&side.lower);
        NO_TEXTURE.clone_into(&mut side.lower);
        return;
    }
    if side.upper != NO_TEXTURE {
        side.middle.clone_from(&side.upper);
        NO_TEXTURE.clone_into(&mut side.upper);
    }
}

/// The `Internal` saying surviving `holder_kind` `holder` references
/// `target_kind` `target`, which the removal deletes or the map does not
/// hold.
fn refusal(target_kind: &str, target: usize, holder_kind: &str, holder: usize) -> CyberError {
    CyberError::Internal(format!(
        "cannot apply the removal: {holder_kind} {holder} references {target_kind} {target}, \
         which the removal deletes or the map does not hold: apply the \
         expand_cascade result instead"
    ))
}

/// The elements outside `removal`, in original order.
pub(super) fn dropped<T: Clone>(items: &[T], removal: &BTreeSet<usize>) -> Vec<T> {
    items
        .iter()
        .enumerate()
        .filter(|&(old, _)| !removal.contains(&old))
        .map(|(_, item)| item.clone())
        .collect()
}

/// The removal set as its sorted list of original indices.
pub(super) fn listed(removal: &BTreeSet<usize>) -> Vec<usize> {
    removal.iter().copied().collect()
}
