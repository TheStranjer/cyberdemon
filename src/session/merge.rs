//! The merged view: folding archives on top of each other the way a `Doom`
//! engine loads them.
//!
//! [`merge`] takes the directory built so far and overlays one archive onto
//! it. The rules are vanilla override semantics, last match wins:
//!
//! 1. a map block (header through its last member, plus its `GL` block)
//!    replaces the base block of the same name in place, or is appended when
//!    the base has no such map;
//! 2. a lump inside a namespace replaces the base lump of the same name in
//!    the same namespace in place; otherwise it joins that region just before
//!    the base's end marker, or, when the base has no such region at all,
//!    arrives as a new region: canonical start marker, lumps, canonical end
//!    marker, appended at the end;
//! 3. any other lump replaces the last base lump of the same name outside
//!    every region and map block, or is appended;
//! 4. overlay markers are never carried over on their own: they reappear only
//!    as the wrappers rule 2 creates.
//!
//! Replacing happens where the replaced lump stood, so an override never
//! shifts the rest of the directory. A start or end marker rule 2 has to
//! create is attributed to the region member it wraps, since it exists in no
//! archive of its own. The first archive onto an empty directory arrives
//! verbatim, markers and order included: there is nothing there to override.
//!
//! This deliberately does not model `ZDoom`-style concatenation, where every
//! `DECORATE`, `MAPINFO` or `ZSCRIPT` in the load order contributes its
//! definitions to one accumulated script. Those lumps follow rule 3: the last
//! archive's copy wins and the earlier ones disappear.
//!
//! The base directory with the edits an overlay wants to make to it stays
//! here beside the entry points; copying one overlay archive's map blocks,
//! regions and globals and the marker and lookup helpers that serve both
//! sides are the private `pass` module.

mod pass;

use crate::session::{MergedLump, Source};
use crate::wad::archive::Wad;
use crate::wad::classify::{Analysis, LumpInfo, LumpKind, Namespace, analyze};
use crate::wad::name::LumpName;

pub(in crate::session) use pass::analyze_directory;
pub use pass::tag;

use pass::{Pass, block_end, in_area, is_region_end, marker_lump, matches_lump};

/// The edits an overlay wants at each position of a base directory.
///
/// Everything is recorded against the positions the directory had when the
/// edits were planned, so indices stay valid while an overlay is applied.
pub(super) struct Edits {
    /// Entries replaced in place by an overlay lump.
    overrides: Vec<Option<MergedLump>>,
    /// Entries swallowed by a map block an overlay map replaced.
    skipped: Vec<bool>,
    /// Overlay entries to emit just before each position.
    pending: Vec<Vec<MergedLump>>,
    /// Maps whose base block an overlay map has already claimed.
    claimed: Vec<LumpName>,
}

impl Edits {
    /// Empty edits for a directory of `len` entries.
    fn with_len(len: usize) -> Self {
        Self {
            overrides: vec![None; len],
            skipped: vec![false; len],
            pending: vec![Vec::new(); len],
            claimed: Vec::new(),
        }
    }

    /// Marks every position of `range` as dropped, clipped to what exists.
    pub(super) fn skip(&mut self, range: (usize, usize)) {
        flag_range(&mut self.skipped, range);
    }

    /// Puts `lump` where entry `at` stood.
    pub(super) fn replace(&mut self, at: usize, lump: MergedLump) {
        if let Some(slot) = self.overrides.get_mut(at) {
            *slot = Some(lump);
        }
    }

    /// Queues `lumps` to emit just before position `at`.
    pub(super) fn insert_before(&mut self, at: usize, lumps: Vec<MergedLump>) {
        if let Some(slot) = self.pending.get_mut(at) {
            slot.extend(lumps);
        }
    }

    /// Claims map `name`'s base range, so a second overlay map of that name
    /// appends instead of replacing the same block again.
    pub(super) fn claim_map(
        &mut self,
        name: LumpName,
        analysis: &Analysis,
    ) -> Option<(usize, usize)> {
        if self.claimed.contains(&name) {
            return None;
        }
        let block = analysis.maps.iter().find(|map| map.name == name)?;
        self.claimed.push(name);
        Some((block.header, block_end(block)))
    }
}

