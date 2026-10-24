//! The `trace_sketch` report: the row types the tool answers with and the
//! one place that builds the whole [`TraceReport`].
//!
//! Every row carries the measurement ids the tool answers (`R1`, `G1`,
//! `M1`) and numbers in original-image pixels or, under `units`, map units,
//! mapped back through [`Frame`]. [`assemble`] folds the raw [`Analysis`],
//! the vectorised shapes, the two thinned lists and the published overlay
//! into the client-facing report: the region rows' `marks` and `gaps` id
//! lists are narrowed to exactly the rows [`super::thin`] kept, the
//! warnings are gathered in report order and [`next_steps`] adds the
//! thinning's hint when either cap left rows out.

use alloc::collections::BTreeSet;

use serde::Serialize;

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::sketch::vectorize::RegionShape;
use crate::sketch::{Analysis, Gap, Mark, Vectorized};

use super::request::Frame;
use super::thin::{Thinned, gap_labels, mark_labels};
use super::{NEXT_STEPS, PixelBox, THINNED_STEP, XY};

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
pub(super) struct RegionRow {
    /// Measurement id, `R` plus the scan-order label.
    pub(super) id: String,
    /// Ring in original-image pixels, clockwise in image space.
    pub(super) points: Vec<XY>,
    /// The same ring in map units under `units`, `null` without them.
    pub(super) map_points: Option<Vec<XY>>,
    /// Inclusive pixel box `(x0, y0, x1, y1)`.
    pub(super) bbox: PixelBox,
    /// Pixels of the grown region, in original square pixels.
    pub(super) area_px: u32,
    /// Rounded pixel centre `(x, y)`.
    pub(super) centroid: XY,
    /// Exactly four vertices, all edges axis-aligned.
    pub(super) is_rectangle: bool,
    /// Non-region islands inside the traced piece's box.
    pub(super) holes: u32,
    /// Ids of the reported marks sitting in this region.
    pub(super) marks: Vec<String>,
    /// Ids of the reported gaps bordering this region.
    pub(super) gaps: Vec<String>,
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
pub(super) struct GapRow {
    /// Measurement id, `G` plus the scan-order label.
    pub(super) id: String,
    /// Rounded centre `(x, y)` in original pixels.
    pub(super) center: XY,
    /// The same centre in map units, `null` without `units`.
    pub(super) map_center: Option<XY>,
    /// The larger box edge, original pixels.
    pub(super) width_px: u32,
    /// The two region ids the gap sits between, `null` when fewer than
    /// two were found.
    pub(super) between: Option<[String; 2]>,
}

/// One `marks` row.
#[derive(Serialize)]
pub(super) struct MarkRow {
    /// Measurement id, `M` plus the scan-order label.
    pub(super) id: String,
    /// Rounded centre `(x, y)` in original pixels.
    pub(super) center: XY,
    /// The same centre in map units, `null` without `units`.
    pub(super) map_center: Option<XY>,
    /// Inclusive pixel box `(x0, y0, x1, y1)`.
    pub(super) bbox: PixelBox,
    /// Pixels in the mark, in original square pixels.
    pub(super) area_px: u32,
    /// Mean `RGB` of the mark's pixels as `#RRGGBB`.
    pub(super) color: String,
    /// The region id the mark sits in, `null` in the background.
    pub(super) region: Option<String>,
}

/// One `plan_rooms` row: the exact wire shape of a plan room footprint.
#[derive(Serialize)]
struct PlanRoom {
    /// The region id, ready to rename into a room id.
    id: String,
    /// Ring in original-image pixels.
    points: Vec<XY>,
}

/// The structured result of [`crate::server::Cyberdemon::trace_sketch`].
#[derive(Serialize)]
pub(super) struct TraceReport {
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
    /// Breaks the closing bridged in the walls, widest first, at most
    /// `max_gaps` of them.
    gaps: Vec<GapRow>,
    /// Gaps the analysis found, before the noise floor and the cap.
    gaps_total: u32,
    /// Small ink components (dots, crosses, letters), most ink first, at
    /// most `max_marks` of them.
    marks: Vec<MarkRow>,
    /// Marks the analysis found, before the noise floor and the cap.
    marks_total: u32,
    /// The noise floor applied, original pixels (`0` drops nothing).
    noise_floor_px: u32,
    /// Whether either cap left rows out of the two lists.
    truncated: bool,
    /// The region polygons in plan-room wire shape, original pixels.
    plan_rooms: Vec<PlanRoom>,
    /// The published debug overlay, labelled with exactly the reported ids.
    debug: AssetRef,
    /// Measurement caveats from the analysis, the vectoriser and the
    /// thinning.
    warnings: Vec<String>,
    /// What to look at and do next.
    next_steps: Vec<&'static str>,
}

/// The `id` for an analysis label: `R`/`G`/`M` plus the number.
pub(super) fn tag(prefix: char, id: u32) -> String {
    format!("{prefix}{id}")
}

/// One vectorised region as a report row, its `marks` and `gaps` lists
/// narrowed to the ids `shown_marks` and `shown_gaps` report.
pub(super) fn region_row(
    frame: &Frame,
    shape: &RegionShape,
    shown_gaps: &BTreeSet<u32>,
    shown_marks: &BTreeSet<u32>,
) -> Result<RegionRow, CyberError> {
    let id = tag('R', shape.id);
    let points = frame.ring(&shape.points, &format!("region {id}"))?;
    let map_points = frame.map_points(&points)?;
    let centroid =
        frame.pixel_point(shape.centroid.0, shape.centroid.1, &format!("region {id}"))?;
    Ok(RegionRow {
        bbox: frame.bbox(shape.bbox, &format!("region {id}"))?,
        area_px: frame.area(shape.area_px),
        centroid,
        gaps: tagged(&shape.gaps, shown_gaps, 'G'),
        holes: shape.holes,
        id,
        is_rectangle: shape.is_rectangle,
        map_points,
        marks: tagged(&shape.marks, shown_marks, 'M'),
        points,
    })
}

/// The `what` ids that reached the report, tagged with `prefix`, in order.
fn tagged(what: &[u32], shown: &BTreeSet<u32>, prefix: char) -> Vec<String> {
    what.iter()
        .filter(|id| shown.contains(id))
        .map(|id| tag(prefix, *id))
        .collect()
}

/// One bridged wall break as a report row.
pub(super) fn gap_row(frame: &Frame, gap: &Gap) -> Result<GapRow, CyberError> {
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
pub(super) fn mark_row(frame: &Frame, mark: &Mark) -> Result<MarkRow, CyberError> {
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

/// The pieces the client-facing report is assembled from.
pub(super) struct Parts<'measure> {
    /// The mapping from analysis pixels back onto the source picture.
    pub(super) frame: &'measure Frame,
    /// The raw measurement: threshold, stroke, closing radius, warnings.
    pub(super) analysis: &'measure Analysis,
    /// The region polygons, `None` when nothing enclosed.
    pub(super) vectorized: Option<&'measure Vectorized>,
    /// The two thinned lists with what stayed out of them.
    pub(super) thinned: Thinned,
    /// The published debug overlay.
    pub(super) asset: AssetRef,
}

/// Assembles the client-facing report out of `parts`.
///
/// # Errors
///
/// [`region_rows`]'s mapping refusals for a region vertex, box or centre
/// no crop and scale can carry.
pub(super) fn assemble(parts: Parts<'_>) -> Result<TraceReport, CyberError> {
    let Parts {
        frame,
        analysis,
        vectorized,
        thinned,
        asset,
    } = parts;
    let shown_gaps = gap_labels(&thinned.gaps.kept.items);
    let shown_marks = mark_labels(&thinned.marks.kept.items);
    let regions = region_rows(frame, vectorized, &shown_gaps, &shown_marks)?;
    let room_rows = plan_rooms(&regions);
    let warnings = all_warnings(analysis, vectorized, &thinned.warnings);
    let truncated = thinned.truncated;
    let floor = thinned.noise_floor_px;
    let gaps_total = thinned.gaps.kept.total;
    let marks_total = thinned.marks.kept.total;
    let gap_rows = thinned.gaps.rows;
    let mark_rows = thinned.marks.rows;
    Ok(TraceReport {
        image: image_info(frame),
        stroke_px: frame.length(analysis.stroke),
        threshold: analysis.threshold,
        close_gaps_px: frame.length(analysis.close_radius),
        plan_rooms: room_rows,
        gaps: gap_rows,
        gaps_total,
        marks: mark_rows,
        marks_total,
        noise_floor_px: floor,
        truncated,
        regions,
        adjacency: adjacency_rows(frame, vectorized),
        debug: asset,
        warnings,
        next_steps: next_steps(truncated),
    })
}

/// The region rows in scan order, their `marks` and `gaps` lists narrowed
/// to the ids the report shows.
///
/// # Errors
///
/// [`region_row`]'s mapping refusals.
fn region_rows(
    frame: &Frame,
    vectorized: Option<&Vectorized>,
    shown_gaps: &BTreeSet<u32>,
    shown_marks: &BTreeSet<u32>,
) -> Result<Vec<RegionRow>, CyberError> {
    let Some(shapes) = vectorized else {
        return Ok(Vec::new());
    };
    shapes
        .regions
        .iter()
        .map(|shape| region_row(frame, shape, shown_gaps, shown_marks))
        .collect()
}

/// The adjacency rows, empty without vectorised shapes.
fn adjacency_rows(frame: &Frame, vectorized: Option<&Vectorized>) -> Vec<AdjacencyRow> {
    vectorized.map_or_else(Vec::new, |shapes| {
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
}

/// The warnings in report order: the analysis's, the vectoriser's and the
/// thinning's.
fn all_warnings(
    analysis: &Analysis,
    vectorized: Option<&Vectorized>,
    thinned: &[String],
) -> Vec<String> {
    let mut warnings = analysis.warnings.clone();
    if let Some(shapes) = vectorized {
        warnings.extend(shapes.warnings.iter().cloned());
    }
    warnings.extend(thinned.iter().cloned());
    warnings
}

/// The report's `image` object.
const fn image_info(frame: &Frame) -> ImageInfo {
    ImageInfo {
        width: frame.source_width,
        height: frame.source_height,
        analysed_scale: ScaleInfo {
            num: frame.num,
            den: frame.den,
        },
    }
}

/// The `plan_rooms` rows: the region footprints in the plan wire shape.
fn plan_rooms(regions: &[RegionRow]) -> Vec<PlanRoom> {
    regions
        .iter()
        .map(|region| PlanRoom {
            id: region.id.clone(),
            points: region.points.clone(),
        })
        .collect()
}

/// The report's next steps: the standing four, plus the thinning's hint
/// when either cap left rows out.
pub(super) fn next_steps(truncated: bool) -> Vec<&'static str> {
    let mut steps = NEXT_STEPS.to_vec();
    if truncated {
        steps.push(THINNED_STEP);
    }
    steps
}
