//! The three content-reading sweeps: trim the working `PNAMES` to the
//! entries the merged textures reference and renumber those lumps, drop
//! working `patches` no merged texture names, and drop working `flats`
//! no merged sector uses.
//!
//! These are the only sweeps that can fail: each reads `PNAMES`, texture
//! or map bytes through the [`Context`] snapshot's merged view and
//! reports a malformed lump as [`CyberError::Format`] rather than guess.
//! The redundancy sweeps over the same snapshots are the sibling
//! `redundant` module.

use alloc::collections::BTreeSet;

use crate::error::CyberError;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::map::read_map;
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchMapping, TextureLump, parse_pnames, parse_texture_lump, remap_patches, write_pnames,
    write_texture_lump,
};

use super::redundant::{is_global, last_global};
use super::{
    Action, Context, Draft, NONE_FLAT, Operation, PNAMES_ENTRY_LEN, PNAMES_RAW, Rewrite,
    TEXTURE1_RAW, TEXTURE2_RAW, Target,
};

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
pub(super) fn unused_patches(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
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
pub(super) fn unused_flats(context: &Context<'_>, draft: &mut Draft) -> Result<(), CyberError> {
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
