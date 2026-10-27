//! The whole-sketch measurement pipeline: [`analyze`] composes the mask,
//! morphology and labelling primitives into one [`Analysis`].
//!
//! The pipeline is fixed and formulaic: ink detection, then small
//! components are split off as [`Mark`]s (dots, crosses, letters) so they
//! never masquerade as walls; the remaining wall ink is closed with a
//! square element, which bridges breaks up to twice the radius; enclosed
//! paper components become [`Component`] regions; ink the closing added on
//! top of the walls becomes the [`Gap`] list, each tagged with the two
//! regions it separates. Numbers only: deciding that a region is a room
//! and a gap is a door belongs to the calling model.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{MAX_OP_DIMENSION, rounded_div};
use crate::gfx::{BYTES_PER_PIXEL, pixel_count, to_pixel};
use crate::sketch::components::{Component, Connectivity, Labels, label_components};
use crate::sketch::mask::{InkMode, Mask, ink_mask, stroke_width};
use crate::sketch::morph::close;

/// A pixel coordinate or coordinate pair in mask space.
type Coord = (u32, u32);

/// Above this many enclosed regions a sketch is probably noisy.
const MANY_REGIONS: usize = 200;

/// Parameters of [`analyze`]; `None` fields take their documented
/// stroke- or size-derived defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalyzeOptions {
    /// Definition of ink.
    pub ink: InkMode,
    /// Closing radius; default `min(2 * stroke, 12)`.
    pub close_radius: Option<u32>,
    /// Minimum enclosed region area; default `max(width * height / 1000, 16)`.
    pub min_region_area: Option<u32>,
    /// Maximum mark bounding-box edge; default `3 * stroke + 8`.
    pub mark_max_size: Option<u32>,
}

/// A break in the walls that the closing bridged (typically a door opening).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    /// Label, `1..` in scan order of the gap pixels.
    pub id: u32,
    /// Rounded centroid `(x, y)`.
    pub centroid: Coord,
    /// Inclusive bounding box `(x0, y0, x1, y1)`.
    pub bbox: (u32, u32, u32, u32),
    /// The larger bounding-box edge, in pixels.
    pub width_px: u32,
    /// The two region ids the gap sits between, from the most frequent
    /// neighbourhood ids; `None` when fewer than two are found.
    pub between: Option<(u32, u32)>,
}

/// A small ink component: a dot, cross or letter, not wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Label, `1..` in scan order of the mark pixels.
    pub id: u32,
    /// Rounded centroid `(x, y)`.
    pub centroid: Coord,
    /// Inclusive bounding box `(x0, y0, x1, y1)`.
    pub bbox: (u32, u32, u32, u32),
    /// Pixels in the mark.
    pub area: u32,
    /// Mean `RGB` of the mark's pixels in the source image.
    pub color: [u8; 3],
    /// The region containing the centroid; `None` when the mark sits in
    /// the background.
    pub region: Option<u32>,
}

/// Everything [`analyze`] measures about one sketch, in pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// The ink mask as detected.
    pub ink: Mask,
    /// Threshold level applied (`InkMode::Dark` only).
    pub threshold: Option<u8>,
    /// Median pen width, at least `1`.
    pub stroke: u32,
    /// Closing radius actually used.
    pub close_radius: u32,
    /// Ink minus the mark pixels.
    pub walls: Mask,
    /// `walls` closed by `close_radius`.
    pub closed: Mask,
    /// Enclosed region labels `1..` in scan order; `0` is everything else.
    pub regions: Labels,
    /// Measurements of the `regions` ids.
    pub region_info: Vec<Component>,
    /// Breaks the closing bridged in the walls.
    pub gaps: Vec<Gap>,
    /// Small ink components filtered out of the walls.
    pub marks: Vec<Mark>,
    /// Measurement caveats for the calling model.
    pub warnings: Vec<String>,
}

