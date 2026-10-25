//! 32-bit binary angles (`BAM`) and the integer trigonometry on them.
//!
//! One full turn is `2^32`: `0` points east (`+x`), `0x4000_0000` is 90
//! degrees counter-clockwise (north, `+y`), and arithmetic wraps. This is
//! vanilla `Doom`'s `angle_t`. Sine and cosine come from the [`trig`]
//! `CORDIC`; so do the inverse (`atan2`) and the whole-number helpers
//! [`hypot`] and [`rotate_point`].

use super::{Fixed, div_round_sat, isqrt_u128};
use crate::fixed::{FRACUNIT, trig};

/// `BAM` per full turn as `u64` (`2^32`).
const TURN: u64 = 0x1_0000_0000;

/// Whole degrees per turn.
const DEGREES: i64 = 360;

/// Millidegrees per turn.
const MILLIDEGREES: i64 = DEGREES * 1_000;

/// Smallest absolute cosine, as a raw 16.16 value, that [`Angle::tan`]
/// accepts: `1/64`, about `0.03` degrees from vertical.
const TAN_MIN_COS: i32 = 0x400;

/// An unsigned 32-bit binary angle (`BAM`), counter-clockwise from east.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Angle(pub u32);

impl Angle {
    /// Due east: 0 degrees, the `+x` axis.
    pub const EAST: Self = Self(0);
    /// Due north: 90 degrees, the `+y` axis.
    pub const NORTH: Self = Self(0x4000_0000);
    /// Due west: 180 degrees.
    pub const WEST: Self = Self(0x8000_0000);
    /// Due south: 270 degrees.
    pub const SOUTH: Self = Self(0xC000_0000);

    /// Whole degrees to `BAM` as `degrees * 2^32 / 360` rounded to
    /// nearest; any integer is accepted and the result wraps.
    #[must_use]
    #[inline]
    pub fn from_degrees(degrees: i64) -> Self {
        Self::from_fraction(degrees, DEGREES)
    }

    /// Millidegrees (thousandths of a degree) to `BAM`, rounded to
    /// nearest; any integer is accepted and the result wraps.
    #[must_use]
    #[inline]
    pub fn from_millidegrees(millidegrees: i64) -> Self {
        Self::from_fraction(millidegrees, MILLIDEGREES)
    }

    /// The angle in whole degrees, rounded to nearest, in `0..=359`.
    #[must_use]
    #[inline]
    pub fn to_degrees(self) -> i64 {
        // round(bam * 360 / 2^32) as (bam * 720 + 2^32) >> 33.
        let degrees = u128::from(self.0)
            .checked_mul(720_u128)
            .and_then(|twice| twice.checked_add(u128::from(TURN)))
            .and_then(|biased| biased.checked_shr(33))
            .and_then(|rounded| i64::try_from(rounded).ok())
            .unwrap_or(0);
        degrees.rem_euclid(DEGREES)
    }

    /// Sum of two angles, wrapping around the full turn.
    #[must_use]
    #[inline]
    pub const fn wrapping_add(self, other: Self) -> Self {
        Self(self.0.wrapping_add(other.0))
    }

    /// Difference of two angles, wrapping around the full turn.
    #[must_use]
    #[inline]
    pub const fn wrapping_sub(self, other: Self) -> Self {
        Self(self.0.wrapping_sub(other.0))
    }

    /// Sine in 16.16 (`CORDIC` rotation mode).
    #[must_use]
    #[inline]
    pub fn sin(self) -> Fixed {
        trig::cos_sin(self).1
    }

    /// Cosine in 16.16 (`CORDIC` rotation mode).
    #[must_use]
    #[inline]
    pub fn cos(self) -> Fixed {
        trig::cos_sin(self).0
    }

    /// Tangent `sin / cos` in 16.16. `None` within about `0.03` degrees
    /// of 90 or 270 (where `|cos|` is below `1/64`) and whenever the true
    /// value leaves the 16.16 range.
    #[must_use]
    #[inline]
    pub fn tan(self) -> Option<Fixed> {
        let (cos, sin) = trig::cos_sin(self);
        if cos.abs().0 >= TAN_MIN_COS {
            sin.checked_div(cos)
        } else {
            None
        }
    }

