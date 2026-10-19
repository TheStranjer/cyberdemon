//! The one-shot sprite: one picture in, every named sprite lump out, in
//! one undoable edit.
//!
//! [`create_sprite_run`] flattens [`ProcessParams`] into its arguments and
//! reuses [`crate::tools::imports::sprite`]' naming: the lump (or lumps)
//! come from `name`, from `sprite` with its `frame`/`rotation` parts, or
//! from `sprite` with a `frames` list that writes the same picture once
//! per frame letter. The picture side runs the documented chain: user
//! ops, [`crate::gfx::ops::ImageOp::RemoveBackground`] (border flood, on
//! by default), [`crate::gfx::ops::ImageOp::Trim`] to the remaining
//! content, and an aspect-kept `Fit` when `height` is given - with no
//! `height`, a picture taller than the native cap is scaled down to
//! [`AUTO_HEIGHT`] with a note. [`to_indexed`] quantizes under the alpha
//! rule, the offsets default anchors the trimmed picture's feet on the
//! origin ([`OffsetDefault::CenterBottom`]), and one
//! [`crate::session::Session::edit`] labelled `create_sprite NAME` places
//! every lump through [`put_lump`], carrying the rotation-coverage
//! warnings of [`warnings_for`] through untouched.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{
    Filter, FitMode, ImageOp, OpsReport, apply_ops, parse_hex_color, rounded_div,
};
use crate::gfx::patch::{Patch, decode_patch, encode_patch};
use crate::gfx::quantize::{DEFAULT_ALPHA_THRESHOLD, Transparency};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{
    LumpWrite, OffsetDefault, OffsetMode, OffsetsParam, parse_offset_mode, put_lump,
    resolve_offsets, to_i16_offsets,
};
use crate::tools::imports::processed::{ProcessParams, Processed, process, to_indexed};
use crate::tools::imports::sprite::{
    Naming, parse_frame, parse_mirror, parse_prefix, parse_rotation, part_name, warnings_for,
    whole_name,
};
use crate::tools::ok;
use crate::tools::palette::load_palette;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// The documented default [`ImageOp::RemoveBackground`] tolerance.
const DEFAULT_BACKGROUND_TOLERANCE: u8 = 32_u8;
/// A processed picture taller than this (and given no `height`) is scaled.
const TALL_HEIGHT: u32 = 256_u32;
/// The height a too-tall picture is scaled to, kept in the notes.
const AUTO_HEIGHT: u32 = 128_u32;
/// The largest target `height` this tool accepts.
const MAX_SPRITE_HEIGHT: u32 = 512_u32;
/// Media type of the published preview.
const PNG_MIME: &str = "image/png";

/// What every successful create reports as the way forward.
const NEXT_STEPS: [&str; 1] = [
    "a new thing type needs a definition: edit_decorate (ZDoom ports) or edit_dehacked to repoint an existing type's frames",
];

/// The full documentation of [`crate::server::Cyberdemon::create_sprite_from_image`].
pub(in crate::tools) const CREATE_SPRITE_TOOL_DESCRIPTION: &str = "One call from a picture file to working Doom sprite lumps: it processes the image (the same ops/dither/palette parameters the preview pipeline takes, see skill/reference/imports.md), removes the border background, trims the empty margin, scales the picture to a sprite-sized height, quantizes it with alpha holes and stores it as the classic column/post patch in the working archive's sprites region (S_START/S_END markers created there when the working archive has none) - all as ONE undoable edit labelled \"create_sprite NAME\" (save_wad writes the result; one undo removes every lump the call wrote). Naming follows import_image_as_sprite exactly: either name is the complete sprite lump name (\"TROOA1\", \"POSSA2A8\" - four sprite characters, one frame letter A-Z, [, \\ or ] and one rotation digit 0-8; a second frame+rotation pair is served by the same image mirrored), or give sprite (the four characters) with frame (default \"A\"), rotation (default 0: the all-angles image) and mirror_rotation (1-8 differing from rotation). frames lists frame letters (e.g. [\"A\",\"B\",\"C\",\"D\"]) to write the SAME final image once per frame - the static-replacement form; it goes with sprite and is invalid together with name or frame. Nothing is derived from the file. remove_background=true (the default) flood-clears pixels matching the border background colour from the edges inwards, so a drawing on white loses the white but keeps enclosed white details; background_tolerance (default 32) is the per-channel match distance and background_color (#RRGGBB or #RRGGBBAA) forces the colour instead of detecting it. trim=true (the default) crops to the bounding box of the pixels that remain (alpha at least 128), padding 0. height is the final pixel height (1..=512, aspect kept, contain fit with the area filter); without it a picture taller than 256 pixels is scaled to 128 tall and notes says \"scaled to 128 px tall; pass height to choose\". offsets take the same forms the patch and sprite importers document (\"zero\", \"grab\", \"center\", \"center_bottom\" or {\"x\": n, \"y\": n}) but resolve against the FINAL trimmed picture: the default center_bottom puts the sprite's feet on its origin, which is what floor-standing monsters and items want; grAb chunks are not consulted because the pipeline may have moved the picture. ops, dither and palette behave exactly as in process_image - your ops run FIRST, then background removal, trim and the height fit. A picture that background removal and trim empty entirely is invalid_argument (lower background_tolerance or pass remove_background=false). replace=true overwrites working sprites of these names; a same-name lump only a read-only layer holds becomes an override, and rotations another sprite lump already serves are reported as warnings, never refusals. Returns {lumps, width, height, left_offset, top_offset, steps, colors_used, transparent_pixels, preview, warnings, notes, next_steps}: lumps lists every name written, steps is one measurement object per applied op (yours first, then the automatic ones), transparent_pixels counts the holes, preview is the first lump decoded back through the sprite decoder. A new monster or decoration then needs a definition: edit_decorate (ZDoom-family) or edit_dehacked to repoint an existing type's frames; get_sprite and list_sprites read the lumps back.";

