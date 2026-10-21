//! Sketch tracing tool: [`Cyberdemon::trace_sketch`], the measurement
//! bridge from a hand-drawn floor plan to `build_map_from_plan`.
//!
//! The pipeline is fetch (before any lock; no session is needed at all) →
//! [`decode_image`] → crop → downscale to `max_size` (area filter) →
//! [`analyze`] → [`vectorize`] → scale every coordinate back onto the
//! picture the caller sees (plus `crop` offsets), optionally mirrored into
//! map coordinates by the same [`Units`] `build_map_from_plan` takes. The
//! tool never names a region a room, a gap a door or a mark a monster: it
//! answers ids (`R1`, `G1`, `M1`), measurements, and a debug `PNG` with
//! those ids drawn on, so the calling model can match its own reading of
//! the sketch against the numbers. The debug overlay itself lives in
//! [`debug`].

mod debug;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::resample::resample;
use crate::gfx::ops::{Filter, MAX_OP_DIMENSION, pixel_at, rounded_div};
use crate::gfx::{BYTES_PER_PIXEL, pixel_index};
use crate::plan::Units;
use crate::server::Cyberdemon;
use crate::sketch::vectorize::RegionShape;
use crate::sketch::{
    Analysis, AnalyzeOptions, Gap, InkMode, Mark, VectorizeOptions, Vectorized, analyze, vectorize,
};
use crate::state::AppState;
use crate::tools::plan::params::UnitsParam;
use crate::tools::{fail, ok};

/// A pixel coordinate pair in the result's coordinate space.
type XY = (i64, i64);

/// An inclusive pixel box `(x0, y0, x1, y1)` in the result's space.
type PixelBox = (i64, i64, i64, i64);

/// Default [`TraceSketchParams::max_size`].
const DEFAULT_MAX_SIZE: u32 = 1_024_u32;
/// Smallest accepted `max_size`.
const MIN_MAX_SIZE: u32 = 256_u32;
/// Largest accepted `max_size` (the image-op edge limit).
const MAX_MAX_SIZE: u32 = MAX_OP_DIMENSION;
/// Default [`InkParams::tolerance`].
const DEFAULT_TOLERANCE: u8 = 48_u8;
/// Default snap grid for `map_points`, map units.
const DEFAULT_GRID: i64 = 8_i64;
/// Name the debug overlay is published under.
const DEBUG_FILENAME: &str = "sketch-debug.png";
/// Media type of the debug overlay.
const DEBUG_MIME: &str = "image/png";

/// What to do next, verbatim in every successful result.
const NEXT_STEPS: [&str; 4] = [
    "compare debug with the sketch; adjust ink/close_gaps/min_region_area if regions are wrong",
    "write a plan: rename rooms, set heights, add connections at gap centers",
    "build_map_from_plan with the same units",
    "render_topdown with this sketch as underlay",
];

/// Arguments for [`Cyberdemon::trace_sketch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct TraceSketchParams {
    /// The picture to measure: absolute path, `file://` URI, `http(s)://`
    /// URL or this server's asset/upload URL. No session is needed.
    source: String,
    /// Rectangle of the original image to analyse, in original pixels
    /// `{x, y, width, height}`; the whole image when unset. Result
    /// coordinates are always in original-image pixels.
    #[serde(default)]
    crop: Option<CropParams>,
    /// The longer image edge is downscaled (area filter) to at most this
    /// many pixels before analysis: `256..=4096`, default `1024`. All
    /// reported coordinates are converted back to original pixels.
    #[serde(default)]
    max_size: Option<u32>,
    /// Ink definition: `{mode: "dark" | "color" = "dark", level?,
    /// color?, tolerance? = 48}`.
    #[serde(default)]
    ink: Option<InkParams>,
    /// Closing radius in original pixels: breaks in the walls up to twice
    /// this wide are bridged and reported as gaps. Default from the stroke
    /// width (`min(2 * stroke, 12)` at analysis scale).
    #[serde(default)]
    close_gaps: Option<u32>,
    /// Smallest enclosed region area, in original square pixels. Default
    /// is a 1/1000-of-the-sheet floor at analysis scale.
    #[serde(default)]
    min_region_area: Option<u32>,
    /// Largest mark (dot, cross, letter) bounding-box edge, in original
    /// pixels. Default `3 * stroke + 8` at analysis scale.
    #[serde(default)]
    mark_max_size: Option<u32>,
    /// Polygon simplification tolerance in original pixels; default
    /// `max(2, stroke)` at analysis scale.
    #[serde(default)]
    simplify: Option<u32>,
    /// Straighten near-axis edges of the region polygons (default `true`).
    #[serde(default = "default_orthogonal")]
    orthogonal: bool,
    /// Welding tolerance in original pixels: region corners that nearly
    /// coincide get one shared coordinate; `0` disables. Default
    /// `max(2, stroke)` at analysis scale.
    #[serde(default)]
    weld: Option<u32>,
    /// The same `units` object `build_map_from_plan` and
    /// `render_topdown.underlay.units` take. When given, every reported
    /// point also gets map coordinates (`map_points` / `map_center`).
    #[serde(default)]
    units: Option<UnitsParam>,
    /// Snap grid in map units the `map_points` are pulled to (default `8`;
    /// `1` switches snapping off, `0` and negatives are refused).
    #[serde(default = "default_grid")]
    grid: i64,
}

