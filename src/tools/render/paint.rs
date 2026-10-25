//! The shared drawing state of `render_topdown`: the pixel colour helpers,
//! the [`Scene`] (geometry, definitions, fitted [`View`] and canvas) with
//! its element reads, and the top-down and isometric painting order with
//! the underlay and grid passes. The per-element strokes live in
//! [`super::marks`].

use crate::defs::Definitions;
use crate::fixed::mul_div_round;
use crate::render::{
    AutomapColors, Canvas, Edge, Pt, Rgba, TopdownStyle, automap_color, side_sector,
};
use crate::wad::map::geometry;
use crate::wad::map::{Linedef, Map};

use super::frame::View;
use super::marks::{
    paint_annotations, paint_iso_lines, paint_iso_things, paint_labels, paint_sector_fills,
    paint_thing_marks, paint_topdown_lines, paint_vertex_marks,
};
use super::params::{Request, Underlay};
use super::{HIGHLIGHT_RED, HIGHLIGHT_WIDTH, LINE_ONE_SIDED, LINE_TWO_SIDED, MAX_OPACITY};

/// Editor style: a line with a special.
const LINE_SPECIAL: Rgba = [106, 140, 255, 255];
/// A `grid` spacing measuring under this many pixels on screen is skipped.
const MIN_GRID_PIXELS: i64 = 4;
/// Every `GRID_MAJOR_EVERY`th grid line draws in the major colour.
const GRID_MAJOR_EVERY: i64 = 8;
/// The grid's minor lines.
pub(super) const GRID_MINOR: Rgba = [42, 42, 42, 255];
/// The grid's every-[`GRID_MAJOR_EVERY`]th lines.
pub(super) const GRID_MAJOR: Rgba = [60, 60, 60, 255];

/// Number of colour channels in a pixel (`background_pixel` parses those).
const COLOUR_CHANNELS: usize = 3;

/// The `#RRGGBB` settings colour as an opaque pixel; text the settings
/// validator already accepted, so a bad byte just reads as `0`.
pub(super) fn background_pixel(text: &str) -> Rgba {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let mut pixel = [0_u8, 0_u8, 0_u8, 255_u8];
    for (slot, pair) in pixel
        .iter_mut()
        .take(COLOUR_CHANNELS)
        .zip(digits.as_bytes().chunks_exact(2))
    {
        let Ok(pair_text) = core::str::from_utf8(pair) else {
            continue;
        };
        *slot = u8::from_str_radix(pair_text, 16).unwrap_or(0);
    }
    pixel
}

/// The editor-style colour of `line`: specials read blue, two-sided lines
/// mid grey, everything else light grey.
pub(super) const fn editor_line_color(line: &Linedef) -> Rgba {
    if line.special != 0_i32 {
        return LINE_SPECIAL;
    }
    if line.back.is_some() {
        return LINE_TWO_SIDED;
    }
    LINE_ONE_SIDED
}

/// The grey a sector's fill gets: half its light level, rounded.
pub(super) fn light_grey(light: i32) -> Rgba {
    let half_light = mul_div_round(i64::from(light.max(0_i32)), 1, 2).unwrap_or(0);
    let level = u8::try_from(half_light).unwrap_or(u8::MAX);
    [level, level, level, 255_u8]
}

/// The world corners of one isometrically drawn linedef.
#[derive(Clone, Copy)]
pub(super) struct IsoPair {
    /// The start vertex's `(x, y)`.
    pub(super) start: (i64, i64),
    /// The end vertex's `(x, y)`.
    pub(super) end: (i64, i64),
}

/// One render's drawing state: the geometry, the definitions for thing
/// markers, the fitted view and the canvas being painted.
pub(super) struct Scene<'render> {
    /// The map being drawn.
    pub(super) map: &'render Map,
    /// The session definitions behind thing markers.
    pub(super) defs: &'render Definitions,
    /// The fitted plane-to-pixel mapping.
    pub(super) view: View,
    /// The pixels so far.
    pub(super) canvas: Canvas,
    /// What the render skipped or adjusted, for the result's `notes`.
    pub(super) notes: Vec<String>,
}