/// Measures one sketch: ink, stroke, marks, closed walls, enclosed
/// regions and wall gaps. See the module docs for the pipeline.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when an edge exceeds
/// [`MAX_OP_DIMENSION`]; the caller downsizes first.
#[inline]
pub fn analyze(img: &RgbaImage, options: &AnalyzeOptions) -> Result<Analysis, CyberError> {
    check_sketch_size(img)?;
    let (ink, threshold) = ink_mask(img, options.ink);
    let stroke = stroke_width(&ink);
    let params = Params::resolve(img.width, img.height, stroke, options);
    let (walls, mut marks) = split_marks(img, &ink, params.mark_max_size);
    let closed = close(&walls, params.close_radius);
    let (regions, region_info, dropped) = find_regions(&closed, params.min_region_area);
    assign_mark_regions(&mut marks, &regions);
    let bridge = closed.without(&walls);
    let radius = params.close_radius.saturating_add(stroke).saturating_add(2);
    let gaps = find_gaps(&bridge, &regions, params.min_gap_area, radius);
    let warnings = build_warnings(dropped, region_info.len());
    Ok(Analysis {
        ink,
        threshold,
        stroke,
        close_radius: params.close_radius,
        walls,
        closed,
        regions,
        region_info,
        gaps,
        marks,
        warnings,
    })
}

/// Rejects sketches larger than [`MAX_OP_DIMENSION`] on either edge.
fn check_sketch_size(img: &RgbaImage) -> Result<(), CyberError> {
    let (name, value) = if img.width > MAX_OP_DIMENSION {
        ("width", img.width)
    } else if img.height > MAX_OP_DIMENSION {
        ("height", img.height)
    } else {
        return Ok(());
    };
    Err(CyberError::InvalidArgument(format!(
        "sketch {name} {value} exceeds {MAX_OP_DIMENSION}; downsize the image first, e.g. with the `fit` op"
    )))
}

/// The resolved numeric parameters of one analysis run.
#[derive(Clone, Copy, Debug)]
struct Params {
    /// Closing radius used.
    close_radius: u32,
    /// Minimum enclosed region area.
    min_region_area: u32,
    /// Maximum mark bounding-box edge.
    mark_max_size: u32,
    /// Minimum bridge blob counted as a wall gap: one stroke's worth of
    /// pixels, so the closing's own noise (thinner specks) never reads
    /// as an opening.
    min_gap_area: u32,
}

impl Params {
    /// Explicit option values win; `None` fields take the defaults
    /// documented on [`AnalyzeOptions`].
    fn resolve(width: u32, height: u32, stroke: u32, options: &AnalyzeOptions) -> Self {
        let default_min_area = pixel_count(width, height)
            .and_then(|count| count.checked_div(1_000_usize))
            .and_then(|area| u32::try_from(area).ok())
            .unwrap_or(16_u32)
            .max(16_u32);
        Self {
            close_radius: options
                .close_radius
                .unwrap_or_else(|| stroke.saturating_mul(2).min(12)),
            min_region_area: options.min_region_area.unwrap_or(default_min_area),
            mark_max_size: options
                .mark_max_size
                .unwrap_or_else(|| stroke.saturating_mul(3).saturating_add(8)),
            min_gap_area: stroke,
        }
    }
}

