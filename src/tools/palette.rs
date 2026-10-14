//! Palette tools: reading the session's `PLAYPAL` and writing it back,
//! with optional regeneration of the vanilla tints and the `COLORMAP`.
//!
//! [`load_palette`] is the shared helper every graphics tool reads the
//! current colours through: the last (winning) `PLAYPAL` of the merged
//! view, one 768-byte entry at a time. `get_palette` returns a palette as
//! hex plus a published `PNG` swatch; `set_palette` replaces palettes from
//! inline colours, a fetched image or raw palette bytes - copy-on-write
//! into the working archive as one undoable edit - and can rebuild the 13
//! vanilla tints from palette 0 and regenerate `COLORMAP` by
//! nearest-colour matching. The tint and `COLORMAP` recipes approximate
//! the vanilla tables; they are formulaic, integer-only and documented in
//! the tool descriptions.

use alloc::collections::BTreeSet;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::image::RgbaImage;
use crate::gfx::palette::Palette;
use crate::gfx::quantize::nearest_index;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::server::Cyberdemon;
use crate::session::{MergedLump, Session};
use crate::state::AppState;
use crate::tools::textures::put_global;
use crate::tools::{fail, ok};
use crate::wad::name::LumpName;

/// Raw directory bytes of the palette lump.
const PLAYPAL_RAW: [u8; 8] = *b"PLAYPAL\0";
/// Raw directory bytes of the colour translation lump.
const COLORMAP_RAW: [u8; 8] = *b"COLORMAP";
/// Bytes one palette occupies inside `PLAYPAL` (256 RGB triples).
const PALETTE_BYTES: usize = 768;
/// Colours in one palette.
const PALETTE_COLORS: usize = 256;
/// Vanilla `PLAYPAL` palette count: normal, red damage, gold pickup,
/// green suit.
const VANILLA_PALETTES: usize = 14;
/// Brightness tables at the head of a `COLORMAP` (full to darkest).
const COLORMAP_LEVELS: usize = 32;
/// Tables in one `COLORMAP`: 32 levels plus invulnerability and all-black.
const COLORMAP_TABLES: usize = 34;
/// Bytes (indices) in one `COLORMAP` table.
const COLORMAP_TABLE_BYTES: usize = 256;
/// Denominator of the brightness ramp: map `k` scales by `(32 - k)/32`.
const LIGHT_SCALE: i64 = 32;
/// First tint index (index 0 is the untinted normal palette).
const FIRST_TINT: usize = 1;
/// Last red (damage) tint index.
const RED_LAST: i64 = 8;
/// Last gold (pickup) tint index.
const GOLD_LAST: i64 = 12;
/// Offset from a gold tint index to its weight numerator: `(i - 8) / 8`.
const GOLD_OFFSET: i64 = 8;
/// Pixels on one swatch cell edge.
const SWATCH_CELL: u32 = 16;
/// Swatch cells per edge: the 256 colours in a 16x16 grid.
const SWATCH_CELLS: u32 = 16;
/// Swatch pixels per edge (`SWATCH_CELL * SWATCH_CELLS`).
const SWATCH_SIDE: u32 = SWATCH_CELL.saturating_mul(SWATCH_CELLS);
/// Alpha byte of every swatch pixel.
const OPAQUE: u8 = 255;
/// Channel value `0` as an `i64`, for checked arithmetic.
const ZERO: i64 = 0;
/// Channel value `255` as an `i64`, for checked arithmetic.
const FULL: i64 = 255;
/// Weight numerator/denominator one, for checked arithmetic.
const ONE: i64 = 1;

/// Arguments for [`Cyberdemon::get_palette`].
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct GetPaletteParams {
    /// Which `PLAYPAL` entry to read (default 0: the normal palette;
    /// Doom's 1..=8 are the red damage tints, 9..=12 the gold pickup
    /// tint and 13 the green radiation-suit tint).
    #[serde(default)]
    index: Option<usize>,
}

/// Structured result of [`Cyberdemon::get_palette`].
#[derive(Serialize)]
struct GetPaletteReport {
    /// Palettes the `PLAYPAL` lump holds.
    palettes: usize,
    /// The palette this call read.
    index: usize,
    /// The 256 colours as `#RRGGBB` strings, uppercase hex.
    colors: Vec<String>,
    /// The published 256x256 `PNG` swatch: a 16x16 grid of 16-pixel cells.
    swatch: AssetRef,
    /// Whether the session has a `COLORMAP` lump.
    has_colormap: bool,
}

