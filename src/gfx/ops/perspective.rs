//! Perspective correction: the corner-quad-to-rectangle warp behind
//! [`crate::gfx::ops::ImageOp::PerspectiveCorrect`].
//!
//! Every destination pixel centre is mapped back to a source position
//! through the inverse homography: the eight projective coefficients are
//! solved from the four rectangle-corner to quad-corner correspondences
//! (the `DLT` system) by partial-pivoted Gaussian elimination in `16.16`
//! fixed point, every product and quotient rounded through
//! [`crate::fixed`] helpers over `i128` intermediates. Sampling is the shared
//! bilinear, alpha-weighted [`bilinear`] tap blend `resample` scales through;
//! a source position outside the picture reads as transparent black.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::CyberError;
use crate::fixed::{FRACUNIT, Fixed, mul_div_round};
use crate::gfx::filter::{bilinear, last_centre};
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{arg_error, check_op_dimension};
use crate::gfx::rgba_byte_count;

/// The `DLT` system's size: eight unknown projective coefficients.
const SYSTEM: usize = 8;
/// The corner-index triples deciding non-degeneracy.
const CORNER_TRIPLES: [(usize, usize, usize); 4] = [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)];
/// The rectangle's own corners in normalised destination coordinates,
/// in the order the quad's corners are given.
const DEST_CORNERS: [(Fixed, Fixed); 4] = [
    (Fixed::ZERO, Fixed::ZERO),
    (Fixed::ONE, Fixed::ZERO),
    (Fixed::ONE, Fixed::ONE),
    (Fixed::ZERO, Fixed::ONE),
];
/// No corner coordinate may exceed this once the power-of-two scaling is
/// applied, keeping every elimination product inside `16.16`.
const SCALED_LIMIT: i64 = 64;
/// How far a solved source position may cross a picture edge in `16.16`
/// units and still count as sitting on it (solver rounding noise).
const EDGE_SLACK: i64 = 16;
/// The pixel a source position outside the picture becomes.
const TRANSPARENT: [u8; 4] = [0_u8; 4];
/// The `arg_error` expectation naming a refused corner set.
const DEGENERATE_EXPECTATION: &str =
    "form a non-degenerate quad (four distinct corners, no three on one line)";

/// One source-plane point of
/// [`crate::gfx::ops::ImageOp::PerspectiveCorrect`], in pixel
/// coordinates; corners may be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct Corner {
    /// Horizontal pixel position.
    pub x: i32,
    /// Vertical pixel position.
    pub y: i32,
}

/// Parameters of [`crate::gfx::ops::ImageOp::PerspectiveCorrect`].
pub(crate) struct PerspectiveSpec {
    /// The quad's source corners: top-left, top-right, bottom-right,
    /// bottom-left.
    pub(crate) corners: [Corner; 4],
    /// Destination width in pixels.
    pub(crate) width: u32,
    /// Destination height in pixels.
    pub(crate) height: u32,
}

/// A solved rectangle-to-quad homography: normalised destination
/// coordinates map to [`Homography::shift`]-scaled source pixel
/// positions through `x_num` / `depth` and `y_num` / `depth`.
struct Homography {
    /// The `x` numerator coefficients (of `u`, `v` and `1`).
    x_num: [Fixed; 3],
    /// The `y` numerator coefficients (of `u`, `v` and `1`).
    y_num: [Fixed; 3],
    /// The denominator coefficients (of `u`, `v` and `1`).
    depth: [Fixed; 3],
    /// The corners were scaled down by `2^shift` before solving.
    shift: u32,
}

/// One augmented `DLT` row: eight coefficients and the right-hand side
/// at index [`SYSTEM`].
type Row = [Fixed; SYSTEM + 1];

/// The whole augmented system.
type System = [Row; SYSTEM];

/// Warps the source quad `spec.corners` onto a `spec.width` x
/// `spec.height` rectangle.
///
/// # Errors
///
/// Reports `ops[index].width/height` for an out-of-range target edge and
/// `ops[index].corners` when no homography maps the rectangle onto the
/// quad (a repeated or collinear corner).
pub(crate) fn correct(
    img: &RgbaImage,
    index: usize,
    spec: &PerspectiveSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    check_op_dimension(index, "width", spec.width)?;
    check_op_dimension(index, "height", spec.height)?;
    let Some(map) = solve(&spec.corners) else {
        return Err(arg_error(index, "corners", DEGENERATE_EXPECTATION));
    };
    let out = warp(img, &map, spec.width, spec.height);
    Ok((out, step(spec)))
}

