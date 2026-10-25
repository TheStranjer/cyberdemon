//! Float-free math: `Doom`'s 16.16 fixed point, `BAM` angles, integer trig
//! and the crate's shared integer helpers.
//!
//! The crate denies `float_arithmetic`, and geometry (rotation, the BSP
//! node builder, the software renderer) needs fractions and trigonometry,
//! so everything here is integer-only, computed the way vanilla `Doom`
//! did: [`Fixed`] is a signed 16.16 value, [`angle`] adds 32-bit binary
//! angles with `CORDIC` trig, and this root adds exact square roots and
//! the one home for the idioms the whole crate repeats: [`div_round`]
//! (and [`mul_div_round`], [`div_round_sat`], [`mul_div_round_sat`])
//! divides with the crate's rule, rounding to nearest with halves away
//! from zero; [`narrow_i64`] and [`narrow_i32`] saturate a wider value
//! into a narrower one; [`gcd`] is the never-zero greatest common
//! divisor; and [`midpoint`] is the [`div_round`]-rounded mean of two
//! coordinates. Use this module instead of `f32`/`f64`.

pub mod angle;
pub mod trig;

pub use angle::{Angle, atan2, hypot, rotate_point};

/// Fractional bits of a 16.16 [`Fixed`] value.
pub const FRACBITS: u32 = 16;
/// One whole unit in [`Fixed`] representation (`65536`).
pub const FRACUNIT: i32 = 0x0001_0000;

/// Values at or above this magnitude do not fit 16.16 (`32768`).
const MAGNITUDE_LIMIT: i32 = 0x8000;

/// A signed 16.16 fixed-point number: 15 integer bits, a sign bit and 16
/// fraction bits, exactly vanilla `Doom`'s `fixed_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fixed(pub i32);

impl Fixed {
    /// `0.0`.
    pub const ZERO: Self = Self(0);
    /// `1.0`.
    pub const ONE: Self = Self(FRACUNIT);

    /// Wraps a whole number, or `None` when `|value| >= 32768` does not
    /// fit the 15 integer bits.
    #[must_use]
    #[inline]
    pub fn from_int(value: i32) -> Option<Self> {
        if value
            .checked_abs()
            .is_none_or(|magnitude| magnitude >= MAGNITUDE_LIMIT)
        {
            return None;
        }
        Some(Self(value.checked_mul(FRACUNIT)?))
    }

    /// `numerator / denominator` in 16.16, rounded to nearest (halves
    /// away from zero). `None` when the denominator is zero or the result
    /// leaves the 16.16 range.
    #[must_use]
    #[inline]
    pub fn from_ratio(numerator: i64, denominator: i64) -> Option<Self> {
        let scaled = mul_div_round(numerator, i64::from(FRACUNIT), denominator)?;
        Some(Self(i32::try_from(scaled).ok()?))
    }

    /// The integer part rounded toward negative infinity.
    #[must_use]
    #[inline]
    pub fn to_int_floor(self) -> i32 {
        self.0.checked_shr(FRACBITS).unwrap_or(0)
    }

    /// The integer part rounded to nearest, halves away from zero
    /// (`-1.5` becomes `-2`, `1.5` becomes `2`).
    #[must_use]
    #[inline]
    pub fn to_int_round(self) -> i32 {
        div_round(i128::from(self.0), i128::from(FRACUNIT)).map_or(i32::MAX, narrow_i32)
    }

