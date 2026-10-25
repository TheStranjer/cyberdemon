//! Lump directory and payload tools: the browsing pair
//! [`Cyberdemon::list_lumps`] and [`Cyberdemon::search_lumps`], the
//! single-lump readers [`Cyberdemon::get_lump`] and
//! [`Cyberdemon::extract_lump`], and the editing tools
//! [`Cyberdemon::add_lump`], [`Cyberdemon::import_asset`],
//! [`Cyberdemon::import_file_as_lump`], [`Cyberdemon::replace_lump`],
//! [`Cyberdemon::remove_lump`], [`Cyberdemon::rename_lump`] and
//! [`Cyberdemon::reorder_lumps`].
//!
//! This file keeps what the group shares: the limits, the [`reply`] handoff
//! and the router. Each tool's own arguments, reports and work live
//! beside them - [`list`] is the browsing pair, built on the shared rows in
//! [`rows`] and the filters in [`parse`], [`get`] is the one-lump read on the
//! shared selector in [`select`], [`extract`] is the same selector writing
//! bytes to a file, and [`add`] is `add_lump` and the two import tools. The
//! rewriting tools are [`edit`], the move of a whole run is [`reorder`], and
//! both share the placement in [`place`].
//!
//! The scope, namespace, kind and limit parsers stay reachable through this
//! module, which is how [`select`], [`place`] and [`edit`] keep calling
//! their shared parsers.

mod add;
mod edit;
mod extract;
mod get;
mod list;
mod parse;
pub(in crate::tools) mod place;
mod reorder;
pub(in crate::tools) mod rows;
pub(in crate::tools) mod select;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::server::Cyberdemon;
use crate::tools::lumps::add::{AddLumpParams, ImportParams, commit, imported, plan_add};
use crate::tools::lumps::edit::{RemoveLumpParams, RenameLumpParams, ReplaceLumpParams};
use crate::tools::lumps::extract::{
    ExtractLumpParams, ExtractReport, extract_run, parse_target, path_text,
};
use crate::tools::lumps::get::{GetLumpParams, deliver, parse_get_options, run_get_lump};
use crate::tools::lumps::list::{
    ListLumpsParams, SearchLumpsParams, list_run, parse_listing, parse_search, search_run,
};
use crate::tools::lumps::parse::{hex_bytes, parse_namespace};
use crate::tools::lumps::reorder::ReorderLumpsParams;
use crate::tools::{fail, ok, reply};
use crate::wad::archive::write_file_atomic;

