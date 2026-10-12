//! Shared harness: real rmcp clients over in-memory pipes, plus a probe
//! server that speaks the structured-result contract (`ok`/`fail`) so those
//! helpers are exercised end to end, not just compiled.

use cyberdemon::Cyberdemon;
use cyberdemon::error::CyberError;
use cyberdemon::tools::{fail, ok};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{ErrorData, RoleClient, ServerHandler, ServiceExt as _};
use rmcp::{tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// Buffer size for the in-memory pipes: 1 MiB, generous enough that no
/// message in these tests can block the writer.
const DUPLEX_BUFFER: usize = 0x10_0000;

/// Connect a real rmcp client to a spawned [`Cyberdemon`] over an in-memory pipe.
///
/// # Panics
///
/// Panics if the server cannot start or the client handshake fails; both
/// mean the server under test is broken.
#[inline]
pub async fn connect() -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(DUPLEX_BUFFER);
    tokio::spawn(async move {
        let server = Cyberdemon::new()
            .serve(server_io)
            .await
            .expect("serve cyberdemon");
        server.waiting().await.expect("cyberdemon run loop");
    });
    ().serve(client_io).await.expect("client handshake")
}

/// A probe server exercising the same router/result plumbing as
/// [`Cyberdemon`], with tools that return structured success and failure.
#[derive(Clone)]
struct Probe {
    /// The probe's tool router.
    tool_router: ToolRouter<Self>,
}

#[tool_router(router = probe_router)]
impl Probe {
    /// Creates the probe server with its tool router.
    fn new() -> Self {
        Self {
            tool_router: Self::probe_router(),
        }
    }

    /// Echoes the `message` argument back as structured content.
    #[tool(description = "Echo the message argument.")]
    async fn echo(
        &self,
        Parameters(args): Parameters<EchoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        ok(&json!({ "echo": args.message }))
    }

    /// Always fails with a `not_found` domain error.
    #[tool(description = "Always fail with a not_found error.")]
    async fn missing(&self) -> Result<CallToolResult, ErrorData> {
        fail(&CyberError::NotFound(
            "probe lump does not exist".to_owned(),
        ))
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's #[tool_handler] expands to async trait methods without awaits"
)]
#[tool_handler(router = self.tool_router, name = "probe")]
impl ServerHandler for Probe {}

/// Arguments for the probe's `echo` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct EchoArgs {
    /// The text to echo back.
    message: String,
}

/// Connect a real rmcp client to a spawned [`Probe`] server.
///
/// # Panics
///
/// Panics if the probe server cannot start or the client handshake fails.
#[inline]
pub async fn connect_probe() -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(DUPLEX_BUFFER);
    tokio::spawn(async move {
        let server = Probe::new().serve(server_io).await.expect("serve probe");
        server.waiting().await.expect("probe run loop");
    });
    ().serve(client_io).await.expect("probe client handshake")
}

/// Build call parameters; `Value::Null` means "no arguments".
fn params_for(tool: &str, args: Value) -> CallToolRequestParams {
    let params = CallToolRequestParams::new(tool.to_owned());
    if args.is_null() {
        params
    } else {
        params.with_arguments(rmcp::model::object(args))
    }
}

/// Call a tool, assert it succeeded, and return its structured content.
///
/// # Panics
///
/// Panics if the protocol call fails, the result carries `is_error`, or the
/// result has no structured content.
#[inline]
pub async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> Value {
    let result = client
        .call_tool(params_for(tool, args))
        .await
        .expect("tool call round trip");
    assert_ne!(
        result.is_error,
        Some(true),
        "tool {tool} unexpectedly failed: {result:?}"
    );
    result
        .structured_content
        .expect("structured content on success")
}

/// Call a tool, assert it failed with the documented error shape, and return
/// the `(error code, message)` pair from its structured content.
///
/// # Panics
///
/// Panics if the protocol call fails, the result is not an error, or the
/// structured content lacks the documented `error`/`message` string fields.
#[inline]
pub async fn call_err(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    args: Value,
) -> (String, String) {
    let result = client
        .call_tool(params_for(tool, args))
        .await
        .expect("tool call round trip");
    assert_eq!(
        result.is_error,
        Some(true),
        "tool {tool} unexpectedly succeeded: {result:?}"
    );
    let content = result
        .structured_content
        .expect("structured content on failure");
    let code = string_field(&content, "error");
    let message = string_field(&content, "message");
    (code, message)
}

/// Read a required string field out of a structured content object.
fn string_field(content: &Value, key: &str) -> String {
    let found = content.get(key).and_then(Value::as_str);
    let Some(text) = found else {
        panic!("structured content {content} has no string field {key:?}");
    };
    text.to_owned()
}