/// A palette and its lump's facts, read under the session lock.
struct LoadedPalette {
    /// Palettes the `PLAYPAL` lump holds.
    palettes: usize,
    /// The palette that was asked for.
    palette: Palette,
    /// Whether the session has a `COLORMAP` lump.
    has_colormap: bool,
}

/// The `index`th palette of the session's `PLAYPAL`.
///
/// Reads the last (winning) `PLAYPAL` of the merged view. This is the one
/// entry point every graphics tool loads current colours through.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds a `PLAYPAL` or
/// `index` names a palette the lump does not contain.
#[inline]
pub(in crate::tools) fn load_palette(
    session: &Session,
    index: usize,
) -> Result<Palette, CyberError> {
    Palette::from_playpal(&require_playpal(session)?, index)
}

/// The merged view's winning `PLAYPAL` bytes.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming `open_iwad` when no loaded archive
/// holds a `PLAYPAL` lump.
fn require_playpal(session: &Session) -> Result<Vec<u8>, CyberError> {
    let entry = find_global(session, PLAYPAL_RAW).ok_or_else(|| {
        CyberError::NotFound("no PLAYPAL in the session; open an IWAD with open_iwad".to_owned())
    })?;
    Ok(entry.lump.data.to_vec())
}

/// The merged view's last lump with the raw directory name `raw`.
fn find_global(session: &Session, raw: [u8; 8]) -> Option<MergedLump> {
    session.find(LumpName::from_raw(raw), None)
}

/// The 256x256 preview image: a 16x16 grid of 16-pixel cells in palette
/// order, every pixel opaque.
fn swatch_image(palette: &Palette) -> Result<RgbaImage, CyberError> {
    let side = usize::try_from(SWATCH_SIDE).unwrap_or(PALETTE_COLORS);
    let mut pixels: Vec<u8> = Vec::with_capacity(side.saturating_mul(side).saturating_mul(4));
    for row in 0..SWATCH_SIDE {
        let cell_row = row.checked_div(SWATCH_CELL).unwrap_or(0);
        for column in 0..SWATCH_SIDE {
            let cell = cell_row
                .checked_mul(SWATCH_CELLS)
                .and_then(|line| line.checked_add(column.checked_div(SWATCH_CELL).unwrap_or(0)));
            let color = cell
                .and_then(|number| usize::try_from(number).ok())
                .and_then(|number| palette.color(number))
                .unwrap_or([0_u8; 3]);
            let [red, green, blue] = color;
            pixels.extend_from_slice(&[red, green, blue, OPAQUE]);
        }
    }
    RgbaImage::new(SWATCH_SIDE, SWATCH_SIDE, pixels)
}

/// `get_palette`'s work: read under the session lock, then publish the
/// swatch.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a missing `PLAYPAL` or an out-of-range
/// `index`; the encoder's and publisher's errors for the swatch.
fn get_palette_run(state: &AppState, index: usize) -> Result<GetPaletteReport, CyberError> {
    let loaded = state.with_session(|session| {
        let palette = load_palette(session, index)?;
        Ok(LoadedPalette {
            palettes: require_playpal(session)?
                .chunks_exact(PALETTE_BYTES)
                .count(),
            palette,
            has_colormap: find_global(session, COLORMAP_RAW).is_some(),
        })
    })?;
    let png = encode_png(&swatch_image(&loaded.palette)?, None)?;
    let swatch = state.publish(&format!("PLAYPAL_{index}.png"), "image/png", png)?;
    Ok(GetPaletteReport {
        palettes: loaded.palettes,
        index,
        colors: loaded.palette.to_hex_list(),
        swatch,
        has_colormap: loaded.has_colormap,
    })
}

