//! `set_palette`'s side of the palette group: one source in, a new
//! `PLAYPAL` out.
//!
//! [`set_palette_run`] resolves the request's single source - inline
//! [`SourceArg::Colors`] hex colours or a fetched [`SourceArg::Uri`] read by
//! [`interpret_bytes`] - into a [`Request`], and [`commit_palette`] applies
//! it to a copy of the merged `PLAYPAL` (rebuilding the vanilla tints with
//! [`rebuild_tints`] and regenerating `COLORMAP` when asked) inside exactly
//! one [`crate::session::Session::edit`] step, so one undo entry stands for
//! the whole call. The shared reader, the swatch builder and the constants
//! the two halves share live in the root module.

use alloc::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::gfx::codec::decode_image;
use crate::gfx::colormap;
use crate::gfx::image::RgbaImage;
use crate::gfx::palette::Palette;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::textures::put_global;
use crate::wad::name::LumpName;

use super::{
    COLORMAP_RAW, FIRST_TINT, GOLD_LAST, GOLD_OFFSET, ONE, PALETTE_BYTES, PALETTE_COLORS,
    PLAYPAL_RAW, RED_LAST, VANILLA_PALETTES, ZERO, find_global,
};

/// Arguments for [`Cyberdemon::set_palette`]: exactly one of `colors` and
/// `source` is required.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct SetPaletteParams {
    /// The new palette as exactly 256 `"#RRGGBB"` (or `RRGGBB`) colour
    /// strings. Give this or `source`, never both.
    #[serde(default)]
    pub(super) colors: Option<Vec<String>>,
    /// Absolute path, `file://` URI, `http(s)://` URL or uploaded asset
    /// URL of the palette to load: either an image with exactly 256
    /// pixels (read row-major, alpha ignored - e.g. a 16x16 PNG) or raw
    /// palette bytes whose length is a multiple of 768, which then
    /// replace the whole `PLAYPAL`. Give this or `colors`, never both.
    #[serde(default)]
    pub(super) source: Option<String>,
    /// Which `PLAYPAL` entry the 256-colour source replaces (default 0:
    /// the normal palette); passing the current palette count appends a
    /// new one. A raw multi-palette `source` replaces every entry and
    /// ignores this.
    #[serde(default)]
    pub(super) index: Option<usize>,
    /// Rebuild palettes 1..=13 from palette 0, appending what is missing
    /// to reach the vanilla 14 (default false). Approximates the vanilla
    /// damage, pickup and suit tints.
    #[serde(default)]
    pub(super) regenerate_tints: Option<bool>,
    /// Rebuild `COLORMAP` (34 tables of 256 indices) from palette 0 with
    /// nearest-colour matching (default false).
    #[serde(default)]
    pub(super) regenerate_colormap: Option<bool>,
}

/// Structured result of [`Cyberdemon::set_palette`].
#[derive(Debug, Serialize)]
pub(super) struct SetPaletteReport {
    /// Palettes the working `PLAYPAL` holds after the edit.
    pub(super) palettes: usize,
    /// Palette entries the call wrote, ascending.
    pub(super) replaced: Vec<usize>,
    /// Whether `COLORMAP` was rebuilt.
    pub(super) colormap_regenerated: bool,
}

/// Where a `set_palette` call's new data comes from.
#[derive(Debug)]
pub(super) enum Payload {
    /// One palette, replacing or appending at the request's `index`.
    Colors(Box<Palette>),
    /// A whole-`PLAYPAL` replacement read from raw bytes.
    Playpal(Vec<u8>),
}

/// The single source argument one `set_palette` call carries.
#[derive(Debug)]
pub(super) enum SourceArg<'arg> {
    /// 256 hex colours, inline.
    Colors(&'arg [String]),
    /// A path/URI to fetch.
    Uri(&'arg str),
}

/// A validated `set_palette` request, ready to commit against a session.
pub(super) struct Request {
    /// What to write.
    pub(super) payload: Payload,
    /// Entry the [`Payload::Colors`] variant replaces or appends.
    pub(super) index: usize,
    /// Rebuild the vanilla tints from palette 0.
    pub(super) tints: bool,
    /// Rebuild `COLORMAP` from palette 0.
    pub(super) colormap: bool,
}

/// The refusal for a call that gives none or more than one source.
fn exactly_one_source() -> CyberError {
    CyberError::InvalidArgument(
        "set_palette takes exactly one source: pass colors (256 #RRGGBB strings) or source (an \
         image with 256 pixels, or raw palette bytes in multiples of 768) - and only one of them"
            .to_owned(),
    )
}

/// The single source the arguments carry, refusing none and both.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless exactly one of `colors` and
/// `source` is present.
pub(super) fn pick_source(args: &SetPaletteParams) -> Result<SourceArg<'_>, CyberError> {
    let mut given = [
        args.colors.as_deref().map(SourceArg::Colors),
        args.source.as_deref().map(SourceArg::Uri),
    ]
    .into_iter()
    .flatten();
    let Some(first) = given.next() else {
        return Err(exactly_one_source());
    };
    if given.next().is_some() {
        return Err(exactly_one_source());
    }
    Ok(first)
}

