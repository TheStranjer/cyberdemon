//! The `trace_sketch` request: the wire parameters resolved into engine
//! values and the picture prepared for [`crate::sketch::analyze()`].
//!
//! [`parse_request`] turns one [`TraceSketchParams`] into a [`Request`];
//! [`prepare`] crops the decoded picture, downscales it by the exact reduced
//! fraction [`scale_for`] finds, and hands back a [`Frame`]: the route every
//! reported coordinate travels back onto the picture the caller sees (plus
//! the `crop` offsets), and the optional [`Units`] mirror into map
//! coordinates. [`analyze_options`] and [`vectorize_options`] bring the
//! request's pixel parameters onto the analysis raster, and
//! [`run_vectorize`] skips [`crate::sketch::vectorize()`] for the zero-region
//! case the tool reports instead of refusing.

use crate::error::CyberError;
use crate::fixed::{gcd, mul_div_round};
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::resample::resample;
use crate::gfx::ops::{Filter, pixel_at, rounded_div};
use crate::gfx::{BYTES_PER_PIXEL, pixel_index};
use crate::plan::Units;
use crate::sketch::{Analysis, AnalyzeOptions, InkMode, VectorizeOptions, Vectorized, vectorize};
use crate::tools::plan::params::UnitsParam;

use super::{
    CropParams, DEFAULT_MAX_ROWS, DEFAULT_MAX_SIZE, InkParams, MAX_MAX_ROWS, MAX_MAX_SIZE,
    MIN_MAX_ROWS, MIN_MAX_SIZE, PixelBox, TraceSketchParams, XY,
};

/// The validated request: parameter texts resolved into engine values.
pub(super) struct Request {
    /// The `source` to fetch.
    pub(super) source: String,
    /// The crop to take, `None` for the whole image.
    pub(super) crop: Option<CropParams>,
    /// The resolved longer-edge analysis limit.
    pub(super) max_size: u32,
    /// The resolved ink definition.
    pub(super) ink: InkMode,
    /// Closing radius in original pixels.
    pub(super) close_gaps: Option<u32>,
    /// Minimum region area in original square pixels.
    pub(super) min_region_area: Option<u32>,
    /// Maximum mark edge in original pixels.
    pub(super) mark_max_size: Option<u32>,
    /// Simplification tolerance in original pixels.
    pub(super) simplify: Option<u32>,
    /// Whether rings are straightened.
    pub(super) orthogonal: bool,
    /// Welding tolerance in original pixels.
    pub(super) weld: Option<u32>,
    /// The map transform, `None` when no `units` object was given.
    pub(super) units: Option<Units>,
    /// The resolved gap row cap.
    pub(super) max_gaps: usize,
    /// The resolved mark row cap.
    pub(super) max_marks: usize,
    /// The asked noise floor in original pixels, `None` for "the measured
    /// stroke".
    pub(super) noise_floor_px: Option<u32>,
    /// Whether the floor and both caps are lifted.
    pub(super) verbose: bool,
}

/// The analysis raster and the exact route back onto the source picture.
pub(super) struct Frame {
    /// Numerator of the analysis-pixels-per-source-pixel scale (>= 1).
    pub(super) num: u32,
    /// Denominator of that scale (>= 1).
    pub(super) den: u32,
    /// Left edge of the crop in the decoded image.
    pub(super) crop_x: i64,
    /// Top edge of the crop in the decoded image.
    pub(super) crop_y: i64,
    /// Width of the decoded (uncropped) image.
    pub(super) source_width: u32,
    /// Height of the decoded (uncropped) image.
    pub(super) source_height: u32,
    /// The caller's map transform, if any.
    pub(super) units: Option<Units>,
}

/// The picture analysis runs on, with its frame.
pub(super) struct Prepared {
    /// The cropped (and downscaled) image [`crate::sketch::analyze()`] measures.
    pub(super) picture: RgbaImage,
    /// The mapping from analysis pixels back to the decoded image.
    pub(super) frame: Frame,
}

impl Frame {
    /// One analysis-space corner point in original-image pixels.
    pub(super) fn point(&self, x: i64, y: i64) -> Option<XY> {
        let num = i64::from(self.num);
        let den = i64::from(self.den);
        let px = mul_div_round(x, den, num)?.checked_add(self.crop_x)?;
        let py = mul_div_round(y, den, num)?.checked_add(self.crop_y)?;
        Some((px, py))
    }

    /// One analysis-space pixel coordinate in original-image pixels.
    pub(super) fn pixel_point(&self, x: u32, y: u32, what: &str) -> Result<XY, CyberError> {
        self.point(i64::from(x), i64::from(y))
            .ok_or_else(|| out_of_range(what))
    }