impl Scene<'_> {
    /// The sector the sidedef `side` opens into, `None` for an absent or
    /// dangling side.
    fn side_sector(&self, side: Option<usize>) -> Option<usize> {
        side_sector(self.map, side)
    }

    /// The floor height behind sidedef `side`, `None` when unresolvable.
    pub(super) fn side_floor(&self, side: Option<usize>) -> Option<i64> {
        self.side_sector(side)
            .and_then(|index| self.map.sectors.get(index))
            .map(|sector| i64::from(sector.floor_height))
    }

    /// The ceiling height behind sidedef `side`, `None` when unresolvable.
    pub(super) fn side_ceiling(&self, side: Option<usize>) -> Option<i64> {
        self.side_sector(side)
            .and_then(|index| self.map.sectors.get(index))
            .map(|sector| i64::from(sector.ceiling_height))
    }

    /// The pixel endpoints of `line`, `None` when a vertex dangles.
    pub(super) fn line_pixels(&self, line: &Linedef) -> Option<(Pt, Pt)> {
        let start = self.map.vertices.get(line.v1)?;
        let end = self.map.vertices.get(line.v2)?;
        Some((
            self.view.point(i64::from(start.x), i64::from(start.y)),
            self.view.point(i64::from(end.x), i64::from(end.y)),
        ))
    }

    /// The boundary of `sector` as pixel-space edges, for the even-odd
    /// fill; dangling references drop out.
    pub(super) fn sector_edges(&self, sector: usize) -> Vec<Edge> {
        geometry::sector_boundary(self.map, sector)
            .into_iter()
            .filter_map(|(index, _)| {
                self.map
                    .linedefs
                    .get(index)
                    .and_then(|line| self.line_pixels(line))
            })
            .collect()
    }

    /// The world `x`/`y` corners of `line`'s two vertices, `None` when a
    /// vertex dangles.
    pub(super) fn iso_endpoints(&self, line: &Linedef) -> Option<IsoPair> {
        let start = self.map.vertices.get(line.v1)?;
        let end = self.map.vertices.get(line.v2)?;
        Some(IsoPair {
            start: (i64::from(start.x), i64::from(start.y)),
            end: (i64::from(end.x), i64::from(end.y)),
        })
    }

    /// The colour and width one linedef gets in either projection.
    pub(super) fn line_mark(
        &self,
        line: &Linedef,
        index: usize,
        request: &Request,
    ) -> Option<(Rgba, u32)> {
        if request.highlight.linedefs.contains(&index) {
            return Some((HIGHLIGHT_RED, HIGHLIGHT_WIDTH));
        }
        match request.style {
            TopdownStyle::Editor => Some((editor_line_color(line), 1_u32)),
            TopdownStyle::Automap => self.automap_line_color(line).map(|rgba| (rgba, 1_u32)),
        }
    }

    /// The `Doom` automap colour of `line` with the RGB default palette,
    /// through the shared classification in [`crate::render::topdown`].
    pub(super) fn automap_line_color(&self, line: &Linedef) -> Option<Rgba> {
        automap_color(self.map, line, &AutomapColors::DEFAULT)
    }
}

/// Paints the whole top-down view in order: background (already the
/// canvas ground), underlay, grid, sector fills, lines, vertices, things
/// with their highlights, then the labels and annotations on top.
pub(super) fn draw_topdown(scene: &mut Scene<'_>, request: &Request) {
    if let Some(underlay) = request.underlay.as_ref() {
        paint_underlay(scene, underlay);
    }
    if let Some(spacing) = request.grid {
        paint_grid(scene, spacing);
    }
    if request.fill_sectors.is_on() {
        paint_sector_fills(scene, request);
    }
    paint_topdown_lines(scene, request);
    if request.show_vertices.is_on() {
        paint_vertex_marks(scene, request);
    }
    if request.show_things.is_on() {
        paint_thing_marks(scene, request);
    }
    if request.labels.any() {
        paint_labels(scene, request);
    }
    paint_annotations(scene, request);
}

/// Draws the `underlay`: every output pixel is inverted through the
/// fitted [`View`] to a world point, mapped back into image pixels by
/// [`Units::to_image`] and nearest-neighbour sampled; the texel blends
/// over the background at `opacity` percent of its own alpha. A world point
/// answers with a texel only while it lies within the picture's own columns
/// and rows, so the image is drawn once and never repeated past an edge:
/// every other output pixel, in either direction and whatever the origin,
/// keeps what was painted there.
fn paint_underlay(scene: &mut Scene<'_>, underlay: &Underlay) {
    let percent = i64::from(underlay.opacity.min(MAX_OPACITY));
    for row in 0..i64::from(scene.canvas.height()) {
        for col in 0..i64::from(scene.canvas.width()) {
            paint_underlay_pixel(scene, underlay, percent, (col, row));
        }
    }
}