/// Arguments for [`Cyberdemon::set_palette`]: exactly one of `colors` and
/// `source` is required.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct SetPaletteParams {
    /// The new palette as exactly 256 `"#RRGGBB"` (or `RRGGBB`) colour
    /// strings. Give this or `source`, never both.
    #[serde(default)]
    colors: Option<Vec<String>>,
    /// Absolute path, `file://` URI, `http(s)://` URL or uploaded asset
    /// URL of the palette to load: either an image with exactly 256
    /// pixels (read row-major, alpha ignored - e.g. a 16x16 PNG) or raw
    /// palette bytes whose length is a multiple of 768, which then
    /// replace the whole `PLAYPAL`. Give this or `colors`, never both.
    #[serde(default)]
    source: Option<String>,
    /// Which `PLAYPAL` entry the 256-colour source replaces (default 0:
    /// the normal palette); passing the current palette count appends a
    /// new one. A raw multi-palette `source` replaces every entry and
    /// ignores this.
    #[serde(default)]
    index: Option<usize>,
    /// Rebuild palettes 1..=13 from palette 0, appending what is missing
    /// to reach the vanilla 14 (default false). Approximates the vanilla
    /// damage, pickup and suit tints.
    #[serde(default)]
    regenerate_tints: Option<bool>,
    /// Rebuild `COLORMAP` (34 tables of 256 indices) from palette 0 with
    /// nearest-colour matching (default false).
    #[serde(default)]
    regenerate_colormap: Option<bool>,
}

/// Structured result of [`Cyberdemon::set_palette`].
#[derive(Debug, Serialize)]
struct SetPaletteReport {
    /// Palettes the working `PLAYPAL` holds after the edit.
    palettes: usize,
    /// Palette entries the call wrote, ascending.
    replaced: Vec<usize>,
    /// Whether `COLORMAP` was rebuilt.
    colormap_regenerated: bool,
}

/// Where a `set_palette` call's new data comes from.
#[derive(Debug)]
enum Payload {
    /// One palette, replacing or appending at the request's `index`.
    Colors(Box<Palette>),
    /// A whole-`PLAYPAL` replacement read from raw bytes.
    Playpal(Vec<u8>),
}

