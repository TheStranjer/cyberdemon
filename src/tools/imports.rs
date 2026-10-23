//! Image import tools: whole pictures in, lumps out.
//!
//! [`Cyberdemon::import_image_as_patch`] (and its alias
//! [`Cyberdemon::import_png_as_patch`]) runs the shared pipeline of
//! [`image_pipeline`] - fetch, decode, quantize to the session's
//! `PLAYPAL`, resolve drawing offsets, encode a `Doom` patch - and drops
//! the result into the working archive's patches region as one undoable
//! edit, optionally listing the name in `PNAMES` (copy-on-write).
//! [`Cyberdemon::import_image_as_flat`] (and its alias
//! [`Cyberdemon::import_png_as_flat`]) reuses the same pieces on the flat
//! side: decode, opaque quantization in the call's [`Dither`] mode,
//! nearest scaling to a supported flat shape, `encode_flat`, and insertion
//! into the working `flats` region.
//! [`Cyberdemon::import_image_as_raw`] validates that the bytes are a
//! readable image and stores them unchanged, for `ZDoom`-family ports
//! that read `PNG` lumps directly.
//! [`Cyberdemon::import_image_as_sprite`] (and its alias
//! [`Cyberdemon::import_png_as_sprite`]) runs the same pipeline on the
//! sprite side: [`sprite`] names the lump through the sprite
//! grammar of [`crate::gfx::sprite_name::parse_sprite_name`], anchors the
//! picture on its feet and reports the rotations the session's sprites
//! region already serves. The patch lifecycle tools of
//! [`crate::tools::patches`] stage and place their lumps on these same
//! pieces.
//!
//! [`Cyberdemon::process_image`] runs the shared [`processed`] helper -
//! [`crate::gfx::ops::apply_ops`] over the decoded picture - and publishes
//! the RGBA and the palette-quantized result as `PNG` assets without
//! touching any archive, so a caller can look at a pipeline before
//! committing it. [`Cyberdemon::create_texture_from_image`] commits the
//! whole wall side in one [`crate::session::Session::edit`]: [`texture`]
//! settles a tiling-friendly size, quantizes the picture opaquely, stores
//! the patch, grows `PNAMES` and defines the `TEXTURE1`/`TEXTURE2` record
//! through the [`crate::tools::textures`] internals, optionally pointing a
//! theme wall role at the result. [`Cyberdemon::create_flat_from_image`]
//! is its floor/ceiling twin: [`create_flat`] fits the processed picture
//! to the square `size` you ask for (`64x64` by default), tiles and
//! quantizes it opaquely, and places the raw flat in the working `flats`
//! region with the stricter everywhere-used collision rule, optionally
//! assigning a theme flat role; a side above 64 adds a warning that
//! vanilla Doom cannot load the flat. [`Cyberdemon::create_sprite_from_image`]
//! is the sprite twin:
//! [`create_sprite`] removes the border background, trims, scales to a
//! sprite height, quantizes with alpha holes and places the named lump
//! (or one per `frames` letter) in the working `sprites` region,
//! carrying [`sprite`]' rotation warnings through.

pub(in crate::tools) mod create_flat;
pub(in crate::tools) mod create_sprite;
pub(in crate::tools) mod image_pipeline;
pub(in crate::tools) mod processed;
mod sprite;
mod texture;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::codec::{decode_image, format_name};
use crate::gfx::flat::{encode_flat, is_flat_shape};
use crate::gfx::image::{RgbaImage, resize_nearest};
use crate::gfx::ops::dither::{Dither, quantize_dithered};
use crate::gfx::quantize::{
    DEFAULT_ALPHA_THRESHOLD, QuantizeOptions, Transparency, quantize_stats,
};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::create_flat::{
    CREATE_FLAT_TOOL_DESCRIPTION, CreateFlatParams, create_flat_run,
};
use crate::tools::imports::create_sprite::{
    CREATE_SPRITE_TOOL_DESCRIPTION, CreateSpriteParams, create_sprite_run,
};
use crate::tools::imports::image_pipeline::{
    DEFAULT_PALETTE, ImageImportParams, ImportReport, LumpWrite, OffsetDefault, Patched,
    build_patch, chosen_name, parse_import, put_lump,
};
use crate::tools::imports::processed::{
    PROCESS_TOOL_DESCRIPTION, ProcessImageParams, dither_or_default, process_image_run,
};
use crate::tools::imports::sprite::{
    ALIASED_SPRITE_TOOL_DESCRIPTION, ImportSpriteParams, SPRITE_TOOL_DESCRIPTION, sprite_imported,
};
use crate::tools::imports::texture::{
    CREATE_TEXTURE_TOOL_DESCRIPTION, CreateTextureParams, create_texture_run,
};
use crate::tools::ok;
use crate::tools::palette::load_palette;
use crate::tools::textures::put_global;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;
use crate::wad::textures::{parse_pnames, write_pnames};

