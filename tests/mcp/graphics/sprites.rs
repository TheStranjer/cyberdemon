//! The sprite readers: `get_sprite` serving a named lump as stored and a
//! mirror-pair rotation flipped horizontally, the all-angles image that answers
//! every rotation of a one-image frame, `decode_patch` publishing both
//! encodings of a lump or of a patch file, and `get_thing_sprite` previewing a
//! rotation from a definition's type number, including the custom `DECORATE`
//! and `MAPINFO` rows. The pair-rotation reader and the thing-sprite refusal
//! table live here with them.

use cyberdemon::gfx::codec::decode_image;
use cyberdemon::gfx::grab::read_grab;
use serde_json::json;

use crate::fixtures::{iwad, pwad_custom_defs};
use crate::graphics::{
    Client, Refusal, add_in, add_stripe_mirror, asset_url, check_dims, check_offsets, fetch_asset,
    fetch_image, hex_of, iwad_client, path_arg, refuse, refuse_all, stripe_bytes,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn get_sprite_serves_a_named_lump_as_stored() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    add_stripe_mirror(&client).await;
    let report = call(&client, "get_sprite", json!({ "name": "trooa1" })).await;
    assert_eq!(report["name"], json!("TROOA1"), "the requested name");
    assert_eq!(
        report["lump"],
        json!("TROOA1"),
        "and the lump that served it"
    );
    assert_eq!(report["mirrored"], json!(false), "served as stored");
    check_dims(&report, 8_u64, 8_u64);
    check_offsets(&report, 0_i64, 0_i64);
    let pair = call(&client, "get_sprite", json!({ "name": "TROOA2A8" })).await;
    assert_eq!(pair["lump"], json!("TROOA2A8"), "the whole name is read");
    assert_eq!(pair["mirrored"], json!(false), "as stored, not flipped");
    check_dims(&pair, 4_u64, 2_u64);
    check_offsets(&pair, 1_i64, 2_i64);
    let image = fetch_image(&asset_url(&pair, "asset")).await;
    assert_eq!(
        image.get(0, 0).map(|[_, _, _, alpha]| alpha),
        Some(255),
        "the stored image keeps its opaque column at the left"
    );
    assert_eq!(
        read_grab(&fetch_asset(&asset_url(&pair, "asset")).await).expect("a chunked PNG"),
        Some((1_i32, 2_i32)),
        "grAb carries the stored offsets"
    );
}

#[tokio::test]
async fn get_sprite_serves_a_mirrored_pair_rotation_flipped() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    add_stripe_mirror(&client).await;
    let direct = call(
        &client,
        "get_sprite",
        json!({ "sprite": "troo", "frame": "a", "rotation": 1_u64 }),
    )
    .await;
    assert_eq!(direct["lump"], json!("TROOA1"), "case-insensitive parts");
    assert_eq!(
        direct["mirrored"],
        json!(false),
        "the rotation has its own lump"
    );
    check_dims(&direct, 8_u64, 8_u64);
    let eight = call(
        &client,
        "get_sprite",
        json!({ "sprite": "TROO", "frame": "A", "rotation": 8_u64 }),
    )
    .await;
    assert_eq!(
        eight["name"],
        json!("TROOA8"),
        "the rotation that was asked for"
    );
    assert_eq!(
        eight["lump"],
        json!("TROOA2A8"),
        "served by the mirror pair"
    );
    assert_eq!(eight["mirrored"], json!(true), "flipped horizontally");
    check_dims(&eight, 4_u64, 2_u64);
    check_offsets(&eight, 3_i64, 2_i64);
    let url = asset_url(&eight, "asset");
    let image = fetch_image(&url).await;
    assert_eq!(
        image.get(3, 0).map(|[_, _, _, alpha]| alpha),
        Some(255),
        "the opaque column moved to the right edge"
    );
    assert_eq!(
        image.get(0, 1).map(|[_, _, _, alpha]| alpha),
        Some(0),
        "and the left edge is a hole"
    );
    assert_eq!(
        read_grab(&fetch_asset(&url).await).expect("a chunked PNG"),
        Some((3_i32, 2_i32)),
        "the mirrored left offset is what gets stored"
    );
}

