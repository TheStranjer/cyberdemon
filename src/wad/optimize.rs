//! Pure cleanup planning for one working archive: what it can drop and
//! what the `PNAMES` pruning rewrites.
//!
//! [`plan`] takes the three directory snapshots the session hands it —
//! the working archive, the read-only base view and the merged view —
//! plus the [`Operation`]s to run, and returns a [`Plan`]: the [`Action`]
//! rows to report, the working directory entries to remove and the
//! re-encoded `PNAMES`/`TEXTURE1`/`TEXTURE2` bytes the pruning's
//! renumbering needs. [`apply`] commits a finished plan to a [`Wad`].
//! Every rule is the one the `optimize_wad` tool documents — identical
//! overrides, empty marker pairs, shadowed global duplicates,
//! unreferenced `PNAMES` entries, patches and flats — and none of them
//! guesses: a snapshot that cannot be read fails the plan with
//! [`CyberError::Format`] instead of quietly keeping or dropping.
//! Nothing here knows about sessions or MCP; content comparison reuses
//! [`mod@crate::wad::diff`]'s length-then-`FNV`-then-bytes rule and the lump
//! keys are the same `(namespace, name, occurrence)` units the diff
//! reports.

use std::collections::HashMap;

use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use serde::Serialize;

use crate::error::CyberError;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{
    LumpInfo, LumpKind, MapBlock, Namespace, analyze, namespace_end, namespace_start,
};
use crate::wad::diff::same_content;
use crate::wad::map::read_map;
use crate::wad::name::{LumpName, NAME_LEN};
use crate::wad::textures::{
    PatchMapping, TextureLump, parse_pnames, parse_texture_lump, remap_patches, write_pnames,
    write_texture_lump,
};

/// The raw directory name of the `PNAMES` lump.
const PNAMES_RAW: [u8; NAME_LEN] = *b"PNAMES\0\0";
/// The raw directory name of the `TEXTURE1` lump.
const TEXTURE1_RAW: [u8; NAME_LEN] = *b"TEXTURE1";
/// The raw directory name of the `TEXTURE2` lump.
const TEXTURE2_RAW: [u8; NAME_LEN] = *b"TEXTURE2";
/// Bytes one `PNAMES` list entry occupies.
const PNAMES_ENTRY_LEN: usize = 8;
/// The sector flat field that stores "no flat".
const NONE_FLAT: &[u8] = b"-";

/// The global lump names the `ZDoom` family concatenates: every copy in
/// the load order contributes its definitions, so no copy of one of these
/// may be pruned as a shadowed duplicate.
pub const CONCATENATED_NAMES: [&str; 25] = [
    "DECORATE", "ZSCRIPT", "SNDINFO", "MAPINFO", "ZMAPINFO", "UMAPINFO", "LANGUAGE", "GLDEFS",
    "ANIMDEFS", "KEYCONF", "TEXTURES", "DECALDEF", "LOCKDEFS", "SBARINFO", "TERRAIN", "SNDSEQ",
    "LOADACS", "MENUDEF", "FONTDEFS", "CVARINFO", "MODELDEF", "VOXELDEF", "TEAMINFO", "X11R6RGB",
    "DEHACKED",
];

/// One redundancy sweep [`plan`] can run over the working archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Working lumps and whole map blocks byte-identical to what the base
    /// view provides for the same namespace and name.
    IdenticalOverrides,
    /// Marker pairs in the working archive with nothing between them,
    /// inner `F1_START`-style markers that end up empty included.
    EmptyNamespaces,
    /// Working globals whose name appears more than once, keeping only
    /// the last copy; `ZDoom`-concatenated names are never pruned.
    ShadowedDuplicates,
    /// `PNAMES` entries no texture of the merged view references, removed
    /// with every texture lump's patch references renumbered.
    UnusedPnames,
    /// Working `patches`-namespace lumps no texture references by
    /// `PNAMES` name.
    UnusedPatches,
    /// Working `flats`-namespace lumps no sector of any merged map uses.
    UnusedFlats,
}

impl Operation {
    /// Every operation, in planning order.
    pub const ALL: [Self; 6] = [
        Self::IdenticalOverrides,
        Self::EmptyNamespaces,
        Self::ShadowedDuplicates,
        Self::UnusedPnames,
        Self::UnusedPatches,
        Self::UnusedFlats,
    ];

    /// The operations that run when the client names none: everything
    /// whose removal leaves what an engine sees unchanged.
    pub const SAFE: [Self; 4] = [
        Self::IdenticalOverrides,
        Self::EmptyNamespaces,
        Self::ShadowedDuplicates,
        Self::UnusedPnames,
    ];

    /// The snake-case name of this operation, exactly what `Serialize`
    /// writes.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdenticalOverrides => "identical_overrides",
            Self::EmptyNamespaces => "empty_namespaces",
            Self::ShadowedDuplicates => "shadowed_duplicates",
            Self::UnusedPnames => "unused_pnames",
            Self::UnusedPatches => "unused_patches",
            Self::UnusedFlats => "unused_flats",
        }
    }

    /// Parses an operation name, case-insensitively; `None` for anything
    /// else.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|operation| operation.as_str().eq_ignore_ascii_case(text))
    }

    /// The names of `list`, comma-joined, for error messages.
    #[must_use]
    #[inline]
    pub fn names(list: &[Self]) -> String {
        list.iter()
            .map(|operation| operation.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What one [`Action`] takes away.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// One lump — a directory entry or a `PNAMES` list entry — by name.
    Lump(String),
    /// One whole working map block, keyed by its map name.
    Map(LumpName),
}

/// One planned removal, reported to the client and enacted by [`apply`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Action {
    /// The sweep that planned it.
    pub operation: Operation,
    /// What goes: a lump row carries `"lump"`, a map block row `"map"`.
    #[serde(flatten)]
    pub target: Target,
    /// The working directory index of the removed entry — the block
    /// header's for a map — or the `PNAMES` list index of a trimmed name.
    pub index: usize,
    /// Payload bytes the plan frees.
    pub bytes: usize,
}

