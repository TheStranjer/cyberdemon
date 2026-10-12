//! Asset tools: finding the asset server and tuning the store.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::store::MAX_ASSET_BYTES;
use crate::server::Cyberdemon;
use crate::tools::{fail, ok};

/// Arguments for [`Cyberdemon::set_asset_capacity`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetAssetCapacityParams {
    /// New number of most recent assets the store keeps, within
    /// `1..=10000`; shrinking the store below its current fill evicts the
    /// oldest assets immediately.
    capacity: usize,
}

/// Structured result of [`Cyberdemon::get_asset_server_info`].
#[derive(Serialize)]
struct AssetServerInfo {
    /// The origin every asset `URL` of this session shares, e.g.
    /// `http://localhost:41234`.
    base_url: String,
    /// The `URL` to `PUT` a file body to; replace `{filename}` with the
    /// name to store it under.
    upload_url_template: String,
    /// The maximum number of assets the store keeps.
    capacity: usize,
    /// The number of assets currently stored.
    stored: usize,
    /// The total bytes held by the stored assets.
    stored_bytes: usize,
    /// The largest single asset the store accepts, in bytes.
    max_asset_bytes: usize,
}

/// Structured result of [`Cyberdemon::set_asset_capacity`].
#[derive(Serialize)]
struct CapacityReport {
    /// The capacity the store now has.
    capacity: usize,
    /// The number of oldest assets the change evicted.
    evicted: usize,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = assets_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports the asset server's origin and the store's current fill.
    #[tool(
        description = "Reports the localhost asset server (base URL and upload URL template) and the store's capacity and current fill; call it before uploading files or checking what is stored. Starts the asset server if it is not running yet."
    )]
    async fn get_asset_server_info(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state();
        let base_url = match state.asset_base_url() {
            Ok(url) => url,
            Err(error) => return fail(&error),
        };
        let info = state.with_assets(|store| {
            Ok(AssetServerInfo {
                base_url: base_url.clone(),
                upload_url_template: format!("{base_url}/upload/{{filename}}"),
                capacity: store.capacity(),
                stored: store.len(),
                stored_bytes: store.total_bytes(),
                max_asset_bytes: MAX_ASSET_BYTES,
            })
        });
        match info {
            Ok(value) => ok(&value),
            Err(error) => fail(&error),
        }
    }

    /// Resizes the asset store at runtime.
    #[tool(
        description = "Sets how many of the most recent assets the store keeps (1..=10000) and reports how many the change evicted; use it when the default of 50 keeps too few or too many generated files in memory."
    )]
    async fn set_asset_capacity(
        &self,
        Parameters(args): Parameters<SetAssetCapacityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self.state().with_assets(|store| {
            let evicted = store.set_capacity(args.capacity)?;
            Ok(CapacityReport {
                capacity: args.capacity,
                evicted,
            })
        });
        match report {
            Ok(value) => ok(&value),
            Err(error) => fail(&error),
        }
    }
}
