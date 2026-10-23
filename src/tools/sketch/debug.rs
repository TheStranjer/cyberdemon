//! The `trace_sketch` debug overlay: the measured sketch redrawn with its
//! ids.
//!
//! The analysed picture (cropped, at analysis scale) is washed out to
//! forty percent, every [`crate::sketch::vectorize::RegionShape`] is
//! filled with a distinct colour from a fixed twelve-colour table at
//! forty-five percent alpha, its simplified ring drawn on top darkened,
//! the gaps as magenta discs (the radius is half the gap width), the
//! marks as cyan boxes, and the `R`, `G`, `M` ids with [`draw_text`] at
//! the centroids. Only the lists the caller passes are drawn, so the ids
//! on the picture are exactly the ids the report answers: a thinned trace
//! hands over its kept gaps and marks, never the whole analysis. The
//! calling model compares this picture against its own reading of the
//! sketch: it is measurement made visible, never an interpretation.

use crate::gfx::image::RgbaImage;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::render::canvas::{Canvas, Edge, Stroke};
use crate::render::font::{TextStyle, draw_text};
use crate::sketch::{Gap, Mark, Vectorized};

/// Pixels across one label glyph at overlay scale.
const LABEL_SCALE: u32 = 1_u32;
/// How many distinct region colours the table holds.
const PALETTE_LEN: u32 = 12_u32;
/// The twelve region fill colours, cycled by region id.
const REGION_COLORS: [[u8; 3]; 12] = [
    [228, 26, 28],
    [55, 126, 184],
    [77, 175, 74],
    [152, 78, 163],
    [255, 127, 0],
    [255, 255, 51],
    [166, 86, 40],
    [247, 129, 191],
    [147, 147, 147],
    [31, 120, 180],
    [170, 200, 60],
    [200, 60, 170],
];
/// The region fill's alpha: forty-five percent over the washed picture.
const REGION_ALPHA: u8 = 115_u8;
/// The gap disc colour: opaque magenta.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
/// The mark box colour: opaque cyan.
const CYAN: [u8; 4] = [0, 255, 255, 255];
/// The label fill: black glyphs.
const LABEL_FILL: [u8; 4] = [0, 0, 0, 255];
/// The label halo: white, so ids read on any fill colour.
const LABEL_HALO: [u8; 4] = [255, 255, 255, 255];
/// The region outline width in pixels.
const OUTLINE_PX: u32 = 3_u32;
/// Full channel value.
const CHANNEL_FULL: u32 = 255_u32;
/// The share of the ink the wash keeps, percent.
const WASH_KEEP: u32 = 40_u32;
/// The wash denominator, percent.
const WASH_TOTAL: u32 = 100_u32;

/// The label style: black glyphs on a white halo.
const LABEL: TextStyle = TextStyle {
    colour: LABEL_FILL,
    outline: LABEL_HALO,
    scale: LABEL_SCALE,
};

/// The whole overlay for one measured sketch, at analysis scale: the
/// regions as vectorised, and only the `gaps` and `marks` the report shows.
pub(super) fn build(
    picture: &RgbaImage,
    vectorized: Option<&Vectorized>,
    gaps: &[Gap],
    marks: &[Mark],
) -> RgbaImage {
    let mut canvas = washed_canvas(picture);
    if let Some(shapes) = vectorized {
        paint_regions(&mut canvas, shapes);
    }
    paint_gaps(&mut canvas, gaps);
    paint_marks(&mut canvas, marks);
    paint_labels(&mut canvas, vectorized, gaps, marks);
    canvas.to_rgba_image()
}

/// The picture washed out towards paper white, as the canvas base.
fn washed_canvas(picture: &RgbaImage) -> Canvas {
    let mut canvas = Canvas::new(picture.width, picture.height, [255_u8, 255, 255, 255]);
    let Some(width) = usize::try_from(picture.width).ok().filter(|w| *w > 0) else {
        return canvas;
    };
    for (index, chunk) in picture.pixels.chunks_exact(BYTES_PER_PIXEL).enumerate() {
        let [red, green, blue, _] = to_pixel(chunk);
        let Some(x) = index.checked_rem(width).and_then(|v| u32::try_from(v).ok()) else {
            continue;
        };
        let Some(y) = index.checked_div(width).and_then(|v| u32::try_from(v).ok()) else {
            continue;
        };
        let pixel = [wash(red), wash(green), wash(blue), 255_u8];
        canvas.set(i64::from(x), i64::from(y), pixel);
    }
    canvas
}

/// One channel mixed forty percent of its ink with sixty percent white.
fn wash(channel: u8) -> u8 {
    let paper = CHANNEL_FULL.saturating_mul(WASH_TOTAL.saturating_sub(WASH_KEEP));
    let mixed = u32::from(channel)
        .saturating_mul(WASH_KEEP)
        .saturating_add(paper)
        .checked_div(WASH_TOTAL)
        .unwrap_or(CHANNEL_FULL);
    u8::try_from(mixed).unwrap_or(u8::MAX)
}