/// The op's report entry: the target shape and the corner pairs.
fn step(spec: &PerspectiveSpec) -> Value {
    let corners: Vec<[i32; 2]> = spec
        .corners
        .iter()
        .map(|corner| [corner.x, corner.y])
        .collect();
    json!({
        "op": "perspective_correct",
        "width": spec.width,
        "height": spec.height,
        "corners": corners,
    })
}

/// Solves the rectangle-to-quad homography, or `None` for a corner set
/// that admits none (repeated corners, three on one line, or a system
/// the fixed-point elimination cannot finish).
fn solve(corners: &[Corner; 4]) -> Option<Homography> {
    if is_degenerate(corners) {
        return None;
    }
    let shift = scaling_shift(corners);
    let system = build_system(corners, shift)?;
    let coefficients = back_substitute(&eliminate(system)?)?;
    Some(homography(&coefficients, shift))
}

/// Whether any three corners sit on one line; the cross product also
/// vanishes for every triple containing a repeated corner.
fn is_degenerate(corners: &[Corner; 4]) -> bool {
    CORNER_TRIPLES
        .iter()
        .any(|triple| zero_crossing(corners, triple))
}

/// Whether the named corner triple is degenerate.
fn zero_crossing(corners: &[Corner; 4], triple: &(usize, usize, usize)) -> bool {
    let (ia, ib, ic) = *triple;
    let (Some(first), Some(second), Some(third)) =
        (corners.get(ia), corners.get(ib), corners.get(ic))
    else {
        return true;
    };
    cross_product(*first, *second, *third) == 0_i128
}

/// The `(b - a) x (c - a)` cross product of three corners in `i128`,
/// exact for every `i32` coordinate pair.
fn cross_product(a: Corner, b: Corner, c: Corner) -> i128 {
    let delta = |from: Corner, to: Corner| {
        (
            i128::from(to.x).checked_sub(i128::from(from.x)),
            i128::from(to.y).checked_sub(i128::from(from.y)),
        )
    };
    let (Some(first_x), Some(first_y)) = delta(a, b) else {
        return 0_i128;
    };
    let (Some(second_x), Some(second_y)) = delta(a, c) else {
        return 0_i128;
    };
    let left = first_x.checked_mul(second_y).unwrap_or(0_i128);
    let right = first_y.checked_mul(second_x).unwrap_or(0_i128);
    left.checked_sub(right).unwrap_or(0_i128)
}

/// The power-of-two exponent scaling the widest corner coordinate down
/// to [`SCALED_LIMIT`].
fn scaling_shift(corners: &[Corner; 4]) -> u32 {
    let widest = corners
        .iter()
        .flat_map(|corner| [i64::from(corner.x).abs(), i64::from(corner.y).abs()])
        .max()
        .unwrap_or(0_i64);
    let mut shift = 0_u32;
    while widest
        .checked_shr(shift)
        .is_some_and(|edge| edge > SCALED_LIMIT)
    {
        shift = shift.saturating_add(1);
    }
    shift
}

/// Scales one corner coordinate into the elimination's range.
fn scaled_coord(value: i32, shift: u32) -> Option<Fixed> {
    Fixed::from_ratio(i64::from(value), 1_i64.checked_shl(shift)?)
}

/// The eight augmented rows the four correspondences impose: each corner
/// contributes one `x` and one `y` row.
fn build_system(corners: &[Corner; 4], shift: u32) -> Option<System> {
    let scaled: Vec<(Fixed, Fixed)> = corners
        .iter()
        .filter_map(|corner| {
            Some((
                scaled_coord(corner.x, shift)?,
                scaled_coord(corner.y, shift)?,
            ))
        })
        .collect();
    let rows: Vec<Row> = scaled
        .iter()
        .zip(DEST_CORNERS)
        .flat_map(|(&source, dest)| dlt_rows(dest, source))
        .collect();
    rows.try_into().ok()
}

