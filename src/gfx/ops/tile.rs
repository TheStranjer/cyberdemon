//! Seamless-tiling blend for [`crate::gfx::ops::ImageOp::MakeTileable`].
//!
//! Each enabled axis mixes every edge pixel with its half-period shift,
//! `S(x) = img((x + length / 2) mod length)`, weighted by `wo`, the
//! distance from the edge clamped to `band`: the result is
//! `(img * wo + S * (band - wo) + band / 2) / band`. At the edge the
//! output is pure `S`, which is continuous across the wrap; at least
//! `band` pixels in, it is the untouched original, hiding `S`'s own seam
//! in the middle. Blending one axis never breaks the other axis's
//! tileability, so a horizontal pass followed by a vertical one tiles
//! both ways.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{arg_error, rounded_div};

/// Parameters of [`crate::gfx::ops::ImageOp::MakeTileable`].
pub(crate) struct TileSpec {
    /// Blend width; `None` uses a quarter of the enabled axis length.
    pub(crate) band: Option<u32>,
    /// Blend the left and right edges.
    pub(crate) horizontal: bool,
    /// Blend the top and bottom edges.
    pub(crate) vertical: bool,
}

/// Bands resolved per axis; `None` on an axis that stays untouched.
#[derive(Debug)]
pub(crate) struct TileBands {
    /// Blend width on the horizontal axis.
    pub(crate) x: Option<u32>,
    /// Blend width on the vertical axis.
    pub(crate) y: Option<u32>,
}

/// Validates and resolves the band of one enabled axis; the default is
/// `length / 4`, clamped up to at least `1`.
fn axis_band(
    index: usize,
    axis: &str,
    requested: Option<u32>,
    length: u32,
) -> Result<u32, CyberError> {
    if length < 2 {
        return Err(arg_error(
            index,
            axis,
            &format!("be at least 2 pixels to tile on the {axis} axis, got {length}"),
        ));
    }
    let half = length.checked_div(2).unwrap_or(1_u32);
    let band = requested.unwrap_or_else(|| length.checked_div(4).unwrap_or(1_u32).max(1_u32));
    if band == 0 || band > half {
        return Err(arg_error(
            index,
            "band",
            &format!("be within 1..={half} for {axis} {length}, got {band}"),
        ));
    }
    Ok(band)
}

/// Resolves and validates the bands of the enabled axes.
fn resolve_bands(img: &RgbaImage, index: usize, spec: &TileSpec) -> Result<TileBands, CyberError> {
    let x = spec
        .horizontal
        .then(|| axis_band(index, "width", spec.band, img.width))
        .transpose()?;
    let y = spec
        .vertical
        .then(|| axis_band(index, "height", spec.band, img.height))
        .transpose()?;
    Ok(TileBands { x, y })
}

/// Blends the enabled axes so the result tiles in both directions.
pub(crate) fn make_tileable(
    img: RgbaImage,
    index: usize,
    spec: &TileSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    let bands = resolve_bands(&img, index, spec)?;
    let mut out = img;
    if let Some(band) = bands.x {
        out = blend_axis(&out, band, true);
    }
    if let Some(band) = bands.y {
        out = blend_axis(&out, band, false);
    }
    Ok((
        out,
        json!({"op": "make_tileable", "band_x": bands.x, "band_y": bands.y}),
    ))
}

/// One axis of a seamless-tiling pass.
struct Axis {
    /// Pixels along the blended axis.
    length: u32,
    /// Half-period shift.
    half: u32,
    /// Blend width.
    band: u32,
    /// Whether this is the horizontal axis.
    horizontal: bool,
}

impl Axis {
    /// Blends one pixel with its half-period shift.
    fn blend_one(&self, img: &RgbaImage, out: &mut RgbaImage, spot: (u32, u32)) {
        let Some(shifted) = self.shift_of(spot) else {
            return;
        };
        let (Some(source), Some(sample)) = (img.get(spot.0, spot.1), img.get(shifted.0, shifted.1))
        else {
            return;
        };
        let position = if self.horizontal { spot.0 } else { spot.1 };
        let weight = edge_weight(position, self.length, self.band);
        out.set(
            spot.0,
            spot.1,
            blend_pixel(source, sample, weight, self.band),
        );
    }

    /// The coordinate of `spot`'s half-period shift along the axis.
    fn shift_of(&self, spot: (u32, u32)) -> Option<(u32, u32)> {
        let along = if self.horizontal { spot.0 } else { spot.1 };
        let shift = along.checked_add(self.half)?.checked_rem(self.length)?;
        Some(if self.horizontal {
            (shift, spot.1)
        } else {
            (spot.0, shift)
        })
    }

