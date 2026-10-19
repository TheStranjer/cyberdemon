//! The one-shot wall texture: one picture in, patch + `PNAMES` entry +
//! texture definition out, in one undoable edit.
//!
//! [`create_texture_run`] flattens [`ProcessParams`] into its arguments:
//! [`process`] runs the user's ops, [`choose_size`] settles the texture
//! size (explicit dimensions win, a missing one follows the picture's
//! aspect ratio rounded to a multiple of `8`, and a picture that cannot
//! tile falls back to `128x128` with a note), an automatic `Fit` and
//! optional `MakeTileable` op shape the picture, [`to_indexed`] quantizes
//! it against the session palette with [`fill_low_alpha`] keeping the wall
//! opaque, and [`encode_patch`] stores it with zero offsets. The
//! collision rules ([`check_patch_clash`] and the texture-name refusal)
//! run before anything is written; [`plan_create`] and [`commit_create`]
//! reuse the [`crate::tools::textures`] internals behind `build_texture`
//! so patch, `PNAMES` and the definition land as ONE
//! [`crate::session::Session::edit`] labelled `create_texture NAME`. An
//! `assign_role` request afterwards points a theme wall role at the new
//! texture; theme state is session state and is not part of the undo
//! history, which the result notes say.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::composite::{Component, compose};
use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::ops::{FitMode, ImageOp, OpsReport, apply_ops, rounded_div};
use crate::gfx::palette::Palette;
use crate::gfx::patch::{Patch, encode_patch};
use crate::gfx::quantize::Transparency;
use crate::session::{LayerRole, Session, Source};
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{LumpWrite, put_lump};
use crate::tools::imports::processed::{
    ProcessParams, Processed, fill_low_alpha, process, to_indexed,
};
use crate::tools::ok;
use crate::tools::palette::load_palette;
use crate::tools::textures::{
    Placement, TextureSet, checked_name, definition, lump_copy, missing_names, place_definition,
    put_global, target_lump,
};
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;
use crate::wad::textures::{write_pnames, write_texture_lump};

/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";
/// Smallest texture width a vanilla engine tiles cleanly.
const MIN_WIDTH: u32 = 16_u32;
/// Largest texture edge this tool accepts.
const MAX_TEXTURE_EDGE: u32 = 1024_u32;
/// The widths a created texture may carry: powers of two in the range.
const VALID_WIDTHS: [u32; 7] = [16_u32, 32, 64, 128, 256, 512, 1024];
/// The square fallback size for a picture that cannot keep its shape.
const FALLBACK_SIZE: u32 = 128_u32;
/// Rounding step for a derived texture edge.
const SIZE_ROUND_STEP: u32 = 8_u32;
/// Media type of the published preview.
const PNG_MIME: &str = "image/png";

/// What every successful create reports as the way forward.
const NEXT_STEPS: [&str; 2] = [
    "apply with edit_sidedef (texture fields) or auto_texture mode retexture after assign_role",
    "render_view to see it in place",
];

/// The full documentation of
/// [`crate::server::Cyberdemon::create_texture_from_image`].
pub(in crate::tools) const CREATE_TEXTURE_TOOL_DESCRIPTION: &str = "One call from a picture file to a working Doom wall texture: it processes the image (the same ops/dither/palette parameters the preview pipeline takes, see skill/reference/imports.md), settles a tiling-friendly size, quantizes it to the session PLAYPAL, stores it as a patch lump, lists the patch in PNAMES and defines a single-patch TEXTURE1/TEXTURE2 record - all as ONE undoable edit labelled \"create_texture NAME\" (save_wad writes the result; one undo removes patch, PNAMES entry and definition together). source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL (the processed URL of an earlier process_image call is a valid source). name is the texture name (1 to 8 printable ASCII characters, uppercased); patch_name defaults to it. Size: explicit width (a power of two in 16..=1024, else invalid_argument naming the nearest valid widths) and height (1..=1024) win; a missing one follows the picture's aspect ratio rounded to a multiple of 8 (a derived width to the nearest valid power of two); with neither, a processed picture whose width is a power of two in 16..=1024 and height at most 1024 keeps its size, else it is fit to 128x128 and a note says so. fit is \"cover\" (default), \"contain\" or \"stretch\" for that automatic resize step; tileable=true appends a make_tileable op blending opposite edges. Walls are opaque: pixels whose alpha is below 128 become the palette colour nearest to black and a warning counts them. ops, dither and palette behave exactly as in process_image. Collisions refuse before anything is written: a patch lump patch_name a lower (IWAD or resource) layer holds and the working archive does not is already_exists (pass another patch_name, or replace=true to write a working override), and a texture name the merged view already defines is already_exists unless replace=true redefines it in place. target is \"TEXTURE1\" or \"TEXTURE2\" (case-insensitive); unset means the lump that already defines name, else TEXTURE1. assign_role (e.g. \"wall\") afterwards points that theme role at the new texture: wall roles only - a flat or unknown role is invalid_argument and nothing is written; theme state is session state, so undo does not roll the override back (the result notes say). Returns {texture, patch, width, height, lump, steps, colors_used, max_color_error, notes, warnings, preview, role, next_steps}: steps is one measurement object per applied op (user ops first, then the automatic ones), preview is the composited texture's PNG asset URL and role the assigned role, if any.";

/// Arguments for [`crate::server::Cyberdemon::create_texture_from_image`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct CreateTextureParams {
    /// Picture to turn into a wall texture: absolute path, `file://` URI,
    /// `http(s)://` URL or this server's asset/upload URL.
    pub(in crate::tools) source: String,
    /// Texture name: 1 to 8 printable `ASCII` characters, uppercased.
    pub(in crate::tools) name: String,
    /// Patch lump name to store the picture under (default: `name`).
    #[serde(default)]
    pub(in crate::tools) patch_name: Option<String>,
    /// Which lump defines the texture: `"TEXTURE1"` or `"TEXTURE2"`
    /// (case-insensitive). Unset means the lump that already defines
    /// `name`, else `TEXTURE1`.
    #[serde(default)]
    pub(in crate::tools) target: Option<String>,
    /// Theme wall role to point at the new texture afterwards (e.g.
    /// `"wall"`); a flat or unknown role is `invalid_argument`.
    #[serde(default)]
    pub(in crate::tools) assign_role: Option<String>,
    /// Redefine an existing texture of this name in place and allow
    /// overriding a layer-held patch name (default `false`: such clashes
    /// are `already_exists`).
    #[serde(default)]
    pub(in crate::tools) replace: bool,
    /// The size side: `width`, `height`, `fit` and `tileable` (see
    /// `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) size: TextureSizeParams,
    /// The shared processing parameters: `ops`, `dither` and `palette`
    /// (see `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) shared: ProcessParams,
}