/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";
/// Square flat side both flat tools settle on when their `size` argument
/// is unset: the vanilla edge.
pub(in crate::tools) const DEFAULT_FLAT_SIZE: u32 = 64_u32;
/// The square sides a `size` parameter may name.
pub(in crate::tools) const FLAT_SIZES: [u32; 3] = [64_u32, 128_u32, 256_u32];

/// Arguments for [`Cyberdemon::import_image_as_patch`] and its alias
/// [`Cyberdemon::import_png_as_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportPatchParams {
    /// The shared image-import parameters: `source`, `name`, `palette`,
    /// `transparency`, `offsets` and `replace` (see
    /// `skill/reference/imports.md`).
    #[serde(flatten)]
    common: ImageImportParams,
    /// Also list the name in `PNAMES`, appending it copy-on-write when
    /// missing (default `true`).
    #[serde(default)]
    add_to_pnames: Option<bool>,
}

/// Arguments for [`Cyberdemon::import_image_as_flat`] and its alias
/// [`Cyberdemon::import_png_as_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportFlatParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL.
    source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    name: Option<String>,
    /// Which `PLAYPAL` entry the picture is quantized against (default 0:
    /// the normal palette). Past the entries the lump holds is `not_found`,
    /// as is a session with no `PLAYPAL`.
    #[serde(default)]
    palette: Option<usize>,
    /// What to do with a picture that is not already a flat shape:
    /// `"none"` (default) refuses it, `"nearest"` scales it to `size` x
    /// `size`.
    #[serde(default)]
    resize: Option<String>,
    /// Square side for `resize`: 64 (default), 128 or 256; anything else
    /// is `invalid_argument`. Only 64x64 loads in vanilla Doom; 128 and
    /// 256 need a ZDoom-family port. The result echoes this as `size`.
    #[serde(default)]
    size: Option<u32>,
    /// Palette dithering of the quantization: `"none"` (plain nearest
    /// colour), `"floyd_steinberg"` (serpentine error diffusion, the
    /// default, same as `process_image`) or `"ordered"` (a fixed `4x4`
    /// `Bayer` offset). The result echoes the mode used as `dither`.
    #[serde(default)]
    dither: Option<Dither>,
    /// Overwrite a working flat of this name in place (default `false`;
    /// without it such a lump is `already_exists`).
    #[serde(default)]
    replace: bool,
}

/// Arguments for [`Cyberdemon::import_image_as_raw`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportRawParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL. Its bytes are stored unchanged
    /// after a decode check.
    source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    name: Option<String>,
    /// Region to place the lump in (`sprites`, `flats`, `patches`,
    /// `colormaps`, `acs`, `voices`, `hires` or `textures`); without it
    /// the lump is stored loose, in the global area.
    #[serde(default)]
    namespace: Option<String>,
    /// Overwrite a working lump of this name in the target region in place
    /// (default `false`).
    #[serde(default)]
    replace: bool,
}

