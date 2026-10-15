//! Turning an [`Analysis`] into weldable vector shapes.
//!
//! [`vectorize`] composes the pure pieces of this module: regions grow
//! into the closed walls ([`expand_regions`]) by default
//! `(stroke + 1) / 2 + close_radius` rings, enough to reach the centreline
//! of a stroke; each grown region is traced ([`trace_outline`]) and
//! simplified to `simplify_px` vertices, straightened
//! ([`snap_right_angles`]) and the whole set is welded ([`weld`]) so
//! shared walls carry one identical coordinate. A shape that is still
//! self-intersecting after its simplification is retried at half the
//! tolerance, then at `1`, and only then falls back to its bounding-box
//! rectangle with a warning naming the region. [`RegionShape`] reports
//! what came out in measurable numbers; deciding what it *is* stays with
//! the calling model.

use crate::error::CyberError;
use crate::gfx::ops::rounded_div;
use crate::sketch::components::Labels;

use super::analyze::Analysis;
use super::expand::{Adjacent, adjacency, expand_regions};
use super::outline::{is_simple, trace_outline};
use super::simplify::{edges_axis_aligned, simplify, snap_right_angles};
use super::weld::weld;

/// A polygon vertex in pixel-corner coordinates.
type Point = (i64, i64);

/// Area, inclusive box and rounded centre of one label.
type Measure = (u32, (u32, u32, u32, u32), (u32, u32));

/// The tolerated skew for right-angle snapping, in thousandths.
const MAX_SKEW_PERMILLE: u32 = 170;

/// Knobs of [`vectorize`]; `None` numeric fields take the
/// stroke-derived defaults documented on each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorizeOptions {
    /// Grow regions to the stroke centrelines first; default `true`.
    pub expand: bool,
    /// Simplification tolerance; default `max(2, stroke)`.
    pub simplify_px: Option<u32>,
    /// Straighten near-axis edges; default `true`.
    pub orthogonal: bool,
    /// Welding tolerance; default `max(2, stroke)`, `0` disables.
    pub weld_px: Option<u32>,
}

impl Default for VectorizeOptions {
    #[inline]
    fn default() -> Self {
        Self {
            expand: true,
            simplify_px: None,
            orthogonal: true,
            weld_px: None,
        }
    }
}

/// One vectorised region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionShape {
    /// The region label it came from.
    pub id: u32,
    /// Welded, simplified, snapped ring in pixel-corner coordinates.
    pub points: Vec<Point>,
    /// Pixels the grown label occupies.
    pub area_px: u32,
    /// Inclusive pixel bounding box `(x0, y0, x1, y1)` of the label.
    pub bbox: (u32, u32, u32, u32),
    /// Rounded pixel centroid `(x, y)` of the label.
    pub centroid: (u32, u32),
    /// Exactly four vertices, all edges axis-aligned.
    pub is_rectangle: bool,
    /// Non-region islands inside the traced piece's bounding box.
    pub holes: u32,
    /// Ids of the [`Mark`](super::Mark)s sitting in this region.
    pub marks: Vec<u32>,
    /// Ids of the [`Gap`](super::Gap)s bordering this region.
    pub gaps: Vec<u32>,
}

/// The whole vectorised sketch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vectorized {
    /// One shape per region that produced an outline.
    pub regions: Vec<RegionShape>,
    /// Region contacts measured on the grown labels, sorted by `(a, b)`.
    pub adjacency: Vec<Adjacent>,
    /// Measurement caveats for the calling model.
    pub warnings: Vec<String>,
}

/// A region mid-pipeline: identity plus the first-attempt ring.
#[derive(Clone, Debug)]
struct Draft {
    /// Region label.
    id: u32,
    /// Inclusive pixel bounding box from the analysis.
    bbox: (u32, u32, u32, u32),
    /// Hole count reported by the trace.
    holes: u32,
    /// Ring after trace, simplification and (optional) snapping.
    points: Vec<Point>,
}