/// The size fields of [`CreateTextureParams`], flattened into its
/// arguments and kept as one struct so each stays under the flag budget.
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct TextureSizeParams {
    /// Texture width in pixels: a power of two in `16..=1024` (vanilla
    /// tiling). Unset keeps the processed size when it fits, else the
    /// aspect-derived or `128` width.
    #[serde(default)]
    pub(in crate::tools) width: Option<u32>,
    /// Texture height in pixels: `1..=1024`. Unset follows the picture's
    /// aspect ratio rounded to a multiple of `8`, or `128`.
    #[serde(default)]
    pub(in crate::tools) height: Option<u32>,
    /// How the picture maps onto the texture rectangle for the automatic
    /// resize: `"cover"` (default), `"contain"` or `"stretch"`.
    #[serde(default)]
    pub(in crate::tools) fit: Option<FitMode>,
    /// Append a `make_tileable` op blending opposite edges so the texture
    /// tiles seamlessly (default `false`).
    #[serde(default)]
    pub(in crate::tools) tileable: bool,
}

/// The validated arguments of one create call.
#[derive(Debug)]
struct CreateRequest {
    /// The `source` to fetch.
    source: String,
    /// The texture name, uppercased.
    name: String,
    /// The patch lump name to store under.
    patch: LumpName,
    /// The explicit width, when validated.
    width: Option<u32>,
    /// The explicit height, when validated.
    height: Option<u32>,
    /// How the picture maps onto the texture rectangle.
    fit: FitMode,
    /// Whether a `make_tileable` op follows the fit.
    tileable: bool,
    /// The requested `target` word, if any.
    target: Option<String>,
    /// The theme role to assign afterwards, if any.
    role: Option<Role>,
    /// Whether existing names may be overwritten.
    replace: bool,
    /// The shared processing parameters.
    shared: ProcessParams,
}

/// Validate the create arguments: both names, the explicit size bounds
/// and the theme role. Every refusal here happens before the `source` is
/// fetched.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, a width that is no
/// power of two in `16..=1024`, a height outside `1..=1024`, or an
/// unknown or flat `assign_role`.
fn parse_create(args: &CreateTextureParams) -> Result<CreateRequest, CyberError> {
    let name = checked_name("texture", &args.name)?;
    let patch_text = args.patch_name.clone().unwrap_or_else(|| name.clone());
    let patch = LumpName::parse(&patch_text).map_err(|error| {
        CyberError::InvalidArgument(format!("patch_name {patch_text:?} is not valid: {error}"))
    })?;
    Ok(CreateRequest {
        source: args.source.clone(),
        name,
        patch,
        width: args.size.width.map(check_width).transpose()?,
        height: args.size.height.map(check_height).transpose()?,
        fit: args.size.fit.unwrap_or(FitMode::Cover),
        tileable: args.size.tileable,
        target: args.target.clone(),
        role: parse_assign_role(args.assign_role.as_deref())?,
        replace: args.replace,
        shared: args.shared.clone(),
    })
}

/// The explicit `width`: a power of two in `16..=1024`; the refusal names
/// the nearest valid widths.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other value.
fn check_width(value: u32) -> Result<u32, CyberError> {
    if value.is_power_of_two() && (MIN_WIDTH..=MAX_TEXTURE_EDGE).contains(&value) {
        return Ok(value);
    }
    Err(CyberError::InvalidArgument(format!(
        "width {value} is not a power of two in {MIN_WIDTH}..={MAX_TEXTURE_EDGE}: pass one \
         of the nearest valid widths {} (vanilla engines tile a texture by its width)",
        nearest_widths(value),
    )))
}

/// The explicit `height`: `1..=1024`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other value.
fn check_height(value: u32) -> Result<u32, CyberError> {
    if value == 0 || value > MAX_TEXTURE_EDGE {
        return Err(CyberError::InvalidArgument(format!(
            "height {value} is out of range: pass a value within 1..={MAX_TEXTURE_EDGE}"
        )));
    }
    Ok(value)
}

/// The two [`VALID_WIDTHS`] nearest `value`, as `"64 or 128"` (one name
/// at the ends of the range).
fn nearest_widths(value: u32) -> String {
    let below = VALID_WIDTHS.iter().copied().rfind(|w| *w <= value);
    let above = VALID_WIDTHS.iter().copied().find(|w| *w >= value);
    let mut names: Vec<String> = Vec::new();
    for width in below.into_iter().chain(above) {
        let text = width.to_string();
        if !names.contains(&text) {
            names.push(text);
        }
    }
    names.join(" or ")
}

/// The [`VALID_WIDTHS`] member nearest `value`; ties take the smaller.
fn nearest_width(value: u32) -> u32 {
    VALID_WIDTHS
        .iter()
        .copied()
        .min_by_key(|width| {
            width
                .saturating_sub(value)
                .max(value.saturating_sub(*width))
        })
        .unwrap_or(FALLBACK_SIZE)
}

/// The `assign_role` word: a parseable wall role.
///
/// # Errors
///
/// [`Role::parse`]' [`CyberError::InvalidArgument`] for an unknown name,
/// and [`CyberError::InvalidArgument`] naming the flat tools for a flat
/// role.
fn parse_assign_role(text: Option<&str>) -> Result<Option<Role>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let role = Role::parse(raw)?;
    if role.is_flat() {
        return Err(CyberError::InvalidArgument(format!(
            "role {raw:?} is a flat role: create_texture_from_image defines wall textures - \
             import a flat with import_image_as_flat and point flat roles at it with \
             set_texture_theme"
        )));
    }
    Ok(Some(role))
}

/// The texture size one build writes, with the note the fallback rule
/// adds.
#[derive(Debug)]
struct WallSize {
    /// Texture width in pixels.
    width: u32,
    /// Texture height in pixels.
    height: u32,
    /// Why a picture that could not keep its own size got `128x128`.
    note: Option<String>,
}

