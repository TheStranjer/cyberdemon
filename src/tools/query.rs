//! Query tools: `find_elements`, the set-valued read behind every
//! plain-English "all the X" request, CD-108's bulk `replace_*` swaps and
//! the `bulk_edit` field editor those reads drive.
//!
//! The name globs of `replace_texture` and `replace_flat` and the
//! doomednum-or-name sides of `replace_thing_type` fold their per-map
//! counts across whole maps in one undoable step. The filtering is the pure
//! [`crate::wad::map::query`] engine's; the group resolves the flattened
//! query parameters, reads the map and the session's definitions (custom
//! `DECORATE`/`DEHACKED` things included) under one lock exactly like the
//! playability group does, runs the query and pages the result: `count`
//! totals every hit, `indices` and `rows` carry one ascending page of the
//! hits with a compact summary per kind, and `next_offset` names the
//! `offset` where the rest continues. `bulk_edit` takes the very same
//! selection and hands its indices to the pure
//! [`crate::wad::map::bulk::edits`] engine as one
//! [`crate::session::Session::edit_map`] step, with a `dry_run` preview and
//! a `max` guard over the matched set. The bulk edits act on exactly the
//! elements `find_elements` reports; the session-facing machinery they
//! share (the `maps` selector, the one-undo-step-over-many-maps runner, the
//! texture stock check and the folded [`replace::ReplaceReport`]) lives in
//! the public `replace` submodule; the pure per-map passes are
//! [`crate::wad::map::bulk::replace_textures`] and its siblings.
//!
//! The work is split three ways below this root: the `find_elements`
//! parameters, summary rows and paging are the private `find` module, the
//! three `replace_*` runs the private `replace_tools` one and the
//! `bulk_edit` request, report and steps the private `bulk` one. This file
//! keeps the row-cap and report-window constants and the whole
//! [`Cyberdemon`] `#[tool_router]` block; the moved items are reached
//! through their `pub(super)` visibility, so no outside import path
//! changes. The tests are the private `tests` module, its fixture builders
//! shared, the read, the swaps and the bulk edit one topic file apiece.

pub mod replace;

mod bulk;
mod find;
mod replace_tools;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::server::Cyberdemon;
use crate::tools::query::bulk::{BulkEditParams, bulk_run};
use crate::tools::query::find::{FindElementsParams, find_run};
use crate::tools::query::replace_tools::{
    ReplaceFlatParams, ReplaceTextureParams, ReplaceThingTypeParams, flat_run, texture_run,
    thing_type_run,
};
use crate::tools::reply;

/// Default row cap for the `indices` and `rows` lists.
const DEFAULT_LIMIT: usize = 200;
/// Largest row cap the lists accept.
const MAX_LIMIT: usize = 1000;
/// Default `max` for [`Cyberdemon::bulk_edit`]: more matches refuse.
const DEFAULT_MAX: usize = 5000;
/// Longest `indices` list a `bulk_edit` report carries.
const INDEX_PAGE: usize = 500;
/// Longest `preview` list a `bulk_edit` report carries.
const PREVIEW_PAGE: usize = 20;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = query_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Runs a query over one map and reports the matching element indices.
    #[tool(
        description = "Answers \"which elements?\" as a set instead of a scan: runs a {kind, where, within} query over one map - sectors, linedefs, sidedefs, things or vertices, ANDed {field, op, value} filters over stored and derived values (globs, flag names, sector roles), optionally bounded by bbox or sector list. Returns {count, indices, rows, next_offset}. Use it to pick indices for the edit tools, and as the selection for bulk_edit."
    )]
    fn find_elements(
        &self,
        Parameters(args): Parameters<FindElementsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(find_run(self.state(), args))
    }

    /// Applies one field-edit set to every element of a query selection.
    #[tool(
        description = "Query-driven bulk editing: selects exactly like find_elements ({kind, where, within}) and applies set/add amounts and named flag edits to every matched element as ONE undoable step - sector heights, light, flats, special and tag; sidedef textures and offsets; linedef special, tag and args; thing type, position and angle; vertex x/y add-only. Values are range-checked before anything writes; dry_run previews and max caps the set. Prefer it over looping the edit_* tools."
    )]
    fn bulk_edit(
        &self,
        Parameters(args): Parameters<BulkEditParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(bulk_run(self.state(), args))
    }

    /// Swaps wall texture names across whole maps in one undoable step.
    #[tool(
        description = "Swaps wall textures across maps in ONE undoable step: rewrites every sidedef face whose texture the case-insensitive glob from matches to the name to. maps selects the blocks (default the working archive), dry_run previews, and the report totals changes per map and names the blocks copied in. to should exist - list_textures shows names or create_texture_from_image makes one; undo takes the whole swap back."
    )]
    fn replace_texture(
        &self,
        Parameters(args): Parameters<ReplaceTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| texture_run(session, &args)),
        )
    }

    /// Swaps flat names across whole maps in one undoable step.
    #[tool(
        description = "Renames floor and ceiling flats across maps in ONE undoable step: every sector surface whose flat the case-insensitive glob from matches becomes to. maps selects the blocks (default the working archive), dry_run previews and the report counts per-map changes and copied blocks. list_flats shows the names; prefer it over looping edit_sector's floor_flat and ceiling_flat."
    )]
    fn replace_flat(
        &self,
        Parameters(args): Parameters<ReplaceFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| flat_run(session, &args)),
        )
    }

    /// Rewrites thing type numbers across whole maps in one undoable step.
    #[tool(
        description = "Changes thing types across maps in ONE undoable step: rewrites every thing whose type equals from to the type to - each given as a doomednum or a definition name resolved through get_thing_types (an unknown or ambiguous name is refused listing the candidates). maps selects the blocks, max guards the count, dry_run previews and allow_unknown accepts an undefined target."
    )]
    fn replace_thing_type(
        &self,
        Parameters(args): Parameters<ReplaceThingTypeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| thing_type_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
