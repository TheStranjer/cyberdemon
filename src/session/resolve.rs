//! The shared graphic-resource resolvers: a lump name in, decoded pixels
//! out.
//!
//! `load_flat`, `load_patch`, `load_texture`, `load_sprite`,
//! `load_sprite_cached` and `load_sprite_lump` implement how an engine
//! finds one picture in the merged view - the flats region first for a
//! flat, the patches region
//! then any loose lump for a patch (a `PNAMES` component may sit
//! anywhere), the `TEXTURE2` then `TEXTURE1` definition (with a `TX_`
//! patch as fallback) for a composite texture, the sprite regions
//! plus the name grammar for a sprite rotation - and decode what they
//! find: raw `Doom` flats and column/post patches, or the `PNG` form ports
//! also carry, quantized against `PLAYPAL` entry 0 with alpha transparency
//! and with the drawing offsets taken from a `grAb` chunk. `lump_bytes`
//! is the same name lookup for a reader of a lump of any kind. Every
//! resolver reads through a `PictureIndex` built once from the cached
//! merged view - the winning row per name and place, the decoded texture
//! definitions and a patch cache shared by one operation - so a texture's
//! N components, a scene's whole picture set and a render's rotation sweep
//! each walk the directory once, not once per picture.
//! The image tools of the `graphics` group publish through these,
//! [`crate::render::scene`] builds its scenes through them and the `assets`
//! group dispatches to all of them, so name resolution is written once. It
//! lives here, beside the session it reads, depending only on the session,
//! [`crate::gfx`] and [`crate::wad`], so no core module has to reach into
//! the tool layer for a picture. The lookup itself is `index`, composite
//! textures resolve in `texture`, sprite rotations in `sprite`.

pub mod index;
pub mod sprite;
pub mod texture;

use sprite::clamp_offset;

pub(crate) use index::PictureIndex;
use index::SpriteRow;
pub(crate) use sprite::{load_sprite, load_sprite_cached, load_sprite_lump};
pub(crate) use texture::load_texture;

use crate::error::CyberError;
use crate::gfx::codec::{decode_image, is_png};
use crate::gfx::flat::decode_flat;
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::{Patch, decode_patch};
use crate::gfx::quantize::{QuantizeOptions, quantize};
use crate::gfx::sprite_name::{ALL_ANGLES, SpriteName};
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// The `PLAYPAL` entry `PNG` graphics are quantized to: ports store such
/// a picture's colours in the normal palette, and a call's own `palette`
/// only recolours the resulting indices.
const PNG_PALETTE: usize = 0_usize;
/// The drawing offsets of a `PNG` picture without a `grAb` chunk.
const NO_OFFSETS: GrabOffsets = (0_i32, 0_i32);
/// Where [`load_patch`] looks: the patches region first, then any loose
/// lump outside the flats and sprites regions.
const PATCH_PLACES: [Place; 2] = [Place::In(Namespace::Patches), Place::Loose];
/// Where [`load_flat`] looks: the flats region first, then anywhere.
const FLAT_PLACES: [Place; 2] = [Place::In(Namespace::Flats), Place::Anywhere];

/// Where a name lookup may find its lump in the merged view. A [`PictureIndex`]
/// files every data row under each place that admits it, so a lookup is one
/// map probe per place rather than a scan of the directory.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Place {
    /// Inside one namespace region.
    In(Namespace),
    /// Outside the flats and sprites regions: the loose lumps `PNAMES` can
    /// name.
    Loose,
    /// Any data lump.
    Anywhere,
}

/// How a found sprite lump serves the rotation that was asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Serving {
    /// The stored image is the answer.
    Direct,
    /// The stored image serves it horizontally flipped.
    Mirrored,
}

/// One sprite rotation a resolver looks for, with the two lump names it
/// can be answered by.
#[derive(Debug)]
struct Request {
    /// Uppercased four-character sprite prefix (`TROO`).
    sprite: String,
    /// Uppercased frame letter.
    frame: char,
    /// Rotation number `0`-`8`.
    rotation: u8,
    /// The name this rotation's own image carries (`TROOA8`).
    exact: LumpName,
    /// The frame's all-angles name (`TROOA0`).
    angles: LumpName,
}

impl Request {
    /// Builds the request for one sprite rotation, uppercasing the prefix
    /// and frame and naming the two lumps through the grammar's
    /// [`SpriteName::format`].
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a frame or prefix character
    /// cannot appear in a lump name.
    fn new(sprite: &str, frame: char, rotation: u8) -> Result<Self, CyberError> {
        let prefix = sprite.to_ascii_uppercase();
        let letter = frame.to_ascii_uppercase();
        let wanted = SpriteName {
            sprite: prefix.clone(),
            frame: letter,
            rotation,
            mirror: None,
        };
        let angles = SpriteName {
            sprite: prefix,
            frame: letter,
            rotation: ALL_ANGLES,
            mirror: None,
        };
        Ok(Self {
            exact: LumpName::parse(&wanted.format())?,
            angles: LumpName::parse(&angles.format())?,
            sprite: wanted.sprite,
            frame: wanted.frame,
            rotation: wanted.rotation,
        })
    }

    /// The `not_found` for a rotation no sprite region serves.
    fn missing(&self) -> CyberError {
        CyberError::NotFound(format!(
            "no {} frame {} rotation {} sprite in the merged view; list_sprites with prefix \
             \"{}\" lists what the session has",
            self.sprite, self.frame, self.rotation, self.sprite
        ))
    }
}

