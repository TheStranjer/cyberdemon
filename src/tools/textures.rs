//! Texture tools: the composite-texture definitions of the merged view and
//! the copy-on-write `PNAMES` editing behind `edit_pnames`.
//!
//! [`TextureSet`] loads the merged `PNAMES`, `TEXTURE1` and `TEXTURE2`
//! lumps; it lives beside the session it reads, in
//! [`crate::session::textures`], and this group re-exports it so the listings
//! and editors keep reaching it at [`crate::tools::textures`].
//! `list_textures` and `list_pnames` page through it, `edit_pnames` rewrites
//! the name list renumbering every texture patch reference so the indices
//! stay truthful, and `build_texture`/`remove_texture` define and drop single
//! texture records (appending needed `PNAMES` names along the way). The
//! removal half of that machinery is reusable on its own
//! (`plan_pnames_removal`/`write_pnames_rewrite`) so other tools can fold a
//! `PNAMES` removal and its renumbering into one edit. Every write goes
//! through exactly one [`crate::session::Session::edit`], bringing the lumps
//! into the working archive as overrides.
//!
//! `textures/list` holds the two listings' arguments, rows (built from
//! [`TextureSet::texture_entries`]) and runs, `textures/pnames` the
//! `edit_pnames` arguments and the plan one call computes,
//! `textures/pnames_rewrite` the rewrite that commits it and the reusable
//! removal, `textures/build` the two single-texture tools with the definition
//! steps the texture import shares, and `textures/tests` the group's unit
//! tests. This root keeps the constants, the re-exported [`TextureSet`] and
//! [`WhichLump`], the glob helper the listings share, the re-exports the other
//! groups import and the router methods; every global-lump write these tools
//! run goes through [`crate::wad::archive::Wad::put_global`].

mod build;
mod list;
mod pnames;
mod pnames_rewrite;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::server::Cyberdemon;
use crate::tools::textures::build::{
    BuildTextureParams, RemoveTextureParams, build_texture_run, remove_texture_run,
};
use crate::tools::textures::list::{
    ListPnamesParams, ListTexturesParams, list_pnames_run, list_textures_run,
};
use crate::tools::textures::pnames::EditPnamesParams;
use crate::tools::textures::pnames_rewrite::{edit_pnames_run, pick_operation};
use crate::tools::{Page, fail, reply};
use crate::wad::name::glob_match;

pub use crate::session::textures::{TextureSet, WhichLump};

// The `PNAMES` rewrite and the definition steps behind
// `create_texture_from_image`, kept at the paths the `patches`, `imports`,
// `palette` and `plan` groups already import; each lives beside its tool.
pub(in crate::tools) use crate::tools::textures::build::{
    Placement, checked_name, definition, lump_copy, missing_names, place_definition, target_lump,
};
pub(in crate::tools) use crate::tools::textures::pnames_rewrite::{
    PnamesRewrite, plan_pnames_removal, write_pnames_rewrite,
};

/// Default page size for the listing tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page either listing tool will return.
const MAX_LIMIT: usize = 2000;
/// Largest texture width or height `build_texture` accepts.
const MAX_TEXTURE_SIZE: u16 = 4096;

/// Whether `name` passes the optional glob filter.
fn passes(name: &str, pattern: Option<&str>) -> bool {
    pattern.is_none_or(|glob| glob_match(name.as_bytes(), glob))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = textures_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the composite texture definitions of the session.
    #[tool(
        description = "Lists the composite textures the merged view's TEXTURE1/TEXTURE2 lumps define, in lump order, with name, dimensions, patch count and source lump; pattern is a case-insensitive glob. Use it before build_texture, remove_texture or any sidedef edit so the name you write exists; get_asset_url publishes one as an image."
    )]
    fn list_textures(
        &self,
        Parameters(args): Parameters<ListTexturesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match Page::parse(args.offset, args.limit, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_textures_run(session, &page, args.pattern.as_deref()));
        reply(report)
    }

    /// Lists the merged `PNAMES` patch-name index.
    #[tool(
        description = "Lists the merged view's PNAMES entries in index order: {index, name, lump_found} - the index is exactly what TEXTURE1/TEXTURE2 patch references carry, and lump_found false marks a dangling name. Use it when a patch index matters; edit_pnames edits the list and keeps the references truthful."
    )]
    fn list_pnames(
        &self,
        Parameters(args): Parameters<ListPnamesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match Page::parse(args.offset, args.limit, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_pnames_run(session, &page, args.pattern.as_deref()));
        reply(report)
    }

    /// Edits `PNAMES` and renumbers every texture reference.
    #[tool(
        description = "Edits the merged view's PNAMES patch-name list as one undoable edit and renumbers the TEXTURE1/TEXTURE2 patch references so the indices stay truthful: exactly one of add, remove, rename ({from, to} pairs that keep their slots) or replace_all (the complete new list). For one patch at a time prefer add_patch, replace_patch and remove_patch."
    )]
    fn edit_pnames(
        &self,
        Parameters(args): Parameters<EditPnamesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = match pick_operation(&args) {
            Ok(operation) => operation,
            Err(error) => return fail(&error),
        };
        let report = self.state().with_session(|session| {
            edit_pnames_run(session, operation, args.force.unwrap_or(false))
        });
        reply(report)
    }

    /// Defines a composite texture record in `TEXTURE1`/`TEXTURE2`.
    #[tool(
        description = "Defines a composite texture in the working archive's TEXTURE1/TEXTURE2 as ONE undoable edit: name, width and height, and a non-empty list of {patch, x, y} placements drawn in order, patch names taken from PNAMES (missing ones are added unless declined). Prefer create_texture_from_image when the texture comes from one picture - it processes, stores and defines together; use this to compose existing patches."
    )]
    fn build_texture(
        &self,
        Parameters(args): Parameters<BuildTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| build_texture_run(session, &args));
        reply(report)
    }

    /// Removes one texture definition from whichever `TEXTUREx` holds it.
    #[tool(
        description = "Removes a texture definition from whichever TEXTURE1/TEXTURE2 lump of the merged view defines it, written into the working archive as ONE undoable edit; the PNAMES list and the patch lumps are left alone. A name no texture lump defines is not_found - list_textures shows the names. validate_map then flags sidedefs still wearing the removed name."
    )]
    fn remove_texture(
        &self,
        Parameters(args): Parameters<RemoveTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| remove_texture_run(session, &args));
        reply(report)
    }
}

#[cfg(test)]
mod tests;
