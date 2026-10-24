//! The `patches` tool group as seen by a real MCP client: the helpers every
//! patch test drives the server through live here - a connected client with
//! the fixture `IWAD` as its base, the 4x4 test pictures (an opaque `PNG` of
//! palette colours written with the library's own encoder and the fully-opaque
//! `Doom` patch bytes one `encode_patch` produces), the one-call `add_patch`,
//! `replace_patch` and `remove_patch` wrappers, the `PNAMES`, `list_patches`,
//! stored-bytes and working-directory readers, and the shared refusal case
//! table with its `add_patch` half. The tests themselves are split by topic
//! into the `add`, `replace` and `remove` modules, which cover converting and
//! storing patches, overriding them in the working archive and deleting them
//! with their `PNAMES` entry.

use std::path::{Path, PathBuf};

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::{IndexedImage, RgbaImage};
use cyberdemon::gfx::patch::{Patch, encode_patch};
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::iwad;
use crate::support::{call, call_err, connect};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// Pixels on a side of the 4x4 test pictures.
const SIDE: u32 = 4_u32;
/// Pixels in the 4x4 test pictures.
const PIXELS: usize = 16;
/// Entries in the fixture `IWAD`'s `PNAMES`.
const FIXTURE_PNAMES: u64 = 2_u64;
/// Header offsets the passthrough fixture patch carries.
const PATCH_OFFSETS: (i16, i16) = (-2_i16, 11_i16);
/// The file name [`iwad`] writes the fixture `IWAD` under.
const IWAD_FILE: &str = "iwad-doom2.wad";
/// Pixels on a side of the fixture `IWAD`'s `WALLP1`.
const IWAD_SIDE: u64 = 16_u64;

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

/// The palette-colour pixel of raster position `p`, fully opaque.
fn color_pixel(p: usize) -> [u8; 4] {
    let slot = u8::try_from(p).expect("test indices fit u8");
    [slot, 255_u8.wrapping_sub(slot), slot, 255_u8]
}

/// An opaque 4x4 `PNG` of palette colours.
fn palette_png() -> Vec<u8> {
    let pixels: Vec<u8> = (0..PIXELS).flat_map(color_pixel).collect();
    let img = RgbaImage::new(SIDE, SIDE, pixels).expect("test shape is valid");
    encode_png(&img, None).expect("test PNGs encode")
}

/// The 4x4 fully-opaque `Doom` patch bytes the passthrough tests store,
/// header offsets [`PATCH_OFFSETS`], every pixel index `7`.
fn doom_patch() -> Vec<u8> {
    let indices = vec![7_u8; PIXELS];
    let opaque = vec![true; PIXELS];
    let image = IndexedImage::new(SIDE, SIDE, indices, opaque).expect("test shape is valid");
    let patch = Patch {
        left_offset: PATCH_OFFSETS.0,
        top_offset: PATCH_OFFSETS.1,
        image,
    };
    encode_patch(&patch).expect("a 4x4 patch encodes")
}

/// Write `bytes` to `file` in `dir` and return the path.
fn write_file(dir: &Path, file: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, bytes).expect("test files write");
    path
}

/// `add_patch` arguments over a `source` path merged with extras.
fn patch_args(file: &Path, extra: Value) -> Value {
    let mut args = json!({ "source": file.display().to_string() });
    if let Value::Object(additions) = extra {
        args.as_object_mut()
            .expect("a JSON object")
            .extend(additions);
    }
    args
}

/// One successful `add_patch` under `name` of `file`.
async fn add(client: &Client, file: &Path, name: &str) -> Value {
    call(
        client,
        "add_patch",
        patch_args(file, json!({ "name": name })),
    )
    .await
}

/// One successful `replace_patch` of `name` with `file`.
async fn replace(client: &Client, file: &Path, name: &str) -> Value {
    call(
        client,
        "replace_patch",
        patch_args(file, json!({ "name": name })),
    )
    .await
}

/// One successful `remove_patch` of `name` with extra arguments merged
/// in (`remove_from_pnames`, `force`).
async fn remove(client: &Client, name: &str, extra: Value) -> Value {
    let mut args = json!({ "name": name });
    if let Value::Object(additions) = extra {
        args.as_object_mut()
            .expect("a JSON object")
            .extend(additions);
    }
    call(client, "remove_patch", args).await
}

/// The `list_pnames` names, in entry order.
fn pnames_list(report: &Value) -> Vec<String> {
    report["pnames"]
        .as_array()
        .expect("a listing")
        .iter()
        .map(|row| row["name"].as_str().expect("a named row").to_owned())
        .collect()
}

/// The `list_textures` patch count of the texture `name`.
async fn texture_patches(client: &Client, name: &str) -> u64 {
    let listed = call(client, "list_textures", json!({ "pattern": name })).await;
    assert_eq!(listed["total"], json!(1_u64), "{name} is defined");
    listed["textures"][0]["patches"]
        .as_u64()
        .unwrap_or_else(|| panic!("{name} reports its patch count"))
}

/// The fixture `IWAD` file's own bytes on disk, for the untouched-base
/// assertions.
fn iwad_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(IWAD_FILE)).expect("the fixture IWAD reads back")
}

/// The working archive's directory index of the lump `name`.
async fn working_index(client: &Client, name: &str) -> u64 {
    let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
    let wanted = json!(name);
    listing["lumps"]
        .as_array()
        .expect("a listing")
        .iter()
        .find(|row| row["name"] == wanted)
        .and_then(|row| row["index"].as_u64())
        .unwrap_or_else(|| panic!("{name} is not in the working directory"))
}

/// One `list_patches` row matching `name` exactly.
async fn patch_row(client: &Client, name: &str) -> Value {
    let listed = call(client, "list_patches", json!({ "pattern": name })).await;
    assert_eq!(listed["total"], json!(1_u64), "{name} is listed once");
    listed["patches"][0].clone()
}

/// The stored bytes of lump `name`, extracted to disk.
async fn stored_bytes(client: &Client, dir: &Path, name: &str) -> Vec<u8> {
    let out = dir.join(format!("{name}.stored"));
    call(
        client,
        "extract_lump",
        json!({ "name": name, "path": out.display().to_string() }),
    )
    .await;
    std::fs::read(&out).expect("the extracted lump reads back")
}

/// The working archive's directory, as lump names in order.
async fn working_names(client: &Client) -> Vec<String> {
    let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
    listing["lumps"]
        .as_array()
        .expect("a listing")
        .iter()
        .map(|row| row["name"].as_str().expect("a named row").to_owned())
        .collect()
}

/// One refusal case: arguments, the expected `(code, message-substring)`
/// pair and a debug label.
type Refusal = (&'static str, Value, &'static str, &'static str);

/// Assert every `add_patch` call in `cases` refuses as documented.
async fn refuse_all(client: &Client, cases: &[Refusal]) {
    for case in cases {
        let (label, args, expect_code, expect_text) = (case.0, &case.1, case.2, case.3);
        let (code, message, _) = call_err(client, "add_patch", args.clone()).await;
        assert_eq!(code, *expect_code, "{label} code: {message}");
        assert!(
            message.contains(expect_text),
            "{label} message should name {expect_text:?}: {message}"
        );
    }
}

#[cfg(test)]
#[path = "patches/add.rs"]
mod add;
#[cfg(test)]
#[path = "patches/remove.rs"]
mod remove;
#[cfg(test)]
#[path = "patches/replace.rs"]
mod replace;