/// The size rule over the processed picture: explicit dimensions (already
/// validated by [`parse_create`]) win, a missing one follows the picture's
/// aspect ratio, and with neither a tileable picture is kept and any other
/// falls back to [`FALLBACK_SIZE`] square with a note.
fn choose_size(request: &CreateRequest, picture: &RgbaImage) -> WallSize {
    match (request.width, request.height) {
        (Some(width), Some(height)) => WallSize {
            width,
            height,
            note: None,
        },
        (Some(width), None) => WallSize {
            width,
            height: derive_height(width, picture),
            note: None,
        },
        (None, Some(height)) => WallSize {
            width: derive_width(height, picture),
            height,
            note: None,
        },
        (None, None) => keep_or_fallback(picture),
    }
}

/// The aspect-derived height for an explicit `width`, rounded to a
/// multiple of [`SIZE_ROUND_STEP`] and kept inside the edge limits.
fn derive_height(width: u32, picture: &RgbaImage) -> u32 {
    let scaled = rounded_div(
        u64::from(width).saturating_mul(u64::from(picture.height)),
        u64::from(picture.width),
    );
    let exact = u32::try_from(scaled).unwrap_or(MAX_TEXTURE_EDGE);
    round_to_step(exact).clamp(SIZE_ROUND_STEP, MAX_TEXTURE_EDGE)
}

/// The aspect-derived width for an explicit `height`, taken as the
/// nearest member of [`VALID_WIDTHS`].
fn derive_width(height: u32, picture: &RgbaImage) -> u32 {
    let scaled = rounded_div(
        u64::from(height).saturating_mul(u64::from(picture.width)),
        u64::from(picture.height),
    );
    nearest_width(u32::try_from(scaled).unwrap_or(u32::MAX))
}

/// Keep the processed size when a vanilla engine can tile it, else the
/// square fallback with its note.
fn keep_or_fallback(picture: &RgbaImage) -> WallSize {
    let keeps = picture.width.is_power_of_two()
        && (MIN_WIDTH..=MAX_TEXTURE_EDGE).contains(&picture.width)
        && picture.height <= MAX_TEXTURE_EDGE;
    if keeps {
        return WallSize {
            width: picture.width,
            height: picture.height,
            note: None,
        };
    }
    WallSize {
        width: FALLBACK_SIZE,
        height: FALLBACK_SIZE,
        note: Some(format!(
            "resized to {FALLBACK_SIZE}x{FALLBACK_SIZE}; pass width/height to choose"
        )),
    }
}

/// `value` to the nearest multiple of [`SIZE_ROUND_STEP`].
fn round_to_step(value: u32) -> u32 {
    let half = SIZE_ROUND_STEP.checked_div(2).unwrap_or(0_u32);
    value
        .saturating_add(half)
        .checked_div(SIZE_ROUND_STEP)
        .and_then(|steps| steps.checked_mul(SIZE_ROUND_STEP))
        .unwrap_or(value)
}

/// The automatic `Fit` (when the size changed) and `MakeTileable` (when
/// asked) ops, appended to the processed picture and its report.
///
/// # Errors
///
/// [`apply_ops`]' [`CyberError::InvalidArgument`] widened to say the
/// failure came from the automatic step, not the user's ops.
fn shape_picture(
    processed: Processed,
    size: &WallSize,
    fit: FitMode,
    tileable: bool,
) -> Result<Processed, CyberError> {
    let mut extra: Vec<ImageOp> = Vec::new();
    if (processed.rgba.width, processed.rgba.height) != (size.width, size.height) {
        extra.push(ImageOp::Fit {
            width: size.width,
            height: size.height,
            mode: fit,
            filter: crate::gfx::ops::Filter::Area,
            background: None,
        });
    }
    if tileable {
        extra.push(ImageOp::MakeTileable {
            band: None,
            horizontal: true,
            vertical: true,
        });
    }
    if extra.is_empty() {
        return Ok(processed);
    }
    let (rgba, second) = apply_ops(processed.rgba, &extra).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "the automatic resize/tiling step failed: {error}: adjust width/height or pass \
             tileable=false"
        ))
    })?;
    let mut steps = processed.report.steps;
    steps.extend(second.steps);
    Ok(Processed {
        rgba,
        report: OpsReport { steps },
    })
}

/// The shaped, quantized and encoded picture, ready to plan and commit.
struct Built {
    /// Texture width in pixels.
    width: u32,
    /// Texture height in pixels.
    height: u32,
    /// The encoded `Doom` patch bytes with zero offsets.
    patch_bytes: Vec<u8>,
    /// The composited texture's `PNG` bytes.
    preview_png: Vec<u8>,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct opaque source colours.
    colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    max_color_error: i32,
    /// How the size was chosen.
    notes: Vec<String>,
    /// Non-fatal picture facts.
    warnings: Vec<String>,
}

/// Run the whole picture side: user ops, the size rule, the automatic
/// shape ops, opaque quantization, patch encoding and the composited
/// preview. The palette read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors for the bytes and the user ops, [`load_palette`]'
/// [`CyberError::NotFound`], [`shape_picture`]' and [`encode_patch`]'
/// errors and [`composite_png`]' errors.
fn build_picture(
    state: &AppState,
    request: &CreateRequest,
    fetched: &Fetched,
) -> Result<Built, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let size = choose_size(request, &processed.rgba);
    let shaped = shape_picture(processed, &size, request.fit, request.tileable)?;
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let dither = request.shared.dither_choice();
    let mut indexed = to_indexed(&shaped, &palette, Transparency::None, dither);
    let filled = fill_low_alpha(&mut indexed.image, &shaped.rgba, &palette);
    let patch = Patch {
        left_offset: 0_i16,
        top_offset: 0_i16,
        image: indexed.image.clone(),
    };
    let patch_bytes = encode_patch(&patch)?;
    let preview_png = composite_png(&request.name, &indexed.image, &palette)?;
    Ok(Built {
        width: size.width,
        height: size.height,
        patch_bytes,
        preview_png,
        steps: shaped.report.steps,
        colors_used: indexed.colors_used,
        max_color_error: indexed.max_color_error,
        notes: size.note.into_iter().collect(),
        warnings: filled_warning(filled),
    })
}