/// Asserts one pair-named rotation resolves to the lump that carries
/// it, flipped only for the second of its two rotations.
async fn pair_serves_its_own_rotation(client: &Client, rotation: u64, lump: &str, flipped: bool) {
    let served = call(
        client,
        "get_sprite",
        json!({ "sprite": "TROO", "frame": "A", "rotation": rotation }),
    )
    .await;
    assert_eq!(served["lump"], json!(lump), "rotation {rotation}");
    assert_eq!(
        served["mirrored"],
        json!(flipped),
        "rotation {rotation} of {lump}"
    );
    check_dims(&served, 8_u64, 8_u64);
}

/// An `IWAD` writes a frame's rotations `2`, `3` and `4` into its
/// mirror-pair lumps and nowhere else: the same lump that serves the
/// high rotation flipped has to serve the low one as stored.
#[tokio::test]
async fn get_sprite_serves_a_pair_names_own_rotation_unflipped() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    pair_serves_its_own_rotation(&client, 2_u64, "TROOA2A8", false).await;
    pair_serves_its_own_rotation(&client, 3_u64, "TROOA3A7", false).await;
    pair_serves_its_own_rotation(&client, 4_u64, "TROOA4A6", false).await;
    pair_serves_its_own_rotation(&client, 6_u64, "TROOA4A6", true).await;
    pair_serves_its_own_rotation(&client, 7_u64, "TROOA3A7", true).await;
    let thing = call(
        &client,
        "get_thing_sprite",
        json!({ "type": 3001_u64, "rotation": 3_u64 }),
    )
    .await;
    assert_eq!(thing["lump"], json!("TROOA3A7"), "by definition too");
    assert_eq!(thing["mirrored"], json!(false), "as stored");
}

#[tokio::test]
async fn get_sprite_falls_back_to_the_all_angles_image() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let angles = call(
        &client,
        "get_sprite",
        json!({ "sprite": "PLAY", "rotation": 5_u64 }),
    )
    .await;
    assert_eq!(
        angles["lump"],
        json!("PLAYA0"),
        "the all-angles image serves any rotation"
    );
    assert_eq!(angles["mirrored"], json!(false), "used as stored");
    check_dims(&angles, 16_u64, 16_u64);
    assert_eq!(
        angles["name"],
        json!("PLAYA5"),
        "the frame defaults to A and the report keeps the request"
    );
    let zero = call(
        &client,
        "get_sprite",
        json!({ "sprite": "PLAY", "rotation": 0_u64, "scale": 3_u64 }),
    )
    .await;
    check_dims(&zero, 48_u64, 48_u64);
    assert_eq!(zero["lump"], json!("PLAYA0"), "rotation 0 is itself");
}

#[tokio::test]
async fn decode_patch_publishes_both_encodings_of_a_lump() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let report = call(&client, "decode_patch", json!({ "name": "stepp1" })).await;
    assert_eq!(report["name"], json!("STEPP1"), "the lump it read");
    check_dims(&report, 16_u64, 8_u64);
    check_offsets(&report, 0_i64, 0_i64);
    assert_eq!(report["palette"], json!(0_u64), "the default palette");
    let png_url = asset_url(&report, "png");
    assert_eq!(
        report["png"]["filename"],
        json!("STEPP1.png"),
        "the PNG asset name"
    );
    assert_eq!(
        report["rgba"]["filename"],
        json!("STEPP1.rgba"),
        "the raw asset name"
    );
    let bytes = fetch_asset(&png_url).await;
    assert_eq!(
        read_grab(&bytes).expect("a chunked PNG"),
        Some((0_i32, 0_i32)),
        "the offsets are published even when they are zero"
    );
    let image = decode_image(&bytes).expect("the PNG decodes");
    assert_eq!(
        image.get(15, 7),
        Some([150_u8, 105, 150, 255]),
        "the fill index 150 in colour"
    );
    let raw = fetch_asset(&asset_url(&report, "rgba")).await;
    assert_eq!(
        raw.len(),
        16_usize
            .checked_mul(8)
            .expect("16x8 fits")
            .checked_mul(4)
            .expect("RGBA8 fits"),
        "width*height*4 raw bytes"
    );
    assert_eq!(&raw[..4], &[150_u8, 105, 150, 255], "row-major RGBA8");
    let coloured = call(
        &client,
        "decode_patch",
        json!({ "name": "STEPP1", "palette": 13_u64 }),
    )
    .await;
    assert_eq!(
        coloured["palette"],
        json!(13_u64),
        "the palette entry the caller asked for"
    );
}

