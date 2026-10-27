//! The three content-reading sweeps: trim the working `PNAMES` to the
//! entries the merged textures reference and renumber those lumps, drop
//! working `patches` no merged texture names, and drop working `flats` no
//! merged sector uses.
//!
//! These are the only sweeps that can fail: each reads `PNAMES`, texture
//! or map bytes through the [`Context`] snapshot's merged view and reports
//! a malformed lump as [`CyberError::Format`] rather than guess. The
//! redundancy sweeps over the same snapshots are the sibling `redundant`
//! module.
//!
//! What each sweep reads as referenced is only the start: an engine finds
//! an animation's frames by walking from a used name to the range's other
//! endpoint, so every frame between them, every switch partner and every
//! name a merged `ANIMATED` lump declares is kept too. Those tables are
//! the sibling [`animations`] module, and what a sweep kept that way is
//! counted in the plan's [`KeptForAnimation`](super::KeptForAnimation).

use alloc::collections::BTreeSet;

use crate::error::CyberError;
use crate::wad::classify::{LumpInfo, LumpKind, Namespace};
use crate::wad::lumps;
use crate::wad::map::consts::{HEXEN_SKY_FLAT, SKY_FLAT, normalized_picture};
use crate::wad::map::read_map;
use crate::wad::name::LumpName;
use crate::wad::optimize::animations::Animations;
use crate::wad::textures::{
    PatchMapping, TextureLump, parse_pnames, parse_texture_lump, remap_patches, write_pnames,
    write_texture_lump,
};

use super::redundant::{is_global, last_global};
use super::{Action, Context, Draft, Operation, PNAMES_ENTRY_LEN, Rewrite, Target};

/// The flats every engine draws a sky with: no sector has to name one for
/// the sweep to keep it, since a sky is chosen by map, not by sector.
/// The union of the per-game markers
/// ([`crate::wad::game::sky_flat`] answers one of them per game).
const SKY_FLATS: [&str; 2] = [SKY_FLAT, HEXEN_SKY_FLAT];

/// The working `PNAMES` to trim: the last global copy, `None` when the
/// working archive holds none or the plan already removes it — a
/// redundant copy of the base's list is dropped, not rewritten.
fn trimmable_pnames(context: &Context<'_>, draft: &Draft) -> Option<usize> {
    let info = context
        .working_info
        .iter()
        .rev()
        .find(|entry| entry.name == lumps::PNAMES && is_global(entry))?;
    (!draft.remove.contains(&info.index)).then_some(info.index)
}