impl Action {
    /// The name this row removes: the lump's or `PNAMES` entry's, or the
    /// map's.
    #[must_use]
    #[inline]
    pub fn name(&self) -> String {
        match self.target.clone() {
            Target::Lump(text) => text,
            Target::Map(map) => map.to_string(),
        }
    }
}

/// The re-encoded lumps of one `unused_pnames` sweep, ready for
/// [`apply`] to write into the working archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    /// The new complete `PNAMES` bytes.
    pub pnames: Vec<u8>,
    /// The renumbered `TEXTURE1` bytes, when the mapping changed it.
    pub texture1: Option<Vec<u8>>,
    /// The renumbered `TEXTURE2` bytes, when the mapping changed it.
    pub texture2: Option<Vec<u8>>,
}

/// The three directory snapshots one [`plan`] call reads.
pub struct Inputs<'dir> {
    /// The editable working archive.
    pub working: &'dir [Lump],
    /// The read-only layers folded: what the base view provides.
    pub base: &'dir [Lump],
    /// Everything an engine sees: the base view with the working archive
    /// folded on top.
    pub merged: &'dir [Lump],
}

/// The whole outcome of one [`plan`] call.
#[derive(Debug)]
pub struct Plan {
    /// Every planned removal: operation order, then directory order
    /// within an operation.
    pub actions: Vec<Action>,
    /// The working directory entries [`apply`] removes.
    pub remove: BTreeSet<usize>,
    /// The `PNAMES` and texture lumps [`apply`] rewrites, set when the
    /// `unused_pnames` sweep trimmed entries.
    pub rewrite: Option<Rewrite>,
    /// Duplicated `ZDoom`-concatenated names the plan keeps whole.
    pub kept_concatenated: Vec<LumpName>,
}

impl Plan {
    /// Whether the plan has nothing to do.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// The payload bytes the plan frees.
    #[must_use]
    #[inline]
    pub fn bytes_saved(&self) -> usize {
        self.actions.iter().map(|action| action.bytes).sum()
    }

    /// The working directory entries the plan removes.
    #[must_use]
    #[inline]
    pub fn lumps_removed(&self) -> usize {
        self.remove.len()
    }
}

/// The snapshots with their classifications, shared by every sweep.
struct Context<'dir> {
    /// The working directory.
    working: &'dir [Lump],
    /// [`Context::working`] classified.
    working_info: &'dir [LumpInfo],
    /// The working directory's map blocks.
    working_maps: &'dir [MapBlock],
    /// The base directory.
    base: &'dir [Lump],
    /// [`Context::base`] classified.
    base_info: &'dir [LumpInfo],
    /// The base directory's map blocks.
    base_maps: &'dir [MapBlock],
    /// The merged directory.
    merged: &'dir [Lump],
    /// [`Context::merged`] classified.
    merged_info: &'dir [LumpInfo],
    /// The merged directory's map blocks.
    merged_maps: &'dir [MapBlock],
}

/// What the sweeps accumulate while one [`plan`] call runs.
#[derive(Default)]
struct Draft {
    /// The rows collected so far.
    actions: Vec<Action>,
    /// The working entries marked for removal.
    remove: BTreeSet<usize>,
    /// Concatenated names found duplicated.
    kept: BTreeSet<LumpName>,
    /// The pending `PNAMES`/texture rewrite.
    rewrite: Option<Rewrite>,
}

impl Draft {
    /// Plans the removal of the working directory entry `index`.
    fn drop_lump(&mut self, operation: Operation, index: usize, lump: &Lump) {
        self.remove.insert(index);
        self.actions.push(Action {
            operation,
            target: Target::Lump(lump.name.to_string()),
            index,
            bytes: lump.size(),
        });
    }

    /// Plans the removal of the whole working `block` under one map row.
    fn drop_block(&mut self, block: &MapBlock, lumps: &[Lump]) {
        let positions = block_positions(block);
        let bytes: usize = positions
            .iter()
            .filter_map(|index| lumps.get(*index))
            .map(Lump::size)
            .sum();
        self.remove.extend(positions);
        self.actions.push(Action {
            operation: Operation::IdenticalOverrides,
            target: Target::Map(block.name),
            index: block.header,
            bytes,
        });
    }
}

/// Every directory index `block` covers: header, members and any `GL` run.
fn block_positions(block: &MapBlock) -> Vec<usize> {
    let mut out: Vec<usize> = (block.header..block.end).collect();
    if let Some((start, end)) = block.gl {
        out.extend(start..end);
    }
    out
}