#[tokio::test]
async fn decode_patch_reads_a_file_and_refuses_bad_inputs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let patch = dir.path().join("MYPATCH.dat");
    std::fs::write(&patch, stripe_bytes(5, 4, (-1_i16, -2_i16))).expect("write the file");
    let report = call(
        &client,
        "decode_patch",
        json!({ "source": patch.display().to_string() }),
    )
    .await;
    assert_eq!(
        report["name"],
        json!("MYPATCH"),
        "the file stem names the patch"
    );
    check_dims(&report, 5_u64, 2_u64);
    check_offsets(&report, -1_i64, -2_i64);
    let junk = dir.path().join("notes.txt");
    std::fs::write(&junk, b"this is a text file, not a patch").expect("write the file");
    refuse_all(
        &client,
        &[
            (
                "decode_patch",
                json!({ "source": junk.display().to_string() }),
                "format_error",
                "patch",
            ),
            (
                "decode_patch",
                json!({ "name": "WALLP1", "source": junk.display().to_string() }),
                "invalid_argument",
                "not both",
            ),
            (
                "decode_patch",
                json!({ "palette": 0_u64 }),
                "invalid_argument",
                "needs a patch",
            ),
            (
                "decode_patch",
                json!({ "name": "NOPE" }),
                "not_found",
                "NOPE",
            ),
            (
                "decode_patch",
                json!({ "source": dir.path().join("absent.dat").display().to_string() }),
                "not_found",
                "absent.dat",
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn get_thing_sprite_publishes_the_definition_rotation() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let imp = call(&client, "get_thing_sprite", json!({ "type": 3001_u64 })).await;
    assert_eq!(imp["type"], json!(3001_u64), "the number echoes");
    assert_eq!(imp["name"], json!("Imp"), "the definition's display name");
    assert_eq!(imp["sprite"], json!("TROO"), "the sprite column");
    assert_eq!(
        imp["frame"],
        json!("A"),
        "the definition's frame by default"
    );
    assert_eq!(imp["rotation"], json!(1_u64), "the documented default");
    assert_eq!(imp["lump"], json!("TROOA1"), "the exact rotation lump");
    assert_eq!(imp["mirrored"], json!(false), "served as stored");
    assert_eq!(imp["hanging"], json!(false), "an Imp stands on the floor");
    check_dims(&imp, 8_u64, 8_u64);
    check_offsets(&imp, 0_i64, 0_i64);
    assert_eq!(
        imp["asset"]["filename"],
        json!("TROOA1.png"),
        "published under the rotation name"
    );
    let image = fetch_image(&asset_url(&imp, "asset")).await;
    assert_eq!(
        image.get(0, 0),
        Some([51_u8, 204, 51, 255]),
        "the fixture fill index in palette colour"
    );
    let eight = call(
        &client,
        "get_thing_sprite",
        json!({ "type": 3001_u64, "rotation": 8_u64 }),
    )
    .await;
    assert_eq!(eight["lump"], json!("TROOA2A8"), "the mirror pair serves 8");
    assert_eq!(eight["mirrored"], json!(true), "flipped horizontally");
    check_offsets(&eight, 8_i64, 0_i64);
}

#[tokio::test]
async fn get_thing_sprite_scales_up_and_marks_a_hanging_thing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let raw = call(
        &client,
        "get_thing_sprite",
        json!({ "type": 3001_u64, "format": "rgba", "scale": 2_u64 }),
    )
    .await;
    check_dims(&raw, 16_u64, 16_u64);
    assert_eq!(
        raw["asset"]["filename"],
        json!("TROOA1.rgba"),
        "the raw encoding"
    );
    check_offsets(&raw, 0_i64, 0_i64);
    add_in(
        &client,
        "GOR1A1",
        &hex_of(&stripe_bytes(4, 0, (0_i16, 0_i16))),
        "sprites",
    )
    .await;
    let hanging = call(&client, "get_thing_sprite", json!({ "type": 63_u64 })).await;
    assert_eq!(
        hanging["name"],
        json!("Hanging Swaying Body"),
        "the gore row"
    );
    assert_eq!(hanging["hanging"], json!(true), "the flag survives");
    assert_eq!(hanging["lump"], json!("GOR1A1"), "the added custom lump");
    check_dims(&hanging, 4_u64, 2_u64);
}

#[tokio::test]
async fn get_thing_sprite_falls_back_to_the_all_angles_image() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let player = call(&client, "get_thing_sprite", json!({ "type": 1_u64 })).await;
    assert_eq!(player["name"], json!("Player 1 Start"), "the spawn row");
    assert_eq!(player["sprite"], json!("PLAY"), "the sprite column");
    assert_eq!(
        player["lump"],
        json!("PLAYA0"),
        "the all-angles image answers rotation 1"
    );
    assert_eq!(player["mirrored"], json!(false), "used as stored");
    check_dims(&player, 16_u64, 16_u64);
}

