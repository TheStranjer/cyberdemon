//! The colour side of the operation framework.
//!
//! The crate's one strict hex parser, [`parse_hex_colour`], over the one
//! digit decoder [`hex_digit`] every hex reader shares, the integer
//! [`luminance`] the binarizing and tonal ops measure with, and the channel
//! clamp they write back through.

use crate::error::CyberError;
use crate::gfx::Rgba;

/// Hex digits of an `RRGGBB` colour.
const RGB_DIGITS: usize = 6;
/// Hex digits of an `RRGGBBAA` colour.
const RGBA_DIGITS: usize = 8;
/// Radix of one hex digit's place value in a pair.
const DIGIT_SHIFT: u8 = 16;

/// The value of one `ASCII` hex digit, `None` for any other byte.
///
/// The one digit decoder every hex reader in the crate shares: percent
/// escapes, byte-string needles and [`parse_hex_colour`]' channels alike.
#[must_use]
#[inline]
pub fn hex_digit(byte: u8) -> Option<u8> {
    if byte.is_ascii_digit() {
        byte.checked_sub(b'0')
    } else if (b'a'..=b'f').contains(&byte) {
        byte.checked_sub(b'a')?.checked_add(10_u8)
    } else if (b'A'..=b'F').contains(&byte) {
        byte.checked_sub(b'A')?.checked_add(10_u8)
    } else {
        None
    }
}

/// Parses a strict hex colour: digits only, an optional leading `#`, and
/// exactly six hex digits, or eight when `allow_alpha`.
///
/// A six-digit colour is opaque; anything else - signs, underscores, odd
/// lengths - is refused.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `text` is not one of the accepted
/// hex forms.
#[inline]
pub fn parse_hex_colour(text: &str, allow_alpha: bool) -> Result<Rgba, CyberError> {
    let shapes = if allow_alpha {
        "#RRGGBBAA or #RRGGBB"
    } else {
        "#RRGGBB"
    };
    let malformed = || {
        CyberError::InvalidArgument(format!(
            "colour {text:?} is not a {shapes} hex value: digits only, optional leading #"
        ))
    };
    let digits = text.strip_prefix('#').unwrap_or(text);
    let channels = match digits.len() {
        RGB_DIGITS => 3_usize,
        RGBA_DIGITS if allow_alpha => 4_usize,
        _ => return Err(malformed()),
    };
    let mut pixel = [0_u8, 0, 0, 255_u8];
    for (slot, pair) in pixel
        .iter_mut()
        .take(channels)
        .zip(digits.as_bytes().chunks_exact(2))
    {
        *slot = hex_pair(pair).ok_or_else(malformed)?;
    }
    Ok(pixel)
}

/// The channel two `ASCII` hex bytes spell, `None` unless both are digits.
fn hex_pair(pair: &[u8]) -> Option<u8> {
    let &[high, low] = pair else {
        return None;
    };
    hex_digit(high)?
        .checked_mul(DIGIT_SHIFT)?
        .checked_add(hex_digit(low)?)
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
