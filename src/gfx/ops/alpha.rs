//! Alpha image operations.
//!
//! [`crate::gfx::ops::ImageOp::ColorKey`] and [`crate::gfx::ops::ImageOp::Trim`].
//!
//! Both work on the alpha channel: keying zeroes matching pixels, trimming
//! crops to the bounding box of the surviving content and re-pads it, so
//! sprite offsets can be derived from the report.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{MAX_OP_DIMENSION, arg_error, parse_hex_colour};
use crate::gfx::{BYTES_PER_PIXEL, pixel_count, pixel_index, to_pixel};

/// Parameters of [`crate::gfx::ops::ImageOp::ColorKey`].
pub(crate) struct ColorKeySpec<'key> {
    /// Key colour as text, validated by [`parse_hex_colour`].
    pub(crate) color: &'key str,
    /// Per-channel match tolerance.
    pub(crate) tolerance: u8,
}

/// Parameters of [`crate::gfx::ops::ImageOp::Trim`].
pub(crate) struct TrimSpec {
    /// Alpha at or above which a pixel counts as content.
    pub(crate) alpha_threshold: u8,
    /// Transparent pixels added on every side of the bounding box.
    pub(crate) padding: u32,
}

/// Sets alpha `0` on every pixel whose maximum `RGB` difference to the key
/// colour fits within the tolerance.
pub(crate) fn color_key(
    img: RgbaImage,
    index: usize,
    spec: &ColorKeySpec<'_>,
) -> Result<(RgbaImage, Value), CyberError> {
    let Ok(key) = parse_hex_colour(spec.color, true) else {
        return Err(arg_error(
            index,
            "color",
            &format!("be a #RRGGBBAA or #RRGGBB hex colour, got {}", spec.color),
        ));
    };
    let mut out = img;
    let mut removed = 0_u32;
    for chunk in out.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        let pixel = to_pixel(chunk);
        if max_channel_delta(pixel, key) <= u32::from(spec.tolerance) {
            let [red, green, blue, alpha] = pixel;
            removed = removed.saturating_add(u32::from(alpha > 0));
            chunk.copy_from_slice(&[red, green, blue, 0_u8]);
        }
    }
    Ok((out, json!({"op": "color_key", "removed_pixels": removed})))
}

/// The largest per-channel absolute difference of the two colours' `RGB`.
fn max_channel_delta(pixel: [u8; 4], key: [u8; 4]) -> u32 {
    let [red, green, blue, _] = pixel;
    let [key_red, key_green, key_blue, _] = key;
    [red, green, blue]
        .into_iter()
        .zip([key_red, key_green, key_blue])
        .map(|(value, target)| {
            let difference = i32::from(value)
                .checked_sub(i32::from(target))
                .unwrap_or(0_i32)
                .checked_abs()
                .unwrap_or(0_i32);
            u32::try_from(difference).unwrap_or(0_u32)
        })
        .max()
        .unwrap_or(0_u32)
}

/// Crops to the bounding box of pixels reaching the alpha threshold and
/// re-pads it with transparency.
pub(crate) fn trim(
    img: &RgbaImage,
    index: usize,
    spec: &TrimSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    let Some((x0, y0, x1, y1)) = content_bounds(img, spec.alpha_threshold) else {
        return Err(CyberError::InvalidArgument(format!(
            "ops[{index}] nothing left after trim: no pixel reaches alpha {}",
            spec.alpha_threshold
        )));
    };
    let content_w = x1.saturating_sub(x0).saturating_add(1);
    let content_h = y1.saturating_sub(y0).saturating_add(1);
    let extra = spec.padding.saturating_mul(2);
    let (width, height) = (
        content_w.saturating_add(extra),
        content_h.saturating_add(extra),
    );
    if width > MAX_OP_DIMENSION || height > MAX_OP_DIMENSION {
        return Err(arg_error(
            index,
            "padding",
            &format!(
                "keep the trimmed {width}x{height} result within {MAX_OP_DIMENSION} pixels on an edge"
            ),
        ));
    }
    let left = i64::from(x0)
        .checked_sub(i64::from(spec.padding))
        .unwrap_or(i64::MIN);
    let top = i64::from(y0)
        .checked_sub(i64::from(spec.padding))
        .unwrap_or(i64::MIN);
    let mut pixels = vec![
        0_u8;
        pixel_count(width, height)
            .unwrap_or(0)
            .saturating_mul(BYTES_PER_PIXEL)
    ];
    copy_content_rows(
        img,
        &mut pixels,
        (x0, y0, content_w, content_h),
        (width, spec.padding),
    );
    Ok((
        RgbaImage {
            width,
            height,
            pixels,
        },
        json!({"op": "trim", "left": left, "top": top, "width": width, "height": height}),
    ))
}