/// The single source argument one `set_palette` call carries.
#[derive(Debug)]
enum SourceArg<'arg> {
    /// 256 hex colours, inline.
    Colors(&'arg [String]),
    /// A path/URI to fetch.
    Uri(&'arg str),
}

/// A validated `set_palette` request, ready to commit against a session.
struct Request {
    /// What to write.
    payload: Payload,
    /// Entry the [`Payload::Colors`] variant replaces or appends.
    index: usize,
    /// Rebuild the vanilla tints from palette 0.
    tints: bool,
    /// Rebuild `COLORMAP` from palette 0.
    colormap: bool,
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
fn pick_source(args: &SetPaletteParams) -> Result<SourceArg<'_>, CyberError> {
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
fn interpret_bytes(bytes: &[u8]) -> Result<Payload, CyberError> {
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
fn palette_from_image(image: &RgbaImage) -> Result<Palette, CyberError> {
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
fn tint_of(index: i64) -> ([u8; 3], i64, i64) {
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
fn tint_channel(current: u8, target: u8, num: i64, den: i64) -> u8 {
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
fn rebuild_tints(data: &mut Vec<u8>, replaced: &mut BTreeSet<usize>) -> Result<(), CyberError> {
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

/// The channel value scaled by `num`/`den`, rounded to nearest.
fn scale_channel(value: u8, num: i64, den: i64) -> u8 {
    mul_div_round(i64::from(value), num, den)
        .and_then(|scaled| u8::try_from(scaled).ok())
        .unwrap_or(value)
}

/// The colour scaled by `num`/`den`.
fn scale_color([red, green, blue]: [u8; 3], num: i64, den: i64) -> [u8; 3] {
    [
        scale_channel(red, num, den),
        scale_channel(green, num, den),
        scale_channel(blue, num, den),
    ]
}

/// The greyscale value `(299r + 587g + 114b) / 1000`, truncated.
fn luminance([red, green, blue]: [u8; 3]) -> u8 {
    i64::from(red)
        .checked_mul(299)
        .unwrap_or(ZERO)
        .checked_add(i64::from(green).checked_mul(587).unwrap_or(ZERO))
        .unwrap_or(ZERO)
        .checked_add(i64::from(blue).checked_mul(114).unwrap_or(ZERO))
        .unwrap_or(ZERO)
        .checked_div(1000)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(0)
}

/// The invulnerability-table entry of one palette colour: its inverted
/// greyscale as a nearest-match index.
fn inverse_entry(color: [u8; 3], palette: &Palette) -> u8 {
    let shade = i64::from(luminance(color));
    let inverse = FULL
        .checked_sub(shade)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(0);
    nearest_index([inverse, inverse, inverse], palette, &[])
}

/// The 34-table `COLORMAP` (8704 bytes) derived from palette 0 of `data`:
/// tables 0..=31 scale colours by `(32 - k)/32` to the nearest palette
/// entry, table 32 is the inverted greyscale (invulnerability) and table
/// 33 maps everything to the nearest colour to black.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `data` holds no palette 0 (which the
/// payload checks rule out).
fn build_colormap(data: &[u8]) -> Result<Vec<u8>, CyberError> {
    let palette = Palette::from_playpal(data, 0)?;
    let capacity = COLORMAP_TABLES.saturating_mul(COLORMAP_TABLE_BYTES);
    let mut tables = Vec::with_capacity(capacity);
    for level in 0..COLORMAP_LEVELS {
        let num = LIGHT_SCALE
            .checked_sub(i64::try_from(level).unwrap_or(ZERO))
            .unwrap_or(ZERO);
        for color in palette.colors() {
            tables.push(nearest_index(
                scale_color(*color, num, LIGHT_SCALE),
                &palette,
                &[],
            ));
        }
    }
    for color in palette.colors() {
        tables.push(inverse_entry(*color, &palette));
    }
    let black = nearest_index([0_u8; 3], &palette, &[]);
    for _entry in 0..COLORMAP_TABLE_BYTES {
        tables.push(black);
    }
    Ok(tables)
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
fn apply_payload(
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
fn commit_palette(session: &mut Session, request: Request) -> Result<SetPaletteReport, CyberError> {
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
    let maps = colormap.then(|| build_colormap(&data)).transpose()?;
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
async fn set_palette_run(
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = palette_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reads one `PLAYPAL` palette of the session.
    #[tool(
        description = "Reads one PLAYPAL palette of the merged view (index, default 0: 0 is the normal palette, 1..=8 the red damage tints, 9..=12 the gold pickup tints and 13 the green radiation-suit tint; other entries exist in some ports). Returns {palettes: entries the PLAYPAL holds, index, colors: 256 \"#RRGGBB\" strings, swatch: a published 256x256 PNG preview drawn as a 16x16 grid of 16-pixel cells, has_colormap: whether the session also has a COLORMAP lump}. not_found when no loaded archive holds a PLAYPAL (open an IWAD first) or when index is past the last entry. Change the palette with set_palette."
    )]
    fn get_palette(
        &self,
        Parameters(args): Parameters<GetPaletteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match get_palette_run(self.state(), args.index.unwrap_or(0_usize)) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Replaces `PLAYPAL` palettes, optionally rebuilding tints and `COLORMAP`.
    #[tool(
        description = "Writes palettes into the working archive's PLAYPAL as ONE undoable edit labelled `set_palette` (copy-on-write: the merged PLAYPAL is brought in first when the working archive has none; save_wad writes the result; read-only layers are never touched). Exactly one source: colors (exactly 256 \"#RRGGBB\" strings - bad hex or the wrong count is invalid_argument) or source (an absolute path, file:// URI, http(s):// URL or uploaded asset URL of either an image with exactly 256 pixels, read row-major with alpha dropped - e.g. a 16x16 PNG - or raw palette bytes whose length is a non-empty multiple of 768, which replace the WHOLE PLAYPAL). index (default 0) says which entry a 256-colour payload replaces; index equal to the current palette count appends a palette, a larger index is invalid_argument. regenerate_tints=true rebuilds palettes 1..=13 from palette 0, appending what is missing to reach the vanilla 14: red tints 1..=8 tint towards (255,0,0) with weight i/9, gold 9..=12 towards (215,186,69) with weight (i-8)/8 and green 13 towards (0,255,0) with weight 1/8, per channel c' = c + (t - c)*num/den rounded - an integer approximation of the vanilla tables. regenerate_colormap=true rebuilds COLORMAP (34 tables x 256) from palette 0 by nearest-colour matching: map k scales colours by (32-k)/32, map 32 is the inverted greyscale 255 - (299r+587g+114b)/1000 (the invulnerability map) and map 33 maps everything to the nearest colour to black. Returns {palettes: entries after the edit, replaced: ascending indices the call wrote (a raw source writes all of them; the tints add 1..=13), colormap_regenerated}."
    )]
    async fn set_palette(
        &self,
        Parameters(args): Parameters<SetPaletteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match set_palette_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::{Layer, LayerRole};
    use crate::wad::archive::{Lump, Wad, WadKind};

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// `palettes` copies of the fixture palette (colour `i` is
    /// `(i, 255 - i, i)`) as raw `PLAYPAL` bytes.
    fn fixture_bytes(palettes: usize) -> Vec<u8> {
        let mut first = Vec::new();
        for index in 0_u8..=255 {
            first.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
        }
        first.repeat(palettes)
    }

    /// The fixture palette.
    fn fixture_palette() -> Palette {
        Palette::from_playpal(&fixture_bytes(1), 0).expect("one palette was built")
    }

    /// A session whose `IWAD` layer holds the fixture `PLAYPAL` (and a
    /// zeroed `COLORMAP` when `with_colormap` is set).
    fn session_with(palettes: usize, with_colormap: bool) -> Session {
        let mut lumps = vec![Lump::new(name("PLAYPAL"), fixture_bytes(palettes))];
        if with_colormap {
            let size = COLORMAP_TABLES.saturating_mul(COLORMAP_TABLE_BYTES);
            lumps.push(Lump::new(name("COLORMAP"), vec![0_u8; size]));
        }
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ));
        session
    }

    /// A 16x16 image of the fixture palette, fully opaque.
    fn fixture_grid() -> RgbaImage {
        let mut pixels = Vec::new();
        for index in 0_u8..=255 {
            pixels.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index, OPAQUE]);
        }
        RgbaImage::new(16, 16, pixels).expect("256 RGBA8 pixels fit a 16x16 image")
    }

    /// The fixture palette with entry 5 changed.
    fn changed_palette() -> Palette {
        let mut colors = fixture_palette().0;
        colors[5] = [1_u8, 2_u8, 3_u8];
        Palette::from(colors)
    }

    /// A colours payload of the changed fixture palette.
    fn changed_payload() -> Payload {
        Payload::Colors(Box::new(changed_palette()))
    }

    /// A commit request over a payload, with `(tints, colormap)` flags.
    fn request(payload: Payload, index: usize, flags: (bool, bool)) -> Request {
        Request {
            payload,
            index,
            tints: flags.0,
            colormap: flags.1,
        }
    }

    #[test]
    fn load_palette_reads_the_named_entry_of_the_merged_playpal() {
        let session = session_with(2, false);
        let palette = load_palette(&session, 1).expect("palette 1 exists");
        assert_eq!(
            palette.color(3),
            Some([3_u8, 252_u8, 3_u8]),
            "fixture colour 3"
        );
        assert_eq!(palette, fixture_palette(), "the copies hold the fixture");
    }

    #[test]
    fn load_palette_without_a_playpal_points_at_open_iwad() {
        let session = Session::default();
        let error = load_palette(&session, 0).expect_err("no PLAYPAL is loaded");
        assert_eq!(error.code(), "not_found", "missing lump code");
        assert!(
            error.to_string().contains("open_iwad"),
            "the hint names the tool: {error}"
        );
    }

    #[test]
    fn load_palette_refuses_an_index_past_the_lump() {
        let session = session_with(1, false);
        let error = load_palette(&session, 1).expect_err("one palette has no index 1");
        assert_eq!(error.code(), "not_found", "out-of-range code");
        assert!(
            error.to_string().contains("holds 1 palettes"),
            "the message states the count: {error}"
        );
    }

    #[test]
    fn the_swatch_grids_the_palette_in_sixteen_pixel_cells() {
        let image = swatch_image(&fixture_palette()).expect("a 256x256 swatch");
        assert_eq!((image.width, image.height), (256, 256), "the shape");
        assert_eq!(
            image.get(15, 15),
            Some([0_u8, 255_u8, 0_u8, OPAQUE]),
            "cell 0 covers the first sixteen pixels"
        );
        assert_eq!(
            image.get(16, 0),
            Some([1_u8, 254_u8, 1_u8, OPAQUE]),
            "cell 1 starts at x 16"
        );
        assert_eq!(
            image.get(255, 255),
            Some([255_u8, 0_u8, 255_u8, OPAQUE]),
            "the last cell carries the last colour"
        );
    }

    #[test]
    fn tint_targets_follow_the_vanilla_ramps() {
        assert_eq!(
            tint_of(1),
            ([255_u8, 0_u8, 0_u8], 1, 9),
            "first damage tint"
        );
        assert_eq!(tint_of(8), ([255_u8, 0_u8, 0_u8], 8, 9), "last damage tint");
        assert_eq!(
            tint_of(9),
            ([215_u8, 186_u8, 69_u8], 1, 8),
            "first pickup tint"
        );
        assert_eq!(
            tint_of(12),
            ([215_u8, 186_u8, 69_u8], 4, 8),
            "last pickup tint"
        );
        assert_eq!(
            tint_of(13),
            ([0_u8, 255_u8, 0_u8], 1, 8),
            "radiation-suit tint"
        );
    }

    #[test]
    fn tint_channel_moves_colours_toward_the_target_rounded() {
        assert_eq!(tint_channel(0, 255, 1, 9), 28, "25.5 rounds to 28");
        assert_eq!(tint_channel(255, 0, 8, 9), 28, "255 - 226.7 rounds to 28");
        assert_eq!(tint_channel(128, 0, 1, 2), 64, "an exact half");
        assert_eq!(tint_channel(100, 100, 1, 8), 100, "no delta stays put");
    }

    #[test]
    fn luminance_weights_the_channels_as_documented() {
        assert_eq!(luminance([255_u8, 255_u8, 255_u8]), 255, "white");
        assert_eq!(luminance([0_u8, 0_u8, 0_u8]), 0, "black");
        assert_eq!(luminance([255_u8, 0_u8, 0_u8]), 76, "0.299 red");
        assert_eq!(luminance([0_u8, 255_u8, 0_u8]), 149, "0.587 green");
        assert_eq!(luminance([0_u8, 0_u8, 255_u8]), 29, "0.114 blue");
    }

    #[test]
    fn the_colormap_ramps_from_identity_through_grey_to_black() {
        let maps = build_colormap(&fixture_bytes(1)).expect("the fixture derives maps");
        let size = COLORMAP_TABLES.saturating_mul(COLORMAP_TABLE_BYTES);
        assert_eq!(maps.len(), size, "34 tables of 256");
        let identity: Vec<u8> = (0_u8..=255).collect();
        assert_eq!(
            &maps[..size.min(PALETTE_COLORS)],
            identity.as_slice(),
            "map 0 is exact"
        );
        let middle = PALETTE_COLORS.checked_mul(16).expect("test offset fits");
        let level_sixteen = maps.get(middle).copied().expect("map 16 exists");
        assert!(level_sixteen < 255, "map 16 darkens the top colour");
        let grey_start = PALETTE_COLORS.checked_mul(32).expect("test offset fits");
        assert_eq!(
            maps.get(grey_start).copied().expect("map 32 exists"),
            120,
            "index 0 (0,255,0) inverts to 106-grey, nearest at 120"
        );
        assert_eq!(
            maps.get(grey_start.checked_add(255).expect("test offset fits"))
                .copied()
                .expect("map 32 has 256 entries"),
            135,
            "index 255 (255,0,255) inverts to 150-grey, nearest at 135"
        );
        let black_start = PALETTE_COLORS.checked_mul(33).expect("test offset fits");
        let tail = maps.get(black_start..).expect("map 33 exists");
        let black = tail.first().copied().expect("map 33 has entries");
        assert!(
            tail.iter().all(|&value| value == black),
            "map 33 is uniform"
        );
        assert_eq!(
            black, 85,
            "the fixture's nearest colour to black is entry 85"
        );
    }

    #[test]
    fn a_source_image_must_hold_exactly_256_pixels() {
        let wide = RgbaImage::new(2, 1, vec![0_u8; 8]).expect("a 2x1 image");
        let error = palette_from_image(&wide).expect_err("two pixels are not a palette");
        assert_eq!(error.code(), "invalid_argument", "pixel count code");
        assert!(
            error.to_string().contains("has 2 pixels"),
            "the message states the count: {error}"
        );
        let palette = palette_from_image(&fixture_grid()).expect("256 pixels read row-major");
        assert_eq!(palette, fixture_palette(), "row-major colour order");
    }

    #[test]
    fn interpret_bytes_sorts_images_from_raw_palettes_and_refuses_junk() {
        let png = encode_png(&fixture_grid(), None).expect("the grid encodes");
        let from_image = interpret_bytes(&png).expect("a 256-pixel PNG is a palette");
        assert!(
            matches!(from_image, Payload::Colors(palette) if *palette == fixture_palette()),
            "the image branch yields the fixture palette"
        );
        let raw = fixture_bytes(2);
        let from_raw = interpret_bytes(&raw).expect("1536 bytes are two palettes");
        assert!(
            matches!(&from_raw, Payload::Playpal(bytes) if *bytes == raw),
            "the raw branch keeps every byte"
        );
        let odd = vec![7_u8; 10];
        assert_eq!(
            interpret_bytes(&odd)
                .expect_err("ten bytes are no palette")
                .code(),
            "invalid_argument",
            "odd length code"
        );
        assert_eq!(
            interpret_bytes(b"")
                .expect_err("empty is no palette")
                .code(),
            "invalid_argument",
            "empty code"
        );
    }

    #[test]
    fn colour_payloads_replace_and_append_and_refuse_the_future() {
        let base = fixture_bytes(1);
        let mut touched = BTreeSet::new();
        let same = apply_payload(
            &base,
            Payload::Colors(Box::new(fixture_palette())),
            0,
            &mut touched,
        )
        .expect("entry 0 is replaced");
        assert_eq!(same, base, "the same palette re-written lands identically");
        assert_eq!(
            touched.into_iter().collect::<Vec<usize>>(),
            vec![0],
            "entry 0"
        );

        let mut appended = BTreeSet::new();
        let grown =
            apply_payload(&base, changed_payload(), 1, &mut appended).expect("index 1 appends");
        assert_eq!(
            grown.chunks_exact(PALETTE_BYTES).count(),
            2,
            "one palette more"
        );

        let mut refused = BTreeSet::new();
        let error = apply_payload(&base, changed_payload(), 2, &mut refused)
            .expect_err("index 2 is past the single palette");
        assert_eq!(error.code(), "invalid_argument", "future index code");
        assert!(refused.is_empty(), "the refusal touched nothing");
    }

    #[test]
    fn a_raw_payload_replaces_every_entry_it_carries() {
        let mut replaced = BTreeSet::new();
        let data = apply_payload(
            &fixture_bytes(14),
            Payload::Playpal(fixture_bytes(3)),
            0,
            &mut replaced,
        )
        .expect("the whole lump is replaced");
        assert_eq!(
            data.chunks_exact(PALETTE_BYTES).count(),
            3,
            "three palettes stand in"
        );
        assert_eq!(
            replaced.into_iter().collect::<Vec<usize>>(),
            vec![0, 1, 2],
            "every entry was written"
        );
    }

    #[test]
    fn tints_rebuild_the_vanilla_ramp_and_grow_short_data() {
        let mut data = fixture_bytes(1);
        let mut replaced = BTreeSet::new();
        rebuild_tints(&mut data, &mut replaced).expect("13 tints from palette 0");
        assert_eq!(
            data.chunks_exact(PALETTE_BYTES).count(),
            VANILLA_PALETTES,
            "grown to the vanilla 14"
        );
        assert_eq!(replaced.len(), 13, "entries 1..=13 rebuilt");
        let red = Palette::from_playpal(&data, 1).expect("tint 1 exists");
        assert_eq!(
            red.color(0),
            Some([28_u8, 227_u8, 0_u8]),
            "the fixture green tinted 1/9 toward red"
        );
        let suit = Palette::from_playpal(&data, 13).expect("tint 13 exists");
        assert_eq!(
            suit.color(255),
            Some([223_u8, 32_u8, 223_u8]),
            "magenta lifted 1/8 toward green"
        );
        rebuild_tints(&mut data, &mut replaced).expect("idempotent on full data");
        assert_eq!(
            data.chunks_exact(PALETTE_BYTES).count(),
            VANILLA_PALETTES,
            "no further growth"
        );
    }

    #[test]
    fn a_commit_copies_on_write_undoes_as_one_edit_and_leaves_the_layer() {
        let mut session = session_with(1, false);
        let layer_before = session
            .layers()
            .first()
            .expect("the IWAD layer")
            .wad
            .lumps
            .len();
        let report = commit_palette(&mut session, request(changed_payload(), 0, (false, false)))
            .expect("the edit lands");
        assert_eq!(report.palettes, 1, "the count stands");
        assert_eq!(report.replaced, vec![0], "entry 0 replaced");
        assert!(!report.colormap_regenerated, "no COLORMAP was asked for");
        assert_eq!(
            session.undo_labels(),
            vec!["set_palette"],
            "one labelled edit"
        );
        assert!(session.is_dirty(), "unsaved work");
        let working: Vec<String> = session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(working, vec!["PLAYPAL"], "the override was appended");
        assert_eq!(
            load_palette(&session, 0).expect("merged"),
            changed_palette(),
            "the override won"
        );
        assert_eq!(
            session
                .layers()
                .first()
                .expect("the IWAD layer")
                .wad
                .lumps
                .len(),
            layer_before,
            "the layer itself never changed"
        );
        assert!(
            load_palette(&session, 1).is_err(),
            "no tint was added without a flag"
        );
        session.undo().expect("undo steps back over the edit");
        assert_eq!(
            load_palette(&session, 0).expect("restored"),
            fixture_palette(),
            "the fixture is back"
        );
        assert!(session.working().lumps.is_empty(), "the override is gone");
    }

    #[test]
    fn a_full_commit_grows_the_tints_and_writes_both_lumps() {
        let mut session = session_with(1, true);
        let report = commit_palette(
            &mut session,
            request(
                Payload::Colors(Box::new(fixture_palette())),
                0,
                (true, true),
            ),
        )
        .expect("palette, tints and maps land together");
        assert_eq!(report.palettes, VANILLA_PALETTES, "grown to the vanilla 14");
        assert!(report.colormap_regenerated, "COLORMAP was rebuilt");
        assert_eq!(
            report.replaced.first().copied(),
            Some(0),
            "entry 0 written first"
        );
        assert_eq!(
            report.replaced.last().copied(),
            Some(13),
            "up to the suit tint"
        );
        let working: Vec<String> = session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(
            working,
            vec!["PLAYPAL", "COLORMAP"],
            "both overrides appended"
        );
        let maps = session
            .find(name("COLORMAP"), None)
            .expect("the merged view serves it");
        assert_eq!(
            maps.lump.data.len(),
            COLORMAP_TABLES.saturating_mul(COLORMAP_TABLE_BYTES),
            "34 tables of 256"
        );
    }

    #[test]
    fn a_commit_creates_a_missing_playpal_at_index_zero() {
        let mut session = Session::default();
        let report = commit_palette(&mut session, request(changed_payload(), 0, (false, false)))
            .expect("the first palette is created");
        assert_eq!(report.palettes, 1, "one palette stands");
        assert_eq!(
            load_palette(&session, 0).expect("readable"),
            changed_palette(),
            "its colours"
        );
    }

    #[test]
    fn zero_or_two_sources_are_refused() {
        let error = pick_source(&SetPaletteParams::default()).expect_err("no source given");
        assert_eq!(error.code(), "invalid_argument", "no source code");
        assert!(
            error.to_string().contains("exactly one"),
            "the message says so: {error}"
        );
        let both = SetPaletteParams {
            colors: Some(vec!["#000000".to_owned(); PALETTE_COLORS]),
            source: Some("/palette.png".to_owned()),
            ..SetPaletteParams::default()
        };
        assert_eq!(
            pick_source(&both).expect_err("two sources given").code(),
            "invalid_argument",
            "both sources code"
        );
        let only = SetPaletteParams {
            colors: Some(vec!["#000000".to_owned(); PALETTE_COLORS]),
            ..SetPaletteParams::default()
        };
        assert!(
            matches!(pick_source(&only), Ok(SourceArg::Colors(list)) if list.len() == PALETTE_COLORS),
            "one source is accepted"
        );
    }

    #[tokio::test]
    async fn set_palette_run_reports_the_fetch_failure_of_a_missing_file() {
        let state = AppState::default();
        let args = SetPaletteParams {
            source: Some("/definitely-not-here/cyberdemon-palette.png".to_owned()),
            ..SetPaletteParams::default()
        };
        let error = set_palette_run(&state, &args)
            .await
            .expect_err("no such file exists");
        assert_eq!(error.code(), "not_found", "missing file code");
    }
}