/// Resolved settings of one run.
#[derive(Clone, Debug)]
struct Plan {
    /// Labels the shapes are traced from (grown, or the analysis's own).
    labels: Labels,
    /// First-attempt simplification tolerance.
    tolerance: u32,
    /// Welding tolerance; `0` disables.
    weld_px: u32,
    /// Whether rings are straightened.
    orthogonal: bool,
}

impl Plan {
    /// The pipeline ring for traced points `raw` at `tolerance`.
    fn shaped_ring(&self, raw: &[Point], tolerance: u32) -> Vec<Point> {
        let loose = simplify(raw, tolerance);
        if self.orthogonal {
            return snap_right_angles(&loose, MAX_SKEW_PERMILLE);
        }
        loose
    }

    /// Trace, simplify and (when asked) straighten region `id`.
    fn shaped(&self, id: u32, tolerance: u32) -> Vec<Point> {
        let raw = trace_outline(&self.labels, id).map_or_else(Vec::new, |outline| outline.points);
        self.shaped_ring(&raw, tolerance)
    }
}

/// The rings under repair plus the warnings collected so far.
struct Run<'plan> {
    /// The resolved settings.
    plan: &'plan Plan,
    /// One ring per draft, repaired in place.
    rings: Vec<Vec<Point>>,
    /// Measurement caveats for the calling model.
    warnings: Vec<String>,
}

impl Run<'_> {
    /// An empty run for `plan`.
    const fn new(plan: &Plan) -> Run<'_> {
        Run {
            plan,
            rings: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Whether ring `index` exists and does not cross itself.
    fn simple_at(&self, index: usize) -> bool {
        self.rings.get(index).is_some_and(|ring| is_simple(ring))
    }

    /// Swap ring `index` for `points`.
    fn replace(&mut self, index: usize, points: Vec<Point>) {
        if let Some(slot) = self.rings.get_mut(index) {
            *slot = points;
        }
    }

    /// Weld every ring at once, unless the plan disabled welding.
    fn weld_all(&mut self) {
        if self.plan.weld_px != 0 {
            weld(&mut self.rings, self.plan.weld_px);
        }
    }
}

/// Vectorise one [`Analysis`]: grown regions, outlines, simplification,
/// right-angle snapping, welding and simplicity repair, in that order.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the analysis found no regions;
/// lowering `min_region_area` or raising `close_radius` in the analysis
/// is the way out.
#[inline]
pub fn vectorize(
    analysis: &Analysis,
    options: &VectorizeOptions,
) -> Result<Vectorized, CyberError> {
    if analysis.region_info.is_empty() {
        return Err(CyberError::InvalidArgument(
            "analysis found no regions; lower min_region_area or raise close_radius before vectorizing".to_owned(),
        ));
    }
    let plan = make_plan(analysis, options);
    let mut run = Run::new(&plan);
    let drafts = build_drafts(&mut run, analysis);
    run.rings = drafts.iter().map(|draft| draft.points.clone()).collect();
    run.weld_all();
    let mut regions: Vec<RegionShape> = Vec::new();
    for (index, draft) in drafts.iter().enumerate() {
        ensure_simple(&mut run, index, draft);
        regions.push(shape_from(&run, analysis, draft, index));
    }
    Ok(Vectorized {
        regions,
        adjacency: adjacency(&plan.labels),
        warnings: run.warnings,
    })
}

/// The resolved [`Plan`] of one run.
fn make_plan(analysis: &Analysis, options: &VectorizeOptions) -> Plan {
    let fallback = 2_u32.max(analysis.stroke);
    let labels = if options.expand {
        expand_regions(
            &analysis.regions,
            &analysis.closed,
            expand_steps(analysis.stroke, analysis.close_radius),
        )
    } else {
        analysis.regions.clone()
    };
    Plan {
        labels,
        tolerance: options.simplify_px.unwrap_or(fallback),
        weld_px: options.weld_px.unwrap_or(fallback),
        orthogonal: options.orthogonal,
    }
}