/// The warning for repainted holes, empty for a fully opaque picture.
fn filled_warning(filled: usize) -> Vec<String> {
    if filled == 0 {
        return Vec::new();
    }
    vec![format!(
        "{filled} pixels below alpha 128 became the palette colour nearest to black: walls are \
         opaque - use import_image_as_patch for a masked patch"
    )]
}

/// The texture's `PNG` through the compositor: the record's single patch
/// folded onto its canvas and coloured with the quantization palette.
///
/// # Errors
///
/// [`compose`]' and [`encode_png`]' errors.
fn composite_png(
    name: &str,
    image: &IndexedImage,
    palette: &Palette,
) -> Result<Vec<u8>, CyberError> {
    let component = Component {
        name,
        image: Some(image),
        x: 0_i32,
        y: 0_i32,
    };
    let composite = compose(name, image.width, image.height, &[component])?;
    encode_png(&composite.image.to_rgba(palette), None)
}

/// The lumps one commit writes, planned before anything changes.
#[derive(Debug)]
struct Plan {
    /// The grown `PNAMES` bytes, when this call appends a name.
    pnames_bytes: Option<Vec<u8>>,
    /// The re-encoded target texture lump.
    texture_bytes: Vec<u8>,
    /// The raw directory name of the target lump.
    target_raw: [u8; 8],
    /// The target lump's wire name.
    lump: &'static str,
}

/// Plan the whole write: the collision rules first, then the grown
/// `PNAMES` list and the definition over a copy of the target lump.
///
/// # Errors
///
/// [`check_patch_clash`]' and the texture-name rule's
/// [`CyberError::AlreadyExists`], [`target_lump`]'
/// [`CyberError::InvalidArgument`], [`TextureSet::load`]'
/// [`CyberError::Format`] and the codecs' errors.
fn plan_create(
    session: &Session,
    request: &CreateRequest,
    built: &Built,
) -> Result<Plan, CyberError> {
    check_patch_clash(session, request.patch, request.replace)?;
    let set = TextureSet::load(session)?;
    if set.find(&request.name).is_some() && !request.replace {
        return Err(CyberError::AlreadyExists(format!(
            "texture {} is already defined in the merged view: pass replace=true to redefine \
             it in place, or another name",
            request.name
        )));
    }
    let target = target_lump(&set, request.target.as_deref(), &request.name)?;
    let placements = [Placement {
        name: request.patch.to_string(),
        x: 0_i16,
        y: 0_i16,
    }];
    let added = missing_names(set.pnames(), &placements);
    let mut pnames: Vec<String> = set.pnames().to_vec();
    pnames.extend(added);
    let def = definition(&request.name, texture_size(built)?, &placements, &pnames)?;
    let mut lump = lump_copy(&set, target);
    place_definition(&mut lump, target, def, request.replace)?;
    Ok(Plan {
        pnames_bytes: (pnames != *set.pnames())
            .then(|| write_pnames(&pnames))
            .transpose()?,
        texture_bytes: write_texture_lump(&lump)?,
        target_raw: target.as_raw(),
        lump: target.as_str(),
    })
}

/// The `(width, height)` pair a texture record stores, refusing an edge
/// past the range it can hold.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a value outside `u16` (the size
/// rules keep this unreachable in a request the tool accepts; the check
/// keeps a broken size from storing a wrapped dimension).
fn texture_size(built: &Built) -> Result<(u16, u16), CyberError> {
    let width = u16::try_from(built.width).map_err(|_conversion| oversize("width", built.width))?;
    let height =
        u16::try_from(built.height).map_err(|_conversion| oversize("height", built.height))?;
    Ok((width, height))
}

/// The refusal for an oversized texture edge.
fn oversize(what: &str, value: u32) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{what} {value} is past the {MAX_TEXTURE_EDGE}-pixel texture limit"
    ))
}

/// Refuse a patch name a read-only layer holds while the working archive
/// does not - unless `replace` was given. A working-name twin is
/// `put_lump`' own refusal, with its own hint.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] naming the layer and the way out.
fn check_patch_clash(session: &Session, patch: LumpName, replace: bool) -> Result<(), CyberError> {
    if replace || session.working().find_last(patch).is_some() {
        return Ok(());
    }
    let Some(entry) = session.find(patch, None) else {
        return Ok(());
    };
    match entry.source {
        Source::Working => Ok(()),
        Source::Layer(index) => Err(CyberError::AlreadyExists(format!(
            "a patch named {patch} already exists in {} and other textures use it: pass \
             patch_name to store yours under another name, or replace=true to override the \
             layer's copy in the working archive",
            layer_label(session, index),
        ))),
    }
}

/// What a layer reads as in a message: the base game archive or a
/// resource, by its position in the stack.
pub(in crate::tools) fn layer_label(session: &Session, index: usize) -> String {
    session.layers().get(index).map_or_else(
        || format!("layer {index}"),
        |layer| match layer.role {
            LayerRole::Iwad => "the IWAD".to_owned(),
            LayerRole::Resource => format!("resource layer {index}"),
        },
    )
}

/// The synchronous half: patch, `PNAMES` and definition as ONE undoable
/// edit.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`put_global`]' stale-index [`CyberError::Internal`].
fn commit_create(
    session: &mut Session,
    request: &CreateRequest,
    built: &Built,
    plan: &Plan,
) -> Result<(), CyberError> {
    let label = format!("create_texture {}", request.name);
    let write = LumpWrite {
        name: request.patch,
        data: &built.patch_bytes,
        namespace: Some(Namespace::Patches),
        replace: request.replace,
    };
    session.edit(&label, |wad| {
        put_lump(wad, &write)?;
        if let Some(bytes) = plan.pnames_bytes.as_ref() {
            put_global(wad, LumpName::from_raw(PNAMES_RAW), bytes.clone())?;
        }
        put_global(
            wad,
            LumpName::from_raw(plan.target_raw),
            plan.texture_bytes.clone(),
        )?;
        Ok(())
    })?;
    Ok(())
}

/// Point the requested theme role at the new texture. Theme state is
/// session state: no undo step records it.
///
/// # Errors
///
/// [`AppState::with_session`]' lock errors.
fn apply_role(
    state: &AppState,
    request: &CreateRequest,
) -> Result<Option<&'static str>, CyberError> {
    let Some(role) = request.role else {
        return Ok(None);
    };
    let name = request.name.clone();
    state.with_session(move |session| {
        session.texture_theme_mut().overrides.insert(role, name);
        Ok(Some(role.as_str()))
    })
}