/// The two `DLT` rows of one corner correspondence:
/// `[u, v, 1, 0, 0, 0, -xu, -xv | x]` and the same for `y`. The products
/// are exact because the destination coordinates are `0` or `1`.
fn dlt_rows((du, dv): (Fixed, Fixed), (sx, sy): (Fixed, Fixed)) -> [Row; 2] {
    [
        [
            du,
            dv,
            Fixed::ONE,
            Fixed::ZERO,
            Fixed::ZERO,
            Fixed::ZERO,
            negated(du, sx),
            negated(dv, sx),
            sx,
        ],
        [
            Fixed::ZERO,
            Fixed::ZERO,
            Fixed::ZERO,
            du,
            dv,
            Fixed::ONE,
            negated(du, sy),
            negated(dv, sy),
            sy,
        ],
    ]
}

/// `-a * b` for the system's exact small products.
fn negated(a: Fixed, b: Fixed) -> Fixed {
    a.saturating_mul(b).neg()
}

/// Forward elimination with partial pivoting; `None` when a column has
/// no pivot left (a singular system).
fn eliminate(mut system: System) -> Option<System> {
    for column in 0..SYSTEM {
        let pivot = largest_row(&system, column)?;
        system.swap(pivot, column);
        let reference: Row = *system.get(column)?;
        let divisor = *reference.get(column)?;
        for row in system.iter_mut().skip(column.checked_add(1)?) {
            reduce_row(row, divisor, &reference, column)?;
        }
    }
    Some(system)
}

/// The row at or below `column` with the largest magnitude in `column`,
/// or `None` when they are all zero.
fn largest_row(system: &System, column: usize) -> Option<usize> {
    let (index, magnitude) = system
        .iter()
        .enumerate()
        .skip(column)
        .map(|(row_index, row)| {
            (
                row_index,
                row.get(column).map_or(0_i32, |cell| cell.abs().0),
            )
        })
        .max_by_key(|&(_, size)| size)?;
    (magnitude > 0_i32).then_some(index)
}

/// Subtracts the pivot-scaled reference row from `row`, zeroing its
/// `column` cell; `None` when an operation leaves `16.16`.
fn reduce_row(row: &mut Row, divisor: Fixed, reference: &Row, column: usize) -> Option<()> {
    let factor = div_fixed(*row.get(column)?, divisor)?;
    for (cell, &pivoted) in row.iter_mut().zip(reference.iter()) {
        *cell = cell.checked_sub(mul_fixed(factor, pivoted)?)?;
    }
    Some(())
}

/// Solves the upper-triangular system from the last row upward; `None`
/// when back-substitution leaves `16.16` or a zero pivot stands.
fn back_substitute(system: &System) -> Option<[Fixed; SYSTEM]> {
    let mut solution = [Fixed::ZERO; SYSTEM];
    for column in (0..SYSTEM).rev() {
        let row = system.get(column)?;
        let mut total = *row.get(SYSTEM)?;
        let tail = row.get(column.checked_add(1)?..SYSTEM)?;
        for (offset, coefficient) in tail.iter().enumerate() {
            let index = column.checked_add(1)?.checked_add(offset)?;
            total = total.checked_sub(mul_fixed(*coefficient, *solution.get(index)?)?)?;
        }
        *solution.get_mut(column)? = div_fixed(total, *row.get(column)?)?;
    }
    Some(solution)
}

/// Groups the solved coefficients into the evaluation form.
const fn homography(coefficients: &[Fixed; SYSTEM], shift: u32) -> Homography {
    let [x_u, x_v, x_c, y_u, y_v, y_c, d_u, d_v] = *coefficients;
    Homography {
        x_num: [x_u, x_v, x_c],
        y_num: [y_u, y_v, y_c],
        depth: [d_u, d_v, Fixed::ONE],
        shift,
    }
}