/// Splits small ink components off as marks: returns the wall mask
/// (ink without them) and the marks in scan order, regions unfilled.
fn split_marks(img: &RgbaImage, ink: &Mask, max_side: u32) -> (Mask, Vec<Mark>) {
    let (labels, components) = label_components(ink, true, Connectivity::Eight);
    let chosen: Vec<Component> = components
        .into_iter()
        .filter(|component| is_mark(component, max_side))
        .collect();
    let ids: BTreeSet<u32> = chosen.iter().map(|component| component.id).collect();
    let bits = ink
        .bits
        .iter()
        .zip(labels.ids.iter())
        .map(|(&bit, &id)| bit && !ids.contains(&id))
        .collect();
    let walls = Mask {
        width: ink.width,
        height: ink.height,
        bits,
    };
    let colors = mark_colors(img, &labels, &ids);
    let marks = chosen
        .iter()
        .enumerate()
        .map(|(position, component)| Mark {
            id: u32::try_from(position.saturating_add(1)).unwrap_or(u32::MAX),
            centroid: centroid(component),
            bbox: component.bbox,
            area: component.area,
            color: colors
                .get(&component.id)
                .copied()
                .unwrap_or([0_u8, 0_u8, 0_u8]),
            region: None,
        })
        .collect();
    (walls, marks)
}

/// A mark is a small ink component: at least two pixels, and its
/// bounding box no wider than `max_side` on the longest edge.
fn is_mark(component: &Component, max_side: u32) -> bool {
    component.area >= 2 && max_bbox_side(component.bbox) <= max_side
}

/// The larger edge of an inclusive bounding box.
fn max_bbox_side(bbox: (u32, u32, u32, u32)) -> u32 {
    let (x0, y0, x1, y1) = bbox;
    let width = x1.saturating_sub(x0).saturating_add(1);
    let height = y1.saturating_sub(y0).saturating_add(1);
    width.max(height)
}

/// The mean `RGB` of every marked component's pixels in the source image.
fn mark_colors(img: &RgbaImage, labels: &Labels, ids: &BTreeSet<u32>) -> BTreeMap<u32, [u8; 3]> {
    let mut sums: BTreeMap<u32, [u64; 4]> = BTreeMap::new();
    for (chunk, id) in img
        .pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .zip(labels.ids.iter())
    {
        if !ids.contains(id) {
            continue;
        }
        let [red, green, blue, _] = to_pixel(chunk);
        let adds = [u64::from(red), u64::from(green), u64::from(blue), 1_u64];
        for (slot, add) in sums.entry(*id).or_default().iter_mut().zip(adds) {
            *slot = slot.saturating_add(add);
        }
    }
    sums.into_iter()
        .map(|(id, [sum_red, sum_green, sum_blue, count])| {
            let mean = |sum: u64| u8::try_from(rounded_div(sum, count)).unwrap_or(0_u8);
            (id, [mean(sum_red), mean(sum_green), mean(sum_blue)])
        })
        .collect()
}

/// The rounded centroid of a component.
fn centroid(component: &Component) -> Coord {
    let area = u64::from(component.area);
    (
        to_u32(rounded_div(component.sum_x, area)),
        to_u32(rounded_div(component.sum_y, area)),
    )
}

/// A `u64` measurement clamped into `u32`.
fn to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The enclosed paper components of `closed`: not touching the image
/// border and at least `min_area` pixels. Returns the relabelled
/// `1..` raster, the relabelled measurements and the number of enclosed
/// components dropped for being too small.
fn find_regions(closed: &Mask, min_area: u32) -> (Labels, Vec<Component>, u32) {
    let (labels, components) = label_components(closed, false, Connectivity::Four);
    let mut file = RegionFile::default();
    for component in components {
        file.keep(component, min_area);
    }
    let ids = labels
        .ids
        .iter()
        .map(|old| file.mapping.get(old).copied().unwrap_or(0))
        .collect();
    let regions = Labels {
        width: closed.width,
        height: closed.height,
        ids,
    };
    (regions, file.kept, file.dropped)
}

/// Accumulator of the enclosed components that pass the size test.
#[derive(Default)]
struct RegionFile {
    /// Old component id to new region id.
    mapping: BTreeMap<u32, u32>,
    /// Relabelled measurements, in scan order.
    kept: Vec<Component>,
    /// Enclosed components dropped for being too small.
    dropped: u32,
}