/// Arguments for [`crate::server::Cyberdemon::create_sprite_from_image`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct CreateSpriteParams {
    /// Picture to turn into sprite lump(s): absolute path, `file://` URI,
    /// `http(s)://` URL or this server's asset/upload URL.
    pub(in crate::tools) source: String,
    /// The complete sprite lump name (`TROOA1`, `POSSA2A8`); never with
    /// `sprite`, `frames`, `frame`, `rotation` or `mirror_rotation`.
    #[serde(default)]
    pub(in crate::tools) name: Option<String>,
    /// Four-character sprite prefix to build the name from, e.g. `"TROO"`.
    #[serde(default)]
    pub(in crate::tools) sprite: Option<String>,
    /// Frame letter the image serves (default `"A"`). With `sprite`; never
    /// with `frames`.
    #[serde(default)]
    pub(in crate::tools) frame: Option<String>,
    /// Frame letters to write the same image under, e.g. `["A","B","C"]`.
    /// With `sprite`; invalid together with `name` and `frame`.
    #[serde(default)]
    pub(in crate::tools) frames: Option<Vec<String>>,
    /// Rotation every named lump serves, `0`-`8` (default `0`: all angles).
    #[serde(default)]
    pub(in crate::tools) rotation: Option<u8>,
    /// Rotation `1`-`8` the mirrored image serves, extending each name to
    /// the pair form. With `sprite`.
    #[serde(default)]
    pub(in crate::tools) mirror_rotation: Option<u8>,
    /// Flood-clear the border background before trimming (default `true`).
    #[serde(default)]
    pub(in crate::tools) remove_background: Option<bool>,
    /// Per-channel background match tolerance, `0..=255` (default `32`).
    #[serde(default)]
    pub(in crate::tools) background_tolerance: Option<u8>,
    /// Background colour as `#RRGGBB` or `#RRGGBBAA`; unset detects the
    /// dominant border colour.
    #[serde(default)]
    pub(in crate::tools) background_color: Option<String>,
    /// Crop to the bounding box of the remaining content (default `true`).
    #[serde(default)]
    pub(in crate::tools) trim: Option<bool>,
    /// Final height in pixels, `1..=512`, aspect kept. Unset scales a
    /// picture taller than 256 px to 128 tall and says so in `notes`.
    #[serde(default)]
    pub(in crate::tools) height: Option<u32>,
    /// Drawing offsets for the final picture: `"zero"`, `"grab"`,
    /// `"center"`, `"center_bottom"` (the default) or `{"x": n, "y": n}`.
    #[serde(default)]
    pub(in crate::tools) offsets: Option<OffsetsParam>,
    /// Overwrite working sprites of these names in place (default `false`:
    /// working twins are `already_exists`).
    #[serde(default)]
    pub(in crate::tools) replace: bool,
    /// The shared processing parameters: `ops`, `dither` and `palette`
    /// (see `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) shared: ProcessParams,
}

/// The validated background-removal and trim parameters of one call.
struct Cleanup {
    /// Whether the border flood-clear runs.
    remove: bool,
    /// Per-channel background match tolerance.
    tolerance: u8,
    /// The forced background colour, validated, if any.
    color: Option<String>,
    /// Whether the content trim runs.
    trim: bool,
}

/// The validated arguments of one create-sprite call.
struct SpriteRequest {
    /// The `source` to fetch.
    source: String,
    /// Every lump this call names, deduplicated, in call order.
    names: Vec<Naming>,
    /// The border-cleanup half of the pipeline.
    cleanup: Cleanup,
    /// The explicit final height, validated, if any.
    height: Option<u32>,
    /// The requested offsets, `None` for the feet-on-origin default.
    offsets: Option<OffsetMode>,
    /// Whether working twins may be overwritten.
    replace: bool,
    /// The shared processing parameters.
    shared: ProcessParams,
}

/// Validate every create-sprite argument. Every refusal here happens
/// before the `source` is fetched.
///
/// # Errors
///
/// [`parse_sprite_names`]' naming errors and [`CyberError::InvalidArgument`]
/// for a `height` outside `1..=512` or an unparseable `background_color`.
fn parse_create(args: &CreateSpriteParams) -> Result<SpriteRequest, CyberError> {
    Ok(SpriteRequest {
        source: args.source.clone(),
        names: parse_sprite_names(args)?,
        cleanup: Cleanup {
            remove: args.remove_background.unwrap_or(true),
            tolerance: args
                .background_tolerance
                .unwrap_or(DEFAULT_BACKGROUND_TOLERANCE),
            color: args
                .background_color
                .as_deref()
                .map(check_background_color)
                .transpose()?,
            trim: args.trim.unwrap_or(true),
        },
        height: args.height.map(check_height).transpose()?,
        offsets: args.offsets.clone().map(parse_offset_mode).transpose()?,
        replace: args.replace,
        shared: args.shared.clone(),
    })
}

/// Which sprite lumps one call names: the whole `name`, the `sprite` with
/// its parts, or `sprite` with a `frames` list.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any selector combination outside
/// the three forms, and for text outside the sprite grammar.
fn parse_sprite_names(args: &CreateSpriteParams) -> Result<Vec<Naming>, CyberError> {
    match (
        args.name.as_deref(),
        args.sprite.as_deref(),
        args.frames.as_ref(),
    ) {
        (Some(text), None, None) => whole_name(text).map(|naming| vec![naming]),
        (Some(_), Some(_), _) => Err(CyberError::InvalidArgument(
            "create_sprite_from_image takes either name or sprite, not both: name one lump like \
             \"TROOA2A8\", or give the four-character sprite with frame, frames, rotation and \
             mirror_rotation"
                .to_owned(),
        )),
        (Some(_), None, Some(_)) => Err(CyberError::InvalidArgument(
            "frames is invalid together with name: a full lump name serves one lump - drop \
             frames, or name with sprite and frame letters"
                .to_owned(),
        )),
        (None, Some(prefix), None) => part_name(
            prefix,
            args.frame.as_deref(),
            args.rotation,
            args.mirror_rotation,
        )
        .map(|naming| vec![naming]),
        (None, Some(prefix), Some(list)) => frames_names(prefix, list, args),
        (None, None, Some(_)) => Err(CyberError::InvalidArgument(
            "frames goes with the four-character sprite, not alone: pass sprite with the frame \
             letters to write"
                .to_owned(),
        )),
        (None, None, None) => Err(CyberError::InvalidArgument(
            "create_sprite_from_image needs a sprite to name: pass the full lump name like \
             \"TROOA2A8\", or the four-character sprite with an optional frame, frames, \
             rotation and mirror_rotation"
                .to_owned(),
        )),
    }
}

