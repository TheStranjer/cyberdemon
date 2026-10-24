//! The transparent pass of `render_view` and the last of its parameter
//! checks: the imp sprite standing near the view centre, `show_sprites` off
//! dropping it and leaving the wall it hid, a wall between camera and thing
//! depth-testing every sprite out, an absent sprite lump reported once per
//! sprite name, and the masked-column count staying independent of the sprite
//! switch. The stock `IWAD` fixture's reads: the eight compass views of an
//! indoor ring reporting nothing, a sky found in the `TEXTURE1` definitions
//! rather than a lump, and a genuinely missing sprite costing exactly one
//! line. A camera outside the map, an out-of-range camera argument and a
//! session without a `PLAYPAL` are refused. The fixture camera comes from
//! [`crate::render::view`].

use std::fs;
use std::path::Path;

use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::wad::Wad;
use serde_json::{Value, json};

use crate::fixtures::{iwad, iwad_stock};
use crate::render::view::{fullbright, sky_room_client, view_args};
use crate::render::{Client, count_pixels, iwad_client, path_arg, rendered_image};
use crate::support::{call, call_err, connect};

/// Palette index 50 of the fixture `PLAYPAL`: the colour of every
/// texel of the view fixture's `TROOA1` under full bright.
const IMP_GREEN: [u8; 4] = [50_u8, 205, 50, 255];

/// Refuses one `render_view` camera argument and requires the message
/// to show `wanted`.
async fn view_refused(client: &Client, field: &str, value: Value, wanted: &str) {
    let mut args = view_args(&Value::Null);
    args[field] = value;
    let (code, message, _) = call_err(client, "render_view", args).await;
    assert_eq!(code, "invalid_argument", "{field} out of range");
    assert!(
        message.contains(wanted),
        "the {field} refusal shows {wanted}: {message}"
    );
}

/// The `masked_columns` count of a `render_view` report.
fn masked_of(report: &Value) -> u64 {
    report["masked_columns"]
        .as_u64()
        .expect("masked_columns is a number")
}

/// A fullbright client on the `DOOM II`-shaped fixture `IWAD`.
async fn stock_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_stock(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    fullbright(&client).await;
    client
}

/// The stock fixture's hall camera: the middle of its `512x512` room,
/// facing `angle` degrees, over `map`.
fn hall_view(map: &str, angle: i64) -> Value {
    json!({
        "map": map, "x": 256_i64, "y": 256_i64, "angle": angle,
        "width": 160_u64, "height": 100_u64,
    })
}

/// Every compass view from the middle of a stock-fixture hall is quiet:
/// one of the eight ringed imps stands at each of the engine's rotation
/// buckets, and each bucket resolves against the fixture's pair-named
/// lumps, so nothing is reported and the imp dead ahead paints.
async fn every_ring_angle_is_quiet(client: &Client, map: &str) {
    for angle in [0_i64, 45, 90, 135, 180, 225, 270, 315] {
        let report = call(client, "render_view", hall_view(map, angle)).await;
        assert_eq!(
            report["problems"],
            json!([]),
            "the {angle}-degree view of the indoor ring reports nothing"
        );
        let drawn = report["sprites_drawn"]
            .as_u64()
            .expect("sprites_drawn is a number");
        assert!(
            drawn >= 1,
            "the imp straight ahead paints at {angle} degrees: {drawn}"
        );
    }
}

/// Asserts the frame top of a stock-fixture sky-room view reads the
/// `RSKY1` patch's colour: the hall's whole ceiling is `F_SKY1` and its
/// sky rows start at the frame top, well above the horizon.
fn assert_stock_sky_band(image: &RgbaImage) {
    for column in [40_u32, 80_u32, 120_u32] {
        assert_eq!(
            image.get(column, 0).expect("a pixel"),
            [200_u8, 55, 200, 255],
            "the F_SKY1 ceiling draws the RSKY1 patch at column {column}"
        );
    }
}

#[tokio::test]
async fn render_view_draws_the_imp_sprite_near_the_view_centre() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    let drawn = report["sprites_drawn"]
        .as_u64()
        .expect("sprites_drawn is a number");
    assert!(drawn >= 1, "the imp's sprite painted: {drawn}");
    assert_eq!(
        report["masked_columns"],
        json!(0_u64),
        "a sprite paints no masked column: MAP01 masks nothing"
    );
    let image = rendered_image(&report).await;
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        IMP_GREEN,
        "the sprite stands in front of the far wall, on the horizon row"
    );
    assert_eq!(
        image.get(80, 52).expect("a pixel"),
        IMP_GREEN,
        "and below the horizon, where it hid the hall floor"
    );
    let green = count_pixels(&image, IMP_GREEN);
    assert!(
        green >= 4,
        "the patch covers a small block at the centre: {green}"
    );
}

#[tokio::test]
async fn show_sprites_off_drops_the_imp_from_the_view() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    call(
        &client,
        "set_render_options",
        json!({ "show_sprites": false }),
    )
    .await;
    let report = call(&client, "render_view", view_args(&Value::Null)).await;
    assert_eq!(
        report["sprites_drawn"],
        json!(0_u64),
        "the switch draws no sprite at all"
    );
    let image = rendered_image(&report).await;
    assert_eq!(
        count_pixels(&image, IMP_GREEN),
        0,
        "no sprite texel lands anywhere"
    );
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        [108_u8, 147, 108, 255],
        "the far wall keeps the centre column where the sprite stood"
    );
}

