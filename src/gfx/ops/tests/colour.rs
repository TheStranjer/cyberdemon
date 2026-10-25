//! The colour and integer helpers: the two hex forms with their optional
//! `#` and their refusals, the integer luminance formula and the rounding
//! and clamping of the shared division helpers.

use crate::gfx::ops::{clamp_u8, div_ceil, luminance, parse_hex_color, rounded_div};

#[test]
fn hex_color_parses_both_forms_and_optional_hash() {
    assert_eq!(
        parse_hex_color("#ff0000").expect("#RRGGBB parses"),
        [255, 0, 0, 255],
        "six digits get alpha 255"
    );
    assert_eq!(
        parse_hex_color("00ff0080").expect("RRGGBBAA parses"),
        [0, 255, 0, 128],
        "eight digits carry alpha"
    );
    assert_eq!(
        parse_hex_color("#FF0000").expect("uppercase parses"),
        [255, 0, 0, 255],
        "digits are case-insensitive"
    );
}

#[test]
fn hex_color_rejects_bad_input() {
    for bad in ["", "xyz", "12345", "#ff00", "gg0000", "#ff0000ff00"] {
        let error = parse_hex_color(bad).expect_err("must not parse");
        assert_eq!(error.code(), "invalid_argument", "code for {bad}");
    }
}

#[test]
fn luminance_matches_the_integer_formula() {
    assert_eq!(luminance([0, 0, 0, 255]), 0, "black");
    assert_eq!(luminance([255, 255, 255, 255]), 255, "white");
    assert_eq!(luminance([255, 0, 0, 255]), 76, "red: 299*255/1000");
    assert_eq!(luminance([0, 255, 0, 255]), 149, "green: 587*255/1000");
    assert_eq!(luminance([0, 0, 255, 255]), 29, "blue: 114*255/1000");
    assert_eq!(luminance([128, 128, 128, 0]), 128, "grey is alpha-free");
}

#[test]
fn integer_helpers_round_and_clamp_as_documented() {
    assert_eq!(rounded_div(7, 2), 4, "rounds up on halves");
    assert_eq!(rounded_div(0, 0), 0, "zero denominator is zero");
    assert_eq!(div_ceil(7, 2), 4, "ceil on remainders");
    assert_eq!(div_ceil(6, 2), 3, "exact division");
    assert_eq!(div_ceil(1, 0), u64::MAX, "zero denominator saturates");
    assert_eq!(clamp_u8(-5), 0, "below black");
    assert_eq!(clamp_u8(300), 255, "above white");
    assert_eq!(clamp_u8(10), 10, "in range");
}
