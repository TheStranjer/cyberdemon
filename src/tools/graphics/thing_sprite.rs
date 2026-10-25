//! `get_thing_sprite`: preview one thing type's sprite, resolved through
//! the session's own definitions.
//!
//! [`thing_request`] validates [`GetThingSpriteParams`] into a
//! [`ThingRequest`]: the same frame check `get_sprite` makes, with
//! [`parse_thing_rotation`] refusing the all-angles `0` this tool does not
//! serve. [`prepare_thing_sprite`] then looks the type up with
//! [`thing_sprite_def`] over [`crate::session::Session::definitions`] -
//! the built-in table of the session's game overlaid with its customs -
//! resolves the rotation as `get_sprite` does and renders it, reporting a
//! rotation no lump serves through [`missing_thing_sprite`].
//! [`get_thing_sprite_run`] publishes that [`PreparedThing`] as its own
//! [`ThingSpriteReport`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::ThingDef;
use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::graphics::get::parse_frame;
use crate::tools::graphics::picture::{
    ImageOptions, ImageParams, Rendered, publish_image, render_patch,
};
use crate::tools::graphics::resolve::load_sprite;
use crate::tools::graphics::{DEFAULT_ROTATION, MAX_ROTATION};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::get_thing_sprite`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetThingSpriteParams {
    /// The thing type number to preview, as editors write it into
    /// `THINGS`, e.g. 3001 (the Imp). Looked up in the session's
    /// definitions: the game's built-in table plus its custom
    /// DEHACKED/DECORATE/ZScript/MAPINFO types.
    pub(super) r#type: u32,
    /// Sprite frame letter to read: one character of `A`-`Z` then `[`,
    /// `\`, `]` (case-insensitive). Default: the definition's own frame.
    #[serde(default)]
    pub(super) frame: Option<String>,
    /// Rotation to read, `1`-`8` (default 1): the eight views, `1` the
    /// front a head-on viewer sees, `5` the back. Resolved by the engine's
    /// pair-name and rotation-`0` fallback rules: a frame's rotation `2`
    /// lives in the lump named `TROOA2A8`, unflipped.
    #[serde(default)]
    pub(super) rotation: Option<u8>,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    pub(super) common: ImageParams,
}

/// The validated selection of one `get_thing_sprite` call.
#[derive(Debug)]
pub(super) struct ThingRequest {
    /// Thing type number, as the client passed it.
    pub(super) kind: u32,
    /// Requested frame letter, `None` for the definition's own.
    pub(super) frame: Option<char>,
    /// Rotation number `1..=8`.
    pub(super) rotation: u8,
}

/// The selection behind a `get_thing_sprite` call: the type number with
/// its optional `frame` and `rotation` (default 1).
///
/// # Errors
///
/// [`parse_frame`] and [`parse_thing_rotation`]' [`CyberError::InvalidArgument`]
/// errors.
pub(super) fn thing_request(args: &GetThingSpriteParams) -> Result<ThingRequest, CyberError> {
    Ok(ThingRequest {
        kind: args.r#type,
        frame: args
            .frame
            .as_deref()
            .map(|text| parse_frame(Some(text)))
            .transpose()?,
        rotation: parse_thing_rotation(args.rotation)?,
    })
}

/// The `get_thing_sprite` rotation number: `1..=8`, defaulting to
/// [`DEFAULT_ROTATION`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected number.
pub(super) fn parse_thing_rotation(value: Option<u8>) -> Result<u8, CyberError> {
    match value {
        None => Ok(DEFAULT_ROTATION),
        Some(rotation @ 1_u8..=MAX_ROTATION) => Ok(rotation),
        Some(rotation) => Err(CyberError::InvalidArgument(format!(
            "rotation {rotation} is out of range: get_thing_sprite serves the eight views 1 to \
             {MAX_ROTATION}; get_sprite also takes the all-angles 0"
        ))),
    }
}

/// The definition one `get_thing_sprite` call previews: a known type with
/// a non-empty sprite.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a type the session definitions do not
/// hold, and for a definition whose sprite is empty.
pub(super) fn thing_sprite_def(session: &Session, kind: u32) -> Result<ThingDef, CyberError> {
    let def = session.definitions()?.thing(kind).cloned().ok_or_else(|| {
        CyberError::NotFound(format!(
            "no thing type {kind} in the session definitions; get_thing_types lists what the \
                 game has, and custom types need their archive loaded"
        ))
        .with_hint("get_thing_types with a search term")
    })?;
    if def.sprite.is_empty() {
        return Err(CyberError::NotFound(format!(
            "thing type {kind} has no sprite; get_thing_types shows which types define one"
        )));
    }
    Ok(def)
}

/// The `not_found` for a rotation no sprite lump serves: names the
/// expected lump pattern, the rotation's own image and the frame's
/// all-angles image.
pub(super) fn missing_thing_sprite(sprite: &str, frame: char, rotation: u8) -> CyberError {
    CyberError::NotFound(format!(
        "no {sprite}{frame}{rotation} / {sprite}{frame}0 sprite lump in the merged view; \
         list_sprites with prefix \"{sprite}\" lists what the session has"
    ))
}

/// What the session-side half of `get_thing_sprite` prepared.
pub(super) struct PreparedThing {
    /// The definition row the picture came from.
    def: ThingDef,
    /// The frame letter that was served.
    frame: char,
    /// The rotation name the call resolves to (`TROOA1`), published under.
    name: LumpName,
    /// The lump whose pixels served the rotation.
    lump: LumpName,
    /// Whether the stored image was flipped to serve it.
    mirrored: bool,
    /// The rendered picture.
    rendered: Rendered,
}

/// `get_thing_sprite`' work under the session lock: definition lookup,
/// rotation resolution and rendering.
///
/// # Errors
///
/// [`thing_sprite_def`]' and [`missing_thing_sprite`]' [`CyberError::NotFound`]
/// errors, [`load_sprite`]' and [`render_patch`]' errors.
pub(super) fn prepare_thing_sprite(
    session: &Session,
    request: &ThingRequest,
    options: &ImageOptions,
) -> Result<PreparedThing, CyberError> {
    let def = thing_sprite_def(session, request.kind)?;
    let frame = request.frame.unwrap_or(def.frame);
    let sprite = def.sprite.clone();
    let found = match load_sprite(session, &sprite, frame, request.rotation) {
        Ok(found) => found,
        Err(error) if error.code() == "not_found" => {
            return Err(missing_thing_sprite(&sprite, frame, request.rotation));
        }
        Err(error) => return Err(error),
    };
    let rotation = request.rotation;
    let name = LumpName::parse(&format!("{sprite}{frame}{rotation}"))?;
    let rendered = render_patch(session, &found.patch, options)?;
    Ok(PreparedThing {
        def,
        frame,
        name,
        lump: found.lump,
        mirrored: found.mirrored,
        rendered,
    })
}

/// Structured result of [`Cyberdemon::get_thing_sprite`].
#[derive(Serialize)]
pub(super) struct ThingSpriteReport {
    /// The thing type number that was asked for.
    pub(super) r#type: u32,
    /// The definition's editor display name.
    name: String,
    /// The four-character sprite name.
    sprite: String,
    /// The frame letter the picture came from.
    frame: char,
    /// The rotation number `1..=8`.
    rotation: u8,
    /// The lump whose pixels served the rotation.
    lump: LumpName,
    /// Whether the stored image was flipped horizontally to serve it.
    mirrored: bool,
    /// Width in published pixels: `scale` times the stored image.
    width: u32,
    /// Height in published pixels.
    height: u32,
    /// Horizontal drawing offset in published pixels.
    left_offset: i32,
    /// Vertical drawing offset in published pixels.
    top_offset: i32,
    /// Whether the thing hangs from the ceiling.
    hanging: bool,
    /// The published picture.
    asset: AssetRef,
}

/// `get_thing_sprite`'s work: resolve and render under the session lock,
/// then publish outside it.
///
/// # Errors
///
/// [`prepare_thing_sprite`]' errors and [`publish_image`]' for the asset.
pub(super) fn get_thing_sprite_run(
    state: &AppState,
    request: &ThingRequest,
    options: &ImageOptions,
) -> Result<ThingSpriteReport, CyberError> {
    let prepared = state.with_session(|session| prepare_thing_sprite(session, request, options))?;
    let PreparedThing {
        def,
        frame,
        name,
        lump,
        mirrored,
        rendered,
    } = prepared;
    let (width, height) = (rendered.image.width, rendered.image.height);
    let left_offset = rendered.left_offset.unwrap_or(0_i32);
    let top_offset = rendered.top_offset.unwrap_or(0_i32);
    let asset = publish_image(state, &name.to_string(), &rendered, options)?;
    Ok(ThingSpriteReport {
        r#type: def.kind,
        name: def.name,
        sprite: def.sprite,
        frame,
        rotation: request.rotation,
        lump,
        mirrored,
        width,
        height,
        left_offset,
        top_offset,
        hanging: def.hanging,
        asset,
    })
}
