//! Server behavior as seen by a real MCP client: identity, tool listing,
//! the `version` tool, protocol errors, and the real binary over stdio.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::process::Stdio;

    use rmcp::ServiceExt as _;
    use rmcp::model::{CallToolRequestParams, ErrorCode};
    use rmcp::service::ServiceError;
    use rmcp::transport::TokioChildProcess;
    use serde_json::{Value, json};
    use tokio::process::Command;

    use crate::support::{call, call_err, connect, connect_probe};

    const EXPECTED_VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

    #[tokio::test]
    async fn initialize_reports_identity_and_tools_capability() {
        let client = connect().await;
        let info = client.peer_info().expect("server info after initialize");
        let server_info = info.server_info.as_ref().expect("server implementation");
        assert_eq!(server_info.name, "cyberdemon", "server name");
        assert_eq!(
            server_info.version,
            env!("CARGO_PKG_VERSION"),
            "server version"
        );
        assert!(info.capabilities.tools.is_some(), "tools capability");
        assert_eq!(
            info.instructions.as_deref(),
            Some("Cyberdemon reads and edits Doom WAD files."),
            "instructions"
        );
    }

    #[tokio::test]
    async fn lists_the_meta_assets_defs_lumps_render_and_session_tools() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "get_asset_server_info",
                "get_render_settings",
                "get_session_info",
                "get_thing_types",
                "get_wad_info",
                "list_lumps",
                "merge_wad",
                "new_wad",
                "open_iwad",
                "open_wad",
                "save_wad",
                "search_lumps",
                "set_asset_capacity",
                "set_render_options",
                "set_render_resolution",
                "version",
            ],
            "sorted tool names"
        );
    }

    #[tokio::test]
    async fn version_reports_name_and_version() {
        let client = connect().await;
        let result = client
            .call_tool(CallToolRequestParams::new("version"))
            .await
            .expect("call version");
        assert_ne!(result.is_error, Some(true), "is_error");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(text, Some(EXPECTED_VERSION), "version text");
    }

    #[tokio::test]
    async fn unknown_tool_is_a_protocol_error() {
        let client = connect().await;
        let err = client
            .call_tool(CallToolRequestParams::new("no_such_tool"))
            .await
            .expect_err("unknown tool must fail");
        assert!(
            matches!(&err, ServiceError::McpError(e) if e.code == ErrorCode::INVALID_PARAMS),
            "expected INVALID_PARAMS McpError, got {err:?}"
        );
    }

    /// Exercises `main.rs`: spawns the real binary and speaks MCP over its stdio.
    #[tokio::test]
    async fn binary_serves_mcp_over_stdio() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyberdemon"));
        command.stderr(Stdio::null());
        let transport = TokioChildProcess::new(command).expect("spawn binary");
        let mut client = tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
            .await
            .expect("initialize timed out")
            .expect("initialize");
        let info = client.peer_info().expect("server info");
        let name = info.server_info.as_ref().map(|i| i.name.as_str());
        assert_eq!(name, Some("cyberdemon"), "server name over stdio");
        let result = client
            .call_tool(CallToolRequestParams::new("version"))
            .await
            .expect("call version");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(text, Some(EXPECTED_VERSION), "version text over stdio");
        client.close().await.expect("close");
    }

    #[tokio::test]
    async fn call_returns_structured_success_content() {
        let client = connect_probe().await;
        let value = call(&client, "echo", json!({ "message": "E1M1" })).await;
        assert_eq!(value, json!({ "echo": "E1M1" }), "echo structured content");
    }

    #[tokio::test]
    async fn call_err_returns_the_documented_error_pair() {
        let client = connect_probe().await;
        let (code, message) = call_err(&client, "missing", Value::Null).await;
        assert_eq!(code, "not_found", "error code field");
        assert_eq!(message, "probe lump does not exist", "message field");
    }
}