impl RegionFile {
    /// Files `component` under a fresh id when it is enclosed and large
    /// enough; counts it as dropped when it is enclosed but small.
    fn keep(&mut self, component: Component, min_area: u32) {
        if component.touches_border {
            return;
        }
        if component.area < min_area {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let new_id = u32::try_from(self.kept.len().saturating_add(1)).unwrap_or(u32::MAX);
        self.mapping.insert(component.id, new_id);
        self.kept.push(Component {
            id: new_id,
            ..component
        });
    }
}

/// The gap components of the bridge mask with at least `min_gap_area`
/// pixels (see [`Params::min_gap_area`], the stroke-wide noise floor),
/// each tagged with the two region ids its `radius`-pixel neighbourhood
/// most often sees.
fn find_gaps(bridge: &Mask, regions: &Labels, min_gap_area: u32, radius: u32) -> Vec<Gap> {
    let (labels, components) = label_components(bridge, true, Connectivity::Eight);
    let pixels = collect_pixels(&labels);
    let mut gaps: Vec<Gap> = Vec::new();
    for component in components
        .into_iter()
        .filter(|candidate| candidate.area >= min_gap_area)
    {
        let counts = neighbour_region_counts(regions, &pixels, component.id, radius);
        gaps.push(Gap {
            id: u32::try_from(gaps.len().saturating_add(1)).unwrap_or(u32::MAX),
            centroid: centroid(&component),
            bbox: component.bbox,
            width_px: max_bbox_side(component.bbox),
            between: top_two(&counts),
        });
    }
    gaps
}

/// The pixels of every labelled component, keyed by id.
fn collect_pixels(labels: &Labels) -> BTreeMap<u32, Vec<Coord>> {
    let mut pixels: BTreeMap<u32, Vec<Coord>> = BTreeMap::new();
    let Ok(width) = usize::try_from(labels.width) else {
        return pixels;
    };
    for (position, id) in labels.ids.iter().enumerate() {
        record_pixel(&mut pixels, width, position, *id);
    }
    pixels
}

/// Files one raster position under its component id; `0` stays out.
fn record_pixel(pixels: &mut BTreeMap<u32, Vec<Coord>>, width: usize, position: usize, id: u32) {
    if id == 0 || width == 0 {
        return;
    }
    let (Some(x), Some(y)) = (
        position
            .checked_rem(width)
            .and_then(|v| u32::try_from(v).ok()),
        position
            .checked_div(width)
            .and_then(|v| u32::try_from(v).ok()),
    ) else {
        return;
    };
    pixels.entry(id).or_default().push((x, y));
}

/// Region-id frequencies in the `radius`-pixel neighbourhood of one gap.
fn neighbour_region_counts(
    regions: &Labels,
    pixels: &BTreeMap<u32, Vec<Coord>>,
    id: u32,
    radius: u32,
) -> BTreeMap<u32, u32> {
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    let Some(points) = pixels.get(&id) else {
        return counts;
    };
    let half = i64::from(radius);
    let low = half.checked_neg().unwrap_or(i64::MIN);
    for dy in low..=half {
        count_band(regions, points, Band { dy, half }, &mut counts);
    }
    counts
}

/// One window row of [`neighbour_region_counts`]: `dy` below the gap
/// pixels and `half` pixels wide to each side.
#[derive(Clone, Copy, Debug)]
struct Band {
    /// Row offset from each gap pixel.
    dy: i64,
    /// Half-width of the window.
    half: i64,
}

/// Adds the region ids found on one window row around the gap pixels.
fn count_band(regions: &Labels, points: &[Coord], band: Band, counts: &mut BTreeMap<u32, u32>) {
    for point in points.iter().copied() {
        count_window_row(regions, point, band, counts);
    }
}

/// Bumps every region id visible within the band around one gap pixel.
fn count_window_row(regions: &Labels, (x, y): Coord, band: Band, counts: &mut BTreeMap<u32, u32>) {
    let Some(ny) = shift(y, band.dy) else {
        return;
    };
    let low = band.half.checked_neg().unwrap_or(i64::MIN);
    for dx in low..=band.half {
        if let Some(nx) = shift(x, dx) {
            bump(regions, nx, ny, counts);
        }
    }
}

/// The coordinate `delta` pixels away, `None` when it leaves `u32`.
fn shift(coordinate: u32, delta: i64) -> Option<u32> {
    i64::from(coordinate)
        .checked_add(delta)
        .and_then(|value| u32::try_from(value).ok())
}

/// Counts the region id one neighbourhood pixel carries, if any.
fn bump(regions: &Labels, x: u32, y: u32, counts: &mut BTreeMap<u32, u32>) {
    if let Some(id) = regions.get(x, y).filter(|region| *region > 0) {
        let entry = counts.entry(id).or_insert(0_u32);
        *entry = entry.saturating_add(1);
    }
}

/// The two most frequent region ids, ties won by the lower id; `None`
/// when the counts hold fewer than two ids.
fn top_two(counts: &BTreeMap<u32, u32>) -> Option<(u32, u32)> {
    let mut ranked: Vec<(u32, u32)> = counts.iter().map(|(id, count)| (*count, *id)).collect();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let first = *ranked.first()?;
    let second = *ranked.get(1)?;
    Some((first.1, second.1))
}

/// Fills each mark's `region` with the region id under its centroid; the
/// mark pixels were paper when the walls were closed, so the id, if any,
/// is the region the mark sits in.
fn assign_mark_regions(marks: &mut [Mark], regions: &Labels) {
    for mark in marks {
        mark.region = regions
            .get(mark.centroid.0, mark.centroid.1)
            .filter(|id| *id > 0);
    }
}

/// The analysis warnings.
fn build_warnings(dropped: u32, region_count: usize) -> Vec<String> {
    let mut warnings = Vec::new();
    if region_count == 0 {
        warnings
            .push("no enclosed areas; lower min_region_area or increase close_radius".to_owned());
    }
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} enclosed areas below min_region_area were dropped"
        ));
    }
    if region_count > MANY_REGIONS {
        warnings
            .push("very many regions; the sketch may be noisy - raise min_region_area".to_owned());
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::{AnalyzeOptions, InkMode, analyze};
    use crate::gfx::image::RgbaImage;

    /// Dark ink at a fixed level; defaults everywhere else.
    fn options() -> AnalyzeOptions {
        AnalyzeOptions {
            ink: InkMode::Dark { level: Some(128) },
            close_radius: None,
            min_region_area: None,
            mark_max_size: None,
        }
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

    /// Paints both diagonals of the `X` centred on `centre` out to the
    /// given arm length, in `colour`.
    fn draw_x(img: &mut RgbaImage, centre: (u32, u32), arm: u32, colour: [u8; 4]) {
        let (cx, cy) = centre;
        let points = (0..=arm).flat_map(move |d| {
            [
                (cx.saturating_add(d), cy.saturating_add(d)),
                (cx.saturating_add(d), cy.saturating_sub(d)),
                (cx.saturating_sub(d), cy.saturating_add(d)),
                (cx.saturating_sub(d), cy.saturating_sub(d)),
            ]
        });
        for (x, y) in points {
            assert!(img.set(x, y, colour), "fixture in bounds");
        }
    }

    /// Two rooms on a 200x120 sheet: 3-px walls, outer box
    /// `(30,30)..(99,89)`, shared wall at `x=63..65`. When
    /// `with_break`, the shared wall has a 10-px door gap at rows 55..64.
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

    #[test]
    fn two_rooms_measure_two_regions_and_no_gaps() {
        let result = analyze(&rooms(false), &options()).expect("fixture is analysable");
        assert_eq!(
            result.threshold,
            Some(128),
            "the explicit level is reported"
        );
        assert_eq!(result.stroke, 3, "the pen measures 3 pixels");
        assert_eq!(
            result.close_radius, 6,
            "default closing radius is 2 * stroke"
        );
        assert_eq!(result.region_info.len(), 2, "two enclosed regions");
        assert_eq!(result.region_info[0].id, 1, "scan order ids");
        assert_eq!(result.region_info[1].id, 2, "second region");
        assert_eq!(
            result.region_info[0].bbox,
            (33, 33, 62, 86),
            "left room box"
        );
        assert_eq!(
            result.region_info[1].bbox,
            (66, 33, 96, 86),
            "right room box"
        );
        assert_eq!(result.region_info[0].area, 1620, "left room interior");
        assert_eq!(result.region_info[1].area, 1674, "right room interior");
        assert_eq!(
            result.regions.get(48, 60),
            Some(1),
            "left room centre carries region 1"
        );
        assert_eq!(
            result.regions.get(80, 60),
            Some(2),
            "right room centre carries region 2"
        );
        assert!(result.gaps.is_empty(), "solid walls bridge nothing");
        assert!(result.marks.is_empty(), "nothing here is a mark");
        assert!(result.warnings.is_empty(), "no caveats for a clean sketch");
        assert_eq!(
            result.walls.bits, result.ink.bits,
            "no marks: walls are the ink"
        );
    }

    #[test]
    fn a_door_break_is_bridged_and_reported_as_one_gap() {
        let result = analyze(&rooms(true), &options()).expect("fixture is analysable");
        assert_eq!(result.stroke, 3, "the pen still measures 3");
        assert_eq!(result.region_info.len(), 2, "the closing seals the door");
        assert_eq!(result.region_info[0].area, 1620, "left room interior");
        assert_eq!(result.region_info[1].area, 1674, "right room interior");
        assert_eq!(result.gaps.len(), 1, "exactly the door was bridged");
        let gap = &result.gaps[0];
        assert_eq!(gap.between, Some((1, 2)), "the door sits between the rooms");
        assert!(
            (8..=12).contains(&gap.width_px),
            "bridged door width is about 10, got {}",
            gap.width_px
        );
        assert_eq!(gap.bbox, (63, 55, 65, 64), "the bridge covers the door");
        assert!(result.marks.is_empty(), "a break is not a mark");
        assert!(result.warnings.is_empty(), "no caveats");
    }

    #[test]
    fn an_x_inside_a_room_is_a_mark_not_a_wall() {
        let mut img = rooms(false);
        draw_x(&mut img, (48, 60), 3, [0, 0, 0, 255]);
        let result = analyze(&img, &options()).expect("fixture is analysable");
        assert_eq!(result.marks.len(), 1, "the X is the only mark");
        let mark = &result.marks[0];
        assert_eq!(mark.bbox, (45, 57, 51, 63), "a 7x7 X");
        assert_eq!(mark.area, 13, "13 pixels in the X");
        assert_eq!(mark.centroid, (48, 60), "centroid at the crossing");
        assert_eq!(mark.color, [0, 0, 0], "the pen is black");
        assert_eq!(mark.region, Some(1), "the X sits in region 1");
        assert_eq!(result.region_info.len(), 2, "the X creates no region");
        assert_eq!(
            result.region_info[0].area, 1620,
            "and no hole: room areas stand"
        );
        assert!(result.gaps.is_empty(), "removing the X bridges nothing");
        let ink_count = result.ink.bits.iter().filter(|&&bit| bit).count();
        let wall_count = result.walls.bits.iter().filter(|&&bit| bit).count();
        assert_eq!(wall_count, ink_count - 13, "walls lost exactly the mark");
    }

    #[test]
    fn colour_ink_follows_one_pen() {
        let mut img = paper(100, 60);
        draw_x(&mut img, (20, 30), 2, [255, 0, 0, 255]);
        let mut with_black = img.clone();
        paint(&mut with_black, (60, 10, 62, 49));
        let red = InkMode::Color {
            rgb: [255, 0, 0],
            tolerance: 64,
        };
        let colour_options = AnalyzeOptions {
            ink: red,
            ..options()
        };
        let red_only = analyze(&with_black, &colour_options).expect("analysable");
        assert_eq!(
            red_only.ink.bits.iter().filter(|&&bit| bit).count(),
            9,
            "only the red X pixels are ink"
        );
        assert_eq!(red_only.threshold, None, "colour mode has no level");
        assert_eq!(red_only.marks.len(), 1, "the red X is the only mark");
        assert_eq!(
            red_only.marks[0].color,
            [255, 0, 0],
            "mean colour is the pen"
        );
        assert_eq!(red_only.marks[0].region, None, "it sits in the background");
        assert_eq!(
            red_only.warnings,
            vec!["no enclosed areas; lower min_region_area or increase close_radius".to_owned()],
            "colour ink walls nothing"
        );
        let both = analyze(&with_black, &options()).expect("analysable");
        assert_eq!(
            both.ink.bits.iter().filter(|&&bit| bit).count(),
            9 + 3 * 40,
            "dark ink takes the red X and the black bar"
        );
        assert_eq!(both.marks.len(), 1, "only the X is small enough to mark");
        assert_eq!(both.marks[0].color, [255, 0, 0], "the mark reads as red");
    }

    #[test]
    fn a_small_enclosed_loop_can_be_filtered_with_a_warning() {
        let mut img = rooms(false);
        paint(&mut img, (150, 10, 169, 12));
        paint(&mut img, (150, 27, 169, 29));
        paint(&mut img, (150, 10, 152, 29));
        paint(&mut img, (167, 10, 169, 29));
        let strict = AnalyzeOptions {
            min_region_area: Some(200),
            ..options()
        };
        let result = analyze(&img, &strict).expect("fixture is analysable");
        assert_eq!(
            result.region_info.len(),
            2,
            "only the two rooms pass 200 px"
        );
        assert_eq!(result.warnings.len(), 1, "one caveat");
        assert!(
            result.warnings[0].starts_with("1 enclosed areas below min_region_area"),
            "the drop is reported with its count, got {:?}",
            result.warnings
        );
        assert!(result.gaps.is_empty(), "the loop is intact wall, not a gap");
        assert!(
            result.marks.is_empty(),
            "a 20-px loop is larger than the mark size"
        );
    }

    #[test]
    fn a_blank_sheet_encloses_nothing_and_warns() {
        let result = analyze(&paper(100, 80), &options()).expect("analysable");
        assert!(result.ink.bits.iter().all(|bit| !*bit), "no ink");
        assert_eq!(result.stroke, 1, "an inkless pen measures 1");
        assert!(result.region_info.is_empty(), "nothing is enclosed");
        assert!(result.marks.is_empty(), "nothing to mark");
        assert!(result.gaps.is_empty(), "nothing to bridge");
        assert_eq!(
            result.warnings,
            vec!["no enclosed areas; lower min_region_area or increase close_radius".to_owned()],
            "the empty sheet explains itself"
        );
    }

    #[test]
    fn many_regions_are_reported_as_noise() {
        let mut img = paper(128, 128);
        for line in (0..=120).step_by(8) {
            paint(&mut img, (line, 0, line, 127));
            paint(&mut img, (0, line, 127, line));
        }
        let result = analyze(&img, &options()).expect("analysable");
        assert_eq!(result.region_info.len(), 225, "a 15x15 grid of cells");
        assert_eq!(
            result.warnings,
            vec!["very many regions; the sketch may be noisy - raise min_region_area".to_owned()],
            "the noisy sketch warns exactly once"
        );
    }

    #[test]
    fn an_oversized_sketch_is_rejected() {
        let error = analyze(&paper(5_000, 1), &options()).expect_err("too wide");
        assert_eq!(
            error.code(),
            "invalid_argument",
            "size errors are argument errors"
        );
    }
}
