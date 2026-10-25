//! The flat side of the image imports: a picture in, a raw `PLAYPAL`
//! index plane out.
//!
//! [`import_flat_run`] validates the arguments with [`parse_flat`], runs
//! the shared pieces of [`super::image_pipeline`] and [`super::processed`]
//! over the picture and hands the encoded bytes to [`commit_flat`], one
//! undoable edit in the working `flats` region. [`build_flat`] is the
//! pipeline itself: [`fit_flat_shape`] settles a supported shape (the
//! three square sides of [`FLAT_SIZES`]), the quantization is opaque -
//! [`transparent_alpha_pixels`] only counts the holes it keeps - and
//! [`Dither`] spreads the error the call asks for.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::codec::decode_image;
use crate::gfx::flat::{encode_flat, is_flat_shape};
use crate::gfx::image::{RgbaImage, resize_nearest};
use crate::gfx::ops::dither::{Dither, quantize_dithered};
use crate::gfx::quantize::{
    DEFAULT_ALPHA_THRESHOLD, QuantizeOptions, Transparency, quantize_stats,
};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::imports::image_pipeline::{DEFAULT_PALETTE, LumpWrite, chosen_name, put_lump};
use crate::tools::imports::processed::dither_or_default;
use crate::tools::palette::load_palette;
use crate::tools::reply;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// Square flat side both flat tools settle on when their `size` argument
/// is unset: the vanilla edge.
pub(in crate::tools) const DEFAULT_FLAT_SIZE: u32 = 64_u32;
/// The square sides a `size` parameter may name.
const FLAT_SIZES: [u32; 3] = [64_u32, 128_u32, 256_u32];

/// Arguments for [`crate::server::Cyberdemon::import_image_as_flat`] and its alias
/// [`crate::server::Cyberdemon::import_png_as_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ImportFlatParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL.
    pub(super) source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Which `PLAYPAL` entry the picture is quantized against (default 0:
    /// the normal palette). Past the entries the lump holds is `not_found`,
    /// as is a session with no `PLAYPAL`.
    #[serde(default)]
    pub(super) palette: Option<usize>,
    /// What to do with a picture that is not already a flat shape:
    /// `"none"` (default) refuses it, `"nearest"` scales it to `size` x
    /// `size`.
    #[serde(default)]
    pub(super) resize: Option<String>,
    /// Square side for `resize`: 64 (default), 128 or 256; anything else
    /// is `invalid_argument`. Only 64x64 loads in vanilla Doom; 128 and
    /// 256 need a ZDoom-family port. The result echoes this as `size`.
    #[serde(default)]
    pub(super) size: Option<u32>,
    /// Palette dithering of the quantization: `"none"` (plain nearest
    /// colour), `"floyd_steinberg"` (serpentine error diffusion, the
    /// default, same as `process_image`) or `"ordered"` (a fixed `4x4`
    /// `Bayer` offset). The result echoes the mode used as `dither`.
    #[serde(default)]
    pub(super) dither: Option<Dither>,
    /// Overwrite a working flat of this name in place (default `false`;
    /// without it such a lump is `already_exists`).
    #[serde(default)]
    pub(super) replace: bool,
}

/// The flat import's `resize` choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResizeChoice {
    /// Keep the decoded dimensions; a picture in no flat shape refuses.
    Keep,
    /// Nearest-neighbour scale to `size` x `size`.
    Nearest,
}

/// The validated arguments of one flat import.
#[derive(Debug)]
pub(super) struct ParsedFlat {
    /// The `source` to fetch.
    pub(super) source: String,
    /// The explicit lump name, `None` when it is derived from the file.
    pub(super) name: Option<LumpName>,
    /// `PLAYPAL` entry to quantize against.
    pub(super) palette: usize,
    /// What to do with a non-flat picture shape.
    pub(super) resize: ResizeChoice,
    /// The square side a resized picture takes.
    pub(super) size: u32,
    /// The dither mode the quantization spreads its error with.
    pub(super) dither: Dither,
    /// Whether a same-name working flat may be overwritten.
    pub(super) replace: bool,
}

