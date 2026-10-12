//! Meta tools: server identity and liveness.

use rmcp::tool;
use rmcp::tool_router;

use crate::server::Cyberdemon;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one inherent impl block per #[tool_router]"
)]
#[tool_router(router = meta_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports the server's name and version.
    #[tool(description = "Returns the Cyberdemon server name and version.")]
    fn version() -> String {
        concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION")).to_owned()
    }
}
