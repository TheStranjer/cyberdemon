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
//! is the same name lookup for a tool that reads a lump of any kind. The
//! image tools of [`crate::tools::graphics`] publish through these,
//! [`crate::render::scene`] builds its scenes through them and
//! `crate::tools::assets` dispatches to all of them, so name resolution
//! is written once. Composite textures resolve in `texture`, sprite
//! rotations in `sprite`.

mod sprite;
pub mod texture;

use sprite::clamp_offset;

pub(in crate::tools) use sprite::{load_sprite, load_sprite_cached, load_sprite_lump};
pub(crate) use texture::load_texture;

use crate::error::CyberError;
use crate::gfx::codec::{decode_image, is_png};
use crate::gfx::flat::decode_flat;
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::IndexedImage;
use crate::gfx::palette::Palette;
use crate::gfx::patch::{Patch, decode_patch};
use crate::gfx::quantize::{QuantizeOptions, quantize};
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::palette::load_palette;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// The `PLAYPAL` entry `PNG` graphics are quantized to: ports store such
/// a picture's colours in the normal palette, and a call's own `palette`
/// only recolours the resulting indices.
const PNG_PALETTE: usize = 0_usize;
/// The drawing offsets of a `PNG` picture without a `grAb` chunk.
const NO_OFFSETS: GrabOffsets = (0_i32, 0_i32);
/// The sprite rotation every frame's single all-angles image serves.
const ALL_ANGLES: u8 = 0_u8;
/// Where [`load_patch`] looks: the patches region first, then any loose
/// lump outside the flats and sprites regions.
const PATCH_PLACES: [Place; 2] = [Place::In(Namespace::Patches), Place::Loose];
/// Where [`load_flat`] looks: the flats region first, then anywhere.
const FLAT_PLACES: [Place; 2] = [Place::In(Namespace::Flats), Place::Anywhere];

/// Where a name lookup may find its lump in the merged view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Place {
    /// Inside one namespace region.
    In(Namespace),
    /// Outside the flats and sprites regions: the loose lumps `PNAMES` can
    /// name.
    Loose,
    /// Any data lump.
    Anywhere,
}

impl Place {
    /// Whether [`build_rows`]' row is a lump this rule lets a lookup
    /// reach; region markers carry no pixels and never match.
    fn allows(self, row: &LumpRow) -> bool {
        if row.kind == LumpKind::Marker {
            return false;
        }
        match self {
            Self::In(wanted) => row.namespace == Some(wanted),
            Self::Loose => !matches!(row.namespace, Some(Namespace::Flats | Namespace::Sprites)),
            Self::Anywhere => true,
        }
    }
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
    /// and frame.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a frame or prefix character
    /// cannot appear in a lump name.
    fn new(sprite: &str, frame: char, rotation: u8) -> Result<Self, CyberError> {
        let prefix = sprite.to_ascii_uppercase();
        let letter = frame.to_ascii_uppercase();
        Ok(Self {
            exact: LumpName::parse(&format!("{prefix}{letter}{rotation}"))?,
            angles: LumpName::parse(&format!("{prefix}{letter}{ALL_ANGLES}"))?,
            sprite: prefix,
            frame: letter,
            rotation,
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

/// The merged directory's rows: every lump of the view an engine reads,
/// in directory order.
///
/// # Errors
///
/// [`build_rows`]' errors for the merged scope.
fn merged_rows(session: &Session) -> Result<Vec<LumpRow>, CyberError> {
    build_rows(session, Scope::Merged)
}

/// The winning row for `name`: the last row of that name the first of
/// `places` with a match allows, which is what an engine's lookup of a
/// name reaches for.
fn locate(rows: &[LumpRow], name: LumpName, places: &[Place]) -> Option<LumpRow> {
    places
        .iter()
        .find_map(|place| {
            rows.iter()
                .rev()
                .find(|row| row.name == name && place.allows(row))
        })
        .cloned()
}

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
/// supported size nor a readable `PNG`; [`load_palette`]' errors when a
/// `PNG` flat has no `PLAYPAL` to quantize against.
pub(crate) fn load_flat(session: &Session, name: LumpName) -> Result<IndexedImage, CyberError> {
    let row = locate(&merged_rows(session)?, name, &FLAT_PLACES)
        .ok_or_else(|| not_found("flat", "list_flats", name))?;
    decode_flat_bytes(session, &row.lump.data)
}

/// The patch `name`: the last patches-region lump, else the last loose one
/// outside the flats and sprites regions, as a `Doom` patch or a quantized
/// `PNG` with its `grAb` offsets.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`;
/// [`decode_patch_bytes`]' [`CyberError::Format`] for bytes that are
/// neither format; [`load_palette`]' errors for a `PNG` without a
/// `PLAYPAL`.
pub(in crate::tools) fn load_patch(session: &Session, name: LumpName) -> Result<Patch, CyberError> {
    let row = locate(&merged_rows(session)?, name, &PATCH_PLACES)
        .ok_or_else(|| not_found("patch", "list_patches", name))?;
    decode_patch_bytes(session, &row.lump.data)
}

/// A flat lump's pixels: the raw indexed plane, or a `PNG` flat quantized
/// against palette 0.
///
/// # Errors
///
/// [`decode_flat`]' [`CyberError::Format`] for a raw lump of an
/// unsupported size; [`quantize_png`]' errors for a `PNG`.
pub(in crate::tools) fn decode_flat_bytes(
    session: &Session,
    data: &[u8],
) -> Result<IndexedImage, CyberError> {
    if is_png(data) {
        return quantize_png(session, data);
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
pub(in crate::tools) fn decode_patch_bytes(
    session: &Session,
    data: &[u8],
) -> Result<Patch, CyberError> {
    if !is_png(data) {
        return decode_patch(data);
    }
    let image = quantize_png(session, data)?;
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
/// [`load_palette`]' [`CyberError::NotFound`] for a session without a
/// `PLAYPAL`.
fn quantize_png(session: &Session, data: &[u8]) -> Result<IndexedImage, CyberError> {
    let rgba = decode_image(data)?;
    Ok(quantize(
        &rgba,
        &reference_palette(session)?,
        &QuantizeOptions::default(),
    ))
}

/// The palette `PNG` graphics are quantized against: `PLAYPAL` entry 0.
///
/// # Errors
///
/// [`load_palette`]' [`CyberError::NotFound`] for a session without a
/// `PLAYPAL`.
fn reference_palette(session: &Session) -> Result<Palette, CyberError> {
    load_palette(session, PNG_PALETTE)
}

/// The winning lump's payload bytes anywhere in the merged view: what a
/// tool that reads one named lump, whatever it holds, starts from.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds `name`.
pub(in crate::tools) fn lump_bytes(
    session: &Session,
    name: LumpName,
) -> Result<Vec<u8>, CyberError> {
    locate(&merged_rows(session)?, name, &[Place::Anywhere])
        .map(|row| row.lump.data.to_vec())
        .ok_or_else(|| not_found("lump", "list_lumps", name))
}

#[cfg(test)]
mod tests;