/// An encoded flat ready for insertion.
#[derive(Debug)]
struct FlatImported {
    /// The lump name: the explicit one or the derived one.
    name: LumpName,
    /// The raw `PLAYPAL` index bytes to store.
    data: Vec<u8>,
    /// The stored width in pixels.
    width: u32,
    /// The stored height in pixels.
    height: u32,
    /// The `size` argument in force.
    size: u32,
    /// The dither mode the quantization used.
    dither: Dither,
    /// Whether the picture was nearest-neighbour scaled.
    resized: bool,
    /// Distinct source colours.
    colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    max_color_error: i32,
    /// Source pixels that had alpha below the cutoff and stayed opaque.
    transparent_pixels_ignored: usize,
}

/// Structured result of [`crate::server::Cyberdemon::import_image_as_flat`] and its alias.
#[derive(Serialize)]
struct FlatReport {
    /// The name the flat was stored under.
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// The stored width in pixels.
    width: u32,
    /// The stored height in pixels.
    height: u32,
    /// The `size` argument in force: the square side `resize` scales to.
    size: u32,
    /// The dither mode the quantization used.
    dither: Dither,
    /// Whether the picture was nearest-neighbour scaled.
    resized: bool,
    /// Distinct source colours among the (fully opaque) pixels.
    colors_used: usize,
    /// Worst squared `RGB` distance from a source colour to its palette
    /// entry.
    max_color_error: i32,
    /// Source pixels that had alpha below 128 and were kept opaque.
    transparent_pixels_ignored: usize,
}

/// Validate the flat-import arguments: name, `resize` word, `size` and
/// `dither`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse, an
/// unknown `resize` mode or a `size` outside the three square sides.
pub(super) fn parse_flat(args: &ImportFlatParams) -> Result<ParsedFlat, CyberError> {
    Ok(ParsedFlat {
        source: args.source.clone(),
        name: args.name.as_deref().map(LumpName::parse).transpose()?,
        palette: args.palette.unwrap_or(DEFAULT_PALETTE),
        resize: parse_resize(args.resize.as_deref())?,
        size: parse_flat_size(args.size)?,
        dither: dither_or_default(args.dither),
        replace: args.replace,
    })
}

/// The `resize` word: `"none"` (the default) or `"nearest"`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted words.
pub(super) fn parse_resize(text: Option<&str>) -> Result<ResizeChoice, CyberError> {
    match text {
        None | Some("none") => Ok(ResizeChoice::Keep),
        Some("nearest") => Ok(ResizeChoice::Nearest),
        Some(other) => Err(CyberError::InvalidArgument(format!(
            "resize {other:?} is unknown: pass \"none\" to keep the picture size or \
             \"nearest\" to scale it to size x size"
        ))),
    }
}

