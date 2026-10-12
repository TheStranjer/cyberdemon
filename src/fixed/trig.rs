//! Integer `CORDIC`: the engine behind `Angle::sin`, `Angle::cos` and
//! `atan2`.
//!
//! Rotation mode turns the vector (`K`, `0`) by a signed angle to read off
//! sine and cosine; vector mode turns an arbitrary vector down onto the
//! `+x` axis, accumulating its angle. Intermediates are `Q30` so fifteen
//! bits of headroom absorb the `CORDIC` growth factor (below `2`); inputs
//! are scaled under `2^31` first, so nothing leaves `i64`.

use super::Fixed;
use super::angle::Angle;
use crate::fixed::FRACBITS;

/// `atan(2^-i)` in `BAM`, `i = 0..=30`, computed offline. The `BAM` table
/// entry for `i = 31` is `0` and contributes nothing, so it is dropped.
const ATAN_STEPS: [i32; 31] = [
    0x2000_0000,
    0x12E4_051E,
    0x09FB_385B,
    0x0511_11D4,
    0x028B_0D43,
    0x0145_D7E1,
    0x00A2_F61E,
    0x0051_7C55,
    0x0028_BE53,
    0x0014_5F2F,
    0x000A_2F98,
    0x0005_17CC,
    0x0002_8BE6,
    0x0001_45F3,
    0x0000_A2FA,
    0x0000_517D,
    0x0000_28BE,
    0x0000_145F,
    0x0000_0A30,
    0x0000_0518,
    0x0000_028C,
    0x0000_0146,
    0x0000_00A3,
    0x0000_0051,
    0x0000_0029,
    0x0000_0014,
    0x0000_000A,
    0x0000_0005,
    0x0000_0003,
    0x0000_0001,
    0x0000_0001,
];

/// `CORDIC` gain `K = 0.6072529350088813` in `Q30`: the starting `x` of
/// rotation mode, making the outputs come out near unit length.
const GAIN_Q30: i64 = 0x26DD_3B6A;

/// Rounding bias for the `Q30` to 16.16 shift: half of `2^14`.
const Q30_ROUND: i64 = 0x2000;

/// `Q30` to 16.16: the bits dropped by the final shift.
const Q30_SHIFT: u32 = 30 - FRACBITS;

/// 90 degrees as a signed `BAM`, the `CORDIC` rotation-mode range limit.
const QUARTER_TURN: i32 = 0x4000_0000;

/// Negated [`QUARTER_TURN`].
const NEG_QUARTER_TURN: i32 = -0x4000_0000;

/// 180 degrees for signed `BAM` arithmetic (raw [`i32::MIN`]).
const HALF_TURN: i32 = i32::MIN;

/// `(cos, sin)` of `angle` in 16.16, from `CORDIC` rotation mode.
///
/// The angle is first reduced into `[-90, +90]` degrees by rotating 180
/// degrees and remembering to negate both results, keeping `z` inside the
/// table's convergent range.
pub(crate) fn cos_sin(angle: Angle) -> (Fixed, Fixed) {
    let mut reduced = i32::from_le_bytes(angle.0.to_le_bytes());
    let flipped = !(NEG_QUARTER_TURN..=QUARTER_TURN).contains(&reduced);
    if flipped {
        reduced = reduced.wrapping_add(HALF_TURN);
    }
    let mut x = GAIN_Q30;
    let mut y = 0_i64;
    let mut z = i64::from(reduced);
    let mut shift: u32 = 0;
    for &step in &ATAN_STEPS {
        let delta = i64::from(step);
        let previous_x = x;
        if z >= 0 {
            x = x.wrapping_sub(y.wrapping_shr(shift));
            y = y.wrapping_add(previous_x.wrapping_shr(shift));
            z = z.wrapping_sub(delta);
        } else {
            x = x.wrapping_add(y.wrapping_shr(shift));
            y = y.wrapping_sub(previous_x.wrapping_shr(shift));
            z = z.wrapping_add(delta);
        }
        shift = shift.wrapping_add(1);
    }
    let cos = round_to_fixed(x);
    let sin = round_to_fixed(y);
    if flipped {
        (Fixed(cos.wrapping_neg()), Fixed(sin.wrapping_neg()))
    } else {
        (Fixed(cos), Fixed(sin))
    }
}

/// The `BAM` direction of the vector `(dx, dy)`, from `CORDIC` vector
/// mode: the vector is scaled near `Q30` and rotated onto `+x` while the
/// accumulated rotation tracks its angle. `(0, 0)` yields east.
pub(crate) fn vector(dx: i64, dy: i64) -> Angle {
    let (flipped, start_x, start_y) = if dx < 0 {
        // Rotate the vector by 180 degrees and solve the mirrored one;
        // negating the raw minimum saturates to the maximum, a direction
        // error of one part in 2^63.
        (
            true,
            dx.checked_neg().unwrap_or(i64::MAX),
            dy.checked_neg().unwrap_or(i64::MAX),
        )
    } else {
        (false, dx, dy)
    };
    let magnitude = start_x.unsigned_abs().max(start_y.unsigned_abs());
    if magnitude == 0 {
        return Angle::EAST;
    }
    let (mut x, mut y) = scale_to_q30(start_x, start_y, magnitude);
    let mut z = 0_i32;
    let mut shift: u32 = 0;
    for &step in &ATAN_STEPS {
        let previous_x = x;
        if y > 0 {
            x = x.wrapping_add(y.wrapping_shr(shift));
            y = y.wrapping_sub(previous_x.wrapping_shr(shift));
            z = z.wrapping_add(step);
        } else {
            x = x.wrapping_sub(y.wrapping_shr(shift));
            y = y.wrapping_add(previous_x.wrapping_shr(shift));
            z = z.wrapping_sub(step);
        }
        shift = shift.wrapping_add(1);
    }
    let angle = Angle(u32::from_le_bytes(z.to_le_bytes()));
    if flipped {
        angle.wrapping_add(Angle::WEST)
    } else {
        angle
    }
}

/// Shifts both components so the larger magnitude lands in `[2^30, 2^31)`,
/// keeping the ratio that `CORDIC` iterates on.
const fn scale_to_q30(x: i64, y: i64, magnitude: u64) -> (i64, i64) {
    // Bit 63 - leading_zeros is the magnitude's top bit; move it to bit 30.
    let zeros = magnitude.leading_zeros();
    if zeros > 33 {
        let left = zeros.wrapping_sub(33);
        (x.wrapping_shl(left), y.wrapping_shl(left))
    } else {
        let right = 33_u32.wrapping_sub(zeros);
        (x.wrapping_shr(right), y.wrapping_shr(right))
    }
}

/// A `Q30` `CORDIC` output as an `i32` raw 16.16 value, rounded.
fn round_to_fixed(value: i64) -> i32 {
    let scaled = value.wrapping_add(Q30_ROUND).wrapping_shr(Q30_SHIFT);
    i32::try_from(scaled).unwrap_or(if scaled >= 0 { i32::MAX } else { i32::MIN })
}
