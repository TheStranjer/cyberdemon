//! The fold of one overlay archive: the `Pass` that copies an overlay's
//! map blocks, namespace regions and global lumps onto the base
//! directory, plus the helpers it and the entry points share: the block
//! ends and ranges, the marker and lookup tests, the synthesized region
//! marker, the whole-directory classifier and the whole-archive tagger.

use super::{Base, MapPlan};
use crate::session::{MergedLump, Source};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, LumpInfo, LumpKind, MapBlock, Namespace, analyze};
use crate::wad::name::{LumpName, NAME_LEN};

/// One archive being folded onto a directory, with its classification.
pub(super) struct Pass<'overlay> {
    /// The archive's entries.
    pub(super) wad: &'overlay Wad,
    /// Their classification.
    pub(super) analysis: Analysis,
    /// Where the entries this pass copies came from.
    pub(super) source: Source,
    /// Entries already copied as part of a map block.
    pub(super) consumed: Vec<bool>,
}

impl Pass<'_> {
    /// The entry at `index` tagged with this pass's source, marked consumed.
    /// `None` for a position outside the overlay.
    fn take(&mut self, index: usize) -> Option<MergedLump> {
        if let Some(flag) = self.consumed.get_mut(index) {
            *flag = true;
        }
        self.wad.lumps.get(index).cloned().map(|lump| MergedLump {
            lump,
            source: self.source,
            source_index: index,
        })
    }

    /// Whether `index` was already copied as part of a map block.
    fn is_consumed(&self, index: usize) -> bool {
        self.consumed.get(index).copied().unwrap_or(false)
    }

    /// Whether `info` names a lump this pass may copy on its own: not a
    /// marker, not map data, not already taken by a map block.
    fn is_copiable(&self, info: &LumpInfo) -> bool {
        info.kind != LumpKind::Marker && info.map.is_none() && !self.is_consumed(info.index)
    }

    /// The overlay's map blocks, in directory order.
    fn maps(&self) -> Vec<MapPlan> {
        self.analysis
            .maps
            .iter()
            .map(|block| MapPlan {
                name: block.name,
                ranges: block_ranges(block),
            })
            .collect()
    }

    /// Copies every overlay map block: over the base block of its name, or
    /// appended when the base has none.
    pub(super) fn copy_maps(&mut self, target: &mut Base) {
        let plans = self.maps();
        for plan in &plans {
            copy_map_block(self, target, plan);
        }
    }

    /// The overlay entries of `ranges`, in order, marked as consumed.
    fn take_ranges(&mut self, ranges: &[(usize, usize)]) -> Vec<MergedLump> {
        ranges
            .iter()
            .flat_map(|range| range.0..range.1)
            .filter_map(|index| self.take(index))
            .collect()
    }

    /// The overlay's regions: every namespace it carries with the positions of
    /// its members, in first-appearance order.
    fn regions(&self) -> Vec<(Namespace, Vec<usize>)> {
        let mut groups: Vec<(Namespace, Vec<usize>)> = Vec::new();
        push_regions(&mut groups, self);
        groups
    }

    /// Copies every namespace lump into the base's region of that namespace.
    pub(super) fn copy_namespaces(&mut self, target: &mut Base) {
        let groups = self.regions();
        for group in &groups {
            copy_region(self, target, group.0, &group.1);
        }
    }

    /// The overlay's global lumps: outside every region and map block.
    fn globals(&self) -> Vec<usize> {
        self.analysis
            .lumps
            .iter()
            .filter(|info| self.is_copiable(info) && info.namespace.is_none())
            .map(|info| info.index)
            .collect()
    }

    /// Copies every global lump, replacing the last base copy of its name.
    pub(super) fn copy_globals(&mut self, target: &mut Base) {
        let positions = self.globals();
        for index in positions {
            copy_global(self, target, index);
        }
    }
}

/// Collects `pass`'s namespace groups into `groups`, in first-appearance order.
fn push_regions(groups: &mut Vec<(Namespace, Vec<usize>)>, pass: &Pass<'_>) {
    for info in &pass.analysis.lumps {
        let Some(namespace) = info.namespace else {
            continue;
        };
        if !pass.is_copiable(info) {
            continue;
        }
        push_group(groups, namespace, info.index);
    }
}

/// Copies one overlay map block onto `target`: over its namesake's base range,
/// or at the end.
fn copy_map_block(pass: &mut Pass<'_>, target: &mut Base, plan: &MapPlan) {
    let lumps = pass.take_ranges(&plan.ranges);
    let claimed = target.edits.claim_map(plan.name, &target.analysis);
    match claimed {
        Some(range) => {
            target.edits.skip(range);
            target.edits.insert_before(range.0, lumps);
        }
        None => target.append(lumps),
    }
}

