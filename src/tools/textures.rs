//! Texture tools: the composite-texture definitions of the merged view and
//! the copy-on-write `PNAMES` editing behind `edit_pnames`.
//!
//! [`TextureSet`] loads the merged `PNAMES`, `TEXTURE1` and `TEXTURE2`
//! lumps (the last copy of each name, decoded with the codecs in
//! [`crate::wad::textures`]); `list_textures` and `list_pnames` page
//! through it, `edit_pnames` rewrites the name list renumbering every
//! texture patch reference so the indices stay truthful, and
//! `build_texture`/`remove_texture` define and drop single texture
//! records (appending needed `PNAMES` names along the way). The removal
//! half of that machinery is reusable on its own
//! (`plan_pnames_removal`/`write_pnames_rewrite`) so other tools can
//! fold a `PNAMES` removal and its renumbering into one edit. Every write
//! goes through exactly one [`crate::session::Session::edit`], bringing
//! the lumps into the working archive as overrides.
//!
//! `textures/list` holds the two listings' arguments, rows and runs,
//! `textures/pnames` the `edit_pnames` arguments and the plan one call
//! computes, `textures/pnames_rewrite` the rewrite that commits it and the
//! reusable removal, `textures/build` the two single-texture tools with the
//! definition steps the texture import shares, and `textures/tests` the
//! group's unit tests. This root keeps the constants, the [`TextureSet`]
//! loader with its decode and paging helpers, the re-exports the other
//! groups import and the router methods; every global-lump write these
//! tools run goes through [`crate::wad::archive::Wad::put_global`].

mod build;
mod list;
mod pnames;
mod pnames_rewrite;

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{MergedLump, Session, Source};
use crate::tools::textures::build::{
    BuildTextureParams, RemoveTextureParams, build_texture_run, remove_texture_run,
};
use crate::tools::textures::list::{
    ListPnamesParams, ListTexturesParams, TextureRow, list_pnames_run, list_textures_run,
};
use crate::tools::textures::pnames::EditPnamesParams;
use crate::tools::textures::pnames_rewrite::{edit_pnames_run, pick_operation};
use crate::tools::{Page, fail, reply};
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::name::glob_match;
use crate::wad::textures::{TextureDef, TextureLump, parse_pnames, parse_texture_lump};

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

/// The lump a listed texture came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhichLump {
    /// The merged view's `TEXTURE1` lump.
    Texture1,
    /// The merged view's `TEXTURE2` lump.
    Texture2,
}

impl WhichLump {
    /// The lump name clients see in the `lump` field.
    pub(in crate::tools) const fn as_str(self) -> &'static str {
        match self {
            Self::Texture1 => "TEXTURE1",
            Self::Texture2 => "TEXTURE2",
        }
    }

    /// The name this lump carries.
    pub(in crate::tools) const fn name(self) -> LumpName {
        match self {
            Self::Texture1 => lumps::TEXTURE1,
            Self::Texture2 => lumps::TEXTURE2,
        }
    }

    /// The set's decoded entry for this lump, `None` when no archive
    /// holds it.
    const fn decoded(self, set: &TextureSet) -> Option<&(TextureLump, Source)> {
        match self {
            Self::Texture1 => set.texture1.as_ref(),
            Self::Texture2 => set.texture2.as_ref(),
        }
    }
}

/// The merged view's texture definitions, loaded once per tool call.
///
/// `TEXTURE2` entries override `TEXTURE1` ones of the same name in
/// [`TextureSet::find`]; the listings show both lumps, each row tagged
/// with its origin. This is the session-level loader the texture tools
/// read and write through; it is public so embedding callers can reuse
/// the lookup (an unused crate-private item would not survive the
/// `dead_code` check).
#[derive(Debug)]
pub struct TextureSet {
    /// The merged `PNAMES` list; empty when no `PNAMES` lump exists.
    pnames: Vec<String>,
    /// The decoded `TEXTURE1` lump and where it came from.
    texture1: Option<(TextureLump, Source)>,
    /// The decoded `TEXTURE2` lump and where it came from.
    texture2: Option<(TextureLump, Source)>,
}