/// The `frames` form: `sprite` with the rotation and one lump name per
/// listed frame letter, duplicates folded away in call order.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `frame` beside `frames`, an empty
/// list, or any part the sprite grammar refuses.
fn frames_names(
    prefix: &str,
    list: &[String],
    args: &CreateSpriteParams,
) -> Result<Vec<Naming>, CyberError> {
    if args.frame.is_some() {
        return Err(CyberError::InvalidArgument(
            "frame and frames are invalid together: pass either one frame letter or the list \
             of frame letters to write the same image under"
                .to_owned(),
        ));
    }
    if list.is_empty() {
        return Err(CyberError::InvalidArgument(
            "frames is empty: list at least one frame letter like [\"A\", \"B\"], or drop \
             frames and pass a single frame"
                .to_owned(),
        ));
    }
    let upper = parse_prefix(prefix)?;
    let rotation = parse_rotation(args.rotation)?;
    let names = list
        .iter()
        .map(|letter| pair_name(&upper, parse_frame(Some(letter))?, rotation, args))
        .collect::<Result<Vec<Naming>, CyberError>>()?;
    Ok(dedupe(names))
}

/// The one lump name `prefix` and `letter` answer with at the validated
/// rotation, in the plain or mirrored pair form.
///
/// # Errors
///
/// [`parse_mirror`]' [`CyberError::InvalidArgument`] and [`whole_name`]'
/// grammar error.
fn pair_name(
    prefix: &str,
    letter: char,
    rotation: u8,
    args: &CreateSpriteParams,
) -> Result<Naming, CyberError> {
    let mirror = parse_mirror(args.mirror_rotation, rotation)?;
    let text = mirror.map_or_else(
        || format!("{prefix}{letter}{rotation}"),
        |second| format!("{prefix}{letter}{rotation}{letter}{second}"),
    );
    whole_name(&text)
}

/// The names with later duplicates dropped, keeping the first order.
fn dedupe(names: Vec<Naming>) -> Vec<Naming> {
    let mut seen: Vec<LumpName> = Vec::new();
    let mut unique: Vec<Naming> = Vec::new();
    for naming in names {
        if !seen.contains(&naming.name) {
            seen.push(naming.name);
            unique.push(naming);
        }
    }
    unique
}

/// The explicit `height`: `1..=512`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other value.
fn check_height(value: u32) -> Result<u32, CyberError> {
    if value == 0 || value > MAX_SPRITE_HEIGHT {
        return Err(CyberError::InvalidArgument(format!(
            "height {value} is out of range: pass a final sprite height within \
             1..={MAX_SPRITE_HEIGHT} pixels"
        )));
    }
    Ok(value)
}

/// The forced background colour, kept as text once it parses.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for anything that is no hex colour.
fn check_background_color(text: &str) -> Result<String, CyberError> {
    parse_hex_color(text)
        .map(|_| text.to_owned())
        .map_err(|_error| {
            CyberError::InvalidArgument(format!(
                "background_color {text:?} is not a hex colour: pass #RRGGBB or #RRGGBBAA, or \
                 omit it to detect the dominant border colour"
            ))
        })
}

/// The automatic background-removal and trim ops, in pipeline order.
fn background_ops(cleanup: &Cleanup) -> Vec<ImageOp> {
    let mut ops: Vec<ImageOp> = Vec::new();
    if cleanup.remove {
        ops.push(ImageOp::RemoveBackground {
            tolerance: cleanup.tolerance,
            color: cleanup.color.clone(),
        });
    }
    if cleanup.trim {
        ops.push(ImageOp::Trim {
            alpha_threshold: DEFAULT_ALPHA_THRESHOLD,
            padding: 0_u32,
        });
    }
    ops
}

/// Run `ops` over `processed`, appending their measurements to the report.
fn apply_extra(processed: Processed, ops: &[ImageOp]) -> Result<Processed, CyberError> {
    if ops.is_empty() {
        return Ok(processed);
    }
    let (rgba, second) = apply_ops(processed.rgba, ops)?;
    let mut steps = processed.report.steps;
    steps.extend(second.steps);
    Ok(Processed {
        rgba,
        report: OpsReport { steps },
    })
}

/// The `height` plan for the final picture, and the note the automatic
/// rule adds. `None` keeps the trimmed picture exactly as it is.
struct HeightPlan {
    /// Target width in pixels, keeping the aspect.
    width: u32,
    /// Target height in pixels.
    height: u32,
    /// Why the too-tall rule scaled the picture, when it fired.
    note: Option<String>,
}

/// The height rule: the explicit `height` wins (when it changes anything),
/// else a picture taller than [`TALL_HEIGHT`] is planned for
/// [`AUTO_HEIGHT`] with its note.
fn choose_height(request: &SpriteRequest, picture: &RgbaImage) -> Option<HeightPlan> {
    match request.height {
        Some(height) => {
            let width = scaled_width(picture, height);
            let changes = picture.width != width || picture.height != height;
            changes.then_some(HeightPlan {
                width,
                height,
                note: None,
            })
        }
        None if picture.height > TALL_HEIGHT => Some(HeightPlan {
            width: scaled_width(picture, AUTO_HEIGHT),
            height: AUTO_HEIGHT,
            note: Some(format!(
                "scaled to {AUTO_HEIGHT} px tall; pass height to choose"
            )),
        }),
        None => None,
    }
}

/// The aspect-kept width for a picture scaled to `height`, at least one.
fn scaled_width(picture: &RgbaImage, height: u32) -> u32 {
    let scaled = rounded_div(
        u64::from(picture.width).saturating_mul(u64::from(height)),
        u64::from(picture.height.max(1_u32)),
    );
    u32::try_from(scaled)
        .unwrap_or(crate::gfx::ops::MAX_OP_DIMENSION)
        .max(1_u32)
}