/// Copies the content rectangle from the source into the padded output at
/// the `padding` offset; rows that do not fit either buffer are skipped.
fn copy_content_rows(
    img: &RgbaImage,
    out: &mut [u8],
    content: (u32, u32, u32, u32),
    padded: (u32, u32),
) {
    let (x0, y0, content_w, content_h) = content;
    let (out_width, padding) = padded;
    let row_bytes = usize::try_from(content_w)
        .unwrap_or(0)
        .saturating_mul(BYTES_PER_PIXEL);
    for row in 0..content_h {
        let source = row_span(img.width, x0, y0.saturating_add(row), row_bytes);
        let destination = row_span(out_width, padding, padding.saturating_add(row), row_bytes);
        let (Some(src_range), Some(dst_range)) = (source, destination) else {
            continue;
        };
        let (Some(src_slice), Some(dst_slice)) =
            (img.pixels.get(src_range), out.get_mut(dst_range))
        else {
            continue;
        };
        dst_slice.copy_from_slice(src_slice);
    }
}

/// The byte range of the `bytes`-long strip at `(x, y)`, if addressable.
fn row_span(img_width: u32, x: u32, y: u32, bytes: usize) -> Option<core::ops::Range<usize>> {
    let start = pixel_index(img_width, x, y)?.checked_mul(BYTES_PER_PIXEL)?;
    let end = start.checked_add(bytes)?;
    Some(start..end)
}

/// The inclusive bounding box of pixels with alpha at or above `threshold`,
/// `None` when no pixel reaches it.
fn content_bounds(img: &RgbaImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (position, chunk) in img.pixels.chunks_exact(BYTES_PER_PIXEL).enumerate() {
        let [_red, _green, _blue, alpha] = to_pixel(chunk);
        if alpha < threshold {
            continue;
        }
        let Some(point) = position_of(img.width, position) else {
            continue;
        };
        bounds = Some(grow_bounds(bounds, point));
    }
    bounds
}

/// The `(x, y)` coordinate of a row-major pixel number, `None` when it
/// does not fit.
fn position_of(width: u32, position: usize) -> Option<(u32, u32)> {
    let columns = usize::try_from(width).ok()?;
    let x = position.checked_rem(columns)?;
    let y = position.checked_div(columns)?;
    Some((u32::try_from(x).ok()?, u32::try_from(y).ok()?))
}

/// The box that covers `bounds` and the point `(x, y)`.
fn grow_bounds(bounds: Option<(u32, u32, u32, u32)>, point: (u32, u32)) -> (u32, u32, u32, u32) {
    let (x, y) = point;
    match bounds {
        None => (x, y, x, y),
        Some((left, top, right, bottom)) => (left.min(x), top.min(y), right.max(x), bottom.max(y)),
    }
}

#[cfg(test)]
mod tests {
    use super::{ColorKeySpec, TrimSpec, color_key, trim};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;

    /// Alpha per pixel of the fixture, row-major: an opaque red 2x2 block at
    /// (1,1) and one half-transparent red pixel at (3,3).
    const ALPHA_ROWS: [[u8; 4]; 4] = [
        [0, 0, 0, 0],
        [0, 255, 255, 0],
        [0, 255, 255, 0],
        [0, 0, 0, 100],
    ];

    /// The 4x4 fixture image built from [`ALPHA_ROWS`].
    fn red_block() -> RgbaImage {
        let pixels: Vec<u8> = ALPHA_ROWS
            .iter()
            .flatten()
            .flat_map(|alpha| [255_u8, 0, 0, *alpha])
            .collect();
        RgbaImage::new(4, 4, pixels).expect("4x4 RGBA8 is valid")
    }

    /// The message of the `InvalidArgument` this result must hold.
    fn rejection(result: Result<(RgbaImage, serde_json::Value), CyberError>) -> String {
        let CyberError::InvalidArgument(text) = result.expect_err("input must be rejected") else {
            panic!("expected InvalidArgument");
        };
        text
    }

