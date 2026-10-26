//! MCP server definition: identity, capabilities and the tool router.

use alloc::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::tool_handler;

use crate::state::AppState;

/// The Cyberdemon MCP server.
///
/// One instance is created per client connection; it owns the shared
/// [`AppState`] (behind an [`Arc`]) and the composed [`ToolRouter`], and
/// clones cheaply.
#[derive(Clone)]
pub struct Cyberdemon {
    /// State shared by every tool call on this connection.
    state: Arc<AppState>,
    /// All tools served by this connection.
    tool_router: ToolRouter<Self>,
}

impl Cyberdemon {
    /// Creates a server with a fresh session and the full tool router.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            state: Arc::new(AppState::default()),
            tool_router: Self::router(),
        }
    }

    /// The composed tool router: every tool group of the server, merged.
    ///
    /// Each ticket adds its tools as another `#[tool_router]` impl block in
    /// `src/tools/` and merges its router here.
    #[inline]
    fn router() -> ToolRouter<Self> {
        let mut router = Self::meta_router();
        router.merge(Self::assets_router());
        router.merge(Self::batch_router());
        router.merge(Self::audio_router());
        router.merge(Self::convert_router());
        router.merge(Self::defs_router());
        router.merge(Self::edit_geometry_router());
        router.merge(Self::edit_linedefs_router());
        router.merge(Self::edit_sectors_router());
        router.merge(Self::edit_sidedefs_router());
        router.merge(Self::edit_things_router());
        router.merge(Self::edit_vertices_router());
        router.merge(Self::features_router());
        router.merge(Self::graphics_router());
        router.merge(Self::history_router());
        router.merge(Self::imports_router());
        router.merge(Self::inspect_router());
        router.merge(Self::lumps_router());
        router.merge(Self::maps_router());
        router.merge(Self::music_router());
        router.merge(Self::nodes_router());
        router.merge(Self::palette_router());
        router.merge(Self::patches_router());
        router.merge(Self::plan_router());
        router.merge(Self::playability_router());
        router.merge(Self::query_router());
        router.merge(Self::render_router());
        router.merge(Self::scripts_router());
        router.merge(Self::session_router());
        router.merge(Self::sketch_router());
        router.merge(Self::textures_router());
        router.merge(Self::themes_router());
        router.merge(Self::validate_router());
        router
    }

    /// The shared state of this connection, for tool implementations.
    ///
    /// Public (rather than crate-visible) so the accessor survives until the
    /// first tools call it: an unused `pub(crate)` item is a `dead_code`
    /// error in this repo.
    #[must_use]
    #[inline]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// The composed tool router, for the one tool that runs the others
    /// (`run_batch`).
    #[must_use]
    #[inline]
    pub(crate) const fn tool_router(&self) -> &ToolRouter<Self> {
        &self.tool_router
    }

    /// The tool groups a `run_batch` step may never dispatch, composed as
    /// one router: the batch group (a batch cannot nest inside itself) and
    /// the history group (`undo` and `redo`, which would fight the open
    /// batch's single entry). Membership is the refusal — a property of the
    /// groups, so a tool added to either is refused in batches without any
    /// hand-kept name list. Every other state a batch cannot survive
    /// (whole-archive and saved-state swaps) is refused at runtime by the
    /// session itself while a batch is open.
    #[must_use]
    pub(crate) fn non_nestable_router() -> ToolRouter<Self> {
        let mut router = Self::batch_router();
        router.merge(Self::history_router());
        router
    }
}

impl Default for Cyberdemon {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's #[tool_handler] expands to async trait methods without awaits"
)]
#[tool_handler(
    router = self.tool_router,
    name = "cyberdemon",
    instructions = "Cyberdemon reads and edits Doom WAD files over MCP: maps, graphics, audio and game definitions, with every edit undoable.
Start a session with open_iwad (the game data), then new_wad for a fresh project archive or open_wad for an existing file.
Prefer high-level tools over element edits: build_map_from_plan compiles a whole playable map from one plan; add_door, add_window, add_stairs, add_lift, add_teleporter, add_switch and add_exit place working features; create_texture_from_image, create_flat_from_image and create_sprite_from_image turn pictures into finished assets; bulk_edit changes many elements at once; place_things populates rooms.
The element tools (add_linedef, add_sector, add_vertex, edit_thing, ...) are fine control, not first choices.
For a hand-drawn sketch: trace_sketch measures it (no WAD needed), turn the reported regions and gaps into a plan, build_map_from_plan compiles it, then render_topdown with an underlay draws the picture behind the lines.
Verify after every meaningful change: validate_map for map health, check_playability for whether a player can finish, render_topdown and render_view to look.
Use run_batch to make a multi-step change atomic and reversible as one step; undo (and redo) take changes back.
Failed calls return an error object with a hint field: do what the hint says before retrying.
Image and sound results are URLs on the server's localhost asset origin - open them to look or listen.
The skill's guides/ directory holds the judgment guides: guides/design-rules.md, guides/verification.md, guides/recipes.md, guides/sketch-to-map.md, guides/editing-existing-maps.md and guides/images-to-assets.md."
)]
impl ServerHandler for Cyberdemon {}
