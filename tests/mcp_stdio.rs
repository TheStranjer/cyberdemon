//! End-to-end tests: a real rmcp client talking to the server, both over an
//! in-memory pipe and over the real binary's stdin/stdout.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::process::Stdio;

    use anyhow::Result;
    use cyberdemon::Cyberdemon;
    use rmcp::{
        RoleClient, ServiceExt as _,
        model::{CallToolRequestParams, ErrorCode},
        service::{RunningService, ServiceError},
        transport::TokioChildProcess,
    };
    use tokio::process::Command;

    const EXPECTED_VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

    async fn connect() -> Result<RunningService<RoleClient, ()>> {
        let (server_io, client_io) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let server = Cyberdemon.serve(server_io).await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
        Ok(().serve(client_io).await?)
    }

    #[tokio::test]
    async fn initialize_reports_identity_and_tools_capability() {
        let client = connect().await.expect("connect");
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
    async fn lists_exactly_the_version_tool() {
        let client = connect().await.expect("connect");
        let tools = client.list_all_tools().await.expect("list tools");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, ["version"], "tool names");
    }

    #[tokio::test]
    async fn version_reports_name_and_version() {
        let client = connect().await.expect("connect");
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
        let client = connect().await.expect("connect");
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
}
