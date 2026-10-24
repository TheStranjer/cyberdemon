//! `get_flat`, `get_patch` and `get_sprite`: resolve a name, render it and
//! publish the picture.
//!
//! Each run resolves and renders under the session lock through the shared
//! resolvers in [`crate::tools::graphics::resolve`] and the render helpers
//! of [`crate::tools::graphics::picture`], then publishes outside it with
//! [`finish`]. [`sprite_request`] is the either-or of [`GetSpriteParams`]:
//! one whole [`SpriteRequest::Lump`] served as stored, or a
//! [`SpriteRequest::Rotation`] that [`by_rotation`] builds over
//! [`crate::tools::graphics::list_sprites::parse_sprite`], [`parse_frame`]
//! and [`parse_rotation`]. [`prepare_sprite`] answers either of them.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::graphics::list_sprites::parse_sprite;
use crate::tools::graphics::picture::{
    ImageOptions, ImageParams, ImageReport, Prepared, finish, render_indexed, render_patch,
};
use crate::tools::graphics::resolve::{load_flat, load_patch, load_sprite, load_sprite_lump};
use crate::tools::graphics::{DEFAULT_FRAME, DEFAULT_ROTATION, MAX_ROTATION};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::get_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetFlatParams {
    /// The flat's lump name: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively.
    pub(super) name: String,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    pub(super) common: ImageParams,
}

/// Arguments for [`Cyberdemon::get_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetPatchParams {
    /// The patch's lump name: 1 to 8 printable `ASCII` characters,
    /// matched case-insensitively.
    pub(super) name: String,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    pub(super) common: ImageParams,
}

/// Arguments for [`Cyberdemon::get_sprite`]: exactly one of `name` and
/// `sprite` says which picture to read.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetSpriteParams {
    /// The whole sprite lump name to serve as stored, e.g. `"TROOA1"` (1
    /// to 8 printable `ASCII` characters, case-insensitive). Give this or
    /// `sprite`, never both.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// The four-character sprite name to resolve, e.g. `"TROO"` (case-
    /// insensitive). Give this with `frame`/`rotation` instead of `name`.
    #[serde(default)]
    pub(super) sprite: Option<String>,
    /// Frame letter of the rotation to read: one character of `A`-`Z` then
    /// `[`, `\`, `]` (default `"A"`). With `sprite`.
    #[serde(default)]
    pub(super) frame: Option<String>,
    /// Rotation to read, `0`-`8` (default 1): `0` is the all-angles image,
    /// `1`..`8` the eight views, served by the pair name that carries the
    /// rotation itself or, for its second rotation, flipped, when no lump
    /// names the rotation on its own. With `sprite`.
    #[serde(default)]
    pub(super) rotation: Option<u8>,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    pub(super) common: ImageParams,
}

/// `get_flat`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`load_flat`]' errors for the lump and [`finish`]' for the asset.
pub(super) fn get_flat_run(
    state: &AppState,
    name: LumpName,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| {
        let image = load_flat(session, name)?;
        Ok(Prepared::plain(
            name,
            render_indexed(session, &image, None, options)?,
        ))
    })?;
    finish(state, prepared, options)
}

/// `get_patch`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`load_patch`]' errors for the lump and [`finish`]' for the asset.
pub(super) fn get_patch_run(
    state: &AppState,
    name: LumpName,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| {
        let patch = load_patch(session, name)?;
        Ok(Prepared::plain(
            name,
            render_patch(session, &patch, options)?,
        ))
    })?;
    finish(state, prepared, options)
}

/// What a `get_sprite` call names: one lump exactly, or one rotation.
#[derive(Debug)]
pub(super) enum SpriteRequest {
    /// The whole lump name to serve as stored (`TROOA2A8`).
    Lump(LumpName),
    /// One rotation of a sprite, resolved by the engine's rules.
    Rotation {
        /// The name this rotation would carry: what the report answers
        /// with (`TROOA8`).
        name: LumpName,
        /// Four-character sprite prefix, uppercased.
        sprite: String,
        /// Frame letter, uppercased.
        frame: char,
        /// Rotation `0`-`8`.
        rotation: u8,
    },
}