/// The committed facts a report is assembled from.
struct Landed {
    /// The lump that now defines the texture.
    lump: &'static str,
    /// The role the theme now points at the texture, if any.
    role: Option<&'static str>,
    /// The published composited preview.
    preview: AssetRef,
}

/// Structured result of
/// [`crate::server::Cyberdemon::create_texture_from_image`].
#[derive(Debug, Serialize)]
struct CreatedTextureReport {
    /// The texture name, uppercased.
    texture: String,
    /// The patch lump name the picture was stored under.
    patch: String,
    /// The texture width in pixels.
    width: u32,
    /// The texture height in pixels.
    height: u32,
    /// The lump that now defines the texture.
    lump: &'static str,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct opaque source colours.
    colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    max_color_error: i32,
    /// How the size was chosen and what the theme override means.
    notes: Vec<String>,
    /// Non-fatal picture facts.
    warnings: Vec<String>,
    /// The composited texture's `PNG`, ready to look at.
    preview: AssetRef,
    /// The theme role now standing for this texture, if any.
    role: Option<&'static str>,
    /// What to do next.
    next_steps: &'static [&'static str],
}

/// Fold the request, the built picture and the landed facts into the
/// documented result.
fn report_of(request: &CreateRequest, built: &Built, landed: Landed) -> CreatedTextureReport {
    let mut notes = built.notes.clone();
    if let Some(role) = landed.role {
        notes.push(format!(
            "theme override {role} -> {} set in session state: undo does not roll it back, \
             clear it with set_texture_theme overrides",
            request.name
        ));
    }
    CreatedTextureReport {
        texture: request.name.clone(),
        patch: request.patch.to_string(),
        width: built.width,
        height: built.height,
        lump: landed.lump,
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        max_color_error: built.max_color_error,
        notes,
        warnings: built.warnings.clone(),
        preview: landed.preview,
        role: landed.role,
        next_steps: &NEXT_STEPS,
    }
}

