//! The cascade rules that grow a [`Removal`] until it is self-consistent.
//!
//! Each rule adds the elements that cannot survive what is already doomed:
//! a removed sector takes every sidedef of it, a removed vertex takes every
//! linedef ending at it, and a linedef whose remaining sides have all gone
//! goes with them; under [`CascadeOptions`](super::CascadeOptions) the sides
//! and vertices those removals leave orphaned follow. Nothing here ever adds
//! things or sectors. [`expand_cascade`](super::expand_cascade) in the parent
//! repeats these rules until a round changes nothing.

use alloc::collections::BTreeSet;

use super::Removal;
use crate::wad::map::model::{Linedef, Map};

/// Cascade rule: every sidedef of a removed sector goes with it.
pub(super) fn add_sides_of_removed_sectors(map: &Map, removal: &mut Removal) {
    if removal.sectors.is_empty() {
        return;
    }
    let doomed: Vec<usize> = map
        .sidedefs
        .iter()
        .enumerate()
        .filter(|&(_, side)| removal.sectors.contains(&side.sector))
        .map(|(index, _)| index)
        .collect();
    removal.sidedefs.extend(doomed);
}

/// Cascade rule: every linedef using a removed vertex goes with it.
pub(super) fn add_lines_of_removed_vertices(map: &Map, removal: &mut Removal) {
    if removal.vertices.is_empty() {
        return;
    }
    let doomed: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| uses_removed_vertex(removal, line))
        .map(|(index, _)| index)
        .collect();
    removal.linedefs.extend(doomed);
}

/// Whether either endpoint of `line` is in the removed vertex set.
fn uses_removed_vertex(removal: &Removal, line: &Linedef) -> bool {
    removal.vertices.contains(&line.v1) || removal.vertices.contains(&line.v2)
}

/// Cascade rule: with `prune`, sidedefs of removed linedefs that no
/// surviving linedef references go too.
pub(super) fn add_orphaned_sides(map: &Map, removal: &mut Removal, prune: bool) {
    if !prune || removal.linedefs.is_empty() {
        return;
    }
    let mut candidates = BTreeSet::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        if removal.linedefs.contains(&index) {
            candidates.extend([line.front, line.back].into_iter().flatten());
        }
    }
    let doomed: Vec<usize> = candidates
        .into_iter()
        .filter(|side| !removal.sidedefs.contains(side))
        .filter(|side| !survivor_uses_side(map, removal, *side))
        .collect();
    removal.sidedefs.extend(doomed);
}

/// Cascade rule: a linedef whose front is gone and whose back is gone or
/// absent loses its last reason to exist and goes.
pub(super) fn add_sideless_lines(map: &Map, removal: &mut Removal) {
    if removal.sidedefs.is_empty() {
        return;
    }
    let doomed: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(index, line)| {
            !removal.linedefs.contains(&index) && loses_every_side(line, &removal.sidedefs)
        })
        .map(|(index, _)| index)
        .collect();
    removal.linedefs.extend(doomed);
}

/// Whether `line` keeps no side once the removed sidedefs are gone.
fn loses_every_side(line: &Linedef, removed: &BTreeSet<usize>) -> bool {
    line.front.is_some_and(|side| removed.contains(&side))
        && line.back.is_none_or(|side| removed.contains(&side))
}

/// Cascade rule: with `prune`, vertices that were endpoints of removed
/// linedefs and are used by no surviving linedef go too.
pub(super) fn add_orphaned_vertices(map: &Map, removal: &mut Removal, prune: bool) {
    if !prune || removal.linedefs.is_empty() {
        return;
    }
    let mut candidates = BTreeSet::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        if removal.linedefs.contains(&index) {
            candidates.insert(line.v1);
            candidates.insert(line.v2);
        }
    }
    let doomed: Vec<usize> = candidates
        .into_iter()
        .filter(|vertex| map.vertices.get(*vertex).is_some())
        .filter(|vertex| !survivor_uses_vertex(map, removal, *vertex))
        .collect();
    removal.vertices.extend(doomed);
}

/// Whether any linedef outside the removal set references `side`.
fn survivor_uses_side(map: &Map, removal: &Removal, side: usize) -> bool {
    map.linedefs.iter().enumerate().any(|(index, line)| {
        !removal.linedefs.contains(&index) && (line.front == Some(side) || line.back == Some(side))
    })
}

/// Whether any linedef outside the removal set has `vertex` as an endpoint.
fn survivor_uses_vertex(map: &Map, removal: &Removal, vertex: usize) -> bool {
    map.linedefs.iter().enumerate().any(|(index, line)| {
        !removal.linedefs.contains(&index) && (line.v1 == vertex || line.v2 == vertex)
    })
}
