//! The three redundancy sweeps: working lumps and map blocks the base
//! view already provides byte-identical, marker pairs with nothing
//! between them (inner pairs emptied from the inside included), and
//! duplicated working globals reduced to the last copy — the
//! `ZDoom`-concatenated names excepted, which are kept whole and only
//! reported. None of them decodes a lump payload, so none can fail.
//!
//! Every sweep plans against the shared [`Context`] snapshot and records
//! its findings in the [`Draft`]; the shapes they report and the [`super::plan`]
//! and [`super::apply`] entry points stay in the root.

use std::collections::HashMap;

use crate::wad::archive::Lump;
use crate::wad::classify::{
    LumpInfo, LumpKind, MapBlock, Namespace, namespace_end, namespace_start,
};
use crate::wad::diff::same_content;
use crate::wad::name::LumpName;

use super::{CONCATENATED_NAMES, Context, Draft, Operation};

/// The `(namespace, name, occurrence)` key [`mod@crate::wad::diff`] compares
/// non-marker, non-map lumps under.
type ContentKey = (Option<Namespace>, LumpName, usize);

/// The region-and-name key the occurrence counting runs on.
type RegionKey = (Option<Namespace>, LumpName);

/// Record `key` once more in `counts` and report its zero-based
/// occurrence.
fn occurrence(counts: &mut HashMap<RegionKey, usize>, key: RegionKey) -> usize {
    let slot = counts.entry(key).or_insert(0_usize);
    let at = *slot;
    *slot = at.saturating_add(1);
    at
}

/// Whether a classified entry takes part in content comparison: neither a
/// marker nor map-block data.
fn comparable(info: &LumpInfo) -> bool {
    info.kind != LumpKind::Marker && info.map.is_none()
}

/// Key every comparable entry of one directory to its lump.
fn content_map<'lumps>(
    lumps: &'lumps [Lump],
    infos: &[LumpInfo],
) -> HashMap<ContentKey, &'lumps Lump> {
    let mut counts: HashMap<RegionKey, usize> = HashMap::new();
    let mut out: HashMap<ContentKey, &Lump> = HashMap::new();
    for (info, lump) in infos.iter().zip(lumps) {
        if comparable(info) {
            let key = (
                info.namespace,
                info.name,
                occurrence(&mut counts, (info.namespace, info.name)),
            );
            out.insert(key, lump);
        }
    }
    out
}

/// Sweeps [`Operation::IdenticalOverrides`]: working entries the base
/// view already provides byte-identical, lumps one by one and whole map
/// blocks as units.
pub(super) fn identical_overrides(context: &Context<'_>, draft: &mut Draft) {
    let base_content = content_map(context.base, context.base_info);
    let mut counts: HashMap<RegionKey, usize> = HashMap::new();
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if !comparable(info) {
            continue;
        }
        let key = (
            info.namespace,
            info.name,
            occurrence(&mut counts, (info.namespace, info.name)),
        );
        if base_content
            .get(&key)
            .is_some_and(|base| same_content(&base.data, &lump.data))
        {
            draft.drop_lump(Operation::IdenticalOverrides, info.index, lump);
        }
    }
    identical_blocks(context, draft);
}

/// Record `name` once more in `counts` and report its zero-based
/// occurrence.
fn next_occurrence(counts: &mut HashMap<LumpName, usize>, name: LumpName) -> usize {
    let slot = counts.entry(name).or_insert(0_usize);
    let at = *slot;
    *slot = at.saturating_add(1);
    at
}

/// Sweeps the map-block half of [`Operation::IdenticalOverrides`]:
/// working blocks whose every member matches the base block of the same
/// name occurrence, header and `GL` run included.
fn identical_blocks(context: &Context<'_>, draft: &mut Draft) {
    let mut base_counts: HashMap<LumpName, usize> = HashMap::new();
    let mut base_blocks: HashMap<(LumpName, usize), &MapBlock> = HashMap::new();
    for block in context.base_maps {
        let at = next_occurrence(&mut base_counts, block.name);
        base_blocks.insert((block.name, at), block);
    }
    let mut working_counts: HashMap<LumpName, usize> = HashMap::new();
    for block in context.working_maps {
        let at = next_occurrence(&mut working_counts, block.name);
        let identical = base_blocks
            .get(&(block.name, at))
            .is_some_and(|base| blocks_identical(context.base, base, context.working, block));
        if identical {
            draft.drop_block(block, context.working);
        }
    }
}

/// Whether two map blocks hold the same members in the same order with
/// the same bytes.
fn blocks_identical(
    base_lumps: &[Lump],
    base: &MapBlock,
    working_lumps: &[Lump],
    working: &MapBlock,
) -> bool {
    let ours = block_members(base_lumps, base);
    let theirs = block_members(working_lumps, working);
    ours.len() == theirs.len()
        && core::iter::zip(ours, theirs).all(|(ours_lump, theirs_lump)| {
            ours_lump.name == theirs_lump.name && same_content(&ours_lump.data, &theirs_lump.data)
        })
}

/// The member lumps of `block`: after the header to the end, plus any
/// `GL` run.
fn block_members<'lumps>(lumps: &'lumps [Lump], block: &MapBlock) -> Vec<&'lumps Lump> {
    let mut out: Vec<&Lump> = lumps
        .get(block.header.saturating_add(1)..block.end)
        .unwrap_or(&[])
        .iter()
        .collect();
    if let Some((start, end)) = block.gl {
        out.extend(lumps.get(start..end).unwrap_or(&[]).iter());
    }
    out
}