    #[test]
    fn color_key_removes_matching_pixels_within_tolerance() {
        let img = RgbaImage::new(3, 1, vec![255, 0, 0, 255, 240, 0, 0, 255, 0, 0, 255, 255])
            .expect("3x1 is valid");
        let spec = ColorKeySpec {
            color: "#ff0000",
            tolerance: 20,
        };
        let (out, step) = color_key(img, 0, &spec).expect("colour parses");
        assert_eq!(
            out.pixels,
            vec![255, 0, 0, 0, 240, 0, 0, 0, 0, 0, 255, 255],
            "matched pixels lose alpha, colours stay, blue survives"
        );
        assert_eq!(step["op"], "color_key", "op name");
        assert_eq!(step["removed_pixels"], 2_i32, "both reds counted once");
    }

    #[test]
    fn color_key_without_tolerance_is_exact_and_skips_invisible() {
        let img = RgbaImage::new(2, 1, vec![255, 0, 0, 255, 254, 0, 0, 255]).expect("2x1 valid");
        let spec = ColorKeySpec {
            color: "#ff0000",
            tolerance: 0,
        };
        let (out, step) = color_key(img, 1, &spec).expect("colour parses");
        assert_eq!(out.get(0, 0), Some([255, 0, 0, 0]), "exact match keyed out");
        assert_eq!(out.get(1, 0), Some([254, 0, 0, 255]), "one off survives");
        assert_eq!(
            step["removed_pixels"], 1_i32,
            "only the exact match removed"
        );

        let invisible = RgbaImage::new(1, 1, vec![255, 0, 0, 0]).expect("1x1 valid");
        let (_kept, ghost_step) = color_key(invisible, 0, &spec).expect("colour parses");
        assert_eq!(
            ghost_step["removed_pixels"], 0_i32,
            "already transparent is not removed"
        );
    }

    #[test]
    fn color_key_rejects_a_malformed_colour_naming_the_op() {
        let img = RgbaImage::new(1, 1, vec![0; 4]).expect("1x1 valid");
        let spec = ColorKeySpec {
            color: "red",
            tolerance: 0,
        };
        let bad_text = rejection(color_key(img, 2, &spec));
        assert!(
            bad_text.contains("ops[2].color"),
            "error names the op and field: {bad_text}"
        );
    }

    #[test]
    fn trim_crops_to_the_opaque_bounding_box() {
        let spec = TrimSpec {
            alpha_threshold: 128,
            padding: 0,
        };
        let (out, step) = trim(&red_block(), 0, &spec).expect("block present");
        assert_eq!(
            (out.width, out.height),
            (2, 2),
            "2x2 block, faint pixel excluded"
        );
        assert_eq!(
            out.get(0, 0),
            Some([255, 0, 0, 255]),
            "content moved to the origin"
        );
        assert_eq!(step["op"], "trim", "op name");
        assert_eq!(step["left"], 1_i32, "crop origin in the input");
        assert_eq!(step["top"], 1_i32, "crop origin in the input");
        assert_eq!(step["width"], 2_i32, "cropped width");
        assert_eq!(step["height"], 2_i32, "cropped height");
    }

    #[test]
    fn trim_padding_extends_the_box_and_can_make_offsets_negative() {
        let spec = TrimSpec {
            alpha_threshold: 128,
            padding: 2,
        };
        let (out, step) = trim(&red_block(), 0, &spec).expect("block present");
        assert_eq!(
            (out.width, out.height),
            (6, 6),
            "block plus two pixels per side"
        );
        assert_eq!(out.get(0, 0), Some([0, 0, 0, 0]), "padding is transparent");
        assert_eq!(
            out.get(2, 2),
            Some([255, 0, 0, 255]),
            "content starts after padding"
        );
        assert_eq!(step["left"], -1_i32, "origin minus padding can go negative");
        assert_eq!(step["top"], -1_i32, "origin minus padding can go negative");
    }

    #[test]
    fn trim_reports_the_threshold_as_an_instruction_when_nothing_survives() {
        let spec = TrimSpec {
            alpha_threshold: 128,
            padding: 0,
        };
        let empty = RgbaImage::new(2, 2, vec![0; 16]).expect("2x2 valid");
        let empty_text = rejection(trim(&empty, 1, &spec));
        assert!(
            empty_text.contains("ops[1]") && empty_text.contains("nothing left after trim"),
            "clear empty-trim error: {empty_text}"
        );
    }

    #[test]
    fn trim_rejects_padding_beyond_the_op_dimension() {
        let spec = TrimSpec {
            alpha_threshold: 128,
            padding: 4_096,
        };
        let pad_text = rejection(trim(&red_block(), 0, &spec));
        assert!(
            pad_text.contains("ops[0].padding") && pad_text.contains("4096"),
            "padding error names the field and limit: {pad_text}"
        );
    }
}
