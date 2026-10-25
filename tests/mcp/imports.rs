//! The `imports` tool group as seen by a real MCP client. The helpers every
//! topic drives the server through live here: a connected client with the
//! fixture `IWAD` as its base, the palette-colour test pictures - the 8x8
//! `PNG` rasters with an optional alpha hole and `grAb` chunk and the
//! palette-cycling flat `PNG`s, all written with the library's own `PNG`
//! encoder -, the `source` argument builder, the asset fetcher, the stored
//! `RGBA` readers for patches, flats and sprites, the expected-pixel and
//! report cross-checkers, the `steps`/`warnings` readers with their pixel
//! readers, and the `Refusal` table type with its `refuse_all` runner. The
//! tests themselves are split by topic into the `patch`, `patch_sources`,
//! `flat`, `raw`, `sprite`, `process`, `create_texture`, `create_flat` and
//! `create_sprite` modules: the shared image pipeline through
//! `import_image_as_patch` and its alias and its source and naming rules,
//! the flat importer `import_image_as_flat` with its alias, the raw importer
//! `import_image_as_raw`, the sprite importer `import_image_as_sprite` with
//! its alias, the preview pipeline `process_image`, and the one-shot
//! `create_texture_from_image`, `create_flat_from_image` and
//! `create_sprite_from_image`.

use std::path::{Path, PathBuf};

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::grab::insert_grab;
use cyberdemon::gfx::image::RgbaImage;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::iwad;
use crate::support::{call, call_err, connect, http_get};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// Pixels on a side of the 8x8 test rasters.
const SIDE: u32 = 8_u32;

/// Pixels in the 8x8 test rasters.
const PIXELS: usize = 64;

/// Palette entries the flat test rasters cycle through.
const CYCLE: usize = 256_usize;

/// The pixel index the alpha hole is placed at.
const HOLE: usize = 9;

/// Entries in the fixture `IWAD`'s `PNAMES`.
const FIXTURE_PNAMES: u64 = 2_u64;

/// Flats in the fixture `IWAD` (`FLOOR1` and `PNGFLAT`).
const FIXTURE_FLATS: u64 = 2_u64;

/// Load the fixture `IWAD` (palettes, patches, `PNAMES`) as the base.
async fn iwad_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    client
}

/// Fixture palette colour `i`: `(i, 255 - i, i)`.
fn palette_color(index: u8) -> [u8; 3] {
    [index, 255_u8.wrapping_sub(index), index]
}

/// An opaque pixel of palette colour `p`.
fn color_pixel(p: usize) -> [u8; 4] {
    let slot = u8::try_from(p).expect("test indices fit u8");
    let [red, green, blue] = palette_color(slot);
    [red, green, blue, 255_u8]
}

/// The palette-colour pixel of raster position `p`, fully transparent
/// when `hole` names it.
fn raster_pixel(p: usize, hole: Option<usize>) -> [u8; 4] {
    let mut pixel = color_pixel(p);
    if hole.is_some_and(|dropped| dropped == p) {
        pixel[3] = 0_u8;
    }
    pixel
}

/// A palette-colour PNG of `width` by `height`, pixel `p` coloured
/// index `p`, optionally alpha-transparent at `alpha_of` and carrying
/// `grAb` offsets.
fn palette_png(
    width: u32,
    height: u32,
    alpha_of: Option<usize>,
    grab: Option<(i32, i32)>,
) -> Vec<u8> {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count).flat_map(|p| raster_pixel(p, alpha_of)).collect();
    let img = RgbaImage::new(width, height, pixels).expect("test shapes are valid");
    let png = encode_png(&img, None).expect("test PNGs encode");
    match grab {
        Some((x, y)) => insert_grab(&png, x, y).expect("test grAb inserts"),
        None => png,
    }
}

/// The 8x8 alpha-hole PNG: palette colours `0..=63`, pixel [`HOLE`]
/// fully transparent.
fn hole_png() -> Vec<u8> {
    palette_png(SIDE, SIDE, Some(HOLE), None)
}

/// Write `bytes` to `file` in `dir` and return the path.
fn write_file(dir: &Path, file: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, bytes).expect("test files write");
    path
}

/// Arguments carrying a `source` path merged with an object of extras.
fn source_args(file: &Path, extra: Value) -> Value {
    let mut args = json!({ "source": file.display().to_string() });
    if let Value::Object(additions) = extra {
        args.as_object_mut()
            .expect("a JSON object")
            .extend(additions);
    }
    args
}

/// Fetch a published asset's bytes.
async fn fetch_asset(url: &str) -> Vec<u8> {
    let (base, rest) = url.split_once("/assets/").expect("an asset url");
    let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
    assert_eq!(status, 200, "the asset serves: {url}");
    body
}

/// The `RGBA` bytes of the stored patch `name`, via `get_patch`.
async fn patch_rgba(client: &Client, name: &str) -> Vec<u8> {
    let report = call(
        client,
        "get_patch",
        json!({ "name": name, "format": "rgba" }),
    )
    .await;
    let url = report["asset"]["url"]
        .as_str()
        .expect("get_patch publishes an asset");
    fetch_asset(url).await
}

/// The expected `RGBA` of [`hole_png`] after a patch round-trip: every
/// opaque palette colour, and [`HOLE`] as an alpha-`0` pixel at index
/// `0` (a patch stores no index under its holes).
fn expected_rgba() -> Vec<u8> {
    (0..PIXELS).flat_map(expected_pixel).collect()
}