/// Turn fetched `source` bytes into a payload: an image first (its pixels
/// must be exactly 256), raw palette bytes otherwise (a non-empty
/// multiple of 768).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an image with the wrong pixel
/// count, or bytes that are neither a decodable image nor whole palettes.
pub(super) fn interpret_bytes(bytes: &[u8]) -> Result<Payload, CyberError> {
    if let Ok(image) = decode_image(bytes) {
        return palette_from_image(&image).map(|palette| Payload::Colors(Box::new(palette)));
    }
    if !bytes.is_empty() && bytes.len().checked_rem(PALETTE_BYTES) == Some(0) {
        return Ok(Payload::Playpal(bytes.to_vec()));
    }
    Err(CyberError::InvalidArgument(format!(
        "the source is neither a decodable image with exactly {PALETTE_COLORS} pixels nor raw \
         palette bytes (a non-empty multiple of {PALETTE_BYTES}): got {} bytes",
        bytes.len()
    )))
}

/// The palette of an image with exactly 256 pixels, read row-major with
/// the alpha channel dropped.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless the image holds exactly 256
/// pixels.
pub(super) fn palette_from_image(image: &RgbaImage) -> Result<Palette, CyberError> {
    let pixels = image.pixels.chunks_exact(BYTES_PER_PIXEL);
    if pixels.len() != PALETTE_COLORS {
        return Err(CyberError::InvalidArgument(format!(
            "the source image has {} pixels; a palette image needs exactly {PALETTE_COLORS} \
             (e.g. a 16x16 PNG), read in row-major order",
            pixels.len()
        )));
    }
    let mut colors = [[0_u8; 3]; PALETTE_COLORS];
    for (color, pixel) in colors.iter_mut().zip(pixels.map(to_pixel)) {
        let [red, green, blue, _alpha] = pixel;
        *color = [red, green, blue];
    }
    Ok(Palette::from(colors))
}

/// The vanilla tint of palette `index` (1..=13): target colour and weight
/// as a `num`/`den` pair.
pub(super) fn tint_of(index: i64) -> ([u8; 3], i64, i64) {
    if index <= RED_LAST {
        ([255_u8, 0_u8, 0_u8], index, 9)
    } else if index <= GOLD_LAST {
        (
            [215_u8, 186_u8, 69_u8],
            index.checked_sub(GOLD_OFFSET).unwrap_or(ONE),
            8,
        )
    } else {
        ([0_u8, 255_u8, 0_u8], ONE, 8)
    }
}

/// `current + (target - current) * num / den`, rounded to nearest.
pub(super) fn tint_channel(current: u8, target: u8, num: i64, den: i64) -> u8 {
    let delta = i64::from(target)
        .checked_sub(i64::from(current))
        .unwrap_or(ZERO);
    let shifted = mul_div_round(delta, num, den).unwrap_or(ZERO);
    i64::from(current)
        .checked_add(shifted)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(target)
}

/// `base` tinted to `([target], num, den)`, as one 768-byte palette.
fn tint_palette(base: &Palette, tint: ([u8; 3], i64, i64)) -> Vec<u8> {
    let ([target_red, target_green, target_blue], num, den) = tint;
    let mut out = Vec::with_capacity(PALETTE_BYTES);
    for &[red, green, blue] in base.colors() {
        out.push(tint_channel(red, target_red, num, den));
        out.push(tint_channel(green, target_green, num, den));
        out.push(tint_channel(blue, target_blue, num, den));
    }
    out
}

/// Overwrite the `index`th 768-byte entry of `data`; `index` equal to the
/// entry count appends.
///
/// # Errors
///
/// [`CyberError::Internal`] when the arithmetic lands outside the data
/// (which the callers' checks rule out).
fn write_entry(data: &mut Vec<u8>, index: usize, bytes: &[u8]) -> Result<(), CyberError> {
    let lost = || {
        CyberError::Internal(format!(
            "palette entry {index} does not line up with {PALETTE_BYTES}-byte palettes"
        ))
    };
    let start = index.checked_mul(PALETTE_BYTES).ok_or_else(lost)?;
    if bytes.len() != PALETTE_BYTES {
        return Err(lost());
    }
    if index == data.chunks_exact(PALETTE_BYTES).count() {
        data.extend_from_slice(bytes);
        return Ok(());
    }
    let (_head, tail) = data.split_at_mut_checked(start).ok_or_else(lost)?;
    tail.get_mut(..PALETTE_BYTES)
        .ok_or_else(lost)?
        .copy_from_slice(bytes);
    Ok(())
}

