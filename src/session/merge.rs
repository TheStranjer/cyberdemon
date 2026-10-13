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

use crate::session::{MergedLump, Source};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, LumpInfo, LumpKind, MapBlock, Namespace, analyze};
use crate::wad::name::{LumpName, NAME_LEN};

/// The edits an overlay wants at each position of a base directory.
///
/// Everything is recorded against the positions the directory had when the
/// edits were planned, so indices stay valid while an overlay is applied.
struct Edits {
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
    fn skip(&mut self, range: (usize, usize)) {
        flag_range(&mut self.skipped, range);
    }

    /// Puts `lump` where entry `at` stood.
    fn replace(&mut self, at: usize, lump: MergedLump) {
        if let Some(slot) = self.overrides.get_mut(at) {
            *slot = Some(lump);
        }
    }

    /// Queues `lumps` to emit just before position `at`.
    fn insert_before(&mut self, at: usize, lumps: Vec<MergedLump>) {
        if let Some(slot) = self.pending.get_mut(at) {
            slot.extend(lumps);
        }
    }

    /// Claims map `name`'s base range, so a second overlay map of that name
    /// appends instead of replacing the same block again.
    fn claim_map(&mut self, name: LumpName, analysis: &Analysis) -> Option<(usize, usize)> {
        if self.claimed.contains(&name) {
            return None;
        }
        let block = analysis.maps.iter().find(|map| map.name == name)?;
        self.claimed.push(name);
        Some((block.header, block_end(block)))
    }
}