/// The round-tripped pixel `p`: its colour when opaque, index `0` and
/// alpha `0` at the hole.
fn expected_pixel(p: usize) -> [u8; 4] {
    if p == HOLE {
        [0_u8, 255, 0, 0]
    } else {
        color_pixel(p)
    }
}

/// Assert two patch-import reports agree on every shared pipeline
/// field.
fn assert_same_pipeline(main: &Value, alias: &Value) {
    for field in [
        "width",
        "height",
        "left_offset",
        "top_offset",
        "transparent_pixels",
        "colors_used",
        "max_color_error",
    ] {
        assert_eq!(
            alias[field], main[field],
            "the alias reports {field} identically"
        );
    }
}

/// A patch-import refusal: arguments plus the expected `(code,
/// message-substring)` pair and a debug label.
type Refusal = (&'static str, Value, &'static str, &'static str);

/// Assert every `tool` call in `cases` refuses as documented.
async fn refuse_all(client: &Client, tool: &str, cases: &[Refusal]) {
    for case in cases {
        let (label, expect_code, expect_text) = (case.0, case.2, case.3);
        let (code, message, _) = call_err(client, tool, case.1.clone()).await;
        assert_eq!(code, expect_code, "{label} code: {message}");
        assert!(
            message.contains(expect_text),
            "{label} message should name {expect_text:?}: {message}"
        );
    }
}

/// The palette colour index for flat raster position `p`.
fn cycled(p: usize) -> usize {
    p.checked_rem(CYCLE).expect("non-zero cycle")
}

/// The `RGBA` of flat raster position `p`: its cycled palette colour,
/// fully transparent when `transparent` picks it.
fn flat_pixel(p: usize, transparent: &impl Fn(usize) -> bool) -> [u8; 4] {
    let mut pixel = color_pixel(cycled(p));
    if transparent(p) {
        pixel[3] = 0_u8;
    }
    pixel
}

/// A palette PNG of `width` by `height` cycling palette colours, with
/// alpha `0` where `transparent` picks the pixel.
fn flat_png(width: u32, height: u32, transparent: impl Fn(usize) -> bool) -> Vec<u8> {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count)
        .flat_map(|p| flat_pixel(p, &transparent))
        .collect();
    let img = RgbaImage::new(width, height, pixels).expect("test shapes are valid");
    encode_png(&img, None).expect("test PNGs encode")
}

/// The `RGBA` bytes a stored [`flat_png`] answers back through
/// `get_flat`: every cycled palette colour, fully opaque.
fn flat_expected_rgba(count: usize) -> Vec<u8> {
    (0..count).flat_map(|p| color_pixel(cycled(p))).collect()
}

/// The `RGBA` bytes of the stored flat `name`, via `get_flat`.
async fn flat_rgba(client: &Client, name: &str) -> Vec<u8> {
    let report = call(
        client,
        "get_flat",
        json!({ "name": name, "format": "rgba" }),
    )
    .await;
    let url = report["asset"]["url"]
        .as_str()
        .expect("get_flat publishes an asset");
    fetch_asset(url).await
}

/// The `RGBA` bytes of the sprite lump `name` as stored, via
/// `get_sprite`.
async fn sprite_rgba(client: &Client, name: &str) -> Vec<u8> {
    let report = call(
        client,
        "get_sprite",
        json!({ "name": name, "format": "rgba" }),
    )
    .await;
    let url = report["asset"]["url"]
        .as_str()
        .expect("get_sprite publishes an asset");
    fetch_asset(url).await
}

/// The `steps` of a report, as op names in order.
fn step_ops(report: &Value) -> Vec<&str> {
    report["steps"]
        .as_array()
        .expect("a step list")
        .iter()
        .map(|step| step["op"].as_str().expect("a named step"))
        .collect()
}

/// Whether a `warnings` entry is text mentioning every one of `parts`.
fn mentions(warning: &Value, parts: &[&str]) -> bool {
    let text = warning.as_str().unwrap_or_default();
    parts.iter().all(|part| text.contains(part))
}

/// The `RGBA` pixel at row-major `index` of a fetched picture.
fn rgba_pixel(rgba: &[u8], index: usize) -> [u8; 4] {
    let start = index.checked_mul(4).expect("byte offset");
    let byte = |shift: usize| {
        *rgba
            .get(start.checked_add(shift).expect("in range"))
            .expect("pixel byte")
    };
    [byte(0), byte(1), byte(2), byte(3)]
}

/// The `RGB` triple at row-major `index` of a fetched picture.
fn rgb_at(rgba: &[u8], index: usize) -> [u8; 3] {
    let pixel = rgba_pixel(rgba, index);
    [pixel[0], pixel[1], pixel[2]]
}

#[cfg(test)]
#[path = "imports/create_flat.rs"]
mod create_flat;
#[cfg(test)]
#[path = "imports/create_sprite.rs"]
mod create_sprite;
#[cfg(test)]
#[path = "imports/create_texture.rs"]
mod create_texture;
#[cfg(test)]
#[path = "imports/flat.rs"]
mod flat;
#[cfg(test)]
#[path = "imports/patch.rs"]
mod patch;
#[cfg(test)]
#[path = "imports/patch_sources.rs"]
mod patch_sources;
#[cfg(test)]
#[path = "imports/process.rs"]
mod process;
#[cfg(test)]
#[path = "imports/raw.rs"]
mod raw;
#[cfg(test)]
#[path = "imports/sprite.rs"]
mod sprite;