impl TextureSet {
    /// Reads the last (winning) `PNAMES`, `TEXTURE1` and `TEXTURE2` of the
    /// merged view. Without a `PNAMES` lump the patch indices inside the
    /// texture lumps stay unbounded.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when `PNAMES` is malformed or a texture lump
    /// decodes neither as the `Doom` nor as the `Strife` layout.
    #[inline]
    pub fn load(session: &Session) -> Result<Self, CyberError> {
        let pnames_lump = session.find_global(lumps::PNAMES);
        let pnames = match pnames_lump.as_ref() {
            Some(entry) => parse_pnames(&entry.lump.data)?,
            None => Vec::new(),
        };
        let bound = if pnames_lump.is_some() {
            pnames.len()
        } else {
            usize::MAX
        };
        Ok(Self {
            texture1: decode_texture_lump(session.find_global(lumps::TEXTURE1), bound)?,
            texture2: decode_texture_lump(session.find_global(lumps::TEXTURE2), bound)?,
            pnames,
        })
    }

    /// The texture named `name`, case-insensitively; a `TEXTURE2` entry
    /// wins over a `TEXTURE1` one of the same name.
    #[must_use]
    #[inline]
    pub fn find(&self, name: &str) -> Option<(&TextureDef, WhichLump)> {
        first_match(self.texture2.as_ref(), name)
            .map(|def| (def, WhichLump::Texture2))
            .or_else(|| {
                first_match(self.texture1.as_ref(), name).map(|def| (def, WhichLump::Texture1))
            })
    }

    /// The merged `PNAMES` names, in index order; empty when the session
    /// has no `PNAMES` lump, which leaves every texture's patch references
    /// unresolvable.
    #[must_use]
    #[inline]
    pub fn pnames(&self) -> &[String] {
        &self.pnames
    }

    /// Every texture of both lumps, in lump order, as listing rows.
    fn texture_rows(&self) -> Vec<TextureRow> {
        let mut rows = Vec::new();
        push_rows(&mut rows, self.texture1.as_ref(), WhichLump::Texture1);
        push_rows(&mut rows, self.texture2.as_ref(), WhichLump::Texture2);
        rows
    }

    /// The texture lumps a renumbering would touch, as
    /// `(lump, lump name)` pairs for conflict reporting.
    fn textures(&self) -> Vec<(&TextureLump, &'static str)> {
        let mut list = Vec::new();
        if let Some(pair) = self.texture1.as_ref() {
            list.push((&pair.0, WhichLump::Texture1.as_str()));
        }
        if let Some(pair) = self.texture2.as_ref() {
            list.push((&pair.0, WhichLump::Texture2.as_str()));
        }
        list
    }

    /// Every defined texture name (uppercased) with its width: the
    /// `TEXTURE1` records first, `TEXTURE2` wins on a duplicate name
    /// exactly as [`TextureSet::find`] does. The theme tools read the
    /// merged view's texture inventory through this.
    #[must_use]
    #[inline]
    pub fn widths(&self) -> BTreeMap<String, u16> {
        [&self.texture1, &self.texture2]
            .into_iter()
            .flatten()
            .flat_map(|pair| pair.0.textures.iter())
            .map(|def| (def.name.to_ascii_uppercase(), def.width))
            .collect()
    }
}

/// The first definition named `name` (case-insensitive) in one lump entry.
fn first_match<'lump>(
    entry: Option<&'lump (TextureLump, Source)>,
    name: &str,
) -> Option<&'lump TextureDef> {
    let pair = entry?;
    pair.0
        .textures
        .iter()
        .find(|def| def.name.eq_ignore_ascii_case(name))
}

/// One texture lump decoded together with the archive it came from.
type DecodedLump = Option<(TextureLump, Source)>;

/// Decodes one found texture lump; `None` stays `None`.
///
/// # Errors
///
/// [`crate::wad::textures::parse_texture_lump`]'s [`CyberError::Format`].
fn decode_texture_lump(
    found: Option<MergedLump>,
    pnames_len: usize,
) -> Result<DecodedLump, CyberError> {
    let Some(entry) = found else {
        return Ok(None);
    };
    let lump = parse_texture_lump(&entry.lump.data, pnames_len)?;
    Ok(Some((lump, entry.source)))
}

/// Appends `entry`'s textures to `rows`, tagged with `which` and its source.
fn push_rows(rows: &mut Vec<TextureRow>, entry: Option<&(TextureLump, Source)>, which: WhichLump) {
    let Some(pair) = entry else {
        return;
    };
    for def in &pair.0.textures {
        rows.push(TextureRow {
            name: def.name.clone(),
            width: def.width,
            height: def.height,
            patches: def.patches.len(),
            lump: which.as_str(),
            source: pair.1,
        });
    }
}

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
