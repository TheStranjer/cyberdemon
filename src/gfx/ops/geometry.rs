//! Geometric image operations.
//!
//! [`crate::gfx::ops::ImageOp::Crop`], [`crate::gfx::ops::ImageOp::Fit`],
//! [`crate::gfx::ops::ImageOp::Rotate`] and [`crate::gfx::ops::ImageOp::Flip`]
//! as exact pixel permutations and rational-scale resamples.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::resample::resample;
use crate::gfx::ops::{
    Filter, FitMode, arg_error, check_op_dimension, div_ceil, parse_hex_colour, pixel_at,
};
use crate::gfx::{BYTES_PER_PIXEL, MAX_DIMENSION, pixel_index, rgba_byte_count};

/// Parameters of [`crate::gfx::ops::ImageOp::Crop`].
pub(crate) struct CropSpec {
    /// Left edge in the source.
    pub(crate) x: u32,
    /// Top edge in the source.
    pub(crate) y: u32,
    /// Rectangle width in pixels.
    pub(crate) width: u32,
    /// Rectangle height in pixels.
    pub(crate) height: u32,
}

/// Parameters of [`crate::gfx::ops::ImageOp::Flip`].
pub(crate) struct FlipSpec {
    /// Mirror left-to-right.
    pub(crate) horizontal: bool,
    /// Mirror top-to-bottom.
    pub(crate) vertical: bool,
}

/// Parameters of [`crate::gfx::ops::ImageOp::Fit`].
pub(crate) struct FitSpec<'fit> {
    /// Target width in pixels.
    pub(crate) width: u32,
    /// Target height in pixels.
    pub(crate) height: u32,
    /// Mapping between source and target rectangle.
    pub(crate) mode: FitMode,
    /// Sampling filter for the size change.
    pub(crate) filter: Filter,
    /// `contain` canvas colour text, if given.
    pub(crate) background: Option<&'fit str>,
}

/// Extracts a rectangle that lies fully inside `img`.
///
/// # Errors
///
/// Reports `ops[index].width/height/x/y` when the rectangle is degenerate,
/// oversized or outside the image.
pub(crate) fn crop(
    img: &RgbaImage,
    index: usize,
    spec: &CropSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    check_op_dimension(index, "width", spec.width)?;
    check_op_dimension(index, "height", spec.height)?;
    check_inside(index, "x", (spec.x, spec.width), img.width)?;
    check_inside(index, "y", (spec.y, spec.height), img.height)?;
    let mut pixels = Vec::with_capacity(rgba_byte_count(spec.width, spec.height).unwrap_or(0));
    for row in 0..spec.height {
        for column in 0..spec.width {
            let source_x = spec.x.saturating_add(column);
            let source_y = spec.y.saturating_add(row);
            let source = pixel_index(img.width, source_x, source_y).unwrap_or(usize::MAX);
            pixels.extend_from_slice(&pixel_at(&img.pixels, source).unwrap_or([0_u8; 4]));
        }
    }
    Ok((
        RgbaImage::new(spec.width, spec.height, pixels)?,
        json!({"op": "crop", "width": spec.width, "height": spec.height}),
    ))
}

/// Checks that `start + span` stays inside the `limit` edge.
fn check_inside(
    index: usize,
    field: &str,
    placement: (u32, u32),
    limit: u32,
) -> Result<(), CyberError> {
    let (start, span) = placement;
    match start.checked_add(span) {
        Some(end) if end <= limit => Ok(()),
        _ => Err(arg_error(
            index,
            field,
            &format!("keep its {span}-pixel span inside the {limit}-pixel edge, got start {start}"),
        )),
    }
}

/// Rotates `img` clockwise by `quarter_turns` (`1..=3`) 90-degree turns.
pub(crate) fn rotate(
    img: &RgbaImage,
    index: usize,
    quarter_turns: u8,
) -> Result<(RgbaImage, Value), CyberError> {
    let turns = u32::from(quarter_turns);
    if turns == 0 || turns > 3 {
        return Err(arg_error(
            index,
            "quarter_turns",
            &format!("be within 1..=3 clockwise turns, got {quarter_turns}"),
        ));
    }
    let (width, height) = if turns == 2 {
        (img.width, img.height)
    } else {
        (img.height, img.width)
    };
    let mut pixels = Vec::with_capacity(img.pixels.len());
    for dst_y in 0..height {
        for dst_x in 0..width {
            pixels.extend_from_slice(&rotated_pixel(img, turns, dst_x, dst_y));
        }
    }
    Ok((
        RgbaImage {
            width,
            height,
            pixels,
        },
        json!({"op": "rotate", "quarter_turns": quarter_turns, "width": width, "height": height}),
    ))
}

