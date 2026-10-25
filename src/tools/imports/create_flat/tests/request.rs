//! The call's arguments: the documented defaults and the name and square
//! refusals, the three sides and the past-vanilla warning, the warning
//! order, the `assign_role` rule and the opaque-hole count it keeps, and
//! the documented wire form.

use serde_json::json;

use crate::gfx::ops::FitMode;
use crate::gfx::ops::dither::Dither;
use crate::themes::Role;
use crate::tools::imports::create_flat::build::{
    flat_warnings, opaque_warning, vanilla_size_warning,
};
use crate::tools::imports::create_flat::{
    CREATE_FLAT_TOOL_DESCRIPTION, CreateFlatParams, parse_create_flat, parse_flat_role,
};

use super::{args, code, name};

#[test]
fn parse_applies_the_documented_defaults() {
    let request = parse_create_flat(&args()).expect("a source and name are enough");
    assert_eq!(request.name, name("MYFLAT"), "the name carries");
    assert_eq!(request.fit, FitMode::Cover, "cover by default");
    assert_eq!(request.size, 64_u32, "the vanilla square by default");
    assert!(request.tileable, "tileable is on unless passed false");
    assert!(request.role.is_none(), "no role asked for");
    assert!(!request.replace, "no silent overwrite");
    let mut lower = args();
    lower.name = "myflat".to_owned();
    assert_eq!(
        parse_create_flat(&lower).expect("lowercase").name,
        name("MYFLAT"),
        "the name uppercases"
    );
    let mut bad = args();
    bad.name = "way too long".to_owned();
    assert_eq!(
        code(&parse_create_flat(&bad).expect_err("no lump name")),
        "invalid_argument"
    );
}

#[test]
fn parse_takes_the_three_square_sides_and_refuses_else() {
    for side in [64_u32, 128, 256] {
        let mut call = args();
        call.size = Some(side);
        assert_eq!(
            parse_create_flat(&call).expect("a square side").size,
            side,
            "the size carries"
        );
    }
    let mut odd = args();
    odd.size = Some(96_u32);
    let error = parse_create_flat(&odd).expect_err("no 96 side");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("64, 128, 256"),
        "the message lists the sides: {error}"
    );
}

#[test]
fn only_a_side_past_the_vanilla_square_warns() {
    assert!(
        vanilla_size_warning(64_u32).is_none(),
        "the vanilla shape is quiet"
    );
    for side in [128_u32, 256] {
        let warning = vanilla_size_warning(side).expect("a past-vanilla side warns");
        assert!(
            warning.contains(&format!("{side}x{side}"))
                && warning.contains("vanilla Doom loads only 64x64 flats")
                && warning.contains("ZDoom"),
            "the warning names the shape, the limit and the way out: {warning}"
        );
    }
}

#[test]
fn the_warnings_carry_the_size_note_after_the_opaque_one() {
    assert!(
        flat_warnings(0_usize, 64_u32).is_empty(),
        "quiet and vanilla"
    );
    let both = flat_warnings(5_usize, 256_u32);
    assert_eq!(both.len(), 2_usize, "the hole count and the size note");
    assert!(
        both[0].contains("5 pixels") && both[1].contains("256x256"),
        "the opaque warning comes first: {both:?}"
    );
}

#[test]
fn the_opaque_warning_counts_kept_holes_only() {
    assert!(
        opaque_warning(0_usize).is_empty(),
        "an opaque flat is quiet"
    );
    let warnings = opaque_warning(12_usize);
    assert_eq!(warnings.len(), 1_usize, "one warning");
    assert!(
        warnings[0].contains("12 pixels") && warnings[0].contains("no transparency"),
        "the warning explains the keep: {}",
        warnings[0]
    );
}

#[test]
fn roles_must_parse_and_stay_on_the_flats() {
    assert!(parse_flat_role(None).expect("absent").is_none());
    assert_eq!(
        parse_flat_role(Some("LIGHT_CEILING")).expect("a flat role"),
        Some(Role::LightCeiling)
    );
    let error = parse_flat_role(Some("wall")).expect_err("a wall role");
    assert_eq!(code(&error), "invalid_argument");
    let message = error.to_string();
    assert!(
        message.contains("wall role"),
        "the message says why: {message}"
    );
    assert!(
        message.contains("create_texture_from_image"),
        "and names the way out: {message}"
    );
    assert_eq!(
        code(&parse_flat_role(Some("roof")).expect_err("no such role")),
        "invalid_argument",
        "an unknown role refuses too"
    );
}

#[test]
fn the_wire_form_flattens_the_shared_fields() {
    let call: CreateFlatParams = serde_json::from_value(json!({
        "source": "/x/y.png",
        "name": "MYFLAT",
        "fit": "contain",
        "size": 256_u64,
        "tileable": false,
        "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
        "dither": "ordered",
        "palette": 2_u64,
        "assign_role": "ceiling",
        "replace": true,
    }))
    .expect("the documented wire form");
    assert_eq!(call.fit, Some(FitMode::Contain));
    assert_eq!(call.size, Some(256_u32), "the square side carries");
    assert!(!call.tileable, "the flag carries");
    assert_eq!(call.shared.ops.len(), 1_usize, "the op list carries");
    assert_eq!(call.shared.dither, Some(Dither::Ordered));
    assert_eq!(call.shared.palette, Some(2_usize));
    assert_eq!(call.assign_role.as_deref(), Some("ceiling"));
    assert!(call.replace);
    assert!(
        CREATE_FLAT_TOOL_DESCRIPTION.contains("ONE undoable edit"),
        "the description promises one undoable edit"
    );
}