#[tokio::test]
async fn a_wall_between_camera_and_thing_hides_the_sprite_behind_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let report = call(
        &client,
        "render_view",
        view_args(&json!({ "map": "MAP03" })),
    )
    .await;
    assert_eq!(
        report["sprites_drawn"],
        json!(0_u64),
        "the sealed hall depth-tests every sprite out"
    );
    let image = rendered_image(&report).await;
    assert_eq!(
        count_pixels(&image, IMP_GREEN),
        0,
        "the imp paints no pixel from behind the wall"
    );
    assert_eq!(
        image.get(80, 50).expect("a pixel"),
        [108_u8, 147, 108, 255],
        "the WALL1 face that seals the hall paints the centre column it hides behind"
    );
}

#[tokio::test]
async fn absent_sprite_lumps_are_reported_once_per_sprite_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let report = call(
        &client,
        "render_view",
        view_args(&json!({ "map": "MAP03" })),
    )
    .await;
    let problems = report["problems"].as_array().expect("problems is a list");
    let sprite_lines: Vec<&str> = problems
        .iter()
        .filter_map(Value::as_str)
        .filter(|line| line.ends_with("POSS") || line.ends_with("TROO"))
        .collect();
    assert_eq!(
        sprite_lines,
        ["missing sprite POSS"],
        "the two POSS things report one line and the present TROO none: {problems:?}"
    );
}

/// The stock fixture stores the sky as a `TEXTURE1` record and the
/// `TROO` frame over the mirror-pair names a real `IWAD` writes, so an
/// indoor hall ringed by eight imps — one per engine rotation — has
/// nothing to report from any of the eight compass views.
#[tokio::test]
async fn render_view_reports_nothing_for_a_stock_iwads_indoor_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = stock_client(dir.path()).await;
    every_ring_angle_is_quiet(&client, "MAP01").await;
}

/// The stock fixture has no lump named `SKY1`: its sky is a texture
/// definition, which is where the check has to look once a map actually
/// shows a sky plane.
#[tokio::test]
async fn render_view_draws_a_sky_it_found_in_the_texture_definitions() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = stock_client(dir.path()).await;
    let report = call(&client, "render_view", hall_view("MAP02", 0)).await;
    assert_eq!(
        report["problems"],
        json!([]),
        "the SKY1 record is no missing sky"
    );
    let image = rendered_image(&report).await;
    assert_stock_sky_band(&image);
}

/// The other half of the same fix: a sprite the session really does not
/// carry still costs exactly one line, and nothing else in the frame of
/// the stock-fixture hall does.
#[tokio::test]
async fn render_view_reports_a_genuinely_missing_sprite_once() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = stock_client(dir.path()).await;
    let report = call(&client, "render_view", hall_view("MAP03", 0)).await;
    assert_eq!(
        report["problems"],
        json!(["missing sprite POSS"]),
        "the two POSS things are one line and the ring of imps is no line at all"
    );
    let drawn = report["sprites_drawn"]
        .as_u64()
        .expect("sprites_drawn is a number");
    assert!(
        drawn >= 1,
        "the imps the session has dress and draw: {drawn}"
    );
}

#[tokio::test]
async fn masked_columns_stay_independent_of_the_sprite_switch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = sky_room_client(dir.path()).await;
    let on = call(
        &client,
        "render_view",
        view_args(&json!({ "map": "MAP02" })),
    )
    .await;
    call(
        &client,
        "set_render_options",
        json!({ "show_sprites": false }),
    )
    .await;
    let off = call(
        &client,
        "render_view",
        view_args(&json!({ "map": "MAP02" })),
    )
    .await;
    assert!(masked_of(&on) >= 1, "the grate masks columns");
    assert_eq!(
        masked_of(&on),
        masked_of(&off),
        "the sprite switch moves no masked column"
    );
    assert_eq!(
        on["sprites_drawn"],
        json!(0_u64),
        "MAP02's only thing stands on the camera and draws nothing"
    );
}

#[tokio::test]
async fn render_view_refuses_a_camera_outside_the_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "render_view",
        json!({ "map": "MAP01", "x": -100_i64, "y": -100_i64 }),
    )
    .await;
    assert_eq!(code, "invalid_argument", "no sector holds the camera");
    assert!(
        message.contains("outside"),
        "the message says the camera is outside: {message}"
    );
}

#[tokio::test]
async fn render_view_refuses_out_of_range_camera_arguments() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_client(dir.path()).await;
    view_refused(&client, "pitch", json!(90_i64), "-60..=60").await;
    view_refused(&client, "fov", json!(200_u64), "10..=170").await;
    view_refused(&client, "width", json!(8_u64), "16..=4096").await;
    view_refused(&client, "lighting", json!("cinematic"), "\"fullbright\"").await;
}

#[tokio::test]
async fn render_view_without_a_playpal_is_not_found() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad(dir.path());
    let bytes = fs::read(&base).expect("the fixture WAD is readable");
    let mut wad = Wad::parse(&bytes).expect("the fixture WAD parses");
    wad.lumps.retain(|lump| lump.name.to_string() != "PLAYPAL");
    let stripped = dir.path().join("no-playpal.wad");
    let out = wad.to_bytes().expect("the stripped archive serializes");
    fs::write(&stripped, out).expect("the stripped archive writes");
    call(&client, "open_iwad", path_arg(&stripped)).await;
    let (code, message, _) = call_err(
        &client,
        "render_view",
        json!({ "map": "MAP01", "x": 64_i64, "y": 128_i64 }),
    )
    .await;
    assert_eq!(code, "not_found", "a view needs a palette");
    assert!(
        message.contains("PLAYPAL"),
        "the refusal names the missing lump: {message}"
    );
}