/// One `crop` rectangle: an inclusive-start exclusive-end pixel box in
/// original-image coordinates.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
struct CropParams {
    /// Left edge, original pixels.
    x: u32,
    /// Top edge, original pixels.
    y: u32,
    /// Width, `1..=4096`.
    width: u32,
    /// Height, `1..=4096`.
    height: u32,
}

/// The `ink` parameter object.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct InkParams {
    /// Which pixels are ink: `"dark"` (default) or `"color"`.
    mode: Option<String>,
    /// Dark mode: fixed luminance cut-off `0..=255`; unset detects the
    /// `Otsu` level (reported as `threshold`).
    level: Option<u8>,
    /// Color mode: the pen colour `#RRGGBB`; required with
    /// `mode: "color"`.
    color: Option<String>,
    /// Color mode: per-channel match tolerance `0..=255`, default `48`.
    tolerance: u8,
}

impl Default for InkParams {
    /// The documented defaults: dark ink, detected level, no colour,
    /// tolerance `48`.
    fn default() -> Self {
        Self {
            mode: None,
            level: None,
            color: None,
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// The `serde` default for [`TraceSketchParams::orthogonal`].
const fn default_orthogonal() -> bool {
    true
}

/// The `serde` default for [`TraceSketchParams::grid`].
const fn default_grid() -> i64 {
    DEFAULT_GRID
}

/// The validated request: parameter texts resolved into engine values.
struct Request {
    /// The `source` to fetch.
    source: String,
    /// The crop to take, `None` for the whole image.
    crop: Option<CropParams>,
    /// The resolved longer-edge analysis limit.
    max_size: u32,
    /// The resolved ink definition.
    ink: InkMode,
    /// Closing radius in original pixels.
    close_gaps: Option<u32>,
    /// Minimum region area in original square pixels.
    min_region_area: Option<u32>,
    /// Maximum mark edge in original pixels.
    mark_max_size: Option<u32>,
    /// Simplification tolerance in original pixels.
    simplify: Option<u32>,
    /// Whether rings are straightened.
    orthogonal: bool,
    /// Welding tolerance in original pixels.
    weld: Option<u32>,
    /// The map transform, `None` when no `units` object was given.
    units: Option<Units>,
}

/// The analysis raster and the exact route back onto the source picture.
struct Frame {
    /// Numerator of the analysis-pixels-per-source-pixel scale (>= 1).
    num: u32,
    /// Denominator of that scale (>= 1).
    den: u32,
    /// Left edge of the crop in the decoded image.
    crop_x: i64,
    /// Top edge of the crop in the decoded image.
    crop_y: i64,
    /// Width of the decoded (uncropped) image.
    source_width: u32,
    /// Height of the decoded (uncropped) image.
    source_height: u32,
    /// The caller's map transform, if any.
    units: Option<Units>,
}

/// The picture analysis runs on, with its frame.
struct Prepared {
    /// The cropped (and downscaled) image [`analyze`] measures.
    picture: RgbaImage,
    /// The mapping from analysis pixels back to the decoded image.
    frame: Frame,
}

impl Frame {
    /// One analysis-space corner point in original-image pixels.
    fn point(&self, x: i64, y: i64) -> Option<XY> {
        let num = i64::from(self.num);
        let den = i64::from(self.den);
        let px = mul_div_round(x, den, num)?.checked_add(self.crop_x)?;
        let py = mul_div_round(y, den, num)?.checked_add(self.crop_y)?;
        Some((px, py))
    }

    /// One analysis-space pixel coordinate in original-image pixels.
    fn pixel_point(&self, x: u32, y: u32, what: &str) -> Result<XY, CyberError> {
        self.point(i64::from(x), i64::from(y))
            .ok_or_else(|| out_of_range(what))
    }

    /// A whole analysis-space ring in original-image pixels, refusing at
    /// `what` when a vertex leaves the reportable range.
    fn ring(&self, points: &[XY], what: &str) -> Result<Vec<XY>, CyberError> {
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
    fn bbox(&self, bbox: (u32, u32, u32, u32), what: &str) -> Result<PixelBox, CyberError> {
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
    fn map_points(&self, points: &[XY]) -> Result<Option<Vec<XY>>, CyberError> {
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
    fn map_centre(&self, x: u32, y: u32) -> Result<Option<XY>, CyberError> {
        let point = self.pixel_point(x, y, "centre")?;
        self.units.map_or_else(
            || Ok(None),
            |units| map_point(units, point.0, point.1).map(Some),
        )
    }

    /// An analysis-space length in original pixels.
    fn length(&self, value: u32) -> u32 {
        scale_length(value, self.den, self.num)
    }

    /// An analysis-space area in original square pixels.
    fn area(&self, value: u32) -> u32 {
        scale_area(value, self.den, self.num)
    }

    /// An original-pixel length at analysis scale.
    fn to_analysis(&self, value: u32) -> u32 {
        scale_length(value, self.num, self.den)
    }

    /// An original-square-pixel area at analysis scale.
    fn to_analysis_area(&self, value: u32) -> u32 {
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
fn scale_length(value: u32, num: u32, den: u32) -> u32 {
    let wide = u64::from(value).saturating_mul(u64::from(num));
    u32::try_from(rounded_div(wide, u64::from(den))).unwrap_or(u32::MAX)
}

/// `value * num^2 / den^2` rounded, saturating into `u32`.
fn scale_area(value: u32, num: u32, den: u32) -> u32 {
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

/// Resolve the parameter texts: sizes, ink, units and grid.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `max_size` outside `256..=4096`,
/// an unknown `ink.mode`, `color` mode without `color`, a malformed
/// `ink.color`, or a `units`/`grid` value [`Units::check`] refuses.
fn parse_request(args: &TraceSketchParams) -> Result<Request, CyberError> {
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
    })
}

/// The `max_size`: default `1024`, accepted `256..=4096`.
fn parse_max_size(value: Option<u32>) -> Result<u32, CyberError> {
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
fn parse_ink(ink: Option<&InkParams>) -> Result<InkMode, CyberError> {
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
fn parse_units(requested: Option<UnitsParam>, grid: i64) -> Result<Option<Units>, CyberError> {
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
fn prepare(request: &Request, decoded: &RgbaImage) -> Result<Prepared, CyberError> {
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
fn check_crop(crop_box: CropParams, img: &RgbaImage) -> Result<(), CyberError> {
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
fn scale_for(width: u32, height: u32, max_size: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_size {
        return (1_u32, 1_u32);
    }
    let divisor = gcd(long, max_size).max(1_u32);
    let num = max_size.checked_div(divisor).unwrap_or(1_u32);
    let den = long
        .checked_div(divisor)
        .unwrap_or_else(|| max_size.max(1_u32));
    (num, den)
}

/// The greatest common divisor of two edge lengths (`0` inputs yield `0`).
fn gcd(one: u32, two: u32) -> u32 {
    let mut big = one.max(two);
    let mut small = one.min(two);
    while small != 0 {
        let Some(rest) = big.checked_rem(small) else {
            break;
        };
        big = small;
        small = rest;
    }
    big
}

/// The area-filter downscale, the untouched image when the scale is `1/1`.
fn downscale(img: RgbaImage, num: u32, den: u32) -> RgbaImage {
    if num == den {
        return img;
    }
    let width = fit_edge(img.width, num, den);
    let height = fit_edge(img.height, num, den);
    resample(&img, width, height, Filter::Area)
}

/// One edge scaled by `num / den` rounded, at least one pixel.
fn fit_edge(edge: u32, num: u32, den: u32) -> u32 {
    scale_length(edge, num, den).max(1_u32)
}

/// The [`AnalyzeOptions`] of one request: the explicit pixel parameters
/// converted onto the analysis raster, `None` fields taking CD-101
/// defaults.
fn analyze_options(request: &Request, frame: &Frame) -> AnalyzeOptions {
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
fn vectorize_options(request: &Request, frame: &Frame) -> VectorizeOptions {
    VectorizeOptions {
        expand: true,
        simplify_px: request.simplify.map(|value| frame.to_analysis(value)),
        orthogonal: request.orthogonal,
        weld_px: request.weld.map(|value| frame.to_analysis(value)),
    }
}

/// Vectorise unless the analysis found no regions (zero regions is not an
/// error for this tool; [`vectorize`] would refuse it).
fn run_vectorize(
    request: &Request,
    frame: &Frame,
    analysis: &Analysis,
) -> Result<Option<Vectorized>, CyberError> {
    if analysis.region_info.is_empty() {
        return Ok(None);
    }
    vectorize(analysis, &vectorize_options(request, frame)).map(Some)
}

/// The `analysed_scale` report pair.
#[derive(Serialize)]
struct ScaleInfo {
    /// Analysis pixels per `den` source pixels.
    num: u32,
    /// Source pixels per `num` analysis pixels.
    den: u32,
}

/// The `image` report object.
#[derive(Serialize)]
struct ImageInfo {
    /// Width of the decoded image in pixels.
    width: u32,
    /// Height of the decoded image in pixels.
    height: u32,
    /// The exact reduced analysis-scale fraction.
    analysed_scale: ScaleInfo,
}

/// One `regions` row.
#[derive(Serialize)]
struct RegionRow {
    /// Measurement id, `R` plus the scan-order label.
    id: String,
    /// Ring in original-image pixels, clockwise in image space.
    points: Vec<XY>,
    /// The same ring in map units under `units`, `null` without them.
    map_points: Option<Vec<XY>>,
    /// Inclusive pixel box `(x0, y0, x1, y1)`.
    bbox: PixelBox,
    /// Pixels of the grown region, in original square pixels.
    area_px: u32,
    /// Rounded pixel centre `(x, y)`.
    centroid: XY,
    /// Exactly four vertices, all edges axis-aligned.
    is_rectangle: bool,
    /// Non-region islands inside the traced piece's box.
    holes: u32,
    /// Ids of the marks sitting in this region.
    marks: Vec<String>,
    /// Ids of the gaps bordering this region.
    gaps: Vec<String>,
}

/// One `adjacency` row.
#[derive(Serialize)]
struct AdjacencyRow {
    /// Ids of the touching regions, lower label first.
    a: String,
    /// The other region's id.
    b: String,
    /// Pixels of shared border, rescaled to original pixels.
    shared_px: u32,
}

/// One `gaps` row.
#[derive(Serialize)]
struct GapRow {
    /// Measurement id, `G` plus the scan-order label.
    id: String,
    /// Rounded centre `(x, y)` in original pixels.
    center: XY,
    /// The same centre in map units, `null` without `units`.
    map_center: Option<XY>,
    /// The larger box edge, original pixels.
    width_px: u32,
    /// The two region ids the gap sits between, `null` when fewer than
    /// two were found.
    between: Option<[String; 2]>,
}

/// One `marks` row.
#[derive(Serialize)]
struct MarkRow {
    /// Measurement id, `M` plus the scan-order label.
    id: String,
    /// Rounded centre `(x, y)` in original pixels.
    center: XY,
    /// The same centre in map units, `null` without `units`.
    map_center: Option<XY>,
    /// Inclusive pixel box `(x0, y0, x1, y1)`.
    bbox: PixelBox,
    /// Pixels in the mark, in original square pixels.
    area_px: u32,
    /// Mean `RGB` of the mark's pixels as `#RRGGBB`.
    color: String,
    /// The region id the mark sits in, `null` in the background.
    region: Option<String>,
}

/// One `plan_rooms` row: the exact wire shape of a plan room footprint.
#[derive(Serialize)]
struct PlanRoom {
    /// The region id, ready to rename into a room id.
    id: String,
    /// Ring in original-image pixels.
    points: Vec<XY>,
}

/// The structured result of [`Cyberdemon::trace_sketch`].
#[derive(Serialize)]
struct TraceReport {
    /// The decoded picture and the scale it was analysed at.
    image: ImageInfo,
    /// Median pen width, original pixels.
    stroke_px: u32,
    /// The dark-mode luminance cut-off applied, `null` in color mode.
    threshold: Option<u8>,
    /// The closing radius actually used, original pixels.
    close_gaps_px: u32,
    /// The enclosed regions, in scan order.
    regions: Vec<RegionRow>,
    /// Region pairs that share a wall after growth.
    adjacency: Vec<AdjacencyRow>,
    /// Breaks the closing bridged in the walls.
    gaps: Vec<GapRow>,
    /// Small ink components (dots, crosses, letters).
    marks: Vec<MarkRow>,
    /// The region polygons in plan-room wire shape, original pixels.
    plan_rooms: Vec<PlanRoom>,
    /// The published debug overlay.
    debug: AssetRef,
    /// Measurement caveats from the analysis and the vectoriser.
    warnings: Vec<String>,
    /// What to look at and do next.
    next_steps: Vec<&'static str>,
}

/// The `id` for an analysis label: `R`/`G`/`M` plus the number.
fn tag(prefix: char, id: u32) -> String {
    format!("{prefix}{id}")
}

/// One vectorised region as a report row.
fn region_row(frame: &Frame, shape: &RegionShape) -> Result<RegionRow, CyberError> {
    let id = tag('R', shape.id);
    let points = frame.ring(&shape.points, &format!("region {id}"))?;
    let map_points = frame.map_points(&points)?;
    let centroid =
        frame.pixel_point(shape.centroid.0, shape.centroid.1, &format!("region {id}"))?;
    Ok(RegionRow {
        bbox: frame.bbox(shape.bbox, &format!("region {id}"))?,
        area_px: frame.area(shape.area_px),
        centroid,
        gaps: shape.gaps.iter().map(|gap| tag('G', *gap)).collect(),
        holes: shape.holes,
        id,
        is_rectangle: shape.is_rectangle,
        map_points,
        marks: shape.marks.iter().map(|mark| tag('M', *mark)).collect(),
        points,
    })
}

/// One bridged wall break as a report row.
fn gap_row(frame: &Frame, gap: &Gap) -> Result<GapRow, CyberError> {
    let id = tag('G', gap.id);
    let center = frame.pixel_point(gap.centroid.0, gap.centroid.1, &format!("gap {id}"))?;
    let map_center = frame.map_centre(gap.centroid.0, gap.centroid.1)?;
    let between = gap.between.map(|(one, two)| [tag('R', one), tag('R', two)]);
    Ok(GapRow {
        between,
        center,
        id,
        map_center,
        width_px: frame.length(gap.width_px),
    })
}

/// One small ink component as a report row.
fn mark_row(frame: &Frame, mark: &Mark) -> Result<MarkRow, CyberError> {
    let id = tag('M', mark.id);
    let center = frame.pixel_point(mark.centroid.0, mark.centroid.1, &format!("mark {id}"))?;
    let map_center = frame.map_centre(mark.centroid.0, mark.centroid.1)?;
    let [red, green, blue] = mark.color;
    Ok(MarkRow {
        area_px: frame.area(mark.area),
        bbox: frame.bbox(mark.bbox, &format!("mark {id}"))?,
        center,
        color: format!("#{red:02X}{green:02X}{blue:02X}"),
        id,
        map_center,
        region: mark.region.map(|region| tag('R', region)),
    })
}

/// Assemble the client-facing report from the measured picture.
fn build_report(
    frame: &Frame,
    analysis: &Analysis,
    vectorized: Option<&Vectorized>,
    asset: AssetRef,
) -> Result<TraceReport, CyberError> {
    let regions: Vec<RegionRow> = match vectorized {
        None => Vec::new(),
        Some(shapes) => shapes
            .regions
            .iter()
            .map(|shape| region_row(frame, shape))
            .collect::<Result<Vec<RegionRow>, CyberError>>()?,
    };
    let adjacency: Vec<AdjacencyRow> = vectorized
        .map(|shapes| {
            shapes
                .adjacency
                .iter()
                .map(|pair| AdjacencyRow {
                    a: tag('R', pair.a),
                    b: tag('R', pair.b),
                    shared_px: frame.length(pair.shared_px),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut warnings = analysis.warnings.clone();
    if let Some(shapes) = vectorized {
        warnings.extend(shapes.warnings.iter().cloned());
    }
    Ok(TraceReport {
        image: ImageInfo {
            width: frame.source_width,
            height: frame.source_height,
            analysed_scale: ScaleInfo {
                num: frame.num,
                den: frame.den,
            },
        },
        stroke_px: frame.length(analysis.stroke),
        threshold: analysis.threshold,
        close_gaps_px: frame.length(analysis.close_radius),
        plan_rooms: regions
            .iter()
            .map(|region| PlanRoom {
                id: region.id.clone(),
                points: region.points.clone(),
            })
            .collect(),
        gaps: analysis
            .gaps
            .iter()
            .map(|gap| gap_row(frame, gap))
            .collect::<Result<Vec<GapRow>, CyberError>>()?,
        marks: analysis
            .marks
            .iter()
            .map(|mark| mark_row(frame, mark))
            .collect::<Result<Vec<MarkRow>, CyberError>>()?,
        regions,
        adjacency,
        debug: asset,
        warnings,
        next_steps: NEXT_STEPS.to_vec(),
    })
}

/// `trace_sketch`' work: validate, fetch, decode, crop, downscale,
/// analyse, vectorise, draw the debug overlay, publish, report.
///
/// # Errors
///
/// [`parse_request`]'s refusals, [`AppState::fetch_source`]'s errors for
/// the `source`, [`decode_image`]'s for bytes that are no readable image,
/// [`check_crop`]'s and [`analyze`]'s argument errors, [`vectorize`]'s
/// only when regions exist but none traces, and [`AppState::publish`]'
/// failures. Zero regions is not an error: the lists come back empty
/// with the analysis warning and the debug image.
async fn trace_run(state: &AppState, args: &TraceSketchParams) -> Result<TraceReport, CyberError> {
    let request = parse_request(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let decoded = decode_image(&fetched.bytes)?;
    let prepared = prepare(&request, &decoded)?;
    let options = analyze_options(&request, &prepared.frame);
    let analysis = analyze(&prepared.picture, &options)?;
    let vectorized = run_vectorize(&request, &prepared.frame, &analysis)?;
    let overlay = debug::build(&prepared.picture, &analysis, vectorized.as_ref());
    let png = encode_png(&overlay, None)?;
    let asset = state.publish(DEBUG_FILENAME, DEBUG_MIME, png)?;
    build_report(&prepared.frame, &analysis, vectorized.as_ref(), asset)
}

/// The full documentation of [`Cyberdemon::trace_sketch`].
const TRACE_TOOL_DESCRIPTION: &str = "Measures a hand-drawn floor plan (PNG, JPEG, GIF or BMP from source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL) and returns exact numbers for it: enclosed regions with welded, simplified integer polygons, the gaps where wall lines break, the small marks (dots, crosses, letters) filtered off the walls, region adjacency, and a debug PNG with every id drawn on. No session or open WAD is needed. This tool only measures: it never decides that a region is a room, a gap a door or a mark a monster - match your own reading of the sketch against the ids (R1, G1, M1) on the debug image. Parameters: crop {x, y, width, height} analyses a rectangle (coordinates still come back in original-image pixels); max_size (256..=4096, default 1024) downscales the image (area filter) so its longer side fits before analysis, and every coordinate is converted back to original pixels, so results match the picture you see; analysed_scale {num, den} in the result is the exact analysis-source fraction. ink {mode: \"dark\" (default) | \"color\", level?, color?, tolerance? = 48}: dark ink is luminance at or below level (unset: the detected Otsu level, reported as threshold), color ink is pixels within tolerance of the #RRGGBB pen (mode \"color\" without color is invalid_argument). close_gaps is the closing radius in original pixels - breaks up to twice it wide are bridged and reported as gaps (default from the measured stroke); min_region_area (original square pixels) drops smaller enclosures; mark_max_size (original pixels) bounds what counts as a mark. simplify (original pixels) is the polygon tolerance, orthogonal (default true) straightens near-axis edges, weld (original pixels, default from the stroke, 0 disables) gives shared walls one identical coordinate across regions. units {scale_num, scale_den, origin_x, origin_y, flip_y} (the same object build_map_from_plan and render_topdown.underlay.units take) adds map_points/map_center to every point under the grid snap (grid, default 8, 1 off); flip_y: true mirrors image-space y into map coordinates. Errors: unsupported/format_error for no readable image, invalid_argument for a bad crop, max_size, ink or units. Zero regions is NOT an error: empty lists plus the warning and the debug image come back. Result: {image {width, height, analysed_scale}, stroke_px, threshold, close_gaps_px, regions [{id, points, map_points, bbox, area_px, centroid, is_rectangle, holes, marks, gaps}], adjacency [{a, b, shared_px}], gaps [{id, center, map_center, width_px, between}], marks [{id, center, map_center, bbox, area_px, color, region}], plan_rooms [{id, points}], debug (asset), warnings, next_steps}. Region points are clockwise in image space (the plan compiler accepts either orientation); plan_rooms is exactly the rooms shape build_map_from_plan takes - pair it with the same units. The debug PNG shows the sketch washed out, regions in distinct colours with their ids, gaps as magenta circles (radius = width/2) and marks in cyan boxes.";

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = sketch_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Measures a hand-drawn floor plan into regions, gaps, marks and a
    /// debug overlay.
    #[tool(description = TRACE_TOOL_DESCRIPTION)]
    async fn trace_sketch(
        &self,
        Parameters(args): Parameters<TraceSketchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match trace_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CropParams, Frame, InkParams, Request, TraceSketchParams, analyze_options, check_crop,
        default_grid, default_orthogonal, downscale, fit_edge, gap_row, mark_row, parse_ink,
        parse_max_size, parse_request, parse_units, region_row, scale_area, scale_for,
        scale_length, tag, vectorize_options,
    };
    use crate::gfx::image::RgbaImage;
    use crate::plan::Units;
    use crate::sketch::vectorize::RegionShape;
    use crate::sketch::{Gap, InkMode, Mark, analyze, vectorize};

    /// A `width` by `height` white test image.
    fn paper(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for _ in 0..width.saturating_mul(height) {
            pixels.extend_from_slice(&[255, 255, 255, 255]);
        }
        RgbaImage::new(width, height, pixels).expect("fixture")
    }

    /// A frame at scale `num / den` with no crop and no units.
    fn frame(num: u32, den: u32) -> Frame {
        Frame {
            num,
            den,
            crop_x: 0_i64,
            crop_y: 0_i64,
            source_width: 800_u32,
            source_height: 600_u32,
            units: None,
        }
    }

    /// The trace parameters with only the source set.
    fn args(source: &str) -> TraceSketchParams {
        TraceSketchParams {
            source: source.to_owned(),
            crop: None,
            max_size: None,
            ink: None,
            close_gaps: None,
            min_region_area: None,
            mark_max_size: None,
            simplify: None,
            orthogonal: default_orthogonal(),
            weld: None,
            units: None,
            grid: default_grid(),
        }
    }

    /// An identity-region fixture for the row builders.
    fn shape() -> RegionShape {
        RegionShape {
            id: 2,
            points: vec![
                (10_i64, 10_i64),
                (90_i64, 10_i64),
                (90_i64, 90_i64),
                (10_i64, 90_i64),
            ],
            area_px: 6_400_u32,
            bbox: (10_u32, 10_u32, 89_u32, 89_u32),
            centroid: (50_u32, 50_u32),
            is_rectangle: true,
            holes: 0_u32,
            marks: vec![1_u32],
            gaps: vec![3_u32],
        }
    }

    #[test]
    fn max_size_defaults_to_1024_and_refuses_the_out_of_range() {
        assert_eq!(parse_max_size(None).expect("default"), 1_024_u32);
        assert_eq!(parse_max_size(Some(256)).expect("floor"), 256_u32);
        assert_eq!(parse_max_size(Some(4_096)).expect("ceiling"), 4_096_u32);
        for bad in [0_u32, 64_u32, 4_097_u32] {
            let error = parse_max_size(Some(bad)).expect_err("out of range");
            assert_eq!(error.code(), "invalid_argument", "code for {bad}");
            assert!(
                error.to_string().contains("between 256 and 4096"),
                "the message names the range: {error}"
            );
        }
    }

    #[test]
    fn ink_defaults_to_dark_and_color_needs_a_pen() {
        assert_eq!(
            parse_ink(None).expect("absent"),
            InkMode::Dark { level: None },
            "unset asks for the detected level"
        );
        let dark = InkParams {
            mode: Some("Dark".to_owned()),
            level: Some(128_u8),
            ..InkParams::default()
        };
        assert_eq!(
            parse_ink(Some(&dark)).expect("explicit"),
            InkMode::Dark { level: Some(128) },
            "the level rides through, case-insensitively"
        );
        let color = InkParams {
            mode: Some("color".to_owned()),
            color: Some("#C02020".to_owned()),
            ..InkParams::default()
        };
        assert_eq!(
            parse_ink(Some(&color)).expect("a pen"),
            InkMode::Color {
                rgb: [192, 32, 32],
                tolerance: 48,
            },
            "the default tolerance is 48"
        );
        let bare = InkParams {
            mode: Some("color".to_owned()),
            ..InkParams::default()
        };
        let error = parse_ink(Some(&bare)).expect_err("no colour given");
        assert_eq!(error.code(), "invalid_argument", "color needs color");
        assert!(
            error.to_string().contains("ink.color"),
            "the message names the field: {error}"
        );
        let odd = InkParams {
            mode: Some("pencil".to_owned()),
            ..InkParams::default()
        };
        assert_eq!(
            parse_ink(Some(&odd)).expect_err("not a mode").code(),
            "invalid_argument",
            "unknown modes are refused"
        );
    }

    #[test]
    fn units_stay_none_until_the_wire_gives_them() {
        assert!(parse_units(None, 8_i64).expect("absent").is_none());
        let wire = crate::tools::plan::params::UnitsParam {
            scale_num: 4_i64,
            ..crate::tools::plan::params::UnitsParam::default()
        };
        let given = parse_units(Some(wire), 16_i64)
            .expect("a wire object")
            .expect("Some");
        assert_eq!(given.scale_num, 4_i64, "the scale travels");
        assert_eq!(given.grid, 16_i64, "the grid rides inside the units");
        let error = parse_units(None, 0_i64).expect_err("zero grid");
        assert_eq!(error.code(), "invalid_argument", "grid 0 is refused");
    }

    #[test]
    fn the_scale_reduces_to_an_exact_fraction() {
        assert_eq!(
            scale_for(400_u32, 300_u32, 1_024_u32),
            (1_u32, 1_u32),
            "fits"
        );
        assert_eq!(scale_for(2_400_u32, 1_800_u32, 600_u32), (1_u32, 4_u32));
        assert_eq!(scale_for(1_200_u32, 900_u32, 1_024_u32), (64_u32, 75_u32));
        assert_eq!(
            scale_for(1_024_u32, 10_u32, 1_024_u32),
            (1_u32, 1_u32),
            "edge"
        );
    }

    #[test]
    fn the_frame_maps_points_lengths_and_areas_back_exactly() {
        let shrunk = frame(1_u32, 4_u32);
        assert_eq!(shrunk.point(10_i64, 20_i64), Some((40_i64, 80_i64)));
        assert_eq!(shrunk.length(6_u32), 24_u32, "a 6 px stroke is 24 at 1/4");
        assert_eq!(shrunk.area(100_u32), 1_600_u32, "areas square the scale");
        assert_eq!(shrunk.to_analysis(96_u32), 24_u32, "lengths go in");
        assert_eq!(shrunk.to_analysis_area(1_600_u32), 100_u32, "areas go in");
        let halved = frame(1_u32, 2_u32);
        assert_eq!(halved.point(21_i64, 21_i64), Some((42_i64, 42_i64)));
        assert_eq!(halved.point(21_i64, 22_i64), Some((42_i64, 44_i64)));
        assert_eq!(fit_edge(901_u32, 1_u32, 2_u32), 451_u32, "halves round up");
        assert_eq!(scale_length(3_u32, 1_u32, 2_u32), 2_u32, "1.5 rounds to 2");
        assert_eq!(scale_area(1_u32, 2_u32, 1_u32), 4_u32, "a px is 4 at 2x");
    }

    #[test]
    fn a_crop_offset_shifts_every_mapped_point() {
        let mut shifted = frame(1_u32, 1_u32);
        shifted.crop_x = 10_i64;
        shifted.crop_y = 20_i64;
        assert_eq!(shifted.point(5_i64, 5_i64), Some((15_i64, 25_i64)));
        assert_eq!(
            shifted
                .bbox((5_u32, 5_u32, 9_u32, 9_u32), "x")
                .expect("in range"),
            (15_i64, 25_i64, 19_i64, 29_i64),
            "the inclusive far edge stays inclusive"
        );
    }

    #[test]
    fn units_add_map_points_and_none_stays_null() {
        let plain = frame(1_u32, 1_u32);
        assert_eq!(
            plain.map_points(&[(8_i64, 16_i64)]).expect("no units"),
            None
        );
        let mapped = Frame {
            units: Some(Units {
                scale_num: 4_i64,
                flip_y: true,
                ..Units::default()
            }),
            ..plain
        };
        let rows = mapped
            .map_points(&[(8_i64, 16_i64)])
            .expect("in range")
            .expect("a row list");
        assert_eq!(rows, vec![(32_i64, -64_i64)], "scale 4, mirrored");
        assert_eq!(
            mapped.map_centre(8_u32, 16_u32).expect("in range"),
            Some((32_i64, -64_i64)),
            "centres follow the same transform"
        );
    }

    #[test]
    fn crop_edges_outside_the_image_are_refused_naming_the_field() {
        let img = paper(100_u32, 80_u32);
        let good = CropParams {
            x: 10_u32,
            y: 10_u32,
            width: 80_u32,
            height: 60_u32,
        };
        check_crop(good, &img).expect("inside");
        let wide = CropParams {
            width: 95_u32,
            ..good
        };
        let error = check_crop(wide, &img).expect_err("past the right edge");
        assert_eq!(error.code(), "invalid_argument");
        assert!(error.to_string().contains("crop.x"), "{error}");
        let zero = CropParams {
            height: 0_u32,
            ..good
        };
        let empty = check_crop(zero, &img).expect_err("empty");
        assert!(empty.to_string().contains("crop.height"), "{empty}");
    }

    #[test]
    fn explicit_parameters_land_on_the_analysis_raster() {
        let mut base = args("/x.png");
        base.close_gaps = Some(96_u32);
        base.min_region_area = Some(1_600_u32);
        base.mark_max_size = Some(40_u32);
        base.simplify = Some(8_u32);
        base.weld = Some(0_u32);
        base.orthogonal = false;
        let request = parse_request(&base).expect("the wire parses");
        let shrunk = frame(1_u32, 4_u32);
        let options = analyze_options(&request, &shrunk);
        assert_eq!(options.close_radius, Some(24_u32));
        assert_eq!(options.min_region_area, Some(100_u32));
        assert_eq!(options.mark_max_size, Some(10_u32));
        let voptions = vectorize_options(&request, &shrunk);
        assert_eq!(voptions.simplify_px, Some(2_u32));
        assert_eq!(voptions.weld_px, Some(0_u32), "weld 0 keeps its meaning");
        assert!(!voptions.orthogonal, "the flag rides through");
        assert!(
            voptions.expand,
            "regions always grow to the wall centrelines"
        );
    }

    #[test]
    fn region_rows_carry_tags_and_the_scaled_geometry() {
        let shrunk = frame(1_u32, 2_u32);
        let row = region_row(&shrunk, &shape()).expect("in range");
        assert_eq!(row.id, "R2", "the label id is prefixed");
        assert_eq!(row.points.first().copied(), Some((20_i64, 20_i64)));
        assert_eq!(
            row.bbox,
            (20_i64, 20_i64, 179_i64, 179_i64),
            "inclusive edges"
        );
        assert_eq!(row.area_px, 25_600_u32, "the area squares the scale");
        assert_eq!(row.centroid, (100_i64, 100_i64));
        assert_eq!(row.marks, vec!["M1".to_owned()], "mark ids");
        assert_eq!(row.gaps, vec!["G3".to_owned()], "gap ids");
        assert!(row.is_rectangle && row.holes == 0, "flags ride through");
    }

    #[test]
    fn gap_and_mark_rows_scale_centres_and_colours() {
        let shrunk = frame(1_u32, 2_u32);
        let gap = Gap {
            id: 1_u32,
            centroid: (30_u32, 60_u32),
            bbox: (28_u32, 58_u32, 32_u32, 62_u32),
            width_px: 15_u32,
            between: Some((2_u32, 3_u32)),
        };
        let gap_row_data = gap_row(&shrunk, &gap).expect("in range");
        assert_eq!(gap_row_data.id, "G1");
        assert_eq!(gap_row_data.center, (60_i64, 120_i64));
        assert_eq!(gap_row_data.width_px, 30_u32);
        assert_eq!(
            gap_row_data.between,
            Some(["R2".to_owned(), "R3".to_owned()]),
            "the pair is labelled"
        );
        let mark = Mark {
            id: 2_u32,
            centroid: (10_u32, 20_u32),
            bbox: (8_u32, 18_u32, 12_u32, 22_u32),
            area: 25_u32,
            color: [192, 32, 32],
            region: Some(1_u32),
        };
        let mark_row_data = mark_row(&shrunk, &mark).expect("in range");
        assert_eq!(mark_row_data.id, "M2");
        assert_eq!(mark_row_data.color, "#C02020", "upper-case hex");
        assert_eq!(mark_row_data.area_px, 100_u32);
        assert_eq!(mark_row_data.region, Some("R1".to_owned()));
        assert_eq!(tag('R', 7_u32), "R7");
    }

    #[test]
    fn downscaling_keeps_the_identity_at_1_to_1() {
        let kept = downscale(paper(20_u32, 10_u32), 1_u32, 1_u32);
        assert_eq!((kept.width, kept.height), (20_u32, 10_u32), "no resample");
        let shrunk = downscale(paper(20_u32, 10_u32), 1_u32, 2_u32);
        assert_eq!((shrunk.width, shrunk.height), (10_u32, 5_u32));
        assert_eq!(fit_edge(1_u32, 1_u32, 4_096_u32), 1_u32, "never zero");
    }

    /// Paints the solid half-open rectangle `rect` scaled by `k`.
    fn scaled_bar(sheet: &mut RgbaImage, k: u32, rect: (u32, u32, u32, u32), colour: [u8; 4]) {
        let (x0, y0, x1, y1) = rect;
        let points = (y0.checked_mul(k).expect("t")..y1.checked_mul(k).expect("t")).flat_map(|y| {
            (x0.checked_mul(k).expect("t")..x1.checked_mul(k).expect("t")).map(move |x| (x, y))
        });
        for (column, row) in points {
            assert!(sheet.set(column, row, colour), "fixture in bounds");
        }
    }

    /// The three-room fixture's walls drawn at `k` times.
    fn rooms_at(k: u32) -> RgbaImage {
        let mut sheet = paper(
            k.checked_mul(400).expect("t"),
            k.checked_mul(300).expect("t"),
        );
        let walls: [(u32, u32, u32, u32); 12] = [
            (20, 20, 24, 164),
            (20, 20, 94, 24),
            (90, 20, 94, 94),
            (90, 90, 154, 94),
            (150, 90, 154, 164),
            (20, 160, 154, 164),
            (240, 20, 244, 281),
            (357, 20, 361, 281),
            (240, 20, 361, 24),
            (240, 277, 361, 281),
            (240, 157, 281, 161),
            (311, 157, 361, 161),
        ];
        for wall in walls {
            scaled_bar(&mut sheet, k, wall, [0, 0, 0, 255]);
        }
        sheet
    }

    #[test]
    fn a_downscaled_fixture_still_measures_three_regions() {
        let shrunk_image = rooms_at(6);
        let (num, den) = scale_for(shrunk_image.width, shrunk_image.height, 600_u32);
        assert_eq!((num, den), (1_u32, 4_u32), "2400 into 600 is a quarter");
        let picture = downscale(shrunk_image, num, den);
        assert_eq!((picture.width, picture.height), (600_u32, 450_u32));
        let frame = Frame {
            num,
            den,
            crop_x: 0_i64,
            crop_y: 0_i64,
            source_width: 2_400_u32,
            source_height: 1_800_u32,
            units: None,
        };
        let request = Request {
            source: String::new(),
            crop: None,
            max_size: 600_u32,
            ink: InkMode::Dark { level: None },
            close_gaps: Some(100_u32),
            min_region_area: None,
            mark_max_size: None,
            simplify: None,
            orthogonal: true,
            weld: None,
            units: None,
        };
        let options = analyze_options(&request, &frame);
        let result = analyze(&picture, &options).expect("analysable");
        assert_eq!(result.close_radius, 25_u32, "100 original px at 1/4");
        assert_eq!(result.region_info.len(), 3, "three rooms survive the blur");
        let shapes =
            vectorize(&result, &vectorize_options(&request, &frame)).expect("vectorizable");
        assert_eq!(shapes.regions.len(), 3, "three vectorised rings");
    }
}
