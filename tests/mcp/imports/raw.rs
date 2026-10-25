//! `import_image_as_raw`: validated image bytes stored byte-for-byte,
//! placed by namespace with `replace` obeyed, with the format named and
//! non-images refused.

use serde_json::{Value, json};

use crate::imports::{FIXTURE_PNAMES, iwad_client, palette_png, source_args, write_file};
use crate::support::{call, call_err};

/// A 4x4 `JPEG`, encoded with the `image` crate the library ships.
fn jpeg_of() -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder as _};
    let side = 4_usize;
    let bytes = side
        .checked_mul(side)
        .expect("test pixels fit")
        .checked_mul(3)
        .expect("test rgb fits");
    let rgb = vec![3_u8; bytes];
    let mut out: Vec<u8> = Vec::new();
    JpegEncoder::new(std::io::Cursor::new(&mut out))
        .write_image(&rgb, 4_u32, 4_u32, ExtendedColorType::Rgb8)
        .expect("a 4x4 image encodes as JPEG");
    out
}

/// A raw import stores the file's own bytes, loose by default, and
/// never touches `PNAMES`.
#[tokio::test]
async fn raw_imports_keep_the_original_bytes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let png = palette_png(4_u32, 4_u32, None, Some((2_i32, 6_i32)));
    let file = write_file(dir.path(), "RAWTEX.png", &png);
    let report = call(
        &client,
        "import_image_as_raw",
        source_args(&file, Value::Null),
    )
    .await;
    assert_eq!(report["name"], json!("RAWTEX"), "derived from the file");
    assert_eq!(report["format"], json!("png"), "the sniffed format");
    assert_eq!(report["width"], json!(4_u64), "the decoded width");
    assert_eq!(report["height"], json!(4_u64), "the decoded height");
    let out = dir.path().join("roundtrip.png");
    call(
        &client,
        "extract_lump",
        json!({ "name": "RAWTEX", "path": out.display().to_string() }),
    )
    .await;
    assert_eq!(
        std::fs::read(&out).expect("read back"),
        png,
        "byte for byte"
    );
    let listed = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    let rows = listed["lumps"].as_array().expect("a listing");
    assert_eq!(rows.len(), 1, "one loose lump");
    assert_eq!(rows[0]["name"], json!("RAWTEX"), "the stored lump");
    assert_eq!(rows[0]["namespace"], Value::Null, "stored loose");
    let pnames = call(&client, "list_pnames", json!({})).await;
    assert_eq!(pnames["total"], json!(FIXTURE_PNAMES), "PNAMES untouched");
}

/// A raw import joins a named region and obeys the duplicate rule.
#[tokio::test]
async fn raw_imports_place_by_namespace_and_obey_replace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "INREG.png",
        &palette_png(4_u32, 4_u32, None, None),
    );
    let placed = call(
        &client,
        "import_image_as_raw",
        source_args(&file, json!({ "namespace": "patches" })),
    )
    .await;
    assert_eq!(
        placed["lump_index"],
        json!(1_u64),
        "inside the created region"
    );
    let listed = call(
        &client,
        "list_lumps",
        json!({ "scope": "working", "namespace": "patches" }),
    )
    .await;
    assert_eq!(
        listed["lumps"][1]["namespace"],
        json!("patches"),
        "the region classifies it"
    );
    let (code, message, _) = call_err(
        &client,
        "import_image_as_raw",
        source_args(&file, json!({ "namespace": "patches" })),
    )
    .await;
    assert_eq!(code, "already_exists", "the twin refuses");
    assert!(
        message.contains("replace=true"),
        "the message names the way out: {message}"
    );
    let replaced = call(
        &client,
        "import_image_as_raw",
        source_args(&file, json!({ "namespace": "patches", "replace": true })),
    )
    .await;
    assert_eq!(
        replaced["lump_index"], placed["lump_index"],
        "replace overwrites in place"
    );
}

/// A raw import names the format, refuses non-images and bad regions.
#[tokio::test]
async fn raw_imports_name_the_format_and_refuse_non_images() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let jpeg = write_file(dir.path(), "PHOTO.jpg", &jpeg_of());
    let report = call(
        &client,
        "import_image_as_raw",
        source_args(&jpeg, Value::Null),
    )
    .await;
    assert_eq!(report["format"], json!("jpeg"), "the sniffed format");
    assert_eq!(report["name"], json!("PHOTO"), "the stem names it");
    let junk = write_file(
        dir.path(),
        "notes.txt",
        b"this is a text file, not an image",
    );
    let (code, message, _) = call_err(
        &client,
        "import_image_as_raw",
        source_args(&junk, Value::Null),
    )
    .await;
    assert_eq!(code, "format_error", "a non-image refuses");
    assert!(message.contains("image"), "the message explains: {message}");
    let empty = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(empty["total"], json!(1_u64), "only the JPEG landed");
    let (bad_region, _, _) = call_err(
        &client,
        "import_image_as_raw",
        source_args(&jpeg, json!({ "namespace": "nope", "name": "SECOND" })),
    )
    .await;
    assert_eq!(bad_region, "invalid_argument", "an unknown region refuses");
}