    /// Blends one line along the axis.
    fn blend_line(&self, img: &RgbaImage, out: &mut RgbaImage, line: u32) {
        for position in 0..self.length {
            let spot = coords(self.horizontal, position, line);
            self.blend_one(img, out, spot);
        }
    }
}

/// Blends every line along one axis against its half-period shift.
fn blend_axis(img: &RgbaImage, band: u32, horizontal: bool) -> RgbaImage {
    let (length, cross) = if horizontal {
        (img.width, img.height)
    } else {
        (img.height, img.width)
    };
    let axis = Axis {
        length,
        half: length.checked_div(2).unwrap_or(0_u32),
        band,
        horizontal,
    };
    let mut out = img.clone();
    for line in 0..cross {
        axis.blend_line(img, &mut out, line);
    }
    out
}

/// The image coordinate of an `(position, line)` pair on one axis.
const fn coords(horizontal: bool, position: u32, line: u32) -> (u32, u32) {
    if horizontal {
        (position, line)
    } else {
        (line, position)
    }
}

/// `min(min(x, length - x), band)`: the edge weight of one position.
const fn edge_weight(position: u32, length: u32, band: u32) -> u32 {
    let mirrored = length.saturating_sub(position);
    let distance = if position > mirrored {
        mirrored
    } else {
        position
    };
    if distance > band { band } else { distance }
}