/// Structured result of [`Cyberdemon::import_image_as_patch`] and its
/// alias: the shared pipeline report with the placement facts.
#[derive(Serialize)]
struct PatchReport {
    /// The name the patch was stored under.
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// Its `PNAMES` entry index, `null` when the name is not listed.
    pnames_index: Option<usize>,
    /// The shared `width`, `height`, `left_offset`, `top_offset`,
    /// `transparent_pixels`, `colors_used` and `max_color_error`.
    #[serde(flatten)]
    pipeline: ImportReport,
}

/// Structured result of [`Cyberdemon::import_image_as_raw`].
#[derive(Serialize)]
struct RawReport {
    /// The name the bytes were stored under.
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// The sniffed image format: `"png"`, `"jpeg"`, `"gif"` or `"bmp"`.
    format: &'static str,
    /// Width in pixels, from the decode check.
    width: u32,
    /// Height in pixels.
    height: u32,
}

/// The `PNAMES` side of one patch import.
pub(in crate::tools) struct PnamesPlan {
    /// The index the name answers with in the effective list.
    pub(in crate::tools) index: usize,
    /// The new `PNAMES` bytes when the list grew, `None` when the name was
    /// already listed and nothing needs writing.
    pub(in crate::tools) bytes: Option<Vec<u8>>,
}

/// The flat import's `resize` choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResizeChoice {
    /// Keep the decoded dimensions; a picture in no flat shape refuses.
    Keep,
    /// Nearest-neighbour scale to `size` x `size`.
    Nearest,
}

/// The validated arguments of one flat import.
#[derive(Debug)]
struct ParsedFlat {
    /// The `source` to fetch.
    source: String,
    /// The explicit lump name, `None` when it is derived from the file.
    name: Option<LumpName>,
    /// `PLAYPAL` entry to quantize against.
    palette: usize,
    /// What to do with a non-flat picture shape.
    resize: ResizeChoice,
    /// The square side a resized picture takes.
    size: u32,
    /// The dither mode the quantization spreads its error with.
    dither: Dither,
    /// Whether a same-name working flat may be overwritten.
    replace: bool,
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

/// Structured result of [`Cyberdemon::import_image_as_flat`] and its alias.
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

/// The name to import under: the explicit one, else one derived from the
/// fetched file name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
fn chosen(explicit: Option<&str>, filename: &str) -> Result<LumpName, CyberError> {
    let parsed = explicit.map(LumpName::parse).transpose()?;
    chosen_name(parsed, filename)
}

/// The region a placement keeps, `None` staying the global area.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_namespace(text: Option<&str>) -> Result<Option<Namespace>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    Namespace::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown namespace {raw:?}: expected one of {}",
            namespace_names()
        ))
    })
}

