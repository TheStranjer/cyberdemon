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
//! is written once.

use std::collections::HashMap;

use crate::defs::ThingDef;
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, is_png};
use crate::gfx::composite::{Component, Composite, compose};
use crate::gfx::flat::decode_flat;
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::{IndexedImage, flip_horizontal};
use crate::gfx::palette::Palette;
use crate::gfx::patch::{Patch, decode_patch};
use crate::gfx::quantize::{QuantizeOptions, quantize};
use crate::gfx::sprite_name::parse_sprite_name;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::palette::load_palette;
use crate::tools::textures::TextureSet;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;
use crate::wad::textures::{TextureDef, parse_pnames};

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

/// The composed pixels of the wall texture `name`: the merged view's
/// `TEXTURE2` definition, else its `TEXTURE1` one, else a patch lump of
/// that name inside the `TX_` region (what ports store already-composed
/// textures as). A definition composites through
/// [`crate::gfx::composite`]: present patches draw in record order,
/// absent ones leave holes and land in [`Composite::missing`] and
/// [`Composite::problems`].
///
/// # Errors
///
/// [`CyberError::NotFound`] when neither texture lump defines `name` and
/// no `TX_` patch carries it; [`TextureSet::load`]' and
/// [`decode_patch_bytes`]' format errors for lumps that do not decode; a
/// `TX_` fallback that is not a decodable patch fails the same way.
pub(crate) fn load_texture(session: &Session, name: LumpName) -> Result<Composite, CyberError> {
    let set = TextureSet::load(session)?;
    let text = name.to_string();
    if let Some((def, _lump)) = set.find(&text) {
        return compose_texture(session, &text, def);
    }
    let row = locate(
        &merged_rows(session)?,
        name,
        &[Place::In(Namespace::Textures)],
    )
    .ok_or_else(|| not_found("texture", "list_textures", name))?;
    let patch = decode_patch_bytes(session, &row.lump.data)?;
    Ok(Composite {
        image: patch.image,
        missing: Vec::new(),
        problems: Vec::new(),
    })
}

/// The merged view's `PNAMES` list: the winning `PNAMES` lump parsed, or
/// empty when none exists or it does not decode (a texture component
/// index then has no name to resolve).
fn patch_name_list(session: &Session) -> Vec<String> {
    lump_bytes(session, LumpName::from_raw(*b"PNAMES\0\0"))
        .ok()
        .and_then(|bytes| parse_pnames(&bytes).ok())
        .unwrap_or_default()
}

/// The pixels of the patch lump `name`, `None` when the name is no lump
/// name or no patches-region or loose lump of it decodes.
fn patch_pixels(session: &Session, name: &str) -> Option<IndexedImage> {
    let lump = LumpName::parse(name).ok()?;
    load_patch(session, lump).ok().map(|patch| patch.image)
}

