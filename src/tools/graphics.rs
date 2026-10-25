//! Graphics tools: the flats, patch and sprite listings of the merged view
//! and the image tools that publish one picture as an asset.
//!
//! Every tool reads [`crate::session::Scope::Merged`] through the shared
//! rows in `crate::tools::lumps::rows`: the lumps of one namespace, last
//! lump with a name wins. `list_flats` describes the floor and ceiling
//! planes of the `F_START` region, `list_patches` the texture components
//! of the `P_START` region (plus any lump `PNAMES` names from outside the
//! flats and sprites regions), and `list_sprites` groups the `S_START`
//! region by the sprite name grammar of
//! [`crate::gfx::sprite_name::parse_sprite_name`]. The listings decode no
//! pixels: sizes, dimensions, offsets and names only.
//!
//! `get_flat`, `get_patch`, `get_sprite`, `get_thing_sprite` and
//! `decode_patch` do read pixels, through the shared resolvers in
//! [`resolve`], and never inline them: each publishes a `PNG` or a raw
//! `RGBA` asset and returns the [`crate::assets::server::AssetRef`] with
//! the dimensions and drawing offsets it encoded. `get_thing_sprite`
//! starts from a thing type number and reads its sprite through the
//! session-aware definitions of [`crate::defs::session_definitions`].
//!
//! This file keeps what the group shares: the limits, the page, glob and
//! last-wins helpers every listing runs on, and the router. Each tool's own
//! arguments, reports and work live beside them - `list` is the flats and
//! patch listings, `list_sprites` the sprite grouping, `picture` the
//! options, render and publication half the five image tools share, `get`
//! the flat, patch and sprite readers, `thing_sprite` the thing-type
//! preview built on the session's definitions and `decode` the patch
//! reader that answers with both encodings.

pub mod resolve;

mod decode;
mod get;
mod list;
mod list_sprites;
mod picture;
mod thing_sprite;

use alloc::collections::BTreeSet;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::tools::graphics::decode::{DecodePatchParams, decode_patch_run};
use crate::tools::graphics::get::{
    GetFlatParams, GetPatchParams, GetSpriteParams, get_flat_run, get_patch_run, get_sprite_run,
    sprite_request,
};
use crate::tools::graphics::list::{
    ListFlatsParams, ListPatchesParams, list_flats_run, list_patches_run,
};
use crate::tools::graphics::list_sprites::{ListSpritesParams, list_sprites_run, parse_prefix};
use crate::tools::graphics::picture::parse_options;
use crate::tools::graphics::thing_sprite::{
    GetThingSpriteParams, get_thing_sprite_run, thing_request,
};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::{Page, fail, reply};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::{LumpName, glob_match};

// The thing-table game default, kept at the path the `query`, `render` and
// `playability` groups already import; the function lives beside its tool.

/// Default page size for the listing tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page any listing tool will return.
const MAX_LIMIT: usize = 2000;
/// Characters of the four-character sprite prefix `list_sprites` filters by.
const SPRITE_PREFIX_LEN: usize = 4;
/// Default `PLAYPAL` entry the image tools colour a picture with.
const DEFAULT_PALETTE: usize = 0_usize;
/// Default published-image scale: the stored size.
const DEFAULT_SCALE: u32 = 1_u32;
/// Largest whole-pixel upscale the image tools accept.
const MAX_SCALE: u32 = 8_u32;
/// Frame letter `get_sprite` assumes without one.
const DEFAULT_FRAME: char = 'A';
/// Rotation `get_sprite` assumes without one: the first of the eight views.
const DEFAULT_ROTATION: u8 = 1_u8;
/// Largest rotation number a sprite frame serves.
const MAX_ROTATION: u8 = 8_u8;
/// Name `decode_patch` publishes under when a `source` names no file.
const FALLBACK_NAME: &str = "PATCH";
/// Media type of the raw `RGBA8` assets.
const RGBA_MIME: &str = "application/octet-stream";

/// Whether `name` passes the optional glob filter.
fn passes(name: LumpName, pattern: Option<&str>) -> bool {
    pattern.is_none_or(|glob| glob_match(name.as_bytes(), glob))
}

/// Keeps the last occurrence of every lump name: a name lookup over the
/// merged view resolves to the last lump, so a listing shows one row per
/// name even when one archive repeats it.
fn last_wins(rows: Vec<&LumpRow>) -> Vec<&LumpRow> {
    let mut seen: BTreeSet<LumpName> = BTreeSet::new();
    let mut kept: Vec<&LumpRow> = rows
        .into_iter()
        .rev()
        .filter(|row| seen.insert(row.name))
        .collect();
    kept.reverse();
    kept
}

/// The merged directory's rows, for a namespace listing.
///
/// # Errors
///
/// [`build_rows`]' errors for the merged scope.
fn merged_rows(session: &Session) -> Result<Vec<LumpRow>, CyberError> {
    build_rows(session, Scope::Merged)
}