/// Where [`load_sprite_lump`] looks: the sprite regions first, then the
/// patches region and any loose lump, because a whole name is not bound to
/// one region.
const SPRITE_PLACES: [Place; 3] = [
    Place::In(Namespace::Sprites),
    Place::In(Namespace::Patches),
    Place::Loose,
];

/// The `not_found` for a missing picture: names what was looked for and
/// the listing tool that knows what the session holds. Flat and texture
/// refusals carry the create/list hint: those two kinds are what the
/// import tools can make.
fn not_found(kind: &str, lister: &str, name: LumpName) -> CyberError {
    let error = CyberError::NotFound(format!(
        "no {kind} named {name} in the merged view; {lister} lists what is there"
    ));
    match kind {
        "flat" | "texture" => error.with_hint(SURFACE_HINT),
        _ => error,
    }
}

/// The hint attached to missing-flat and missing-texture refusals.
const SURFACE_HINT: &str = "list_textures / list_flats with a glob pattern; or create one with \
                            create_texture_from_image / create_flat_from_image";

/// The indexed pixels of the flat `name`: the last flats-region lump of
/// that name, else the last lump of that name anywhere, decoded as a raw
/// `PLAYPAL` plane or, for `PNG` bytes, quantized against palette 0 with
/// alpha transparency.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`;
/// [`CyberError::Format`] when the bytes are neither a raw flat of a
/// supported size nor a readable `PNG`; [`PictureIndex::reference_palette`]'
/// errors when a `PNG` flat has no `PLAYPAL` to quantize against.
pub(crate) fn load_flat(
    index: &PictureIndex<'_>,
    name: LumpName,
) -> Result<IndexedImage, CyberError> {
    let lump = index
        .located(name, &FLAT_PLACES)
        .ok_or_else(|| not_found("flat", "list_flats", name))?;
    decode_flat_bytes(index, &lump.data)
}

/// The patch `name`: the last patches-region lump, else the last loose one
/// outside the flats and sprites regions, as a `Doom` patch or a quantized
/// `PNG` with its `grAb` offsets. The decode is shared: within one
/// operation - one texture's repeated components, one scene's whole
/// texture set - a name is decoded once and answered from the index's
/// cache after that.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`;
/// [`decode_patch_bytes`]' [`CyberError::Format`] for bytes that are
/// neither format; [`PictureIndex::reference_palette`]' errors for a `PNG`
/// without a `PLAYPAL`.
pub(crate) fn load_patch(index: &PictureIndex<'_>, name: LumpName) -> Result<Patch, CyberError> {
    if let Some(patch) = index.cached_patch(name) {
        return Ok(patch);
    }
    let lump = index
        .located(name, &PATCH_PLACES)
        .ok_or_else(|| not_found("patch", "list_patches", name))?;
    let patch = decode_patch_bytes(index, &lump.data)?;
    index.store_patch(name, &patch);
    Ok(patch)
}

/// A flat lump's pixels: the raw indexed plane, or a `PNG` flat quantized
/// against palette 0.
///
/// # Errors
///
/// [`decode_flat`]' [`CyberError::Format`] for a raw lump of an
/// unsupported size; [`quantize_png`]' errors for a `PNG`.
pub(crate) fn decode_flat_bytes(
    index: &PictureIndex<'_>,
    data: &[u8],
) -> Result<IndexedImage, CyberError> {
    if is_png(data) {
        return quantize_png(index, data);
    }
    decode_flat(data)
}

/// A patch or sprite lump's pixels and drawing offsets: the `Doom`
/// column/post format, or a `PNG` quantized against palette 0 with its
/// offsets from a `grAb` chunk (defaulting to `(0, 0)`).
///
/// # Errors
///
/// [`decode_patch`]' [`CyberError::Format`] for `Doom` bytes that are
/// corrupt; [`quantize_png`]' errors for a `PNG`; [`read_grab`]'
/// [`CyberError::Format`] for a chunk stream that does not parse.
pub(crate) fn decode_patch_bytes(
    index: &PictureIndex<'_>,
    data: &[u8],
) -> Result<Patch, CyberError> {
    if !is_png(data) {
        return decode_patch(data);
    }
    let image = quantize_png(index, data)?;
    let (left, top) = read_grab(data)?.unwrap_or(NO_OFFSETS);
    Ok(Patch {
        left_offset: clamp_offset(left),
        top_offset: clamp_offset(top),
        image,
    })
}

/// A `PNG` image as indexed pixels against the session's palette 0:
/// `Doom` lumps store indices, and pixels below the alpha threshold stay
/// transparent.
///
/// # Errors
///
/// [`decode_image`]' errors for bytes that are not a readable image;
/// [`PictureIndex::reference_palette`]' [`CyberError::NotFound`] for a
/// session without a `PLAYPAL`.
fn quantize_png(index: &PictureIndex<'_>, data: &[u8]) -> Result<IndexedImage, CyberError> {
    let rgba = decode_image(data)?;
    Ok(quantize(
        &rgba,
        &index.reference_palette()?,
        &QuantizeOptions::default(),
    ))
}

/// The winning lump's payload bytes anywhere in the merged view: what a
/// reader of one named lump, whatever it holds, starts from.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`.
pub(crate) fn lump_bytes(index: &PictureIndex<'_>, name: LumpName) -> Result<Vec<u8>, CyberError> {
    index
        .located(name, &[Place::Anywhere])
        .map(|lump| lump.data.to_vec())
        .ok_or_else(|| not_found("lump", "list_lumps", name))
}

#[cfg(test)]
mod tests;
