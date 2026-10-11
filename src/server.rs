//! MCP server definition: identity, capabilities and the tool router.

use rmcp::{ServerHandler, tool, tool_handler, tool_router};

/// The Cyberdemon MCP server.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cyberdemon;

#[tool_router]
impl Cyberdemon {
    /// Reports the server's name and version.
    #[tool(description = "Returns the Cyberdemon server name and version.")]
    fn version() -> String {
        concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION")).to_owned()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's #[tool_handler] expands to async trait methods without awaits"
)]
#[tool_handler(
    name = "cyberdemon",
    instructions = "Cyberdemon reads and edits Doom WAD files."
)]
impl ServerHandler for Cyberdemon {}