/// The directory built so far, with the edits one overlay wants to make to it.
pub(super) struct Base {
    /// The directory entries, in order.
    lumps: Vec<MergedLump>,
    /// The classification of [`Base::lumps`].
    pub(super) analysis: Analysis,
    /// What an overlay asked for at each position.
    pub(super) edits: Edits,
    /// Overlay entries to emit after the whole directory.
    trailing: Vec<MergedLump>,
}

impl Base {
    /// Prepares `directory` to receive an overlay.
    fn new(directory: Vec<MergedLump>) -> Self {
        let len = directory.len();
        let analysis = analyze_directory(&directory);
        Self {
            lumps: directory,
            analysis,
            edits: Edits::with_len(len),
            trailing: Vec::new(),
        }
    }

    /// Emits `lumps` as a region of their own, wrapped in `namespace`'s
    /// markers. The markers are synthesized here, so they report the position
    /// of the first and last member they wrap.
    pub(super) fn append_region(&mut self, namespace: Namespace, mut lumps: Vec<MergedLump>) {
        let first = lumps.first().cloned();
        let last = lumps.last().cloned();
        let mut region = Vec::with_capacity(lumps.len().saturating_add(2));
        if let Some(entry) = first.as_ref() {
            region.push(marker_lump(namespace.start_marker(), entry));
        }
        region.append(&mut lumps);
        if let Some(entry) = last.as_ref() {
            region.push(marker_lump(namespace.end_marker(), entry));
        }
        self.append(region);
    }

    /// Emits `lumps` after the whole directory.
    pub(super) fn append(&mut self, lumps: Vec<MergedLump>) {
        self.trailing.extend(lumps);
    }

    /// Whether the base holds anything in `namespace`, markers included.
    pub(super) fn has_namespace(&self, namespace: Namespace) -> bool {
        self.analysis
            .lumps
            .iter()
            .any(|info| info.namespace == Some(namespace))
    }

    /// The position of the marker closing `namespace`.
    pub(super) fn end_marker(&self, namespace: Namespace) -> Option<usize> {
        self.analysis
            .lumps
            .iter()
            .rev()
            .find(|info| is_region_end(info, namespace))
            .map(|info| info.index)
    }

    /// The last base entry named `name` inside `namespace`; `None` namespace
    /// means outside every region and map block.
    pub(super) fn find_lump(&self, name: LumpName, namespace: Option<Namespace>) -> Option<usize> {
        self.analysis
            .lumps
            .iter()
            .rev()
            .find(|info| matches_lump(info, name, namespace))
            .map(|info| info.index)
    }

    /// Rebuilds the directory with every recorded edit applied.
    fn finish(self) -> Vec<MergedLump> {
        let mut out = rebuild(self.lumps, self.edits);
        out.extend(self.trailing);
        out
    }
}

/// Marks every position of `range` in `flags`, clipped to what exists.
fn flag_range(flags: &mut [bool], range: (usize, usize)) {
    for slot in range.0..range.1.min(flags.len()) {
        if let Some(flag) = flags.get_mut(slot) {
            *flag = true;
        }
    }
}

/// Replays `base` with `edits` applied, in one pass.
fn rebuild(base: Vec<MergedLump>, mut edits: Edits) -> Vec<MergedLump> {
    let mut out: Vec<MergedLump> = Vec::with_capacity(base.len());
    for (index, lump) in base.into_iter().enumerate() {
        if let Some(here) = edits.pending.get_mut(index) {
            out.append(here);
        }
        if kept(&edits, index) {
            out.push(replacement(&edits, index).unwrap_or(lump));
        }
    }
    out
}

/// Whether position `index` survives the edits, rather than being swallowed
/// by a map block replacement.
fn kept(edits: &Edits, index: usize) -> bool {
    !edits.skipped.get(index).copied().unwrap_or(false)
}

/// The lump `index` was replaced with, if any.
fn replacement(edits: &Edits, index: usize) -> Option<MergedLump> {
    edits.overrides.get(index).cloned().flatten()
}