/// Default page size for both tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page either tool will return.
const MAX_LIMIT: usize = 2000;
/// Default byte count of a `get_lump` `hex` window.
const DEFAULT_HEX_LENGTH: usize = 256;
/// Largest byte count a `get_lump` `hex` window accepts.
const MAX_HEX_LENGTH: usize = 4_096;
/// Bytes of `preview_hex` the `auto` format attaches to published lumps.
const HEX_PREVIEW_BYTES: usize = 64;
/// Default character cap for a `get_lump` `text` body.
const DEFAULT_MAX_CHARS: usize = 100_000;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = lumps_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists one scope's lump directory with filters and paging.
    #[tool(
        description = "Lists one scope's lumps in directory order with index, name, size, kind, namespace, owning map and source archive. scope is merged (default: what an engine sees), working or layer:N; namespace, kind, map and a name pattern filter, offset and limit page. Use it to see what exists; search_lumps matches content instead."
    )]
    fn list_lumps(
        &self,
        Parameters(args): Parameters<ListLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let listing = match parse_listing(&args) {
            Ok(listing) => listing,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_run(session, &listing));
        reply(report)
    }

    /// Searches a scope's directory by name, type, namespace or content.
    #[tool(
        description = "Searches a scope's lumps by at least one criterion: pattern (a case-insensitive name glob with * and ?), contains_hex (a byte needle), kind, namespace and map filters, paged by offset and limit. Returns the matching rows. Use it whenever a lump name is unknown or a not_found error points here."
    )]
    fn search_lumps(
        &self,
        Parameters(args): Parameters<SearchLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (scope, limit, search) = match parse_search(&args) {
            Ok(parsed) => parsed,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| search_run(session, scope, limit, &search));
        reply(report)
    }

    /// Reads one lump through the shared selector.
    #[tool(
        description = "Reads one lump chosen by the shared selector: name plus optional scope (merged default, working, layer:N), namespace and occurrence (default the last, winning copy). format info returns the row fields, text the decoded string, hex a byte window, url the bytes as an asset URL, and auto (default) picks by the lump's kind. Use it to inspect any lump; extract_lump writes it to a file."
    )]
    fn get_lump(
        &self,
        Parameters(args): Parameters<GetLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let opts = match parse_get_options(&args) {
            Ok(opts) => opts,
            Err(error) => return fail(&error),
        };
        let outcome = match self
            .state()
            .with_session(|session| run_get_lump(session, &args.selector, &opts))
        {
            Ok(outcome) => outcome,
            Err(error) => return fail(&error),
        };
        reply(deliver(self.state(), outcome))
    }

    /// Writes one lump's raw bytes to a file.
    #[tool(
        description = "Writes one lump's raw bytes to an absolute filesystem path, atomically (staged in a unique file beside the target, then landed). Takes the same selector as get_lump; an existing target, one that appears mid-write included, is already_exists unless overwrite: true. Returns path and bytes. Use it when the user needs the real file; get_lump format=url only publishes it."
    )]
    fn extract_lump(
        &self,
        Parameters(args): Parameters<ExtractLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let target = match parse_target(&args.path) {
            Ok(target) => target,
            Err(error) => return fail(&error),
        };
        let bytes = match self
            .state()
            .with_session(|session| extract_run(session, &args.selector))
        {
            Ok(bytes) => bytes,
            Err(error) => return fail(&error),
        };
        match write_file_atomic(&target, &bytes, args.overwrite) {
            Ok(()) => ok(&ExtractReport {
                path: path_text(&target),
                bytes: bytes.len(),
            }),
            Err(error) => fail(&error),
        }
    }

    /// Adds a new lump to the working archive.
    #[tool(
        description = "Adds a new lump to the working archive as one undoable edit; read-only layers are never touched. name is 1 to 8 printable ASCII characters; the payload is at most one of text, hex or source (path or URL), or nothing for an empty marker; placement is at most one of index, before, after, namespace or map. Use plain lumps; pictures and sounds have their own import tools."
    )]
    async fn add_lump(
        &self,
        Parameters(args): Parameters<AddLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let pending = match plan_add(&args) {
            Ok(pending) => pending,
            Err(error) => return fail(&error),
        };
        reply(commit(self.state(), pending).await)
    }

    /// Fetches a source and inserts its bytes as a new lump.
    #[tool(
        description = "Fetches source (absolute path, file:// URI, http(s):// URL or this server's asset URL) and inserts its bytes into the working archive without conversion, as one undoable edit. name defaults to the fetched file's stem uppercased to 8 characters; placement works as in add_lump. Use it for ready-made lumps; import_image_as_raw checks images first."
    )]
    async fn import_asset(
        &self,
        Parameters(args): Parameters<ImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
        imported(self.state(), &args).await
    }

    /// Fetches a file and inserts its bytes as a new lump.
    #[tool(
        description = "Fetches source (absolute path, file:// URI, http(s):// URL or this server's asset URL) and inserts its bytes into the working archive without conversion, as one undoable edit. name defaults to the fetched file's stem uppercased to 8 characters; placement works as in add_lump. Use it for ready-made lumps; import_image_as_raw checks images first."
    )]
    async fn import_file_as_lump(
        &self,
        Parameters(args): Parameters<ImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
        imported(self.state(), &args).await
    }

    /// Overwrites one lump's payload, overriding a read-only copy when needed.
    #[tool(
        description = "Overwrites one lump's payload as a single undoable edit, and it is the way in for a lump the IWAD or a resource layer owns: the bytes land in the working archive as a winning override (created_override true) while the lower archive stays untouched. The payload is exactly one of text, hex or source. For patches prefer replace_patch, which knows PNAMES."
    )]
    async fn replace_lump(
        &self,
        Parameters(args): Parameters<ReplaceLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(edit::replace_lump(self.state(), &args).await)
    }

    /// Removes lumps from the working archive, all or nothing.
    #[tool(
        description = "Removes named lumps from the working archive as one undoable edit: names is a list, each resolved with the optional namespace and occurrence. A lump only a read-only layer holds cannot be removed - replace_lump overrides it instead. Reports the removed names and indices; undo brings them back. For patches prefer remove_patch, which maintains PNAMES and texture references."
    )]
    fn remove_lump(
        &self,
        Parameters(args): Parameters<RemoveLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| edit::remove_lump(session, &args));
        reply(report)
    }

    /// Renames one lump of the working archive.
    #[tool(
        description = "Renames one lump of the working archive as one undoable edit, keeping its position and bytes: old and new names are both 1 to 8 printable ASCII characters. Only working lumps can be renamed; a lower layer's copy with the old name keeps showing through the merged view. For map blocks use rename_map, which moves the markers too."
    )]
    fn rename_lump(
        &self,
        Parameters(args): Parameters<RenameLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| edit::rename_lump(session, &args));
        reply(report)
    }

    /// Moves a contiguous run of lumps within the working archive.
    #[tool(
        description = "Moves one contiguous run of lumps inside the working archive as a single undoable edit: the run is a name (with optional namespace and occurrence) or a from_index, plus count (a whole map block is its header with count 11); the destination is exactly one of index, before, after or namespace, resolved after the run is taken out. Returns the moved names and new position."
    )]
    fn reorder_lumps(
        &self,
        Parameters(args): Parameters<ReorderLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| reorder::reorder_lumps(session, &args));
        reply(report)
    }
}

#[cfg(test)]
mod tests;