    /// Sum, `None` on overflow of the 16.16 range.
    #[must_use]
    #[inline]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self(self.0.checked_add(other.0)?))
    }

    /// Difference, `None` on overflow of the 16.16 range.
    #[must_use]
    #[inline]
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self(self.0.checked_sub(other.0)?))
    }

    /// Sum clamped to the 16.16 range.
    #[must_use]
    #[inline]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Difference clamped to the 16.16 range.
    #[must_use]
    #[inline]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// Product through the full `i64` intermediate, shifted down by 16
    /// (truncating). `None` when the result leaves the 16.16 range.
    #[must_use]
    #[inline]
    pub fn checked_mul(self, other: Self) -> Option<Self> {
        let product = i64::from(self.0).checked_mul(i64::from(other.0))?;
        Some(Self(i32::try_from(product.checked_shr(FRACBITS)?).ok()?))
    }

    /// Product clamped to the 16.16 range.
    #[must_use]
    #[inline]
    pub fn saturating_mul(self, other: Self) -> Self {
        let product = i64::from(self.0).wrapping_mul(i64::from(other.0));
        let shifted = product.checked_shr(FRACBITS).unwrap_or(0);
        Self(i32::try_from(shifted).unwrap_or(if shifted >= 0 { i32::MAX } else { i32::MIN }))
    }

    /// Quotient as `(self << 16) / other` in `i64`, truncating. `None`
    /// for a zero divisor or a quotient beyond the 16.16 range.
    #[must_use]
    #[inline]
    pub fn checked_div(self, other: Self) -> Option<Self> {
        let scaled = i64::from(self.0).checked_shl(FRACBITS)?;
        let quotient = scaled.checked_div(i64::from(other.0))?;
        Some(Self(i32::try_from(quotient).ok()?))
    }

    /// Absolute value; the raw minimum saturates to `i32::MAX`.
    #[must_use]
    #[inline]
    pub const fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    /// Negation; the raw minimum saturates to `i32::MAX`. Kept as a
    /// `const fn`, unlike the by-value `std::ops::Neg` counterpart.
    #[must_use]
    #[inline]
    pub const fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

/// `floor(sqrt(n))`, exact for every `u64`.
#[must_use]
#[inline]
pub fn isqrt(n: u64) -> u64 {
    isqrt_u128(u128::from(n))
}

/// `value * num / den` rounded to nearest (halves away from zero) with an
/// `i128` intermediate. `None` when `den` is zero or the result leaves
/// `i64`.
#[must_use]
#[inline]
pub fn mul_div_round(value: i64, num: i64, den: i64) -> Option<i64> {
    let product = i128::from(value).checked_mul(i128::from(num))?;
    let rounded = div_round(product, i128::from(den))?;
    i64::try_from(rounded).ok()
}

/// `numerator / denominator` rounded to nearest, halves away from zero:
/// the crate's one rounded integer division. `None` when the denominator
/// is zero or the rounded quotient leaves `i128`.
#[must_use]
#[inline]
pub fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    let quotient = numerator.checked_div(denominator)?;
    let remainder = numerator.checked_rem(denominator)?;
    let rounds_up = remainder
        .checked_abs()
        .and_then(|magnitude| magnitude.checked_mul(2_i128))
        .is_some_and(|twice| twice >= denominator.saturating_abs());
    if !rounds_up {
        return Some(quotient);
    }
    if (numerator >= 0_i128) == (denominator >= 0_i128) {
        quotient.checked_add(1_i128)
    } else {
        quotient.checked_sub(1_i128)
    }
}

/// [`div_round`] saturating the quotient into `i64`: a refused division
/// (zero denominator included) answers the `i64` extreme in the
/// quotient's direction.
#[must_use]
#[inline]
pub fn div_round_sat(numerator: i128, denominator: i128) -> i64 {
    match div_round(numerator, denominator) {
        Some(exact) => narrow_i64(exact),
        None if (numerator >= 0_i128) == (denominator >= 0_i128) => i64::MAX,
        None => i64::MIN,
    }
}

/// `value * numerator / denominator` rounded like [`mul_div_round`],
/// saturating to the `i64` extreme in the product's direction instead of
/// refusing.
#[must_use]
#[inline]
pub fn mul_div_round_sat(value: i64, numerator: i64, denominator: i64) -> i64 {
    match mul_div_round(value, numerator, denominator) {
        Some(exact) => exact,
        None if (value >= 0_i64) == (numerator >= 0_i64) => i64::MAX,
        None => i64::MIN,
    }
}

/// The `i64` nearest `value`, saturating at the range extremes.
#[must_use]
#[inline]
pub fn narrow_i64(value: i128) -> i64 {
    match i64::try_from(value) {
        Ok(narrowed) => narrowed,
        Err(_error) if value < 0_i128 => i64::MIN,
        Err(_error) => i64::MAX,
    }
}

/// The `i32` nearest `value`, saturating at the range extremes.
#[must_use]
#[inline]
pub fn narrow_i32(value: i128) -> i32 {
    match i32::try_from(value) {
        Ok(narrowed) => narrowed,
        Err(_error) if value < 0_i128 => i32::MIN,
        Err(_error) => i32::MAX,
    }
}