/// The comma-separated namespace region names.
fn namespace_names() -> String {
    Namespace::ALL
        .iter()
        .map(|namespace| namespace.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The merged view's effective `PNAMES` names; no `PNAMES` lump is the
/// empty list.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed lump.
pub(in crate::tools) fn current_pnames(session: &Session) -> Result<Vec<String>, CyberError> {
    let Some(entry) = session.find(LumpName::from_raw(PNAMES_RAW), None) else {
        return Ok(Vec::new());
    };
    parse_pnames(&entry.lump.data)
}

/// The position of `name` in `list`, matched case-insensitively.
fn position_of(list: &[String], name: &str) -> Option<usize> {
    list.iter()
        .position(|entry| entry.eq_ignore_ascii_case(name))
}

/// What an import with `add_to_pnames` answers and writes: the entry index
/// (existing or freshly appended) and, only when the effective list grew,
/// the new lump bytes.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed `PNAMES` and
/// [`write_pnames`]' [`CyberError::InvalidArgument`] for a name it cannot
/// store.
pub(in crate::tools) fn pnames_plan(
    state: &AppState,
    name: LumpName,
    add: Option<bool>,
) -> Result<Option<PnamesPlan>, CyberError> {
    if !add.unwrap_or(true) {
        return Ok(None);
    }
    let current = state.with_session(|session| current_pnames(session))?;
    let text = name.to_string();
    if let Some(index) = position_of(&current, &text) {
        return Ok(Some(PnamesPlan { index, bytes: None }));
    }
    let mut names = current;
    let index = names.len();
    names.push(text);
    Ok(Some(PnamesPlan {
        index,
        bytes: Some(write_pnames(&names)?),
    }))
}

/// `import_image_as_patch`' work: run the pipeline, plan the `PNAMES`
/// entry, then commit patch and list as one undoable edit.
///
/// # Errors
///
/// Whatever [`build_patch`] reports for the source and picture, plus
/// [`pnames_plan`]' and [`put_lump`]' errors; a refusal leaves the working
/// archive untouched.
async fn import_patch_run(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<PatchReport, CyberError> {
    let import = parse_import(&args.common)?;
    let patched = build_patch(state, &import, OffsetDefault::Zero).await?;
    let plan = pnames_plan(state, patched.name, args.add_to_pnames)?;
    let replace = import.replace;
    state.with_session(move |session| commit_patch(session, patched, plan, replace))
}

/// The synchronous half of a patch import: one undoable edit that places
/// the patch lump and, when the effective `PNAMES` lacked the name, writes
/// the grown list as a working copy-on-write.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`crate::tools::textures::put_global`]' stale-index [`CyberError::Internal`].
fn commit_patch(
    session: &mut Session,
    patched: Patched,
    plan: Option<PnamesPlan>,
    replace: bool,
) -> Result<PatchReport, CyberError> {
    let label = format!("import_patch {}", patched.name);
    session.edit(&label, |wad| {
        let write = LumpWrite {
            name: patched.name,
            data: &patched.data,
            namespace: Some(Namespace::Patches),
            replace,
        };
        let lump_index = put_lump(wad, &write)?;
        if let Some(list) = plan.as_ref().and_then(|entry| entry.bytes.as_ref()) {
            put_global(wad, LumpName::from_raw(PNAMES_RAW), list.clone())?;
        }
        Ok(PatchReport {
            name: patched.name,
            lump_index,
            pnames_index: plan.map(|entry| entry.index),
            pipeline: patched.report,
        })
    })
}

/// `import_image_as_raw`' work: fetch, run the decode check, store the
/// original bytes as one undoable edit.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`decode_image`]' errors for bytes that are no readable image and
/// [`put_lump`]' placement and `already_exists` errors.
async fn import_raw_run(state: &AppState, args: &ImportRawParams) -> Result<RawReport, CyberError> {
    let namespace = parse_namespace(args.namespace.as_deref())?;
    let fetched = state.fetch_source(&args.source).await?;
    let name = chosen(args.name.as_deref(), &fetched.filename)?;
    let image = decode_image(&fetched.bytes)?;
    let format = format_name(&fetched.bytes).ok_or_else(|| {
        CyberError::Internal("a decoded image answered no format name".to_owned())
    })?;
    let (width, height) = (image.width, image.height);
    let write = LumpWrite {
        name,
        data: &fetched.bytes,
        namespace,
        replace: args.replace,
    };
    let lump_index = state.with_session(|session| commit_raw(session, &write))?;
    Ok(RawReport {
        name,
        lump_index,
        format,
        width,
        height,
    })
}

/// The synchronous half of a raw import: one undoable byte-for-byte insert.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
fn commit_raw(session: &mut Session, write: &LumpWrite<'_>) -> Result<usize, CyberError> {
    let label = format!("import_raw {}", write.name);
    session.edit(&label, |wad| put_lump(wad, write))
}

/// Validate the flat-import arguments: name, `resize` word, `size` and
/// `dither`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse, an
/// unknown `resize` mode or a `size` outside the three square sides.
fn parse_flat(args: &ImportFlatParams) -> Result<ParsedFlat, CyberError> {
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
fn parse_resize(text: Option<&str>) -> Result<ResizeChoice, CyberError> {
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
fn fit_flat_shape(
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

/// The shared body of [`Cyberdemon::import_image_as_flat`] and
/// [`Cyberdemon::import_png_as_flat`].
async fn flat_imported(
    state: &AppState,
    args: &ImportFlatParams,
) -> Result<CallToolResult, ErrorData> {
    match import_flat_run(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The shared body of [`Cyberdemon::import_image_as_patch`] and
/// [`Cyberdemon::import_png_as_patch`].
async fn patch_imported(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<CallToolResult, ErrorData> {
    match import_patch_run(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The full documentation of the patch import, shared by the tool and its
/// alias; [`ALIASED_PATCH_TOOL_DESCRIPTION`] is the alias's variant.
const PATCH_TOOL_DESCRIPTION: &str = "Imports an image file (PNG, JPEG, GIF or BMP) as a Doom patch lump: the picture is quantized to the session PLAYPAL and stored in the classic column/post format in the working archive's patches region as one undoable edit; name defaults to the file stem. Prefer create_texture_from_image to go from a picture to a finished, PNAMES-listed, TEXTURE-defined wall texture in one call.";

/// [`PATCH_TOOL_DESCRIPTION`] with the alias note on the front.
const ALIASED_PATCH_TOOL_DESCRIPTION: &str = "Alias of import_image_as_patch for clients that think in PNG: it accepts any supported image, not just PNG, and is otherwise identical - same parameters, patches-region placement, undoable edit and result. See import_image_as_patch (prefer create_texture_from_image for a finished wall texture).";

/// The full documentation of the flat import, shared by the tool and its
/// alias; [`ALIASED_FLAT_TOOL_DESCRIPTION`] is the alias's variant.
const FLAT_TOOL_DESCRIPTION: &str = "Imports an image file (PNG, JPEG, GIF or BMP) as a Doom flat lump: the picture is quantized to the session PLAYPAL in your dither mode (none, ordered or floyd_steinberg - default floyd_steinberg, as in process_image) and stored as the raw index plane in the working archive's flats region as one undoable edit, name defaulting to the file stem; other shapes resize to the square size you pass (64, 128 or 256). The result echoes the dither and size used. Prefer create_flat_from_image - it fits a size you choose, blends the edges to tile and can assign a theme role.";

/// [`FLAT_TOOL_DESCRIPTION`] with the alias note on the front.
const ALIASED_FLAT_TOOL_DESCRIPTION: &str = "Alias of import_image_as_flat for clients that think in PNG: it accepts any supported image and is otherwise identical - same parameters including dither and size, flats-region placement, undoable edit and result. See import_image_as_flat (prefer create_flat_from_image for a finished tiling flat).";

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = imports_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Imports an image file as a Doom patch lump in the patches region.
    #[tool(description = PATCH_TOOL_DESCRIPTION)]
    async fn import_image_as_patch(
        &self,
        Parameters(args): Parameters<ImportPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        patch_imported(self.state(), &args).await
    }

    /// Alias of [`Cyberdemon::import_image_as_patch`].
    #[tool(description = ALIASED_PATCH_TOOL_DESCRIPTION)]
    async fn import_png_as_patch(
        &self,
        Parameters(args): Parameters<ImportPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        patch_imported(self.state(), &args).await
    }

    /// Imports an image file as a Doom flat lump in the flats region.
    #[tool(description = FLAT_TOOL_DESCRIPTION)]
    async fn import_image_as_flat(
        &self,
        Parameters(args): Parameters<ImportFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        flat_imported(self.state(), &args).await
    }

    /// Alias of [`Cyberdemon::import_image_as_flat`].
    #[tool(description = ALIASED_FLAT_TOOL_DESCRIPTION)]
    async fn import_png_as_flat(
        &self,
        Parameters(args): Parameters<ImportFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        flat_imported(self.state(), &args).await
    }

    /// Imports an image file as a named sprite lump in the sprites region.
    #[tool(description = SPRITE_TOOL_DESCRIPTION)]
    async fn import_image_as_sprite(
        &self,
        Parameters(args): Parameters<ImportSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        sprite_imported(self.state(), &args).await
    }

    /// Alias of [`Cyberdemon::import_image_as_sprite`].
    #[tool(description = ALIASED_SPRITE_TOOL_DESCRIPTION)]
    async fn import_png_as_sprite(
        &self,
        Parameters(args): Parameters<ImportSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        sprite_imported(self.state(), &args).await
    }

    /// Stores a validated image file's bytes unchanged as a lump.
    #[tool(
        description = "Validates that source bytes are a readable image (PNG, JPEG, GIF or BMP) and inserts them into the working archive exactly as they arrived - no quantization, no re-encoding - as one undoable edit; optional namespace places the lump in a region. This is how ZDoom-family ports carry graphics; vanilla Doom wants import_image_as_patch, import_image_as_flat or import_image_as_sprite."
    )]
    async fn import_image_as_raw(
        &self,
        Parameters(args): Parameters<ImportRawParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match import_raw_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Runs the image-processing pipeline and publishes the result.
    #[tool(description = PROCESS_TOOL_DESCRIPTION)]
    async fn process_image(
        &self,
        Parameters(args): Parameters<ProcessImageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        process_image_run(self.state(), &args).await
    }

    /// Builds a whole wall texture from one picture in one undoable step.
    #[tool(description = CREATE_TEXTURE_TOOL_DESCRIPTION)]
    async fn create_texture_from_image(
        &self,
        Parameters(args): Parameters<CreateTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        create_texture_run(self.state(), &args).await
    }

    /// Builds a working floor/ceiling flat from one picture in one step.
    #[tool(description = CREATE_FLAT_TOOL_DESCRIPTION)]
    async fn create_flat_from_image(
        &self,
        Parameters(args): Parameters<CreateFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        create_flat_run(self.state(), &args).await
    }

    /// Builds working sprite lump(s) from one picture in one undoable step.
    #[tool(description = CREATE_SPRITE_TOOL_DESCRIPTION)]
    async fn create_sprite_from_image(
        &self,
        Parameters(args): Parameters<CreateSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        create_sprite_run(self.state(), &args).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dither, ImportFlatParams, ImportRawParams, ResizeChoice, chosen, current_pnames,
        fit_flat_shape, namespace_names, parse_flat, parse_flat_size, parse_namespace,
        parse_resize, pnames_plan, position_of, transparent_alpha_pixels,
    };
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;
    use crate::session::Session;
    use crate::state::AppState;
    use crate::wad::archive::Lump;
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;
    use crate::wad::textures::write_pnames;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `PNAMES` payload listing `texts`.
    fn pnames_bytes(texts: &[&str]) -> Vec<u8> {
        write_pnames(
            &texts
                .iter()
                .map(|text| (*text).to_owned())
                .collect::<Vec<String>>(),
        )
        .expect("test names are valid")
    }

    #[test]
    fn chosen_takes_the_explicit_name_or_derives_one() {
        assert_eq!(
            chosen(Some("given"), "art.png").expect("explicit"),
            name("GIVEN")
        );
        assert_eq!(chosen(None, "art.png").expect("derived"), name("ART"));
        assert_eq!(
            code(&chosen(Some("too long"), "art.png").expect_err("bad name")),
            "invalid_argument"
        );
        let error = chosen(None, "###.png").expect_err("nothing derivable");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("name"),
            "names the parameter: {error}"
        );
    }

    #[test]
    fn namespaces_parse_case_insensitively_and_refuse_else() {
        assert_eq!(parse_namespace(None).expect("absent"), None);
        assert_eq!(
            parse_namespace(Some("Patches")).expect("region"),
            Some(Namespace::Patches)
        );
        let error = parse_namespace(Some("nope")).expect_err("not a region");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            namespace_names()
                .split(", ")
                .all(|region| error.to_string().contains(region)),
            "the message lists every region: {error}"
        );
    }

    #[test]
    fn position_matching_ignores_case() {
        let list = ["WALLP1".to_owned(), "STEPP1".to_owned()];
        assert_eq!(position_of(&list, "stepp1"), Some(1));
        assert_eq!(position_of(&list, "GONE"), None);
    }

    #[test]
    fn current_pnames_reads_the_merged_lump_or_nothing() {
        let mut session = Session::default();
        assert!(
            current_pnames(&session).expect("no lump").is_empty(),
            "without a PNAMES lump the list is empty"
        );
        seed_pnames_session(&mut session, &["WALLP1"]);
        assert_eq!(
            current_pnames(&session).expect("the lump lists"),
            vec!["WALLP1".to_owned()]
        );
    }

    /// Appends a `PNAMES` lump listing `texts` to a session's working archive.
    fn seed_pnames_session(session: &mut Session, texts: &[&str]) {
        let bytes = pnames_bytes(texts);
        session
            .edit("seed", move |wad| {
                wad.lumps.push(Lump::new(name("PNAMES"), bytes));
                Ok(())
            })
            .expect("plain append");
    }

    /// Seeds a state's working archive with a `PNAMES` lump.
    fn seed_pnames_state(state: &AppState, texts: &[&str]) {
        state
            .with_session(|session| {
                seed_pnames_session(session, texts);
                Ok(())
            })
            .expect("the seed edit runs");
    }

    #[tokio::test]
    async fn pnames_plans_keep_existing_entries_and_append_new_ones() {
        let state = AppState::default();
        seed_pnames_state(&state, &["WALLP1", "STEPP1"]);
        assert!(
            pnames_plan(&state, name("NEW"), Some(false))
                .expect("off stays off")
                .is_none(),
            "add_to_pnames false writes nothing"
        );
        let kept = pnames_plan(&state, name("wallp1"), None)
            .expect("existing")
            .expect("planned");
        assert_eq!(
            kept.index, 0_usize,
            "the existing entry answers with its index"
        );
        assert!(kept.bytes.is_none(), "the list does not change");
        let added = pnames_plan(&state, name("NEWART"), None)
            .expect("appended")
            .expect("planned");
        assert_eq!(added.index, 2_usize, "the new entry lands last");
        assert!(added.bytes.is_some(), "the grown list gets written");
    }

    /// The raw arguments' wire shape deserializes with the documented flags.
    #[test]
    fn raw_arguments_deserialize() {
        let args: ImportRawParams = serde_json::from_value(serde_json::json!({
            "source": "/x/y.png",
            "namespace": "patches",
            "replace": true,
        }))
        .expect("the wire form");
        assert_eq!(args.namespace.as_deref(), Some("patches"));
        assert!(args.replace, "the flag carries");
    }

    /// Flat arguments over `source` with everything else unset.
    fn flat_params(source: &str) -> ImportFlatParams {
        ImportFlatParams {
            source: source.to_owned(),
            name: None,
            palette: None,
            resize: None,
            size: None,
            dither: None,
            replace: false,
        }
    }

    /// A `width` by `height` test image whose every pixel carries `alpha`.
    fn rgba_image(width: u32, height: u32, alpha: u8) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let mut pixels: Vec<u8> = Vec::new();
        for _pixel in 0..count {
            pixels.extend_from_slice(&[1_u8, 2, 3, alpha]);
        }
        RgbaImage::new(width, height, pixels).expect("test shape is valid")
    }

    #[test]
    fn parse_flat_applies_defaults_and_validates_the_name() {
        let parsed = parse_flat(&flat_params("/x/y.png")).expect("a source is enough");
        assert_eq!(parsed.source, "/x/y.png", "the source carries");
        assert_eq!(parsed.name, None, "no name was given");
        assert_eq!(parsed.palette, 0_usize, "the normal palette by default");
        assert_eq!(parsed.resize, ResizeChoice::Keep, "no resizing by default");
        assert_eq!(parsed.size, 64_u32, "the vanilla side by default");
        assert_eq!(
            parsed.dither,
            Dither::FloydSteinberg,
            "the same dither default process_image documents"
        );
        assert!(!parsed.replace, "no silent overwrite");
        let mut args = flat_params("/x/y.png");
        args.name = Some("too long for wad".to_owned());
        assert_eq!(
            code(&parse_flat(&args).expect_err("bad name")),
            "invalid_argument"
        );
    }

    #[test]
    fn parse_flat_keeps_an_explicit_dither_mode() {
        for mode in [Dither::None, Dither::Ordered, Dither::FloydSteinberg] {
            let mut args = flat_params("/x/y.png");
            args.dither = Some(mode);
            assert_eq!(
                parse_flat(&args).expect("a parsed mode carries").dither,
                mode,
                "the mode passes through untouched"
            );
        }
    }

    #[test]
    fn resize_words_parse_and_name_the_rejections() {
        assert_eq!(parse_resize(None).expect("absent"), ResizeChoice::Keep);
        assert_eq!(
            parse_resize(Some("none")).expect("keep"),
            ResizeChoice::Keep
        );
        assert_eq!(
            parse_resize(Some("nearest")).expect("scale"),
            ResizeChoice::Nearest
        );
        let error = parse_resize(Some("linear")).expect_err("not a word");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("\"nearest\""),
            "the message lists the words: {error}"
        );
    }

    #[test]
    fn flat_sizes_take_the_three_sides_and_refuse_else() {
        assert_eq!(parse_flat_size(None).expect("default"), 64_u32);
        for side in [64_u32, 128, 256] {
            assert_eq!(parse_flat_size(Some(side)).expect("a square"), side);
        }
        let error = parse_flat_size(Some(32_u32)).expect_err("no 32 side");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("64, 128, 256"),
            "the message lists the sides: {error}"
        );
    }

    #[test]
    fn fit_flat_shape_keeps_supported_shapes_and_scales_the_rest() {
        for (width, height) in [(64_u32, 64_u32), (64, 128), (128, 128), (256, 256)] {
            let picture = rgba_image(width, height, 255_u8);
            let (kept, resized) =
                fit_flat_shape(&picture, ResizeChoice::Nearest, 256_u32).expect("a flat shape");
            assert!(!resized, "{width}x{height} is kept as it is");
            assert_eq!((kept.width, kept.height), (width, height));
        }
        let odd = rgba_image(100_u32, 50_u32, 255_u8);
        let (scaled, resized) =
            fit_flat_shape(&odd, ResizeChoice::Nearest, 128_u32).expect("nearest scales");
        assert!(resized, "the odd shape was scaled");
        assert_eq!((scaled.width, scaled.height), (128_u32, 128_u32));
    }

    #[test]
    fn an_unsupported_shape_without_resize_names_the_way_out() {
        let odd = rgba_image(100_u32, 50_u32, 255_u8);
        let error = fit_flat_shape(&odd, ResizeChoice::Keep, 64_u32).expect_err("keep refuses");
        assert_eq!(code(&error), "invalid_argument");
        let message = error.to_string();
        assert!(message.contains("256x256"), "names the shapes: {message}");
        assert!(
            message.contains("resize=\"nearest\""),
            "names the option: {message}"
        );
    }

    #[test]
    fn alpha_pixels_below_the_cutoff_are_counted() {
        let pixels = vec![1_u8, 2, 3, 0, 1, 2, 3, 128, 1, 2, 3, 127, 1, 2, 3, 255];
        let img = RgbaImage::new(4_u32, 1_u32, pixels).expect("4x1 is a valid shape");
        assert_eq!(
            transparent_alpha_pixels(&img),
            2_usize,
            "0 and 127 are below 128, 128 and 255 are not"
        );
    }

    /// The flat arguments' wire shape deserializes with the documented flags.
    #[test]
    fn flat_arguments_deserialize() {
        let args: ImportFlatParams = serde_json::from_value(serde_json::json!({
            "source": "/x/y.png",
            "name": "FLAT",
            "palette": 3_u64,
            "resize": "nearest",
            "size": 128_u64,
            "dither": "ordered",
            "replace": true,
        }))
        .expect("the wire form");
        assert_eq!(args.name.as_deref(), Some("FLAT"));
        assert_eq!(args.palette, Some(3_usize));
        assert_eq!(args.resize.as_deref(), Some("nearest"));
        assert_eq!(args.size, Some(128_u32));
        assert_eq!(args.dither, Some(Dither::Ordered));
        assert!(args.replace, "the flag carries");
    }
}
