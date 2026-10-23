//! The `PNAMES` and `TEXTURE1`/`TEXTURE2` listings as one `list_*` call
//! reports them over the fixture `IWAD`.
//!
//! An empty session lists nothing rather than failing, the reported rows
//! carry the lump and source they were read from, patterns are
//! case-insensitive globs and paging counts every match, a working
//! `TEXTURE2` keeps the `Strife` layout it was read in through a forced
//! renumber and a truncated lump surfaces as a `format_error`. The lump bytes
//! behind the layout check and [`assert_iwad_texture1_count`], which the
//! `build` tests share, live here.

use std::path::Path;

use cyberdemon::wad::archive::Wad;
use cyberdemon::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, parse_texture_lump, write_texture_lump,
};
use serde_json::json;

use crate::support::{call, call_err, connect};
use crate::textures::{hex_of, iwad_client, listed};

/// A `Strife`-layout lump (no column directory, three-field mappatch)
/// for the layout-detection check.
fn strife_bytes() -> Vec<u8> {
    let lump = TextureLump {
        layout: TextureLayout::Strife,
        textures: vec![TextureDef {
            name: "STRIFEY".to_owned(),
            flags_raw: 0,
            width: 40,
            height: 24,
            patches: vec![
                PatchRef {
                    patch: 0,
                    x: 0,
                    y: 8,
                },
                PatchRef {
                    patch: 0,
                    x: 16,
                    y: 8,
                },
            ],
        }],
    };
    write_texture_lump(&lump).expect("the test lump serializes")
}

/// Asserts the fixture `IWAD` on disk still defines exactly `count`
/// textures in its `TEXTURE1`.
pub(super) fn assert_iwad_texture1_count(base: &Path, count: usize) {
    let on_disk = Wad::read_file(base).expect("the fixture file reads");
    let original = on_disk
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"TEXTURE1")
        .expect("the IWAD still holds its TEXTURE1");
    let untouched = parse_texture_lump(&original.data, 2).expect("the original decodes");
    assert_eq!(
        untouched.textures.len(),
        count,
        "the IWAD's TEXTURE1 holds {count} definitions"
    );
}

#[tokio::test]
async fn an_empty_session_lists_no_textures_or_names() {
    let client = connect().await;
    let textures = call(&client, "list_textures", json!({})).await;
    assert_eq!(textures["total"], json!(0_u64), "no texture lumps");
    assert_eq!(
        textures["textures"],
        json!([]),
        "an empty list, not an error"
    );
    let names = call(&client, "list_pnames", json!({})).await;
    assert_eq!(names["total"], json!(0_u64), "no PNAMES lump");
    assert_eq!(names["pnames"], json!([]), "an empty list, not an error");
}

#[tokio::test]
async fn list_textures_reports_the_fixture_definitions() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "list_textures", json!({})).await;
    assert_eq!(report["total"], json!(2_u64), "WALL1 and STEP1");
    assert_eq!(
        listed(&report, "textures"),
        vec!["WALL1", "STEP1"],
        "lump order"
    );
    let wall = &report["textures"][0];
    assert_eq!(wall["width"], json!(64_u64), "the tiled width");
    assert_eq!(wall["height"], json!(128_u64), "the tiled height");
    assert_eq!(wall["patches"], json!(32_u64), "four columns, eight rows");
    assert_eq!(
        wall["lump"],
        json!("TEXTURE1"),
        "from the first texture lump"
    );
    assert_eq!(wall["source"], json!({"layer": 0_u64}), "the IWAD's copy");
    let step = &report["textures"][1];
    assert_eq!(step["patches"], json!(16_u64), "four columns, four rows");
}

#[tokio::test]
async fn list_textures_filters_by_glob_and_pages() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let one = call(&client, "list_textures", json!({ "pattern": "w?ll1" })).await;
    assert_eq!(one["total"], json!(1_u64), "case-insensitive glob");
    assert_eq!(listed(&one, "textures"), vec!["WALL1"], "the matched name");
    let none = call(&client, "list_textures", json!({ "pattern": "Z*" })).await;
    assert_eq!(none["total"], json!(0_u64), "nothing matches");
    let page = call(
        &client,
        "list_textures",
        json!({ "offset": 1_u64, "limit": 1_u64 }),
    )
    .await;
    assert_eq!(page["total"], json!(2_u64), "total counts all matches");
    assert_eq!(listed(&page, "textures"), vec!["STEP1"], "the second row");
    let zero_args = json!({ "limit": 0_u64 });
    let (zero_code, zero_message, _) = call_err(&client, "list_textures", zero_args).await;
    assert_eq!(zero_code, "invalid_argument", "limit 0 code");
    assert!(
        zero_message.contains("between 1 and 2000"),
        "the message states the range: {zero_message}"
    );
    let big_args = json!({ "limit": 2001_u64 });
    let (big_code, _, _) = call_err(&client, "list_textures", big_args).await;
    assert_eq!(big_code, "invalid_argument", "limit 2001 code");
}

