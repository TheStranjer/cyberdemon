//! The size side of one call: the widths a wall texture may carry and the
//! two the refusal names, the heights it accepts, the rounding step an
//! aspect-derived edge lands on, the rule that keeps a tileable picture
//! and falls back otherwise, the up-front order of the argument checks,
//! the patch name that follows the texture name, the wall roles
//! `assign_role` accepts and the documented wire form.

use serde_json::json;

use crate::gfx::ops::FitMode;
use crate::themes::Role;
use crate::tools::imports::texture::size::{
    check_height, check_width, choose_size, keep_or_fallback, nearest_width, nearest_widths,
    round_to_step,
};
use crate::tools::imports::texture::{
    CreateTextureParams, MAX_TEXTURE_EDGE, parse_assign_role, parse_create,
};

use super::{args, assert_chosen, code, name, picture, sized};

#[test]
fn widths_take_the_powers_of_two_and_name_the_nearest() {
    for width in [16_u32, 32, 64, 128, 256, 512, 1024] {
        assert_eq!(check_width(width).expect("a valid width"), width);
    }
    let error = check_width(100_u32).expect_err("no 100");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("64 or 128"),
        "the message names the neighbours: {error}"
    );
    assert_eq!(nearest_widths(2048_u32), "1024", "above the top end");
    assert_eq!(nearest_widths(8_u32), "16", "below the bottom end");
    assert_eq!(nearest_width(96_u32), 64_u32, "ties take the smaller");
    assert_eq!(nearest_width(100_u32), 128_u32, "closer above wins");
}

#[test]
fn heights_take_the_i16_comfortable_range() {
    assert_eq!(check_height(1_u32).expect("one row"), 1_u32);
    assert_eq!(check_height(MAX_TEXTURE_EDGE).expect("the edge"), 1024_u32);
    assert_eq!(
        code(&check_height(0_u32).expect_err("zero")),
        "invalid_argument"
    );
    let error = check_height(1025_u32).expect_err("past the edge");
    assert!(error.to_string().contains("1..=1024"), "the range: {error}");
}

#[test]
fn round_to_step_neighbours_the_multiple() {
    assert_eq!(round_to_step(20_u32), 24_u32, "the tie rounds up");
    assert_eq!(round_to_step(24_u32), 24_u32, "an exact step stays");
    assert_eq!(
        round_to_step(3_u32),
        0_u32,
        "zero is the nearest multiple of 3"
    );
    assert_eq!(round_to_step(19_u32), 16_u32, "19 sits nearer 16");
}

#[test]
fn the_size_rule_keeps_tileable_pictures_and_falls_back_otherwise() {
    let request = sized(None, None);
    let kept = choose_size(&request, &picture(64_u32, 128_u32));
    assert_eq!(
        (kept.width, kept.height),
        (64_u32, 128_u32),
        "a tileable size stays"
    );
    assert!(kept.note.is_none(), "no note for a kept size");
    let fallback = choose_size(&request, &picture(100_u32, 60_u32));
    assert_eq!(
        (fallback.width, fallback.height),
        (128_u32, 128_u32),
        "the square fallback"
    );
    assert_eq!(
        fallback.note.as_deref(),
        Some("resized to 128x128; pass width/height to choose"),
        "the documented note"
    );
    let too_tall = choose_size(&request, &picture(64_u32, 1025_u32));
    assert_eq!(
        (too_tall.width, too_tall.height),
        (128_u32, 128_u32),
        "height caps out"
    );
    let tiny = keep_or_fallback(&picture(8_u32, 8_u32));
    assert_eq!(
        (tiny.width, tiny.height),
        (128_u32, 128_u32),
        "under 16 falls back"
    );
}

#[test]
fn a_missing_edge_follows_the_aspect_ratio() {
    assert_chosen(
        &sized(Some(128_u32), None),
        &picture(64_u32, 128_u32),
        (128_u32, 256_u32),
    );
    assert_chosen(
        &sized(Some(128_u32), None),
        &picture(100_u32, 60_u32),
        (128_u32, 80_u32),
    );
    assert_chosen(
        &sized(None, Some(256_u32)),
        &picture(64_u32, 128_u32),
        (128_u32, 256_u32),
    );
    assert_chosen(
        &sized(Some(1024_u32), None),
        &picture(8192_u32, 1_u32),
        (1024_u32, 8_u32),
    );
}

#[test]
fn explicit_edges_are_validated_before_anything_else() {
    let mut call = args(Some(100_u32), Some(0_u32));
    call.patch_name = Some("too long for wad".to_owned());
    assert_eq!(
        code(&parse_create(&call).expect_err("bad patch name")),
        "invalid_argument"
    );
    call.patch_name = None;
    let error = parse_create(&call).expect_err("bad width");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("width 100"),
        "names the field: {error}"
    );
    call.size.width = Some(64_u32);
    assert_eq!(
        code(&parse_create(&call).expect_err("bad height")),
        "invalid_argument"
    );
}

#[test]
fn the_patch_name_defaults_to_the_texture_name_uppercased() {
    let mut call = args(None, None);
    call.name = "mytex".to_owned();
    let request = parse_create(&call).expect("valid");
    assert_eq!(request.name, "MYTEX", "the texture name uppercases");
    assert_eq!(request.patch, name("MYTEX"), "the patch follows");
    call.patch_name = Some("mypatch".to_owned());
    assert_eq!(parse_create(&call).expect("valid").patch, name("MYPATCH"));
}

#[test]
fn roles_must_parse_and_stay_on_the_walls() {
    assert!(parse_assign_role(None).expect("absent").is_none());
    assert_eq!(
        parse_assign_role(Some("WALL_ALT")).expect("a wall role"),
        Some(Role::WallAlt)
    );
    let error = parse_assign_role(Some("floor")).expect_err("a flat role");
    assert_eq!(code(&error), "invalid_argument");
    let message = error.to_string();
    assert!(message.contains("flat"), "the message says why: {message}");
    assert!(
        message.contains("import_image_as_flat"),
        "and names the way out: {message}"
    );
    assert_eq!(
        code(&parse_assign_role(Some("roof")).expect_err("no such role")),
        "invalid_argument",
        "an unknown role refuses too"
    );
}

#[test]
fn the_wire_form_flattens_ops_size_and_shared_fields() {
    let call: CreateTextureParams = serde_json::from_value(json!({
        "source": "/x/y.png",
        "name": "MYTEX",
        "width": 256_u64,
        "fit": "stretch",
        "tileable": true,
        "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
        "dither": "ordered",
        "palette": 2_u64,
        "assign_role": "wall",
        "replace": true,
    }))
    .expect("the documented wire form");
    assert_eq!(call.size.width, Some(256_u32));
    assert_eq!(call.size.fit, Some(FitMode::Stretch));
    assert!(call.size.tileable);
    assert_eq!(call.shared.ops.len(), 1, "the op list carries");
    assert_eq!(call.shared.palette, Some(2_usize));
    assert!(call.replace);
}