    /// A whole analysis-space ring in original-image pixels, refusing at
    /// `what` when a vertex leaves the reportable range.
    pub(super) fn ring(&self, points: &[XY], what: &str) -> Result<Vec<XY>, CyberError> {
        points
            .iter()
            .copied()
            .map(|(x, y)| {
                self.point(x, y)
                    .ok_or_else(|| out_of_range(&format!("{what} vertex ({x}, {y})")))
            })
            .collect()
    }

    /// An inclusive analysis-space bounding box in original-image pixels.
    pub(super) fn bbox(
        &self,
        bbox: (u32, u32, u32, u32),
        what: &str,
    ) -> Result<PixelBox, CyberError> {
        let (x0, y0, x1, y1) = bbox;
        let near = self.pixel_point(x0, y0, &format!("{what} box"))?;
        let far_x = i64::from(x1)
            .checked_add(1)
            .ok_or_else(|| out_of_range(what))?;
        let far_y = i64::from(y1)
            .checked_add(1)
            .ok_or_else(|| out_of_range(what))?;
        let far = self
            .point(far_x, far_y)
            .ok_or_else(|| out_of_range(&format!("{what} box")))?;
        let x_end = far.0.checked_sub(1).ok_or_else(|| out_of_range(what))?;
        let y_end = far.1.checked_sub(1).ok_or_else(|| out_of_range(what))?;
        Ok((near.0, near.1, x_end, y_end))
    }

    /// The `map_points` for original-image pixels `points`, `None` when
    /// no `units` object was given.
    pub(super) fn map_points(&self, points: &[XY]) -> Result<Option<Vec<XY>>, CyberError> {
        self.units.map_or_else(
            || Ok(None),
            |units| {
                points
                    .iter()
                    .map(|&(x, y)| map_point(units, x, y))
                    .collect::<Result<Vec<XY>, CyberError>>()
                    .map(Some)
            },
        )
    }

    /// One map-unit coordinate pair for an analysis-space pixel centre.
    pub(super) fn map_centre(&self, x: u32, y: u32) -> Result<Option<XY>, CyberError> {
        let point = self.pixel_point(x, y, "centre")?;
        self.units.map_or_else(
            || Ok(None),
            |units| map_point(units, point.0, point.1).map(Some),
        )
    }

    /// An analysis-space length in original pixels.
    pub(super) fn length(&self, value: u32) -> u32 {
        scale_length(value, self.den, self.num)
    }

    /// An analysis-space area in original square pixels.
    pub(super) fn area(&self, value: u32) -> u32 {
        scale_area(value, self.den, self.num)
    }

    /// An original-pixel length at analysis scale.
    pub(super) fn to_analysis(&self, value: u32) -> u32 {
        scale_length(value, self.num, self.den)
    }

    /// An original-square-pixel area at analysis scale.
    pub(super) fn to_analysis_area(&self, value: u32) -> u32 {
        scale_area(value, self.num, self.den)
    }
}

/// One original-image pixel in map units under `units`, snapped to the
/// units' grid.
fn map_point(units: Units, x: i64, y: i64) -> Result<XY, CyberError> {
    units
        .point(units.grid, x, y)
        .ok_or_else(|| out_of_range(&format!("map point ({x}, {y}) under these units")))
}

/// `value * num / den` rounded, saturating into `u32`.
pub(super) fn scale_length(value: u32, num: u32, den: u32) -> u32 {
    let wide = u64::from(value).saturating_mul(u64::from(num));
    u32::try_from(rounded_div(wide, u64::from(den))).unwrap_or(u32::MAX)
}

/// `value * num^2 / den^2` rounded, saturating into `u32`.
pub(super) fn scale_area(value: u32, num: u32, den: u32) -> u32 {
    let wide = u64::from(value)
        .saturating_mul(u64::from(num))
        .saturating_mul(u64::from(num));
    let squared = u64::from(den).saturating_mul(u64::from(den)).max(1_u64);
    u32::try_from(rounded_div(wide, squared)).unwrap_or(u32::MAX)
}

/// The refusal of a coordinate no mapping can carry.
fn out_of_range(what: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{what} leaves the reportable range under the crop and scale mapping; pass a \
         smaller max_size or units.scale_num, or move the units origin closer"
    ))
}

