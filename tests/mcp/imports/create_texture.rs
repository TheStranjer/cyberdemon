//! The one-shot `create_texture_from_image`: the size rules, the
//! automatic `fit`/`make_tileable` steps, the `PNAMES`/patch/
//! definition landing together, the layer-patch and texture-name
//! collisions with and without `replace`, `assign_role` against the
//! theme, and the single `undo`.

use std::path::Path;

use serde_json::{Value, json};

use crate::imports::{
    Client, FIXTURE_PNAMES, fetch_asset, flat_png, iwad_client, source_args, write_file,
};
use crate::support::{call, call_err};

/// One `create_texture_from_image` of `file` with `extra`.
async fn create_texture(client: &Client, file: &Path, extra: Value) -> Value {
    call(
        client,
        "create_texture_from_image",
        source_args(file, extra),
    )
    .await
}

/// The default size rule: a 64x128 picture is kept, a 100x60 one
/// falls back to 128x128 with the documented note, and an explicit
/// width keeps the aspect.
#[tokio::test]
async fn create_texture_settles_a_tiling_friendly_size() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let tall = write_file(
        dir.path(),
        "TALL.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let kept = create_texture(&client, &tall, json!({ "name": "KEPT" })).await;
    assert_eq!(
        (kept["width"].as_u64(), kept["height"].as_u64()),
        (Some(64_u64), Some(128_u64)),
        "the kept size"
    );
    assert_eq!(kept["notes"], json!([]), "a kept size adds no note");
    assert_eq!(kept["steps"], json!([]), "no automatic op at this size");
    let odd = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 60_u32, |_| false));
    let fallback = create_texture(&client, &odd, json!({ "name": "FALLBACK" })).await;
    assert_eq!(
        (fallback["width"].as_u64(), fallback["height"].as_u64()),
        (Some(128_u64), Some(128_u64)),
        "the square fallback"
    );
    assert_eq!(
        fallback["notes"],
        json!(["resized to 128x128; pass width/height to choose"]),
        "the documented note"
    );
    assert_eq!(
        fallback["steps"][0]["op"],
        json!("fit"),
        "the automatic fit"
    );
    let aspect = create_texture(
        &client,
        &tall,
        json!({ "name": "ASPECT", "width": 128_u64 }),
    )
    .await;
    assert_eq!(aspect["height"].as_u64(), Some(256_u64), "the aspect kept");
    assert_eq!(
        aspect["width"].as_u64(),
        Some(128_u64),
        "the explicit width"
    );
}

/// A non-power-of-two width refuses, naming the nearest valid widths.
#[tokio::test]
async fn create_texture_refuses_widths_that_cannot_tile() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 60_u32, |_| false));
    let (code, message, _) = call_err(
        &client,
        "create_texture_from_image",
        source_args(&file, json!({ "name": "NOPE", "width": 100_u64 })),
    )
    .await;
    assert_eq!(code, "invalid_argument", "100 is no tile width");
    assert!(
        message.contains("64 or 128"),
        "the message names the neighbours: {message}"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "nothing was written");
}

/// `tileable` appends the `make_tileable` step to the report.
#[tokio::test]
async fn create_texture_appends_the_tileable_op() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "TILE.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let report = create_texture(&client, &file, json!({ "name": "TILE", "tileable": true })).await;
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 1_usize, "only the tiling op ran");
    assert_eq!(steps[0]["op"], json!("make_tileable"), "the tileable step");
}