/// Runs the warp over the whole destination rectangle.
fn warp(img: &RgbaImage, map: &Homography, width: u32, height: u32) -> RgbaImage {
    let mut pixels = Vec::with_capacity(rgba_byte_count(width, height).unwrap_or(0));
    for row in 0..height {
        let v = unit_position(height, row);
        for column in 0..width {
            let u = unit_position(width, column);
            pixels.extend_from_slice(&warped_pixel(img, map, u, v));
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// The normalised coordinate of destination pixel `index` on an axis of
/// `length` pixels: `0` at the first, `1` at the last; a one-pixel axis
/// sits at `0`.
fn unit_position(length: u32, index: u32) -> Fixed {
    let Some(whole) = length.checked_sub(1) else {
        return Fixed::ZERO;
    };
    Fixed::from_ratio(i64::from(index), i64::from(whole)).unwrap_or(Fixed::ZERO)
}

/// One destination pixel: the source sample at its projective position,
/// or transparent black when no position exists.
fn warped_pixel(img: &RgbaImage, map: &Homography, u: Fixed, v: Fixed) -> [u8; 4] {
    let Some((sx, sy)) = source_position(map, u, v) else {
        return TRANSPARENT;
    };
    sample(img, sx, sy)
}

/// The `16.16` source pixel-centre coordinates of destination position
/// `(u, v)`, or `None` when the evaluation leaves the range or the
/// denominator vanishes.
fn source_position(map: &Homography, u: Fixed, v: Fixed) -> Option<(i64, i64)> {
    let depth = weighted_sum(map.depth, u, v)?;
    let sx = div_fixed(weighted_sum(map.x_num, u, v)?, depth)?;
    let sy = div_fixed(weighted_sum(map.y_num, u, v)?, depth)?;
    Some((to_pixels(sx, map.shift)?, to_pixels(sy, map.shift)?))
}

/// `a * u + b * v + c` with rounded products; `None` when it leaves
/// `16.16`.
fn weighted_sum(terms: [Fixed; 3], u: Fixed, v: Fixed) -> Option<Fixed> {
    let [cu, cv, cc] = terms;
    mul_fixed(cu, u)?
        .checked_add(mul_fixed(cv, v)?)?
        .checked_add(cc)
}

/// Re-applies the corner scaling the solve divided out.
fn to_pixels(value: Fixed, shift: u32) -> Option<i64> {
    let scaled = i128::from(value.0).checked_shl(shift)?;
    i64::try_from(scaled).ok()
}

/// The rounded `16.16` product `left * right`, or `None` when it leaves
/// the range.
fn mul_fixed(left: Fixed, right: Fixed) -> Option<Fixed> {
    let raw = mul_div_round(i64::from(left.0), i64::from(right.0), i64::from(FRACUNIT))?;
    Some(Fixed(i32::try_from(raw).ok()?))
}

/// The rounded `16.16` quotient, or `None` when the divisor is zero or
/// the result leaves the range.
fn div_fixed(numerator: Fixed, denominator: Fixed) -> Option<Fixed> {
    Fixed::from_ratio(i64::from(numerator.0), i64::from(denominator.0))
}

/// The bilinear, alpha-weighted source sample at the `16.16` position
/// `(x, y)`: the shared tap blend over the picture, pulled onto an edge the
/// solver's rounding overshot by no more than [`EDGE_SLACK`], and
/// transparent black when it still lies outside.
fn sample(img: &RgbaImage, x: i64, y: i64) -> [u8; 4] {
    let at_x = clamp_edge(x, last_centre(img.width));
    let at_y = clamp_edge(y, last_centre(img.height));
    bilinear(img, at_x, at_y).unwrap_or(TRANSPARENT)
}

/// Pulls a position sitting within [`EDGE_SLACK`] units of the picture
/// onto the edge it overshot; anything further stays outside.
fn clamp_edge(value: i64, last: i64) -> i64 {
    let near = value >= -EDGE_SLACK && value <= last.checked_add(EDGE_SLACK).unwrap_or(i64::MAX);
    if near { value.clamp(0, last) } else { value }
}

#[cfg(test)]
mod tests {
    use super::{Corner, PerspectiveSpec, correct};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::to_pixel;

    /// One corner, for compact quad literals.
    fn corner(x: i32, y: i32) -> Corner {
        Corner { x, y }
    }

    /// A spec over four corners.
    fn spec(corners: [Corner; 4], width: u32, height: u32) -> PerspectiveSpec {
        PerspectiveSpec {
            corners,
            width,
            height,
        }
    }

    /// A 4x4 colour-block image: pixel `p` is opaque grey `17 * p`.
    fn blocks() -> RgbaImage {
        let pixels: Vec<u8> = (0..16_u32)
            .flat_map(|p| {
                let shade = u8::try_from(p.checked_mul(17).expect("test fits")).expect("test fits");
                [shade, shade, shade, 255_u8]
            })
            .collect();
        RgbaImage::new(4, 4, pixels).expect("4x4 RGBA8 is valid")
    }

    /// The 8x8 four-quadrant picture: red top-left, lime top-right,
    /// blue bottom-right, black bottom-left.
    fn quadrants() -> RgbaImage {
        let pixels: Vec<u8> = (0..8_u32)
            .flat_map(|y| (0..8_u32).flat_map(move |x| quadrant_pixel(x, y)))
            .collect();
        RgbaImage::new(8, 8, pixels).expect("8x8 RGBA8 is valid")
    }

    /// The quadrant colour of pixel `(x, y)`.
    fn quadrant_pixel(x: u32, y: u32) -> [u8; 4] {
        match (x < 4, y < 4) {
            (true, true) => [255_u8, 0, 0, 255],
            (false, true) => [0_u8, 255, 0, 255],
            (false, false) => [0_u8, 0, 255, 255],
            (true, false) => [0_u8, 0, 0, 255],
        }
    }

    /// The message of the `InvalidArgument` this result must hold.
    fn rejection(result: Result<(RgbaImage, serde_json::Value), CyberError>) -> String {
        let CyberError::InvalidArgument(text) = result.expect_err("must be invalid") else {
            panic!("expected InvalidArgument");
        };
        text
    }

    /// An image's pixels as arrays, for the golden comparisons.
    fn picture(img: &RgbaImage) -> Vec<[u8; 4]> {
        img.pixels.chunks_exact(4_usize).map(to_pixel).collect()
    }

    /// The trapezoid of `corners(0, 0)..(5, 1)..(5, 5)..(0, 7)` warped onto
    /// a `4 x 4` rectangle, pixel by pixel.
    const TRAPEZOID_4X4: [[u8; 4]; 16] = [
        [255, 0, 0, 255],
        [255, 0, 0, 255],
        [28, 227, 0, 255],
        [0, 255, 0, 255],
        [255, 0, 0, 255],
        [255, 0, 0, 255],
        [28, 227, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 0, 255],
        [0, 0, 0, 255],
        [3, 25, 201, 255],
        [0, 85, 170, 255],
        [0, 0, 0, 255],
        [0, 0, 0, 255],
        [0, 0, 227, 255],
        [0, 0, 255, 255],
    ];

    /// A stronger skew of the same quad onto a `5 x 3` rectangle.
    const SKEW_5X3: [[u8; 4]; 15] = [
        [255, 0, 0, 255],
        [255, 0, 0, 255],
        [143, 112, 0, 255],
        [0, 255, 0, 255],
        [0, 255, 0, 255],
        [124, 0, 0, 255],
        [187, 0, 0, 255],
        [147, 105, 1, 255],
        [0, 255, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 0, 255],
        [0, 0, 0, 255],
        [0, 0, 98, 255],
        [0, 0, 255, 255],
        [0, 0, 255, 255],
    ];

    #[test]
    fn the_warped_samples_hold_their_golden_bytes() {
        let source = quadrants();
        let (trapezoid, _step) = correct(
            &source,
            0,
            &spec(
                [corner(0, 0), corner(5, 1), corner(5, 5), corner(0, 7)],
                4,
                4,
            ),
        )
        .expect("the trapezoid is sound");
        assert_eq!(picture(&trapezoid), TRAPEZOID_4X4.to_vec(), "the trapezoid");
        let (skew, _other_step) = correct(
            &source,
            0,
            &spec(
                [corner(1, 1), corner(6, 0), corner(7, 6), corner(0, 7)],
                5,
                3,
            ),
        )
        .expect("the skew is sound");
        assert_eq!(picture(&skew), SKEW_5X3.to_vec(), "the skew");
    }

    #[test]
    fn the_identity_quad_reproduces_the_source_exactly() {
        let source = blocks();
        let (out, step) = correct(
            &source,
            0,
            &spec(
                [corner(0, 0), corner(3, 0), corner(3, 3), corner(0, 3)],
                4,
                4,
            ),
        )
        .expect("the identity quad is sound");
        assert_eq!((out.width, out.height), (4, 4), "the target shape");
        assert_eq!(
            out.pixels, source.pixels,
            "every pixel lands on its own centre"
        );
        assert_eq!(step["op"], "perspective_correct", "the documented name");
        assert_eq!(step["width"], 4_i32, "the width echoes");
        assert_eq!(step["height"], 4_i32, "the height echoes");
        assert_eq!(
            step["corners"],
            serde_json::json!([
                [0_i32, 0_i32],
                [3_i32, 0_i32],
                [3_i32, 3_i32],
                [0_i32, 3_i32]
            ]),
            "the corner pairs echo"
        );
    }

    #[test]
    fn a_trapezoid_skew_puts_corner_colours_at_the_rectangle_corners() {
        let source = quadrants();
        // The left edge (x = 0, y 0..=7) is taller than the right
        // (x = 5, y 1..=5): a trapezoid under a true perspective map.
        let (out, _step) = correct(
            &source,
            1,
            &spec(
                [corner(0, 0), corner(5, 1), corner(5, 5), corner(0, 7)],
                4,
                4,
            ),
        )
        .expect("the trapezoid is sound");
        assert_eq!((out.width, out.height), (4, 4), "the target shape");
        assert_eq!(out.get(0, 0), source.get(0, 0), "the top-left corner");
        assert_eq!(out.get(3, 0), source.get(5, 1), "the top-right corner");
        assert_eq!(out.get(3, 3), source.get(5, 5), "the bottom-right corner");
        assert_eq!(out.get(0, 3), source.get(0, 7), "the bottom-left corner");
    }

    #[test]
    fn a_position_outside_the_picture_samples_transparent_black() {
        let source = quadrants();
        // The left corners sit four pixels left of the picture's edge.
        let (out, _step) = correct(
            &source,
            0,
            &spec(
                [corner(-4, 0), corner(5, 0), corner(5, 7), corner(-4, 7)],
                4,
                1,
            ),
        )
        .expect("the quad is sound");
        assert_eq!(
            out.get(0, 0),
            Some([0, 0, 0, 0]),
            "outside is transparent black"
        );
        assert_eq!(out.get(3, 0), source.get(5, 0), "the inside corner samples");
    }

    #[test]
    fn repeated_and_collinear_quads_are_refused() {
        let source = blocks();
        let quads = [
            [corner(0, 0), corner(0, 0), corner(3, 3), corner(0, 3)],
            [corner(0, 0), corner(2, 0), corner(4, 0), corner(0, 3)],
            [
                corner(0, 0),
                corner(0x100_0000, 0),
                corner(0x80_0000, 1),
                corner(0xC0_0000, 1),
            ],
        ];
        for corners in quads {
            let message = rejection(correct(&source, 4, &spec(corners, 4, 4)));
            assert!(
                message.contains("ops[4].corners must form a non-degenerate quad"),
                "the refusal names the field and the rule: {message}"
            );
        }
    }

    #[test]
    fn a_corner_set_collapsing_towards_a_line_still_anchors_its_first_corner() {
        let source = quadrants();
        // The scaling rounds three corners nearly onto one line: the map
        // degenerates hard, but the op is sound and pixel (0, 0) sits at
        // the corner the rows pin there exactly.
        let (out, _step) = correct(
            &source,
            0,
            &spec(
                [
                    corner(0, 0),
                    corner(0x80_0000, 1),
                    corner(0x100_0000, 1),
                    corner(0x80_0000, 100),
                ],
                4,
                4,
            ),
        )
        .expect("no three corners are exactly collinear");
        assert_eq!((out.width, out.height), (4, 4), "the target shape");
        assert_eq!(out.get(0, 0), source.get(0, 0), "the pinned corner stands");
    }

    #[test]
    fn a_bad_target_edge_names_the_op_index_and_field() {
        let source = blocks();
        let sound = [corner(0, 0), corner(3, 0), corner(3, 3), corner(0, 3)];
        let wide = rejection(correct(&source, 1, &spec(sound, 0, 4)));
        assert!(
            wide.contains("ops[1].width") && wide.contains("1..=4096"),
            "the zero width is refused by name: {wide}"
        );
        let tall = rejection(correct(&source, 2, &spec(sound, 4, 4097)));
        assert!(
            tall.contains("ops[2].height") && tall.contains("1..=4096"),
            "the oversized height is refused by name: {tall}"
        );
    }

    #[test]
    fn a_one_wide_target_samples_the_left_edge() {
        let source = blocks();
        let (out, _step) = correct(
            &source,
            0,
            &spec(
                [corner(0, 0), corner(3, 0), corner(3, 3), corner(0, 3)],
                1,
                4,
            ),
        )
        .expect("the identity quad is sound");
        assert_eq!((out.width, out.height), (1, 4), "the target shape");
        for row in 0..4_u32 {
            assert_eq!(
                out.get(0, row),
                source.get(0, row),
                "row {row} of the left edge"
            );
        }
    }
}
