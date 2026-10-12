//! Float-free math: `Doom`'s 16.16 fixed point, `BAM` angles, integer trig.
//!
//! The crate denies `float_arithmetic`, and geometry (rotation, the BSP
//! node builder, the software renderer) needs fractions and trigonometry,
//! so everything here is integer-only, computed the way vanilla `Doom`
//! did: [`Fixed`] is a signed 16.16 value, [`angle`] adds 32-bit binary
//! angles with `CORDIC` trig, and this root adds exact square roots and
//! rounded multiply-divide. Use this module instead of `f32`/`f64`.

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
        i32::try_from(round_shift(i128::from(self.0), FRACBITS)).unwrap_or(i32::MAX)
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
    let divisor = i128::from(den);
    let quotient = product.checked_div(divisor)?;
    let remainder = product.checked_rem(divisor)?;
    let rounded = if remainder.checked_abs()?.checked_mul(2)? < divisor.checked_abs()? {
        quotient
    } else if (remainder >= 0) == (divisor >= 0) {
        quotient.checked_add(1)?
    } else {
        quotient.checked_sub(1)?
    };
    i64::try_from(rounded).ok()
}

/// `value / 2^shift` rounded to nearest, halves away from zero,
/// saturating the result to `i64`.
pub(crate) fn round_shift(value: i128, shift: u32) -> i64 {
    let half = (1_i128.checked_shl(shift).unwrap_or(2))
        .checked_shr(1)
        .unwrap_or(1);
    let magnitude = value.checked_abs().unwrap_or(i128::MAX);
    let rounded = magnitude
        .checked_add(half)
        .and_then(|biased| biased.checked_shr(shift))
        .unwrap_or(i128::MAX);
    let signed = if value >= 0 {
        rounded
    } else {
        rounded.checked_neg().unwrap_or(i128::MIN)
    };
    i64::try_from(signed).unwrap_or(if signed >= 0 { i64::MAX } else { i64::MIN })
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
    use super::{Fixed, MAGNITUDE_LIMIT, isqrt, mul_div_round};

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
}