/// The source pixel of destination coordinate `(dst_x, dst_y)`.
fn rotated_pixel(img: &RgbaImage, turns: u32, dst_x: u32, dst_y: u32) -> [u8; 4] {
    let last_x = img.width.saturating_sub(1);
    let last_y = img.height.saturating_sub(1);
    let (source_x, source_y) = match turns {
        1 => (dst_y, last_y.saturating_sub(dst_x)),
        2 => (last_x.saturating_sub(dst_x), last_y.saturating_sub(dst_y)),
        _ => (last_x.saturating_sub(dst_y), dst_x),
    };
    let position = pixel_index(img.width, source_x, source_y).unwrap_or(usize::MAX);
    pixel_at(&img.pixels, position).unwrap_or([0_u8; 4])
}

/// Mirrors `img` along the requested axes; a pure permutation.
pub(crate) fn flip(img: &RgbaImage, spec: &FlipSpec) -> (RgbaImage, Value) {
    let row_bytes = rgba_byte_count(img.width, 1).unwrap_or(1);
    let mut rows: Vec<&[u8]> = img.pixels.chunks_exact(row_bytes).collect();
    if spec.vertical {
        rows.reverse();
    }
    let mut pixels = Vec::with_capacity(img.pixels.len());
    for row in rows {
        append_row(&mut pixels, row, spec.horizontal);
    }
    let report = json!({
        "op": "flip",
        "horizontal": spec.horizontal,
        "vertical": spec.vertical,
        "width": img.width,
        "height": img.height,
    });
    (
        RgbaImage {
            width: img.width,
            height: img.height,
            pixels,
        },
        report,
    )
}

/// Appends one source row to the output, mirroring it when requested.
fn append_row(out: &mut Vec<u8>, row: &[u8], mirror: bool) {
    if mirror {
        for chunk in row.chunks_exact(BYTES_PER_PIXEL).rev() {
            out.extend_from_slice(chunk);
        }
    } else {
        out.extend_from_slice(row);
    }
}

/// Resizes `img` to the fitted target rectangle.
///
/// # Errors
///
/// Reports `ops[index].width/height/background` on bad parameters and
/// forwards the crop errors from `cover`.
pub(crate) fn fit(
    img: &RgbaImage,
    index: usize,
    spec: &FitSpec<'_>,
) -> Result<(RgbaImage, Value), CyberError> {
    check_op_dimension(index, "width", spec.width)?;
    check_op_dimension(index, "height", spec.height)?;
    let background = match spec.background {
        None => [0_u8; 4],
        Some(text) => match parse_hex_colour(text, true) {
            Ok(color) => color,
            Err(_) => {
                return Err(arg_error(
                    index,
                    "background",
                    &format!("be a #RRGGBBAA or #RRGGBB hex colour, got {text}"),
                ));
            }
        },
    };
    match spec.mode {
        FitMode::Stretch => Ok(fit_stretch(img, spec)),
        FitMode::Cover => fit_cover(img, index, spec),
        FitMode::Contain => Ok(fit_contain(img, spec, background)),
    }
}

/// Stretch: resample to exactly the target; no crop or padding happens.
fn fit_stretch(img: &RgbaImage, spec: &FitSpec<'_>) -> (RgbaImage, Value) {
    let out = resample(img, spec.width, spec.height, spec.filter);
    (
        out,
        json!({
            "op": "fit",
            "mode": "stretch",
            "scale_num": 1_u32,
            "scale_den": 1_u32,
            "offset_x": 0_u32,
            "offset_y": 0_u32,
        }),
    )
}

/// Cover: uniform scale until both edges reach the target, then centre-crop.
fn fit_cover(
    img: &RgbaImage,
    index: usize,
    spec: &FitSpec<'_>,
) -> Result<(RgbaImage, Value), CyberError> {
    let (num, den) = deciding_scale(img, spec, true);
    let scaled_w = as_dimension(div_ceil(u64::from(img.width).saturating_mul(num), den));
    let scaled_h = as_dimension(div_ceil(u64::from(img.height).saturating_mul(num), den));
    let big = resample(img, scaled_w, scaled_h, spec.filter);
    let offset_x = big
        .width
        .saturating_sub(spec.width)
        .checked_div(2_u32)
        .unwrap_or(0_u32);
    let offset_y = big
        .height
        .saturating_sub(spec.height)
        .checked_div(2_u32)
        .unwrap_or(0_u32);
    let rect = CropSpec {
        x: offset_x,
        y: offset_y,
        width: spec.width,
        height: spec.height,
    };
    let (out, _step) = crop(&big, index, &rect)?;
    Ok((
        out,
        json!({
            "op": "fit",
            "mode": "cover",
            "scale_num": num,
            "scale_den": den,
            "offset_x": offset_x,
            "offset_y": offset_y,
        }),
    ))
}