/// The default growth rings: to the centreline of a stroke plus the
/// closing radius that sealed the walls.
fn expand_steps(stroke: u32, close_radius: u32) -> u32 {
    let half = stroke
        .checked_add(1)
        .and_then(|sum| sum.checked_div(2))
        .unwrap_or(0);
    half.saturating_add(close_radius)
}

/// One [`Draft`] per analysis region that still has pixels.
fn build_drafts(run: &mut Run<'_>, analysis: &Analysis) -> Vec<Draft> {
    let mut drafts = Vec::new();
    for component in &analysis.region_info {
        let Some(outline) = trace_outline(&run.plan.labels, component.id) else {
            push_skipped(run, component.id);
            continue;
        };
        if outline.pieces > 1 {
            push_split(run, component.id, outline.pieces);
        }
        drafts.push(Draft {
            points: run.plan.shaped_ring(&outline.points, run.plan.tolerance),
            id: component.id,
            bbox: component.bbox,
            holes: outline.holes,
        });
    }
    drafts
}

/// Warn that region `id` produced no outline.
fn push_skipped(run: &mut Run<'_>, id: u32) {
    run.warnings
        .push(format!("region {id} has no pixels left; skipped"));
}

/// Warn that region `id` had several pieces.
fn push_split(run: &mut Run<'_>, id: u32, pieces: u32) {
    run.warnings.push(format!(
        "region {id} has {pieces} disconnected pieces; traced the largest"
    ));
}

/// Repair ring `index` until it is simple: half tolerance, then `1`,
/// re-welding against the others each time, bounding box last.
fn ensure_simple(run: &mut Run<'_>, index: usize, draft: &Draft) {
    for retry in retries(run.plan.tolerance) {
        attempt(run, index, draft, retry);
    }
    if run.simple_at(index) {
        return;
    }
    fall_back(run, index, draft);
}

/// One simplicity repair attempt at tolerance `retry`.
fn attempt(run: &mut Run<'_>, index: usize, draft: &Draft, retry: u32) {
    if run.simple_at(index) {
        return;
    }
    let fresh = run.plan.shaped(draft.id, retry);
    run.replace(index, fresh);
    run.weld_all();
}

/// Give ring `index` its bounding box and say so.
fn fall_back(run: &mut Run<'_>, index: usize, draft: &Draft) {
    run.replace(index, bbox_rect(draft.bbox));
    run.warnings.push(format!(
        "region {} simplifies to a self-intersecting outline; fell back to its bounding box",
        draft.id
    ));
}

