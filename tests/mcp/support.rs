//! Shared harness: real rmcp clients over in-memory pipes.
//!
//! Plus a probe server that speaks the structured-result contract
//! (`ok`/`fail`) so those helpers are exercised end to end, not just
//! compiled, small hand-written byte builders for the media files
//! (`PNG`, `MIDI`, `WAV`) the guide recipes import, and the picture
//! helpers (blank sheets, rectangle fills, the ruled photo sheet, `PNG`
//! files and asset fetches) the image-chain tests draw their fixtures
//! with.

use core::time::Duration;
use std::io::Write as _;
use std::path::Path;

use cyberdemon::Cyberdemon;
use cyberdemon::error::CyberError;
use cyberdemon::gfx::codec::{decode_image, encode_png};
use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::tools::{fail, ok};
use cyberdemon::wad::bytes::Writer;
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

/// Appends one `PNG` chunk: length, kind, payload, `CRC32`.
fn png_chunk(out: &mut Writer, kind: [u8; 4], data: &[u8]) {
    out.put_u32_be(u32::try_from(data.len()).expect("a test chunk fits a u32"));
    out.put_bytes(&kind);
    out.put_bytes(data);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&kind);
    hasher.update(data);
    out.put_u32_be(hasher.finalize());
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
    let mut head = Writer::new();
    head.put_u32_be(side);
    head.put_u32_be(side);
    head.put_bytes(&[8_u8, 2_u8, 0_u8, 0_u8, 0_u8]);
    let header = head.into_inner();
    let mut raw = Vec::new();
    for y in 0..side {
        raw.push(0_u8);
        let edge = y == 0 || y == last;
        raw.extend_from_slice(&png_row(side, last, edge));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(&raw).expect("the test PNG deflates");
    let idat = encoder.finish().expect("the test PNG deflates");
    let mut out = Writer::new();
    out.put_bytes(b"\x89PNG\r\n\x1a\n");
    png_chunk(&mut out, *b"IHDR", &header);
    png_chunk(&mut out, *b"IDAT", &idat);
    png_chunk(&mut out, *b"IEND", &[]);
    out.into_inner()
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
    let mut out = Writer::new();
    out.put_bytes(b"MThd");
    out.put_u32_be(6_u32);
    out.put_u16_be(0_u16);
    out.put_u16_be(1_u16);
    out.put_u16_be(480_u16);
    out.put_bytes(b"MTrk");
    out.put_u32_be(u32::try_from(track.len()).expect("a test track fits a u32"));
    out.put_bytes(&track);
    out.into_inner()
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

/// A blank `width` x `height` sheet of opaque white paper to draw
/// picture fixtures on.
///
/// # Panics
///
/// Panics if the shape does not fit (test sizes always fit).
#[must_use]
#[inline]
pub fn paper_sheet(width: u32, height: u32) -> RgbaImage {
    let count = usize::try_from(width)
        .expect("test width fits")
        .checked_mul(usize::try_from(height).expect("test height fits"))
        .expect("test pixels fit")
        .checked_mul(4)
        .expect("test bytes fit");
    RgbaImage::new(width, height, vec![255_u8; count]).expect("test shapes are valid")
}

/// Paints the half-open rectangle `(x0, y0)..(x1, y1)` of `sheet` with
/// `colour`.
///
/// # Panics
///
/// Panics if a pixel of the rectangle falls outside `sheet` (test
/// rectangles always fit).
#[inline]
pub fn fill_rect(sheet: &mut RgbaImage, rect: (u32, u32, u32, u32), colour: [u8; 4]) {
    let (x0, y0, x1, y1) = rect;
    for row in y0..y1 {
        for column in x0..x1 {
            assert!(sheet.set(column, row, colour), "test rect in bounds");
        }
    }
}

/// The room drawn into [`ruled_photo_sheet`]: the outer corners of the
/// wall box, `(x0, y0, x1, y1)`, inset from the frame of a 600x400 sheet
/// by at least 90 px on every side.
pub const PHOTO_ROOM: (u32, u32, u32, u32) = (120_u32, 90_u32, 420_u32, 300_u32);

/// The thickness of every wall bar of [`PHOTO_ROOM`], in pixels: three,
/// so the walls survive the sketch tools' 3x3 morphological opening.
pub const PHOTO_WALL_PX: u32 = 3_u32;

/// The near-black ink of the room's walls and tick: luminance 18 under
/// `(299 r + 587 g + 114 b) / 1000`.
pub const PHOTO_INK: [u8; 4] = [18_u8, 18, 20, 255];

/// The pale blue of the ruling: luminance 196, 59 levels under the
/// paper, far above the ~12 levels a 1 px line needs to read at all.
pub const PHOTO_RULING: [u8; 4] = [180_u8, 200, 224, 255];

/// The vertical period of the ruling, in pixels.
pub const PHOTO_RULING_PERIOD: u32 = 8_u32;

/// The first ruled row: every `y` with `y % 8 == 3` carries one pixel of
/// [`PHOTO_RULING`] across the full width.
pub const PHOTO_RULING_PHASE: u32 = 3_u32;

/// The number of top rows the multiply-style shadow ramp covers.
pub const PHOTO_SHADOW_ROWS: u32 = 160_u32;

/// The short tick inside the room, just off the top-left corner: 12x3,
/// so both its edges sit within 3 * stroke + 8 = 17 px and the sketch
/// tools read it as a mark, never as a wall.
pub const PHOTO_TICK: (u32, u32, u32, u32) = (160_u32, 150_u32, 172_u32, 153_u32);

/// One channel of a pixel multiplied by `factor / 255` in integer math.
fn scale_channel(channel: u8, factor: u32) -> u8 {
    u8::try_from(
        u32::from(channel)
            .checked_mul(factor)
            .expect("a test channel product fits")
            .checked_div(255_u32)
            .expect("the scale divisor is not zero"),
    )
    .expect("a scaled channel fits a u8")
}

/// `pixel` with every colour channel multiplied by `factor / 255`, the
/// alpha byte kept.
fn scale_pixel(pixel: [u8; 4], factor: u32) -> [u8; 4] {
    [
        scale_channel(pixel[0], factor),
        scale_channel(pixel[1], factor),
        scale_channel(pixel[2], factor),
        pixel[3],
    ]
}

/// Multiplies every pixel of `rect` by `factor / 255`: the
/// darken-what-is-already-there step the shadow and the vignette use.
///
/// # Panics
///
/// Panics if a pixel of the rectangle falls outside `sheet` (test
/// rectangles always fit).
fn multiply_rect(sheet: &mut RgbaImage, rect: (u32, u32, u32, u32), factor: u32) {
    let (x0, y0, x1, y1) = rect;
    for row in y0..y1 {
        for column in x0..x1 {
            let pixel = sheet
                .get(column, row)
                .expect("test pixels are inside the sheet");
            assert!(
                sheet.set(column, row, scale_pixel(pixel, factor)),
                "test rect in bounds"
            );
        }
    }
}

/// A ruled photo sheet: white paper under a pale blue 1 px ruling, a
/// 3 px near-black room box with an interior tick, a multiply-style
/// vertical shadow over the top rows and a two-step corner vignette.
///
/// The fixture stands in for an imported photograph whose structure the
/// measurement tools can still read. Every number is that contract,
/// tuned against `adaptive_threshold` at `radius: 6, offset: 10` (a
/// pixel is ink when its luminance sits at least 10 below the mean of
/// its clipped 13x13 window):
///
/// - a hard luminance step above ~20 levels prints a 3-4 px black band
///   on its darker side that a 3x3 opening keeps; so the vignette is
///   two nested blocks, each a further multiply by 246/255 (a max single
///   step of 9 levels), and the shadow ramp falls exactly 1 level per
///   row. At the clipped top border a ramp of slope `s` puts the centre
///   about `4 * s` below its window mean, and that gap must stay under
///   10 or the sheet grows a permanent black edge line;
/// - a 1 px line needs ~12+ levels of contrast to be seen at all: the
///   ruling's 59 ([`PHOTO_RULING`] at luminance 196 against paper 255)
///   is ample;
/// - walls must be at least [`PHOTO_WALL_PX`] = 3 px to survive the 3x3
///   opening, and [`PHOTO_TICK`] keeps both edges within 3 * stroke +
///   8 = 17 px so it reads as a mark and the room stays one clean
///   rectangle.
///
/// Draws in order: paper, the [`PHOTO_RULING_PERIOD`]-period ruling at
/// phase [`PHOTO_RULING_PHASE`], the [`PHOTO_ROOM`] bars plus its tick,
/// the shadow over rows `0..PHOTO_SHADOW_ROWS` with
/// `factor = 255 - (PHOTO_SHADOW_ROWS - y)`, then the nested corner
/// vignette blocks `(0, 0, 84, 44)` / `(0, 0, 56, 28)` and their
/// right-hand mirrors.
///
/// # Panics
///
/// Panics unless `width >= 600 && height >= 400`, or if a drawn
/// coordinate or factor does not fit (test sizes always fit).
#[must_use]
#[inline]
pub fn ruled_photo_sheet(width: u32, height: u32) -> RgbaImage {
    assert!(
        width >= 600_u32 && height >= 400_u32,
        "a ruled photo sheet is at least 600x400 px"
    );
    let mut sheet = paper_sheet(width, height);

    // The ruling: one row every PHOTO_RULING_PERIOD at the phase.
    let mut row = PHOTO_RULING_PHASE;
    while row < height {
        let next = row.checked_add(1_u32).expect("a test row fits");
        fill_rect(&mut sheet, (0_u32, row, width, next), PHOTO_RULING);
        row = row
            .checked_add(PHOTO_RULING_PERIOD)
            .expect("a test row fits");
    }

    // The room: four bars along the edges of PHOTO_ROOM, plus the tick.
    let (x0, y0, x1, y1) = PHOTO_ROOM;
    let left = x0.checked_add(PHOTO_WALL_PX).expect("a test wall fits");
    let right = x1.checked_sub(PHOTO_WALL_PX).expect("a test wall fits");
    let top = y0.checked_add(PHOTO_WALL_PX).expect("a test wall fits");
    let bottom = y1.checked_sub(PHOTO_WALL_PX).expect("a test wall fits");
    fill_rect(&mut sheet, (x0, y0, x1, top), PHOTO_INK);
    fill_rect(&mut sheet, (x0, bottom, x1, y1), PHOTO_INK);
    fill_rect(&mut sheet, (x0, y0, left, y1), PHOTO_INK);
    fill_rect(&mut sheet, (right, y0, x1, y1), PHOTO_INK);
    fill_rect(&mut sheet, PHOTO_TICK, PHOTO_INK);

    // The shadow: rows 0..PHOTO_SHADOW_ROWS multiplied by
    // 255 - (PHOTO_SHADOW_ROWS - row), a ramp of one level per row.
    for ramp in 0_u32..PHOTO_SHADOW_ROWS {
        let decay = PHOTO_SHADOW_ROWS
            .checked_sub(ramp)
            .expect("a ramp row is inside the shadow");
        let factor = 255_u32.checked_sub(decay).expect("the ramp fits");
        let next = ramp.checked_add(1_u32).expect("a test row fits");
        multiply_rect(&mut sheet, (0_u32, ramp, width, next), factor);
    }

    // The vignette: nested blocks at both top corners, each a further
    // multiply by 246/255, so no single step is above 9 levels.
    let corner = 246_u32;
    multiply_rect(&mut sheet, (0_u32, 0_u32, 84_u32, 44_u32), corner);
    multiply_rect(&mut sheet, (0_u32, 0_u32, 56_u32, 28_u32), corner);
    let outer_x = width.checked_sub(84_u32).expect("the sheet is 600+ wide");
    let inner_x = width.checked_sub(56_u32).expect("the sheet is 600+ wide");
    multiply_rect(&mut sheet, (outer_x, 0_u32, width, 44_u32), corner);
    multiply_rect(&mut sheet, (inner_x, 0_u32, width, 28_u32), corner);
    sheet
}

/// Encodes `sheet` as a `PNG` file named `name` inside `dir` and returns
/// the path text for a tool's `source` argument.
///
/// # Panics
///
/// Panics if the picture does not encode or the file cannot be written.
#[must_use]
#[inline]
pub fn write_png(dir: &Path, name: &str, sheet: &RgbaImage) -> String {
    let png = encode_png(sheet, None).expect("test PNGs encode");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write the test PNG");
    path.display().to_string()
}

/// Fetches the published asset at a report's `url` and decodes it as an
/// image: the `HTTP` half of looking at what a tool published.
///
/// # Panics
///
/// Panics if `url` carries no `/assets/` part, the asset does not serve
/// with status 200, or the bytes are no readable `PNG`.
#[inline]
pub async fn asset_png(url: &str) -> RgbaImage {
    let (base, rest) = url.split_once("/assets/").expect("an asset url");
    let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
    assert_eq!(status, 200, "the asset serves: {url}");
    decode_image(&body).expect("the asset is a readable PNG")
}