/// The `id`-th region colour from the fixed table, cycling.
fn region_colour(id: u32) -> [u8; 3] {
    let index = id.saturating_sub(1).checked_rem(PALETTE_LEN).unwrap_or(0);
    let slot = usize::try_from(index).unwrap_or(0);
    REGION_COLORS.get(slot).copied().unwrap_or([0_u8, 0, 0])
}

/// One colour at half brightness, for outlines over their own fill.
fn darken(colour: [u8; 3]) -> [u8; 4] {
    let [red, green, blue] = colour;
    let half =
        |value: u8| u8::try_from(u32::from(value).checked_div(2_u32).unwrap_or(0)).unwrap_or(0_u8);
    [half(red), half(green), half(blue), 255_u8]
}

/// Every region polygon filled with its colour and ringed in its darkened
/// colour.
fn paint_regions(canvas: &mut Canvas, vectorized: &Vectorized) {
    for shape in &vectorized.regions {
        let fill = region_colour(shape.id);
        let [red, green, blue] = fill;
        let edges = ring_edges(&shape.points);
        canvas.blend_polygon_evenodd(&edges, [red, green, blue, REGION_ALPHA]);
        let stroke = Stroke::new(OUTLINE_PX, darken(fill));
        for (from, to) in edges {
            canvas.thick_line(from, to, stroke);
        }
    }
}

/// The closed edge list of a point ring: consecutive pairs plus the
/// closing edge (an empty ring or a lone point yields nothing).
fn ring_edges(points: &[(i64, i64)]) -> Vec<Edge> {
    let mut edges: Vec<Edge> = points
        .windows(2)
        .filter_map(|pair| Some((*pair.first()?, *pair.get(1)?)))
        .collect();
    if points.len() > 2
        && let (Some(first), Some(last)) = (points.first(), points.last())
    {
        edges.push((*last, *first));
    }
    edges
}

/// Every reported gap as a magenta disc of half its width at its centre.
fn paint_gaps(canvas: &mut Canvas, gaps: &[Gap]) {
    for gap in gaps {
        let (x, y) = i64_pair(gap.centroid);
        let radius = i64::from(gap.width_px).checked_div(2_i64).unwrap_or(1_i64);
        canvas.fill_circle((x, y), radius, MAGENTA);
    }
}

/// Every reported mark as a cyan box on its bounding rectangle.
fn paint_marks(canvas: &mut Canvas, marks: &[Mark]) {
    for mark in marks {
        let (x0, y0, x1, y1) = mark.bbox;
        let corners = [
            (i64::from(x0), i64::from(y0)),
            (i64::from(x1), i64::from(y0)),
            (i64::from(x1), i64::from(y1)),
            (i64::from(x0), i64::from(y1)),
        ];
        for (from, to) in box_edges(&corners) {
            canvas.line(from, to, CYAN);
        }
    }
}

/// The four closing edges of a corner quad.
fn box_edges(corners: &[(i64, i64); 4]) -> Vec<Edge> {
    let [first, second, third, fourth] = *corners;
    vec![
        (first, second),
        (second, third),
        (third, fourth),
        (fourth, first),
    ]
}

/// The `R`, `G` and `M` ids at their centroids, only for what is reported.
fn paint_labels(
    canvas: &mut Canvas,
    vectorized: Option<&Vectorized>,
    gaps: &[Gap],
    marks: &[Mark],
) {
    if let Some(shapes) = vectorized {
        for shape in &shapes.regions {
            let (x, y) = i64_pair(shape.centroid);
            draw_text(canvas, (x, y), &format!("R{}", shape.id), LABEL);
        }
    }
    for gap in gaps {
        let (x, y) = i64_pair(gap.centroid);
        draw_text(canvas, (x, y), &format!("G{}", gap.id), LABEL);
    }
    for mark in marks {
        let (x, y) = i64_pair(mark.centroid);
        draw_text(canvas, (x, y), &format!("M{}", mark.id), LABEL);
    }
}

/// A measured `(u32, u32)` point as canvas coordinates.
fn i64_pair(point: (u32, u32)) -> (i64, i64) {
    (i64::from(point.0), i64::from(point.1))
}

#[cfg(test)]
mod tests {
    use super::{CYAN, MAGENTA, REGION_COLORS, build, darken, region_colour, ring_edges, wash};
    use crate::gfx::image::RgbaImage;
    use crate::sketch::vectorize::VectorizeOptions;
    use crate::sketch::{Analysis, AnalyzeOptions, InkMode, Vectorized, analyze, vectorize};

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

    /// The two-room fixture: boxes with a 10-px door break at rows 55..64
    /// and a 16x16 blot inside the left room.
    fn rooms() -> RgbaImage {
        let mut img = paper(200, 120);
        paint(&mut img, (30, 30, 99, 32));
        paint(&mut img, (30, 87, 99, 89));
        paint(&mut img, (30, 30, 32, 89));
        paint(&mut img, (97, 30, 99, 89));
        paint(&mut img, (63, 30, 65, 54));
        paint(&mut img, (63, 65, 65, 89));
        paint(&mut img, (40, 50, 55, 65));
        img
    }

