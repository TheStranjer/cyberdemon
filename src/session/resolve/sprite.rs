//! Sprite rotations resolved through the shared patch decoder.
//!
//! One requested rotation of one sprite prefix and frame, found in the
//! merged view's sprite regions by the name grammar (the exact lump, the
//! rotation's own pair, its mirror pair, the frame's all-angles image) and
//! decoded through `super::decode_patch_bytes`, flipped and offset like an
//! engine draws a mirrored rotation. The rows come from the caller's
//! `PictureIndex`, so a whole render's rotation sweep reads the merged
//! directory once and parses each lump name once.

use std::collections::HashMap;

use crate::defs::ThingDef;
use crate::error::CyberError;
use crate::gfx::image::flip_horizontal;
use crate::gfx::patch::Patch;
use crate::gfx::sprite_name::ALL_ANGLES;
use crate::wad::name::LumpName;

use super::{
    PictureIndex, Request, SPRITE_PLACES, Serving, SpriteRow, decode_patch_bytes, not_found,
};

/// The sprite image for one rotation: `sprite` is the four-character
/// prefix, `frame` its letter and `rotation` a number `0`-`8`. The exact
/// lump wins; failing that a lump whose eight-character name carries this
/// rotation as its own pair (`TROOA2A8` is where an `IWAD` keeps frame `A`
/// rotation `2`) or as its mirror pair (flipped here, with
/// `left' = width - left`); failing that the frame's rotation-`0` image.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a prefix or frame character cannot
/// appear in a lump name; [`Request::missing`]' [`CyberError::NotFound`]
/// when the sprite regions serve nothing for the rotation;
/// [`decode_patch_bytes`]' errors for a lump that is not a picture.
pub(crate) fn load_sprite(
    index: &PictureIndex<'_>,
    sprite: &str,
    frame: char,
    rotation: u8,
) -> Result<Sprite, CyberError> {
    let wanted = Request::new(sprite, frame, rotation)?;
    let (row, serving) = find_rotation(index.sprites(), &wanted).ok_or_else(|| wanted.missing())?;
    let stored = decode_patch_bytes(index, &row.lump.data)?;
    Ok(Sprite::of(row.name, stored, serving))
}

/// The sprite image for one rotation of a thing definition's sprite, with
/// one patch decode per lump name across a whole render: the same
/// resolution [`load_sprite`] does, but the serving lump's patch comes
/// from `cache` when an earlier thing already decoded it. A mirrored
/// serving flips a copy, so the cache keeps every patch exactly as
/// stored and one lump serves its direct and mirrored rotations from a
/// single decode.
///
/// # Errors
///
/// [`load_sprite`]'s errors.
pub(crate) fn load_sprite_cached(
    index: &PictureIndex<'_>,
    def: &ThingDef,
    rotation: u8,
    cache: &mut HashMap<LumpName, Patch>,
) -> Result<Sprite, CyberError> {
    let wanted = Request::new(&def.sprite, def.frame, rotation)?;
    let (row, serving) = find_rotation(index.sprites(), &wanted).ok_or_else(|| wanted.missing())?;
    if let Some(patch) = cache.get(&row.name) {
        return Ok(Sprite::of(row.name, patch.clone(), serving));
    }
    let stored = decode_patch_bytes(index, &row.lump.data)?;
    cache.insert(row.name, stored.clone());
    Ok(Sprite::of(row.name, stored, serving))
}

/// The patch stored in the lump `name`, served exactly as stored: no
/// rotation rules and no flipping, so `TROOA2A8` answers with its own
/// image. A whole sprite name is looked for in the sprite regions first,
/// then wherever else a patch may live.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`;
/// [`decode_patch_bytes`]' errors for bytes that are neither a `Doom`
/// patch nor a readable `PNG`.
pub(crate) fn load_sprite_lump(
    index: &PictureIndex<'_>,
    name: LumpName,
) -> Result<Patch, CyberError> {
    let lump = index
        .located(name, &SPRITE_PLACES)
        .ok_or_else(|| not_found("sprite", "list_sprites", name))?;
    decode_patch_bytes(index, &lump.data)
}