/// Contain: uniform scale until both edges fit, then centre on the canvas.
fn fit_contain(img: &RgbaImage, spec: &FitSpec<'_>, background: [u8; 4]) -> (RgbaImage, Value) {
    let (num, den) = deciding_scale(img, spec, false);
    let scaled_w = as_dimension(
        u64::from(img.width)
            .saturating_mul(num)
            .checked_div(den)
            .unwrap_or(0),
    );
    let scaled_h = as_dimension(
        u64::from(img.height)
            .saturating_mul(num)
            .checked_div(den)
            .unwrap_or(0),
    );
    let small = resample(img, scaled_w, scaled_h, spec.filter);
    let offset_x = spec
        .width
        .saturating_sub(small.width)
        .checked_div(2_u32)
        .unwrap_or(0_u32);
    let offset_y = spec
        .height
        .saturating_sub(small.height)
        .checked_div(2_u32)
        .unwrap_or(0_u32);
    let mut canvas = filled_canvas(spec.width, spec.height, background);
    paste(&mut canvas, offset_x, offset_y, &small);
    (
        canvas,
        json!({
            "op": "fit",
            "mode": "contain",
            "scale_num": num,
            "scale_den": den,
            "offset_x": offset_x,
            "offset_y": offset_y,
        }),
    )
}

/// The uniform scale as a `target / source` rational: the axis with the
/// larger ratio decides `cover`, the smaller decides `contain`. The ratios
/// are compared by cross-multiplication, so no division happens here.
fn deciding_scale(img: &RgbaImage, spec: &FitSpec<'_>, cover: bool) -> (u64, u64) {
    let across = u64::from(spec.width).saturating_mul(u64::from(img.height));
    let down = u64::from(spec.height).saturating_mul(u64::from(img.width));
    let width_decides = if cover {
        across >= down
    } else {
        across <= down
    };
    if width_decides {
        (u64::from(spec.width), u64::from(img.width))
    } else {
        (u64::from(spec.height), u64::from(img.height))
    }
}

/// Saturates a computed scale result into the core image dimension limit,
/// `1..=MAX_DIMENSION`: a uniform scale whose floor rounds the short edge
/// to `0` keeps it a pixel wide instead of resampling it away.
fn as_dimension(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(MAX_DIMENSION).max(1_u32)
}