    /// `value * 2^32 / divisor` rounded to nearest, `value` first wrapped
    /// into `0..divisor` so the `i128` intermediates cannot overflow and
    /// the rounded quotient is always under `2^32`.
    fn from_fraction(value: i64, divisor: i64) -> Self {
        let wrapped = value.rem_euclid(divisor);
        let doubled = i128::from(wrapped)
            .checked_mul(i128::from(TURN))
            .and_then(|scaled| scaled.checked_mul(2_i128))
            .and_then(|twice| twice.checked_div(i128::from(divisor)));
        // Rounding the doubled quotient: (twice + 1) >> 1 gives nearest,
        // halves away from zero.
        let raw = doubled
            .and_then(|twice| twice.checked_add(1_i128))
            .and_then(|biased| biased.checked_shr(1))
            .and_then(|rounded| u32::try_from(rounded).ok())
            .unwrap_or(0);
        Self(raw)
    }
}

/// The direction of the vector `(dx, dy)` as an [`Angle`], via `CORDIC`
/// vector mode. The zero vector yields [`Angle::EAST`].
#[must_use]
#[inline]
pub fn atan2(dy: i64, dx: i64) -> Angle {
    trig::vector(dx, dy)
}

/// `floor(sqrt(dx * dx + dy * dy))`, exact: the squares are summed in
/// `u128` before the integer square root. Saturates to `u64::MAX` if the
/// sum ever leaves `u128`.
#[must_use]
#[inline]
pub fn hypot(dx: i64, dy: i64) -> u64 {
    let xx = u128::from(dx.unsigned_abs());
    let yy = u128::from(dy.unsigned_abs());
    let squared_sum = xx
        .checked_mul(xx)
        .and_then(|xsq| yy.checked_mul(yy).and_then(|ysq| xsq.checked_add(ysq)));
    squared_sum.map_or(u64::MAX, isqrt_u128)
}

/// Rotates `(x, y)` around the origin by `angle`: each output component is
/// `x * cos -/+ y * sin` through the exact `i128` product, rounded to the
/// nearest whole map unit. Values beyond `i64` saturate.
#[must_use]
#[inline]
pub fn rotate_point(x: i64, y: i64, angle: Angle) -> (i64, i64) {
    let cos = angle.cos().0;
    let sin = angle.sin().0;
    (
        component(x, y, cos, sin),
        component(y, x, cos, sin.wrapping_neg()),
    )
}

/// `(first * cos - second * sin) / 2^16` rounded to nearest: one output
/// axis of [`rotate_point`].
fn component(first: i64, second: i64, cos: i32, sin: i32) -> i64 {
    let a = i128::from(first)
        .checked_mul(i128::from(cos))
        .unwrap_or(0_i128);
    let b = i128::from(second)
        .checked_mul(i128::from(sin))
        .unwrap_or(0_i128);
    div_round_sat(a.wrapping_sub(b), i128::from(FRACUNIT))
}

#[cfg(test)]
mod tests {
    use super::{Angle, DEGREES, Fixed, MILLIDEGREES, atan2, hypot, rotate_point};

    /// `2^20` `BAM`, about `0.088` degrees: the compass tolerance.
    const BAM_TOLERANCE: u32 = 0x0010_0000;

    /// Asserts the raw 16.16 value is within `tolerance` of `expected`.
    fn assert_close(actual: i32, expected: i32, tolerance: u64, message: &str) {
        assert!(
            i64::from(actual).abs_diff(i64::from(expected)) <= tolerance,
            "{message}: got {actual}, want {expected}"
        );
    }

    #[test]
    fn cardinal_constants_and_wrapping() {
        assert_eq!(Angle::EAST.0, 0, "east");
        assert_eq!(Angle::NORTH.0, 0x4000_0000, "north");
        assert_eq!(Angle::WEST.0, 0x8000_0000, "west");
        assert_eq!(Angle::SOUTH.0, 0xC000_0000, "south");
        assert_eq!(Angle::default(), Angle::EAST, "default is east");
        assert_eq!(
            Angle::from_degrees(350).wrapping_add(Angle::from_degrees(20)),
            Angle::from_degrees(10),
            "add wraps past a full turn"
        );
        assert_eq!(
            Angle::EAST.wrapping_sub(Angle::NORTH),
            Angle::SOUTH,
            "subtract wraps below zero"
        );
    }