/// One resolved sprite image.
#[derive(Debug)]
pub(crate) struct Sprite {
    /// The lump the pixels came from.
    pub(crate) lump: LumpName,
    /// Its image and drawing offsets, already flipped for a mirrored
    /// rotation.
    pub(crate) patch: Patch,
    /// Whether the stored image was flipped horizontally to serve the
    /// requested rotation.
    pub(crate) mirrored: bool,
}

impl Sprite {
    /// The answer from the lump that was found and how it serves.
    fn of(lump: LumpName, patch: Patch, serving: Serving) -> Self {
        let mirrored = serving == Serving::Mirrored;
        Self {
            lump,
            patch: match serving {
                Serving::Direct => patch,
                Serving::Mirrored => mirror_patch(&patch),
            },
            mirrored,
        }
    }
}

/// The last entry that serves the request, with how it serves it.
pub(super) fn find_rotation<'rows>(
    rows: &'rows [SpriteRow],
    wanted: &Request,
) -> Option<(&'rows SpriteRow, Serving)> {
    exact_row(rows, wanted)
        .or_else(|| pair_row(rows, wanted))
        .or_else(|| mirror_row(rows, wanted))
        .or_else(|| angles_row(rows, wanted))
}

/// The lump named for this very rotation.
fn exact_row<'rows>(
    rows: &'rows [SpriteRow],
    wanted: &Request,
) -> Option<(&'rows SpriteRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| row.name == wanted.exact)
        .map(|row| (row, Serving::Direct))
}

/// The last lump whose name carries the rotation as its own, unmirrored
/// pair: an `IWAD` writes a frame's rotations `2`, `3` and `4` into the
/// mirror-pair names `TROOA2A8`, `TROOA3A7` and `TROOA4A6` and never into
/// the three-character ones, so only this reading finds them.
fn pair_row<'rows>(
    rows: &'rows [SpriteRow],
    wanted: &Request,
) -> Option<(&'rows SpriteRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| serves_pair(row, wanted))
        .map(|row| (row, Serving::Direct))
}

/// Whether `row`'s name carries the request's frame and rotation as its
/// first, directly served, pair.
fn serves_pair(row: &SpriteRow, wanted: &Request) -> bool {
    row.parsed.as_ref().is_some_and(|parsed| {
        parsed.sprite == wanted.sprite
            && parsed.frame == wanted.frame
            && parsed.rotation == wanted.rotation
    })
}

/// The last lump whose name carries this rotation as its mirror pair.
fn mirror_row<'rows>(
    rows: &'rows [SpriteRow],
    wanted: &Request,
) -> Option<(&'rows SpriteRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| serves_mirror(row, wanted))
        .map(|row| (row, Serving::Mirrored))
}

/// The frame's all-angles lump, for a rotation nothing else serves.
fn angles_row<'rows>(
    rows: &'rows [SpriteRow],
    wanted: &Request,
) -> Option<(&'rows SpriteRow, Serving)> {
    if wanted.rotation == ALL_ANGLES {
        return None;
    }
    rows.iter()
        .rev()
        .find(|row| row.name == wanted.angles)
        .map(|row| (row, Serving::Direct))
}

/// Whether `row`'s name carries the request's frame and rotation as its
/// second, mirrored, pair.
fn serves_mirror(row: &SpriteRow, wanted: &Request) -> bool {
    row.parsed.as_ref().is_some_and(|parsed| {
        parsed.sprite == wanted.sprite && parsed.mirror == Some((wanted.frame, wanted.rotation))
    })
}

/// Saturates an offset into the `i16` range a patch header stores.
pub(super) fn clamp_offset(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

/// The same patch flipped horizontally, the way an engine draws a mirrored
/// sprite rotation: `left' = width - left`, the top offset unchanged.
pub(super) fn mirror_patch(patch: &Patch) -> Patch {
    let width = i32::try_from(patch.image.width).unwrap_or_else(|_| i32::from(i16::MAX));
    let left = i32::from(patch.left_offset);
    Patch {
        left_offset: width
            .checked_sub(left)
            .map_or_else(|| clamp_offset(left), clamp_offset),
        top_offset: patch.top_offset,
        image: flip_horizontal(&patch.image),
    }
}
