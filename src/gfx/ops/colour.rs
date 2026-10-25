//! The colour side of the operation framework.
//!
//! The `#RRGGBBAA` hex form every colour-taking parameter parses through,
//! the integer [`luminance`] the binarizing and tonal ops measure with, and
//! the channel clamp they write back through.

use crate::error::CyberError;

/// Parses a `#RRGGBBAA` or `#RRGGBB` colour string (the leading `#` is
/// optional) into `RGBA` bytes; a six-digit colour is opaque.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `value` is not one of the two hex
/// forms.
#[inline]
pub fn parse_hex_color(value: &str) -> Result<[u8; 4], CyberError> {
    let malformed = || {
        CyberError::InvalidArgument(format!(
            "colour {value} is not a #RRGGBBAA or #RRGGBB hex value"
        ))
    };
    let digits = value.strip_prefix('#').unwrap_or(value);
    let alpha_explicit = match digits.len() {
        6 => false,
        8 => true,
        _ => return Err(malformed()),
    };
    let mut channels = [0_u8; 4];
    for (slot, pair) in channels.iter_mut().zip(digits.as_bytes().chunks_exact(2)) {
        *slot = hex_pair(pair).ok_or_else(malformed)?;
    }
    let [red, green, blue, alpha] = channels;
    if alpha_explicit {
        Ok([red, green, blue, alpha])
    } else {
        Ok([red, green, blue, 255_u8])
    }
}

/// Parses exactly two `ASCII` hex bytes into one channel value.
fn hex_pair(pair: &[u8]) -> Option<u8> {
    let text = core::str::from_utf8(pair).ok()?;
    u8::from_str_radix(text, 16).ok()
}

/// Integer luminance helper: `(299 * R + 587 * G + 114 * B) / 1000`.
#[must_use]
#[inline]
pub fn luminance(pixel: [u8; 4]) -> u8 {
    let [red, green, blue, _] = pixel;
    let weighted = u32::from(red)
        .saturating_mul(299)
        .saturating_add(u32::from(green).saturating_mul(587))
        .saturating_add(u32::from(blue).saturating_mul(114));
    let scaled = weighted.checked_div(1_000_u32).unwrap_or(0_u32);
    u8::try_from(scaled).unwrap_or(255_u8)
}

/// Clamps a signed channel value into `0..=255`.
pub(crate) fn clamp_u8(value: i32) -> u8 {
    if value < 0 {
        0_u8
    } else {
        u8::try_from(value).unwrap_or(255_u8)
    }
}