    #[test]
    fn from_degrees_is_exact_on_landmarks() {
        assert_eq!(Angle::from_degrees(0), Angle::EAST, "0");
        assert_eq!(Angle::from_degrees(90), Angle::NORTH, "90");
        assert_eq!(Angle::from_degrees(180), Angle::WEST, "180");
        assert_eq!(Angle::from_degrees(270), Angle::SOUTH, "270");
        assert_eq!(Angle::from_degrees(360), Angle::EAST, "full turn wraps");
        assert_eq!(Angle::from_degrees(-90), Angle::SOUTH, "-90 wraps");
        assert_eq!(Angle::from_degrees(720), Angle::EAST, "two turns wrap");
        assert_eq!(Angle::from_degrees(45).0, 0x2000_0000, "45 is an eighth");
    }

    #[test]
    fn degrees_round_trip_for_every_whole_angle() {
        for degrees in 0..DEGREES {
            let angle = Angle::from_degrees(degrees);
            assert_eq!(angle.to_degrees(), degrees, "round trip for {degrees}");
        }
        assert_eq!(
            Angle(0xFFFF_FFFF).to_degrees(),
            0,
            "just under a turn reads 0"
        );
    }

    #[test]
    fn millidegrees_cover_fractional_angles() {
        assert_eq!(Angle::from_millidegrees(22_500).0, 0x1000_0000, "22.5");
        assert_eq!(
            Angle::from_millidegrees(45_000),
            Angle::from_degrees(45),
            "45.0"
        );
        assert_eq!(
            Angle::from_millidegrees(-22_500).0,
            0xF000_0000,
            "-22.5 wraps"
        );
        assert_eq!(
            Angle::from_millidegrees(MILLIDEGREES),
            Angle::EAST,
            "a full turn"
        );
    }

    #[test]
    fn sine_and_cosine_hit_the_expected_table() {
        // (degrees, sin, cos) raw 16.16 values, tolerance +/- 2.
        let table = [
            (0_i64, 0_i32, 0x0001_0000_i32),
            (1, 1144_i32, 0xFFF6_i32),
            (30, 0x8000_i32, 0xDDB4_i32),
            (45, 0xB505_i32, 0xB505_i32),
            (60, 0xDDB4_i32, 0x8000_i32),
            (90, 0x0001_0000_i32, 0_i32),
            (120, 0xDDB4_i32, -0x8000_i32),
            (180, 0_i32, -0x0001_0000_i32),
            (225, -0xB505_i32, -0xB505_i32),
            (270, -0x0001_0000_i32, 0_i32),
            (315, -0xB505_i32, 0xB505_i32),
            (359, -1144_i32, 0xFFF6_i32),
        ];
        for (degrees, expected_sin, expected_cos) in table {
            let angle = Angle::from_degrees(degrees);
            assert_close(angle.sin().0, expected_sin, 2, &format!("sin {degrees}"));
            assert_close(angle.cos().0, expected_cos, 2, &format!("cos {degrees}"));
        }
    }

    #[test]
    fn sine_squared_plus_cosine_squared_stays_unit() {
        // 0.01% of 65536 squared is 429497; the CORDIC error is far below.
        for degrees in 0..DEGREES {
            let angle = Angle::from_degrees(degrees);
            let sin = i64::from(angle.sin().0);
            let cos = i64::from(angle.cos().0);
            let norm = sin
                .checked_mul(sin)
                .and_then(|ss| cos.checked_mul(cos).and_then(|cc| ss.checked_add(cc)))
                .unwrap();
            assert!(
                norm.abs_diff(0x1_0000_0000_i64) <= 429_497,
                "unit circle at {degrees}: {norm}"
            );
        }
    }

    #[test]
    fn tangent_is_a_quotient_with_a_vertical_guard() {
        assert_eq!(Angle::EAST.tan(), Some(Fixed::ZERO), "tan 0");
        assert_eq!(Angle::from_degrees(45).tan(), Some(Fixed::ONE), "tan 45");
        assert_eq!(
            Angle::from_degrees(135).tan(),
            Some(Fixed::ONE.neg()),
            "tan 135"
        );
        assert_eq!(Angle::NORTH.tan(), None, "tan 90 is guarded");
        assert_eq!(Angle::SOUTH.tan(), None, "tan 270 is guarded");
        assert!(Angle::from_degrees(60).tan().is_some(), "tan 60 exists");
    }

