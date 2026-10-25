//! Where a patch import's picture and name come from: the filename
//! stem as the unnamed lump name, the refusal catalogue for bad images
//! and parameters, the palette and offset-range checks a valid picture
//! still faces, the missing-`PLAYPAL` quantization refusal, and an
//! uploaded asset `URL` as the source.

use serde_json::{Value, json};

use crate::imports::{
    Refusal, SIDE, expected_rgba, hole_png, iwad_client, palette_png, patch_rgba, refuse_all,
    source_args, write_file,
};
use crate::support::{call, call_err, connect, http_put};

/// An unnamed import derives the lump name from the file stem.
#[tokio::test]
async fn the_name_comes_from_the_filename_when_unnamed() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let file = write_file(
        dir.path(),
        "my_art.png",
        &palette_png(SIDE, SIDE, None, None),
    );
    let report = call(
        &client,
        "import_image_as_patch",
        source_args(&file, Value::Null),
    )
    .await;
    assert_eq!(report["name"], json!("MY_ART"), "the stem uppercases");
    assert_eq!(report["pnames_index"], json!(2_u64), "and PNAMES lists it");
}

/// Bad sources and parameters each report their code, and nothing is
/// stored along the way.
#[tokio::test]
async fn bad_images_and_parameters_are_refused_without_edits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
    let qoi = write_file(dir.path(), "picture.qoi", b"qoif\0\0\0\x03\0\0\0\x02");
    let unnamed = write_file(dir.path(), "###.png", &palette_png(SIDE, SIDE, None, None));
    let cases: [Refusal; 7] = [
        (
            "junk",
            source_args(&junk, Value::Null),
            "format_error",
            "image",
        ),
        ("qoi", source_args(&qoi, Value::Null), "unsupported", "PNG"),
        (
            "unnamed",
            source_args(&unnamed, Value::Null),
            "invalid_argument",
            "derive",
        ),
        (
            "long name",
            source_args(&junk, json!({ "name": "TOO LONG NAME" })),
            "invalid_argument",
            "name",
        ),
        (
            "mode",
            source_args(
                &junk,
                json!({ "name": "X", "transparency": { "mode": "blur" } }),
            ),
            "invalid_argument",
            "\"none\"",
        ),
        (
            "preset",
            source_args(&junk, json!({ "name": "X", "offsets": "middle" })),
            "invalid_argument",
            "center_bottom",
        ),
        (
            "missing",
            json!({ "source": dir.path().join("absent.png").display().to_string() }),
            "not_found",
            "absent.png",
        ),
    ];
    refuse_all(&client, "import_image_as_patch", &cases).await;
    let empty = call(&client, "list_patches", json!({})).await;
    assert_eq!(empty["total"], json!(2_u64), "nothing was stored");
}

/// A decodable picture still checks the palette index and the `i16`
/// offset range.
#[tokio::test]
async fn a_valid_picture_still_checks_palette_and_offset_range() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let good = write_file(dir.path(), "GOOD.png", &palette_png(SIDE, SIDE, None, None));
    let cases: [Refusal; 2] = [
        (
            "palette",
            source_args(&good, json!({ "name": "X", "palette": 99_u64 })),
            "not_found",
            "PLAYPAL",
        ),
        (
            "huge offset",
            source_args(
                &good,
                json!({ "name": "X", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
            ),
            "invalid_argument",
            "does not fit",
        ),
    ];
    refuse_all(&client, "import_image_as_patch", &cases).await;
}

/// Without a `PLAYPAL` the quantization step has nothing to answer.
#[tokio::test]
async fn a_session_without_a_playpal_cannot_quantize() {
    let client = connect().await;
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = write_file(dir.path(), "ART.png", &palette_png(SIDE, SIDE, None, None));
    let (code, message, _) = call_err(
        &client,
        "import_image_as_patch",
        source_args(&file, json!({ "name": "ART" })),
    )
    .await;
    assert_eq!(code, "not_found", "no palette to quantize against");
    assert!(
        message.contains("PLAYPAL"),
        "the message names it: {message}"
    );
}

/// The `source` also accepts this server's own upload `URL`s.
#[tokio::test]
async fn a_source_can_be_an_uploaded_asset_url() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let info = call(&client, "get_asset_server_info", json!({})).await;
    let base = info["base_url"]
        .as_str()
        .expect("an asset origin")
        .to_owned();
    let (status, receipt) = http_put(&base, "/upload/UPLOADED.png", &hole_png()).await;
    assert_eq!(status, 201, "the upload is accepted");
    let url: String = serde_json::from_slice::<Value>(&receipt).expect("a JSON receipt")["url"]
        .as_str()
        .expect("the receipt's url")
        .to_owned();
    let report = call(&client, "import_image_as_patch", json!({ "source": url })).await;
    assert_eq!(report["name"], json!("UPLOADED"), "the url names the lump");
    assert_eq!(
        report["transparent_pixels"],
        json!(1_u64),
        "the hole survived"
    );
    assert_eq!(patch_rgba(&client, "UPLOADED").await, expected_rgba());
}