/// The directory built so far, with the edits one overlay wants to make to it.
struct Base {
    /// The directory entries, in order.
    lumps: Vec<MergedLump>,
    /// The classification of [`Base::lumps`].
    analysis: Analysis,
    /// What an overlay asked for at each position.
    edits: Edits,
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
    fn append_region(&mut self, namespace: Namespace, mut lumps: Vec<MergedLump>) {
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
    fn append(&mut self, lumps: Vec<MergedLump>) {
        self.trailing.extend(lumps);
    }

    /// Whether the base holds anything in `namespace`, markers included.
    fn has_namespace(&self, namespace: Namespace) -> bool {
        self.analysis
            .lumps
            .iter()
            .any(|info| info.namespace == Some(namespace))
    }

    /// The position of the marker closing `namespace`.
    fn end_marker(&self, namespace: Namespace) -> Option<usize> {
        self.analysis
            .lumps
            .iter()
            .rev()
            .find(|info| is_region_end(info, namespace))
            .map(|info| info.index)
    }

    /// The last base entry named `name` inside `namespace`; `None` namespace
    /// means outside every region and map block.
    fn find_lump(&self, name: LumpName, namespace: Option<Namespace>) -> Option<usize> {
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
struct MapPlan {
    /// The map's name, from its header lump.
    name: LumpName,
    /// The overlay positions of its lumps.
    ranges: Vec<(usize, usize)>,
}

/// One archive being folded onto a directory, with its classification.
struct Pass<'overlay> {
    /// The archive's entries.
    wad: &'overlay Wad,
    /// Their classification.
    analysis: Analysis,
    /// Where the entries this pass copies came from.
    source: Source,
    /// Entries already copied as part of a map block.
    consumed: Vec<bool>,
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
    fn copy_maps(&mut self, target: &mut Base) {
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
    fn copy_namespaces(&mut self, target: &mut Base) {
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
    fn copy_globals(&mut self, target: &mut Base) {
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
fn block_end(block: &MapBlock) -> usize {
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
fn is_region_end(info: &LumpInfo, namespace: Namespace) -> bool {
    info.kind == LumpKind::Marker
        && info.namespace == Some(namespace)
        && info.name.as_bytes().ends_with(b"_END")
}

/// Whether `info` is the entry a lookup of `name` in `namespace` finds.
fn matches_lump(info: &LumpInfo, name: LumpName, namespace: Option<Namespace>) -> bool {
    info.name == name && info.kind != LumpKind::Marker && in_area(info, namespace)
}

/// Whether `info` sits where a lookup of `namespace` searches; `None` is the
/// global area, outside every region and map block.
fn in_area(info: &LumpInfo, namespace: Option<Namespace>) -> bool {
    namespace.map_or_else(
        || info.namespace.is_none() && info.map.is_none(),
        |wanted| info.namespace == Some(wanted),
    )
}

/// A synthesized marker named `text`, tagged like `behalf` so the merged view
/// reports which archive its region came from.
fn marker_lump(text: &str, behalf: &MergedLump) -> MergedLump {
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
pub(crate) fn analyze_directory(directory: &[MergedLump]) -> Analysis {
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

#[cfg(test)]
mod tests {
    use super::{Base, NAME_LEN, analyze_directory, marker_lump, merge, merge_all, tag};
    use crate::session::{MergedLump, Source};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` whose payload is `text`, so copies are tellable.
    fn lump(text: &str) -> Lump {
        Lump::new(name(text), text.as_bytes().to_vec())
    }

    /// A `PWAD` holding `lumps`.
    fn pwad(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    /// The merged directory's names, in order.
    fn names(directory: &[MergedLump]) -> Vec<String> {
        directory
            .iter()
            .map(|entry| entry.lump.name.to_string())
            .collect()
    }

    /// The merged directory as `NAME@IWAD`, `NAME@R1` or `NAME@W`, so both the
    /// order and the winners are visible in one assertion.
    fn tagged(directory: &[MergedLump]) -> Vec<String> {
        directory
            .iter()
            .map(|entry| {
                let source = match entry.source {
                    Source::Layer(0) => "IWAD".to_owned(),
                    Source::Layer(index) => format!("R{index}"),
                    Source::Working => "W".to_owned(),
                };
                format!("{}@{source}", entry.lump.name)
            })
            .collect()
    }

    #[test]
    fn tag_keeps_the_archives_own_order_and_positions() {
        let wad = pwad(vec![lump("PLAYPAL"), marker("F_START")]);
        let view = tag(&wad, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["PLAYPAL@W", "F_START@W"],
            "every entry keeps its archive"
        );
        assert_eq!(
            view.iter()
                .map(|entry| entry.source_index)
                .collect::<Vec<_>>(),
            vec![0_usize, 1],
            "directory positions survive"
        );
    }

    #[test]
    fn an_empty_directory_takes_the_overlay_verbatim() {
        let overlay = pwad(vec![
            lump("PLAYPAL"),
            marker("F_START"),
            lump("FLOOR_1"),
            marker("F_END"),
        ]);
        let view = merge(Vec::new(), &overlay, Source::Layer(0));
        assert_eq!(
            names(&view),
            vec!["PLAYPAL", "F_START", "FLOOR_1", "F_END"],
            "nothing to override, so nothing moved or was dropped"
        );
        assert!(
            view.iter().all(|entry| entry.source == Source::Layer(0)),
            "every entry reports the overlay"
        );
    }

    #[test]
    fn a_global_lump_replaces_the_last_copy_of_its_name() {
        let base = pwad(vec![lump("PLAYPAL"), lump("DOOMGI"), lump("PLAYPAL")]);
        let overlay = pwad(vec![lump("PLAYPAL")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["PLAYPAL@IWAD", "DOOMGI@IWAD", "PLAYPAL@W"],
            "the last copy changed, in place"
        );
    }

    #[test]
    fn an_unknown_global_lump_is_appended() {
        let base = pwad(vec![lump("PLAYPAL")]);
        let overlay = pwad(vec![lump("GAMEINFO")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(names(&view), vec!["PLAYPAL", "GAMEINFO"], "appended last");
    }

    #[test]
    fn a_map_block_replaces_the_base_block_of_the_same_name() {
        let base = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            lump("LINEDEFS"),
            lump("PLAYPAL"),
        ]);
        let overlay = pwad(vec![marker("MAP01"), lump("THINGS")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["MAP01@W", "THINGS@W", "PLAYPAL@IWAD"],
            "the base block's remaining member was dropped in place"
        );
    }

    #[test]
    fn every_base_map_affects_only_its_own_block() {
        let base = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            marker("MAP02"),
            lump("THINGS"),
        ]);
        let overlay = pwad(vec![
            marker("MAP02"),
            lump("THINGS"),
            lump("LINEDEFS"),
            marker("MAP01"),
            lump("THINGS"),
            lump("SIDEDEFS"),
        ]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["MAP01", "THINGS", "SIDEDEFS", "MAP02", "THINGS", "LINEDEFS"],
            "each block was replaced where it stood, in base order"
        );
        assert!(
            view.iter().all(|entry| entry.source == Source::Working),
            "every entry comes from the overlay"
        );
    }

    #[test]
    fn a_gl_block_moves_with_its_map() {
        let base = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            lump("SECTORS"),
            marker("GL_MAP01"),
            lump("GL_VERT"),
            lump("GL_NODES"),
            lump("PLAYPAL"),
        ]);
        let overlay = pwad(vec![marker("MAP01"), lump("THINGS")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["MAP01@W", "THINGS@W", "PLAYPAL@IWAD"],
            "the base block went as a whole, its `GL` nodes with it"
        );
    }

    #[test]
    fn an_overlay_map_bringing_gl_nodes_replaces_the_base_block() {
        let base = pwad(vec![marker("MAP01"), lump("THINGS"), lump("PLAYPAL")]);
        let overlay = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            marker("GL_MAP01"),
            lump("GL_VERT"),
        ]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec![
                "MAP01@W",
                "THINGS@W",
                "GL_MAP01@W",
                "GL_VERT@W",
                "PLAYPAL@IWAD"
            ],
            "the whole block including the `GL` run arrived in place"
        );
    }

    #[test]
    fn an_overlay_map_without_a_base_block_is_appended() {
        let base = pwad(vec![marker("MAP01"), lump("THINGS")]);
        let overlay = pwad(vec![marker("E1M1"), lump("THINGS")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["MAP01", "THINGS", "E1M1", "THINGS"],
            "the new map landed at the end"
        );
        assert_eq!(
            view.get(3).expect("the appended THINGS").source,
            Source::Working,
            "the new map's data is the overlay's"
        );
        assert_eq!(
            view.get(1).expect("the base THINGS").source,
            Source::Layer(0),
            "the old map is untouched"
        );
    }

    #[test]
    fn two_overlay_maps_of_one_name_replace_the_base_only_once() {
        let base = pwad(vec![marker("MAP01"), lump("THINGS")]);
        let overlay = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            marker("MAP01"),
            lump("LINEDEFS"),
        ]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["MAP01", "THINGS", "MAP01", "LINEDEFS"],
            "the second copy appends instead of claiming the base block again"
        );
    }

    #[test]
    fn map_data_is_not_copied_twice_as_a_global() {
        let base = pwad(vec![lump("PLAYPAL")]);
        let overlay = pwad(vec![marker("MAP05"), lump("THINGS"), lump("SECTORS")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["PLAYPAL", "MAP05", "THINGS", "SECTORS"],
            "the block was appended once"
        );
    }

    #[test]
    fn a_namespace_lump_replaces_the_base_copy() {
        let base = pwad(vec![
            marker("F_START"),
            lump("FLOOR_1"),
            lump("FLOOR_2"),
            marker("F_END"),
        ]);
        let overlay = pwad(vec![marker("F_START"), lump("FLOOR_2"), marker("F_END")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["F_START@IWAD", "FLOOR_1@IWAD", "FLOOR_2@W", "F_END@IWAD"],
            "the region's content was replaced, its markers untouched"
        );
    }

    #[test]
    fn a_new_namespace_lump_goes_before_the_end_marker() {
        let base = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
        let overlay = pwad(vec![marker("F_START"), lump("F_NEW"), marker("F_END")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["F_START", "FLOOR_1", "F_NEW", "F_END"],
            "the new flat joined the region instead of trailing it"
        );
    }

    #[test]
    fn a_namespace_the_base_lacks_arrives_as_a_region() {
        let base = pwad(vec![lump("PLAYPAL")]);
        let overlay = pwad(vec![
            marker("F_START"),
            lump("FLOOR_1"),
            lump("FLOOR_2"),
            marker("F_END"),
        ]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["PLAYPAL", "F_START", "FLOOR_1", "FLOOR_2", "F_END"],
            "the region arrived whole, markers included"
        );
        assert_eq!(
            view.get(1).expect("the start marker").source_index,
            1,
            "the synthesized marker reports the region member it opens"
        );
        assert_eq!(
            view.get(4).expect("the end marker").source_index,
            2,
            "the closing marker reports the region's last member"
        );
    }

    #[test]
    fn alias_markers_open_and_close_the_same_region() {
        let base = pwad(vec![marker("SS_START"), lump("TROOA1"), marker("SS_END")]);
        let overlay = pwad(vec![marker("S_START"), lump("TROOB1"), marker("S_END")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["SS_START", "TROOA1", "TROOB1", "SS_END"],
            "the alias region was joined rather than duplicated"
        );
    }

    #[test]
    fn a_region_lump_does_not_override_the_same_named_global() {
        let base = pwad(vec![lump("FLOOR_1")]);
        let overlay = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["FLOOR_1", "F_START", "FLOOR_1", "F_END"],
            "the global stays and the region gets its own copy"
        );
    }

    #[test]
    fn overlay_markers_are_never_copied_on_their_own() {
        let base = pwad(vec![lump("PLAYPAL")]);
        let overlay = pwad(vec![
            marker("F_START"),
            marker("F1_START"),
            lump("FLOOR_1"),
            marker("F1_END"),
            marker("F_END"),
        ]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            names(&view),
            vec!["PLAYPAL", "F_START", "FLOOR_1", "F_END"],
            "one clean region, no inner markers carried over"
        );
    }

    #[test]
    fn a_marker_only_overlay_contributes_nothing() {
        let base = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
        let overlay = pwad(vec![marker("F_START"), marker("F_END")]);
        let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
        assert_eq!(
            tagged(&view),
            vec!["F_START@IWAD", "FLOOR_1@IWAD", "F_END@IWAD"],
            "markers alone are not an override"
        );
    }

    #[test]
    fn the_last_of_three_archives_wins_every_name() {
        let iwad = pwad(vec![
            lump("PLAYPAL"),
            marker("MAP01"),
            lump("THINGS"),
            marker("F_START"),
            lump("FLOOR_1"),
            marker("F_END"),
        ]);
        let resource = pwad(vec![
            lump("GAMEINFO"),
            marker("F_START"),
            lump("F_SKY1"),
            marker("F_END"),
            lump("MIDWARS"),
        ]);
        let working = pwad(vec![
            marker("MAP01"),
            lump("THINGS"),
            marker("F_START"),
            lump("FLOOR_1"),
            marker("F_END"),
        ]);
        let view = merge_all(&[&iwad, &resource], &working);
        assert_eq!(
            tagged(&view),
            vec![
                "PLAYPAL@IWAD",
                "MAP01@W",
                "THINGS@W",
                "F_START@IWAD",
                "FLOOR_1@W",
                "F_SKY1@R1",
                "F_END@IWAD",
                "GAMEINFO@R1",
                "MIDWARS@R1"
            ],
            "each name was overridden where it stood, by the latest archive"
        );
    }

    #[test]
    fn an_empty_overlay_changes_nothing() {
        let base = pwad(vec![lump("PLAYPAL"), marker("MAP01"), lump("THINGS")]);
        let view = merge(
            tag(&base, Source::Layer(0)),
            &Wad::default(),
            Source::Working,
        );
        assert_eq!(
            tagged(&view),
            vec!["PLAYPAL@IWAD", "MAP01@IWAD", "THINGS@IWAD"]
        );
    }

    #[test]
    fn merge_all_numbers_layers_in_load_order() {
        let first = pwad(vec![lump("PLAYPAL")]);
        let second = pwad(vec![lump("ADVSND")]);
        let view = merge_all(&[&first, &second], &Wad::default());
        assert_eq!(
            tagged(&view),
            vec!["PLAYPAL@IWAD", "ADVSND@R1"],
            "layer 0 then layer 1 then nothing from the working archive"
        );
    }

    #[test]
    fn analyze_reports_the_namespace_of_a_merged_region() {
        let iwad = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
        let overlay = pwad(vec![marker("F_START"), lump("FLOOR_2"), marker("F_END")]);
        let view = merge(tag(&iwad, Source::Layer(0)), &overlay, Source::Working);
        let analysis = analyze_directory(&view);
        let flat = analysis
            .lumps
            .iter()
            .find(|info| info.name == name("FLOOR_2"))
            .expect("the new flat");
        assert_eq!(
            flat.namespace,
            Some(Namespace::Flats),
            "a lump inserted before the end marker stays in its region"
        );
    }

    #[test]
    fn skipping_past_the_end_of_the_directory_is_harmless() {
        let mut base = Base::new(tag(&pwad(vec![lump("PLAYPAL")]), Source::Layer(0)));
        base.edits.skip((0, 99));
        assert!(base.finish().is_empty(), "the whole directory was dropped");
    }

    #[test]
    fn a_marker_lump_pads_and_uppercases_its_name() {
        let behalf = MergedLump {
            lump: lump("FLOOR_1"),
            source: Source::Working,
            source_index: 7,
        };
        let start = marker_lump("f_start", &behalf);
        assert_eq!(start.lump.name, name("F_START"), "uppercased");
        assert_eq!(start.lump.size(), 0, "markers are empty");
        assert_eq!(start.source, Source::Working, "it reports its region");
        assert_eq!(start.source_index, 7, "and the region member's position");
        let long = marker_lump("TOOLONGNAME", &behalf);
        assert_eq!(
            long.lump.name.as_bytes().len(),
            NAME_LEN,
            "a name past the directory width is cut to eight bytes"
        );
    }
}
