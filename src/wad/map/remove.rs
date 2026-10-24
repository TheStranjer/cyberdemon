//! The pure element-removal engine shared by the map edit tools.
//!
//! Deleting map elements shifts every later index and leaves the references
//! that pointed into the removed range dangling, so it happens in exactly
//! one place. A [`Removal`] collects the doomed original indices per kind;
//! [`expand_cascade`] grows such a set until it is self-consistent (a
//! removed sector takes every side of it, a removed vertex takes every line
//! using it, a line whose sides all go dies with it, and — under
//! [`CascadeOptions`] — sides and vertices orphaned by those removals
//! follow); [`apply_removal`] drops the elements, renumbers every surviving
//! reference and normalises the lines that end up one-sided. [`dependents`]
//! reports what still references a removal set, so a tool can answer a
//! non-cascading removal with a `conflict` error instead of tripping
//! [`apply_removal`]'s `internal` guard.
//!
//! The engine never touches the generated lumps: [`Map::other`]
//! (`SEGS`/`SSECTORS`/`NODES`, `REJECT`, `BLOCKMAP`) is carried verbatim
//! and callers rebuild it after editing.
//!
//! The rules that grow a removal live in the private `cascade` module and
//! the renumbering and side normalisation of what survives, with the
//! `Numbering` and `KeptLines` bookkeeping [`apply_removal`] reads, in the
//! private `renumber` module; both are implementation details of the three
//! entry points below.

mod cascade;
mod renumber;

use alloc::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::map::model::Map;
use cascade::{
    add_lines_of_removed_vertices, add_orphaned_sides, add_orphaned_vertices, add_sideless_lines,
    add_sides_of_removed_sectors,
};
use renumber::{Numbering, dropped, keep_linedefs, keep_sidedefs, listed, renumber};

/// The `blocking` linedef flag bit, set when a line becomes one-sided.
pub(super) const BLOCKING_FLAG: u32 = 1_u32;
/// The two-sided linedef flag bit, the same value in both flag families;
/// cleared when a line becomes one-sided.
pub(super) const TWO_SIDED_FLAG: u32 = 4_u32;
/// The model's name for "no texture here" on a
/// [`Sidedef`](super::Sidedef) texture field.
pub(super) const NO_TEXTURE: &str = "-";

/// Map elements selected for removal, by original index.
///
/// The seed and result of [`expand_cascade`] and the input of
/// [`apply_removal`]. Each index is a position in the matching [`Map`]
/// block as it stands before anything is removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// Indices into [`Map::vertices`].
    pub vertices: BTreeSet<usize>,
    /// Indices into [`Map::linedefs`].
    pub linedefs: BTreeSet<usize>,
    /// Indices into [`Map::sidedefs`].
    pub sidedefs: BTreeSet<usize>,
    /// Indices into [`Map::sectors`].
    pub sectors: BTreeSet<usize>,
    /// Indices into [`Map::things`].
    pub things: BTreeSet<usize>,
}

/// How far [`expand_cascade`] prunes the orphans its own removals create.
///
/// [`Default`] turns both prunings on: a cascade leaves no orphans behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CascadeOptions {
    /// Remove sidedefs that no surviving linedef references any more.
    pub prune_sidedefs: bool,
    /// Remove vertices that no surviving linedef references any more.
    pub prune_vertices: bool,
}

impl Default for CascadeOptions {
    /// The cascading default: both prunings on.
    #[inline]
    fn default() -> Self {
        Self {
            prune_sidedefs: true,
            prune_vertices: true,
        }
    }
}