/// Plans the cleanup `operations` ask for over `inputs`.
///
/// The operations run in their canonical order regardless of the request
/// order, each one planning against the same snapshot, so the same inputs
/// always produce the same plan.
///
/// # Errors
///
/// [`CyberError::Format`] when a sweep that must read lump contents
/// ([`Operation::UnusedPnames`], [`Operation::UnusedPatches`] or
/// [`Operation::UnusedFlats`]) meets a malformed `PNAMES`, texture lump
/// or map block.
#[inline]
pub fn plan(inputs: &Inputs<'_>, operations: &[Operation]) -> Result<Plan, CyberError> {
    let working = analyze(inputs.working);
    let base = analyze(inputs.base);
    let merged = analyze(inputs.merged);
    let context = Context {
        working: inputs.working,
        working_info: &working.lumps,
        working_maps: &working.maps,
        base: inputs.base,
        base_info: &base.lumps,
        base_maps: &base.maps,
        merged: inputs.merged,
        merged_info: &merged.lumps,
        merged_maps: &merged.maps,
    };
    let mut wanted: Vec<Operation> = operations.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let mut draft = Draft::default();
    for operation in wanted {
        run(operation, &context, &mut draft)?;
    }
    Ok(Plan {
        actions: draft.actions,
        remove: draft.remove,
        rewrite: draft.rewrite,
        kept_concatenated: draft.kept.into_iter().collect(),
    })
}

/// Runs one sweep over `context`, extending `draft`.
///
/// # Errors
///
/// [`unused_pnames`], [`unused_patches`] and [`unused_flats`]' errors;
/// the other three sweeps cannot fail.
fn run(operation: Operation, context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    match operation {
        Operation::IdenticalOverrides => {
            identical_overrides(context, draft);
            Ok(())
        }
        Operation::EmptyNamespaces => {
            empty_namespaces(context, draft);
            Ok(())
        }
        Operation::ShadowedDuplicates => {
            shadowed_duplicates(context, draft);
            Ok(())
        }
        Operation::UnusedPnames => unused_pnames(context, draft),
        Operation::UnusedPatches => unused_patches(context, draft),
        Operation::UnusedFlats => unused_flats(context, draft),
    }
}

/// Commits `plan` to `wad`: drops the planned directory entries and then
/// writes the `PNAMES`/texture rewrite, if the plan carries one.
#[inline]
pub fn apply(wad: &mut Wad, plan: &Plan) {
    if !plan.remove.is_empty() {
        let kept: Vec<Lump> = core::mem::take(&mut wad.lumps)
            .into_iter()
            .enumerate()
            .filter(|pair| !plan.remove.contains(&pair.0))
            .map(|(_, lump)| lump)
            .collect();
        wad.lumps = kept;
    }
    let Some(rewrite) = plan.rewrite.as_ref() else {
        return;
    };
    put_global(wad, LumpName::from_raw(PNAMES_RAW), &rewrite.pnames);
    if let Some(bytes) = rewrite.texture1.as_ref() {
        put_global(wad, LumpName::from_raw(TEXTURE1_RAW), bytes);
    }
    if let Some(bytes) = rewrite.texture2.as_ref() {
        put_global(wad, LumpName::from_raw(TEXTURE2_RAW), bytes);
    }
}