/// Grow `data` to the vanilla 14 palettes and rebuild entries 1..=13 from
/// palette 0, recording the rebuilt entries in `replaced`.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `data` holds no palette 0 to tint (which
/// the payload checks rule out).
pub(super) fn rebuild_tints(
    data: &mut Vec<u8>,
    replaced: &mut BTreeSet<usize>,
) -> Result<(), CyberError> {
    let base = Palette::from_playpal(data, 0)?;
    let filler = base.to_bytes();
    for _missing in data.chunks_exact(PALETTE_BYTES).count()..VANILLA_PALETTES {
        data.extend_from_slice(&filler);
    }
    for index in FIRST_TINT..VANILLA_PALETTES {
        let level = i64::try_from(index).unwrap_or(ONE);
        let bytes = tint_palette(&base, tint_of(level));
        write_entry(data, index, &bytes)?;
        replaced.insert(index);
    }
    Ok(())
}

/// The regenerated `COLORMAP` bytes for `PLAYPAL` data: its palette 0
/// expanded by the shared [`colormap`] generator.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `data` holds no palette 0 (which the
/// payload checks rule out).
fn regenerate_colormap(data: &[u8]) -> Result<Vec<u8>, CyberError> {
    Ok(colormap::colormap_bytes(&Palette::from_playpal(data, 0)?))
}

/// The merged view's winning `PLAYPAL` bytes, empty when no loaded
/// archive holds one (a `set_palette` call may then create it).
fn current_playpal(session: &Session) -> Vec<u8> {
    find_global(session, PLAYPAL_RAW).map_or_else(Vec::new, |entry| entry.lump.data.to_vec())
}

/// Write the payload onto a copy of the base `PLAYPAL`, recording which
/// entries changed in `replaced`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a colour payload's `index` is past
/// the end (only `index == count` may append); [`CyberError::Internal`]
/// from [`write_entry`]'s arithmetic guard.
pub(super) fn apply_payload(
    base: &[u8],
    payload: Payload,
    index: usize,
    replaced: &mut BTreeSet<usize>,
) -> Result<Vec<u8>, CyberError> {
    match payload {
        Payload::Playpal(bytes) => {
            replaced.extend(0..bytes.chunks_exact(PALETTE_BYTES).count());
            Ok(bytes)
        }
        Payload::Colors(palette) => {
            let count = base.chunks_exact(PALETTE_BYTES).count();
            if index > count {
                return Err(CyberError::InvalidArgument(format!(
                    "palette index {index} is past the end of a PLAYPAL with {count} palettes: \
                     pass an index within 0..={count} (the count appends) or omit it for the \
                     normal palette"
                )));
            }
            let mut data = base.to_vec();
            write_entry(&mut data, index, &palette.to_bytes())?;
            replaced.insert(index);
            Ok(data)
        }
    }
}

/// Apply one request to the session as one undoable `set_palette` edit:
/// copy-on-write the merged `PLAYPAL` into the working archive when that
/// archive has none, then write the new bytes (and a regenerated
/// `COLORMAP`).
///
/// # Errors
///
/// [`apply_payload`]'s and [`build_colormap`]'s errors; the edit itself
/// always commits once they pass.
pub(super) fn commit_palette(
    session: &mut Session,
    request: Request,
) -> Result<SetPaletteReport, CyberError> {
    let Request {
        payload,
        index,
        tints,
        colormap,
    } = request;
    let base = current_playpal(session);
    let mut replaced = BTreeSet::new();
    let mut data = apply_payload(&base, payload, index, &mut replaced)?;
    if tints {
        rebuild_tints(&mut data, &mut replaced)?;
    }
    let maps = colormap.then(|| regenerate_colormap(&data)).transpose()?;
    let palettes = data.chunks_exact(PALETTE_BYTES).count();
    session.edit("set_palette", |wad| {
        put_global(wad, LumpName::from_raw(PLAYPAL_RAW), data.clone())?;
        if let Some(bytes) = maps.as_ref() {
            put_global(wad, LumpName::from_raw(COLORMAP_RAW), bytes.clone())?;
        }
        Ok(())
    })?;
    Ok(SetPaletteReport {
        palettes,
        replaced: replaced.into_iter().collect(),
        colormap_regenerated: maps.is_some(),
    })
}

/// `set_palette`'s work: resolve exactly one source, then commit it.
///
/// # Errors
///
/// [`pick_source`], [`Palette::from_hex_list`], [`interpret_bytes`] and
/// [`AppState::fetch_source`] errors pass through, as do
/// [`commit_palette`]'s.
pub(super) async fn set_palette_run(
    state: &AppState,
    args: &SetPaletteParams,
) -> Result<SetPaletteReport, CyberError> {
    let payload = match pick_source(args)? {
        SourceArg::Colors(list) => Payload::Colors(Box::new(Palette::from_hex_list(list)?)),
        SourceArg::Uri(raw) => {
            let fetched = state.fetch_source(raw).await?;
            interpret_bytes(&fetched.bytes)?
        }
    };
    let request = Request {
        payload,
        index: args.index.unwrap_or(0_usize),
        tints: args.regenerate_tints.unwrap_or(false),
        colormap: args.regenerate_colormap.unwrap_or(false),
    };
    state.with_session(|session| commit_palette(session, request))
}