/// `(source * weight + shifted * (band - weight) + band / 2) / band` per channel.
fn blend_pixel(source: [u8; 4], shifted: [u8; 4], weight: u32, band: u32) -> [u8; 4] {
    let inverse = band.saturating_sub(weight);
    let mut out = [0_u8; 4];
    for (slot, (from, to)) in out.iter_mut().zip(source.iter().zip(shifted)) {
        let numerator = u64::from(*from)
            .saturating_mul(u64::from(weight))
            .saturating_add(u64::from(to).saturating_mul(u64::from(inverse)));
        *slot = u8::try_from(rounded_div(numerator, u64::from(band))).unwrap_or(255_u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{TileBands, TileSpec, edge_weight, make_tileable, resolve_bands};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;

    /// A `width x height` image whose pixel colour is `color(column, row)`.
    fn painted<F>(width: u32, height: u32, color: F) -> RgbaImage
    where
        F: Fn(u32, u32) -> [u8; 4],
    {
        let columns = usize::try_from(width).expect("fixture fits");
        let area = usize::try_from(width.saturating_mul(height)).expect("fixture fits");
        let pixels: Vec<u8> = (0..area)
            .flat_map(|position| {
                let column = position.checked_rem(columns).expect("non-zero");
                let row = position.checked_div(columns).expect("fits");
                color(
                    u32::try_from(column).expect("fits"),
                    u32::try_from(row).expect("fits"),
                )
            })
            .collect();
        RgbaImage::new(width, height, pixels).expect("fixture is valid")
    }

    /// A grey pixel as bright as its `x` coordinate.
    fn grey_x(x: u32, _y: u32) -> [u8; 4] {
        let shade = u8::try_from(x).expect("fits u8");
        [shade, shade, shade, 255_u8]
    }

    /// A grey pixel as bright as its `y` coordinate.
    fn grey_y(_x: u32, y: u32) -> [u8; 4] {
        let shade = u8::try_from(y).expect("fits u8");
        [shade, shade, shade, 255_u8]
    }

    /// A `256`-wide, `height`-tall horizontal grey gradient `0..=255`.
    fn column_gradient(height: u32) -> RgbaImage {
        painted(256, height, grey_x)
    }

    /// A `width x height` image whose pixel colour is its `x` coordinate.
    fn column_strip(width: u32, height: u32) -> RgbaImage {
        painted(width, height, grey_x)
    }

    /// A `width x height` image whose pixel colour is its `y` coordinate.
    fn row_strip(width: u32, height: u32) -> RgbaImage {
        painted(width, height, grey_y)
    }

    /// A spec that tiles only the horizontal axis.
    fn horizontal_only(band: Option<u32>) -> TileSpec {
        TileSpec {
            band,
            horizontal: true,
            vertical: false,
        }
    }

    /// A spec that tiles only the vertical axis.
    fn vertical_only(band: Option<u32>) -> TileSpec {
        TileSpec {
            band,
            horizontal: false,
            vertical: true,
        }
    }

    /// A spec that tiles both axes.
    fn both_axes(band: Option<u32>) -> TileSpec {
        TileSpec {
            band,
            horizontal: true,
            vertical: true,
        }
    }

    /// The rejection message of a failing band resolution.
    fn rejection(result: Result<TileBands, CyberError>) -> String {
        let CyberError::InvalidArgument(text) = result.expect_err("band must be rejected") else {
            panic!("expected InvalidArgument");
        };
        text
    }

    #[test]
    fn horizontal_gradient_wraps_seamlessly() {
        let (out, step) =
            make_tileable(column_gradient(4), 0, &horizontal_only(None)).expect("256x4 tiles");
        assert_eq!(step["op"], "make_tileable", "op name");
        assert_eq!(step["band_x"], 64_i32, "default band is width / 4");
        assert!(step["band_y"].is_null(), "a disabled axis reports null");
        let seam = i32::from(out.get(0, 0).expect("first")[0])
            - i32::from(out.get(255, 0).expect("last")[0]);
        let internal = (0..255_u32)
            .map(|x| {
                i32::from(out.get(x, 0).expect("left")[0])
                    - i32::from(out.get(x.saturating_add(1), 0).expect("right")[0])
            })
            .map(i32::abs)
            .max()
            .expect("neighbours exist");
        let seam_gap = seam.abs();
        assert!(
            seam_gap <= internal,
            "seam jump {seam_gap} exceeds the largest internal step {internal}"
        );
    }

    #[test]
    fn pixels_beyond_the_band_keep_their_colour() {
        let (out, _step) =
            make_tileable(column_gradient(1), 0, &horizontal_only(None)).expect("256x1 tiles");
        for x in [64_u32, 128, 192] {
            assert_eq!(
                out.get(x, 0).expect("in bounds")[0],
                u8::try_from(x).expect("fits u8"),
                "x={x} is at least band away from both edges"
            );
        }
        assert_eq!(out.get(0, 0).expect("edge")[0], 128, "the edge is pure S");
    }

    #[test]
    fn each_pass_blends_only_its_own_axis() {
        // A column-strip varies with x only: the horizontal pass blends
        // real differences, the vertical pass mixes equal values.
        let cols = column_strip(6, 6);
        let (horiz, _h_step) =
            make_tileable(cols.clone(), 0, &horizontal_only(Some(2))).expect("6x6 tiles");
        assert_eq!(
            horiz.get(0, 0).expect("edge")[0],
            3,
            "left edge becomes the shifted column"
        );
        assert_ne!(horiz, cols, "the horizontal pass changed the image");
        let (vert, _v_step) =
            make_tileable(cols.clone(), 1, &vertical_only(Some(2))).expect("6x6 tiles");
        assert_eq!(
            vert, cols,
            "a vertical-only blend cannot touch a column strip"
        );
        // The row-strip is the mirror case.
        let rows = row_strip(6, 6);
        let (flat, _f_step) =
            make_tileable(rows.clone(), 2, &horizontal_only(Some(2))).expect("6x6 tiles");
        assert_eq!(
            flat, rows,
            "a horizontal-only blend cannot touch a row strip"
        );
        let (steep, _s_step) = make_tileable(rows, 3, &vertical_only(Some(2))).expect("6x6 tiles");
        assert_eq!(
            steep.get(0, 0).expect("edge")[0],
            3,
            "top edge becomes the shifted row"
        );
    }

    #[test]
    fn out_of_range_bands_and_thin_axes_are_rejected() {
        let wide = rejection(resolve_bands(
            &column_strip(8, 8),
            0,
            &horizontal_only(Some(5)),
        ));
        assert!(
            wide.contains("ops[0].band") && wide.contains("1..=4"),
            "band over width/2 is rejected: {wide}"
        );
        let thin = rejection(resolve_bands(&column_strip(1, 4), 1, &both_axes(None)));
        assert!(
            thin.contains("ops[1].width") && thin.contains("at least 2"),
            "a 1-pixel-wide axis cannot tile: {thin}"
        );
        let tall = rejection(resolve_bands(&row_strip(4, 1), 2, &vertical_only(None)));
        assert!(
            tall.contains("ops[2].height"),
            "a 1-pixel-tall axis cannot tile vertically: {tall}"
        );
    }

    #[test]
    fn both_axes_disabled_is_a_valid_no_op() {
        let idle = TileSpec {
            band: None,
            horizontal: false,
            vertical: false,
        };
        let (out, step) = make_tileable(column_strip(3, 3), 0, &idle).expect("the no-op validates");
        assert!(
            step["band_x"].is_null() && step["band_y"].is_null(),
            "no bands"
        );
        assert_eq!(out, column_strip(3, 3), "the image passes through");
    }

    #[test]
    fn edge_weight_clamps_distance_from_both_edges() {
        assert_eq!(edge_weight(0, 100, 10), 0, "on the edge");
        assert_eq!(edge_weight(4, 100, 10), 4, "inside the band");
        assert_eq!(edge_weight(20, 100, 10), 10, "past the band");
        assert_eq!(edge_weight(97, 100, 10), 3, "near the far edge");
    }
}