/// The greatest common divisor of the magnitudes of `left` and `right`,
/// never zero: the `0, 0` pair answers `1` so a caller can always divide
/// by the result. `i128::MIN`'s magnitude saturates (defensive).
#[must_use]
#[inline]
pub const fn gcd(mut left: i128, mut right: i128) -> i128 {
    if left < 0 {
        left = left.saturating_neg();
    }
    if right < 0 {
        right = right.saturating_neg();
    }
    while right != 0 {
        let next = match left.checked_rem(right) {
            Some(rem) => rem,
            None => 0,
        };
        left = right;
        right = next;
    }
    if left < 1 { 1 } else { left }
}

/// The midpoint of two coordinates: their mean rounded by
/// [`div_round`]'s rule, halves away from zero. The mean of two `i64`s
/// always fits `i64`, so the saturation is defensive.
#[must_use]
#[inline]
pub fn midpoint(low: i64, high: i64) -> i64 {
    let sum = i128::from(low).saturating_add(i128::from(high));
    narrow_i64(div_round(sum, 2_i128).unwrap_or(sum))
}

/// `floor(sqrt(n))` for `u128` inputs; the result always fits `u64`
/// because `(2^64 - 1)^2` is the largest square a `u128` holds.
fn isqrt_u128(n: u128) -> u64 {
    if n <= 1 {
        return u64::try_from(n).unwrap_or(u64::MAX);
    }
    // Highest power of four not above n; the msb index minus one is even,
    // so the shift cannot fail.
    let msb = 127_u32.saturating_sub(n.leading_zeros());
    let mut bit = 1_u128.checked_shl(msb & !1).unwrap_or(1);
    let mut root: u128 = 0;
    let mut remainder = n;
    while bit != 0 {
        let candidate = root.saturating_add(bit);
        if remainder >= candidate {
            remainder = remainder.saturating_sub(candidate);
            root = bit.saturating_add(root.checked_shr(1).unwrap_or(0));
        } else {
            root = root.checked_shr(1).unwrap_or(0);
        }
        bit = bit.checked_shr(2).unwrap_or(0);
    }
    u64::try_from(root).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        Fixed, MAGNITUDE_LIMIT, div_round, div_round_sat, gcd, isqrt, midpoint, mul_div_round,
        mul_div_round_sat, narrow_i32, narrow_i64,
    };

    /// `n` squared as `u64`, for perfect-square tests.
    fn square(n: u64) -> u64 {
        n.checked_mul(n).unwrap()
    }

    /// A whole value as `Fixed`, panicking only on out-of-range input.
    fn whole(value: i32) -> Fixed {
        Fixed::from_int(value).unwrap()
    }

    #[test]
    fn from_int_accepts_15_bits_and_rejects_the_rest() {
        assert_eq!(Fixed::from_int(0), Some(Fixed::ZERO), "zero");
        assert_eq!(Fixed::from_int(1), Some(Fixed::ONE), "one");
        assert_eq!(Fixed::from_int(-1), Some(Fixed(-0x0001_0000)), "minus one");
        assert_eq!(
            Fixed::from_int(0x7FFF),
            Some(Fixed(0x7FFF_0000)),
            "largest whole value"
        );
        assert_eq!(
            Fixed::from_int(MAGNITUDE_LIMIT),
            None,
            "one above the range"
        );
        assert_eq!(
            Fixed::from_int(-0x7FFF),
            Some(Fixed(-0x7FFF_0000)),
            "most whole"
        );
        assert_eq!(
            Fixed::from_int(MAGNITUDE_LIMIT.checked_neg().unwrap()),
            None,
            "|-32768| does not fit"
        );
        assert_eq!(
            Fixed::from_int(i32::MIN),
            None,
            "raw minimum saturates to None"
        );
    }

    #[test]
    fn from_ratio_rounds_to_nearest_away_from_zero() {
        assert_eq!(Fixed::from_ratio(1, 3), Some(Fixed(21_845)), "1/3");
        assert_eq!(Fixed::from_ratio(-1, 3), Some(Fixed(-21_845)), "-1/3");
        assert_eq!(Fixed::from_ratio(2, 3), Some(Fixed(43_691)), "2/3");
        assert_eq!(Fixed::from_ratio(-2, 3), Some(Fixed(-43_691)), "-2/3");
        assert_eq!(Fixed::from_ratio(1, 1), Some(Fixed::ONE), "exact one");
        assert_eq!(Fixed::from_ratio(0, 5), Some(Fixed::ZERO), "zero numerator");
        assert_eq!(
            Fixed::from_ratio(1, 0x2_0000),
            Some(Fixed(1)),
            "exact half tie rounds away"
        );
        assert_eq!(
            Fixed::from_ratio(-1, 0x2_0000),
            Some(Fixed(-1)),
            "negative half tie rounds away"
        );
    }

    #[test]
    fn from_ratio_rejects_zero_and_out_of_range() {
        assert_eq!(Fixed::from_ratio(1, 0), None, "zero denominator");
        assert_eq!(Fixed::from_ratio(-1, 0), None, "zero negative denominator");
        assert_eq!(
            Fixed::from_ratio(i64::from(MAGNITUDE_LIMIT), 1),
            None,
            "one above range"
        );
        assert_eq!(
            Fixed::from_ratio(
                i64::from(MAGNITUDE_LIMIT.checked_neg().unwrap())
                    .checked_sub(1)
                    .unwrap(),
                1
            ),
            None,
            "below the raw minimum"
        );
        assert_eq!(
            Fixed::from_ratio(i64::from(MAGNITUDE_LIMIT.checked_neg().unwrap()), 1),
            Some(Fixed(i32::MIN)),
            "-32768.0 is the raw minimum and fits"
        );
        assert_eq!(
            Fixed::from_ratio(i64::from(i32::MAX), 1),
            None,
            "huge numerator leaves the range"
        );
    }

    #[test]
    fn floor_and_round_treat_negatives_as_documented() {
        let minus_1_5 = Fixed(-0x0001_8000);
        assert_eq!(minus_1_5.to_int_floor(), -2_i32, "-1.5 floors to -2");
        assert_eq!(
            minus_1_5.to_int_round(),
            -2_i32,
            "-1.5 rounds away from zero"
        );
        let plus_1_5 = Fixed(0x0001_8000);
        assert_eq!(plus_1_5.to_int_floor(), 1_i32, "1.5 floors to 1");
        assert_eq!(plus_1_5.to_int_round(), 2_i32, "1.5 rounds away from zero");
        let minus_half = Fixed(-0x0000_8000);
        assert_eq!(minus_half.to_int_floor(), -1_i32, "-0.5 floors to -1");
        assert_eq!(
            minus_half.to_int_round(),
            -1_i32,
            "-0.5 rounds away from zero"
        );
        assert_eq!(Fixed(26_214).to_int_floor(), 0_i32, "0.4 floors to 0");
        assert_eq!(Fixed(26_214).to_int_round(), 0_i32, "0.4 rounds to 0");
        assert_eq!(Fixed(-26_214).to_int_floor(), -1_i32, "-0.4 floors to -1");
        assert_eq!(Fixed(-26_214).to_int_round(), 0_i32, "-0.4 rounds to 0");
        assert_eq!(Fixed::ONE.to_int_round(), 1_i32, "exact one is unaffected");
    }

    #[test]
    fn multiply_keeps_the_i64_intermediate() {
        assert_eq!(whole(2).checked_mul(whole(3)), Some(whole(6)), "2 * 3");
        assert_eq!(whole(-2).checked_mul(whole(3)), Some(whole(-6)), "-2 * 3");
        assert_eq!(whole(-2).checked_mul(whole(-3)), Some(whole(6)), "-2 * -3");
        let half = Fixed(0x8000);
        assert_eq!(
            half.checked_mul(half),
            Some(Fixed(0x4000)),
            "0.5 * 0.5 = 0.25"
        );
        assert_eq!(
            Fixed(-1).checked_mul(Fixed::ONE),
            Some(Fixed(-1)),
            "truncating shift is -1, not 0"
        );
        assert_eq!(
            Fixed(0x7FFF_FFFF).checked_mul(whole(4)),
            None,
            "overflow reports None"
        );
        assert_eq!(
            Fixed(i32::MIN).checked_mul(whole(4)),
            None,
            "negative overflow reports None"
        );
    }

    #[test]
    fn divide_scales_up_before_truncating() {
        assert_eq!(whole(6).checked_div(whole(2)), Some(whole(3)), "6 / 2");
        assert_eq!(whole(-6).checked_div(whole(2)), Some(whole(-3)), "-6 / 2");
        assert_eq!(whole(6).checked_div(whole(-2)), Some(whole(-3)), "6 / -2");
        assert_eq!(
            Fixed::ONE.checked_div(Fixed(2)),
            None,
            "1 / (2/65536) = 32768 leaves the range"
        );
        assert_eq!(
            Fixed::ONE.checked_div(Fixed(4)),
            Some(Fixed(0x4000_0000)),
            "1 / (4/65536) = 16384"
        );
        assert_eq!(Fixed::ONE.checked_div(Fixed::ZERO), None, "zero divisor");
        assert_eq!(
            whole(1).checked_div(whole(3)),
            Some(Fixed(21_845)),
            "1 / 3 truncates to 21845 (0.33333)"
        );
        assert_eq!(
            Fixed(0x7FFF_FFFF).checked_div(Fixed(1)),
            None,
            "dividing by 1/65536 leaves the range"
        );
    }

    #[test]
    fn saturating_variants_clamp_at_the_edges() {
        let max = Fixed(i32::MAX);
        let min = Fixed(i32::MIN);
        assert_eq!(max.saturating_add(Fixed::ONE), max, "add clamps up");
        assert_eq!(min.saturating_sub(Fixed::ONE), min, "sub clamps down");
        assert_eq!(max.saturating_mul(whole(4)), max, "mul clamps up");
        assert_eq!(min.saturating_mul(whole(4)), min, "mul clamps down");
        assert_eq!(
            max.saturating_add(min),
            Fixed(-1),
            "raw edges sum to minus one without clamping"
        );
        assert_eq!(max.saturating_add(max), max, "raw edges add past the top");
    }

    #[test]
    fn abs_and_neg_saturate_at_the_raw_minimum() {
        assert_eq!(Fixed(-5).abs(), Fixed(5), "plain absolute");
        assert_eq!(Fixed(-5).neg(), Fixed(5), "plain negation");
        assert_eq!(
            Fixed(i32::MIN).abs(),
            Fixed(i32::MAX),
            "raw minimum absolute saturates"
        );
        assert_eq!(
            Fixed(i32::MIN).neg(),
            Fixed(i32::MAX),
            "raw minimum negation saturates"
        );
        assert_eq!(Fixed::ONE.neg(), Fixed(-0x0001_0000), "one negates");
    }

    #[test]
    fn isqrt_matches_small_exact_cases() {
        assert_eq!(isqrt(0), 0, "zero");
        assert_eq!(isqrt(1), 1, "one");
        assert_eq!(isqrt(2), 1, "2 floors to 1");
        assert_eq!(isqrt(3), 1, "3 floors to 1");
        assert_eq!(isqrt(4), 2, "exact square");
        assert_eq!(isqrt(15), 3, "15 floors to 3");
        assert_eq!(isqrt(16), 4, "exact square");
        assert_eq!(isqrt(0xFFFF_FFFF), 0xFFFF, "just below 2^32");
        assert_eq!(isqrt(0x1_0000_0000), 0x1_0000, "2^32");
    }

    #[test]
    fn isqrt_is_exact_on_squares_and_edges() {
        for n in 0..0x1_0000_u64 {
            assert_eq!(isqrt(square(n)), n, "exact square of {n}");
        }
        for n in 1..0x1_0000_u64 {
            let below = square(n).checked_sub(1).unwrap();
            assert_eq!(isqrt(below), n.checked_sub(1).unwrap(), "one below {n}^2");
        }
        assert_eq!(isqrt(square(0xFFFF_FFFF)), 0xFFFF_FFFF, "(2^32 - 1)^2");
        assert_eq!(
            isqrt(square(0xFFFF_FFFF).checked_sub(1).unwrap()),
            0xFFFF_FFFE,
            "one below (2^32 - 1)^2"
        );
        assert_eq!(isqrt(u64::MAX), 0xFFFF_FFFF, "u64::MAX");
    }

    #[test]
    fn mul_div_round_rounds_and_rejects_zero() {
        assert_eq!(mul_div_round(2, 3, 4), Some(2), "1.5 away from zero");
        assert_eq!(mul_div_round(-2, 3, 4), Some(-2), "-1.5 away from zero");
        assert_eq!(mul_div_round(7, 1, 2), Some(4), "3.5 up");
        assert_eq!(mul_div_round(-7, 1, 2), Some(-4), "-3.5 down");
        assert_eq!(mul_div_round(1, 1, 3), Some(0), "0.33 to zero");
        assert_eq!(mul_div_round(-1, 1, 3), Some(0), "-0.33 to zero");
        assert_eq!(mul_div_round(2, 1, 3), Some(1), "0.67 to one");
        assert_eq!(mul_div_round(7, 1, -2), Some(-4), "negative divisor");
        assert_eq!(mul_div_round(0, 5, 3), Some(0), "zero numerator");
        assert_eq!(mul_div_round(1, 0x1_0000, 0x2_0000), Some(1), "0.5 tie");
        assert_eq!(mul_div_round(1, 1, 0), None, "zero denominator");
        assert_eq!(mul_div_round(i64::MAX, 2, 1), None, "result leaves i64");
        assert_eq!(mul_div_round(i64::MAX, 1, 1), Some(i64::MAX), "identity");
    }

    #[test]
    fn div_round_is_nearest_halves_away_from_zero() {
        assert_eq!(div_round(7, 2), Some(4), "3.5 up");
        assert_eq!(div_round(-7, 2), Some(-4), "-3.5 down");
        assert_eq!(div_round(7, -2), Some(-4), "negative divisor");
        assert_eq!(div_round(-7, -2), Some(4), "both negative");
        assert_eq!(div_round(5, 2), Some(3), "2.5 up");
        assert_eq!(div_round(-5, 2), Some(-3), "-2.5 down");
        assert_eq!(div_round(1, 3), Some(0), "0.33 to zero");
        assert_eq!(div_round(-1, 3), Some(0), "-0.33 to zero");
        assert_eq!(div_round(2, 3), Some(1), "0.67 to one");
        assert_eq!(div_round(0, 5), Some(0), "zero numerator");
        assert_eq!(div_round(4, 2), Some(2), "exact division");
        assert_eq!(div_round(1, 0), None, "zero denominator");
        assert_eq!(div_round(0, 0), None, "zero over zero");
        assert_eq!(div_round(i128::MAX, 1), Some(i128::MAX), "top of the range");
        assert_eq!(
            div_round(i128::MIN, 2),
            Some(i128::MIN.checked_div(2).unwrap()),
            "bottom of the range is exact"
        );
        assert_eq!(div_round(i128::MIN, -1), None, "MIN over -1 does not fit");
    }

    #[test]
    fn div_round_sat_saturates_rather_than_refusing() {
        assert_eq!(div_round_sat(7, 2), 4, "plain rounding");
        assert_eq!(div_round_sat(-7, 2), -4, "and negative");
        assert_eq!(div_round_sat(1, 0), i64::MAX, "zero divisor, positive");
        assert_eq!(div_round_sat(-1, 0), i64::MIN, "zero divisor, negative");
        assert_eq!(
            div_round_sat(i128::from(i64::MAX) + 1, 1),
            i64::MAX,
            "past the top saturates"
        );
        assert_eq!(
            div_round_sat(i128::from(i64::MIN) - 1, 1),
            i64::MIN,
            "past the bottom saturates"
        );
    }

    #[test]
    fn mul_div_round_sat_saturates_in_the_products_direction() {
        assert_eq!(mul_div_round_sat(2, 3, 4), 2, "1.5 away from zero");
        assert_eq!(mul_div_round_sat(-2, 3, 4), -2, "-1.5 away from zero");
        assert_eq!(mul_div_round_sat(8, 128, 128), 8, "exact stays exact");
        assert_eq!(
            mul_div_round_sat(i64::MAX, i64::MAX, 1),
            i64::MAX,
            "a leaving result saturates high"
        );
        assert_eq!(
            mul_div_round_sat(i64::MIN, i64::MAX, 1),
            i64::MIN,
            "and low for negative products"
        );
        assert_eq!(
            mul_div_round_sat(8, 2, 0),
            i64::MAX,
            "zero divisor goes high"
        );
        assert_eq!(
            mul_div_round_sat(-8, 2, 0),
            i64::MIN,
            "by the product's sign"
        );
    }

    #[test]
    fn narrow_saturates_towards_the_nearest_extreme() {
        assert_eq!(narrow_i64(0), 0, "zero");
        assert_eq!(narrow_i64(i128::from(i64::MAX)), i64::MAX, "top stays");
        assert_eq!(narrow_i64(i128::from(i64::MIN)), i64::MIN, "bottom stays");
        assert_eq!(
            narrow_i64(i128::from(i64::MAX).checked_add(1).unwrap()),
            i64::MAX,
            "above the top saturates high"
        );
        assert_eq!(
            narrow_i64(i128::from(i64::MIN).checked_sub(1).unwrap()),
            i64::MIN,
            "below the bottom saturates low, not high"
        );
        assert_eq!(narrow_i32(i128::from(i32::MAX)), i32::MAX, "top stays");
        assert_eq!(narrow_i32(i128::from(i32::MIN)), i32::MIN, "bottom stays");
        assert_eq!(
            narrow_i32(i128::from(i32::MAX).checked_add(1).unwrap()),
            i32::MAX,
            "above the top saturates high"
        );
        assert_eq!(
            narrow_i32(i128::from(i32::MIN).checked_sub(1).unwrap()),
            i32::MIN,
            "a large negative saturates low, not high"
        );
    }

    #[test]
    fn gcd_uses_magnitudes_and_never_answers_zero() {
        assert_eq!(gcd(0, 0), 1, "the defensive pair");
        assert_eq!(gcd(0, 5), 5, "one zero");
        assert_eq!(gcd(5, 0), 5, "the other");
        assert_eq!(gcd(12, 18), 6, "plain");
        assert_eq!(gcd(18, 12), 6, "either order");
        assert_eq!(gcd(-12, 18), 6, "negative magnitude");
        assert_eq!(gcd(12, -18), 6, "the other side");
        assert_eq!(gcd(7, 5), 1, "coprime");
        assert_eq!(gcd(i128::from(u64::MAX), 2), 1, "beyond i64");
        assert_eq!(
            gcd(i128::from(u64::MAX).checked_mul(2).unwrap(), 6),
            6,
            "beyond u64"
        );
        assert_eq!(gcd(i128::MIN, 0), i128::MAX, "MIN's magnitude saturates");
    }

    #[test]
    fn midpoint_is_the_mean_rounded_away_from_zero() {
        assert_eq!(midpoint(0, 4), 2, "exact half");
        assert_eq!(midpoint(-4, 0), -2, "exact negative half");
        assert_eq!(midpoint(0, 3), 2, "1.5 goes up");
        assert_eq!(midpoint(0, -3), -2, "-1.5 goes down");
        assert_eq!(midpoint(-3, 0), -2, "from the negative side too");
        assert_eq!(midpoint(1, 2), 2, "sum 3");
        assert_eq!(midpoint(-300, 300), 0, "symmetric span");
        assert_eq!(midpoint(i64::MAX, i64::MAX), i64::MAX, "top edge");
        assert_eq!(midpoint(i64::MIN, i64::MIN), i64::MIN, "bottom edge");
        assert_eq!(
            midpoint(i64::MIN, 0),
            i64::MIN.checked_div(2).unwrap(),
            "half the bottom edge"
        );
    }

    #[test]
    fn to_int_round_uses_the_shared_rule() {
        assert_eq!(
            Fixed::from_ratio(3_i64, 2_i64).map(Fixed::to_int_round),
            Some(2_i32),
            "1.5"
        );
        assert_eq!(
            Fixed::from_ratio(-3_i64, 2_i64).map(Fixed::to_int_round),
            Some(-2_i32),
            "-1.5 saturates nowhere and rounds away"
        );
        assert_eq!(
            Fixed::from_ratio(1_i64, 2_i64).map(Fixed::to_int_round),
            Some(1_i32),
            "0.5"
        );
        assert_eq!(
            Fixed::from_ratio(-1_i64, 2_i64).map(Fixed::to_int_round),
            Some(-1_i32),
            "-0.5"
        );
    }
}