/// One overlay map block: the map it defines and the ranges it covers.
pub(super) struct MapPlan {
    /// The map's name, from its header lump.
    pub(super) name: LumpName,
    /// The overlay positions of its lumps.
    pub(super) ranges: Vec<(usize, usize)>,
}

/// Folds `overlay` onto `directory` and returns the merged directory.
///
/// `source` says where the overlay's entries live, so every copied entry
/// keeps its origin. An empty directory has nothing to override, so the
/// overlay arrives verbatim, in its own order. See the module documentation
/// for the override rules.
#[must_use]
#[inline]
pub fn merge(directory: Vec<MergedLump>, overlay: &Wad, source: Source) -> Vec<MergedLump> {
    if directory.is_empty() {
        return tag(overlay, source);
    }
    let analysis = analyze(&overlay.lumps);
    let len = overlay.lumps.len();
    let mut pass = Pass {
        wad: overlay,
        analysis,
        source,
        consumed: vec![false; len],
    };
    let mut target = Base::new(directory);
    pass.copy_maps(&mut target);
    pass.copy_namespaces(&mut target);
    pass.copy_globals(&mut target);
    target.finish()
}

/// Folds `layers` onto an empty directory in load order, then `working` last,
/// so the working archive wins every name it shares with a layer.
#[must_use]
#[inline]
pub fn merge_all(layers: &[&Wad], working: &Wad) -> Vec<MergedLump> {
    let mut view: Vec<MergedLump> = Vec::new();
    for (index, wad) in layers.iter().enumerate() {
        view = merge(view, wad, Source::Layer(index));
    }
    merge(view, working, Source::Working)
}

/// Folds `overlay` into `base`, returning the result as a plain archive.
///
/// The same rules [`merge`] applies to the merged view, but both sides are
/// `Wad`s and the answer is one too. The header `kind` is the base's; the
/// overlay only contributes lumps. This is what merging a file into the
/// working archive does.
#[must_use]
#[inline]
pub fn merge_into(base: &Wad, overlay: &Wad) -> Wad {
    let directory = tag(base, Source::Working);
    let merged = merge(directory, overlay, Source::Layer(0));
    Wad {
        kind: base.kind,
        lumps: merged.into_iter().map(|entry| entry.lump).collect(),
    }
}

/// What folding one archive over another changes, counted per override unit:
/// a map block counts as one unit no matter how many lumps it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeCounts {
    /// Base units the overlay replaced in place.
    pub replaced: usize,
    /// Units the overlay brought that the base did not have.
    pub added: usize,
}

impl MergeCounts {
    /// Records one override (`replaced`) or arrival (`added`).
    #[inline]
    const fn bump(&mut self, overrode: bool) {
        if overrode {
            self.replaced = self.replaced.saturating_add(1);
        } else {
            self.added = self.added.saturating_add(1);
        }
    }
}

/// Predicts what [`merge_into`] will report, without building the archive:
/// each overlay map block, namespace lump and global lump either overrides
/// the base unit of its name and area or arrives new.
#[must_use]
#[inline]
pub fn merge_counts(base: &Wad, overlay: &Wad) -> MergeCounts {
    let base_analysis = analyze(&base.lumps);
    let overlay_analysis = analyze(&overlay.lumps);
    let mut counts = MergeCounts {
        replaced: 0,
        added: 0,
    };
    for block in &overlay_analysis.maps {
        let overrode = base_analysis
            .maps
            .iter()
            .any(|base_block| base_block.name == block.name);
        counts.bump(overrode);
    }
    for info in &overlay_analysis.lumps {
        if info.map.is_some() || info.kind == LumpKind::Marker {
            continue;
        }
        let overrode = base_analysis
            .lumps
            .iter()
            .any(|base_info| overrides(base_info, info));
        counts.bump(overrode);
    }
    counts
}

/// Whether the base entry `base` is the one the overlay lump `incoming`
/// replaces: same name, not a marker, in the same area.
fn overrides(base: &LumpInfo, incoming: &LumpInfo) -> bool {
    base.name == incoming.name
        && base.kind != LumpKind::Marker
        && base.map.is_none()
        && in_area(base, incoming.namespace)
}

#[cfg(test)]
mod tests;
