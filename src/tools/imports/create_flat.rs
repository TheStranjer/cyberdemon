//! The one-shot floor/ceiling flat: one picture in, one square flat lump
//! out, in one undoable edit.
//!
//! [`create_flat_run`] flattens [`ProcessParams`] into its arguments:
//! [`crate::tools::imports::processed::process`] runs the user's ops, an
//! automatic `Fit` brings the picture to the requested `size` square
//! (`64x64` unless the caller asks for `128x128` or `256x256`, the sides the
//! flat codec stores), a `MakeTileable` op blends opposite edges when
//! `tileable` (the default),
//! [`crate::tools::imports::processed::to_indexed`] quantizes the result
//! opaquely against the session palette in the call's `Dither` mode,
//! [`crate::gfx::flat::encode_flat`] writes the raw index plane and
//! [`crate::gfx::flat::decode_flat`] reads it back for the preview. The
//! collision rules ([`build::check_flat_clash`]) run before anything is
//! written - flats are referenced by name everywhere, so a layer-held name
//! refuses until `replace` - and [`build::commit_flat`] places the lump as
//! one [`crate::session::Session::edit`] labelled `create_flat NAME`. An
//! `assign_role` request afterwards points a theme flat role at the new
//! flat; theme state is session state and is not part of the undo
//! history, which the result warnings say. A side above the vanilla `64`
//! adds its own warning: only a `ZDoom`-family port loads such a flat.
//!
//! The work is split: this root keeps what the call validates - the
//! constants, [`CreateFlatParams`] and the [`FlatRequest`]
//! [`parse_create_flat`] answers with beside the `assign_role` check - while
//! [`build`] runs the picture side on those arguments, from
//! [`build::shape_flat`] through [`build::build_flat`] and
//! [`build::commit_flat`]' one edit up to
//! [`build::create_flat_build`]' [`build::CreatedFlatReport`]. The group's
//! unit tests live in `create_flat/tests.rs`.

mod build;

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::gfx::ops::FitMode;
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::imports::create_flat::build::create_flat_build;
use crate::tools::imports::processed::ProcessParams;
use crate::tools::imports::{DEFAULT_FLAT_SIZE, parse_flat_size};
use crate::tools::reply;
use crate::wad::name::LumpName;

/// The vanilla flat edge: the default square side this tool writes, and
/// the largest one vanilla Doom loads.
const FLAT_SIDE: u32 = DEFAULT_FLAT_SIZE;
/// The blend band (pixels) of the automatic `make_tileable` step.
const TILE_BAND: u32 = 16_u32;
/// Media type of the published preview.
const PNG_MIME: &str = "image/png";

/// What every successful create reports as the way forward.
const NEXT_STEPS: [&str; 2] = [
    "edit_sector floor_flat/ceiling_flat",
    "auto_texture mode retexture after assign_role",
];

/// The full documentation of [`crate::server::Cyberdemon::create_flat_from_image`].
pub(in crate::tools) const CREATE_FLAT_TOOL_DESCRIPTION: &str = "One call from a picture file to a working Doom flat: it runs your image ops, fits the picture to the square size you pass (64 by default, 128 or 256 for ZDoom-family ports; vanilla Doom loads only 64x64 and the result warns past it), blends opposite edges so it tiles seamlessly (tileable, default true), quantizes opaquely to the session PLAYPAL in your dither mode and stores the raw index plane in the working flats region - ONE undoable edit with a preview echoing dither and size. assign_role points a theme floor or ceiling role at it.";

