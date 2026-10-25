//! Sketch tracing tool: [`Cyberdemon::trace_sketch`], the measurement
//! bridge from a hand-drawn floor plan to `build_map_from_plan`.
//!
//! The pipeline is fetch (before any lock; no session is needed at all) →
//! [`decode_image`] → crop → downscale to `max_size` (area filter) →
//! [`analyze()`] → [`crate::sketch::vectorize()`] → scale every coordinate back
//! onto the picture the caller sees (plus the `crop` offsets), optionally
//! mirrored into map coordinates by the same [`crate::plan::Units`]
//! `build_map_from_plan` takes. The tool never names a region a room, a gap
//! a door or a mark a monster: it answers ids (`R1`, `G1`, `M1`),
//! measurements, and a debug `PNG` with those ids drawn on, so the calling
//! model can match its own reading of the sketch against the numbers. The
//! debug overlay itself lives in [`debug`].
//!
//! A noisy scan (ruled paper, uneven light) measures thousands of little
//! things, so the two long lists are thinned before they are reported:
//! rows under the noise floor are dropped, the rest are ranked largest
//! first and cut to `max_gaps` / `max_marks`. The report always carries the
//! totals it found, a `truncated` flag and warnings naming what stayed out,
//! and both the debug overlay and the regions' id lists carry exactly the
//! rows the report shows. `verbose: true` lifts the floor and both caps.
//!
//! The work is split across the private submodules: [`mod@request`]
//! resolves the wire parameters and prepares the picture, [`mod@thin`]
//! decides which gaps and marks are worth a row, [`mod@report`] builds
//! every row and the whole [`TraceReport`], [`mod@debug`] draws the
//! labelled overlay. This root keeps what they share - the parameter types,
//! the constants, the run and the router - and the group's unit tests live
//! in `sketch/tests.rs`.

mod debug;
mod report;
mod request;
mod thin;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::ops::MAX_OP_DIMENSION;
use crate::server::Cyberdemon;
use crate::sketch::analyze;
use crate::state::AppState;
use crate::tools::plan::params::UnitsParam;
use crate::tools::reply;
use crate::tools::sketch::report::{Parts, TraceReport, assemble};
use crate::tools::sketch::request::{analyze_options, parse_request, prepare, run_vectorize};
use crate::tools::sketch::thin::thin_all;

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
/// Default [`TraceSketchParams::max_gaps`] and
/// [`TraceSketchParams::max_marks`].
const DEFAULT_MAX_ROWS: usize = 50_usize;
/// Largest accepted row cap.
const MAX_MAX_ROWS: usize = 500_usize;
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

/// The extra step a thinned report carries.
const THINNED_STEP: &str =
    "gaps or marks stayed out: raise max_gaps/max_marks, lower noise_floor_px or pass verbose";

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
    /// The most gap rows to report, `1..=500`; default `50`. The widest
    /// gaps come first, `gaps_total` counts everything the analysis found
    /// and a warning names how many stayed out.
    #[serde(default)]
    max_gaps: Option<usize>,
    /// The most mark rows to report, `1..=500`; default `50`. The marks
    /// holding the most ink come first; see `max_gaps`.
    #[serde(default)]
    max_marks: Option<usize>,
    /// Smallest gap width, and smallest mark box edge, worth a row, in
    /// original pixels. Default: the measured `stroke_px`, which drops the
    /// speckle noise a photo scan picks up; `0` keeps everything.
    #[serde(default)]
    noise_floor_px: Option<u32>,
    /// Report every gap and mark: the noise floor and both caps are lifted
    /// (default `false`). On a noisy scan the result can then outgrow the
    /// client's size limit, which is why the thinning is the default.
    #[serde(default)]
    verbose: bool,
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

/// `trace_sketch`' work: validate, fetch, decode, crop, downscale,
/// analyse, vectorise, thin the two long lists, draw the debug overlay of
/// what is reported, publish, report.
///
/// # Errors
///
/// [`parse_request`]'s refusals, [`AppState::fetch_source`]'s errors for
/// the `source`, [`decode_image`]'s for bytes that are no readable image,
/// [`request::check_crop`]'s and [`analyze()`]'s argument errors,
/// [`crate::sketch::vectorize()`]'s only when regions exist but none traces,
/// [`thin_all`]'s mapping refusals, and [`AppState::publish`]' failures.
/// Zero regions is not an error: the lists come back empty with the
/// analysis warning and the debug image.
async fn trace_run(state: &AppState, args: &TraceSketchParams) -> Result<TraceReport, CyberError> {
    let request = parse_request(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let decoded = decode_image(&fetched.bytes)?;
    let prepared = prepare(&request, &decoded)?;
    let options = analyze_options(&request, &prepared.frame);
    let analysis = analyze(&prepared.picture, &options)?;
    let vectorized = run_vectorize(&request, &prepared.frame, &analysis)?;
    let thinned = thin_all(&prepared.frame, &analysis, &request)?;
    let overlay = debug::build(
        &prepared.picture,
        vectorized.as_ref(),
        &thinned.gaps.kept.items,
        &thinned.marks.kept.items,
    );
    let png = encode_png(&overlay, None)?;
    let asset = state.publish(DEBUG_FILENAME, DEBUG_MIME, png)?;
    let parts = Parts {
        frame: &prepared.frame,
        analysis: &analysis,
        vectorized: vectorized.as_ref(),
        thinned,
        asset,
    };
    assemble(parts)
}

/// The full documentation of [`Cyberdemon::trace_sketch`].
const TRACE_TOOL_DESCRIPTION: &str = "Measures a hand-drawn floor plan (PNG, JPEG, GIF or BMP from source) and returns exact numbers: enclosed regions as welded, simplified integer polygons, the gaps where wall lines break, the small marks, region adjacency, and a debug PNG labelling everything R1, G1, M1. No session or open WAD is needed, and it only measures - you decide what is a room. Gaps and marks are floored at the measured stroke and capped (max_gaps, max_marks) so a noisy scan stays small; verbose lifts both. plan_rooms feeds build_map_from_plan with the same units; guides/sketch-to-map.md.";

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
        reply(trace_run(self.state(), &args).await)
    }
}

#[cfg(test)]
mod tests;