/// The retry tolerances after a failure: half, then one, each only when
/// strictly smaller than what was already tried.
fn retries(tolerance: u32) -> Vec<u32> {
    let half = tolerance.checked_div(2).unwrap_or(0);
    let mut out: Vec<u32> = Vec::new();
    for candidate in [half, 1] {
        if candidate < tolerance && !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// The pixel bounding box as a corner-space rectangle.
fn bbox_rect(bbox: (u32, u32, u32, u32)) -> Vec<Point> {
    let (x0, y0, x1, y1) = bbox;
    let right = i64::from(x1).saturating_add(1);
    let bottom = i64::from(y1).saturating_add(1);
    vec![
        (i64::from(x0), i64::from(y0)),
        (right, i64::from(y0)),
        (right, bottom),
        (i64::from(x0), bottom),
    ]
}

/// Assemble the reportable [`RegionShape`] for draft `index`.
fn shape_from(run: &Run<'_>, analysis: &Analysis, draft: &Draft, index: usize) -> RegionShape {
    let points = run.rings.get(index).cloned().unwrap_or_default();
    let (area_px, bbox, centroid) = measure(&run.plan.labels, draft.id);
    RegionShape {
        is_rectangle: points.len() == 4 && edges_axis_aligned(&points),
        id: draft.id,
        points,
        area_px,
        bbox,
        centroid,
        holes: draft.holes,
        marks: marks_of(analysis, draft.id),
        gaps: gaps_of(analysis, draft.id),
    }
}

/// The ids of the marks whose centroid fell in region `id`.
fn marks_of(analysis: &Analysis, id: u32) -> Vec<u32> {
    analysis
        .marks
        .iter()
        .filter(|mark| mark.region == Some(id))
        .map(|mark| mark.id)
        .collect()
}

/// The ids of the gaps whose two regions include `id`.
fn gaps_of(analysis: &Analysis, id: u32) -> Vec<u32> {
    analysis
        .gaps
        .iter()
        .filter(|gap| gap.between.is_some_and(|(one, two)| one == id || two == id))
        .map(|gap| gap.id)
        .collect()
}

/// Measures a label: pixel count, inclusive box, rounded centre.
fn measure(labels: &Labels, id: u32) -> Measure {
    let width = usize::try_from(labels.width).unwrap_or(0).max(1);
    let mut seen = labels
        .ids
        .iter()
        .enumerate()
        .filter(|&(_, &pixel)| pixel == id)
        .map(|(index, _)| {
            (
                index.checked_rem(width).unwrap_or(0),
                index.checked_div(width).unwrap_or(0),
            )
        });
    let Some((first_x, first_y)) = seen.next() else {
        return (0, (0, 0, 0, 0), (0, 0));
    };
    let mut box_ = (first_x, first_y, first_x, first_y);
    let mut sums = (first_x, first_y);
    let mut count: u64 = 1;
    for (x, y) in seen {
        count = count.saturating_add(1);
        box_ = (box_.0.min(x), box_.1.min(y), box_.2.max(x), box_.3.max(y));
        sums = (sums.0.saturating_add(x), sums.1.saturating_add(y));
    }
    let area = u32::try_from(count).unwrap_or(u32::MAX);
    let bbox = (
        u32::try_from(box_.0).unwrap_or(u32::MAX),
        u32::try_from(box_.1).unwrap_or(u32::MAX),
        u32::try_from(box_.2).unwrap_or(u32::MAX),
        u32::try_from(box_.3).unwrap_or(u32::MAX),
    );
    let sum_x = u64::try_from(sums.0).unwrap_or(u64::MAX);
    let sum_y = u64::try_from(sums.1).unwrap_or(u64::MAX);
    let centroid = (
        to_u32(rounded_div(sum_x, count)),
        to_u32(rounded_div(sum_y, count)),
    );
    (area, bbox, centroid)
}

/// Clamps a measured `u64` into `u32`.
fn to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        Draft, Plan, Run, VectorizeOptions, ensure_simple, expand_steps, retries, vectorize,
    };
    use crate::gfx::image::RgbaImage;
    use crate::sketch::analyze::{Analysis, AnalyzeOptions, analyze};
    use crate::sketch::components::Labels;
    use crate::sketch::expand::Adjacent;
    use crate::sketch::mask::InkMode;
    use crate::sketch::outline::is_simple;

    /// The CD-101 fixture: two 3-px-walled rooms on a 200x120 sheet,
    /// with an optional 10-px door break in the shared wall at x=63..65.
    fn rooms(with_break: bool) -> RgbaImage {
        let mut img = paper(200, 120);
        paint(&mut img, (30, 30, 99, 32));
        paint(&mut img, (30, 87, 99, 89));
        paint(&mut img, (30, 30, 32, 89));
        paint(&mut img, (97, 30, 99, 89));
        if with_break {
            paint(&mut img, (63, 30, 65, 54));
            paint(&mut img, (63, 65, 65, 89));
        } else {
            paint(&mut img, (63, 30, 65, 89));
        }
        img
    }

    /// A white `width` x `height` image.
    fn paper(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for _ in 0..width.saturating_mul(height) {
            pixels.extend_from_slice(&[255, 255, 255, 255]);
        }
        RgbaImage::new(width, height, pixels).expect("fixture")
    }

    /// Paints the inclusive rectangle black.
    fn paint(img: &mut RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32)) {
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            assert!(img.set(x, y, [0, 0, 0, 255]), "fixture in bounds");
        }
    }

    /// Analyses the fixture with the CD-101 defaults.
    fn analysed(with_break: bool) -> Analysis {
        let options = AnalyzeOptions {
            ink: InkMode::Dark { level: Some(128) },
            close_radius: None,
            min_region_area: None,
            mark_max_size: None,
        };
        analyze(&rooms(with_break), &options).expect("fixture is analysable")
    }

    #[test]
    fn two_rooms_vectorise_into_two_welded_rectangles() {
        let analysis = analysed(true);
        let result = vectorize(&analysis, &VectorizeOptions::default()).expect("two regions");
        assert!(
            result.warnings.is_empty(),
            "clean sketch, got {:?}",
            result.warnings
        );
        assert_eq!(result.regions.len(), 2, "both rooms became shapes");
        let (one, two) = (&result.regions[0], &result.regions[1]);
        assert!(one.is_rectangle && two.is_rectangle, "both are rectangles");
        assert!(
            is_simple(&one.points) && is_simple(&two.points),
            "both are simple"
        );
        let shared_right = one.points.iter().map(|p| p.0).max().unwrap_or(0);
        let shared_left = two.points.iter().map(|p| p.0).min().unwrap_or(0);
        assert_eq!(shared_right, shared_left, "the shared wall is one x");
        assert_eq!(shared_right, 65, "the stroke centrelines meet at 65");
        assert_eq!(
            result.adjacency,
            vec![Adjacent {
                a: 1,
                b: 2,
                shared_px: 60,
            }],
            "the rooms touch along the grown wall"
        );
        assert_eq!(one.gaps, vec![1], "the door is reported on room 1");
        assert_eq!(two.gaps, vec![1], "and on room 2");
        assert_eq!(one.area_px, 2_100, "grown room 1 pixel count");
        assert_eq!(one.bbox, (30, 30, 64, 89), "grown room 1 box");
        assert_eq!(two.bbox, (65, 30, 99, 89), "grown room 2 box");
    }

    #[test]
    fn turning_expansion_off_keeps_the_regions_inside_the_walls() {
        let analysis = analysed(false);
        let options = VectorizeOptions {
            expand: false,
            ..VectorizeOptions::default()
        };
        let result = vectorize(&analysis, &options).expect("two regions");
        assert!(result.adjacency.is_empty(), "un-grown regions do not touch");
        assert_eq!(result.regions[0].bbox, (33, 33, 62, 86), "raw region 1 box");
        assert_eq!(result.regions.len(), 2, "both regions still traced");
    }

    #[test]
    fn turning_orthogonal_and_weld_off_still_traces_boxes() {
        let analysis = analysed(false);
        let options = VectorizeOptions {
            orthogonal: false,
            weld_px: Some(0),
            ..VectorizeOptions::default()
        };
        let result = vectorize(&analysis, &options).expect("two regions");
        assert!(
            result.regions.iter().all(|r| r.is_rectangle),
            "crack rings are axis boxes"
        );
    }

    #[test]
    fn an_analysis_without_regions_is_an_error() {
        let blank = analysed_small();
        let error = vectorize(&blank, &VectorizeOptions::default()).expect_err("no regions");
        assert_eq!(error.code(), "invalid_argument", "the empty sketch errors");
    }

    /// A blank 20x20 sheet analysed: no ink, no regions.
    fn analysed_small() -> Analysis {
        analyze(
            &paper(20, 20),
            &AnalyzeOptions {
                ink: InkMode::Dark { level: Some(128) },
                close_radius: None,
                min_region_area: None,
                mark_max_size: None,
            },
        )
        .expect("analysable")
    }

    #[test]
    fn the_retry_ladder_halves_then_hardens() {
        assert_eq!(retries(8), vec![4, 1], "half then one");
        assert_eq!(retries(2), vec![1], "one only");
        assert_eq!(retries(1), vec![0], "zero only");
        assert!(retries(0).is_empty(), "no way down from zero");
        assert_eq!(expand_steps(3, 6), 8, "(3 + 1) / 2 + 6");
        assert_eq!(expand_steps(1, 0), 1, "one pixel reaches the centreline");
    }

    #[test]
    fn a_bowtie_ring_is_retried_from_the_trace() {
        let plan = Plan {
            labels: spiral_region(),
            tolerance: 20,
            weld_px: 0,
            orthogonal: false,
        };
        let mut run = Run::new(&plan);
        run.rings = vec![vec![(0, 0), (20, 20), (20, 0), (0, 20)]];
        let draft = bowtie_draft();
        ensure_simple(&mut run, 0, &draft);
        assert!(
            run.simple_at(0),
            "the tolerance ladder re-traced a simple ring"
        );
        assert!(
            run.warnings.is_empty(),
            "a repaired ring is silent, got {:?}",
            run.warnings
        );
    }

    #[test]
    fn a_ring_that_cannot_be_retried_falls_back_to_the_bbox() {
        let plan = Plan {
            labels: spiral_region(),
            tolerance: 0,
            weld_px: 0,
            orthogonal: false,
        };
        let mut run = Run::new(&plan);
        run.rings = vec![vec![(0, 0), (20, 20), (20, 0), (0, 20)]];
        let draft = bowtie_draft();
        ensure_simple(&mut run, 0, &draft);
        assert_eq!(
            run.rings[0],
            vec![(0, 0), (20, 0), (20, 20), (0, 20)],
            "the bbox rectangle stands in"
        );
        assert_eq!(run.warnings.len(), 1, "the fallback speaks up");
        assert!(
            run.warnings[0].contains("region 1"),
            "the warning names the region, got {:?}",
            run.warnings
        );
    }

    /// The draft the two repair tests share: label one, pixel box 0..19.
    fn bowtie_draft() -> Draft {
        Draft {
            id: 1,
            bbox: (0, 0, 19, 19),
            holes: 0,
            points: vec![(0, 0), (20, 20), (20, 0), (0, 20)],
        }
    }

    /// A 21x21 raster holding a square spiral stroke two pixels thick.
    fn spiral_region() -> Labels {
        let bars = [
            (0_u32, 0_u32, 20_u32, 1_u32),
            (19, 0, 20, 19),
            (2, 18, 20, 19),
            (2, 4, 3, 19),
            (2, 4, 16, 5),
            (14, 4, 16, 14),
        ];
        let mut ids = vec![0_u32; 21 * 21];
        for &bar in &bars {
            fill_spiral(&mut ids, bar);
        }
        Labels {
            width: 21,
            height: 21,
            ids,
        }
    }

    /// Stamps one inclusive bar of the spiral into the raster.
    fn fill_spiral(ids: &mut [u32], (x0, y0, x1, y1): (u32, u32, u32, u32)) {
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            stamp_spiral(ids, x, y);
        }
    }

    /// Sets the spiral pixel at `(x, y)` in a 21-column raster.
    fn stamp_spiral(ids: &mut [u32], x: u32, y: u32) {
        let index = usize::try_from(x.saturating_add(y.saturating_mul(21))).unwrap_or(0);
        if let Some(slot) = ids.get_mut(index) {
            *slot = 1;
        }
    }

    #[test]
    fn marks_are_listed_on_the_region_holding_them() {
        let mut img = rooms(false);
        for (x, y) in [(45_u32, 60_u32), (46, 60), (47, 60), (46, 59)] {
            assert!(img.set(x, y, [0, 0, 0, 255]), "fixture in bounds");
        }
        let options = AnalyzeOptions {
            ink: InkMode::Dark { level: Some(128) },
            close_radius: None,
            min_region_area: None,
            mark_max_size: None,
        };
        let analysis = analyze(&img, &options).expect("analysable");
        assert_eq!(analysis.marks.len(), 1, "the blob is a mark");
        let result = vectorize(&analysis, &VectorizeOptions::default()).expect("two regions");
        assert_eq!(result.regions[0].marks, vec![1], "it sits in room 1");
        assert!(result.regions[1].marks.is_empty(), "not in room 2");
    }
}
