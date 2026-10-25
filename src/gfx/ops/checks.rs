//! The helpers every operation module shares: the `ops[i].field` argument
//! error and its range guards, the row-major pixel reader and the two
//! rounded integer divisions the samplers and the blur count with.

use crate::error::CyberError;
use crate::fixed::div_round;
use crate::gfx::ops::MAX_OP_DIMENSION;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// `InvalidArgument` reporting field `field` of op `index`; `expectation`
/// completes the sentence "ops[i].field must ...".
pub(crate) fn arg_error(index: usize, field: &str, expectation: &str) -> CyberError {
    CyberError::InvalidArgument(format!("ops[{index}].{field} must {expectation}"))
}

/// Rejects a pixel-dimension parameter outside `1..=4096`.
pub(crate) fn check_op_dimension(index: usize, field: &str, value: u32) -> Result<(), CyberError> {
    if value == 0 || value > MAX_OP_DIMENSION {
        return Err(arg_error(
            index,
            field,
            &format!("be within 1..={MAX_OP_DIMENSION}, got {value}"),
        ));
    }
    Ok(())
}

/// Rejects an `i32` parameter outside the inclusive `bounds`.
pub(crate) fn check_i32_range(
    index: usize,
    field: &str,
    value: i32,
    bounds: (i32, i32),
) -> Result<(), CyberError> {
    if bounds.0 <= value && value <= bounds.1 {
        return Ok(());
    }
    Err(arg_error(
        index,
        field,
        &format!("be within {}..={}, got {value}", bounds.0, bounds.1),
    ))
}

/// The pixel at `index` of a row-major `RGBA` byte buffer, `None` when out
/// of bounds.
pub(crate) fn pixel_at(pixels: &[u8], index: usize) -> Option<[u8; 4]> {
    let start = index.checked_mul(BYTES_PER_PIXEL)?;
    let chunk = pixels.get(start..start.checked_add(BYTES_PER_PIXEL)?)?;
    Some(to_pixel(chunk))
}

/// `numerator / denominator` rounded to nearest; `0` for a zero
/// denominator, saturating results into `u64`.
pub(crate) fn rounded_div(numerator: u64, denominator: u64) -> u64 {
    div_round(i128::from(numerator), i128::from(denominator))
        .and_then(|quotient| u64::try_from(quotient).ok())
        .unwrap_or_default()
}

/// `numerator / denominator` rounded up; saturates for a zero denominator.
pub(crate) fn div_ceil(numerator: u64, denominator: u64) -> u64 {
    let Some(whole) = numerator.checked_div(denominator) else {
        return u64::MAX;
    };
    if numerator.checked_rem(denominator).unwrap_or_default() == 0 {
        whole
    } else {
        whole.saturating_add(1)
    }
}