/// The `size` parameter: one of the three square sides, default 64.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted sides.
pub(in crate::tools) fn parse_flat_size(value: Option<u32>) -> Result<u32, CyberError> {
    let size = value.unwrap_or(DEFAULT_FLAT_SIZE);
    if FLAT_SIZES.contains(&size) {
        return Ok(size);
    }
    Err(CyberError::InvalidArgument(format!(
        "size {size} is not a square flat side: pass {}",
        FLAT_SIZES
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// The picture scaled (or kept) to a supported flat shape, with whether
/// it was scaled.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the four flat shapes and the
/// `resize` option when an unsupported shape meets `resize: "none"`.
pub(super) fn fit_flat_shape(
    img: &RgbaImage,
    resize: ResizeChoice,
    size: u32,
) -> Result<(RgbaImage, bool), CyberError> {
    if is_flat_shape(img.width, img.height) {
        return Ok((img.clone(), false));
    }
    match resize {
        ResizeChoice::Nearest => Ok((resize_nearest(img, size, size), true)),
        ResizeChoice::Keep => Err(CyberError::InvalidArgument(format!(
            "flats are 64x64, 64x128, 128x128 or 256x256, got {}x{}: pass resize=\"nearest\" \
             with size 64, 128 or 256 to scale the picture",
            img.width, img.height
        ))),
    }
}

/// Source pixels with alpha below [`DEFAULT_ALPHA_THRESHOLD`]: flats store
/// no transparency, so they are kept opaque and only counted.
pub(in crate::tools) fn transparent_alpha_pixels(img: &RgbaImage) -> usize {
    img.pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .map(to_pixel)
        .filter(|&[_, _, _, alpha]| alpha < DEFAULT_ALPHA_THRESHOLD)
        .count()
}

/// `import_image_as_flat`' work: run the pipeline, then commit the flat as
/// one undoable edit.
///
/// # Errors
///
/// [`parse_flat`]' argument errors, [`build_flat`]' source, picture and
/// palette errors plus [`commit_flat`]' placement and `already_exists`
/// errors; a refusal leaves the working archive untouched.
async fn import_flat_run(
    state: &AppState,
    args: &ImportFlatParams,
) -> Result<FlatReport, CyberError> {
    let import = parse_flat(args)?;
    let built = build_flat(state, &import).await?;
    let replace = import.replace;
    state.with_session(move |session| commit_flat(session, built, replace))
}

/// Fetch, decode, scale, quantize and encode one flat: the shared pipeline
/// with [`Transparency::None`], because flats have no holes, and the
/// call's [`Dither`] mode.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`decode_image`]' errors for bytes that are no readable image,
/// [`CyberError::InvalidArgument`] for an underivable name, an unusable
/// shape or an image past the flat codec's limits, and [`load_palette`]'
/// [`CyberError::NotFound`] for the palette.
async fn build_flat(state: &AppState, import: &ParsedFlat) -> Result<FlatImported, CyberError> {
    let fetched = state.fetch_source(&import.source).await?;
    let name = chosen_name(import.name, &fetched.filename)?;
    let decoded = decode_image(&fetched.bytes)?;
    let ignored = transparent_alpha_pixels(&decoded);
    let (picture, resized) = fit_flat_shape(&decoded, import.resize, import.size)?;
    let options = QuantizeOptions {
        transparency: Transparency::None,
        reserved: Vec::new(),
    };
    let palette = state.with_session(|session| load_palette(session, import.palette))?;
    let indexed = quantize_dithered(&picture, &palette, &options, import.dither);
    let (colors_used, max_color_error) = quantize_stats(&picture, &palette, &options);
    let (data, _) = encode_flat(&indexed)?;
    Ok(FlatImported {
        name,
        data,
        width: picture.width,
        height: picture.height,
        size: import.size,
        dither: import.dither,
        resized,
        colors_used,
        max_color_error,
        transparent_pixels_ignored: ignored,
    })
}

/// The synchronous half of a flat import: one undoable edit placing the
/// raw indexed bytes in the working archive's flats region.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
fn commit_flat(
    session: &mut Session,
    built: FlatImported,
    replace: bool,
) -> Result<FlatReport, CyberError> {
    let label = format!("import_flat {}", built.name);
    session.edit(&label, move |wad| {
        let write = LumpWrite {
            name: built.name,
            data: &built.data,
            namespace: Some(Namespace::Flats),
            replace,
        };
        let lump_index = put_lump(wad, &write)?;
        Ok(FlatReport {
            name: built.name,
            lump_index,
            width: built.width,
            height: built.height,
            size: built.size,
            dither: built.dither,
            resized: built.resized,
            colors_used: built.colors_used,
            max_color_error: built.max_color_error,
            transparent_pixels_ignored: built.transparent_pixels_ignored,
        })
    })
}

/// The shared body of [`crate::server::Cyberdemon::import_image_as_flat`] and
/// [`crate::server::Cyberdemon::import_png_as_flat`].
pub(super) async fn flat_imported(
    state: &AppState,
    args: &ImportFlatParams,
) -> Result<CallToolResult, ErrorData> {
    reply(import_flat_run(state, args).await)
}
