//! The flat and patch readers: `get_flat` publishing a raw flat as a `PNG`
//! asset, quantizing a `PNG` flat and re-scaling or re-painting it from another
//! palette entry, publishing raw `RGBA` bytes at the scaled size, and
//! `get_patch` reading a `Doom` patch header with its `grAb` offsets, a `PNG`
//! lump and the working archive's override. The picture tools' shared refusal
//! tables for missing or undecodable lumps and for malformed arguments are
//! here too.

use cyberdemon::gfx::codec::decode_image;
use cyberdemon::gfx::grab::read_grab;
use serde_json::json;

use crate::graphics::{
    Refusal, add, add_in, asset_of, asset_url_of, check_dims, check_offsets, fetch_asset,
    fetch_image, hex_of, image_png, iwad_client, refuse, refuse_all, stripe_hex,
};
use crate::support::{call, connect};

#[tokio::test]
async fn get_flat_publishes_a_raw_flat_as_a_png_asset() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "get_flat", json!({ "name": "floor1" })).await;
    assert_eq!(report["name"], json!("FLOOR1"), "the name is uppercased");
    check_dims(&report, 64_u64, 64_u64);
    assert!(
        report["left_offset"].is_null() && report["top_offset"].is_null(),
        "a flat stores no drawing offsets"
    );
    let asset = asset_of(&report, "asset");
    assert_eq!(asset["filename"], json!("FLOOR1.png"), "the published name");
    assert_eq!(asset["mime"], json!("image/png"), "the media type");
    assert!(
        asset["size"].as_u64().expect("a size") > 0,
        "bytes were stored"
    );
    let url = asset_url_of(&report, "asset");
    let image = fetch_image(&url).await;
    assert_eq!(
        image.get(0, 0),
        Some([1_u8, 254, 1, 255]),
        "every pixel is palette colour 1"
    );
    assert_eq!(
        image.get(63, 63),
        Some([1_u8, 254, 1, 255]),
        "the far corner too"
    );
    assert_eq!(
        read_grab(&fetch_asset(&url).await).expect("a chunked PNG"),
        None,
        "a flat is published without a grAb chunk"
    );
}

#[tokio::test]
async fn get_flat_quantizes_a_png_flat_to_palette_zero() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "get_flat", json!({ "name": "PNGFLAT" })).await;
    check_dims(&report, 64_u64, 64_u64);
    assert_eq!(report["asset"]["mime"], json!("image/png"), "a PNG asset");
    let image = fetch_image(&asset_url_of(&report, "asset")).await;
    assert_eq!(
        image.get(32, 32),
        Some([64_u8, 191, 64, 255]),
        "the PNG colour is quantized to index 64 and recoloured from it"
    );
}

/// `width * height * 4`, the `RGBA8` byte count a picture's asset
/// carries.
const fn rgba_len(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(4)
}

#[tokio::test]
async fn get_flat_scales_the_picture_and_repaints_it_from_another_palette() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let plain = call(&client, "get_flat", json!({ "name": "FLOOR1" })).await;
    let doubled = call(
        &client,
        "get_flat",
        json!({ "name": "FLOOR1", "scale": 2_u64 }),
    )
    .await;
    check_dims(&doubled, 128_u64, 128_u64);
    assert_eq!(
        doubled["asset"]["filename"],
        json!("FLOOR1.png"),
        "the same name, another asset"
    );
    let image = fetch_image(&asset_url_of(&doubled, "asset")).await;
    assert_eq!(
        image.get(1, 1),
        Some([1_u8, 254, 1, 255]),
        "nearest-neighbour upscaling keeps the colour"
    );
    let coloured = call(
        &client,
        "get_flat",
        json!({ "name": "FLOOR1", "palette": 13_u64 }),
    )
    .await;
    assert_eq!(
        coloured["name"],
        json!("FLOOR1"),
        "the palette entry is not a new image"
    );
    assert_eq!(
        fetch_asset(&asset_url_of(&plain, "asset")).await,
        fetch_asset(&asset_url_of(&coloured, "asset")).await,
        "the fixture palettes are copies, so palette 13 paints the same picture"
    );
}

#[tokio::test]
async fn get_flat_publishes_raw_rgba_bytes_at_the_scaled_size() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let rgba = call(
        &client,
        "get_flat",
        json!({ "name": "FLOOR1", "format": "rgba", "scale": 2_u64 }),
    )
    .await;
    assert_eq!(
        rgba["asset"]["filename"],
        json!("FLOOR1.rgba"),
        "the raw asset name"
    );
    assert_eq!(
        rgba["asset"]["mime"],
        json!("application/octet-stream"),
        "the raw asset media type"
    );
    check_dims(&rgba, 128_u64, 128_u64);
    let bytes = fetch_asset(&asset_url_of(&rgba, "asset")).await;
    assert_eq!(bytes.len(), rgba_len(128, 128), "row-major RGBA8 bytes");
    assert_eq!(
        bytes.first().copied(),
        Some(1_u8),
        "the first channel is palette colour 1"
    );
}

/// The refusals for pictures that are missing or that no decoder reads.
fn missing_picture_cases() -> Vec<Refusal> {
    vec![
        (
            "get_flat",
            json!({ "name": "NOPE" }),
            "not_found",
            "list_flats",
        ),
        (
            "get_patch",
            json!({ "name": "NOPE" }),
            "not_found",
            "list_patches",
        ),
        (
            "get_sprite",
            json!({ "sprite": "NOPE" }),
            "not_found",
            "list_sprites",
        ),
        (
            "get_sprite",
            json!({ "name": "NOPEA1" }),
            "not_found",
            "list_sprites",
        ),
        (
            "get_patch",
            json!({ "name": "ENDOOM" }),
            "format_error",
            "patch",
        ),
        (
            "get_flat",
            json!({ "name": "ODDFLAT" }),
            "format_error",
            "flat",
        ),
        (
            "get_sprite",
            json!({ "name": "BADNAME" }),
            "format_error",
            "end of data",
        ),
        (
            "decode_patch",
            json!({ "name": "PNGFLAT" }),
            "format_error",
            "patch",
        ),
        (
            "get_flat",
            json!({ "name": "FLOOR1", "palette": 20_u64 }),
            "not_found",
            "PLAYPAL",
        ),
    ]
}