/// Whether `row` is a data lump of `namespace` (markers are not listed).
fn in_namespace(row: &LumpRow, namespace: Namespace) -> bool {
    row.kind != LumpKind::Marker && row.namespace == Some(namespace)
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = graphics_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the flat textures of the session.
    #[tool(
        description = "Lists the flat lumps of the merged view's flats regions (F_START/F_END or FF_START/FF_END markers, last same-name copy winning as an engine reads), with name, size, format, dimensions and source archive. pattern is a case-insensitive glob. Use it to find a flat before edit_sector or replace_flat; get_flat publishes one as a picture."
    )]
    fn list_flats(
        &self,
        Parameters(args): Parameters<ListFlatsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match Page::parse(args.offset, args.limit, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_flats_run(session, &page, args.pattern.as_deref()));
        reply(report)
    }

    /// Lists the patch lumps of the session.
    #[tool(
        description = "Lists the patch lumps of the merged view's patches regions (P_START/P_END or PP_START/PP_END, plus PNAMES-listed loose copies), last same-name winning, with dimensions, offsets and source. pattern filters names. Use it before add_patch or build_texture to see what exists; get_patch publishes one as an image."
    )]
    fn list_patches(
        &self,
        Parameters(args): Parameters<ListPatchesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match Page::parse(args.offset, args.limit, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self.state().with_session(|session| {
            list_patches_run(
                session,
                &page,
                args.pattern.as_deref(),
                args.in_pnames_only.unwrap_or(false),
            )
        });
        reply(report)
    }

    /// Lists the sprite lumps of the session, grouped by sprite.
    #[tool(
        description = "Groups the merged view's sprite-region lumps (S_START/S_END or SS_START/SS_END) by sprite, frame and the rotations their names serve, with an optional prefix filter. Use it before import_image_as_sprite or create_sprite_from_image to see which rotations exist; get_sprite publishes one rotation as a picture."
    )]
    fn list_sprites(
        &self,
        Parameters(args): Parameters<ListSpritesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match Page::parse(args.offset, args.limit, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let prefix = match parse_prefix(args.prefix.as_deref()) {
            Ok(prefix) => prefix,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_sprites_run(session, &page, prefix.as_deref()));
        reply(report)
    }

    /// Publishes one flat as an image asset.
    #[tool(
        description = "Publishes one flat as an image asset on the localhost server and reports its dimensions; nothing is edited. name resolves the way an engine resolves it in the merged view (last flats-region match wins); raw vanilla sizes and ZDoom PNG flats decode, palette picks the PLAYPAL entry, scale upscales and format picks png or rgba. A name no archive holds is not_found - list_flats shows the names."
    )]
    fn get_flat(
        &self,
        Parameters(args): Parameters<GetFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        let name = match LumpName::parse(&args.name) {
            Ok(name) => name,
            Err(error) => return fail(&error),
        };
        reply(get_flat_run(self.state(), name, &options))
    }

    /// Publishes one patch lump as an image asset.
    #[tool(
        description = "Publishes one patch lump (a texture component, HUD or port graphic) as a PNG image asset with its drawing offsets, or raw RGBA bytes with format \"rgba\"; nothing is edited. name resolves the merged view with the last patches-region match winning; palette and scale tune the picture. list_patches shows the names; decode_patch also reads external files."
    )]
    fn get_patch(
        &self,
        Parameters(args): Parameters<GetPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        let name = match LumpName::parse(&args.name) {
            Ok(name) => name,
            Err(error) => return fail(&error),
        };
        reply(get_patch_run(self.state(), name, &options))
    }

    /// Publishes one sprite lump or rotation as an image asset.
    #[tool(
        description = "Publishes one sprite rotation as an image asset; nothing is edited. Either name gives a whole sprite lump name served as stored, or sprite with optional frame (default A) and rotation (0..=8, default 1) resolves the way the engine's viewer-angle buckets do, mirrored frames included. Both or neither is invalid_argument. get_thing_sprite starts from a doomednum."
    )]
    fn get_sprite(
        &self,
        Parameters(args): Parameters<GetSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = match sprite_request(&args) {
            Ok(request) => request,
            Err(error) => return fail(&error),
        };
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        reply(get_sprite_run(self.state(), request, &options))
    }

    /// Previews one thing type's sprite rotation as an image asset.
    #[tool(
        description = "Publishes what one thing type looks like in game as an image asset; nothing is edited. type is looked up in the session-aware definitions - the detected game's table overlaid with DEHACKED, DECORATE, ZScript and MAPINFO customs, so load the archives first; optional frame and rotation pick the view. An unknown type is not_found: get_thing_types lists the numbers."
    )]
    fn get_thing_sprite(
        &self,
        Parameters(args): Parameters<GetThingSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = match thing_request(&args) {
            Ok(request) => request,
            Err(error) => return fail(&error),
        };
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        reply(get_thing_sprite_run(self.state(), &request, &options))
    }

    /// Decodes a Doom-format patch from a lump or a file and publishes it
    /// as both a PNG and raw RGBA.
    #[tool(
        description = "Decodes one Doom-format patch - from a session lump (name) or an external file (source; exactly one) - and publishes it both ways: a PNG with the grAb offsets and raw RGBA8 bytes, so you can pick an encoding; nothing is edited. Tall DeePsea patches read; PNG bytes are not patch data, use get_asset_url for those."
    )]
    async fn decode_patch(
        &self,
        Parameters(args): Parameters<DecodePatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(decode_patch_run(self.state(), &args).await)
    }
}

#[cfg(test)]
mod tests;