/// The aspect-kept `Fit` to the height rule's target, with its note.
///
/// # Errors
///
/// [`apply_ops`]' [`CyberError::InvalidArgument`] widened to say the
/// failure came from the automatic step, not the user's ops.
fn fit_step(
    processed: Processed,
    request: &SpriteRequest,
) -> Result<(Processed, Option<String>), CyberError> {
    let Some(plan) = choose_height(request, &processed.rgba) else {
        return Ok((processed, None));
    };
    let ops = [ImageOp::Fit {
        width: plan.width,
        height: plan.height,
        mode: FitMode::Contain,
        filter: Filter::Area,
        background: None,
    }];
    let shaped = apply_extra(processed, &ops).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "the automatic height step failed: {error}: pass a smaller height or crop the picture"
        ))
    })?;
    Ok((shaped, plan.note))
}

/// Rewrite a failed automatic background/trim step into the documented
/// instruction; any other error stands as it is.
fn emptied(cleanup: &Cleanup, error: CyberError) -> CyberError {
    if !matches!(&error, CyberError::InvalidArgument(text) if text.contains("nothing left after trim"))
    {
        return error;
    }
    if cleanup.remove {
        return CyberError::InvalidArgument(
            "the whole image was removed as background; lower background_tolerance or set \
             remove_background false"
                .to_owned(),
        );
    }
    CyberError::InvalidArgument(
        "nothing was left to trim: every pixel sits below alpha 128; check the source picture \
         or pass trim=false"
            .to_owned(),
    )
}

/// The shaped, quantized and encoded picture, ready to commit.
struct BuiltSprite {
    /// The encoded `Doom` patch bytes every lump is written as.
    data: Vec<u8>,
    /// The first lump's `PNG`, read back through the sprite decoder.
    preview_png: Vec<u8>,
    /// Final width in pixels.
    width: u32,
    /// Final height in pixels.
    height: u32,
    /// Stored horizontal drawing offset.
    left_offset: i32,
    /// Stored vertical drawing offset.
    top_offset: i32,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct opaque source colours.
    colors_used: usize,
    /// Pixels the alpha rule dropped.
    transparent_pixels: usize,
    /// What the height rule did.
    notes: Vec<String>,
}

/// Run the whole picture side: user ops, background removal and trim, the
/// height fit, alpha quantization, patch encoding and the decoded
/// preview. The palette read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors, [`apply_extra`]' and [`fit_step`]' (through
/// [`emptied`] for an emptied picture), [`load_palette`]'
/// [`CyberError::NotFound`], [`to_i16_offsets`], [`encode_patch`],
/// [`decode_patch`] and [`encode_png`]' errors.
fn build_sprite(
    state: &AppState,
    request: &SpriteRequest,
    fetched: &Fetched,
) -> Result<BuiltSprite, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let cleared = apply_extra(processed, &background_ops(&request.cleanup))
        .map_err(|error| emptied(&request.cleanup, error))?;
    let (shaped, note) = fit_step(cleared, request)?;
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let indexed = to_indexed(
        &shaped,
        &palette,
        Transparency::default(),
        request.shared.dither_choice(),
    );
    let pair = resolve_offsets(
        request.offsets.as_ref(),
        None,
        OffsetDefault::CenterBottom,
        &shaped.rgba,
    );
    let (left_offset, top_offset) = to_i16_offsets(pair)?;
    let patch = Patch {
        left_offset,
        top_offset,
        image: indexed.image.clone(),
    };
    let data = encode_patch(&patch)?;
    let decoded = decode_patch(&data)?;
    let grab = (i32::from(left_offset), i32::from(top_offset));
    let preview_png = encode_png(&decoded.image.to_rgba(&palette), Some(grab))?;
    Ok(BuiltSprite {
        data,
        preview_png,
        width: shaped.rgba.width,
        height: shaped.rgba.height,
        left_offset: i32::from(left_offset),
        top_offset: i32::from(top_offset),
        steps: shaped.report.steps,
        colors_used: indexed.colors_used,
        transparent_pixels: indexed.transparent_pixels,
        notes: note.into_iter().collect(),
    })
}

/// The first lump's name, which labels the edit and is previewed.
///
/// # Errors
///
/// [`CyberError::Internal`] for a request whose naming resolved to
/// nothing - [`parse_sprite_names`] guarantees it cannot happen.
fn first_name(names: &[Naming]) -> Result<LumpName, CyberError> {
    names
        .first()
        .map(|naming| naming.name)
        .ok_or_else(|| CyberError::Internal("sprite naming resolved to no lump name".to_owned()))
}

/// The rotation-coverage warnings for every lump of one call, deduplicated
/// in order, read from the merged sprites region before the edit.
///
/// # Errors
///
/// [`warnings_for`]' errors for the merged scope.
fn collect_warnings(session: &Session, names: &[Naming]) -> Result<Vec<String>, CyberError> {
    let mut warnings: Vec<String> = Vec::new();
    for naming in names {
        let mut fresh: Vec<String> = warnings_for(session, naming)?
            .into_iter()
            .filter(|warning| !warnings.contains(warning))
            .collect();
        warnings.append(&mut fresh);
    }
    Ok(warnings)
}

