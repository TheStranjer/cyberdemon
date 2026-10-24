//! The `graphics` tool group as seen by a real MCP client: the helpers every
//! graphics test drives the server through live here - a connected client with
//! the fixture `IWAD` as its base, the `add_lump` argument builders (a flat
//! `PNG` of any shape, a one-colour `PNG` and an asymmetric `Doom` patch
//! column as their hex, and the sprite region's mirror-pair override), the
//! asset readers that GET a published `PNG` or raw `RGBA` back and decode it,
//! the report-shape assertions and the shared refusal table. The tests
//! themselves are split by topic into the
//! `list`, `flat_patch` and `sprites` modules: the flats, patch and sprite
//! listings over the fixture `IWAD` with their dimensions, `PNAMES` and
//! namespace rules, pattern/prefix and paging validation and the merged view's
//! last-wins resolution; the image tools that publish a flat or a patch as a
//! `PNG` or raw `RGBA` asset with their missing-picture and argument refusals;
//! and the sprite rotations, patch decodings and thing-sprite previews that
//! start from a definition's type number.

use std::path::Path;

use cyberdemon::gfx::codec::{decode_image, encode_png};
use cyberdemon::gfx::image::{IndexedImage, RgbaImage};
use cyberdemon::gfx::patch::{Patch, encode_patch};
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::iwad;
use crate::support::{call, call_err, connect, http_get};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// The `{"path": ...}` argument object for `file`.
fn path_arg(file: &Path) -> Value {
    json!({ "path": file.display().to_string() })
}

/// Load the fixture `IWAD` (flats, patches, sprites, textures) as the
/// base.
async fn iwad_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The space-separated uppercase hex of `data`, for `add_lump` inputs.
fn hex_of(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A flat `PNG` of `width` by `height`, optionally with `grAb`
/// offsets, for `add_lump` inputs.
fn png(width: u32, height: u32, offsets: Option<(i32, i32)>) -> String {
    let pixels = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit")
        .checked_mul(4)
        .expect("test pixel bytes fit");
    let img = RgbaImage::new(width, height, vec![0x10_u8; pixels]).expect("test shapes fit");
    hex_of(&encode_png(&img, offsets).expect("test PNGs encode"))
}

/// One opaque `RGBA` pixel of `color`.
fn pixel(color: [u8; 3]) -> [u8; 4] {
    let [red, green, blue] = color;
    [red, green, blue, 255_u8]
}

/// A fully opaque `PNG` of one colour with optional `grAb` offsets, as
/// the hex `add_lump` takes: a port's `PNG` patch or flat.
fn image_png(width: u32, height: u32, color: [u8; 3], offsets: Option<(i32, i32)>) -> String {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = vec![pixel(color); count].concat();
    let img = RgbaImage::new(width, height, pixels).expect("test shapes fit");
    hex_of(&encode_png(&img, offsets).expect("test PNGs encode"))
}

/// A `Doom` patch `width` by 2 whose only opaque pixels are column
/// `column`, as the hex `add_lump` takes: an asymmetric sprite image,
/// so a horizontal flip is visible.
fn stripe_hex(width: u32, column: u32, offsets: (i16, i16)) -> String {
    hex_of(&stripe_bytes(width, column, offsets))
}

/// The asset object a report carries under `field`.
fn asset_of<'report>(report: &'report Value, field: &str) -> &'report Value {
    &report[field]
}

/// The `url` of the asset a report carries under `field`.
fn asset_url(report: &Value, field: &str) -> String {
    asset_of(report, field)["url"]
        .as_str()
        .expect("the report carries an asset url")
        .to_owned()
}

/// GET a published asset and require the bytes it serves.
async fn fetch_asset(url: &str) -> Vec<u8> {
    let (base, rest) = url.split_once("/assets/").expect("an asset url");
    let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
    assert_eq!(status, 200, "the asset serves: {url}");
    body
}

/// GET a published asset and decode it as an image.
async fn fetch_image(url: &str) -> RgbaImage {
    let body = fetch_asset(url).await;
    decode_image(&body).expect("the asset is a readable PNG")
}

/// Assert the published shape of a picture report.
fn check_dims(report: &Value, width: u64, height: u64) {
    assert_eq!(
        (report["width"].as_u64(), report["height"].as_u64()),
        (Some(width), Some(height)),
        "the published dimensions"
    );
}

/// Assert the drawing offsets a picture report answers with.
fn check_offsets(report: &Value, left: i64, top: i64) {
    assert_eq!(
        (
            report["left_offset"].as_i64(),
            report["top_offset"].as_i64()
        ),
        (Some(left), Some(top)),
        "the drawing offsets in published pixels"
    );
}

/// Add one working-archive lump under `name` from `hex`.
async fn add(client: &Client, name: &str, hex: &str) {
    call(client, "add_lump", json!({ "name": name, "hex": hex })).await;
}

/// Add one working-archive lump inside a namespace region.
async fn add_in(client: &Client, name: &str, hex: &str, namespace: &str) {
    call(
        client,
        "add_lump",
        json!({ "name": name, "hex": hex, "namespace": namespace }),
    )
    .await;
}

/// The bytes of a `Doom` patch `width` by 2 whose only opaque pixels
/// are column `column`, for a file fixture.
fn stripe_bytes(width: u32, column: u32, offsets: (i16, i16)) -> Vec<u8> {
    let (left, top) = offsets;
    let columns = usize::try_from(width).expect("test width");
    let total = columns.checked_mul(2).expect("test pixels fit");
    let mut indices = vec![0_u8; total];
    let mut opaque = vec![false; total];
    for row in 0_usize..2 {
        let slot = row
            .checked_mul(columns)
            .and_then(|start| start.checked_add(usize::try_from(column).expect("test column")))
            .expect("test index fits");
        indices[slot] = 5_u8;
        opaque[slot] = true;
    }
    let image = IndexedImage::new(width, 2, indices, opaque).expect("test shapes fit");
    encode_patch(&Patch {
        left_offset: left,
        top_offset: top,
        image,
    })
    .expect("test patches encode")
}

/// Add `TROOA2A8` to the working archive's sprite region as an
/// asymmetric 4x2 patch (opaque column 0, offsets `(1, 2)`),
/// overriding the fixture's plain copy so a mirrored rotation is
/// visible in the published pixels.
async fn add_stripe_mirror(client: &Client) {
    add_in(
        client,
        "TROOA2A8",
        &hex_of(&stripe_bytes(4, 0, (1_i16, 2_i16))),
        "sprites",
    )
    .await;
}

/// Assert the image tools refuse `args` with the error `code` and a
/// `hint` in the message.
async fn refuse(client: &Client, tool: &str, args: Value, expect: (&str, &str)) {
    let (code, hint) = expect;
    let (got, message, _) = call_err(client, tool, args).await;
    assert_eq!(got, code, "{tool} code: {message}");
    assert!(
        message.contains(hint),
        "{tool} should mention {hint:?}: {message}"
    );
}

/// One refusal a picture tool answers with: `(tool, arguments, code,
/// message hint)`.
type Refusal = (&'static str, Value, &'static str, &'static str);

/// Assert each `(tool, args, code, hint)` refusal in `cases`.
async fn refuse_all(client: &Client, cases: &[Refusal]) {
    for case in cases {
        refuse(client, case.0, case.1.clone(), (case.2, case.3)).await;
    }
}

#[cfg(test)]
#[path = "graphics/flat_patch.rs"]
mod flat_patch;
#[cfg(test)]
#[path = "graphics/list.rs"]
mod list;
#[cfg(test)]
#[path = "graphics/sprites.rs"]
mod sprites;