#[tokio::test]
async fn list_textures_adds_a_working_texture2_and_spots_strife_layouts() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    call(
        &client,
        "add_lump",
        json!({ "name": "TEXTURE2", "hex": hex_of(&strife_bytes()) }),
    )
    .await;
    let report = call(&client, "list_textures", json!({})).await;
    assert_eq!(report["total"], json!(3_u64), "the second lump's texture");
    let last = &report["textures"][2];
    assert_eq!(last["name"], json!("STRIFEY"), "the appended definition");
    assert_eq!(
        last["lump"],
        json!("TEXTURE2"),
        "from the second texture lump"
    );
    assert_eq!(
        last["source"],
        json!("working"),
        "the working archive's copy"
    );
    assert_eq!(last["patches"], json!(2_u64), "the Strife mappatch list");
    // Force an edit that renumbers every texture lump: TEXTURE2 must be
    // written back in the Strife layout it was read in, which the
    // 26-byte size of a zero-patch Strife record proves (a Doom record
    // for the same texture would be 34 bytes).
    let forced = call(
        &client,
        "edit_pnames",
        json!({ "remove": ["WALLP1"], "force": true }),
    )
    .await;
    assert_eq!(
        forced["textures_changed"],
        json!(3_u64),
        "STRIFEY shifts too"
    );
    let still = call(&client, "list_textures", json!({ "pattern": "STRIFEY" })).await;
    assert_eq!(
        still["total"],
        json!(1_u64),
        "the rewritten Strife lump decodes"
    );
    assert_eq!(
        still["textures"][0]["patches"],
        json!(0_u64),
        "its WALLP1 references were dropped"
    );
    let working = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    let texture2 = working["lumps"]
        .as_array()
        .expect("array")
        .iter()
        .find(|row| row["name"] == json!("TEXTURE2"))
        .expect("TEXTURE2 was copied in");
    assert_eq!(
        texture2["size"],
        json!(26_u64),
        "the Strife layout was preserved on write-back"
    );
}

#[tokio::test]
async fn list_pnames_reports_the_index_and_lump_presence() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "list_pnames", json!({})).await;
    assert_eq!(report["total"], json!(2_u64), "both patch names");
    assert_eq!(
        listed(&report, "pnames"),
        vec!["WALLP1", "STEPP1"],
        "index order"
    );
    assert_eq!(
        report["pnames"][0]["index"],
        json!(0_u64),
        "the first index"
    );
    assert_eq!(
        report["pnames"][0]["lump_found"],
        json!(true),
        "WALLP1 has a lump"
    );
    assert_eq!(
        report["pnames"][1]["lump_found"],
        json!(true),
        "STEPP1 has a lump"
    );
    let one = call(
        &client,
        "list_pnames",
        json!({ "pattern": "ste*", "limit": 1_u64 }),
    )
    .await;
    assert_eq!(one["total"], json!(1_u64), "the glob applied");
    assert_eq!(listed(&one, "pnames"), vec!["STEPP1"], "the matched entry");
    let (code, _, _) = call_err(&client, "list_pnames", json!({ "limit": 0_u64 })).await;
    assert_eq!(code, "invalid_argument", "limit 0 code");
}

#[tokio::test]
async fn a_malformed_texture_lump_surfaces_as_format_error() {
    let client = connect().await;
    call(
        &client,
        "add_lump",
        json!({ "name": "PNAMES", "hex": "02 00 00 00 41" }),
    )
    .await;
    let (code, message, _) = call_err(&client, "list_pnames", json!({})).await;
    assert_eq!(code, "format_error", "truncated name list code");
    assert!(
        message.contains("end of data"),
        "the message shows where it ran out: {message}"
    );
}