/// Arguments for [`crate::server::Cyberdemon::create_flat_from_image`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct CreateFlatParams {
    /// Picture to turn into a flat: absolute path, `file://` URI,
    /// `http(s)://` URL or this server's asset/upload URL.
    pub(in crate::tools) source: String,
    /// Flat lump name: 1 to 8 printable `ASCII` characters, uppercased.
    pub(in crate::tools) name: String,
    /// How the picture maps onto the square: `"cover"` (default),
    /// `"contain"` or `"stretch"`.
    #[serde(default)]
    pub(in crate::tools) fit: Option<FitMode>,
    /// Square side of the stored flat: 64 (default), 128 or 256; another
    /// value is `invalid_argument`. Vanilla Doom loads only a `64x64`
    /// flat, so a bigger side warns that the flat needs a `ZDoom`-family
    /// port.
    #[serde(default)]
    pub(in crate::tools) size: Option<u32>,
    /// Append a `make_tileable` op (band 16) blending opposite edges so
    /// the flat tiles seamlessly (default `true`).
    #[serde(default = "default_tileable")]
    pub(in crate::tools) tileable: bool,
    /// Theme flat role to point at the new flat afterwards (e.g.
    /// `"floor"`); a wall or unknown role is `invalid_argument`.
    #[serde(default)]
    pub(in crate::tools) assign_role: Option<String>,
    /// Overwrite or override an existing flat of this name (default
    /// `false`: such clashes are `already_exists`).
    #[serde(default)]
    pub(in crate::tools) replace: bool,
    /// The shared processing parameters: `ops`, `dither` and `palette`
    /// (see `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) shared: ProcessParams,
}

/// The documented `tileable` default.
const fn default_tileable() -> bool {
    true
}

/// The validated arguments of one create-flat call.
#[derive(Debug)]
struct FlatRequest {
    /// The `source` to fetch.
    source: String,
    /// The flat lump name, uppercased.
    name: LumpName,
    /// How the picture maps onto the square.
    fit: FitMode,
    /// The square side of the stored flat.
    size: u32,
    /// Whether a `make_tileable` op follows the fit.
    tileable: bool,
    /// The theme flat role to assign afterwards, if any.
    role: Option<Role>,
    /// Whether an existing same-name flat may be overwritten.
    replace: bool,
    /// The shared processing parameters.
    shared: ProcessParams,
}

/// Validate the create-flat arguments: the name, the square `size` and the
/// theme role. Every refusal here happens before the `source` is fetched.
///
/// # Errors
///
/// [`LumpName::parse`]' [`CyberError::InvalidArgument`] for a bad name,
/// [`parse_flat_size`]' error for a side outside the three, and
/// [`parse_flat_role`]' error for a wall or unknown `assign_role`.
fn parse_create_flat(args: &CreateFlatParams) -> Result<FlatRequest, CyberError> {
    Ok(FlatRequest {
        source: args.source.clone(),
        name: LumpName::parse(&args.name)?,
        fit: args.fit.unwrap_or(FitMode::Cover),
        size: parse_flat_size(args.size)?,
        tileable: args.tileable,
        role: parse_flat_role(args.assign_role.as_deref())?,
        replace: args.replace,
        shared: args.shared.clone(),
    })
}

/// The `assign_role` word: a parseable flat role.
///
/// # Errors
///
/// [`Role::parse`]' [`CyberError::InvalidArgument`] for an unknown name,
/// and [`CyberError::InvalidArgument`] naming the wall tools for a wall
/// role.
fn parse_flat_role(text: Option<&str>) -> Result<Option<Role>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let role = Role::parse(raw)?;
    if !role.is_flat() {
        return Err(CyberError::InvalidArgument(format!(
            "role {raw:?} is a wall role: create_flat_from_image stores flats - build a wall \
             texture with create_texture_from_image and point wall roles at it there or with \
             set_texture_theme"
        )));
    }
    Ok(Some(role))
}

/// `create_flat_from_image`' work: validate, fetch and process, check the
/// collisions, commit as one edit, then the theme override and the
/// preview publish.
pub(in crate::tools) async fn create_flat_run(
    state: &AppState,
    args: &CreateFlatParams,
) -> Result<CallToolResult, ErrorData> {
    reply(create_flat_build(state, args).await)
}

#[cfg(test)]
mod tests;