/// Composite one texture definition: its `PNAMES`-resolved components
/// (or a placeholder naming the raw index when `PNAMES` cannot answer),
/// drawn at their offsets in record order.
fn compose_texture(
    session: &Session,
    name: &str,
    def: &TextureDef,
) -> Result<Composite, CyberError> {
    let pnames = patch_name_list(session);
    let labels: Vec<String> = def
        .patches
        .iter()
        .map(|place| {
            pnames
                .get(usize::from(place.patch))
                .cloned()
                .unwrap_or_else(|| format!("PNAMES index {}", place.patch))
        })
        .collect();
    let images: Vec<Option<IndexedImage>> = labels
        .iter()
        .map(|label| patch_pixels(session, label))
        .collect();
    let components: Vec<Component<'_>> = def
        .patches
        .iter()
        .zip(labels.iter().zip(images.iter()))
        .map(|(place, (label, image))| Component {
            name: label,
            image: image.as_ref(),
            x: i32::from(place.x),
            y: i32::from(place.y),
        })
        .collect();
    compose(
        name,
        u32::from(def.width),
        u32::from(def.height),
        &components,
    )
}

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
pub(in crate::tools) fn load_sprite(
    session: &Session,
    sprite: &str,
    frame: char,
    rotation: u8,
) -> Result<Sprite, CyberError> {
    let wanted = Request::new(sprite, frame, rotation)?;
    let rows = sprite_rows(session)?;
    let (row, serving) = find_rotation(&rows, &wanted).ok_or_else(|| wanted.missing())?;
    let stored = decode_patch_bytes(session, &row.lump.data)?;
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
pub(in crate::tools) fn load_sprite_cached(
    session: &Session,
    def: &ThingDef,
    rotation: u8,
    cache: &mut HashMap<LumpName, Patch>,
) -> Result<Sprite, CyberError> {
    let wanted = Request::new(&def.sprite, def.frame, rotation)?;
    let rows = sprite_rows(session)?;
    let (row, serving) = find_rotation(&rows, &wanted).ok_or_else(|| wanted.missing())?;
    if let Some(patch) = cache.get(&row.name) {
        return Ok(Sprite::of(row.name, patch.clone(), serving));
    }
    let stored = decode_patch_bytes(session, &row.lump.data)?;
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
pub(in crate::tools) fn load_sprite_lump(
    session: &Session,
    name: LumpName,
) -> Result<Patch, CyberError> {
    let row = locate(&merged_rows(session)?, name, &SPRITE_PLACES)
        .ok_or_else(|| not_found("sprite", "list_sprites", name))?;
    decode_patch_bytes(session, &row.lump.data)
}

/// One resolved sprite image.
#[derive(Debug)]
pub(in crate::tools) struct Sprite {
    /// The lump the pixels came from.
    pub(in crate::tools) lump: LumpName,
    /// Its image and drawing offsets, already flipped for a mirrored
    /// rotation.
    pub(in crate::tools) patch: Patch,
    /// Whether the stored image was flipped horizontally to serve the
    /// requested rotation.
    pub(in crate::tools) mirrored: bool,
}

impl Sprite {
    /// The answer from the lump that was found and how it served.
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

/// The data lumps of the sprite regions, in directory order.
///
/// # Errors
///
/// [`merged_rows`]' errors.
fn sprite_rows(session: &Session) -> Result<Vec<LumpRow>, CyberError> {
    Ok(merged_rows(session)?
        .into_iter()
        .filter(|row| row.kind != LumpKind::Marker && row.namespace == Some(Namespace::Sprites))
        .collect())
}

/// The last row that serves the request, with how it serves it.
fn find_rotation<'rows>(
    rows: &'rows [LumpRow],
    wanted: &Request,
) -> Option<(&'rows LumpRow, Serving)> {
    exact_row(rows, wanted)
        .or_else(|| pair_row(rows, wanted))
        .or_else(|| mirror_row(rows, wanted))
        .or_else(|| angles_row(rows, wanted))
}

/// The lump named for this very rotation.
fn exact_row<'rows>(rows: &'rows [LumpRow], wanted: &Request) -> Option<(&'rows LumpRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| row.name == wanted.exact)
        .map(|row| (row, Serving::Direct))
}

/// The last lump whose name carries the rotation as its own, unmirrored
/// pair: an `IWAD` writes a frame's rotations `2`, `3` and `4` into the
/// mirror-pair names `TROOA2A8`, `TROOA3A7` and `TROOA4A6` and never into
/// the three-character ones, so only this reading finds them.
fn pair_row<'rows>(rows: &'rows [LumpRow], wanted: &Request) -> Option<(&'rows LumpRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| serves_pair(row, wanted))
        .map(|row| (row, Serving::Direct))
}

/// Whether `row`'s name carries the request's frame and rotation as its
/// first, directly served, pair.
fn serves_pair(row: &LumpRow, wanted: &Request) -> bool {
    parse_sprite_name(&row.name.to_string()).is_some_and(|parsed| {
        parsed.sprite == wanted.sprite
            && parsed.frame == wanted.frame
            && parsed.rotation == wanted.rotation
    })
}

/// The last lump whose name carries this rotation as its mirror pair.
fn mirror_row<'rows>(
    rows: &'rows [LumpRow],
    wanted: &Request,
) -> Option<(&'rows LumpRow, Serving)> {
    rows.iter()
        .rev()
        .find(|row| serves_mirror(row, wanted))
        .map(|row| (row, Serving::Mirrored))
}