    #[test]
    fn atan2_matches_the_compass() {
        // (dy, dx, expected) for the eight principal directions.
        let cases = [
            (0_i64, 1_i64, Angle::EAST),
            (1, 1, Angle(0x2000_0000)),
            (1, 0, Angle::NORTH),
            (1, -1, Angle(0x6000_0000)),
            (0, -1, Angle::WEST),
            (-1, -1, Angle(0xA000_0000)),
            (-1, 0, Angle::SOUTH),
            (-1, 1, Angle(0xE000_0000)),
        ];
        for (dy, dx, expected) in cases {
            assert!(
                atan2(dy, dx).0.abs_diff(expected.0) <= BAM_TOLERANCE,
                "atan2({dy}, {dx}) near {expected:?}"
            );
        }
        assert_eq!(atan2(0, 0), Angle::EAST, "the zero vector is east");
    }

    #[test]
    fn atan2_handles_ratios_and_huge_inputs() {
        // The vector (3, 4) points at 53.13 degrees.
        let diagonal = atan2(4, 3);
        assert!(
            diagonal.0.abs_diff(Angle::from_millidegrees(53_130).0) <= BAM_TOLERANCE,
            "atan2(4, 3) is 53.13 degrees"
        );
        let huge = 0x2000_0000_0000_0000_i64; // i64::MAX / 4, rounded up
        assert!(
            atan2(huge, huge).0.abs_diff(0x2000_0000) <= BAM_TOLERANCE,
            "huge quadrant-I diagonal is 45 degrees"
        );
        let neg_huge = huge.checked_neg().unwrap();
        assert!(
            atan2(neg_huge, huge).0.abs_diff(0xE000_0000) <= BAM_TOLERANCE,
            "huge quadrant-IV diagonal is 315 degrees"
        );
        assert!(
            atan2(huge, neg_huge).0.abs_diff(0x6000_0000) <= BAM_TOLERANCE,
            "huge quadrant-II diagonal is 135 degrees"
        );
        // The raw minimum cannot be negated exactly; it saturates and the
        // direction still lands.
        assert!(
            atan2(0, i64::MIN).0.abs_diff(Angle::WEST.0) <= BAM_TOLERANCE,
            "atan2(0, i64::MIN) is west"
        );
        assert!(
            atan2(i64::MIN, i64::MIN).0.abs_diff(0xA000_0000) <= BAM_TOLERANCE,
            "atan2 of both minima is 225 degrees"
        );
        assert!(
            atan2(i64::MIN, 0).0.abs_diff(Angle::SOUTH.0) <= BAM_TOLERANCE,
            "atan2(MIN, 0) is south"
        );
    }

    #[test]
    fn hypot_is_exact_on_integer_and_large_vectors() {
        assert_eq!(hypot(3, 4), 5, "3-4-5");
        assert_eq!(hypot(-5, 12), 13, "5-12-13");
        assert_eq!(hypot(0, 0), 0, "zero vector");
        assert_eq!(
            hypot(0, -0x0020_0000_0000_0000),
            0x0020_0000_0000_0000,
            "axis"
        );
        assert_eq!(
            hypot(i64::MIN, i64::MIN),
            0xB504_F333_F9DE_6484_u64,
            "floor of sqrt(2) times 2^63"
        );
    }

    #[test]
    fn rotate_point_rotates_landmarks_exactly() {
        assert_eq!(rotate_point(100, 0, Angle::NORTH), (0, 100), "90 degrees");
        assert_eq!(
            rotate_point(100, 0, Angle::from_degrees(45)),
            (71, 71),
            "45 degrees rounds up"
        );
        assert_eq!(rotate_point(100, 0, Angle::WEST), (-100, 0), "180 degrees");
        assert_eq!(rotate_point(100, 0, Angle::SOUTH), (0, -100), "270 degrees");
        assert_eq!(rotate_point(123, 45, Angle::EAST), (123, 45), "no rotation");
    }
}
