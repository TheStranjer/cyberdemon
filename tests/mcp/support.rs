//! Shared harness: real rmcp clients over in-memory pipes.
//!
//! Plus a probe server that speaks the structured-result contract
//! (`ok`/`fail`) so those helpers are exercised end to end, not just
//! compiled, and small hand-written byte builders for the media files
//! (`PNG`, `MIDI`, `WAV`) the guide recipes import.

use core::time::Duration;
use std::io::Write as _;

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
/// the `(error code, message, hint)` triple from its structured content.
///
/// # Panics
///
/// Panics if the protocol call fails, the result is not an error, or the
/// structured content lacks the documented `error`/`message`/`hint` string
/// fields.
#[inline]
pub async fn call_err(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    args: Value,
) -> (String, String, String) {
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
    let hint = string_field(&content, "hint");
    (code, message, hint)
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

/// Issue a raw `HTTP` `GET` to `path` on the origin `base_url` (as
/// [`http_put`]) and return the reply's `(status, body)`.
///
/// # Panics
///
/// Same conditions as [`http_put`].
#[inline]
pub async fn http_get(base_url: &str, path: &str) -> (u16, Vec<u8>) {
    let port_text = base_url
        .strip_prefix("http://localhost:")
        .expect("a http://localhost:<port> origin");
    let port: u16 = port_text.parse().expect("a numeric port");
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the asset server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the request head");
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

/// Appends `value` as a big-endian `u32` by reversing its little-endian
/// bytes (the crate denies `to_be_bytes`).
fn put_be_u32(out: &mut Vec<u8>, value: u32) {
    let mut word = value.to_le_bytes();
    word.reverse();
    out.extend_from_slice(&word);
}

/// Appends `value` as a big-endian `u16`.
fn put_be_u16(out: &mut Vec<u8>, value: u16) {
    let mut word = value.to_le_bytes();
    word.reverse();
    out.extend_from_slice(&word);
}

/// Appends one `PNG` chunk: length, kind, payload, `CRC32`.
fn png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    put_be_u32(
        out,
        u32::try_from(data.len()).expect("a test chunk fits a u32"),
    );
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&kind);
    hasher.update(data);
    put_be_u32(out, hasher.finalize());
}

/// The pixel run of one `PNG` scanline: cyan on the border columns,
/// red inside.
fn png_row(side: u32, last: u32, edge: bool) -> Vec<u8> {
    let mut row = Vec::new();
    for x in 0..side {
        let border = edge || x == 0 || x == last;
        let pixel: [u8; 3] = if border {
            [0_u8, 255_u8, 255_u8]
        } else {
            [220_u8, 40_u8, 40_u8]
        };
        row.extend_from_slice(&pixel);
    }
    row
}

/// A 16x16 8-bit `RGB` `PNG`: cyan border, red middle.
///
/// The test image behind recipe `source` placeholders. The cyan border is
/// what `create_sprite_from_image`'s background flood removes, so the
/// surviving red block keeps every importer on real, non-degenerate
/// pixels.
///
/// # Panics
///
/// Panics if the zlib deflate of the pixel rows fails.
#[must_use]
#[inline]
pub fn png_bytes() -> Vec<u8> {
    let side = 16_u32;
    let last = side.checked_sub(1).expect("16 is above 1");
    let mut header = Vec::new();
    put_be_u32(&mut header, side);
    put_be_u32(&mut header, side);
    header.extend_from_slice(&[8_u8, 2_u8, 0_u8, 0_u8, 0_u8]);
    let mut raw = Vec::new();
    for y in 0..side {
        raw.push(0_u8);
        let edge = y == 0 || y == last;
        raw.extend_from_slice(&png_row(side, last, edge));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(&raw).expect("the test PNG deflates");
    let idat = encoder.finish().expect("the test PNG deflates");
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    png_chunk(&mut out, *b"IHDR", &header);
    png_chunk(&mut out, *b"IDAT", &idat);
    png_chunk(&mut out, *b"IEND", &[]);
    out
}

/// A minimal type-0 `Standard MIDI` file.
///
/// One track with a 500 000 microseconds-per-quarter tempo, a program
/// change, four short quarter notes on `MIDI` channel 1 and the end
/// marker. Small enough to convert to `MUS` and too simple to depend on
/// any `SMF` extra.
///
/// # Panics
///
/// Panics if a length does not fit a `u32` (test data always fits).
#[must_use]
#[inline]
pub fn midi_bytes() -> Vec<u8> {
    let mut track = Vec::new();
    track.extend_from_slice(&[0_u8, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20]);
    track.extend_from_slice(&[0_u8, 0xC0, 0x00]);
    for note in [60_u8, 64_u8, 67_u8, 72_u8] {
        track.extend_from_slice(&[0_u8, 0x90, note, 0x64]);
        track.extend_from_slice(&[60_u8, 0x80, note, 0x40]);
    }
    track.extend_from_slice(&[0_u8, 0xFF, 0x2F, 0x00]);
    let mut out = Vec::new();
    out.extend_from_slice(b"MThd");
    put_be_u32(&mut out, 6_u32);
    put_be_u16(&mut out, 0_u16);
    put_be_u16(&mut out, 1_u16);
    put_be_u16(&mut out, 480_u16);
    out.extend_from_slice(b"MTrk");
    put_be_u32(
        &mut out,
        u32::try_from(track.len()).expect("a test track fits a u32"),
    );
    out.extend_from_slice(&track);
    out
}

/// Appends one little-endian `RIFF` chunk: id, `u32` length, payload.
fn riff_chunk(out: &mut Vec<u8>, id: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&id);
    out.extend_from_slice(
        &u32::try_from(data.len())
            .expect("a test chunk fits a u32")
            .to_le_bytes(),
    );
    out.extend_from_slice(data);
}

/// A minimal 8000 Hz mono 16-bit `PCM` `WAV`.
///
/// 400 samples of a +-6400 triangle, four 100-sample ramps. Long enough
/// to resample, short enough that `import_sound` never nears its length
/// caps.
///
/// # Panics
///
/// Panics if a ramp step or length does not fit its field (test data
/// always fits).
#[must_use]
#[inline]
pub fn wav_bytes() -> Vec<u8> {
    let mut format = Vec::new();
    format.extend_from_slice(&1_u16.to_le_bytes());
    format.extend_from_slice(&1_u16.to_le_bytes());
    format.extend_from_slice(&8000_u32.to_le_bytes());
    format.extend_from_slice(&16000_u32.to_le_bytes());
    format.extend_from_slice(&2_u16.to_le_bytes());
    format.extend_from_slice(&16_u16.to_le_bytes());
    let mut pcm = Vec::new();
    let mut value = 0_i16;
    for ramp in 0..4_usize {
        let step = if ramp & 1 == 0 { 64_i16 } else { -64_i16 };
        for _sample in 0..100_usize {
            value = value.checked_add(step).expect("the test ramp fits");
            pcm.extend_from_slice(&value.to_le_bytes());
        }
    }
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    riff_chunk(&mut body, *b"fmt ", &format);
    riff_chunk(&mut body, *b"data", &pcm);
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    let size = u32::try_from(body.len())
        .expect("a test WAV fits a u32")
        .checked_sub(8_u32)
        .expect("the RIFF header is accounted for");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&body);
    out
}