/// The synchronous half: one undoable edit placing every named lump in the
/// working archive's sprites region.
///
/// # Errors
///
/// [`first_name`]' internal error and [`put_lump`]' placement and
/// `already_exists` errors; a refusal leaves the archive untouched.
fn commit_sprite(
    session: &mut Session,
    request: &SpriteRequest,
    built: &BuiltSprite,
) -> Result<(), CyberError> {
    let label = format!("create_sprite {}", first_name(&request.names)?);
    session.edit(&label, |wad| {
        for naming in &request.names {
            let write = LumpWrite {
                name: naming.name,
                data: &built.data,
                namespace: Some(Namespace::Sprites),
                replace: request.replace,
            };
            put_lump(wad, &write)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Structured result of [`crate::server::Cyberdemon::create_sprite_from_image`].
#[derive(Debug, Serialize)]
struct CreatedSpriteReport {
    /// Every sprite lump name this call wrote, in call order.
    lumps: Vec<String>,
    /// Final width in pixels.
    width: u32,
    /// Final height in pixels.
    height: u32,
    /// Stored horizontal drawing offset.
    left_offset: i32,
    /// Stored vertical drawing offset.
    top_offset: i32,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct opaque source colours.
    colors_used: usize,
    /// Pixels the alpha rule dropped.
    transparent_pixels: usize,
    /// The first lump's `PNG`, read back through the sprite decoder.
    preview: AssetRef,
    /// Rotations the session's other sprites already serve.
    warnings: Vec<String>,
    /// What the height rule did.
    notes: Vec<String>,
    /// What to do next.
    next_steps: &'static [&'static str],
}

/// `create_sprite_from_image`' work: validate, fetch and process, read the
/// warnings, commit every lump as one edit, then publish the preview.
pub(in crate::tools) async fn create_sprite_run(
    state: &AppState,
    args: &CreateSpriteParams,
) -> Result<CallToolResult, ErrorData> {
    match create_sprite_build(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The measured body of [`create_sprite_run`].
///
/// # Errors
///
/// [`parse_create`]' argument errors, [`AppState::fetch_source`]' and
/// [`build_sprite`]' errors, [`collect_warnings`]' and [`commit_sprite`]'
/// refusals and [`AppState::publish`]' errors; a refusal leaves the
/// working archive untouched.
async fn create_sprite_build(
    state: &AppState,
    args: &CreateSpriteParams,
) -> Result<CreatedSpriteReport, CyberError> {
    let request = parse_create(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let built = build_sprite(state, &request, &fetched)?;
    let warnings = state.with_session(|session| collect_warnings(session, &request.names))?;
    state.with_session(|session| commit_sprite(session, &request, &built))?;
    let first = first_name(&request.names)?;
    let preview = state.publish(
        &format!("{first}_sprite.png"),
        PNG_MIME,
        built.preview_png.clone(),
    )?;
    Ok(report_of(&request, &built, warnings, preview))
}

/// Fold the request, the built picture and the read facts into the
/// documented result.
fn report_of(
    request: &SpriteRequest,
    built: &BuiltSprite,
    warnings: Vec<String>,
    preview: AssetRef,
) -> CreatedSpriteReport {
    CreatedSpriteReport {
        lumps: request
            .names
            .iter()
            .map(|naming| naming.name.to_string())
            .collect(),
        width: built.width,
        height: built.height,
        left_offset: built.left_offset,
        top_offset: built.top_offset,
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        transparent_pixels: built.transparent_pixels,
        preview,
        warnings,
        notes: built.notes.clone(),
        next_steps: &NEXT_STEPS,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CREATE_SPRITE_TOOL_DESCRIPTION, CreateSpriteParams, apply_extra, background_ops,
        build_sprite, check_background_color, check_height, choose_height, collect_warnings,
        commit_sprite, dedupe, emptied, first_name, fit_step, parse_create, parse_sprite_names,
        scaled_width,
    };
    use crate::assets::fetch::Fetched;
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::{ImageOp, OpsReport};
    use crate::session::Session;
    use crate::state::AppState;
    use crate::tools::imports::image_pipeline::OffsetMode;
    use crate::tools::imports::processed::ProcessParams;
    use crate::wad::archive::Lump;
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// Parameters over one `source` with nothing else set.
    fn args() -> CreateSpriteParams {
        CreateSpriteParams {
            source: "/x/y.png".to_owned(),
            name: None,
            sprite: None,
            frame: None,
            frames: None,
            rotation: None,
            mirror_rotation: None,
            remove_background: None,
            background_tolerance: None,
            background_color: None,
            trim: None,
            height: None,
            offsets: None,
            replace: false,
            shared: ProcessParams {
                ops: Vec::new(),
                dither: None,
                palette: None,
            },
        }
    }

    /// Arguments over one whole `name`.
    fn by_name(text: &str) -> CreateSpriteParams {
        let mut call = args();
        call.name = Some(text.to_owned());
        call
    }

    /// Arguments over `sprite` with its optional parts.
    fn by_sprite(text: &str) -> CreateSpriteParams {
        let mut call = args();
        call.sprite = Some(text.to_owned());
        call
    }

    /// The lump names one set of arguments resolves to, in order.
    fn names_of(call: &CreateSpriteParams) -> Vec<String> {
        parse_sprite_names(call)
            .expect("test namings are valid")
            .iter()
            .map(|naming| naming.name.to_string())
            .collect()
    }

    #[test]
    fn the_name_form_serves_one_lump() {
        assert_eq!(names_of(&by_name("trooa2a8")), vec!["TROOA2A8"]);
        assert_eq!(names_of(&by_name("PLAYA0")), vec!["PLAYA0"]);
    }

    #[test]
    fn the_parts_form_names_one_lump_with_defaults() {
        assert_eq!(names_of(&by_sprite("boba")), vec!["BOBAA0"]);
        let mut views = by_sprite("TROO");
        views.frame = Some("a".to_owned());
        views.rotation = Some(2_u8);
        views.mirror_rotation = Some(8_u8);
        assert_eq!(names_of(&views), vec!["TROOA2A8"], "the mirror pair form");
    }

    #[test]
    fn the_frames_form_names_the_whole_list() {
        let mut call = by_sprite("TROO");
        call.frames = Some(
            ["A", "B", "C", "D"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        assert_eq!(
            names_of(&call),
            vec!["TROOA0", "TROOB0", "TROOC0", "TROOD0"],
            "one lump per frame at the rotation"
        );
        call.rotation = Some(1_u8);
        call.mirror_rotation = Some(5_u8);
        assert_eq!(
            names_of(&call),
            vec!["TROOA1A5", "TROOB1B5", "TROOC1C5", "TROOD1D5"],
            "every frame keeps the mirror pair"
        );
    }

    #[test]
    fn duplicate_frames_write_the_lump_once() {
        let mut call = by_sprite("BOBA");
        call.frames = Some(
            ["A", "B", "A"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        assert_eq!(
            names_of(&call),
            vec!["BOBAA0", "BOBAB0"],
            "the repeat folds away"
        );
    }

    #[test]
    fn selector_combinations_refuse() {
        let mut both = by_name("TROOA1");
        both.sprite = Some("TROO".to_owned());
        let error = parse_sprite_names(&both).expect_err("two selectors");
        assert_eq!(code(&error), "invalid_argument");
        assert!(error.to_string().contains("not both"), "{error}");
        let mut name_frames = by_name("TROOA1");
        name_frames.frames = Some(vec!["A".to_owned()]);
        assert_eq!(
            code(&parse_sprite_names(&name_frames).expect_err("frames beside name")),
            "invalid_argument"
        );
        let mut frame_frames = by_sprite("TROO");
        frame_frames.frame = Some("A".to_owned());
        frame_frames.frames = Some(vec!["B".to_owned()]);
        let pair_error = parse_sprite_names(&frame_frames).expect_err("frame beside frames");
        assert_eq!(code(&pair_error), "invalid_argument");
        assert!(
            pair_error.to_string().contains("frame and frames"),
            "the message names the pair: {pair_error}"
        );
        let mut alone = args();
        alone.frames = Some(vec!["A".to_owned()]);
        assert_eq!(
            code(&parse_sprite_names(&alone).expect_err("frames without sprite")),
            "invalid_argument"
        );
        alone.frames = Some(Vec::new());
        assert_eq!(
            code(&parse_sprite_names(&alone).expect_err("an empty list")),
            "invalid_argument"
        );
        assert_eq!(
            code(&parse_sprite_names(&args()).expect_err("no selector at all")),
            "invalid_argument"
        );
        let mut bad = by_sprite("TROO");
        bad.frames = Some(vec!["9".to_owned()]);
        assert_eq!(
            code(&parse_sprite_names(&bad).expect_err("a digit is no frame")),
            "invalid_argument"
        );
    }

    #[test]
    fn height_and_background_colour_are_validated_up_front() {
        assert_eq!(check_height(1_u32).expect("one"), 1_u32);
        assert_eq!(check_height(512_u32).expect("the top"), 512_u32);
        for value in [0_u32, 513_u32] {
            let error = check_height(value).expect_err("out of range");
            assert_eq!(code(&error), "invalid_argument");
            assert!(
                error.to_string().contains("1..=512"),
                "the message names the range: {error}"
            );
        }
        assert_eq!(
            check_background_color("#00FF00").expect("a colour"),
            "#00FF00"
        );
        let error = check_background_color("green").expect_err("no colour");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("background_color"),
            "the message names the field: {error}"
        );
    }

    /// A processed picture of a fixture colour over the given shape.
    fn processed(width: u32, height: u32) -> super::Processed {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count).flat_map(|_| [20_u8, 40, 60, 255]).collect();
        let rgba = RgbaImage::new(width, height, pixels).expect("test shape is valid");
        super::Processed {
            rgba,
            report: OpsReport::default(),
        }
    }

    /// The request over the current arguments.
    fn request(call: &CreateSpriteParams) -> super::SpriteRequest {
        parse_create(call).expect("test requests are valid")
    }

    #[test]
    fn the_default_request_is_removal_trim_and_feet_on_origin() {
        let built = request(&by_name("TROOA1"));
        assert!(built.cleanup.remove, "background removal is on by default");
        assert_eq!(built.cleanup.tolerance, 32_u8, "the documented tolerance");
        assert!(built.cleanup.trim, "trim is on by default");
        assert!(
            built.cleanup.color.is_none(),
            "the border colour is detected"
        );
        assert!(built.height.is_none(), "no explicit height");
        assert!(built.offsets.is_none(), "the preset default");
        assert!(!built.replace, "no silent overwrite");
        let ops = background_ops(&built.cleanup);
        assert_eq!(ops.len(), 2_usize, "the documented chain");
        assert!(
            matches!(ops.first(), Some(ImageOp::RemoveBackground { tolerance, .. }) if *tolerance == 32_u8),
            "the flood runs first at the default tolerance"
        );
        assert!(
            matches!(
                ops.get(1_usize),
                Some(ImageOp::Trim { alpha_threshold, padding })
                    if *alpha_threshold == 128_u8 && *padding == 0_u32
            ),
            "the trim runs at alpha 128 with no padding"
        );
        let mut off = by_name("TROOA1");
        off.remove_background = Some(false);
        off.trim = Some(false);
        assert!(
            background_ops(&request(&off).cleanup).is_empty(),
            "both switches off, no ops"
        );
    }

    #[test]
    fn the_height_rule_scales_explicitly_and_catches_tall_pictures() {
        let call = request_for_height(Some(40_u32));
        let plan = choose_height(&call, &processed(10_u32, 20_u32).rgba).expect("a fit");
        assert_eq!((plan.width, plan.height), (20_u32, 40_u32), "aspect kept");
        assert!(plan.note.is_none(), "an explicit height adds no note");
        let equal = choose_height(&call, &processed(20_u32, 40_u32).rgba);
        assert!(equal.is_none(), "an already-exact size does not fit again");
        let tall = request(&by_name("TROOA1"));
        let auto = choose_height(&tall, &processed(60_u32, 300_u32).rgba).expect("the rule fires");
        assert_eq!((auto.width, auto.height), (26_u32, 128_u32), "128 tall");
        assert_eq!(
            auto.note.as_deref(),
            Some("scaled to 128 px tall; pass height to choose"),
            "the documented note"
        );
        let short = choose_height(&tall, &processed(60_u32, 256_u32).rgba);
        assert!(short.is_none(), "256 is not too tall");
        assert_eq!(
            scaled_width(&processed(3_u32, 900_u32).rgba, 1_u32),
            1_u32,
            "never zero"
        );
    }

    /// A request carrying an explicit `height`.
    fn request_for_height(height: Option<u32>) -> super::SpriteRequest {
        let mut call = by_name("TROOA1");
        call.height = height;
        request(&call)
    }

    #[test]
    fn fit_appends_the_step_and_keeps_the_report_in_order() {
        let call = request_for_height(Some(40_u32));
        let picture = processed(10_u32, 20_u32);
        let (shaped, note) = fit_step(picture, &call).expect("the fit runs");
        assert_eq!((shaped.rgba.width, shaped.rgba.height), (20_u32, 40_u32));
        assert!(note.is_none(), "explicit height stays quiet");
        assert_eq!(shaped.report.steps.len(), 1_usize, "one automatic step");
        let kept = fit_step(processed(20_u32, 40_u32), &call).expect("nothing to do");
        assert!(kept.0.report.steps.is_empty(), "an equal size adds no step");
    }

    #[test]
    fn apply_extra_without_ops_is_the_identity() {
        let picture = processed(4_u32, 4_u32);
        let same = apply_extra(picture, &[]).expect("no ops run");
        assert_eq!((same.rgba.width, same.rgba.height), (4_u32, 4_u32));
    }

    /// An `InvalidArgument` carrying one of the step failures.
    fn trim_error(text: &str) -> CyberError {
        CyberError::InvalidArgument(text.to_owned())
    }

    #[test]
    fn an_emptied_picture_names_the_way_out() {
        let with_removal = request(&by_name("TROOA1"));
        let error = emptied(
            &with_removal.cleanup,
            trim_error("ops[1] nothing left after trim: no pixel reaches alpha 128"),
        );
        assert_eq!(code(&error), "invalid_argument");
        let message = error.to_string();
        assert!(
            message.contains("the whole image was removed as background"),
            "the documented hint: {message}"
        );
        assert!(
            message.contains("lower background_tolerance")
                && message.contains("remove_background false"),
            "both ways out are named: {message}"
        );
        let mut trim_only = by_name("TROOA1");
        trim_only.remove_background = Some(false);
        let other = emptied(
            &request(&trim_only).cleanup,
            trim_error("ops[0] nothing left after trim: no pixel reaches alpha 128"),
        );
        assert!(
            other.to_string().contains("trim=false"),
            "without removal the trim owns the message: {other}"
        );
        let passed = emptied(
            &with_removal.cleanup,
            trim_error("ops[0].color is out of range"),
        );
        assert!(
            passed.to_string().contains("out of range"),
            "another error stands as it is: {passed}"
        );
    }

    #[test]
    fn dedupe_keeps_the_first_occurrences_in_order() {
        let mut call = by_sprite("BOBA");
        call.frames = Some(
            ["B", "A", "B"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        let names = parse_sprite_names(&call).expect("valid");
        let folded = dedupe(names);
        let texts: Vec<String> = folded
            .iter()
            .map(|naming| naming.name.to_string())
            .collect();
        assert_eq!(texts, vec!["BOBAB0", "BOBAA0"], "first order, no repeats");
    }

    /// The `white_with_red` raster's pixel at `(x, y)`: red inside the
    /// `10 x 20` block, white outside.
    fn figure_pixel(x: u32, y: u32) -> [u8; 4] {
        let in_block = (10_u32..20_u32).contains(&x) && (20_u32..40_u32).contains(&y);
        if in_block {
            [255_u8, 0, 0, 255]
        } else {
            [255_u8, 255, 255, 255]
        }
    }

    /// A `40 x 60` raster: white around a solid `10 x 20` red block.
    fn white_with_red() -> RgbaImage {
        let pixels: Vec<u8> = (0_u32..60_u32)
            .flat_map(|y| (0_u32..40_u32).map(move |x| figure_pixel(x, y)))
            .flatten()
            .collect();
        RgbaImage::new(40_u32, 60_u32, pixels).expect("40x60 is valid")
    }

    /// The fixture palette bytes: `(i, 255 - i, i)`.
    fn playpal() -> Vec<u8> {
        (0_u32..512_u32)
            .map(|position| {
                u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                    .expect("remainders fit u8")
            })
            .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
            .collect()
    }

    /// Appends a `PLAYPAL` lump to a session's working archive.
    fn seed_playpal(session: &mut Session) -> Result<(), CyberError> {
        session.edit("seed", |wad| {
            wad.lumps.push(Lump::new(name("PLAYPAL"), playpal()));
            Ok(())
        })
    }

    /// A state whose session carries a `PLAYPAL`.
    fn state() -> AppState {
        let state = AppState::default();
        state
            .with_session(seed_playpal)
            .expect("the seed edit runs");
        state
    }

    /// A fetched test picture over `image`.
    fn fetched(image: &RgbaImage) -> Fetched {
        Fetched {
            bytes: encode_png(image, None).expect("test PNG encodes"),
            filename: "art.png".to_owned(),
            mime: Some("image/png".to_owned()),
        }
    }

    #[tokio::test]
    async fn build_removes_the_background_and_anchors_the_feet() {
        let state = state();
        let call = request(&by_name("TROOA1"));
        let built =
            build_sprite(&state, &call, &fetched(&white_with_red())).expect("the pipeline runs");
        assert_eq!(
            (built.width, built.height),
            (10_u32, 20_u32),
            "trimmed to the figure"
        );
        assert_eq!(built.left_offset, 5_i32, "half the final width");
        assert_eq!(built.top_offset, 20_i32, "the full final height");
        assert_eq!(
            built.transparent_pixels, 0_usize,
            "the trim cropped to the opaque figure"
        );
        let steps: Vec<&str> = built
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            steps,
            vec!["remove_background", "trim"],
            "the automatic chain"
        );
        assert_eq!(built.notes, Vec::<String>::new(), "nothing scaled");
    }

    #[tokio::test]
    async fn a_fully_background_picture_refuses_with_the_hint() {
        let state = state();
        let call = request(&by_name("TROOA1"));
        let side = 8_u32;
        let count = usize::try_from(side)
            .expect("test side")
            .checked_mul(usize::try_from(side).expect("test side"))
            .expect("test pixels")
            .checked_mul(4_usize)
            .expect("test bytes");
        let white = RgbaImage::new(side, side, vec![255_u8; count]).expect("8x8 is valid");
        let error = build_sprite(&state, &call, &fetched(&white))
            .map(|_| ())
            .expect_err("the whole picture is background");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error
                .to_string()
                .contains("the whole image was removed as background"),
            "the hint: {error}"
        );
    }

    #[test]
    fn one_edit_writes_every_named_lump_and_undoes_together() {
        let mut session = Session::default();
        let mut call = by_sprite("BOBA");
        call.frames = Some(
            ["A", "B"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        call.trim = Some(false);
        call.remove_background = Some(false);
        let built = super::BuiltSprite {
            data: vec![0_u8; 12],
            preview_png: Vec::new(),
            width: 2,
            height: 3,
            left_offset: 1,
            top_offset: 3,
            steps: Vec::new(),
            colors_used: 1,
            transparent_pixels: 4,
            notes: Vec::new(),
        };
        let request = request(&call);
        commit_sprite(&mut session, &request, &built).expect("the commit lands");
        assert_eq!(
            session.undo_labels(),
            vec!["create_sprite BOBAA0"],
            "one step labelled by the first lump"
        );
        for text in ["BOBAA0", "BOBAB0"] {
            assert!(
                session.find(name(text), Some(Namespace::Sprites)).is_some(),
                "{text} serves the merged view"
            );
        }
        session.undo().expect("one step back");
        assert!(
            session
                .find(name("BOBAA0"), Some(Namespace::Sprites))
                .is_none(),
            "both lumps left with the edit"
        );
    }

    #[test]
    fn a_working_twin_refuses_the_edit_without_writing_anything() {
        let mut session = Session::default();
        session
            .edit("seed", |wad| {
                wad.lumps.push(Lump::marker(name("S_START")));
                wad.lumps.push(Lump::new(name("BOBAB0"), vec![0_u8; 8]));
                wad.lumps.push(Lump::marker(name("S_END")));
                Ok(())
            })
            .expect("plain insert");
        let mut call = by_sprite("BOBA");
        call.frames = Some(
            ["A", "B"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        let built = super::BuiltSprite {
            data: vec![0_u8; 12],
            preview_png: Vec::new(),
            width: 2,
            height: 3,
            left_offset: 1,
            top_offset: 3,
            steps: Vec::new(),
            colors_used: 1,
            transparent_pixels: 4,
            notes: Vec::new(),
        };
        let request = request(&call);
        let error =
            commit_sprite(&mut session, &request, &built).expect_err("the second twin stands");
        assert_eq!(code(&error), "already_exists");
        assert!(session.undo_labels().len() == 1_usize, "the seed edit only");
        assert!(
            session
                .find(name("BOBAA0"), Some(Namespace::Sprites))
                .is_none(),
            "the first lump was not written either"
        );
    }

    #[test]
    fn the_rotation_warnings_are_read_for_every_lump() {
        let mut session = Session::default();
        session
            .edit("seed", |wad| {
                wad.lumps.push(Lump::marker(name("S_START")));
                wad.lumps.push(Lump::new(name("BOBAA2A8"), vec![0_u8; 8]));
                wad.lumps.push(Lump::new(name("BOBAB2A8"), vec![0_u8; 8]));
                wad.lumps.push(Lump::marker(name("S_END")));
                Ok(())
            })
            .expect("plain insert");
        let mut call = by_sprite("BOBA");
        call.frames = Some(
            ["A", "B"]
                .iter()
                .map(|letter| (*letter).to_owned())
                .collect(),
        );
        call.rotation = Some(2_u8);
        let warnings = collect_warnings(&session, &request(&call).names).expect("the read runs");
        assert_eq!(
            warnings.len(),
            2_usize,
            "one per covered frame: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .all(|warning| warning.contains("rotation 2")),
            "both name the rotation: {warnings:?}"
        );
    }

    #[test]
    fn the_first_name_labels_the_edit_and_premiers_the_preview() {
        let mut call = by_sprite("BOBA");
        call.frames = Some(vec!["C".to_owned(), "A".to_owned()]);
        let names = parse_sprite_names(&call).expect("valid");
        assert_eq!(first_name(&names).expect("first"), name("BOBAC0"));
        assert_eq!(
            code(&first_name(&[]).expect_err("nothing")),
            "internal",
            "an empty naming is an internal contradiction"
        );
    }

    #[test]
    fn offsets_presets_and_literals_reach_the_request() {
        let mut call = by_name("TROOA1");
        call.offsets = Some(crate::tools::imports::image_pipeline::OffsetsParam::Preset(
            "center".to_owned(),
        ));
        assert_eq!(
            request(&call).offsets,
            Some(OffsetMode::Center),
            "the preset parses"
        );
    }

    #[test]
    fn the_wire_form_flattens_every_documented_field() {
        let call: CreateSpriteParams = serde_json::from_value(json!({
            "source": "/x/y.png",
            "sprite": "TROO",
            "frames": ["A", "B"],
            "rotation": 2_u64,
            "remove_background": false,
            "background_tolerance": 9_u64,
            "background_color": "#FFFFFF",
            "trim": false,
            "height": 40_u64,
            "offsets": {"x": 3_i64, "y": 8_i64},
            "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
            "dither": "none",
            "palette": 1_u64,
            "replace": true,
        }))
        .expect("the documented wire form");
        assert_eq!(call.frames.as_ref().expect("list").len(), 2_usize);
        assert_eq!(call.rotation, Some(2_u8));
        assert_eq!(call.remove_background, Some(false));
        assert_eq!(call.background_tolerance, Some(9_u8));
        assert_eq!(call.height, Some(40_u32));
        assert!(call.shared.dither.is_some(), "the dither carried");
        let built = request(&call);
        assert!(
            !built.cleanup.remove && !built.cleanup.trim,
            "both switches carry"
        );
        assert_eq!(built.cleanup.tolerance, 9_u8);
        assert_eq!(built.cleanup.color.as_deref(), Some("#FFFFFF"));
        assert_eq!(built.height, Some(40_u32));
        assert_eq!(
            built.offsets,
            Some(OffsetMode::Explicit {
                left: 3_i32,
                top: 8_i32
            })
        );
        assert!(built.replace);
        assert_eq!(built.names.len(), 2_usize, "both frames named");
        assert!(
            CREATE_SPRITE_TOOL_DESCRIPTION.contains("create_sprite NAME"),
            "the description names the undo label"
        );
    }
}