/// The `get_thing_sprite` refusals: unknown types, missing lumps and
/// malformed selections.
fn thing_sprite_cases() -> Vec<Refusal> {
    vec![
        (
            "get_thing_sprite",
            json!({ "type": 999_999_u64 }),
            "not_found",
            "no thing type 999999",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 3004_u64 }),
            "not_found",
            "POSSA1 / POSSA0",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 1_u64, "frame": "B" }),
            "not_found",
            "PLAYB1 / PLAYB0",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 3001_u64, "rotation": 0_u64 }),
            "invalid_argument",
            "rotation",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 3001_u64, "rotation": 9_u64 }),
            "invalid_argument",
            "rotation",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 3001_u64, "frame": "1" }),
            "invalid_argument",
            "frame",
        ),
        (
            "get_thing_sprite",
            json!({ "type": 3001_u64, "palette": 20_u64 }),
            "not_found",
            "PLAYPAL",
        ),
    ]
}

#[tokio::test]
async fn get_thing_sprite_reports_missing_definitions_lumps_and_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    refuse_all(&client, &thing_sprite_cases()).await;
    let fresh = connect().await;
    refuse(
        &fresh,
        "get_thing_sprite",
        json!({ "type": 3001_u64 }),
        ("not_found", "TROOA1 / TROOA0"),
    )
    .await;
}

#[tokio::test]
async fn get_thing_sprite_previews_a_decorate_defined_thing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad(dir.path());
    let extra = pwad_custom_defs(dir.path());
    call(&client, "open_iwad", path_arg(&base)).await;
    call(
        &client,
        "merge_wad",
        json!({ "path": extra.display().to_string() }),
    )
    .await;
    add_in(
        &client,
        "PLSMA1",
        &hex_of(&stripe_bytes(4, 0, (1_i16, 2_i16))),
        "sprites",
    )
    .await;
    let lamp = call(&client, "get_thing_sprite", json!({ "type": 20_010_u64 })).await;
    assert_eq!(lamp["name"], json!("PlasmaLamp"), "the class name");
    assert_eq!(lamp["sprite"], json!("PLSM"), "the spawn state's sprite");
    assert_eq!(lamp["frame"], json!("A"), "its frame letter");
    assert_eq!(lamp["lump"], json!("PLSMA1"), "the custom sprite lump");
    check_dims(&lamp, 4_u64, 2_u64);
    check_offsets(&lamp, 1_i64, 2_i64);
    let (code, message, _) =
        call_err(&client, "get_thing_sprite", json!({ "type": 20_013_u64 })).await;
    assert_eq!(code, "not_found", "the classless MAPINFO entry");
    assert!(
        message.contains("thing type 20013 has no sprite"),
        "the message says so: {message}"
    );
}
