//! Decimal literal arithmetic for [`crate::wad::udmf`]: the float grammar's
//! shape and the digit-only rounding behind [`crate::wad::udmf::decimal_to_rounded_int`].
//!
//! A `UDMF` float is stored as the text it was written in, so converting one
//! to an integer never touches binary floating point: [`split_decimal`] takes
//! `[+-]digits[.digits][e[+-]digits]` apart, [`normalize`] strips the leading
//! zeroes and folds the point and exponent into one power-of-ten shift, and
//! [`rounded_magnitude`] scales the digit text by that shift, rounding half
//! away from zero and refusing past an `i64` magnitude. [`is_float_literal`]
//! answers the narrower question the grammar asks: whether a bare word is a
//! float at all.

use crate::error::CyberError;
use crate::wad::udmf::MAX_I64_MAGNITUDE_DIGITS;
use crate::wad::udmf::parser::accumulate;

/// Whether `word` is the float grammar: `[+-]?digits '.' digits?` with an
/// optional exponent.
pub(super) fn is_float_literal(word: &str) -> bool {
    let Some((head, tail)) = drop_sign(word).split_once('.') else {
        return false;
    };
    let fraction = exponent_digits(tail).unwrap_or(tail);
    !head.is_empty() && all_digits(head) && all_digits(fraction) && exponent_is_valid(tail)
}

/// The digits before the exponent marker of a float's tail.
fn exponent_digits(tail: &str) -> Option<&str> {
    tail.split_once(['e', 'E']).map(|(head, _)| head)
}

/// Whether whatever follows an exponent marker is a signed digit run.
fn exponent_is_valid(tail: &str) -> bool {
    match tail.split_once(['e', 'E']) {
        Some((_head, exponent)) => is_digit_run(drop_sign(exponent)),
        None => true,
    }
}

/// Whether `text` is a non-empty run of decimal digits.
fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && all_digits(text)
}

/// Whether every character of `text` is a decimal digit.
fn all_digits(text: &str) -> bool {
    text.chars().all(|ch| ch.is_ascii_digit())
}

/// Splits a leading sign from `text`, reporting a `'-'` first.
pub(super) fn take_sign(text: &str) -> (bool, &str) {
    if let Some(body) = text.strip_prefix('-') {
        return (true, body);
    }
    (false, text.strip_prefix('+').unwrap_or(text))
}

/// `text` without a leading sign.
pub(super) fn drop_sign(text: &str) -> &str {
    take_sign(text).1
}

/// A decimal literal taken apart.
pub(super) struct Decimal<'text> {
    /// Whether a `'-'` introduced the value.
    pub(super) negative: bool,
    /// The digits before the point.
    whole: &'text str,
    /// The digits after the point, empty when there is no point.
    fraction: &'text str,
    /// The power of ten the digits stand for, saturating away from zero.
    exponent: i128,
}

/// Splits `[+-]digits[.digits][e[+-]digits]` into its parts.
///
/// # Errors
///
/// [`CyberError::Format`] when `text` is not shaped like that.
pub(super) fn split_decimal(text: &str) -> Result<Decimal<'_>, CyberError> {
    let (negative, body) = take_sign(text);
    let (mantissa, exponent) = split_exponent(body);
    let (whole, fraction) = split_point(mantissa);
    if !is_digit_run(whole) || !all_digits(fraction) {
        return Err(CyberError::Format(format!(
            "'{text}' is not a decimal number"
        )));
    }
    Ok(Decimal {
        negative,
        whole,
        fraction,
        exponent: parse_exponent(exponent, text)?,
    })
}

/// Splits `text` at its first exponent marker, if it has one.
fn split_exponent(text: &str) -> (&str, Option<&str>) {
    match text.split_once(['e', 'E']) {
        Some((head, tail)) => (head, Some(tail)),
        None => (text, None),
    }
}

/// Splits `text` at its first point, leaving the fraction empty without one.
fn split_point(text: &str) -> (&str, &str) {
    text.split_once('.').unwrap_or((text, ""))
}

/// Reads an exponent, saturating so that an absurd scale still decides
/// whole against fractional.
///
/// # Errors
///
/// [`CyberError::Format`] when `exponent` is not `[+-]?digits`; `source` is
/// the whole literal, named in the message.
fn parse_exponent(exponent: Option<&str>, source: &str) -> Result<i128, CyberError> {
    let Some(body) = exponent else {
        return Ok(0_i128);
    };
    let (negative, digits) = take_sign(body);
    if !is_digit_run(digits) {
        return Err(CyberError::Format(format!(
            "'{source}' is not a decimal number"
        )));
    }
    let magnitude = accumulate(digits, 10_u32).unwrap_or(i128::MAX);
    if negative {
        return Ok(magnitude.saturating_neg());
    }
    Ok(magnitude)
}

/// The literal's digits without leading zeroes and the power of ten scaling
/// them; the digits stand for zero when none are left.
pub(super) fn normalize(parts: &Decimal<'_>) -> (String, i128) {
    let digits: String = parts.whole.chars().chain(parts.fraction.chars()).collect();
    let scale = digit_count(parts.fraction);
    let shift = parts.exponent.checked_sub(scale).unwrap_or(i128::MIN);
    (digits.trim_start_matches('0').to_owned(), shift)
}

/// The integer a decimal literal names, before its sign is applied.
pub(super) fn rounded_magnitude(parts: &Decimal<'_>) -> Option<i128> {
    let (digits, shift) = normalize(parts);
    if digits.is_empty() {
        return Some(0_i128);
    }
    if shift >= 0 {
        scale_up(&digits, shift)
    } else {
        scale_down(&digits, shift)
    }
}

/// `digits` times ten to the `shift`, or `None` past an `i64` magnitude.
fn scale_up(digits: &str, shift: i128) -> Option<i128> {
    let width = digit_count(digits).checked_add(shift)?;
    if width > MAX_I64_MAGNITUDE_DIGITS {
        return None;
    }
    let magnitude = i128_of_digits(digits)?;
    (0..shift).try_fold(magnitude, |value, _| value.checked_mul(10_i128))
}

/// `digits` divided by ten to the `-shift`, rounded half away from zero.
fn scale_down(digits: &str, shift: i128) -> Option<i128> {
    let Some(places) = shift
        .checked_neg()
        .and_then(|value| usize::try_from(value).ok())
    else {
        return Some(0_i128);
    };
    if places > digits.len() {
        return Some(0_i128);
    }
    let keep = digits.len().saturating_sub(places);
    let head: String = digits.chars().take(keep).collect();
    let round_up = digits.chars().nth(keep).is_some_and(|digit| digit >= '5');
    let magnitude = i128_of_digits(&head)?;
    if round_up {
        magnitude.checked_add(1_i128)
    } else {
        Some(magnitude)
    }
}

/// The number of characters in `text`, counted in `i128`.
fn digit_count(text: &str) -> i128 {
    text.chars()
        .fold(0_i128, |count, _| count.saturating_add(1_i128))
}

/// The value of an ASCII digit run, or `None` past the `i128` range.
fn i128_of_digits(digits: &str) -> Option<i128> {
    digits.chars().try_fold(0_i128, |value, digit| {
        let unit = i128::from(digit.to_digit(10_u32)?);
        value.checked_mul(10_i128)?.checked_add(unit)
    })
}