/// Writes `data` as `wad`'s global lump `name`: over the last working
/// copy in place, or appended as a new override.
fn put_global(wad: &mut Wad, name: LumpName, data: &[u8]) {
    if let Some(index) = wad.find_last(name) {
        if let Some(lump) = wad.lumps.get_mut(index) {
            lump.data = Arc::from(data);
        }
        return;
    }
    wad.lumps.push(Lump::new(name, data.to_vec()));
}

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
fn identical_overrides(context: &Context<'_>, draft: &mut Draft) {
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
fn empty_namespaces(context: &Context<'_>, draft: &mut Draft) {
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
fn is_global(info: &LumpInfo) -> bool {
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
fn is_concatenated(name: LumpName) -> bool {
    let bytes = name.as_bytes();
    CONCATENATED_NAMES
        .iter()
        .any(|text| text.as_bytes() == bytes)
}

/// Sweeps [`Operation::ShadowedDuplicates`]: duplicated working globals
/// keep only the last copy — vanilla last-match-wins — while duplicated
/// concatenated names keep every copy and only get reported.
fn shadowed_duplicates(context: &Context<'_>, draft: &mut Draft) {
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
fn last_global<'lumps>(
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

/// The working `PNAMES` to trim: the last global copy, `None` when the
/// working archive holds none or the plan already removes it — a
/// redundant copy of the base's list is dropped, not rewritten.
fn trimmable_pnames(context: &Context<'_>, draft: &Draft) -> Option<usize> {
    let info = context
        .working_info
        .iter()
        .rev()
        .find(|entry| entry.name == LumpName::from_raw(PNAMES_RAW) && is_global(entry))?;
    (!draft.remove.contains(&info.index)).then_some(info.index)
}

/// The merged view's `PNAMES` names, `None` when it holds no `PNAMES`
/// at all — then no texture reference resolves to a name.
///
/// # Errors
///
/// [`CyberError::Format`] for a malformed `PNAMES`.
fn merged_pnames(context: &Context<'_>) -> Result<Option<Vec<String>>, CyberError> {
    let wanted = LumpName::from_raw(PNAMES_RAW);
    let Some(lump) = last_global(context.merged, context.merged_info, wanted) else {
        return Ok(None);
    };
    Ok(Some(parse_pnames(&lump.data)?))
}

/// A merged texture lump paired with the name it sits under.
type DecodedTextures = Vec<(LumpName, TextureLump)>;

/// The merged `TEXTURE1` and `TEXTURE2` lumps decoded, bounded by
/// `pnames_len` patch names.
///
/// # Errors
///
/// [`CyberError::Format`] naming the malformed texture lump.
fn merged_textures(
    context: &Context<'_>,
    pnames_len: usize,
) -> Result<DecodedTextures, CyberError> {
    let mut out = Vec::new();
    for raw in [TEXTURE1_RAW, TEXTURE2_RAW] {
        let name = LumpName::from_raw(raw);
        let Some(lump) = last_global(context.merged, context.merged_info, name) else {
            continue;
        };
        let parsed = parse_texture_lump(&lump.data, pnames_len).map_err(|error| {
            CyberError::Format(format!(
                "the merged {name} does not decode ({error}): the PNAMES sweeps cannot see \
                 which patch names its textures reference"
            ))
        })?;
        out.push((name, parsed));
    }
    Ok(out)
}

/// The `PNAMES` indices the merged texture lumps reference.
///
/// # Errors
///
/// [`merged_textures`]' errors.
fn referenced_patches(
    context: &Context<'_>,
    pnames_len: usize,
) -> Result<BTreeSet<usize>, CyberError> {
    let mut used = BTreeSet::new();
    for (_, lump) in merged_textures(context, pnames_len)? {
        for texture in &lump.textures {
            used.extend(texture.patches.iter().map(|patch| usize::from(patch.patch)));
        }
    }
    Ok(used)
}

/// The renumbering that keeps only the `used` entries of `names`, `None`
/// when every entry is referenced and the list stays as it is.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the list outgrows the `u16` texture
/// index field.
fn kept_mapping(
    names: &[String],
    used: &BTreeSet<usize>,
) -> Result<Option<PatchMapping>, CyberError> {
    if used.len() == names.len() {
        return Ok(None);
    }
    if names.len() > usize::from(u16::MAX) {
        return Err(CyberError::LimitExceeded(format!(
            "PNAMES holds {} entries, past the {} the texture index field reaches",
            names.len(),
            u16::MAX
        )));
    }
    let mut mapping = PatchMapping::with_capacity(names.len());
    let mut next = 0_u16;
    for entry in 0_usize..names.len() {
        if used.contains(&entry) {
            mapping.push(Some(next));
            next = next.saturating_add(1);
        } else {
            mapping.push(None);
        }
    }
    Ok(Some(mapping))
}

/// Re-encodes the merged texture lumps through `mapping`, keeping every
/// renumbered lump in `rewrite` for the working archive.
///
/// # Errors
///
/// [`merged_textures`]' and [`write_texture_lump`]' errors.
fn rewrite_merged_textures(
    context: &Context<'_>,
    mapping: &PatchMapping,
    rewrite: &mut Rewrite,
) -> Result<(), CyberError> {
    let texture1 = LumpName::from_raw(TEXTURE1_RAW);
    for (name, lump) in merged_textures(context, mapping.len())? {
        let mut edited = lump;
        if remap_patches(&mut edited, mapping) == 0 {
            continue;
        }
        let bytes = write_texture_lump(&edited)?;
        if name == texture1 {
            rewrite.texture1 = Some(bytes);
        } else {
            rewrite.texture2 = Some(bytes);
        }
    }
    Ok(())
}

/// Sweeps [`Operation::UnusedPnames`]: trim the working `PNAMES` to the
/// entries the merged textures reference and renumber those lumps. The
/// trimmed entries become report rows; the re-encoded lumps become the
/// plan's [`Rewrite`]. Without a working `PNAMES` the sweep does nothing:
/// the base's list is read-only.
///
/// # Errors
///
/// [`CyberError::Format`] for a malformed `PNAMES` or texture lump, plus
/// [`kept_mapping`]' and [`write_pnames`]' errors.
fn unused_pnames(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    let Some(index) = trimmable_pnames(context, draft) else {
        return Ok(());
    };
    let Some(lump) = context.working.get(index) else {
        return Ok(());
    };
    let names = parse_pnames(&lump.data).map_err(|error| {
        CyberError::Format(format!(
            "the working PNAMES (index {index}) does not decode ({error}): fix it with \
             edit_pnames or drop unused_pnames from operations"
        ))
    })?;
    let used = referenced_patches(context, names.len())?;
    let Some(mapping) = kept_mapping(&names, &used)? else {
        return Ok(());
    };
    let mut kept = Vec::with_capacity(used.len());
    for (entry, text) in names.iter().enumerate() {
        if mapping.get(entry).copied().flatten().is_some() {
            kept.push(text.clone());
        } else {
            draft.actions.push(Action {
                operation: Operation::UnusedPnames,
                target: Target::Lump(text.clone()),
                index: entry,
                bytes: PNAMES_ENTRY_LEN,
            });
        }
    }
    let mut rewrite = Rewrite {
        pnames: write_pnames(&kept)?,
        texture1: None,
        texture2: None,
    };
    rewrite_merged_textures(context, &mapping, &mut rewrite)?;
    draft.rewrite = Some(rewrite);
    Ok(())
}

/// Sweeps [`Operation::UnusedPatches`]: working `patches`-namespace lumps
/// no merged texture references by `PNAMES` name. Without a merged
/// `PNAMES` no reference resolves to a name, so nothing is touched.
///
/// # Errors
///
/// [`merged_pnames`]' and [`referenced_patches`]' errors.
fn unused_patches(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    let Some(names) = merged_pnames(context)? else {
        return Ok(());
    };
    let used = referenced_patches(context, names.len())?;
    let referenced: BTreeSet<String> = used
        .iter()
        .filter_map(|entry| names.get(*entry))
        .cloned()
        .collect();
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if info.namespace != Some(Namespace::Patches) || info.kind == LumpKind::Marker {
            continue;
        }
        if !referenced.contains(&info.name.to_string()) {
            draft.drop_lump(Operation::UnusedPatches, info.index, lump);
        }
    }
    Ok(())
}

/// Sweeps [`Operation::UnusedFlats`]: working `flats`-namespace lumps no
/// sector of any merged map uses.
///
/// # Errors
///
/// [`used_flats`]' error when a merged map does not decode.
fn unused_flats(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    let used = used_flats(context)?;
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if info.namespace != Some(Namespace::Flats) || info.kind == LumpKind::Marker {
            continue;
        }
        if !used.contains(info.name.as_bytes()) {
            draft.drop_lump(Operation::UnusedFlats, info.index, lump);
        }
    }
    Ok(())
}

/// The flat names every merged sector uses, `NUL`-trimmed and uppercased.
///
/// # Errors
///
/// [`CyberError::Format`] naming the map when a merged block does not
/// decode: its sectors would stay unknown and every flat might still be
/// in use.
fn used_flats(context: &Context<'_>) -> Result<BTreeSet<Vec<u8>>, CyberError> {
    let mut used = BTreeSet::new();
    for block in context.merged_maps {
        let map = read_map(context.merged, block).map_err(|error| {
            CyberError::Format(format!(
                "map {} does not decode ({error}): unused_flats cannot prove which of its \
                 sectors' flats are used; repair the map or drop unused_flats from operations",
                block.name
            ))
        })?;
        for sector in &map.sectors {
            add_flat_name(&mut used, sector.floor_flat.as_bytes());
            add_flat_name(&mut used, sector.ceiling_flat.as_bytes());
        }
    }
    Ok(used)
}

/// Records one sector flat field, skipping the empty and `"-"` markers.
fn add_flat_name(used: &mut BTreeSet<Vec<u8>>, bytes: &[u8]) {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let Some(name_bytes) = bytes.get(..end) else {
        return;
    };
    if name_bytes.is_empty() || name_bytes == NONE_FLAT {
        return;
    }
    let name: Vec<u8> = name_bytes.iter().map(u8::to_ascii_uppercase).collect();
    used.insert(name);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Action, CONCATENATED_NAMES, Inputs, Operation, Plan, Target, apply, is_concatenated, plan,
    };
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Map, Sector, write_map};
    use crate::wad::name::LumpName;
    use crate::wad::textures::{
        PatchRef, TextureDef, TextureLayout, TextureLump, parse_pnames, parse_texture_lump,
        write_pnames, write_texture_lump,
    };

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `payload`.
    fn lump(text: &str, payload: &[u8]) -> Lump {
        Lump::new(name(text), payload.to_vec())
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// The `(operation, target)` pairs of a plan, for one-glance asserts.
    fn rows(plan: &Plan) -> Vec<(Operation, String)> {
        plan.actions
            .iter()
            .map(|action| (action.operation, action.name()))
            .collect()
    }

    /// The names an action list targets, lump or map.
    fn targets(plan: &Plan) -> Vec<String> {
        plan.actions.iter().map(Action::name).collect()
    }

    /// The indices `operation` planned to drop, in action order.
    fn dropped(plan: &Plan, operation: Operation) -> Vec<usize> {
        plan.actions
            .iter()
            .filter(|action| action.operation == operation)
            .map(|action| action.index)
            .collect()
    }

    /// A plan of `operations` where merged is base then working.
    fn planned(working: &[Lump], base: &[Lump], operations: &[Operation]) -> Plan {
        let mut merged: Vec<Lump> = base.to_vec();
        merged.extend_from_slice(working);
        let inputs = Inputs {
            working,
            base,
            merged: &merged,
        };
        plan(&inputs, operations).expect("the test sweeps run")
    }

    /// A one-sector binary `Doom` block named `text` whose sector floors
    /// `floor` and ceilings `ceiling`.
    fn flat_map(text: &str, floor: &str, ceiling: &str) -> Vec<Lump> {
        let mut map = Map::new_empty(name(text), MapFormat::Doom);
        map.sectors.push(Sector {
            floor_flat: floor.to_owned(),
            ceiling_flat: ceiling.to_owned(),
            ..Sector::default()
        });
        write_map(&map).expect("the test map serializes")
    }

    /// A `PNAMES` lump listing `names`.
    fn pnames(names: &[&str]) -> Lump {
        let list: Vec<String> = names.iter().map(|text| (*text).to_owned()).collect();
        Lump::new(
            name("PNAMES"),
            write_pnames(&list).expect("test names are valid"),
        )
    }

    /// A one-texture `TEXTURE1` referencing `patches`.
    fn texture1(patches: &[u16]) -> Lump {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![TextureDef {
                name: "WALL1".to_owned(),
                flags_raw: 0,
                width: 64,
                height: 128,
                patches: patches
                    .iter()
                    .copied()
                    .map(|patch| PatchRef { patch, x: 0, y: 0 })
                    .collect(),
            }],
        };
        Lump::new(
            name("TEXTURE1"),
            write_texture_lump(&lump).expect("the test lump serializes"),
        )
    }

    /// The `PNAMES` names a payload carries.
    fn pnames_of(data: &[u8]) -> Vec<String> {
        parse_pnames(data).expect("the test PNAMES parses")
    }

    /// A working `patches` region holding each of `names`.
    fn patch_region(names: &[&str]) -> Vec<Lump> {
        let mut out = vec![marker("P_START")];
        for text in names {
            out.push(lump(text, b"patch-bytes"));
        }
        out.push(marker("P_END"));
        out
    }

    #[test]
    fn operation_names_round_trip_and_the_default_set_is_the_safe_four() {
        assert_eq!(Operation::ALL.len(), 6, "six sweeps exist");
        for operation in Operation::ALL {
            assert_eq!(
                Operation::parse(operation.as_str()),
                Some(operation),
                "exact parse of {}",
                operation.as_str()
            );
            assert_eq!(
                Operation::parse(&operation.as_str().to_ascii_uppercase()),
                Some(operation),
                "any case parses"
            );
        }
        assert_eq!(
            Operation::parse("unused_everything"),
            None,
            "unknown names do not parse"
        );
        assert_eq!(
            Operation::SAFE,
            [
                Operation::IdenticalOverrides,
                Operation::EmptyNamespaces,
                Operation::ShadowedDuplicates,
                Operation::UnusedPnames,
            ],
            "the documented safe set"
        );
        assert_eq!(
            Operation::names(&Operation::SAFE),
            "identical_overrides, empty_namespaces, shadowed_duplicates, unused_pnames",
            "the join reads as documented"
        );
    }

    #[test]
    fn every_concatenated_name_is_a_valid_lump_name_and_recognised() {
        assert_eq!(CONCATENATED_NAMES.len(), 25, "the documented list");
        for text in CONCATENATED_NAMES {
            let parsed = name(text);
            assert!(
                is_concatenated(parsed),
                "{text} must be recognised after name normalisation"
            );
        }
        assert!(
            !is_concatenated(name("PLAYPAL")),
            "plain globals can be pruned"
        );
    }

    #[test]
    fn an_identical_global_override_is_planned_and_a_changed_one_is_not() {
        let base = vec![lump("PLAYPAL", b"same"), lump("COLORMAP", b"base")];
        let working = vec![lump("PLAYPAL", b"same"), lump("COLORMAP", b"edited")];
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert_eq!(
            rows(&outcome),
            vec![(Operation::IdenticalOverrides, "PLAYPAL".to_owned())],
            "only the byte-identical copy goes"
        );
        assert_eq!(
            dropped(&outcome, Operation::IdenticalOverrides),
            vec![0],
            "at its working index"
        );
        assert_eq!(outcome.bytes_saved(), 4, "the dropped payload");
        assert_eq!(outcome.lumps_removed(), 1, "one entry");
    }

    #[test]
    fn namespace_and_map_membership_key_the_identical_override_match() {
        let base = vec![
            marker("F_START"),
            lump("FLOOR_1", b"flat"),
            marker("F_END"),
            lump("PLAYPAL", b"pp"),
        ];
        let working = vec![
            lump("FLOOR_1", b"flat"),
            marker("F_START"),
            lump("PLAYPAL", b"pp"),
            marker("S_END"),
        ];
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert!(
            rows(&outcome).is_empty(),
            "a loose flat never pairs with the region's copy, and a region PLAYPAL never pairs \
             with the base's global one: {:?}",
            rows(&outcome)
        );
    }

    #[test]
    fn an_identical_map_block_drops_whole_and_a_changed_one_stays() {
        let base = flat_map("MAP01", "FLOOR1", "FLOOR1");
        let mut working = flat_map("MAP01", "FLOOR1", "FLOOR1");
        working.extend(flat_map("MAP02", "FLOOR1", "FLOOR1"));
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert_eq!(
            rows(&outcome),
            vec![(Operation::IdenticalOverrides, "MAP01".to_owned())],
            "only the byte-identical block is a unit worth removing"
        );
        let action = outcome.actions.first().expect("one row");
        assert_eq!(action.index, 0, "the block reports at its header");
        let expected_bytes: usize = base.iter().skip(1).map(Lump::size).sum();
        assert_eq!(
            action.bytes, expected_bytes,
            "every member's bytes are freed"
        );
        assert_eq!(
            outcome.lumps_removed(),
            base.len(),
            "the whole block counts toward the removal"
        );
    }

    #[test]
    fn empty_marker_pairs_go_including_the_ones_emptied_from_inside() {
        let working = vec![
            marker("F_START"),
            marker("F_END"),
            marker("S_START"),
            lump("TROOA1", b"sprite"),
            marker("S_END"),
            marker("FF_START"),
            marker("F1_START"),
            marker("F1_END"),
            marker("F_END"),
            marker("F_START"),
            marker("S_END"),
        ];
        let outcome = planned(&working, &[], &Operation::SAFE);
        let mut empty = dropped(&outcome, Operation::EmptyNamespaces);
        empty.sort_unstable();
        assert_eq!(
            empty,
            vec![0, 1, 5, 6, 7, 8],
            "the bare F_START/F_END and the nested FF pair go; the non-empty sprites region \
             and the mismatched F_START/S_END stay"
        );
        assert!(
            outcome.actions.iter().all(|action| action.bytes == 0),
            "markers free no payload"
        );
    }

    #[test]
    fn region_alias_pairs_pair_and_mismatched_ones_do_not() {
        let working = vec![
            marker("SS_START"),
            marker("F_END"),
            marker("A_START"),
            marker("A_END"),
        ];
        let outcome = planned(&working, &[], &[Operation::EmptyNamespaces]);
        assert_eq!(
            dropped(&outcome, Operation::EmptyNamespaces),
            vec![2, 3],
            "SS_START names sprites while F_END names flats, so the first pair is two different \
             regions and never an empty one; A_START/A_END is"
        );
    }

    #[test]
    fn shadowed_duplicates_keep_the_last_copy_and_concatenated_names_whole() {
        let working = vec![
            lump("NOTE", b"one"),
            lump("NOTE", b"two"),
            lump("NOTE", b"three"),
            lump("DECORATE", b"a"),
            lump("DECORATE", b"b"),
        ];
        let outcome = planned(&working, &[], &Operation::SAFE);
        assert_eq!(
            dropped(&outcome, Operation::ShadowedDuplicates),
            vec![0, 1],
            "only the last NOTE survives the sweep"
        );
        assert_eq!(
            outcome.kept_concatenated,
            vec![name("DECORATE")],
            "the concatenated pair stays whole and is reported"
        );
    }

    #[test]
    fn a_single_copy_is_no_shadow_and_region_copies_are_not_globals() {
        let working = vec![
            lump("NOTE", b"only"),
            marker("P_START"),
            lump("ART", b"a"),
            lump("ART", b"b"),
            marker("P_END"),
        ];
        let outcome = planned(&working, &[], &Operation::SAFE);
        assert!(
            rows(&outcome).is_empty(),
            "one copy shadows nothing and duplicate patches are not globals: {:?}",
            rows(&outcome)
        );
    }

    #[test]
    fn unused_pnames_prunes_the_list_and_renumbers_the_texture_lump() {
        let base = vec![pnames(&["WALLP1", "STEPP1"]), texture1(&[0])];
        let working = vec![pnames(&["WALLP1", "TALK", "STEPP1"]), texture1(&[0, 2])];
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert_eq!(
            rows(&outcome),
            vec![(Operation::UnusedPnames, "TALK".to_owned())],
            "the unreferenced middle entry"
        );
        assert_eq!(outcome.lumps_removed(), 0, "no directory entry goes");
        assert_eq!(outcome.bytes_saved(), 8, "one PNAMES slot");
        assert_eq!(
            dropped(&outcome, Operation::UnusedPnames),
            vec![1],
            "the row carries the PNAMES list index"
        );
        let rewrite = outcome.rewrite.as_ref().expect("the rewrite");
        assert_eq!(
            pnames_of(&rewrite.pnames),
            vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
            "the list shrinks"
        );
        let bytes = rewrite.texture1.as_ref().expect("TEXTURE1 renumbered");
        let parsed = parse_texture_lump(bytes, 2).expect("the re-encoded lump parses");
        let patches = &parsed.textures.first().expect("WALL1").patches;
        assert_eq!(patches.len(), 2, "both references survive");
        assert_eq!(patches[1].patch, 1, "STEPP1 moved from index 2 to 1");
        assert!(
            rewrite.texture2.is_none(),
            "there is no TEXTURE2 to rewrite"
        );
    }

    #[test]
    fn unused_pnames_skips_a_working_copy_the_plan_removes_as_redundant() {
        let shared = pnames(&["WALLP1", "TALK"]);
        let textures = texture1(&[0]);
        let base = vec![shared.clone(), textures];
        let working = vec![shared];
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert_eq!(
            rows(&outcome),
            vec![(Operation::IdenticalOverrides, "PNAMES".to_owned())],
            "the byte-identical override goes whole instead of being trimmed"
        );
        assert!(outcome.rewrite.is_none(), "and nothing is rewritten");
        let empty = vec![lump("MISC", b"m")];
        let second = planned(&empty, &base, &Operation::SAFE);
        assert!(
            second.rewrite.is_none() && second.actions.is_empty(),
            "no working PNAMES means the sweep does nothing"
        );
    }

    #[test]
    fn a_fully_referenced_pnames_plans_nothing() {
        let base: Vec<Lump> = Vec::new();
        let working = vec![pnames(&["WALLP1"]), texture1(&[0])];
        let outcome = planned(&working, &base, &Operation::SAFE);
        assert!(
            outcome.rewrite.is_none() && outcome.actions.is_empty(),
            "every entry is referenced"
        );
    }

    #[test]
    fn a_malformed_working_pnames_fails_the_plan_with_format() {
        let base: Vec<Lump> = Vec::new();
        let working = vec![lump("PNAMES", b"\x05\x00\x00\x00short")];
        let inputs = Inputs {
            working: &working,
            base: &base,
            merged: &working,
        };
        let error = plan(&inputs, &[Operation::UnusedPnames]).expect_err("truncated list");
        assert_eq!(error.code(), "format_error", "malformed PNAMES code");
    }

    #[test]
    fn unused_patches_drops_only_what_no_texture_references_by_name() {
        let base = vec![pnames(&["WALLP1", "STEPP1"]), texture1(&[0])];
        let mut working = patch_region(&["WALLP1", "STEPP1", "EXTRA"]);
        working.push(lump("WALLP1", b"loose-is-not-a-patch"));
        let outcome = planned(&working, &base, &[Operation::UnusedPatches]);
        let names = targets(&outcome);
        assert_eq!(
            names,
            vec!["STEPP1".to_owned(), "EXTRA".to_owned()],
            "the referenced patch and the loose copy stay, the region's unreferenced names go"
        );
    }

    #[test]
    fn unused_patches_without_a_pnames_touches_nothing() {
        let base: Vec<Lump> = Vec::new();
        let working = patch_region(&["ORPHAN"]);
        let outcome = planned(&working, &base, &[Operation::UnusedPatches]);
        assert!(
            outcome.actions.is_empty(),
            "no name list means no reference to check"
        );
    }

    #[test]
    fn unused_flats_drops_only_what_no_merged_sector_uses() {
        let base = flat_map("MAP01", "FLOOR1", "F_SKY1");
        let working = vec![
            marker("F_START"),
            lump("FLOOR1", b"used"),
            lump("F_SKY1", b"sky"),
            lump("GONE", b"nobody"),
            marker("F_END"),
        ];
        let outcome = planned(&working, &base, &[Operation::UnusedFlats]);
        let names = targets(&outcome);
        assert_eq!(
            names,
            vec!["GONE".to_owned()],
            "only the flat no sector names"
        );
    }

    #[test]
    fn unused_flats_refuses_when_a_merged_map_cannot_decode() {
        let mut base = flat_map("MAP01", "FLOOR1", "FLOOR1");
        if let Some(slot) = base.iter_mut().find(|entry| entry.name == name("THINGS")) {
            *slot = lump("THINGS", b"\x01\x02\x03");
        }
        let working = vec![marker("F_START"), lump("GONE", b"nobody"), marker("F_END")];
        let mut merged: Vec<Lump> = base.clone();
        merged.extend_from_slice(&working);
        let inputs = Inputs {
            working: &working,
            base: &base,
            merged: &merged,
        };
        let error = plan(&inputs, &[Operation::UnusedFlats]).expect_err("broken map");
        assert_eq!(error.code(), "format_error", "undecodable map code");
        assert!(
            error.to_string().contains("MAP01"),
            "the message names the map: {error}"
        );
    }

    #[test]
    fn requested_subsets_run_only_their_own_sweep_in_canonical_order() {
        let base = vec![lump("PLAYPAL", b"same")];
        let working = vec![
            lump("PLAYPAL", b"same"),
            lump("NOTE", b"a"),
            lump("NOTE", b"b"),
        ];
        let subset = planned(&working, &base, &[Operation::ShadowedDuplicates]);
        assert_eq!(
            rows(&subset),
            vec![(Operation::ShadowedDuplicates, "NOTE".to_owned())],
            "only the asked-for sweep ran"
        );
        let both = planned(
            &working,
            &base,
            &[Operation::ShadowedDuplicates, Operation::IdenticalOverrides],
        );
        assert_eq!(
            rows(&both),
            vec![
                (Operation::IdenticalOverrides, "PLAYPAL".to_owned()),
                (Operation::ShadowedDuplicates, "NOTE".to_owned()),
            ],
            "the request order never changes the plan"
        );
    }

    #[test]
    fn apply_drops_the_planned_entries_and_writes_the_rewrite_in_place() {
        let base = vec![lump("PLAYPAL", b"same")];
        let working = vec![
            lump("PLAYPAL", b"same"),
            pnames(&["WALLP1", "TALK"]),
            texture1(&[0]),
        ];
        let outcome = planned(&working, &base, &Operation::ALL);
        assert_eq!(
            outcome.bytes_saved(),
            12,
            "PLAYPAL bytes plus one PNAMES slot"
        );
        let mut wad = Wad {
            kind: WadKind::Pwad,
            lumps: working,
        };
        apply(&mut wad, &outcome);
        assert_eq!(wad.lumps.len(), 2, "the identical override went");
        assert_eq!(wad.find_first(name("PLAYPAL")), None, "by name");
        let kept = wad.lumps.first().expect("PNAMES is first now");
        assert_eq!(
            pnames_of(&kept.data),
            vec!["WALLP1".to_owned()],
            "the list was trimmed in place, not appended"
        );
        assert_eq!(
            wad.find_last(name("TEXTURE1")).expect("TEXTURE1 stays"),
            1,
            "and it was not re-added"
        );
    }

    #[test]
    fn actions_serialize_with_the_documented_keys() {
        let lump_action = Action {
            operation: Operation::UnusedPnames,
            target: Target::Lump("TALK".to_owned()),
            index: 1,
            bytes: 8,
        };
        assert_eq!(
            serde_json::to_value(&lump_action).expect("serializable"),
            json!({
                "operation": "unused_pnames",
                "lump": "TALK",
                "index": 1_u64,
                "bytes": 8_u64,
            }),
            "a lump row"
        );
        let map_action = Action {
            operation: Operation::IdenticalOverrides,
            target: Target::Map(name("MAP01")),
            index: 0,
            bytes: 100,
        };
        assert_eq!(
            serde_json::to_value(&map_action).expect("serializable"),
            json!({
                "operation": "identical_overrides",
                "map": "MAP01",
                "index": 0_u64,
                "bytes": 100_u64,
            }),
            "a map row"
        );
    }

    #[test]
    fn no_operations_requested_plans_an_empty_result() {
        let working = vec![lump("PLAYPAL", b"x")];
        let outcome = planned(&working, &[], &[]);
        assert!(outcome.is_empty(), "nothing requested, nothing planned");
        assert_eq!(outcome.bytes_saved(), 0, "no bytes");
        assert_eq!(outcome.lumps_removed(), 0, "no lumps");
    }
}