/// What the elements of a [`Removal`] are still referenced by.
///
/// Each map is keyed by the removed element's original index and lists the
/// referencing elements' original indices, in ascending order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Dependents {
    /// Removed `vertex` index to the linedefs using it as an endpoint.
    pub linedefs_using_vertices: BTreeMap<usize, Vec<usize>>,
    /// Removed `sidedef` index to the linedefs carrying it as a side.
    pub linedefs_using_sidedefs: BTreeMap<usize, Vec<usize>>,
    /// Removed `sector` index to the sidedefs whose sector it is.
    pub sidedefs_using_sectors: BTreeMap<usize, Vec<usize>>,
}

impl Dependents {
    /// Whether nothing outside the removal set references the removal set.
    #[must_use]
    #[inline]
    pub fn is_clear(&self) -> bool {
        self.linedefs_using_vertices.is_empty()
            && self.linedefs_using_sidedefs.is_empty()
            && self.sidedefs_using_sectors.is_empty()
    }
}

/// What [`apply_removal`] did, for the tool to report back.
#[derive(Serialize, Default, Debug)]
pub struct RemovalReport {
    /// Removed vertex indices, original numbering, sorted.
    pub vertices: Vec<usize>,
    /// Removed linedef indices, original numbering, sorted.
    pub linedefs: Vec<usize>,
    /// Removed sidedef indices, original numbering, sorted.
    pub sidedefs: Vec<usize>,
    /// Removed sector indices, original numbering, sorted.
    pub sectors: Vec<usize>,
    /// Removed thing indices, original numbering, sorted.
    pub things: Vec<usize>,
    /// New indices of linedefs flipped because their front side went.
    pub flipped_linedefs: Vec<usize>,
    /// New indices of linedefs that lost their back side.
    pub one_sided_linedefs: Vec<usize>,
}

/// The elements that reference the entries of `removal` from outside it.
///
/// Tools use this to refuse a non-cascading removal with a `conflict`
/// error naming the dependents before applying anything.
#[must_use]
#[inline]
pub fn dependents(map: &Map, removal: &Removal) -> Dependents {
    Dependents {
        linedefs_using_vertices: vertex_uses(map, removal),
        linedefs_using_sidedefs: sidedef_uses(map, removal),
        sidedefs_using_sectors: sector_uses(map, removal),
    }
}

/// Grow `removal` until it is self-consistent under `opts`.
///
/// Repeated until nothing changes: every sidedef of a removed sector goes;
/// every linedef using a removed vertex goes; with
/// [`CascadeOptions::prune_sidedefs`] every sidedef no surviving linedef
/// references goes; a linedef whose remaining sides are all gone goes; and
/// with [`CascadeOptions::prune_vertices`] every vertex that was an
/// endpoint of a removed linedef and is used by no surviving linedef goes.
/// Things are never added by the cascade. Out-of-range seed indices pass
/// through untouched for [`apply_removal`] to report.
#[must_use]
#[inline]
pub fn expand_cascade(map: &Map, removal: &Removal, opts: CascadeOptions) -> Removal {
    let mut grown = removal.clone();
    loop {
        let before = grown.clone();
        add_sides_of_removed_sectors(map, &mut grown);
        add_lines_of_removed_vertices(map, &mut grown);
        add_orphaned_sides(map, &mut grown, opts.prune_sidedefs);
        add_sideless_lines(map, &mut grown);
        add_orphaned_vertices(map, &mut grown, opts.prune_vertices);
        if grown == before {
            break;
        }
    }
    grown
}