/// Which sprite one `get_sprite` call asks for: exactly one of `name` and
/// `sprite`, with `frame` (default `A`) and `rotation` (default 1).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both or neither selector, or for a
/// malformed `sprite`, `frame` or `rotation`.
pub(super) fn sprite_request(args: &GetSpriteParams) -> Result<SpriteRequest, CyberError> {
    match (args.name.as_deref(), args.sprite.as_deref()) {
        (Some(text), None) => Ok(SpriteRequest::Lump(LumpName::parse(text)?)),
        (None, Some(text)) => by_rotation(text, args.frame.as_deref(), args.rotation),
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "get_sprite takes either name or sprite, not both: name one lump like \"TROOA1\", \
             or give sprite with frame and rotation"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "get_sprite needs a sprite to read: pass the full lump name, or the four-character \
             sprite with an optional frame and rotation"
                .to_owned(),
        )),
    }
}

/// The rotation request behind a `sprite` with its `frame` and `rotation`.
///
/// # Errors
///
/// [`parse_sprite`], [`parse_frame`] and [`parse_rotation`]'
/// [`CyberError::InvalidArgument`] errors.
fn by_rotation(
    sprite: &str,
    frame: Option<&str>,
    rotation: Option<u8>,
) -> Result<SpriteRequest, CyberError> {
    let prefix = parse_sprite(sprite)?;
    let letter = parse_frame(frame)?;
    let number = parse_rotation(rotation)?;
    Ok(SpriteRequest::Rotation {
        name: LumpName::parse(&format!("{prefix}{letter}{number}"))?,
        sprite: prefix,
        frame: letter,
        rotation: number,
    })
}

/// The frame letter: one character of `A`-`Z` then `[`, `\`, `]`,
/// uppercased, defaulting to [`DEFAULT_FRAME`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other text.
pub(super) fn parse_frame(text: Option<&str>) -> Result<char, CyberError> {
    let Some(raw) = text else {
        return Ok(DEFAULT_FRAME);
    };
    let upper = raw.to_ascii_uppercase();
    let single = upper.chars().count() == 1;
    if let Some(letter) = upper.chars().next()
        && single
        && is_frame_letter(letter)
    {
        return Ok(letter);
    }
    Err(CyberError::InvalidArgument(format!(
        "frame {text:?} is not one sprite frame letter: pass a single character A-Z, [, \\ or ]"
    )))
}

/// Whether `letter` is a sprite frame letter: `A`-`Z`, then the three
/// characters after `Z`.
const fn is_frame_letter(letter: char) -> bool {
    matches!(letter, 'A'..='Z' | '[' | '\\' | ']')
}

/// The rotation number: `0`-`8`, defaulting to [`DEFAULT_ROTATION`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected number.
pub(super) fn parse_rotation(value: Option<u8>) -> Result<u8, CyberError> {
    match value {
        None => Ok(DEFAULT_ROTATION),
        Some(rotation @ (0_u8..=MAX_ROTATION)) => Ok(rotation),
        Some(rotation) => Err(CyberError::InvalidArgument(format!(
            "rotation {rotation} is out of range: pass 0 for the all-angles image, or 1 to \
             {MAX_ROTATION} for the eight views"
        ))),
    }
}

/// `get_sprite`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`prepare_sprite`]' errors and [`finish`]' for the asset.
pub(super) fn get_sprite_run(
    state: &AppState,
    request: SpriteRequest,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| prepare_sprite(session, request, options))?;
    finish(state, prepared, options)
}

/// Resolves one sprite request and renders the picture it names: a whole
/// lump as stored, or a rotation by the engine's rules.
///
/// # Errors
///
/// [`load_sprite_lump`] and [`load_sprite`]' errors, and
/// [`render_patch`]' for the pixels.
fn prepare_sprite(
    session: &Session,
    request: SpriteRequest,
    options: &ImageOptions,
) -> Result<Prepared, CyberError> {
    match request {
        SpriteRequest::Lump(name) => {
            let patch = load_sprite_lump(session, name)?;
            let rendered = render_patch(session, &patch, options)?;
            Ok(Prepared::served(name, name, false, rendered))
        }
        SpriteRequest::Rotation {
            name,
            sprite,
            frame,
            rotation,
        } => {
            let found = load_sprite(session, &sprite, frame, rotation)?;
            let rendered = render_patch(session, &found.patch, options)?;
            Ok(Prepared::served(name, found.lump, found.mirrored, rendered))
        }
    }
}
