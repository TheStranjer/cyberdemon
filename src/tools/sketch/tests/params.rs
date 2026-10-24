//! The wire half: the sizes, ink modes, units and row caps `parse_request`
//! resolves, the crop rectangle `check_crop` refuses, and where the asked
//! pixel parameters land on the analysis raster.

use crate::sketch::InkMode;
use crate::tools::sketch::request::{
    analyze_options, check_crop, parse_cap, parse_ink, parse_max_size, parse_request, parse_units,
    vectorize_options,
};
use crate::tools::sketch::{CropParams, InkParams};

use super::{args, frame, paper};

#[test]
fn max_size_defaults_to_1024_and_refuses_the_out_of_range() {
    assert_eq!(parse_max_size(None).expect("default"), 1_024_u32);
    assert_eq!(parse_max_size(Some(256)).expect("floor"), 256_u32);
    assert_eq!(parse_max_size(Some(4_096)).expect("ceiling"), 4_096_u32);
    for bad in [0_u32, 64_u32, 4_097_u32] {
        let error = parse_max_size(Some(bad)).expect_err("out of range");
        assert_eq!(error.code(), "invalid_argument", "code for {bad}");
        assert!(
            error.to_string().contains("between 256 and 4096"),
            "the message names the range: {error}"
        );
    }
}

#[test]
fn ink_defaults_to_dark_and_color_needs_a_pen() {
    assert_eq!(
        parse_ink(None).expect("absent"),
        InkMode::Dark { level: None },
        "unset asks for the detected level"
    );
    let dark = InkParams {
        mode: Some("Dark".to_owned()),
        level: Some(128_u8),
        ..InkParams::default()
    };
    assert_eq!(
        parse_ink(Some(&dark)).expect("explicit"),
        InkMode::Dark { level: Some(128) },
        "the level rides through, case-insensitively"
    );
    let color = InkParams {
        mode: Some("color".to_owned()),
        color: Some("#C02020".to_owned()),
        ..InkParams::default()
    };
    assert_eq!(
        parse_ink(Some(&color)).expect("a pen"),
        InkMode::Color {
            rgb: [192, 32, 32],
            tolerance: 48,
        },
        "the default tolerance is 48"
    );
    let bare = InkParams {
        mode: Some("color".to_owned()),
        ..InkParams::default()
    };
    let error = parse_ink(Some(&bare)).expect_err("no colour given");
    assert_eq!(error.code(), "invalid_argument", "color needs color");
    assert!(
        error.to_string().contains("ink.color"),
        "the message names the field: {error}"
    );
    let odd = InkParams {
        mode: Some("pencil".to_owned()),
        ..InkParams::default()
    };
    assert_eq!(
        parse_ink(Some(&odd)).expect_err("not a mode").code(),
        "invalid_argument",
        "unknown modes are refused"
    );
}

#[test]
fn units_stay_none_until_the_wire_gives_them() {
    assert!(parse_units(None, 8_i64).expect("absent").is_none());
    let wire = crate::tools::plan::params::UnitsParam {
        scale_num: 4_i64,
        ..crate::tools::plan::params::UnitsParam::default()
    };
    let given = parse_units(Some(wire), 16_i64)
        .expect("a wire object")
        .expect("Some");
    assert_eq!(given.scale_num, 4_i64, "the scale travels");
    assert_eq!(given.grid, 16_i64, "the grid rides inside the units");
    let error = parse_units(None, 0_i64).expect_err("zero grid");
    assert_eq!(error.code(), "invalid_argument", "grid 0 is refused");
}

#[test]
fn crop_edges_outside_the_image_are_refused_naming_the_field() {
    let img = paper(100_u32, 80_u32);
    let good = CropParams {
        x: 10_u32,
        y: 10_u32,
        width: 80_u32,
        height: 60_u32,
    };
    check_crop(good, &img).expect("inside");
    let wide = CropParams {
        width: 95_u32,
        ..good
    };
    let error = check_crop(wide, &img).expect_err("past the right edge");
    assert_eq!(error.code(), "invalid_argument");
    assert!(error.to_string().contains("crop.x"), "{error}");
    let zero = CropParams {
        height: 0_u32,
        ..good
    };
    let empty = check_crop(zero, &img).expect_err("empty");
    assert!(empty.to_string().contains("crop.height"), "{empty}");
}

#[test]
fn explicit_parameters_land_on_the_analysis_raster() {
    let mut base = args("/x.png");
    base.close_gaps = Some(96_u32);
    base.min_region_area = Some(1_600_u32);
    base.mark_max_size = Some(40_u32);
    base.simplify = Some(8_u32);
    base.weld = Some(0_u32);
    base.orthogonal = false;
    let request = parse_request(&base).expect("the wire parses");
    let shrunk = frame(1_u32, 4_u32);
    let options = analyze_options(&request, &shrunk);
    assert_eq!(options.close_radius, Some(24_u32));
    assert_eq!(options.min_region_area, Some(100_u32));
    assert_eq!(options.mark_max_size, Some(10_u32));
    let voptions = vectorize_options(&request, &shrunk);
    assert_eq!(voptions.simplify_px, Some(2_u32));
    assert_eq!(voptions.weld_px, Some(0_u32), "weld 0 keeps its meaning");
    assert!(!voptions.orthogonal, "the flag rides through");
    assert!(
        voptions.expand,
        "regions always grow to the wall centrelines"
    );
}

#[test]
fn caps_default_to_fifty_and_refuse_the_out_of_range() {
    assert_eq!(parse_cap("max_gaps", None).expect("default"), 50_usize);
    assert_eq!(
        parse_cap("max_marks", Some(1_usize)).expect("floor"),
        1_usize
    );
    assert_eq!(
        parse_cap("max_gaps", Some(500_usize)).expect("ceiling"),
        500_usize
    );
    for bad in [0_usize, 501_usize] {
        let error = parse_cap("max_marks", Some(bad)).expect_err("out of range");
        assert_eq!(error.code(), "invalid_argument", "code for {bad}");
        assert!(
            error.to_string().contains("max_marks") && error.to_string().contains("verbose"),
            "the message names the parameter and the way out: {error}"
        );
    }
}