    /// One analysed fixture with its vectorisation.
    fn measured() -> (RgbaImage, Analysis, Option<Vectorized>) {
        let picture = rooms();
        let options = AnalyzeOptions {
            ink: InkMode::Dark { level: Some(128) },
            close_radius: None,
            min_region_area: None,
            mark_max_size: None,
        };
        let analysis = analyze(&picture, &options).expect("fixture is analysable");
        let shapes = vectorize(&analysis, &VectorizeOptions::default()).ok();
        (picture, analysis, shapes)
    }

    #[test]
    fn the_wash_keeps_forty_percent_of_the_ink_towards_paper() {
        assert_eq!(wash(0_u8), 153_u8, "black lifts to 40% grey");
        assert_eq!(wash(255_u8), 255_u8, "paper stays paper");
        assert!(wash(128_u8) > 128_u8, "mid grey lightens");
    }

    #[test]
    fn the_colour_table_cycles_by_region_id_and_darkens_for_outlines() {
        assert_eq!(region_colour(1_u32), REGION_COLORS[0], "id 1 takes slot 0");
        assert_eq!(region_colour(13_u32), REGION_COLORS[0], "the table cycles");
        assert_eq!(
            darken([255_u8, 128, 66]),
            [127_u8, 64, 33, 255],
            "halved channels, opaque alpha"
        );
    }

    #[test]
    fn ring_edges_close_the_polygon_and_ignore_degenerate_rings() {
        let square = vec![(0_i64, 0_i64), (10, 0), (10, 10), (0, 10)];
        let edges = ring_edges(&square);
        assert_eq!(edges.len(), 4_usize, "one edge per vertex");
        assert_eq!(
            edges.last().copied(),
            Some(((0, 10), (0, 0))),
            "closing edge"
        );
        assert!(ring_edges(&[]).is_empty(), "an empty ring draws nothing");
        assert!(
            ring_edges(&[(1, 1)]).is_empty(),
            "a lone point draws nothing"
        );
    }

    /// Counts the pixels of one exact colour in an overlay.
    fn count_colour(img: &RgbaImage, colour: [u8; 4]) -> usize {
        (0..img.height)
            .flat_map(|row| (0..img.width).map(move |column| (column, row)))
            .filter(|&(column, row)| img.get(column, row) == Some(colour))
            .count()
    }

    #[test]
    fn the_overlay_washes_paints_and_labels_the_measured_sketch() {
        let (picture, analysis, shapes) = measured();
        let overlay = build(&picture, shapes.as_ref(), &analysis.gaps, &analysis.marks);
        assert_eq!(
            (overlay.width, overlay.height),
            (picture.width, picture.height),
            "the overlay keeps the analysed size"
        );
        assert_eq!(
            overlay.get(0_u32, 0_u32),
            Some([255, 255, 255, 255]),
            "blank paper stays white"
        );
        let stroke_px = overlay.get(31_u32, 31_u32).expect("in bounds");
        assert!(
            stroke_px[0] < 255_u8 && stroke_px[1] < 255_u8,
            "the washed, filled stroke darkens: {stroke_px:?}"
        );
        let painted = (0..overlay.height)
            .flat_map(|row| (0..overlay.width).map(move |column| (column, row)))
            .filter(|&(column, row)| overlay.get(column, row) != Some([255, 255, 255, 255]))
            .count();
        assert!(
            painted > 1_000,
            "fills and outlines cover the rooms: {painted}"
        );
    }

    #[test]
    fn the_overlay_draws_only_the_lists_it_is_given() {
        let (picture, analysis, shapes) = measured();
        let full = build(&picture, shapes.as_ref(), &analysis.gaps, &analysis.marks);
        let bare = build(&picture, shapes.as_ref(), &[], &[]);
        assert_eq!(analysis.gaps.len(), 1, "the fixture bridges the door");
        assert_eq!(analysis.marks.len(), 1, "and splits the blot");
        assert!(
            count_colour(&full, MAGENTA) > 0 && count_colour(&full, CYAN) > 0,
            "the kept rows are drawn as a disc and a box: {} {}",
            count_colour(&full, MAGENTA),
            count_colour(&full, CYAN),
        );
        assert_eq!(
            count_colour(&bare, MAGENTA),
            0,
            "no gap list means no magenta"
        );
        assert_eq!(count_colour(&bare, CYAN), 0, "no mark list means no cyan");
    }

    #[test]
    fn the_overlay_of_an_empty_analysis_is_the_washed_picture_alone() {
        let picture = paper(40_u32, 30_u32);
        let options = AnalyzeOptions {
            ink: InkMode::Dark { level: Some(128) },
            close_radius: None,
            min_region_area: None,
            mark_max_size: None,
        };
        let analysis = analyze(&picture, &options).expect("blank is analysable");
        let overlay = build(&picture, None, &analysis.gaps, &analysis.marks);
        assert_eq!((overlay.width, overlay.height), (40_u32, 30_u32));
        assert_eq!(
            overlay.get(20_u32, 15_u32),
            Some([255, 255, 255, 255]),
            "nothing is drawn on the blank sheet"
        );
    }
}