/// The whole write lands: the texture is listed, the patch is in
/// `PNAMES` and decodes, and the definition preview has its size.
#[tokio::test]
async fn create_texture_stores_patch_pnames_and_definition_together() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "MYTEX.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let report = create_texture(&client, &file, json!({ "name": "MYTEX" })).await;
    assert_eq!(report["texture"], json!("MYTEX"), "the texture name");
    assert_eq!(report["patch"], json!("MYTEX"), "the patch follows");
    assert_eq!(report["lump"], json!("TEXTURE1"), "the default target");
    assert!(report["preview"]["url"].is_string(), "a preview asset");
    let listed = call(&client, "list_textures", json!({ "pattern": "MYTEX" })).await;
    assert_eq!(listed["total"], json!(1_u64), "the texture is defined");
    assert_eq!(
        listed["textures"][0]["width"],
        json!(64_u64),
        "the record size"
    );
    assert_eq!(
        listed["textures"][0]["patches"],
        json!(1_u64),
        "the single patch"
    );
    let pnames = call(&client, "list_pnames", json!({ "pattern": "MYTEX" })).await;
    assert_eq!(pnames["total"], json!(1_u64), "PNAMES lists the patch");
    assert_eq!(
        pnames["pnames"][0]["lump_found"],
        json!(true),
        "and the lump stands"
    );
    let patch = call(&client, "get_patch", json!({ "name": "MYTEX" })).await;
    assert_eq!(patch["width"], json!(64_u64), "the patch decodes");
    assert_eq!(patch["height"], json!(128_u64), "at the texture size");
    assert_eq!(patch["left_offset"], json!(0_i64), "zero offsets");
    assert_eq!(patch["top_offset"], json!(0_i64), "both ways");
    let preview = call(
        &client,
        "get_asset_url",
        json!({ "name": "MYTEX", "prefer": "texture" }),
    )
    .await;
    assert_eq!(
        preview["resolved_as"],
        json!("texture"),
        "the definition serves"
    );
    assert_eq!(
        (preview["width"].as_u64(), preview["height"].as_u64()),
        (Some(64_u64), Some(128_u64)),
        "the preview matches the record"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["create_texture MYTEX"]),
        "one undoable step"
    );
}

/// A patch name a lower layer holds refuses until `replace`, and a
/// defined texture name refuses until `replace` too.
#[tokio::test]
async fn create_texture_collisions_refuse_until_replace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "CLASH.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let (patch_code, patch_message, _) = call_err(
        &client,
        "create_texture_from_image",
        source_args(&file, json!({ "name": "CLASH", "patch_name": "WALLP1" })),
    )
    .await;
    assert_eq!(patch_code, "already_exists", "the IWAD patch stands");
    assert!(
        patch_message.contains("a patch named WALLP1 already exists in the IWAD"),
        "the message names the layer: {patch_message}"
    );
    assert!(
        patch_message.contains("pass patch_name"),
        "and the way out: {patch_message}"
    );
    let (texture_code, texture_message, _) = call_err(
        &client,
        "create_texture_from_image",
        source_args(&file, json!({ "name": "WALL1" })),
    )
    .await;
    assert_eq!(texture_code, "already_exists", "the fixture texture stands");
    assert!(
        texture_message.contains("replace=true"),
        "the message names the way out: {texture_message}"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "neither refusal wrote");
    let replaced = create_texture(
        &client,
        &file,
        json!({ "name": "WALL1", "patch_name": "WALLP1", "replace": true }),
    )
    .await;
    assert_eq!(replaced["lump"], json!("TEXTURE1"), "redefined in place");
    let patch = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
    assert_eq!(patch["width"], json!(64_u64), "the override serves");
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "WALLP1 kept its entry"
    );
}

/// `assign_role` points the theme at the new texture, and the theme
/// note says undo will not roll it back.
#[tokio::test]
async fn assign_role_points_the_theme_at_the_new_texture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "ROLE.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let report = create_texture(
        &client,
        &file,
        json!({ "name": "ROLE", "assign_role": "wall" }),
    )
    .await;
    assert_eq!(report["role"], json!("wall"), "the assigned role");
    assert!(
        report["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("theme override wall -> ROLE")
                    && text.contains("undo does not roll it back"))),
        "the note explains the theme state: {}",
        report["notes"]
    );
    let theme = call(&client, "get_texture_theme", json!({})).await;
    assert_eq!(
        theme["roles"]["wall"]["texture"],
        json!("ROLE"),
        "the role resolves"
    );
    assert_eq!(
        theme["roles"]["wall"]["matched"],
        json!("exact"),
        "as an override"
    );
}

/// One `create_texture_from_image` call asking for role `role`.
async fn refuse_role(client: &Client, file: &Path, role: &str) {
    let (code, message, _) = call_err(
        client,
        "create_texture_from_image",
        source_args(file, json!({ "name": "NOPE", "assign_role": role })),
    )
    .await;
    assert_eq!(code, "invalid_argument", "{role} refuses");
    assert!(
        message.contains("flat") || message.contains("role"),
        "the message explains {role}: {message}"
    );
}

/// Both the flat role and an unknown one refuse.
async fn refuse_roles(client: &Client, file: &Path) {
    for role in ["floor", "roof"] {
        refuse_role(client, file, role).await;
    }
}