/// Sweeps [`Operation::EmptyNamespaces`]: adjacent start/end marker pairs
/// naming one region; pairs that end up empty when their inner pair goes
/// are removed in the same sweep, round by round.
pub(super) fn empty_namespaces(context: &Context<'_>, draft: &mut Draft) {
    let mut seq: Vec<usize> = (0_usize..context.working.len()).collect();
    loop {
        let pairs = adjacent_empty_pairs(context.working, &seq);
        if pairs.is_empty() {
            break;
        }
        for pair in pairs {
            drop_pair(context, draft, pair, &mut seq);
        }
    }
}

/// Removes one empty marker pair `pair` from `seq`, recording both.
fn drop_pair(
    context: &Context<'_>,
    draft: &mut Draft,
    (start, end): (usize, usize),
    seq: &mut Vec<usize>,
) {
    for index in [start, end] {
        if let Some(lump) = context.working.get(index) {
            draft.drop_lump(Operation::EmptyNamespaces, index, lump);
        }
    }
    seq.retain(|at| *at != start && *at != end);
}

/// The disjoint adjacent empty marker pairs of `seq`, in directory order.
fn adjacent_empty_pairs(lumps: &[Lump], seq: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cursor = 0_usize;
    while let (Some(first), Some(second)) = (seq.get(cursor), seq.get(cursor.saturating_add(1))) {
        let pair = match (lumps.get(*first), lumps.get(*second)) {
            (Some(start_lump), Some(end_lump)) => is_empty_pair(start_lump, end_lump),
            _ => false,
        };
        if pair {
            out.push((*first, *second));
            cursor = cursor.saturating_add(2);
        } else {
            cursor = cursor.saturating_add(1);
        }
    }
    out
}

/// Whether the two adjacent lumps are an empty marker pair: a start and
/// an end naming the same region back to back.
fn is_empty_pair(first: &Lump, second: &Lump) -> bool {
    region_start(&first.name)
        .is_some_and(|start| region_end(&second.name).is_some_and(|end| start == end))
}

/// The region a marker name opens or closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region<'name> {
    /// A canonical namespace boundary: `F_START`, `SS_END` and friends.
    Namespace(Namespace),
    /// An inner marker such as `F1_START`, identified by the name before
    /// its `_START`/`_END` suffix.
    Inner(&'name [u8]),
}

/// The region `name` opens; `None` when the name starts no region.
fn region_start(name: &LumpName) -> Option<Region<'_>> {
    let bytes = name.as_bytes();
    if let Some(namespace) = namespace_start(bytes) {
        return Some(Region::Namespace(namespace));
    }
    bytes.strip_suffix(b"_START").map(Region::Inner)
}

/// The region `name` closes; `None` when the name ends no region.
fn region_end(name: &LumpName) -> Option<Region<'_>> {
    let bytes = name.as_bytes();
    if let Some(namespace) = namespace_end(bytes) {
        return Some(Region::Namespace(namespace));
    }
    bytes.strip_suffix(b"_END").map(Region::Inner)
}

/// The working directory's global-name copies: how many and where the
/// last one sits.
struct NameTally {
    /// Copy counts per global name.
    counts: HashMap<LumpName, usize>,
    /// The last working index holding each global name.
    last: HashMap<LumpName, usize>,
}

/// Whether this entry is a global: outside every region and map block,
/// and no marker.
pub(super) fn is_global(info: &LumpInfo) -> bool {
    info.namespace.is_none() && info.map.is_none() && info.kind != LumpKind::Marker
}

/// Tally the working directory's global entries by name.
fn tally_globals(context: &Context<'_>) -> NameTally {
    let mut tally = NameTally {
        counts: HashMap::new(),
        last: HashMap::new(),
    };
    for info in context.working_info {
        if is_global(info) {
            let slot = tally.counts.entry(info.name).or_insert(0_usize);
            *slot = slot.saturating_add(1);
            tally.last.insert(info.name, info.index);
        }
    }
    tally
}

/// Whether `name` is one the `ZDoom` family concatenates across copies.
pub(super) fn is_concatenated(name: LumpName) -> bool {
    let bytes = name.as_bytes();
    CONCATENATED_NAMES
        .iter()
        .any(|text| text.as_bytes() == bytes)
}

/// Sweeps [`Operation::ShadowedDuplicates`]: duplicated working globals
/// keep only the last copy — vanilla last-match-wins — while duplicated
/// concatenated names keep every copy and only get reported.
pub(super) fn shadowed_duplicates(context: &Context<'_>, draft: &mut Draft) {
    let tally = tally_globals(context);
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if !is_global(info) {
            continue;
        }
        let duplicated = tally
            .counts
            .get(&info.name)
            .is_some_and(|total| *total > 1_usize);
        if !duplicated {
            continue;
        }
        if is_concatenated(info.name) {
            draft.kept.insert(info.name);
            continue;
        }
        if tally.last.get(&info.name) != Some(&info.index) {
            draft.drop_lump(Operation::ShadowedDuplicates, info.index, lump);
        }
    }
}

/// The last global entry named `name` in one classified directory, the
/// copy an engine's lookup would find.
pub(super) fn last_global<'lumps>(
    lumps: &'lumps [Lump],
    infos: &[LumpInfo],
    name: LumpName,
) -> Option<&'lumps Lump> {
    infos
        .iter()
        .rev()
        .find(|info| info.name == name && is_global(info))
        .and_then(|info| lumps.get(info.index))
}
