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
//!
//! The shared shapes and the [`plan`] and [`apply`] entry points, with the
//! private `run` dispatcher between them, stay here; the three redundancy
//! sweeps are the private `redundant` module, the `PNAMES`-trim, patch and
//! flat sweeps that read the merged textures and maps are the private
//! `unused` one, and the animation and switch tables those three grow
//! their used-name sets with are the private `animations` one.

mod animations;
mod redundant;
mod unused;

use alloc::collections::BTreeSet;
use serde::Serialize;

use crate::error::CyberError;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{LumpInfo, MapBlock, analyze};
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::optimize::redundant::{empty_namespaces, identical_overrides, shadowed_duplicates};
use crate::wad::optimize::unused::{unused_flats, unused_patches, unused_pnames};

/// Bytes one `PNAMES` list entry occupies.
const PNAMES_ENTRY_LEN: usize = 8;

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
    /// with every texture lump's patch references renumbered. A name the
    /// animation or switch tables need beside a referenced patch is kept.
    UnusedPnames,
    /// Working `patches`-namespace lumps no texture references by
    /// `PNAMES` name, keeping every animation and switch partner.
    UnusedPatches,
    /// Working `flats`-namespace lumps no sector of any merged map uses,
    /// keeping every animation frame, `ANIMATED` name and sky flat.
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

/// What the content sweeps kept for an animation.
///
/// Each field counts the removal candidates one sweep left alone because
/// the animation and switch tables need them alongside a name that is in
/// use: the frames of a range a used name sits in, the other state of a
/// switch pair, the names a merged `ANIMATED` lump declares and, for
/// flats, the sky flats no sector has to name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct KeptForAnimation {
    /// `PNAMES` entries the trim would otherwise have dropped.
    pub pnames: usize,
    /// Working `patches`-namespace lumps otherwise dropped.
    pub patches: usize,
    /// Working `flats`-namespace lumps otherwise dropped: animation
    /// frames, `ANIMATED` names and the sky flats.
    pub flats: usize,
}

impl KeptForAnimation {
    /// Every keep the three content sweeps recorded, in `operation` order.
    #[must_use]
    #[inline]
    pub const fn total(&self) -> usize {
        self.pnames
            .saturating_add(self.patches)
            .saturating_add(self.flats)
    }
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
    /// Names the content sweeps kept because an animation, a switch pair
    /// or the sky needs them beside a name that is in use.
    pub kept_for_animation: KeptForAnimation,
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
    /// Names the content sweeps left alone for an animation, a switch or
    /// the sky.
    frames: KeptForAnimation,
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
        kept_for_animation: draft.frames,
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
///
/// # Errors
///
/// [`Wad::put_global`]'s [`CyberError::Internal`] for a vanished lump
/// slot, which only a concurrent modification of `wad` could cause.
///
/// [`Wad::put_global`]: crate::wad::archive::Wad::put_global
#[inline]
pub fn apply(wad: &mut Wad, plan: &Plan) -> Result<(), CyberError> {
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
        return Ok(());
    };
    wad.put_global(lumps::PNAMES, rewrite.pnames.clone())?;
    if let Some(bytes) = rewrite.texture1.as_ref() {
        wad.put_global(lumps::TEXTURE1, bytes.clone())?;
    }
    if let Some(bytes) = rewrite.texture2.as_ref() {
        wad.put_global(lumps::TEXTURE2, bytes.clone())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
