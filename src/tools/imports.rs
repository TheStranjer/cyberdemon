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
//! side: decode, opaque quantization in the call's
//! [`crate::gfx::ops::dither::Dither`] mode,
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
//!
//! The two picture converters that write lumps keep their files beside
//! this one: [`patch`] is the patch and raw imports, [`flat`] the flat
//! import. This root holds what they and the other modules share - the
//! effective-`PNAMES` reader [`current_pnames`] with its [`pnames_plan`],
//! the name and region parsers [`chosen`] and [`parse_namespace`] - plus
//! the tool descriptions and the router itself. The unit tests of that
//! shared machinery live in the `tests` module.

pub(in crate::tools) mod create_flat;
pub(in crate::tools) mod create_sprite;
mod flat;
pub(in crate::tools) mod image_pipeline;
mod patch;
pub(in crate::tools) mod processed;
mod sprite;
mod texture;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
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
use crate::tools::imports::flat::{ImportFlatParams, flat_imported};
use crate::tools::imports::image_pipeline::chosen_name;
use crate::tools::imports::patch::{
    ImportPatchParams, ImportRawParams, import_raw_run, patch_imported,
};
use crate::tools::imports::processed::{
    PROCESS_TOOL_DESCRIPTION, ProcessImageParams, process_image_run,
};
use crate::tools::imports::sprite::{
    ALIASED_SPRITE_TOOL_DESCRIPTION, ImportSpriteParams, SPRITE_TOOL_DESCRIPTION, sprite_imported,
};
use crate::tools::imports::texture::{
    CREATE_TEXTURE_TOOL_DESCRIPTION, CreateTextureParams, create_texture_run,
};
use crate::tools::ok;
use crate::wad::classify::Namespace;
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::textures::{parse_pnames, write_pnames};

pub(in crate::tools) use crate::tools::imports::flat::{
    DEFAULT_FLAT_SIZE, parse_flat_size, transparent_alpha_pixels,
};

/// The `PNAMES` side of one patch import.
pub(in crate::tools) struct PnamesPlan {
    /// The index the name answers with in the effective list.
    pub(in crate::tools) index: usize,
    /// The new `PNAMES` bytes when the list grew, `None` when the name was
    /// already listed and nothing needs writing.
    pub(in crate::tools) bytes: Option<Vec<u8>>,
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
    let Some(entry) = session.find_global(lumps::PNAMES) else {
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
mod tests;