/// Copies one region: replacing the base copies in place and placing the rest
/// inside the region, or in a new one.
fn copy_region(pass: &mut Pass<'_>, target: &mut Base, namespace: Namespace, indices: &[usize]) {
    let mut fresh: Vec<MergedLump> = Vec::new();
    for index in indices {
        let Some(lump) = pass.take(*index) else {
            continue;
        };
        match target.find_lump(lump.lump.name, Some(namespace)) {
            Some(at) => target.edits.replace(at, lump),
            None => fresh.push(lump),
        }
    }
    if fresh.is_empty() {
        return;
    }
    if !target.has_namespace(namespace) {
        target.append_region(namespace, fresh);
        return;
    }
    match target.end_marker(namespace) {
        Some(at) => target.edits.insert_before(at, fresh),
        None => target.append(fresh),
    }
}

/// Copies one overlay global lump: over the base's last copy of that name, or
/// at the end.
fn copy_global(pass: &mut Pass<'_>, target: &mut Base, index: usize) {
    let Some(lump) = pass.take(index) else {
        return;
    };
    match target.find_lump(lump.lump.name, None) {
        Some(at) => target.edits.replace(at, lump),
        None => target.append(vec![lump]),
    }
}

/// Adds `index` to `namespace`'s group, opening one when there is none yet.
fn push_group(groups: &mut Vec<(Namespace, Vec<usize>)>, namespace: Namespace, index: usize) {
    let existing = groups.iter_mut().find(|group| group.0 == namespace);
    match existing {
        Some(group) => group.1.push(index),
        None => groups.push((namespace, vec![index])),
    }
}

/// The end of a map block: past its data lumps and past its `GL` block.
pub(super) fn block_end(block: &MapBlock) -> usize {
    let with_gl = block.gl.map_or(block.end, |(_, past_gl)| past_gl);
    with_gl.max(block.end)
}

/// The ranges a map block covers: header through data, plus its `GL` block.
fn block_ranges(block: &MapBlock) -> Vec<(usize, usize)> {
    let mut ranges = vec![(block.header, block.end)];
    if let Some(gl) = block.gl {
        ranges.push(gl);
    }
    ranges
}

/// Whether `info` is the marker that closes `namespace`.
pub(super) fn is_region_end(info: &LumpInfo, namespace: Namespace) -> bool {
    info.kind == LumpKind::Marker
        && info.namespace == Some(namespace)
        && info.name.as_bytes().ends_with(b"_END")
}

/// Whether `info` is the entry a lookup of `name` in `namespace` finds.
pub(super) fn matches_lump(info: &LumpInfo, name: LumpName, namespace: Option<Namespace>) -> bool {
    info.name == name && info.kind != LumpKind::Marker && in_area(info, namespace)
}

/// Whether `info` sits where a lookup of `namespace` searches; `None` is the
/// global area, outside every region and map block.
pub(super) fn in_area(info: &LumpInfo, namespace: Option<Namespace>) -> bool {
    namespace.map_or_else(
        || info.namespace.is_none() && info.map.is_none(),
        |wanted| info.namespace == Some(wanted),
    )
}

/// A synthesized marker named `text`, tagged like `behalf` so the merged view
/// reports which archive its region came from.
pub(super) fn marker_lump(text: &str, behalf: &MergedLump) -> MergedLump {
    let mut raw = [0_u8; NAME_LEN];
    for (slot, byte) in raw.iter_mut().zip(text.as_bytes()) {
        *slot = *byte;
    }
    MergedLump {
        lump: Lump::marker(LumpName::from_raw(raw)),
        source: behalf.source,
        source_index: behalf.source_index,
    }
}

/// Classifies a merged directory, for lookups over the view.
pub(in crate::session) fn analyze_directory(directory: &[MergedLump]) -> Analysis {
    let lumps: Vec<Lump> = directory.iter().map(|entry| entry.lump.clone()).collect();
    analyze(&lumps)
}

/// Tags a whole archive as merged entries, in its own directory order.
#[must_use]
#[inline]
pub fn tag(wad: &Wad, source: Source) -> Vec<MergedLump> {
    wad.lumps
        .iter()
        .cloned()
        .enumerate()
        .map(|(source_index, lump)| MergedLump {
            lump,
            source,
            source_index,
        })
        .collect()
}