/// `create_texture_from_image`' work: validate, fetch and process, plan,
/// commit as one edit, then the theme override and the preview publish.
pub(in crate::tools) async fn create_texture_run(
    state: &AppState,
    args: &CreateTextureParams,
) -> Result<CallToolResult, ErrorData> {
    match create_texture_build(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The measured body of [`create_texture_run`].
///
/// # Errors
///
/// [`parse_create`]' argument errors, [`AppState::fetch_source`]' and
/// [`build_picture`]' errors, [`plan_create`]' and [`commit_create`]'
/// refusals, [`apply_role`]' and [`AppState::publish`]' errors; a refusal
/// leaves the working archive untouched.
async fn create_texture_build(
    state: &AppState,
    args: &CreateTextureParams,
) -> Result<CreatedTextureReport, CyberError> {
    let request = parse_create(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let built = build_picture(state, &request, &fetched)?;
    let plan = state.with_session(|session| plan_create(session, &request, &built))?;
    let lump = plan.lump;
    state.with_session(|session| commit_create(session, &request, &built, &plan))?;
    let role = apply_role(state, &request)?;
    let preview = state.publish(
        &format!("{}_texture.png", request.name),
        PNG_MIME,
        built.preview_png.clone(),
    )?;
    let landed = Landed {
        lump,
        role,
        preview,
    };
    Ok(report_of(&request, &built, landed))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        CreateTextureParams, TextureSizeParams, apply_role, check_height, check_width, choose_size,
        composite_png, create_texture_build, keep_or_fallback, nearest_width, nearest_widths,
        parse_assign_role, parse_create, plan_create, round_to_step, shape_picture, texture_size,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::{IndexedImage, RgbaImage};
    use crate::gfx::ops::FitMode;
    use crate::gfx::palette::Palette;
    use crate::session::{Layer, LayerRole, Session};
    use crate::state::AppState;
    use crate::themes::Role;
    use crate::tools::imports::current_pnames;
    use crate::tools::imports::processed::ProcessParams;
    use crate::tools::textures::{TextureSet, WhichLump};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;
    use crate::wad::textures::{
        PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
    };

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `width` x `height` opaque test picture.
    fn picture(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count).flat_map(|_| [20_u8, 40, 60, 255]).collect();
        RgbaImage::new(width, height, pixels).expect("test shape is valid")
    }

    /// Parameters over `source`/`name` with the given size fields.
    fn args(width: Option<u32>, height: Option<u32>) -> CreateTextureParams {
        CreateTextureParams {
            source: "/x/y.png".to_owned(),
            name: "MYTEX".to_owned(),
            patch_name: None,
            target: None,
            assign_role: None,
            replace: false,
            size: TextureSizeParams {
                width,
                height,
                fit: None,
                tileable: false,
            },
            shared: ProcessParams {
                ops: Vec::new(),
                dither: None,
                palette: None,
            },
        }
    }

    /// A request over explicit dimensions only.
    fn sized(width: Option<u32>, height: Option<u32>) -> super::CreateRequest {
        parse_create(&args(width, height)).expect("test requests are valid")
    }

    /// A request naming both halves explicitly.
    fn request_for(texture: &str, patch: &str) -> super::CreateRequest {
        let mut call = args(None, None);
        call.name = texture.to_owned();
        call.patch_name = Some(patch.to_owned());
        parse_create(&call).expect("test requests are valid")
    }

    #[test]
    fn widths_take_the_powers_of_two_and_name_the_nearest() {
        for width in [16_u32, 32, 64, 128, 256, 512, 1024] {
            assert_eq!(check_width(width).expect("a valid width"), width);
        }
        let error = check_width(100_u32).expect_err("no 100");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("64 or 128"),
            "the message names the neighbours: {error}"
        );
        assert_eq!(nearest_widths(2048_u32), "1024", "above the top end");
        assert_eq!(nearest_widths(8_u32), "16", "below the bottom end");
        assert_eq!(nearest_width(96_u32), 64_u32, "ties take the smaller");
        assert_eq!(nearest_width(100_u32), 128_u32, "closer above wins");
    }

    #[test]
    fn heights_take_the_i16_comfortable_range() {
        assert_eq!(check_height(1_u32).expect("one row"), 1_u32);
        assert_eq!(
            check_height(super::MAX_TEXTURE_EDGE).expect("the edge"),
            1024_u32
        );
        assert_eq!(
            code(&check_height(0_u32).expect_err("zero")),
            "invalid_argument"
        );
        let error = check_height(1025_u32).expect_err("past the edge");
        assert!(error.to_string().contains("1..=1024"), "the range: {error}");
    }

    #[test]
    fn round_to_step_neighbours_the_multiple() {
        assert_eq!(round_to_step(20_u32), 24_u32, "the tie rounds up");
        assert_eq!(round_to_step(24_u32), 24_u32, "an exact step stays");
        assert_eq!(
            round_to_step(3_u32),
            0_u32,
            "zero is the nearest multiple of 3"
        );
        assert_eq!(round_to_step(19_u32), 16_u32, "19 sits nearer 16");
    }

    #[test]
    fn the_size_rule_keeps_tileable_pictures_and_falls_back_otherwise() {
        let request = sized(None, None);
        let kept = choose_size(&request, &picture(64_u32, 128_u32));
        assert_eq!(
            (kept.width, kept.height),
            (64_u32, 128_u32),
            "a tileable size stays"
        );
        assert!(kept.note.is_none(), "no note for a kept size");
        let fallback = choose_size(&request, &picture(100_u32, 60_u32));
        assert_eq!(
            (fallback.width, fallback.height),
            (128_u32, 128_u32),
            "the square fallback"
        );
        assert_eq!(
            fallback.note.as_deref(),
            Some("resized to 128x128; pass width/height to choose"),
            "the documented note"
        );
        let too_tall = choose_size(&request, &picture(64_u32, 1025_u32));
        assert_eq!(
            (too_tall.width, too_tall.height),
            (128_u32, 128_u32),
            "height caps out"
        );
        let tiny = keep_or_fallback(&picture(8_u32, 8_u32));
        assert_eq!(
            (tiny.width, tiny.height),
            (128_u32, 128_u32),
            "under 16 falls back"
        );
    }

    /// Assert the size rule over a `picture` answers `(width, height)`.
    fn assert_chosen(request: &super::CreateRequest, image: &RgbaImage, edges: (u32, u32)) {
        let chosen = choose_size(request, image);
        assert_eq!((chosen.width, chosen.height), edges, "the chosen size");
    }

    #[test]
    fn a_missing_edge_follows_the_aspect_ratio() {
        assert_chosen(
            &sized(Some(128_u32), None),
            &picture(64_u32, 128_u32),
            (128_u32, 256_u32),
        );
        assert_chosen(
            &sized(Some(128_u32), None),
            &picture(100_u32, 60_u32),
            (128_u32, 80_u32),
        );
        assert_chosen(
            &sized(None, Some(256_u32)),
            &picture(64_u32, 128_u32),
            (128_u32, 256_u32),
        );
        assert_chosen(
            &sized(Some(1024_u32), None),
            &picture(8192_u32, 1_u32),
            (1024_u32, 8_u32),
        );
    }

    #[test]
    fn explicit_edges_are_validated_before_anything_else() {
        let mut call = args(Some(100_u32), Some(0_u32));
        call.patch_name = Some("too long for wad".to_owned());
        assert_eq!(
            code(&parse_create(&call).expect_err("bad patch name")),
            "invalid_argument"
        );
        call.patch_name = None;
        let error = parse_create(&call).expect_err("bad width");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("width 100"),
            "names the field: {error}"
        );
        call.size.width = Some(64_u32);
        assert_eq!(
            code(&parse_create(&call).expect_err("bad height")),
            "invalid_argument"
        );
    }

    #[test]
    fn the_patch_name_defaults_to_the_texture_name_uppercased() {
        let mut call = args(None, None);
        call.name = "mytex".to_owned();
        let request = parse_create(&call).expect("valid");
        assert_eq!(request.name, "MYTEX", "the texture name uppercases");
        assert_eq!(request.patch, name("MYTEX"), "the patch follows");
        call.patch_name = Some("mypatch".to_owned());
        assert_eq!(parse_create(&call).expect("valid").patch, name("MYPATCH"));
    }

    #[test]
    fn roles_must_parse_and_stay_on_the_walls() {
        assert!(parse_assign_role(None).expect("absent").is_none());
        assert_eq!(
            parse_assign_role(Some("WALL_ALT")).expect("a wall role"),
            Some(Role::WallAlt)
        );
        let error = parse_assign_role(Some("floor")).expect_err("a flat role");
        assert_eq!(code(&error), "invalid_argument");
        let message = error.to_string();
        assert!(message.contains("flat"), "the message says why: {message}");
        assert!(
            message.contains("import_image_as_flat"),
            "and names the way out: {message}"
        );
        assert_eq!(
            code(&parse_assign_role(Some("roof")).expect_err("no such role")),
            "invalid_argument",
            "an unknown role refuses too"
        );
    }

    #[test]
    fn the_wire_form_flattens_ops_size_and_shared_fields() {
        let call: CreateTextureParams = serde_json::from_value(json!({
            "source": "/x/y.png",
            "name": "MYTEX",
            "width": 256_u64,
            "fit": "stretch",
            "tileable": true,
            "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
            "dither": "ordered",
            "palette": 2_u64,
            "assign_role": "wall",
            "replace": true,
        }))
        .expect("the documented wire form");
        assert_eq!(call.size.width, Some(256_u32));
        assert_eq!(call.size.fit, Some(FitMode::Stretch));
        assert!(call.size.tileable);
        assert_eq!(call.shared.ops.len(), 1, "the op list carries");
        assert_eq!(call.shared.palette, Some(2_usize));
        assert!(call.replace);
    }

    /// Fixture-palette bytes: two entries of `(i, 255 - i, i)`.
    fn playpal() -> Vec<u8> {
        (0_u32..512_u32)
            .map(|position| {
                u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                    .expect("remainders fit u8")
            })
            .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
            .collect()
    }

    /// A `PNAMES` lump listing the fixture pair.
    fn pnames_lump() -> Lump {
        Lump::new(
            name("PNAMES"),
            write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned()])
                .expect("test names are valid"),
        )
    }

    /// A `TEXTURE1` lump defining `WALL1`.
    fn texture1_lump() -> Lump {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![TextureDef {
                name: "WALL1".to_owned(),
                flags_raw: 0,
                width: 64,
                height: 128,
                patches: vec![PatchRef {
                    patch: 0,
                    x: 0,
                    y: 0,
                }],
            }],
        };
        Lump::new(
            name("TEXTURE1"),
            write_texture_lump(&lump).expect("serializable"),
        )
    }

    /// A session on one `IWAD` layer holding the fixture texture set.
    fn session() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::new(name("PLAYPAL"), playpal()),
                    pnames_lump(),
                    texture1_lump(),
                    Lump::new(name("WALLP1"), vec![b'P']),
                ],
            },
        ));
        session
    }

    /// A built picture with dummy bytes for the plan and commit tests.
    fn make_built(width: u32, height: u32) -> super::Built {
        super::Built {
            width,
            height,
            patch_bytes: vec![1_u8, 2, 3],
            preview_png: vec![4_u8],
            steps: Vec::new(),
            colors_used: 1,
            max_color_error: 0,
            notes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_plan_over_a_free_name_grows_pnames_and_the_target_lump() {
        let session = session();
        let request = request_for("MYTEX", "MYTEX");
        let plan =
            plan_create(&session, &request, &make_built(64_u32, 128_u32)).expect("the plan runs");
        assert_eq!(plan.lump, "TEXTURE1", "the default target");
        let grown = write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned(), "MYTEX".to_owned()])
            .expect("valid");
        assert_eq!(
            plan.pnames_bytes.as_ref(),
            Some(&grown),
            "the new patch name grows PNAMES"
        );
    }

    #[test]
    fn a_layer_patch_name_clashes_with_a_hint() {
        let session = session();
        let request = request_for("MYTEX", "WALLP1");
        let error = plan_create(&session, &request, &make_built(64_u32, 64_u32))
            .expect_err("the IWAD patch stands");
        assert_eq!(code(&error), "already_exists");
        let message = error.to_string();
        assert!(
            message.contains("a patch named WALLP1 already exists in the IWAD"),
            "the message names the layer: {message}"
        );
        assert!(
            message.contains("pass patch_name"),
            "and the way out: {message}"
        );
    }

    #[test]
    fn a_taken_texture_name_clashes_until_replace() {
        let session = session();
        let taken = request_for("WALL1", "MYTEX");
        let error = plan_create(&session, &taken, &make_built(64_u32, 64_u32))
            .expect_err("the texture stands");
        assert_eq!(code(&error), "already_exists");
        assert!(
            error.to_string().contains("replace=true"),
            "the message names the way out: {error}"
        );
    }

    #[test]
    fn replace_overrides_the_layer_patch_and_redefines_the_texture() {
        let mut session = session();
        let mut request = request_for("WALL1", "WALLP1");
        request.replace = true;
        let built = make_built(64_u32, 128_u32);
        let plan = plan_create(&session, &request, &built).expect("replace runs");
        assert!(plan.pnames_bytes.is_none(), "WALLP1 is already listed");
        super::commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
        let set = TextureSet::load(&session).expect("the working override decodes");
        let (def, which) = set.find("WALL1").expect("still defined");
        assert_eq!(which, WhichLump::Texture1, "updated in place");
        assert_eq!(def.patches.len(), 1, "the single patch replaced the tile");
        assert_eq!(
            session.undo_labels(),
            vec!["create_texture WALL1"],
            "one undoable step"
        );
    }

    #[test]
    fn an_undo_step_takes_patch_pnames_and_texture_together() {
        let mut session = session();
        let request = request_for("MYTEX", "MYTEX");
        let built = make_built(64_u32, 128_u32);
        let plan = plan_create(&session, &request, &built).expect("the plan runs");
        super::commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
        assert!(
            session
                .find(name("MYTEX"), Some(Namespace::Patches))
                .is_some(),
            "the patch serves the merged view"
        );
        session.undo().expect("one step back");
        assert!(
            session
                .find(name("MYTEX"), Some(Namespace::Patches))
                .is_none(),
            "the patch left with its edit"
        );
        assert!(
            TextureSet::load(&session)
                .expect("the fixture lumps decode")
                .find("MYTEX")
                .is_none(),
            "the definition left too"
        );
        assert_eq!(
            current_pnames(&session).expect("the fixture PNAMES decode"),
            vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
            "and the PNAMES append"
        );
    }

    #[test]
    fn an_explicit_target_defines_the_other_lump() {
        let mut session = session();
        let mut request = request_for("MYTEX", "MYTEX");
        request.target = Some("texture2".to_owned());
        let built = make_built(64_u32, 64_u32);
        let plan = plan_create(&session, &request, &built).expect("the plan runs");
        assert_eq!(plan.lump, "TEXTURE2", "case-insensitive target");
        super::commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
        let set = TextureSet::load(&session).expect("the override decodes");
        assert_eq!(
            set.find("MYTEX").expect("defined").1,
            WhichLump::Texture2,
            "TEXTURE2 holds it"
        );
    }

    #[test]
    fn a_bad_target_word_refuses_the_plan() {
        let session = session();
        let mut request = request_for("MYTEX", "MYTEX");
        request.target = Some("TEXTURE3".to_owned());
        let error =
            plan_create(&session, &request, &make_built(64_u32, 64_u32)).expect_err("no such lump");
        assert_eq!(code(&error), "invalid_argument");
    }

    #[test]
    fn an_oversized_edge_refuses_the_record() {
        let huge = make_built(70_000_u32, 64_u32);
        let error = texture_size(&huge).expect_err("past u16");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("width 70000"),
            "the message names the edge: {error}"
        );
        assert_eq!(
            texture_size(&make_built(64_u32, 128_u32)).expect("a stored size"),
            (64_u16, 128_u16)
        );
    }

    #[test]
    fn shape_appends_the_automatic_ops_to_the_report() {
        let processed = super::Processed {
            rgba: picture(100_u32, 60_u32),
            report: crate::gfx::ops::OpsReport::default(),
        };
        let size = super::WallSize {
            width: 128_u32,
            height: 128_u32,
            note: None,
        };
        let shaped = shape_picture(processed, &size, FitMode::Cover, true).expect("both ops run");
        assert_eq!((shaped.rgba.width, shaped.rgba.height), (128_u32, 128_u32));
        let kinds: Vec<&str> = shaped
            .report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(kinds, vec!["fit", "make_tileable"], "fit then tileable");
        let same = super::Processed {
            rgba: picture(128_u32, 128_u32),
            report: crate::gfx::ops::OpsReport::default(),
        };
        let untouched = shape_picture(same, &size, FitMode::Cover, false).expect("no ops needed");
        assert!(
            untouched.report.steps.is_empty(),
            "an equal size adds nothing"
        );
    }

    #[test]
    fn a_one_pixel_axis_refuses_the_tileable_op_naming_the_automatic_step() {
        let processed = super::Processed {
            rgba: picture(16_u32, 1_u32),
            report: crate::gfx::ops::OpsReport::default(),
        };
        let size = super::WallSize {
            width: 16_u32,
            height: 1_u32,
            note: None,
        };
        let error = shape_picture(processed, &size, FitMode::Cover, true)
            .expect_err("one row cannot tile vertically");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("automatic resize/tiling step"),
            "the message says which step: {error}"
        );
    }

    /// The session's override for `role` and its undo labels, as read back.
    fn theme_state(state: &AppState, role: Role) -> (Option<String>, usize) {
        state
            .with_session(move |session| {
                Ok((
                    session.texture_theme().overrides.get(&role).cloned(),
                    session.undo_labels().len(),
                ))
            })
            .expect("the read runs")
    }

    #[tokio::test]
    async fn the_theme_override_lands_in_session_state_outside_history() {
        let state = AppState::default();
        let mut request = request_for("MYTEX", "MYTEX");
        request.role = Some(Role::Wall);
        let role = apply_role(&state, &request).expect("the override sets");
        assert_eq!(role, Some("wall"), "the role reports back");
        let clean = request_for("MYTEX", "MYTEX");
        assert_eq!(apply_role(&state, &clean).expect("nothing to set"), None);
        let (override_name, edits) = theme_state(&state, Role::Wall);
        assert_eq!(
            override_name.as_deref(),
            Some("MYTEX"),
            "the override stands"
        );
        assert_eq!(edits, 0_usize, "no edit recorded");
    }

    #[test]
    fn the_preview_is_the_composited_picture() {
        let palette = Palette::from_playpal(&playpal(), 0).expect("the fixture palette");
        let image =
            IndexedImage::new(2_u32, 1_u32, vec![4_u8, 9], vec![true, true]).expect("2x1 is valid");
        let png = composite_png("MYTEX", &image, &palette).expect("compositing works");
        let back = crate::gfx::codec::decode_image(&png).expect("the preview decodes");
        assert_eq!((back.width, back.height), (2_u32, 1_u32), "the shape");
        assert_eq!(
            back.get(0, 0).map(|[r, g, _, _]| [r, g]),
            Some([4_u8, 251]),
            "the palette colours carried"
        );
    }

    #[tokio::test]
    async fn bad_arguments_refuse_before_the_source_is_fetched() {
        let state = AppState::default();
        let mut call = args(Some(100_u32), None);
        call.assign_role = Some("floor".to_owned());
        let role_error = create_texture_build(&state, &call)
            .await
            .expect_err("flat role");
        assert_eq!(
            code(&role_error),
            "invalid_argument",
            "the role answers first"
        );
        call.assign_role = None;
        let width_error = create_texture_build(&state, &call)
            .await
            .expect_err("bad width");
        assert_eq!(
            code(&width_error),
            "invalid_argument",
            "the width answers second"
        );
        call.size.width = None;
        let fetch_error = create_texture_build(&state, &call)
            .await
            .expect_err("missing file");
        assert_eq!(code(&fetch_error), "not_found", "only then the fetch");
    }

    /// Seeds `state`'s session with a PLAYPAL-only `IWAD` layer.
    fn seed_palette_layer(state: &AppState) {
        state
            .with_session(|session| {
                session.set_iwad(Layer::new(
                    PathBuf::from("base.wad"),
                    LayerRole::Iwad,
                    Wad {
                        kind: WadKind::Iwad,
                        lumps: vec![Lump::new(name("PLAYPAL"), playpal())],
                    },
                ));
                Ok(())
            })
            .expect("the seed lands");
    }

    #[tokio::test]
    async fn the_full_build_quantizes_sizes_and_reports_a_real_picture() {
        let state = AppState::default();
        seed_palette_layer(&state);
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("ART.png");
        let png = encode_png(&picture(64_u32, 128_u32), None).expect("a test PNG");
        std::fs::write(&path, &png).expect("the test file writes");
        let mut call = args(None, None);
        call.source = path.display().to_string();
        call.shared.ops = vec![super::ImageOp::Flip {
            horizontal: true,
            vertical: false,
        }];
        let report = create_texture_build(&state, &call)
            .await
            .expect("the create runs");
        assert_eq!(
            (report.width, report.height),
            (64_u32, 128_u32),
            "the kept size"
        );
        assert_eq!(report.lump, "TEXTURE1", "created in TEXTURE1");
        assert_eq!(report.texture, "MYTEX", "the name echoes");
        assert_eq!(report.patch, "MYTEX", "the patch follows the name");
        assert_eq!(report.steps.len(), 1, "only the user's flip ran");
        assert!(report.notes.is_empty(), "a kept size adds no note");
        assert!(report.warnings.is_empty(), "the picture is opaque");
        assert_eq!(
            report.next_steps,
            super::NEXT_STEPS.as_slice(),
            "the documented next steps"
        );
    }

    #[tokio::test]
    async fn an_odd_sized_picture_falls_back_to_the_square_with_a_note() {
        let state = AppState::default();
        seed_palette_layer(&state);
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("ODD.png");
        let png = encode_png(&picture(100_u32, 60_u32), None).expect("a test PNG");
        std::fs::write(&path, &png).expect("the test file writes");
        let mut call = args(None, None);
        call.source = path.display().to_string();
        let report = create_texture_build(&state, &call)
            .await
            .expect("the create runs");
        assert_eq!(
            (report.width, report.height),
            (128_u32, 128_u32),
            "the fallback"
        );
        assert_eq!(
            report.notes,
            vec!["resized to 128x128; pass width/height to choose".to_owned()],
            "the documented note"
        );
        let kinds: Vec<&str> = report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(kinds, vec!["fit"], "the automatic fit step reports");
    }
}