#[tokio::test]
async fn the_image_tools_report_missing_and_undecodable_pictures() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    add_in(&client, "ODDFLAT", &hex_of(&[7_u8; 100]), "flats").await;
    refuse_all(&client, &missing_picture_cases()).await;
    let empty = connect().await;
    refuse(
        &empty,
        "get_flat",
        json!({ "name": "FLOOR1" }),
        ("not_found", "list_flats"),
    )
    .await;
}

/// The refusals for malformed arguments of the picture tools.
fn bad_argument_cases() -> Vec<Refusal> {
    vec![
        (
            "get_sprite",
            json!({ "name": "TROOA1", "sprite": "TROO" }),
            "invalid_argument",
            "not both",
        ),
        (
            "get_sprite",
            json!({ "format": "png" }),
            "invalid_argument",
            "needs a sprite",
        ),
        (
            "get_flat",
            json!({ "name": "FLOOR1", "format": "jpg" }),
            "invalid_argument",
            "rgba",
        ),
        (
            "get_flat",
            json!({ "name": "TOOLONGNAME" }),
            "invalid_argument",
            "lump name",
        ),
        (
            "get_patch",
            json!({ "name": "WALLP1", "scale": 0_u64 }),
            "invalid_argument",
            "scale",
        ),
        (
            "get_patch",
            json!({ "name": "WALLP1", "scale": 9_u64 }),
            "invalid_argument",
            "scale",
        ),
        (
            "get_sprite",
            json!({ "sprite": "TROO", "frame": "1" }),
            "invalid_argument",
            "frame",
        ),
        (
            "get_sprite",
            json!({ "sprite": "TROO", "rotation": 9_u64 }),
            "invalid_argument",
            "rotation",
        ),
        (
            "get_sprite",
            json!({ "sprite": "TROO1" }),
            "invalid_argument",
            "four",
        ),
    ]
}

#[tokio::test]
async fn the_image_tools_validate_their_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    refuse_all(&client, &bad_argument_cases()).await;
}

#[tokio::test]
async fn get_patch_publishes_a_doom_patch_with_grab_offsets() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "get_patch", json!({ "name": "wallp1" })).await;
    assert_eq!(report["name"], json!("WALLP1"), "the lump it read");
    check_dims(&report, 16_u64, 16_u64);
    check_offsets(&report, -2_i64, -14_i64);
    let url = asset_url_of(&report, "asset");
    let bytes = fetch_asset(&url).await;
    assert_eq!(
        read_grab(&bytes).expect("a chunked PNG"),
        Some((-2_i32, -14_i32)),
        "the drawing offsets survive in a grAb chunk"
    );
    let image = decode_image(&bytes).expect("the asset decodes");
    assert_eq!(
        image.get(0, 0),
        Some([100_u8, 155, 100, 255]),
        "the fill index 100 in palette colour 100"
    );
    let doubled = call(
        &client,
        "get_patch",
        json!({ "name": "WALLP1", "scale": 2_u64 }),
    )
    .await;
    check_dims(&doubled, 32_u64, 32_u64);
    check_offsets(&doubled, -4_i64, -28_i64);
    assert_eq!(
        read_grab(&fetch_asset(&asset_url_of(&doubled, "asset")).await).expect("a chunked PNG"),
        Some((-4_i32, -28_i32)),
        "the chunk scales along with the picture"
    );
    let rgba = call(
        &client,
        "get_patch",
        json!({ "name": "WALLP1", "format": "rgba" }),
    )
    .await;
    assert_eq!(
        rgba["asset"]["filename"],
        json!("WALLP1.rgba"),
        "the raw asset"
    );
    assert_eq!(
        fetch_asset(&asset_url_of(&rgba, "asset")).await.len(),
        16_usize
            .checked_mul(16)
            .expect("16x16 fits")
            .checked_mul(4)
            .expect("RGBA8 fits"),
        "row-major RGBA8 bytes"
    );
}

#[tokio::test]
async fn get_patch_resolves_a_png_lump_and_a_working_override() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    add(
        &client,
        "PNGPAT",
        &image_png(3, 2, [17_u8, 238, 17], Some((3_i32, -4_i32))),
    )
    .await;
    let report = call(&client, "get_patch", json!({ "name": "PNGPAT" })).await;
    check_dims(&report, 3_u64, 2_u64);
    check_offsets(&report, 3_i64, -4_i64);
    let image = fetch_image(&asset_url_of(&report, "asset")).await;
    assert_eq!(
        image.get(0, 0),
        Some([17_u8, 238, 17, 255]),
        "a PNG patch is quantized to palette 0 and recoloured from it"
    );
    let loose = call(&client, "list_patches", json!({ "pattern": "PNGPAT" })).await;
    assert_eq!(
        loose["total"],
        json!(0_u64),
        "an unnamed global is no patch"
    );
    call(
        &client,
        "replace_lump",
        json!({ "name": "WALLP1", "hex": stripe_hex(6, 1, (0_i16, 0_i16)) }),
    )
    .await;
    let override_report = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
    check_dims(&override_report, 6_u64, 2_u64);
    assert_eq!(
        override_report["name"],
        json!("WALLP1"),
        "the working copy wins the merged view"
    );
}
