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
//! [`AUTO_HEIGHT`] with a note. [`crate::tools::imports::processed::to_indexed`]
//! quantizes under the alpha rule, the offsets default anchors the
//! trimmed picture's feet on the origin
//! ([`crate::tools::imports::image_pipeline::OffsetDefault::CenterBottom`]),
//! and one [`crate::session::Session::edit`] labelled `create_sprite NAME`
//! places every lump through
//! [`crate::tools::imports::image_pipeline::put_lump`], carrying the
//! rotation-coverage warnings of
//! [`crate::tools::imports::sprite::warnings_for`] through untouched.
//!
//! The work is split: this root keeps what the call validates - the
//! constants, [`CreateSpriteParams`] and the [`SpriteRequest`]
//! [`parse_create`] answers with, its [`Cleanup`] half and the three
//! naming forms - while [`build`] runs the picture side on those
//! arguments, from [`build::background_ops`] through [`build::build_sprite`]
//! and [`build::commit_sprite`]' one edit up to
//! [`build::create_sprite_build`]'s [`build::CreatedSpriteReport`]. The
//! group's unit tests live in `create_sprite/tests.rs`.

mod build;

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::gfx::ops::parse_hex_colour;
use crate::state::AppState;
use crate::tools::imports::create_sprite::build::create_sprite_build;
use crate::tools::imports::image_pipeline::{OffsetMode, OffsetsParam, parse_offset_mode};
use crate::tools::imports::processed::ProcessParams;
use crate::tools::imports::sprite::{
    Naming, parse_frame, parse_mirror, parse_prefix, parse_rotation, part_name, whole_name,
};
use crate::tools::reply;
use crate::wad::name::LumpName;

/// The documented default [`crate::gfx::ops::ImageOp::RemoveBackground`] tolerance.
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
pub(in crate::tools) const CREATE_SPRITE_TOOL_DESCRIPTION: &str = "One call from a picture file to working Doom sprite lumps: it runs your image ops, flood-clears the border background, trims the empty margin, scales to a sprite height, quantizes with alpha holes and stores the patch in the working sprites region - ONE undoable edit; frames writes the same image across several frame letters. Naming follows import_image_as_sprite. The high-level sprite tool; undo removes every lump written.";

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
    parse_hex_colour(text, true)
        .map(|_| text.to_owned())
        .map_err(|_error| {
            CyberError::InvalidArgument(format!(
                "background_color {text:?} is not a hex colour: pass #RRGGBB or #RRGGBBAA, or \
                 omit it to detect the dominant border colour"
            ))
        })
}

/// `create_sprite_from_image`' work: validate, fetch and process, read the
/// warnings, commit every lump as one edit, then publish the preview.
pub(in crate::tools) async fn create_sprite_run(
    state: &AppState,
    args: &CreateSpriteParams,
) -> Result<CallToolResult, ErrorData> {
    reply(create_sprite_build(state, args).await)
}

#[cfg(test)]
mod tests;