/// The merged view's `PNAMES` names, `None` when it holds no `PNAMES`
/// at all — then no texture reference resolves to a name.
///
/// # Errors
///
/// [`CyberError::Format`] for a malformed `PNAMES`.
fn merged_pnames(context: &Context<'_>) -> Result<Option<Vec<String>>, CyberError> {
    let wanted = lumps::PNAMES;
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
    for name in lumps::TEXTURE_DEF_LUMPS {
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

/// The `PNAMES` names the merged textures reference, before any animation
/// or switch partner is added: what the sweep's own rule proves in use.
///
/// # Errors
///
/// [`referenced_patches`]' errors.
fn direct_patch_names(
    context: &Context<'_>,
    names: &[String],
) -> Result<BTreeSet<String>, CyberError> {
    let used = referenced_patches(context, names.len())?;
    Ok(used
        .iter()
        .filter_map(|entry| names.get(*entry))
        .cloned()
        .collect())
}

/// The `names` positions of every kept name, all occurrences included: a
/// list may hold one patch name twice and the engine reads whichever copy
/// a texture points at.
fn kept_indices(names: &[String], kept: &BTreeSet<String>) -> BTreeSet<usize> {
    names
        .iter()
        .enumerate()
        .filter_map(|(entry, text)| kept.contains(text).then_some(entry))
        .collect()
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
    let texture1 = lumps::TEXTURE1;
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

/// Whether a classified entry is a sweep's candidate: a member of
/// `namespace`'s region, never a marker.
fn region_member(info: &LumpInfo, namespace: Namespace) -> bool {
    info.namespace == Some(namespace) && info.kind != LumpKind::Marker
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
pub(super) fn unused_pnames(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
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
    let direct = direct_patch_names(context, &names)?;
    let kept = Animations::read(context).expand(&names, &direct);
    let spared = names
        .iter()
        .filter(|text| kept.contains(*text) && !direct.contains(*text))
        .count();
    draft.frames.pnames = draft.frames.pnames.saturating_add(spared);
    let used = kept_indices(&names, &kept);
    let Some(mapping) = kept_mapping(&names, &used)? else {
        return Ok(());
    };
    let mut survivors = Vec::with_capacity(used.len());
    for (entry, text) in names.iter().enumerate() {
        if mapping.get(entry).copied().flatten().is_some() {
            survivors.push(text.clone());
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
        pnames: write_pnames(&survivors)?,
        texture1: None,
        texture2: None,
    };
    rewrite_merged_textures(context, &mapping, &mut rewrite)?;
    draft.rewrite = Some(rewrite);
    Ok(())
}

/// Sweeps [`Operation::UnusedPatches`]: working `patches`-namespace lumps
/// no merged texture references by `PNAMES` name, keeping the animation
/// frames and switch partners of every referenced name. Without a merged
/// `PNAMES` no reference resolves to a name, so nothing is touched.
///
/// # Errors
///
/// [`merged_pnames`]' and [`direct_patch_names`]' errors.
pub(super) fn unused_patches(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    let Some(names) = merged_pnames(context)? else {
        return Ok(());
    };
    let direct = direct_patch_names(context, &names)?;
    let kept = Animations::read(context).expand(&names, &direct);
    let mut spared = 0_usize;
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if !region_member(info, Namespace::Patches) {
            continue;
        }
        let text = info.name.to_string();
        if !kept.contains(&text) {
            draft.drop_lump(Operation::UnusedPatches, info.index, lump);
            continue;
        }
        spared = spared.saturating_add(usize::from(!direct.contains(&text)));
    }
    draft.frames.patches = draft.frames.patches.saturating_add(spared);
    Ok(())
}

/// Sweeps [`Operation::UnusedFlats`]: working `flats`-namespace lumps no
/// sector of any merged map uses, keeping the frames of every animation
/// range a used flat sits in, every name a merged `ANIMATED` lump declares
/// and the sky flats.
///
/// # Errors
///
/// [`used_flats`]' error when a merged map does not decode.
pub(super) fn unused_flats(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
    let used = used_flats(context)?;
    let order = merged_flat_order(context);
    let kept = kept_flats(context, &order, &used);
    let mut spared = 0_usize;
    for (info, lump) in context.working_info.iter().zip(context.working) {
        if !region_member(info, Namespace::Flats) {
            continue;
        }
        let text = info.name.to_string();
        if !kept.contains(&text) {
            draft.drop_lump(Operation::UnusedFlats, info.index, lump);
            continue;
        }
        spared = spared.saturating_add(usize::from(!used.contains(&text)));
    }
    draft.frames.flats = draft.frames.flats.saturating_add(spared);
    Ok(())
}

/// The flats the sweep keeps: the sectors' own names grown by every
/// animation frame those names need, then the sky flats, which no sector
/// needs to name for an engine to want them.
fn kept_flats(
    context: &Context<'_>,
    order: &[String],
    used: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut kept = Animations::read(context).expand(order, used);
    for sky in SKY_FLATS {
        kept.insert((*sky).to_owned());
    }
    kept
}

/// The merged view's flat names in directory order: the space a vanilla
/// animation range walks when it looks for the frames between its two
/// endpoints.
fn merged_flat_order(context: &Context<'_>) -> Vec<String> {
    context
        .merged_info
        .iter()
        .filter(|info| region_member(info, Namespace::Flats))
        .map(|info| info.name.to_string())
        .collect()
}

/// The flat names every merged sector uses, uppercased and `NUL`-trimmed,
/// the empty and `"-"` markers left out.
///
/// # Errors
///
/// [`CyberError::Format`] naming the map when a merged block does not
/// decode: its sectors would stay unknown and every flat might still be
/// in use.
fn used_flats(context: &Context<'_>) -> Result<BTreeSet<String>, CyberError> {
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
            add_flat_name(&mut used, &sector.floor_flat);
            add_flat_name(&mut used, &sector.ceiling_flat);
        }
    }
    Ok(used)
}

/// Records one sector flat field, skipping the empty and `"-"` markers the
/// way every other stored picture name is read.
fn add_flat_name(used: &mut BTreeSet<String>, name: &str) {
    if let Some(text) = normalized_picture(name.trim_end_matches('\0')) {
        used.insert(text);
    }
}
