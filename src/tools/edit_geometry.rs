//! Multi-kind geometry editing: `delete_geometry` and
//! `transform_selection` (see the `edit_geometry` router methods on
//! [`crate::server::Cyberdemon`]).
//!
//! The two tools live in [`delete`] and [`transform`]: each file holds its
//! tool's parameter and report types beside the steps its router method
//! calls, and this root keeps only the two router methods and the outcome
//! wrapper; both tools read the map through [`crate::tools::maps::common`]
//! and report through [`crate::tools::maps::params`].
//!
//! `transform_selection` moves a mixed selection of vertices and things
//! through one scale/rotate/translate. The math and the selection
//! expansion live in the pure engine [`crate::wad::map::transform`];
//! [`transform`] parses the request, expands the selection against the map
//! as it now stands (so a bad index or an empty selection is refused
//! before anything is copied in) and runs the whole transform inside
//! exactly one [`crate::session::Session::edit_map`] step, which rolls the
//! map back untouched when the engine refuses an out-of-range coordinate or
//! a collapsing linedef.
//!
//! One call deletes a mixed selection of vertices, linedefs, sidedefs,
//! sectors and things with full reference cleanup. [`delete`] is a thin
//! wrapper around the pure removal engine
//! ([`crate::wad::map::remove`]): it validates the selection, builds a
//! [`Removal`], grows it with [`expand_cascade`] — the cascade always runs,
//! unlike the per-kind tools' opt-in `cascade` — and hands the grown set to
//! [`apply_removal`] inside exactly one
//! [`crate::session::Session::edit_map`] step, so one undo entry moves the
//! whole call. `dry_run` computes the same cascade on a copy and only
//! reports: nothing is written, no undo entry is made.
//!
//! The geometry rule: deleting any vertex, line, side or sector changes
//! geometry, so the generated node lumps are emptied, the block's `GL` run
//! is dropped and the result reports `nodes_invalidated: true`. The
//! invalidation sheds the split points the shed run named in its `SEGS`
//! records — the ones a `build_nodes` appended — and keeps every other
//! point, wired or not. Run `build_nodes` before playing such a map in
//! vanilla `Doom`; `ZDoom`-family ports rebuild the missing node data
//! themselves. A selection of things alone changes no buildable geometry and
//! keeps the nodes.
//!
//! [`Removal`]: crate::wad::map::remove::Removal
//! [`expand_cascade`]: crate::wad::map::remove::expand_cascade
//! [`apply_removal`]: crate::wad::map::remove::apply_removal

mod delete;
mod transform;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::server::Cyberdemon;
use crate::tools::edit_geometry::delete::{DeleteGeometryParams, delete_run};
use crate::tools::edit_geometry::transform::{TransformSelectionParams, transform_run};
use crate::tools::reply;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_geometry_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Deletes a mixed selection of map elements with full cleanup.
    #[tool(
        description = "Deletes a mixed selection of vertices, linedefs, sidedefs, sectors and things in a single undoable edit with full reference cleanup: sidedefs of removed sectors go, linedefs on removed vertices go, losing a front flips a line, losing a back makes it one-sided, and prune_sidedefs/prune_vertices (both default true) leave no orphans. dry_run reports the whole cascade without editing anything."
    )]
    fn delete_geometry(
        &self,
        Parameters(args): Parameters<DeleteGeometryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| delete_run(session, &args)),
        )
    }

    /// Moves a selection of vertices and things with one transform.
    #[tool(
        description = "Moves a selection of a map's vertices and things through one scale/rotate/translate transform in a single undoable edit; the math is integer-only and rounded. selection lists vertices, linedefs, sectors and things (or all: true - lines bring their endpoints, sectors their boundary), then scale (an exact fraction), rotate (degrees; things turn too) and translate run around origin, the selection's centre by default. Geometry: rebuild nodes after."
    )]
    fn transform_selection(
        &self,
        Parameters(args): Parameters<TransformSelectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(move |session| transform_run(session, args)),
        )
    }
}

#[cfg(test)]
mod tests;
