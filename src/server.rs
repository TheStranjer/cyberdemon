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
        router.merge(Self::edit_sectors_router());
        router.merge(Self::edit_sidedefs_router());
        router.merge(Self::edit_things_router());
        router.merge(Self::graphics_router());
        router.merge(Self::imports_router());
        router.merge(Self::inspect_router());
        router.merge(Self::lumps_router());
        router.merge(Self::maps_router());
        router.merge(Self::music_router());
        router.merge(Self::nodes_router());
        router.merge(Self::palette_router());
        router.merge(Self::render_router());
        router.merge(Self::scripts_router());
        router.merge(Self::session_router());
        router.merge(Self::textures_router());
        router.merge(Self::themes_router());
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
    instructions = "Cyberdemon reads and edits Doom WAD files."
)]
impl ServerHandler for Cyberdemon {}