/// The frame's all-angles lump, for a rotation nothing else serves.
fn angles_row<'rows>(
    rows: &'rows [LumpRow],
    wanted: &Request,
) -> Option<(&'rows LumpRow, Serving)> {
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
fn serves_mirror(row: &LumpRow, wanted: &Request) -> bool {
    parse_sprite_name(&row.name.to_string()).is_some_and(|parsed| {
        parsed.sprite == wanted.sprite && parsed.mirror == Some((wanted.frame, wanted.rotation))
    })
}

/// The same patch flipped horizontally, the way an engine draws a mirrored
/// sprite rotation: `left' = width - left`, the top offset unchanged.
fn mirror_patch(patch: &Patch) -> Patch {
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

/// Saturates an offset into the `i16` range a patch header stores.
fn clamp_offset(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
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
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::{
        Place, Request, Serving, clamp_offset, decode_patch_bytes, find_rotation, load_flat,
        load_patch, load_sprite, load_sprite_cached, load_sprite_lump, load_texture, locate,
        lump_bytes, merged_rows, mirror_patch, not_found, sprite_rows,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::{IndexedImage, RgbaImage};
    use crate::gfx::patch::{Patch, encode_patch};
    use crate::session::{Layer, LayerRole, Session};
    use crate::tools::lumps::rows::LumpRow;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;
    use crate::wad::textures::{
        PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
    };

    /// A parsed fixture name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `data`.
    fn lump(text: &str, data: Vec<u8>) -> Lump {
        Lump::new(name(text), data)
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// One opaque `RGBA` pixel of `color`.
    fn pixel(color: [u8; 3]) -> [u8; 4] {
        let [red, green, blue] = color;
        [red, green, blue, 255_u8]
    }

    /// A `width`x`height` `PNG` of one colour with optional `grAb` offsets.
    fn png(width: u32, height: u32, color: [u8; 3], offsets: Option<(i32, i32)>) -> Vec<u8> {
        flat_png(width, height, pixel(color), offsets)
    }

    /// A `width`x`height` `PNG` repeating one pixel, with optional `grAb`.
    fn flat_png(width: u32, height: u32, one: [u8; 4], offsets: Option<(i32, i32)>) -> Vec<u8> {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let mut pixels = Vec::with_capacity(count.saturating_mul(4));
        for _pixel in 0..count {
            pixels.extend_from_slice(&one);
        }
        let img = RgbaImage::new(width, height, pixels).expect("test shape is valid");
        encode_png(&img, offsets).expect("test PNGs encode")
    }

    /// One `PLAYPAL` entry: colour `i` is `(i, 255 - i, i)`, 14 copies.
    fn playpal() -> Vec<u8> {
        let mut entry = Vec::new();
        for index in 0_u8..=255 {
            entry.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
        }
        let mut palettes = Vec::new();
        for _palette in 0_u8..14 {
            palettes.extend_from_slice(&entry);
        }
        palettes
    }

    /// A `Doom` patch of a shape and offsets, every pixel `fill`.
    fn doom_patch(width: u32, height: u32, offsets: (i16, i16), fill: u8) -> Vec<u8> {
        let columns = usize::try_from(width).expect("test width");
        let total = columns
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let image = IndexedImage::new(width, height, vec![fill; total], vec![true; total])
            .expect("test shapes are valid");
        encode_patch(&Patch {
            left_offset: offsets.0,
            top_offset: offsets.1,
            image,
        })
        .expect("test patches encode")
    }

    /// A `Doom` patch `width` by 2 whose only opaque column is `column`.
    fn stripe(width: u32, column: u32, offsets: (i16, i16)) -> Vec<u8> {
        let columns = usize::try_from(width).expect("test width");
        let total = columns.checked_mul(2).expect("test pixels fit");
        let mut indices = vec![0_u8; total];
        let mut opaque = vec![false; total];
        for row in 0_usize..2 {
            let slot = row
                .checked_mul(columns)
                .and_then(|start| start.checked_add(usize::try_from(column).expect("test column")))
                .expect("test index fits");
            indices[slot] = 5_u8;
            opaque[slot] = true;
        }
        let image = IndexedImage::new(width, 2, indices, opaque).expect("test shape is valid");
        encode_patch(&Patch {
            left_offset: offsets.0,
            top_offset: offsets.1,
            image,
        })
        .expect("test patches encode")
    }

    /// Appends `extra` to the fixture session as one working-archive edit.
    fn with_extra(extra: Vec<Lump>) -> Session {
        let mut built = session();
        built
            .edit("add extras", |wad| {
                wad.lumps.extend(extra);
                Ok(())
            })
            .expect("plain append");
        built
    }

    /// The fixture session: a loose flat, two `FLOOR1` copies, a `PNG`
    /// flat, a region patch and a loose `PNG` patch, sprite-region lumps
    /// covering the direct, mirrored and all-angles grammar, and one lump
    /// that is no picture at all.
    fn session() -> Session {
        let mut built = Session::default();
        built.set_iwad(Layer::new(
            PathBuf::from("gfx.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    lump("PLAYPAL", playpal()),
                    lump("LOOSE1", vec![9_u8; 4_096]),
                    lump("FLOOR1", vec![1_u8; 4_096]),
                    lump("PNGFLAT", png(2, 2, [64, 191, 64], None)),
                    lump("FLOOR1", vec![2_u8; 4_096]),
                    lump("WALLP1", doom_patch(4, 4, (-2, -14), 0x11)),
                    lump(
                        "PNGPAT",
                        png(3, 2, [0x11, 0x22, 0x33], Some((3_i32, -4_i32))),
                    ),
                    Lump::marker(name("S_START")),
                    lump("TROOA1", doom_patch(8, 8, (4, 0), 0x33)),
                    lump("TROOA2A8", stripe(4, 0, (1, 2))),
                    lump("PLAYA0", doom_patch(16, 16, (0, 0), 0x44)),
                    Lump::marker(name("S_END")),
                    lump("JUNK", vec![b'x'; 40]),
                ],
            },
        ));
        built
    }

    /// The first index of a decoded image, for "which bytes won" checks.
    fn first_index(image: &IndexedImage) -> u8 {
        *image.indices.first().expect("a decoded image holds pixels")
    }

    #[test]
    fn flats_resolve_the_region_copy_first_and_the_last_of_a_name() {
        let session = session();
        assert_eq!(
            first_index(&load_flat(&session, name("FLOOR1")).expect("FLOOR1 is a flat")),
            2,
            "the last FLOOR1 wins"
        );
        assert_eq!(
            load_flat(&session, name("LOOSE1"))
                .expect("a loose flat is the second chance")
                .width,
            64,
            "4096 bytes is 64x64"
        );
        let missing = load_flat(&session, name("NOPE")).expect_err("no such flat");
        assert_eq!(code(&missing), "not_found", "missing flat code");
        assert!(
            missing.to_string().contains("list_flats"),
            "the message names the listing: {missing}"
        );
    }

    #[test]
    fn a_png_flat_is_quantized_to_palette_zero() {
        let session = session();
        let flat = load_flat(&session, name("PNGFLAT")).expect("the PNG flat decodes");
        assert_eq!((flat.width, flat.height), (2, 2), "the PNG shape");
        assert_eq!(first_index(&flat), 64, "(64,191,64) is colour 64");
        assert!(
            flat.opaque.iter().all(|visible| *visible),
            "opaque pixels stay opaque"
        );
    }

    #[test]
    fn a_png_pixel_below_the_alpha_threshold_stays_a_hole() {
        let hole = flat_png(1, 1, [0_u8, 0, 0, 0], None);
        let session = with_extra(vec![lump("HOLE", hole)]);
        let image = load_flat(&session, name("HOLE")).expect("the PNG flat decodes");
        assert_eq!(
            image.get(0, 0).map(|(_, visible)| visible),
            Some(false),
            "alpha 0 is a hole"
        );
    }

    #[test]
    fn patches_prefer_the_region_and_reject_unreadable_bytes() {
        let session = session();
        let wall = load_patch(&session, name("WALLP1")).expect("the region patch");
        assert_eq!(
            (wall.left_offset, wall.top_offset),
            (-2, -14),
            "header offsets survive"
        );
        assert_eq!(wall.image.width, 4, "the stored shape");
        let junk = load_patch(&session, name("JUNK")).expect_err("40 x's are no patch");
        assert_eq!(code(&junk), "format_error", "undecodable patch code");
        let missing = load_patch(&session, name("NOPE")).expect_err("no such patch");
        assert!(
            missing.to_string().contains("list_patches"),
            "the message names the listing: {missing}"
        );
    }

    #[test]
    fn loose_png_patches_quantize_and_carry_their_grab_offsets() {
        let session = session();
        let loose = load_patch(&session, name("PNGPAT")).expect("the loose PNG");
        assert_eq!((loose.left_offset, loose.top_offset), (3, -4), "from grAb");
        assert_eq!(
            (loose.image.width, loose.image.height),
            (3, 2),
            "the PNG shape"
        );
        let no_grab = png(2, 1, [0x10, 0x20, 0x30], None);
        let grown = with_extra(vec![lump("NOGRAB", no_grab)]);
        assert_eq!(
            load_patch(&grown, name("NOGRAB"))
                .expect("the PNG decodes")
                .left_offset,
            0,
            "no grAb is offset 0"
        );
    }

    /// The sprite rows of the fixture session.
    fn rows(session: &Session) -> Vec<LumpRow> {
        sprite_rows(session).expect("the fixture lists")
    }

    /// The request for one fixture rotation.
    fn request(sprite: &str, frame: char, rotation: u8) -> Request {
        Request::new(sprite, frame, rotation).expect("test requests are valid")
    }

    #[test]
    fn rotations_find_their_exact_mirrored_or_all_angles_lump() {
        let directory = rows(&session());
        let one = find_rotation(&directory, &request("TROO", 'A', 1)).expect("A1 is stored");
        assert_eq!((one.0.name, one.1), (name("TROOA1"), Serving::Direct));
        let eight = find_rotation(&directory, &request("TROO", 'A', 8)).expect("A2A8 mirrors it");
        assert_eq!(
            (eight.0.name, eight.1),
            (name("TROOA2A8"), Serving::Mirrored),
            "the pair name serves rotation 8"
        );
        let seven = find_rotation(&directory, &request("TROO", 'A', 7));
        assert!(seven.is_none(), "nothing serves rotation 7");
        let angles = find_rotation(&directory, &request("PLAY", 'A', 5)).expect("PLAYA0");
        assert_eq!(angles.0.name, name("PLAYA0"), "the all-angles image");
        let none = find_rotation(&directory, &request("PLAY", 'A', 0)).expect("PLAYA0 itself");
        assert_eq!(none.0.name, name("PLAYA0"), "the exact name is rotation 0");
    }

    /// An `IWAD` writes a frame's rotations `2`, `3` and `4` into its
    /// mirror-pair lumps only, so those rotations have no three-character
    /// lump to find: the pair name has to serve them as its own image.
    #[test]
    fn a_pair_name_serves_its_first_rotation_without_flipping() {
        let session = with_extra(vec![
            Lump::marker(name("S_START")),
            lump("TROOA3A7", stripe(4, 1, (1, 2))),
            Lump::marker(name("S_END")),
        ]);
        let directory = rows(&session);
        let two =
            find_rotation(&directory, &request("TROO", 'A', 2)).expect("A2A8 serves rotation 2");
        assert_eq!(
            (two.0.name, two.1),
            (name("TROOA2A8"), Serving::Direct),
            "the pair's own rotation is used as stored"
        );
        let three =
            find_rotation(&directory, &request("TROO", 'A', 3)).expect("A3A7 serves rotation 3");
        assert_eq!((three.0.name, three.1), (name("TROOA3A7"), Serving::Direct));
        let seven =
            find_rotation(&directory, &request("TROO", 'A', 7)).expect("A3A7 mirrors rotation 7");
        assert_eq!(
            (seven.0.name, seven.1),
            (name("TROOA3A7"), Serving::Mirrored)
        );
        let sprite = load_sprite(&session, "TROO", 'A', 2).expect("rotation 2 loads");
        assert_eq!(sprite.lump, name("TROOA2A8"), "served by the pair lump");
        assert!(!sprite.mirrored, "as stored, not flipped");
    }

    #[test]
    fn a_frame_letter_outside_a_lump_name_is_an_invalid_argument() {
        let session = session();
        let error = load_sprite(&session, "TROO", ' ', 1).expect_err("a space is no name");
        assert_eq!(code(&error), "invalid_argument", "bad frame code");
        let missing = load_sprite(&session, "BOGUS12", 'A', 1).expect_err("not four characters");
        assert_eq!(
            code(&missing),
            "invalid_argument",
            "a nine-character name is refused"
        );
    }

    #[test]
    fn sprites_decode_flip_and_report_their_lump() {
        let session = session();
        let direct = load_sprite(&session, "troo", 'a', 1).expect("TROOA1 serves rotation 1");
        assert!(
            !direct.mirrored && direct.lump == name("TROOA1"),
            "case-insensitive, stored as is"
        );
        assert_eq!(
            (direct.patch.left_offset, direct.patch.top_offset),
            (4, 0),
            "the header's offsets"
        );
        let flipped = load_sprite(&session, "TROO", 'A', 8).expect("the mirror pair answers");
        assert!(flipped.mirrored, "the pair is flipped");
        assert_eq!(flipped.lump, name("TROOA2A8"), "named for rotation 2");
        assert_eq!(
            (flipped.patch.left_offset, flipped.patch.top_offset),
            (3, 2),
            "left' = width - left = 4 - 1"
        );
        assert_eq!(
            flipped.patch.image.get(3, 0),
            Some((5, true)),
            "the only opaque column moved from 0 to the right edge"
        );
        let all = load_sprite(&session, "PLAY", 'A', 3).expect("the all-angles image");
        assert_eq!(all.lump, name("PLAYA0"), "rotation 0 serves every angle");
        assert!(!all.mirrored, "used as stored");
        let missing = load_sprite(&session, "PLAY", 'B', 1).expect_err("no frame B");
        assert_eq!(code(&missing), "not_found", "missing sprite code");
        assert!(
            missing.to_string().contains("list_sprites"),
            "the message names the listing: {missing}"
        );
    }

    #[test]
    fn mirroring_clamps_offsets_a_stored_width_cannot_reach() {
        let stored = Patch {
            left_offset: i16::MIN,
            top_offset: -1,
            image: IndexedImage::new(4, 1, vec![0_u8; 4], vec![false; 4])
                .expect("4x1 is a valid shape"),
        };
        let flipped = mirror_patch(&stored);
        assert_eq!(flipped.left_offset, i16::MAX, "4 - (-32768) saturates");
        assert_eq!(flipped.top_offset, -1, "the top offset is untouched");
        assert_eq!(clamp_offset(-1_i32), -1, "an offset inside i16 survives");
    }

    #[test]
    fn a_sprite_region_lump_that_is_no_picture_fails_as_a_format_error() {
        let session = with_extra(vec![
            Lump::marker(name("S_START")),
            lump("TROOA3", vec![b'?'; 60]),
            Lump::marker(name("S_END")),
        ]);
        let error = load_sprite(&session, "TROO", 'A', 3).expect_err("junk is not a patch");
        assert_eq!(code(&error), "format_error", "undecodable sprite code");
    }

    #[test]
    fn the_render_cache_decodes_each_lump_once_and_stores_it_unflipped() {
        let session = session();
        let defs = crate::defs::definitions_for(crate::wad::game::Game::Doom2)
            .expect("the embedded tables are valid");
        let imp = defs.thing(3001).expect("the embedded imp");
        let mut cache = HashMap::new();
        let first = load_sprite_cached(&session, imp, 8, &mut cache).expect("A2A8 flips");
        let second = load_sprite_cached(&session, imp, 8, &mut cache).expect("from cache");
        assert!(
            first.mirrored && second.mirrored,
            "rotation 8 serves flipped"
        );
        assert_eq!(
            (second.patch.left_offset, second.patch.image.get(3, 0)),
            (3, Some((5, true))),
            "the cached copy flips the same way the decoded one did"
        );
        let stored = cache.get(&name("TROOA2A8")).expect("the lump is cached");
        assert_eq!(
            (stored.left_offset, stored.image.get(0, 0)),
            (1, Some((5, true))),
            "the cache holds the patch as stored, not as served"
        );
        assert_eq!(cache.len(), 1, "one decode serves both calls");
    }

    #[test]
    fn decoders_read_the_doom_format_before_the_png_one() {
        let session = session();
        let bytes = doom_patch(2, 2, (1, 2), 0x22);
        let stored = decode_patch_bytes(&session, &bytes).expect("a Doom patch decodes");
        assert_eq!(
            (stored.left_offset, stored.top_offset),
            (1, 2),
            "header offsets"
        );
        let png_bytes = png(1, 2, [0x11, 0x11, 0x11], Some((-1_i32, 9_i32)));
        let as_png = decode_patch_bytes(&session, &png_bytes).expect("a PNG decodes");
        assert_eq!(as_png.left_offset, -1, "grAb offsets");
        assert_eq!(as_png.image.height, 2, "the PNG shape");
    }

    #[test]
    fn locate_and_lump_bytes_answer_for_any_winning_lump() {
        let session = session();
        let missing = lump_bytes(&session, name("PNAMES")).expect_err("the fixture has none");
        assert_eq!(code(&missing), "not_found", "missing lump code");
        let bytes = lump_bytes(&session, name("PLAYPAL")).expect("a global resolves");
        assert_eq!(bytes.len(), 768_usize.saturating_mul(14), "the whole lump");
        let directory = merged_rows(&session).expect("the fixture lists");
        assert!(
            locate(&directory, name("PLAYPAL"), &[Place::Anywhere]).is_some(),
            "an anywhere place matches a global"
        );
        assert!(
            locate(&directory, name("PLAYPAL"), &[Place::In(Namespace::Flats)]).is_none(),
            "a wrong region matches nothing"
        );
        assert!(
            locate(&directory, name("S_START"), &[Place::Anywhere]).is_none(),
            "a marker is never a picture"
        );
        let error = not_found("flat", "list_flats", name("NOPE"));
        assert_eq!(code(&error), "not_found", "the helper's code");
    }

    #[test]
    fn a_named_sprite_lump_is_served_exactly_as_stored() {
        let session = session();
        let stored = load_sprite_lump(&session, name("TROOA2A8")).expect("the name resolves");
        assert_eq!(
            (stored.left_offset, stored.top_offset),
            (1, 2),
            "not mirrored"
        );
        assert_eq!(
            stored.image.get(0, 0),
            Some((5, true)),
            "the opaque column stays at the left edge"
        );
        let missing = load_sprite_lump(&session, name("NOPEA1")).expect_err("no such lump");
        assert_eq!(code(&missing), "not_found", "missing name code");
        assert!(
            missing.to_string().contains("list_sprites"),
            "the message names the listing: {missing}"
        );
    }

    /// A `Doom`-layout `TEXTURE1`/`TEXTURE2` lump holding `textures`.
    fn texture_bytes(textures: Vec<TextureDef>) -> Vec<u8> {
        write_texture_lump(&TextureLump {
            layout: TextureLayout::Doom,
            textures,
        })
        .expect("test texture lumps serialize")
    }

    /// A texture definition named `text` placing `patches`.
    fn definition(text: &str, width: u16, height: u16, patches: Vec<PatchRef>) -> TextureDef {
        TextureDef {
            name: text.to_owned(),
            flags_raw: 0,
            width,
            height,
            patches,
        }
    }

    /// A patch placement at `x`/`y`.
    fn place(patch: u16, x: i16, y: i16) -> PatchRef {
        PatchRef { patch, x, y }
    }

    /// A `PNAMES` lump naming `texts`, in index order.
    fn pnames_lump(texts: &[&str]) -> Lump {
        let names: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
        lump(
            "PNAMES",
            write_pnames(&names).expect("test patch names are valid"),
        )
    }

    #[test]
    fn textures_composite_definitions_and_report_missing_patches() {
        let texture1 = texture_bytes(vec![definition(
            "TWALL",
            8,
            4,
            vec![place(0, 0, 0), place(1, 4, 0)],
        )]);
        let session = with_extra(vec![
            pnames_lump(&["WALLP1", "GONE"]),
            lump("TEXTURE1", texture1),
        ]);
        let composite = load_texture(&session, name("TWALL")).expect("TWALL composes");
        assert_eq!(
            (composite.image.width, composite.image.height),
            (8, 4),
            "the record's size"
        );
        assert_eq!(
            composite.image.get(3, 3),
            Some((0x11, true)),
            "the present patch drew"
        );
        assert_eq!(
            composite.image.get(5, 1),
            Some((0, false)),
            "the missing patch leaves a hole"
        );
        assert_eq!(composite.missing, ["GONE"], "the hole is named");
        assert_eq!(
            composite.problems,
            ["texture TWALL is missing patch GONE".to_owned()],
            "the problem names the texture"
        );
    }

    #[test]
    fn texture2_definitions_win_over_their_texture1_namesakes() {
        let step = doom_patch(2, 2, (0, 0), 0x22);
        let texture1 = texture_bytes(vec![definition("AWIN", 4, 4, vec![place(0, 0, 0)])]);
        let texture2 = texture_bytes(vec![definition("AWIN", 2, 2, vec![place(1, 0, 0)])]);
        let session = with_extra(vec![
            pnames_lump(&["WALLP1", "STEPP1"]),
            lump("STEPP1", step),
            lump("TEXTURE1", texture1),
            lump("TEXTURE2", texture2),
        ]);
        let composite = load_texture(&session, name("AWIN")).expect("AWIN composes");
        assert_eq!(
            (composite.image.width, composite.image.height),
            (2, 2),
            "the TEXTURE2 record's size"
        );
        assert_eq!(
            composite.image.get(0, 0),
            Some((0x22, true)),
            "the TEXTURE2 component drew"
        );
        assert!(
            composite.missing.is_empty() && composite.problems.is_empty(),
            "a full composite reports nothing"
        );
    }

    #[test]
    fn without_a_pnames_lump_indices_stay_unnamed_and_missing() {
        let texture1 = texture_bytes(vec![definition("TPTR", 2, 2, vec![place(0, 0, 0)])]);
        let session = with_extra(vec![lump("TEXTURE1", texture1)]);
        let composite = load_texture(&session, name("TPTR")).expect("TPTR composes");
        assert_eq!(
            composite.missing,
            ["PNAMES index 0"],
            "the raw index names the hole"
        );
        assert_eq!(
            composite.image.get(1, 1),
            Some((0, false)),
            "the whole texture is a hole"
        );
    }

    #[test]
    fn a_tx_region_patch_serves_a_texture_without_definitions() {
        let session = with_extra(vec![
            Lump::marker(name("TX_START")),
            lump("TXONE", doom_patch(3, 5, (0, 0), 0x33)),
            Lump::marker(name("TX_END")),
        ]);
        let composite = load_texture(&session, name("TXONE")).expect("the TX patch answers");
        assert_eq!(
            (composite.image.width, composite.image.height),
            (3, 5),
            "stored as a patch"
        );
        assert_eq!(composite.image.get(0, 0), Some((0x33, true)));
        assert!(
            composite.missing.is_empty() && composite.problems.is_empty(),
            "a plain patch has nothing missing"
        );
    }

    #[test]
    fn unknown_texture_names_are_not_found_and_broken_lumps_are_format_errors() {
        let session = session();
        let missing = load_texture(&session, name("NOPE")).expect_err("no such texture");
        assert_eq!(code(&missing), "not_found", "missing texture code");
        assert!(
            missing.to_string().contains("list_textures"),
            "the message names the listing: {missing}"
        );
        let broken = with_extra(vec![
            pnames_lump(&["WALLP1"]),
            lump("TEXTURE1", vec![b'?'; 40]),
        ]);
        let error = load_texture(&broken, name("ANY")).expect_err("junk is no texture lump");
        assert_eq!(code(&error), "format_error", "undecodable lump code");
    }

    #[test]
    fn a_component_that_is_no_picture_stays_a_reported_hole() {
        let texture1 = texture_bytes(vec![definition("AWRY", 2, 2, vec![place(0, 0, 0)])]);
        let session = with_extra(vec![
            pnames_lump(&["JUNK"]),
            lump("TEXTURE1", texture1),
            lump("JUNK", vec![b'?'; 40]),
        ]);
        let composite = load_texture(&session, name("AWRY")).expect("AWRY composes");
        assert_eq!(
            composite.missing,
            ["JUNK"],
            "the undecodable patch is named"
        );
        assert_eq!(
            composite.problems,
            ["texture AWRY is missing patch JUNK".to_owned()],
            "the problem names the texture"
        );
        assert_eq!(
            composite.image.get(1, 1),
            Some((0, false)),
            "its place stays a hole"
        );
    }
}