/// A `width` x `height` canvas uniformly filled with `color`; the
/// dimensions are validated by the caller.
fn filled_canvas(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
    let columns = usize::try_from(width).unwrap_or(0);
    let row_bytes = color.repeat(columns);
    let rows = usize::try_from(height).unwrap_or(0);
    let mut pixels = Vec::with_capacity(row_bytes.len().saturating_mul(rows));
    for _ in 0..height {
        pixels.extend_from_slice(&row_bytes);
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// Blits `patch` onto `canvas` at `(x, y)`; rows outside the canvas are
/// skipped, so the caller's fit guarantee makes the whole patch land.
fn paste(canvas: &mut RgbaImage, x: u32, y: u32, patch: &RgbaImage) {
    let Some(row_bytes) = rgba_byte_count(patch.width, 1) else {
        return;
    };
    for (row, bytes) in patch.pixels.chunks_exact(row_bytes).enumerate() {
        let Ok(offset_row) = u32::try_from(row) else {
            continue;
        };
        let dst_y = y.saturating_add(offset_row);
        let Some(start) = pixel_index(canvas.width, x, dst_y)
            .and_then(|count| count.checked_mul(BYTES_PER_PIXEL))
        else {
            continue;
        };
        let Some(span) = start.checked_add(bytes.len()) else {
            continue;
        };
        if let Some(dst) = canvas.pixels.get_mut(start..span) {
            dst.copy_from_slice(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CropSpec, FlipSpec, crop, flip, rotate};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::{Filter, FitMode, ImageOp, apply_ops};
    use serde_json::Value;

    /// A 2x3 grey image whose pixels are their 1-based reading order.
    fn labelled() -> RgbaImage {
        RgbaImage::new(
            2,
            3,
            vec![
                1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255, 5, 5, 5, 255, 6, 6, 6, 255,
            ],
        )
        .expect("2x3 RGBA8 is valid")
    }

    /// The grey value of pixel `(x, y)`.
    fn value(img: &RgbaImage, x: u32, y: u32) -> u8 {
        img.get(x, y).expect("in bounds")[0]
    }

    /// The message of the `InvalidArgument` this result must hold.
    fn rejection(result: Result<(RgbaImage, Value), CyberError>) -> String {
        let CyberError::InvalidArgument(text) = result.expect_err("must be invalid") else {
            panic!("expected InvalidArgument");
        };
        text
    }

    #[test]
    fn crop_extracts_the_rectangle() {
        let spec = CropSpec {
            x: 1,
            y: 1,
            width: 1,
            height: 2,
        };
        let (out, step) = crop(&labelled(), 0, &spec).expect("rectangle is inside");
        assert_eq!((out.width, out.height), (1, 2), "cropped shape");
        assert_eq!(value(&out, 0, 0), 4, "first row");
        assert_eq!(value(&out, 0, 1), 6, "second row");
        assert_eq!(step["op"], "crop", "op name");
        assert_eq!(step["width"], 1_i32, "reported width");
        assert_eq!(step["height"], 2_i32, "reported height");
    }

    #[test]
    fn crop_out_of_bounds_names_the_offending_field() {
        let too_wide = CropSpec {
            x: 1,
            y: 0,
            width: 2,
            height: 1,
        };
        let wide_text = rejection(crop(&labelled(), 3, &too_wide));
        assert!(
            wide_text.contains("ops[3].x") && wide_text.contains("edge"),
            "x span error: {wide_text}"
        );
        let zero = CropSpec {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        };
        let zero_text = rejection(crop(&labelled(), 0, &zero));
        assert!(
            zero_text.contains("ops[0].width") && zero_text.contains("1..=4096"),
            "width range error: {zero_text}"
        );
    }

    #[test]
    fn rotate_quarter_turns_are_exact_permutations() {
        let quarter = rotate(&labelled(), 0, 1).expect("turns valid").0;
        assert_eq!(
            (quarter.width, quarter.height),
            (3, 2),
            "90 clockwise swaps edges"
        );
        assert_eq!(
            [
                value(&quarter, 0, 0),
                value(&quarter, 1, 0),
                value(&quarter, 2, 0)
            ],
            [5, 3, 1],
            "first destination row"
        );
        let half = rotate(&labelled(), 0, 2).expect("turns valid").0;
        assert_eq!((half.width, half.height), (2, 3), "180 keeps shape");
        assert_eq!(value(&half, 0, 0), 6, "bottom-right moves to top-left");
        let three_quarters = rotate(&labelled(), 0, 3).expect("turns valid").0;
        assert_eq!(
            (three_quarters.width, three_quarters.height),
            (3, 2),
            "270 clockwise swaps edges"
        );
        assert_eq!(
            [
                value(&three_quarters, 0, 0),
                value(&three_quarters, 1, 0),
                value(&three_quarters, 2, 0)
            ],
            [2, 4, 6],
            "first destination row"
        );
    }

    #[test]
    fn rotate_rejects_turns_outside_one_to_three() {
        let turns_text = rejection(rotate(&labelled(), 2, 0));
        assert!(
            turns_text.contains("ops[2].quarter_turns") && turns_text.contains("1..=3"),
            "turns range error: {turns_text}"
        );
    }

    #[test]
    fn flip_mirrors_the_requested_axes() {
        let horizontal = flip(
            &labelled(),
            &FlipSpec {
                horizontal: true,
                vertical: false,
            },
        );
        let left_column: Vec<u8> = (0..3).map(|y| value(&horizontal.0, 0, y)).collect();
        assert_eq!(
            left_column,
            vec![2, 4, 6],
            "each row mirrored left-to-right"
        );
        let vertical = flip(
            &labelled(),
            &FlipSpec {
                horizontal: false,
                vertical: true,
            },
        );
        let top_row: Vec<u8> = (0..3).map(|y| value(&vertical.0, 0, y)).collect();
        assert_eq!(top_row, vec![5, 3, 1], "rows reversed top-to-bottom");
        assert_eq!(vertical.1["op"], "flip", "report names the op");
    }

    #[test]
    fn fit_stretch_resamples_to_exactly_the_target() {
        let ops = [ImageOp::Fit {
            width: 4,
            height: 6,
            mode: FitMode::Stretch,
            filter: Filter::Nearest,
            background: None,
        }];
        let (out, report) = apply_ops(labelled(), &ops).expect("stretch fits");
        assert_eq!((out.width, out.height), (4, 6), "target shape");
        assert_eq!(value(&out, 0, 0), 1, "top-left anchor kept");
        assert_eq!(value(&out, 3, 5), 6, "bottom-right anchor kept");
        assert_eq!(report.steps[0]["mode"], "stretch", "mode reported");
        assert_eq!(report.steps[0]["offset_x"], 0_i32, "no crop offset");
    }

    #[test]
    fn fit_cover_scales_uniformly_and_centres_the_crop() {
        let ops = [ImageOp::Fit {
            width: 4,
            height: 2,
            mode: FitMode::Cover,
            filter: Filter::Nearest,
            background: None,
        }];
        let (out, report) = apply_ops(labelled(), &ops).expect("cover fits");
        assert_eq!((out.width, out.height), (4, 2), "target shape");
        assert_eq!(
            [
                value(&out, 0, 0),
                value(&out, 2, 0),
                value(&out, 0, 1),
                value(&out, 3, 1)
            ],
            [3, 4, 3, 4],
            "the crop keeps the middle source row, duplicated by nearest scaling"
        );
        let step = &report.steps[0];
        assert_eq!(step["scale_num"], 4_i32, "width decides the scale");
        assert_eq!(step["scale_den"], 2_i32, "source width is the denominator");
        assert_eq!(step["offset_x"], 0_i32, "no horizontal crop");
        assert_eq!(step["offset_y"], 2_i32, "two rows cropped from the top");
    }

    #[test]
    fn fit_contain_centres_on_the_background_canvas() {
        let ops = [ImageOp::Fit {
            width: 4,
            height: 4,
            mode: FitMode::Contain,
            filter: Filter::Nearest,
            background: Some("#00ff00ff".to_owned()),
        }];
        let (out, report) = apply_ops(labelled(), &ops).expect("contain fits");
        assert_eq!((out.width, out.height), (4, 4), "canvas shape");
        assert_eq!(
            out.get(0, 0),
            Some([0, 255, 0, 255]),
            "padding uses the background"
        );
        assert_eq!(
            out.get(1, 0),
            Some([1, 1, 1, 255]),
            "content starts at offset_x"
        );
        let step = &report.steps[0];
        assert_eq!(step["mode"], "contain", "mode reported");
        assert_eq!(step["scale_num"], 4_i32, "height decides the scale");
        assert_eq!(step["scale_den"], 3_i32, "source height is the denominator");
        assert_eq!(step["offset_x"], 1_i32, "centre padding to the left");
        assert_eq!(step["offset_y"], 0_i32, "no vertical padding");
    }

    #[test]
    fn fit_contain_never_scales_an_edge_away() {
        // A 4000x1 line contain-fit into 64x64 scales the height by
        // `64/4000`: the floor division rounds it to `0` px, so the scale
        // result must clamp to one kept row.
        let pixels = vec![7_u8; 4_000_usize.saturating_mul(4)];
        let strip = RgbaImage::new(4_000, 1, pixels).expect("4000x1 is valid");
        let ops = [ImageOp::Fit {
            width: 64,
            height: 64,
            mode: FitMode::Contain,
            filter: Filter::Nearest,
            background: Some("#00000000".to_owned()),
        }];
        let (out, report) = apply_ops(strip, &ops).expect("contain fits");
        assert_eq!((out.width, out.height), (64, 64), "canvas shape");
        let step = &report.steps[0];
        assert_eq!(step["scale_den"], 4_000_i32, "the width decides");
        assert_eq!(step["offset_y"], 31_i32, "the kept row centres at 31");
        assert_eq!(out.get(0, 31), Some([7, 7, 7, 7]), "the row survived");
        assert_eq!(
            out.get(0, 0),
            Some([0, 0, 0, 0]),
            "the padding is the canvas"
        );
    }

    #[test]
    fn fit_validates_background_in_every_mode() {
        let ops = [ImageOp::Fit {
            width: 4,
            height: 4,
            mode: FitMode::Cover,
            filter: Filter::Nearest,
            background: Some("bogus".to_owned()),
        }];
        let error = apply_ops(labelled(), &ops).expect_err("bad colour must fail");
        let CyberError::InvalidArgument(text) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            text.contains("ops[0].background"),
            "error names the field: {text}"
        );
    }
}
