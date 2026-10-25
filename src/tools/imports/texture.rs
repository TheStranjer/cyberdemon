//! The one-shot wall texture: one picture in, patch + `PNAMES` entry +
//! texture definition out, in one undoable edit.
//!
//! [`create_texture_run`] flattens [`ProcessParams`] into its arguments:
//! [`build_picture`] runs the user's ops through
//! [`process`](crate::tools::imports::processed::process),
//! [`size::choose_size`] settles the texture size (explicit dimensions
//! win, a missing one follows the picture's aspect ratio rounded to a
//! multiple of `8`, and a picture that cannot tile falls back to
//! `128x128` with a note), an automatic `Fit` and optional `MakeTileable`
//! op shape the picture,
//! [`to_indexed`](crate::tools::imports::processed::to_indexed) quantizes
//! it against the session palette with
//! [`fill_low_alpha`](crate::tools::imports::processed::fill_low_alpha)
//! keeping the wall opaque, and `encode_patch` stores it with zero
//! offsets. The collision rules ([`commit::check_patch_clash`] and the
//! texture-name refusal) run before anything is written;
//! [`commit::plan_create`] and [`commit::commit_create`] reuse the
//! [`crate::tools::textures`] internals behind `build_texture` so patch,
//! `PNAMES` and the definition land as ONE
//! [`crate::session::Session::edit`] labelled `create_texture NAME`. An
//! `assign_role` request afterwards points a theme wall role at the new
//! texture; theme state is session state and is not part of the undo
//! history, which the result notes say.
//!
//! The work is split: this root keeps what one call validates - the
//! limits, [`CreateTextureParams`] with its [`TextureSizeParams`] half,
//! the [`CreateRequest`] [`parse_create`] answers with and
//! [`parse_assign_role`] - beside [`create_texture_build`], the body that
//! runs the stages in order; [`size`] is the tiling size rule, [`build`]
//! the picture side up to the [`build::Built`] it hands on and
//! [`commit`] the collision rules, the one edit and the report. The
//! group's unit tests live in `texture/tests.rs`.

mod build;
mod commit;
mod size;

use build::build_picture;
use commit::{CreatedTextureReport, Landed, apply_role, commit_create, plan_create, report_of};
use size::{check_height, check_width};

pub(in crate::tools) use commit::layer_label;

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::gfx::ops::FitMode;
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::imports::processed::ProcessParams;
use crate::tools::reply;
use crate::tools::textures::checked_name;
use crate::wad::name::LumpName;

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
pub(in crate::tools) const CREATE_TEXTURE_TOOL_DESCRIPTION: &str = "One call from a picture file to a working Doom wall texture: it runs your image ops, settles a tiling-friendly size, quantizes to the session PLAYPAL, stores the patch, lists it in PNAMES and defines the TEXTURE1/TEXTURE2 record - all as ONE undoable edit with a preview URL. tileable blends opposite edges and assign_role points a theme wall role at the result. The high-level texture tool.";

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

/// `create_texture_from_image`' work: validate, fetch and process, plan,
/// commit as one edit, then the theme override and the preview publish.
pub(in crate::tools) async fn create_texture_run(
    state: &AppState,
    args: &CreateTextureParams,
) -> Result<CallToolResult, ErrorData> {
    reply(create_texture_build(state, args).await)
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
mod tests;