/// Drop the elements of `removal` from `map`, renumber every surviving
/// reference and normalise the linedefs left with one side.
///
/// A linedef referencing a removed sidedef loses that side; a linedef whose
/// front went but whose back survived is flipped (`v1`/`v2` swap, back
/// becomes front) and reported in [`RemovalReport::flipped_linedefs`]; a
/// linedef that ends without a back gets the two-sided bit cleared, the
/// blocking bit set and its remaining side's lower or upper texture moved
/// to the middle when the middle is empty, reported in
/// [`RemovalReport::one_sided_linedefs`].
///
/// # Errors
///
/// [`CyberError::NotFound`] when `removal` names an index the map does not
/// have. [`CyberError::Internal`] when a surviving linedef still references
/// a removed vertex or a surviving sidedef a removed sector: run
/// [`expand_cascade`] (or check [`dependents`]) and apply the expanded set.
///
/// On error `map` is left untouched.
#[inline]
pub fn apply_removal(map: &mut Map, removal: &Removal) -> Result<RemovalReport, CyberError> {
    check_present(map, removal)?;
    let sector_map = renumber(map.sectors.len(), &removal.sectors);
    let mut new_sides = keep_sidedefs(map, removal, &sector_map)?;
    let numbering = Numbering {
        vertices: renumber(map.vertices.len(), &removal.vertices),
        sidedefs: renumber(map.sidedefs.len(), &removal.sidedefs),
    };
    let kept = keep_linedefs(map, removal, &numbering, &mut new_sides)?;
    let report = RemovalReport {
        vertices: listed(&removal.vertices),
        linedefs: listed(&removal.linedefs),
        sidedefs: listed(&removal.sidedefs),
        sectors: listed(&removal.sectors),
        things: listed(&removal.things),
        flipped_linedefs: kept.flipped,
        one_sided_linedefs: kept.one_sided,
    };
    map.things = dropped(&map.things, &removal.things);
    map.vertices = dropped(&map.vertices, &removal.vertices);
    map.sidedefs = new_sides;
    map.linedefs = kept.linedefs;
    map.sectors = dropped(&map.sectors, &removal.sectors);
    Ok(report)
}

/// The `NotFound` when any removal set names an index at or past its block.
fn check_present(map: &Map, removal: &Removal) -> Result<(), CyberError> {
    check_highest("vertex", &removal.vertices, map.vertices.len())?;
    check_highest("linedef", &removal.linedefs, map.linedefs.len())?;
    check_highest("sidedef", &removal.sidedefs, map.sidedefs.len())?;
    check_highest("sector", &removal.sectors, map.sectors.len())?;
    check_highest("thing", &removal.things, map.things.len())
}

/// The `NotFound` when `removal`'s highest `kind` index is past `count`.
fn check_highest(kind: &str, removal: &BTreeSet<usize>, count: usize) -> Result<(), CyberError> {
    removal
        .iter()
        .next_back()
        .filter(|highest| **highest >= count)
        .map_or(Ok(()), |highest| {
            Err(CyberError::NotFound(format!(
                "cannot remove {kind} {highest}: the map holds {count} of them"
            )))
        })
}

/// The linedefs using each removed vertex, keyed by vertex index.
fn vertex_uses(map: &Map, removal: &Removal) -> BTreeMap<usize, Vec<usize>> {
    let mut uses: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        let doomed_ends = BTreeSet::from([line.v1, line.v2])
            .into_iter()
            .filter(|end| removal.vertices.contains(end));
        for end in doomed_ends {
            uses.entry(end).or_default().push(index);
        }
    }
    uses
}

/// The linedefs carrying each removed sidedef as a side, keyed by sidedef.
fn sidedef_uses(map: &Map, removal: &Removal) -> BTreeMap<usize, Vec<usize>> {
    let mut uses: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, line) in map.linedefs.iter().enumerate() {
        let doomed_sides = BTreeSet::from([line.front, line.back])
            .into_iter()
            .flatten()
            .filter(|side| removal.sidedefs.contains(side));
        for side in doomed_sides {
            uses.entry(side).or_default().push(index);
        }
    }
    uses
}

/// The sidedefs over each removed sector, keyed by sector index.
fn sector_uses(map: &Map, removal: &Removal) -> BTreeMap<usize, Vec<usize>> {
    let mut uses: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, side) in map.sidedefs.iter().enumerate() {
        if removal.sectors.contains(&side.sector) {
            uses.entry(side.sector).or_default().push(index);
        }
    }
    uses
}

#[cfg(test)]
mod tests;