/// Resolve the parameter texts: sizes, ink, units, grid and the row caps.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `max_size` outside `256..=4096`,
/// an unknown `ink.mode`, `color` mode without `color`, a malformed
/// `ink.color`, a `units`/`grid` value [`Units::check`] refuses or a row
/// cap outside `1..=500`.
pub(super) fn parse_request(args: &TraceSketchParams) -> Result<Request, CyberError> {
    Ok(Request {
        source: args.source.clone(),
        crop: args.crop,
        max_size: parse_max_size(args.max_size)?,
        ink: parse_ink(args.ink.as_ref())?,
        close_gaps: args.close_gaps,
        min_region_area: args.min_region_area,
        mark_max_size: args.mark_max_size,
        simplify: args.simplify,
        orthogonal: args.orthogonal,
        weld: args.weld,
        units: parse_units(args.units, args.grid)?,
        max_gaps: parse_cap("max_gaps", args.max_gaps)?,
        max_marks: parse_cap("max_marks", args.max_marks)?,
        noise_floor_px: args.noise_floor_px,
        verbose: args.verbose,
    })
}

/// One row cap: default `50`, accepted `1..=500`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `name` for a value outside the
/// range, pointing at `verbose` for every row.
pub(super) fn parse_cap(name: &str, value: Option<usize>) -> Result<usize, CyberError> {
    let cap = value.unwrap_or(DEFAULT_MAX_ROWS);
    if (MIN_MAX_ROWS..=MAX_MAX_ROWS).contains(&cap) {
        return Ok(cap);
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {cap} is out of range: pass a value between {MIN_MAX_ROWS} and {MAX_MAX_ROWS}, \
         or verbose: true for every row"
    )))
}

/// The `max_size`: default `1024`, accepted `256..=4096`.
pub(super) fn parse_max_size(value: Option<u32>) -> Result<u32, CyberError> {
    let size = value.unwrap_or(DEFAULT_MAX_SIZE);
    if (MIN_MAX_SIZE..=MAX_MAX_SIZE).contains(&size) {
        return Ok(size);
    }
    Err(CyberError::InvalidArgument(format!(
        "max_size {size} is out of range: pass a value between {MIN_MAX_SIZE} and \
         {MAX_MAX_SIZE} (default {DEFAULT_MAX_SIZE})"
    )))
}

/// The `ink` object as an [`InkMode`].
pub(super) fn parse_ink(ink: Option<&InkParams>) -> Result<InkMode, CyberError> {
    let empty = InkParams::default();
    let wanted = ink.unwrap_or(&empty);
    let mode = wanted
        .mode
        .as_deref()
        .unwrap_or("dark")
        .to_ascii_lowercase();
    match mode.as_str() {
        "dark" => Ok(InkMode::Dark {
            level: wanted.level,
        }),
        "color" => {
            let Some(color) = wanted.color.as_deref() else {
                return Err(CyberError::InvalidArgument(
                    "ink.mode \"color\" needs ink.color: the pen colour as #RRGGBB".to_owned(),
                ));
            };
            InkMode::from_hex(color, wanted.tolerance)
        }
        other => Err(CyberError::InvalidArgument(format!(
            "ink.mode {other:?} is unknown: pass \"dark\" or \"color\""
        ))),
    }
}

/// The wire `units` and `grid` as a checked [`Units`], `None` when the
/// caller left `units` out (map coordinates are then not reported).
pub(super) fn parse_units(
    requested: Option<UnitsParam>,
    grid: i64,
) -> Result<Option<Units>, CyberError> {
    let given = requested.is_some();
    let wire = requested.unwrap_or_default();
    let units = Units {
        scale_num: wire.scale_num,
        scale_den: wire.scale_den,
        origin_x: wire.origin_x,
        origin_y: wire.origin_y,
        grid,
        flip_y: wire.flip_y,
    };
    units.check()?;
    Ok(given.then_some(units))
}

/// Crop (when asked) and downscale the decoded picture; build its frame.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `crop` outside the image or with
/// a degenerate edge.
pub(super) fn prepare(request: &Request, decoded: &RgbaImage) -> Result<Prepared, CyberError> {
    let crop_box = request.crop.unwrap_or(CropParams {
        x: 0_u32,
        y: 0_u32,
        width: decoded.width,
        height: decoded.height,
    });
    check_crop(crop_box, decoded)?;
    let taken = crop_region(decoded, crop_box)?;
    let (num, den) = scale_for(crop_box.width, crop_box.height, request.max_size);
    let picture = downscale(taken, num, den);
    let frame = Frame {
        num,
        den,
        crop_x: i64::from(crop_box.x),
        crop_y: i64::from(crop_box.y),
        source_width: decoded.width,
        source_height: decoded.height,
        units: request.units,
    };
    Ok(Prepared { picture, frame })
}

