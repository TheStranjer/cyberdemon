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

use alloc::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::map::model::{Linedef, Map, Sidedef};

/// The `blocking` linedef flag bit, set when a line becomes one-sided.
const BLOCKING_FLAG: u32 = 1_u32;
/// The two-sided linedef flag bit, the same value in both flag families;
/// cleared when a line becomes one-sided.
const TWO_SIDED_FLAG: u32 = 4_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture field.
const NO_TEXTURE: &str = "-";

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

/// Cascade rule: every sidedef of a removed sector goes with it.
fn add_sides_of_removed_sectors(map: &Map, removal: &mut Removal) {
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
fn add_lines_of_removed_vertices(map: &Map, removal: &mut Removal) {
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
fn add_orphaned_sides(map: &Map, removal: &mut Removal, prune: bool) {
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
fn add_sideless_lines(map: &Map, removal: &mut Removal) {
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
fn add_orphaned_vertices(map: &Map, removal: &mut Removal, prune: bool) {
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

/// The old-to-new indices a surviving linedef's fields renumber through.
struct Numbering {
    /// Numbering of [`Map::vertices`]: `None` at removed and dropped slots.
    vertices: Vec<Option<usize>>,
    /// Numbering of [`Map::sidedefs`]: `None` at removed and dropped slots.
    sidedefs: Vec<Option<usize>>,
}

/// The renumbered linedef block and the side-normalisation records.
#[derive(Default)]
struct KeptLines {
    /// The surviving linedefs, in order, with remapped references.
    linedefs: Vec<Linedef>,
    /// New indices of the linedefs that were flipped.
    flipped: Vec<usize>,
    /// New indices of the linedefs that lost their back side.
    one_sided: Vec<usize>,
}

/// The new index of old index `old` among the elements outside `removal`.
fn renumber(count: usize, removal: &BTreeSet<usize>) -> Vec<Option<usize>> {
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
fn keep_sidedefs(
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
fn keep_linedefs(
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
        moved.flags = (moved.flags & !TWO_SIDED_FLAG) | BLOCKING_FLAG;
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
fn dropped<T: Clone>(items: &[T], removal: &BTreeSet<usize>) -> Vec<T> {
    items
        .iter()
        .enumerate()
        .filter(|&(old, _)| !removal.contains(&old))
        .map(|(_, item)| item.clone())
        .collect()
}

/// The removal set as its sorted list of original indices.
fn listed(removal: &BTreeSet<usize>) -> Vec<usize> {
    removal.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{CascadeOptions, Removal, apply_removal, dependents, expand_cascade};
    use crate::wad::map::common::fixtures::{solid_line, two_room_map};
    use crate::wad::map::{Linedef, Map};

    /// A removal of the one vertex `vertex` and nothing else.
    fn vertex_removal(vertex: usize) -> Removal {
        Removal {
            vertices: BTreeSet::from([vertex]),
            ..Default::default()
        }
    }

    /// A removal of the one linedef `linedef` and nothing else.
    fn linedef_removal(linedef: usize) -> Removal {
        Removal {
            linedefs: BTreeSet::from([linedef]),
            ..Default::default()
        }
    }

    /// A removal of the one sidedef `sidedef` and nothing else.
    fn sidedef_removal(sidedef: usize) -> Removal {
        Removal {
            sidedefs: BTreeSet::from([sidedef]),
            ..Default::default()
        }
    }

    /// A removal of the one sector `sector` and nothing else.
    fn sector_removal(sector: usize) -> Removal {
        Removal {
            sectors: BTreeSet::from([sector]),
            ..Default::default()
        }
    }

    /// The `(start_x, start_y, stop_x, stop_y)` of `line` in `map`.
    fn ends_of(map: &Map, line: &Linedef) -> (i32, i32, i32, i32) {
        let start = &map.vertices[line.v1];
        let stop = &map.vertices[line.v2];
        (start.x, start.y, stop.x, stop.y)
    }

    #[test]
    fn removing_a_thing_renumbers_the_things_only() {
        let fixture = two_room_map();
        let mut untouched = fixture.clone();
        let empty = apply_removal(&mut untouched, &Removal::default())
            .expect("an empty removal is a no-op");
        assert!(
            empty.things.is_empty() && untouched == fixture,
            "nothing is reported and nothing moves"
        );
        let mut map = fixture.clone();
        let report = apply_removal(
            &mut map,
            &Removal {
                things: BTreeSet::from([0]),
                ..Default::default()
            },
        )
        .expect("the player start goes");
        assert_eq!(report.things, vec![0], "the removed original index");
        assert!(
            report.vertices.is_empty()
                && report.linedefs.is_empty()
                && report.sidedefs.is_empty()
                && report.sectors.is_empty()
                && report.flipped_linedefs.is_empty()
                && report.one_sided_linedefs.is_empty(),
            "nothing else is reported: {report:?}"
        );
        assert_eq!(map.things.len(), 1, "the imp survives");
        assert_eq!(map.things[0].kind, 3_001_i32, "and renumbered to index 0");
        assert_eq!(map.vertices, fixture.vertices, "vertices untouched");
        assert_eq!(map.linedefs, fixture.linedefs, "linedefs untouched");
        assert_eq!(map.sidedefs, fixture.sidedefs, "sidedefs untouched");
        assert_eq!(map.sectors, fixture.sectors, "sectors untouched");
    }

    #[test]
    fn dependents_lists_every_reference_into_the_removal_set() {
        let map = two_room_map();
        let shared_line = dependents(&map, &vertex_removal(4));
        assert_eq!(
            shared_line.linedefs_using_vertices.get(&4),
            Some(&vec![1_usize, 2, 4]),
            "vertex 4 is used by the three lines meeting there"
        );
        assert!(
            shared_line.linedefs_using_sidedefs.is_empty()
                && shared_line.sidedefs_using_sectors.is_empty(),
            "only the vertex relation is populated"
        );
        assert!(!shared_line.is_clear(), "the removal has dependents");
        assert!(
            dependents(&map, &Removal::default()).is_clear(),
            "an empty removal depends on nothing"
        );
        let sides = dependents(
            &map,
            &Removal {
                sidedefs: BTreeSet::from([3]),
                sectors: BTreeSet::from([1]),
                ..Default::default()
            },
        );
        assert_eq!(
            sides.linedefs_using_sidedefs.get(&3),
            Some(&vec![2_usize]),
            "the shared line carries the removed back side"
        );
        assert_eq!(
            sides.sidedefs_using_sectors.get(&1),
            Some(&vec![3_usize, 5, 6, 7]),
            "four sides frame sector 1"
        );
        assert!(sides.linedefs_using_vertices.is_empty(), "no vertices");
    }

    #[test]
    fn dependents_lists_a_self_referencing_line_once() {
        let mut map = two_room_map();
        map.linedefs.push(Linedef {
            v1: 4,
            v2: 4,
            front: Some(2),
            back: Some(2),
            ..Default::default()
        });
        let found = dependents(
            &map,
            &Removal {
                vertices: BTreeSet::from([4]),
                sidedefs: BTreeSet::from([2]),
                ..Default::default()
            },
        );
        assert_eq!(
            found.linedefs_using_vertices.get(&4),
            Some(&vec![1_usize, 2, 4, 7]),
            "a zero-length line lists its shared endpoint once"
        );
        assert_eq!(
            found.linedefs_using_sidedefs.get(&2),
            Some(&vec![2_usize, 7]),
            "a line with the same sidedef on both sides lists itself once"
        );
    }

    #[test]
    fn cascading_a_vertex_removal_prunes_its_lines_and_their_sides() {
        let fixture = two_room_map();
        let removal = expand_cascade(&fixture, &vertex_removal(2), CascadeOptions::default());
        assert_eq!(
            removal.linedefs,
            BTreeSet::from([5_usize, 6]),
            "both back-room lines on vertex 2 go"
        );
        assert_eq!(
            removal.sidedefs,
            BTreeSet::from([6_usize, 7]),
            "the sides only those lines had are pruned"
        );
        assert_eq!(
            removal.vertices,
            BTreeSet::from([2_usize]),
            "only the seeded vertex goes: the hall's corners stay used"
        );
        assert!(
            removal.things.is_empty() && removal.sectors.is_empty(),
            "the cascade never adds things or sectors"
        );
        let mut map = fixture.clone();
        let report = apply_removal(&mut map, &removal).expect("the expanded removal applies");
        assert_eq!(report.linedefs, vec![5, 6], "sorted originals reported");
        assert_eq!(report.sidedefs, vec![6, 7], "sorted originals reported");
        assert_eq!(
            map.vertices.len(),
            5,
            "vertex 2 dropped, later ones shifted"
        );
        assert_eq!(map.linedefs.len(), 5, "five linedefs survive");
        assert_eq!(map.sidedefs.len(), 6, "six sidedefs survive");
        assert_eq!(
            map.sidedefs[2].sector, 0,
            "the step side still fronts sector 0"
        );
        for (index, line) in map.linedefs.iter().enumerate() {
            assert_eq!(
                ends_of(&map, line),
                ends_of(&fixture, &fixture.linedefs[index]),
                "surviving linedef {index} keeps its coordinates"
            );
        }
    }

    #[test]
    fn cascading_a_sector_removal_closes_the_shared_line() {
        let fixture = two_room_map();
        let removal = expand_cascade(&fixture, &sector_removal(1), CascadeOptions::default());
        assert_eq!(
            removal.sidedefs,
            BTreeSet::from([3_usize, 5, 6, 7]),
            "every side of sector 1 goes"
        );
        assert_eq!(
            removal.linedefs,
            BTreeSet::from([4_usize, 5, 6]),
            "the lines that only those sides held go"
        );
        assert_eq!(
            removal.vertices,
            BTreeSet::from([2_usize, 3]),
            "the back-room corners go orphaned"
        );
        let mut map = fixture;
        let report = apply_removal(&mut map, &removal).expect("the expanded removal applies");
        assert_eq!(report.sectors, vec![1], "sorted originals reported");
        assert_eq!(report.vertices, vec![2, 3], "sorted originals reported");
        assert_eq!(report.one_sided_linedefs, vec![2], "the shared line closed");
        assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
        let shared = &map.linedefs[2];
        assert_eq!(shared.flags, 1, "two-sided bit cleared, blocking set");
        assert_eq!((shared.v1, shared.v2), (2, 1), "renumbered ends");
        assert_eq!(
            ends_of(&map, shared),
            (256_i32, 256_i32, 256_i32, 0_i32),
            "and same coordinates"
        );
        assert!(shared.back.is_none(), "the sector 1 side is gone");
        let front = &map.sidedefs[shared.front.expect("a front side")];
        assert_eq!(
            (front.middle.as_str(), front.lower.as_str()),
            ("STEP1", "-"),
            "the step texture moved to the middle so the wall shows"
        );
        assert_eq!(map.sidedefs.len(), 4, "only the hall's sides survive");
        assert_eq!(map.sectors.len(), 1, "the hall survives");
        assert_eq!(front.sector, 0, "and the step side still fronts it");
    }

    #[test]
    fn cascading_a_front_side_removal_flips_the_line() {
        let fixture = two_room_map();
        let removal = expand_cascade(&fixture, &sidedef_removal(2), CascadeOptions::default());
        assert_eq!(
            removal,
            sidedef_removal(2),
            "the line keeps its back side, so nothing cascades"
        );
        let mut map = fixture;
        let report = apply_removal(&mut map, &removal).expect("the removal applies");
        assert_eq!(report.flipped_linedefs, vec![2], "the shared line flipped");
        assert_eq!(
            report.one_sided_linedefs,
            vec![2],
            "and it lost its back with the flip"
        );
        assert_eq!(report.sidedefs, vec![2], "the removed original index");
        let shared = &map.linedefs[2];
        assert_eq!((shared.v1, shared.v2), (1, 4), "the ends swapped");
        assert_eq!(shared.front, Some(2), "sector 1's side became the front");
        assert!(shared.back.is_none(), "there is no back any more");
        assert_eq!(shared.flags, 1, "two-sided bit cleared, blocking set");
        let front = &map.sidedefs[2];
        assert_eq!(front.sector, 1, "the raised room's side faces out now");
    }

    #[test]
    fn cascade_drops_a_line_that_lost_both_sides() {
        let fixture = two_room_map();
        let removal = expand_cascade(
            &fixture,
            &Removal {
                sidedefs: BTreeSet::from([2_usize, 3]),
                ..Default::default()
            },
            CascadeOptions::default(),
        );
        assert_eq!(
            removal.linedefs,
            BTreeSet::from([2_usize]),
            "a line with no side left dies with them"
        );
        assert_eq!(
            removal.sidedefs,
            BTreeSet::from([2_usize, 3]),
            "the seed stays"
        );
        assert!(
            removal.vertices.is_empty(),
            "both ends are still used by surviving lines"
        );
    }

    #[test]
    fn applying_a_side_only_removal_leaves_an_unsided_line() {
        let fixture = two_room_map();
        let mut map = fixture;
        let report = apply_removal(
            &mut map,
            &Removal {
                sidedefs: BTreeSet::from([2_usize, 3]),
                ..Default::default()
            },
        )
        .expect("sidedef references drop to None instead of failing");
        assert_eq!(
            report.one_sided_linedefs,
            vec![2],
            "the line lost its back and was normalised"
        );
        assert!(
            report.flipped_linedefs.is_empty(),
            "there was nothing to flip"
        );
        let shared = &map.linedefs[2];
        assert!(
            shared.front.is_none() && shared.back.is_none(),
            "both sides dropped"
        );
        assert_eq!(shared.flags, 1, "one-sided and blocking");
    }

    #[test]
    fn cascade_options_keep_orphans_when_pruning_is_off() {
        let defaults = CascadeOptions::default();
        assert!(
            defaults.prune_sidedefs && defaults.prune_vertices,
            "pruning is on by default"
        );
        let fixture = two_room_map();
        let shared_sides = expand_cascade(
            &fixture,
            &linedef_removal(2),
            CascadeOptions {
                prune_sidedefs: false,
                ..defaults
            },
        );
        assert_eq!(
            shared_sides,
            linedef_removal(2),
            "the shared line's sides stay orphaned"
        );
        let unpruned = expand_cascade(
            &fixture,
            &Removal {
                linedefs: BTreeSet::from([5_usize, 6]),
                ..Default::default()
            },
            CascadeOptions {
                prune_vertices: false,
                ..defaults
            },
        );
        assert_eq!(
            unpruned.sidedefs,
            BTreeSet::from([6_usize, 7]),
            "side pruning still runs"
        );
        assert!(unpruned.vertices.is_empty(), "the orphaned vertex stays");
        let mut map = fixture;
        apply_removal(&mut map, &unpruned).expect("the unpruned removal still applies");
        assert_eq!(map.vertices.len(), 6, "vertex 2 survives unreferenced");
        let used: BTreeSet<usize> = map
            .linedefs
            .iter()
            .flat_map(|line| [line.v1, line.v2])
            .collect();
        assert!(!used.contains(&2), "no surviving line uses vertex 2");
    }

    #[test]
    fn cascade_prunes_only_sides_and_vertices_no_survivor_needs() {
        let mut fixture = two_room_map();
        fixture.linedefs.push(solid_line(0, 5, 0));
        let removal = expand_cascade(&fixture, &linedef_removal(0), CascadeOptions::default());
        assert_eq!(
            removal,
            linedef_removal(0),
            "the twin keeps side 0 and both endpoints in use"
        );
    }

    #[test]
    fn one_sided_normalisation_moves_only_an_invisible_texture() {
        let fixture = two_room_map();
        let removal = expand_cascade(&fixture, &sector_removal(1), CascadeOptions::default());
        let mut upper_only = fixture.clone();
        let side = &mut upper_only.sidedefs[2];
        side.upper = "TOP1".to_owned();
        side.lower = "-".to_owned();
        apply_removal(&mut upper_only, &removal).expect("the expanded removal applies");
        assert_eq!(
            (
                upper_only.sidedefs[2].middle.as_str(),
                upper_only.sidedefs[2].upper.as_str()
            ),
            ("TOP1", "-"),
            "a lone upper texture moves to the middle"
        );
        let mut middle_set = fixture;
        middle_set.sidedefs[2].middle = "DOOR1".to_owned();
        apply_removal(&mut middle_set, &removal).expect("the expanded removal applies");
        assert_eq!(
            (
                middle_set.sidedefs[2].middle.as_str(),
                middle_set.sidedefs[2].lower.as_str()
            ),
            ("DOOR1", "STEP1"),
            "a side that already shows a middle keeps its layout"
        );
    }

    #[test]
    fn applying_a_removal_with_live_references_fails_untouched() {
        let fixture = two_room_map();
        let mut map = fixture.clone();
        let vertex_error = apply_removal(&mut map, &vertex_removal(4))
            .expect_err("surviving lines still reference vertex 4");
        assert_eq!(
            vertex_error.code(),
            "internal",
            "the dangling line is caught"
        );
        assert!(
            vertex_error.to_string().contains("vertex 4"),
            "the message names the target: {vertex_error}"
        );
        let sector_error = apply_removal(&mut map, &sector_removal(1))
            .expect_err("surviving sides still reference sector 1");
        assert_eq!(
            sector_error.code(),
            "internal",
            "the dangling side is caught"
        );
        assert_eq!(map, fixture, "the map is untouched after refusals");
    }

    #[test]
    fn applying_a_removal_over_an_out_of_range_reference_fails() {
        let mut map = two_room_map();
        map.linedefs[0].v2 = 99;
        let mut scratch = map.clone();
        let error = apply_removal(&mut scratch, &Removal::default())
            .expect_err("the pre-broken reference is caught");
        assert_eq!(
            error.code(),
            "internal",
            "a missing target is an internal error"
        );
        assert_eq!(scratch, map, "the map is untouched");
    }

    #[test]
    fn removing_an_index_the_map_does_not_have_reports_not_found() {
        let fixture = two_room_map();
        let cases = [
            ("vertex", vertex_removal(6)),
            ("linedef", linedef_removal(7)),
            ("sidedef", sidedef_removal(8)),
            ("sector", sector_removal(2)),
            (
                "thing",
                Removal {
                    things: BTreeSet::from([2]),
                    ..Default::default()
                },
            ),
        ];
        let mut map = fixture.clone();
        for (kind, removal) in cases {
            let error = apply_removal(&mut map, &removal)
                .expect_err("the index is past the end of the block");
            assert_eq!(error.code(), "not_found", "{kind} index is missing");
            assert!(
                error.to_string().contains(kind),
                "the message names {kind}: {error}"
            );
        }
        assert_eq!(map, fixture, "the map is untouched");
    }
}
