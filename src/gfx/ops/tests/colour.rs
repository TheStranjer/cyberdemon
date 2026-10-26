//! The colour and integer helpers: the one strict hex parser with its two
//! shapes, its `#`-optional exact lengths and its refusals, the shared digit
//! decoder, the integer luminance formula and the rounding and clamping of
//! the shared division helpers.

use crate::gfx::ops::{clamp_u8, div_ceil, hex_digit, luminance, parse_hex_colour, rounded_div};

#[test]
fn hex_colour_parses_both_forms_and_optional_hash() {
    assert_eq!(
        parse_hex_colour("#ff0000", true).expect("#RRGGBB parses"),
        [255, 0, 0, 255],
        "six digits get alpha 255"
    );
    assert_eq!(
        parse_hex_colour("00ff0080", true).expect("RRGGBBAA parses"),
        [0, 255, 0, 128],
        "eight digits carry alpha"
    );
    assert_eq!(
        parse_hex_colour("#FF0000", true).expect("uppercase parses"),
        [255, 0, 0, 255],
        "digits are case-insensitive"
    );
    assert_eq!(
        parse_hex_colour("aBcDeF01", true).expect("mixed case parses"),
        [171, 205, 239, 1],
        "mixed case carries every channel"
    );
}

#[test]
fn hex_colour_without_alpha_keeps_six_digits_only() {
    assert_eq!(
        parse_hex_colour("#0a1b2c", false).expect("#RRGGBB parses"),
        [10, 27, 44, 255],
        "the alpha byte is opaque"
    );
    let error = parse_hex_colour("0a1b2c3d", false).expect_err("eight digits are refused");
    assert_eq!(
        error.code(),
        "invalid_argument",
        "no alpha without the flag"
    );
}

#[test]
fn hex_colour_rejects_bad_input() {
    for bad in [
        "",
        "xyz",
        "fff",
        "12345",
        "#1234567",
        "#ff00",
        "gg0000",
        "#ff0000ff00",
        "+f+f+f",
        "#+f+f+f",
        "1f_2e_30",
        " 0a0b0c",
        "#ff0000 ",
    ] {
        let error = parse_hex_colour(bad, true).expect_err("must not parse");
        assert_eq!(error.code(), "invalid_argument", "code for {bad:?}");
        assert!(
            error.to_string().contains("#RRGGBB"),
            "the refusal for {bad:?} shows the shape: {error}"
        );
    }
}

#[test]
fn hex_digit_decodes_only_ascii_hex_digits() {
    assert_eq!(hex_digit(b'0'), Some(0), "bottom of the digits");
    assert_eq!(hex_digit(b'9'), Some(9), "top of the digits");
    assert_eq!(hex_digit(b'a'), Some(10), "lowercase a");
    assert_eq!(hex_digit(b'f'), Some(15), "lowercase f");
    assert_eq!(hex_digit(b'A'), Some(10), "uppercase A");
    assert_eq!(hex_digit(b'F'), Some(15), "uppercase F");
    assert_eq!(hex_digit(b'+'), None, "a sign is not a digit");
    assert_eq!(hex_digit(b'_'), None, "an underscore is not a digit");
    assert_eq!(hex_digit(b'g'), None, "past the letters");
    assert_eq!(hex_digit(b' '), None, "a space is not a digit");
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
