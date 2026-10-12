//! Shared harness: real rmcp clients over in-memory pipes, plus a probe
//! server that speaks the structured-result contract (`ok`/`fail`) so those
//! helpers are exercised end to end, not just compiled.

use core::time::Duration;

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
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

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

/// How long a raw-HTTP test request waits for the full reply.
const HTTP_REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Issue a raw `HTTP` `PUT` of `body` to `path` on the origin `base_url`
/// (e.g. `http://localhost:41234`) and return the reply's `(status, body)`.
///
/// The asset server is exercised without pulling in an `HTTP` client crate:
/// one `Connection: close` request on a fresh socket, whose end-of-stream is
/// the end of the reply.
///
/// # Panics
///
/// Panics if `base_url` is not a `http://localhost:<port>` origin, the
/// socket fails, or no complete reply arrives within [`HTTP_REPLY_TIMEOUT`].
#[inline]
pub async fn http_put(base_url: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let port_text = base_url
        .strip_prefix("http://localhost:")
        .expect("a http://localhost:<port> origin");
    let port: u16 = port_text.parse().expect("a numeric port");
    let request = format!(
        "PUT {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len(),
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the asset server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the request head");
    stream
        .write_all(body)
        .await
        .expect("write the request body");
    let mut raw = Vec::new();
    tokio::time::timeout(HTTP_REPLY_TIMEOUT, stream.read_to_end(&mut raw))
        .await
        .expect("the reply arrives within the timeout")
        .expect("read the whole reply");
    split_reply(&raw)
}

/// Split a raw `HTTP` reply into its status code and body bytes.
fn split_reply(raw: &[u8]) -> (u16, Vec<u8>) {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("the header terminator");
    let body_start = head_end.checked_add(4).expect("head offset fits");
    let head = core::str::from_utf8(&raw[..head_end]).expect("ascii header lines");
    let status_line = head.lines().next().expect("a status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("a status code")
        .parse()
        .expect("a numeric status code");
    (status, raw[body_start..].to_vec())
}