/// A flat role (and an unknown one) refuse before anything is
/// written; an empty undo stack proves it.
#[tokio::test]
async fn flat_roles_refuse_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "FLATROLE.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    refuse_roles(&client, &file).await;
    let listed = call(&client, "list_textures", json!({ "pattern": "NOPE" })).await;
    assert_eq!(listed["total"], json!(0_u64), "nothing was defined");
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(session["undo"], json!([]), "no edit landed");
    let theme = call(&client, "get_texture_theme", json!({})).await;
    assert_ne!(
        theme["roles"]["floor"]["texture"],
        json!("NOPE"),
        "the theme is untouched"
    );
}

/// One `undo` removes the patch, the `PNAMES` entry and the texture
/// definition together.
#[tokio::test]
async fn one_undo_rolls_back_the_whole_created_texture() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "UNDONE.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    create_texture(&client, &file, json!({ "name": "UNDONE" })).await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["create_texture UNDONE"]),
        "the single step rolls back"
    );
    let textures = call(&client, "list_textures", json!({ "pattern": "UNDONE" })).await;
    assert_eq!(textures["total"], json!(0_u64), "the definition left");
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(
        pnames["total"],
        json!(FIXTURE_PNAMES),
        "the PNAMES entry left"
    );
    let patch = call_err(&client, "get_patch", json!({ "name": "UNDONE" })).await;
    assert_eq!(patch.0, "not_found", "the patch lump left with them");
}

/// A 64x128 palette PNG with every other pixel fully transparent.
fn holes_png() -> Vec<u8> {
    let even = |p: usize| p.checked_rem(2_usize).is_some_and(|rest| rest == 0_usize);
    flat_png(64_u32, 128_u32, even)
}

/// A transparent patch side is filled for the wall and warned about,
/// and `ops` run before the size rule.
#[tokio::test]
async fn create_texture_fills_transparent_pixels_and_runs_user_ops() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(dir.path(), "HOLES.png", &holes_png());
    let report = create_texture(
        &client,
        &file,
        json!({
            "name": "HOLES",
            "ops": [{ "op": "flip", "horizontal": true, "vertical": false }],
        }),
    )
    .await;
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps[0]["op"], json!("flip"), "the user op ran first");
    let warnings = report["warnings"].as_array().expect("a warning list");
    assert_eq!(warnings.len(), 1_usize, "one fill warning");
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|text| text.contains("palette colour nearest to black")),
        "the warning explains the fill: {warnings:?}"
    );
    let patch = call(
        &client,
        "get_patch",
        json!({ "name": "HOLES", "format": "rgba" }),
    )
    .await;
    let url = patch["asset"]["url"].as_str().expect("an asset");
    let rgba = fetch_asset(url).await;
    assert!(
        rgba.chunks_exact(4).all(|pixel| pixel[3] == 255_u8),
        "the wall patch is fully opaque"
    );
}

/// The layer-patch clash reads the working archive's `patches` region, not
/// just its names: a same-named flat the session holds does not hide the
/// layer's patch, and only `replace=true` accepts the override.
#[tokio::test]
async fn create_texture_clashes_with_a_layer_patch_despite_a_working_flat_twin() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let flat = write_file(
        dir.path(),
        "WALLP1.png",
        &flat_png(64_u32, 64_u32, |_| false),
    );
    call(
        &client,
        "import_image_as_flat",
        source_args(&flat, json!({ "name": "WALLP1" })),
    )
    .await;
    let file = write_file(
        dir.path(),
        "SHADOW.png",
        &flat_png(64_u32, 128_u32, |_| false),
    );
    let (code, message, _) = call_err(
        &client,
        "create_texture_from_image",
        source_args(&file, json!({ "name": "SHADOW", "patch_name": "WALLP1" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the working flat is no twin");
    assert!(
        message.contains("a patch named WALLP1 already exists in the IWAD"),
        "the message names the layer's patch: {message}"
    );
    let session = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        session["undo"],
        json!(["import_flat WALLP1"]),
        "the refusal wrote nothing"
    );
    let replaced = create_texture(
        &client,
        &file,
        json!({ "name": "SHADOW", "patch_name": "WALLP1", "replace": true }),
    )
    .await;
    assert_eq!(replaced["patch"], json!("WALLP1"), "replace overrides");
    assert_eq!(
        replaced["lump"],
        json!("TEXTURE1"),
        "the texture is defined"
    );
    let flat_back = call(&client, "get_flat", json!({ "name": "WALLP1" })).await;
    assert_eq!(flat_back["width"], json!(64_u64), "the flat still serves");
}