/// One [`paint_underlay`] output pixel, dropping everything the mapping
/// or the image cannot answer for.
fn paint_underlay_pixel(scene: &mut Scene<'_>, underlay: &Underlay, percent: i64, at: Pt) {
    let (x, y) = scene.view.unproject(at.0, at.1);
    let Some(texel) = underlay.units.to_image(x, y).and_then(|(px, py)| {
        underlay
            .image
            .get(u32::try_from(px).ok()?, u32::try_from(py).ok()?)
    }) else {
        return;
    };
    let [red, green, blue, alpha] = texel;
    let mixed = scale_alpha(alpha, percent);
    if mixed == 0_u8 {
        return;
    }
    scene.canvas.blend(at.0, at.1, [red, green, blue, mixed]);
}

/// `alpha` scaled to `percent` percent, rounded; `percent` is `0..=100`.
pub(super) fn scale_alpha(alpha: u8, percent: i64) -> u8 {
    let lifted = i64::from(alpha)
        .saturating_mul(percent)
        .saturating_add(50_i64)
        .checked_div(100_i64)
        .unwrap_or(0_i64);
    u8::try_from(lifted).unwrap_or(0_u8)
}

/// Draws the map-unit grid: a line at every multiple of `spacing` across
/// the visible world, every [`GRID_MAJOR_EVERY`]th one in the major
/// colour. A spacing that measures under [`MIN_GRID_PIXELS`] pixels on
/// screen is skipped with a [`Scene::notes`] line instead of mud.
fn paint_grid(scene: &mut Scene<'_>, spacing: i64) {
    let pixels = scene.view.scale(spacing);
    if pixels < MIN_GRID_PIXELS {
        scene.notes.push(format!(
            "grid skipped: {spacing} map units measure {pixels} pixels at this zoom; use a \
             coarser spacing or a larger output"
        ));
        return;
    }
    let (left, top, right, bottom) = grid_frame(scene);
    for k in left.div_euclid(spacing)..=right.div_euclid(spacing) {
        let line = GridLine {
            at: k.saturating_mul(spacing),
            vertical: true,
            low: bottom,
            high: top,
        };
        paint_grid_line(scene, &line, k);
    }
    for k in bottom.div_euclid(spacing)..=top.div_euclid(spacing) {
        let line = GridLine {
            at: k.saturating_mul(spacing),
            vertical: false,
            low: left,
            high: right,
        };
        paint_grid_line(scene, &line, k);
    }
}

/// The world rectangle the whole canvas covers, as `(left, top, right,
/// bottom)`: the fitted view read at the canvas corners.
pub(super) fn grid_frame(scene: &Scene<'_>) -> (i64, i64, i64, i64) {
    let (left, top) = scene.view.unproject(0, 0);
    let (right, bottom) = scene.view.unproject(
        i64::from(scene.canvas.width().saturating_sub(1)),
        i64::from(scene.canvas.height().saturating_sub(1)),
    );
    (left, top, right, bottom)
}

/// One grid line's placement: the world coordinate it runs along, the
/// span it covers on the other axis, and which axis is which.
pub(super) struct GridLine {
    /// The fixed coordinate: `k * spacing` map units.
    pub(super) at: i64,
    /// Whether [`at`](Self::at) measures along `x` (a vertical line) or
    /// along `y`.
    pub(super) vertical: bool,
    /// The span's lower world coordinate on the other axis.
    pub(super) low: i64,
    /// The span's higher world coordinate on the other axis.
    pub(super) high: i64,
}

/// Draws one grid line, in the [`GRID_MAJOR`] colour when its multiple
/// index `k` is a full [`GRID_MAJOR_EVERY`]th step, [`GRID_MINOR`] else.
pub(super) fn paint_grid_line(scene: &mut Scene<'_>, line: &GridLine, k: i64) {
    let colour = if k.rem_euclid(GRID_MAJOR_EVERY) == 0_i64 {
        GRID_MAJOR
    } else {
        GRID_MINOR
    };
    let (from, to) = if line.vertical {
        (
            scene.view.point(line.at, line.low),
            scene.view.point(line.at, line.high),
        )
    } else {
        (
            scene.view.point(line.low, line.at),
            scene.view.point(line.high, line.at),
        )
    };
    scene.canvas.line(from, to, colour);
}

/// Paints the whole isometric wireframe: height-aware lines and thing
/// markers. There is no hidden-line removal: every edge draws.
pub(super) fn draw_isometric(scene: &mut Scene<'_>, request: &Request) {
    paint_iso_lines(scene, request);
    if request.show_things.is_on() {
        paint_iso_things(scene, request);
    }
}