/// Refuses a `crop` with an empty edge or one that leaves the image.
pub(super) fn check_crop(crop_box: CropParams, img: &RgbaImage) -> Result<(), CyberError> {
    check_nonempty("width", crop_box.width)?;
    check_nonempty("height", crop_box.height)?;
    check_fits("x", crop_box.x, crop_box.width, img.width)?;
    check_fits("y", crop_box.y, crop_box.height, img.height)
}

/// One `crop` extent must be at least one pixel.
fn check_nonempty(name: &str, value: u32) -> Result<(), CyberError> {
    if value == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "crop.{name} must be at least 1 pixel, got {value}"
        )));
    }
    Ok(())
}

/// One `crop` axis must keep its span inside the image edge.
fn check_fits(name: &str, start: u32, span: u32, limit: u32) -> Result<(), CyberError> {
    match start.checked_add(span) {
        Some(end) if end <= limit => Ok(()),
        _ => Err(CyberError::InvalidArgument(format!(
            "crop.{name} offset {start} with a {span}-pixel span leaves the {limit}-pixel \
             image: keep the span inside the edge"
        ))),
    }
}

/// The crop's pixels as an owned image (the whole image when uncropped).
fn crop_region(img: &RgbaImage, crop_box: CropParams) -> Result<RgbaImage, CyberError> {
    let count = usize::try_from(crop_box.width.saturating_mul(crop_box.height))
        .ok()
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        .unwrap_or(0);
    let mut pixels = Vec::with_capacity(count);
    for row in 0..crop_box.height {
        for column in 0..crop_box.width {
            let source_x = crop_box.x.saturating_add(column);
            let source_y = crop_box.y.saturating_add(row);
            let index = pixel_index(img.width, source_x, source_y).unwrap_or(usize::MAX);
            pixels.extend_from_slice(&pixel_at(&img.pixels, index).unwrap_or([0_u8; 4]));
        }
    }
    RgbaImage::new(crop_box.width, crop_box.height, pixels)
}

/// The exact reduced analysis-source scale fitting `max_size`.
pub(super) fn scale_for(width: u32, height: u32, max_size: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_size {
        return (1_u32, 1_u32);
    }
    let exact = gcd(i128::from(long), i128::from(max_size));
    let divisor = u32::try_from(exact).unwrap_or(1_u32);
    let num = max_size.checked_div(divisor).unwrap_or(1_u32);
    let den = long
        .checked_div(divisor)
        .unwrap_or_else(|| max_size.max(1_u32));
    (num, den)
}

/// The area-filter downscale, the untouched image when the scale is `1/1`.
pub(super) fn downscale(img: RgbaImage, num: u32, den: u32) -> RgbaImage {
    if num == den {
        return img;
    }
    let width = fit_edge(img.width, num, den);
    let height = fit_edge(img.height, num, den);
    resample(&img, width, height, Filter::Area)
}

/// One edge scaled by `num / den` rounded, at least one pixel.
pub(super) fn fit_edge(edge: u32, num: u32, den: u32) -> u32 {
    scale_length(edge, num, den).max(1_u32)
}

/// The [`AnalyzeOptions`] of one request: the explicit pixel parameters
/// converted onto the analysis raster, `None` fields taking CD-101
/// defaults.
pub(super) fn analyze_options(request: &Request, frame: &Frame) -> AnalyzeOptions {
    AnalyzeOptions {
        ink: request.ink,
        close_radius: request.close_gaps.map(|value| frame.to_analysis(value)),
        min_region_area: request
            .min_region_area
            .map(|value| frame.to_analysis_area(value)),
        mark_max_size: request.mark_max_size.map(|value| frame.to_analysis(value)),
    }
}

/// The [`VectorizeOptions`] of one request.
pub(super) fn vectorize_options(request: &Request, frame: &Frame) -> VectorizeOptions {
    VectorizeOptions {
        expand: true,
        simplify_px: request.simplify.map(|value| frame.to_analysis(value)),
        orthogonal: request.orthogonal,
        weld_px: request.weld.map(|value| frame.to_analysis(value)),
    }
}

/// Vectorise unless the analysis found no regions (zero regions is not an
/// error for this tool; [`vectorize()`] would refuse it).
pub(super) fn run_vectorize(
    request: &Request,
    frame: &Frame,
    analysis: &Analysis,
) -> Result<Option<Vectorized>, CyberError> {
    if analysis.region_info.is_empty() {
        return Ok(None);
    }
    vectorize(analysis, &vectorize_options(request, frame)).map(Some)
}
