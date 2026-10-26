//! Integer raster canvas: the shared pixel target behind every render tool.
//!
//! A [`Canvas`] is an owned `RGBA8` buffer with drawing primitives that take
//! `i64` coordinates and silently clip to the buffer: [`Canvas::set`],
//! [`Canvas::blend`] (alpha-over, for highlights), [`Canvas::line`] and
//! [`Canvas::thick_line`] (integer Bresenham, `Cohen-Sutherland` clipped),
//! [`Canvas::fill_rect`], [`Canvas::fill_circle`] and the scanline
//! even-odd [`Canvas::fill_polygon_evenodd`]. Nothing here knows about
//! maps, styles or MCP: callers map their own geometry into pixel space,
//! then finish with [`Canvas::apply_gamma`] and [`Canvas::to_rgba_image`].

use core::ops::RangeInclusive;

use crate::fixed::{isqrt, narrow_i64};
use crate::gfx::image::RgbaImage;
use crate::gfx::{
    BYTES_PER_PIXEL, MAX_DIMENSION, clamp_dimension, pixel_count, pixel_index, to_pixel,
};

/// One canvas position in integer pixel coordinates.
pub type Pt = (i64, i64);

/// One `RGBA8` colour.
pub use crate::gfx::Rgba;

/// One polygon edge: a pair of [`Pt`] endpoints.
pub type Edge = (Pt, Pt);

/// Number of colour channels in front of a pixel's alpha byte.
const RGB_CHANNELS: usize = 3;

/// The brightest value an 8-bit channel holds.
const CHANNEL_FULL: u32 = 255;

/// Half of 255, the bias that turns alpha-over products into rounded
/// divisions.
const ALPHA_HALF: u32 = 127;

/// The largest circle radius the canvas measures; anything bigger covers
/// the whole [`MAX_DIMENSION`] canvas anyway, and this bound keeps every
/// squared product exact.
const MAX_RADIUS: i64 = 8_192;

/// Rounds a `Cohen-Sutherland` clip needs at most: four edges, one
/// settled per round per endpoint.
const CLIP_ROUNDS: u32 = 8;

/// Outcode bit: the point is left of the canvas.
const OUT_LEFT: u32 = 1;
/// Outcode bit: the point is right of the canvas.
const OUT_RIGHT: u32 = 2;
/// Outcode bit: the point is above the canvas.
const OUT_TOP: u32 = 4;
/// Outcode bit: the point is below the canvas.
const OUT_BOTTOM: u32 = 8;

/// A line stroke: a colour drawn at a pixel width.
///
/// An odd width centres on the line; an even width is floored to the next
/// odd one (see [`Canvas::thick_line`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    /// Width in pixels; `0` draws like `1`.
    pub width: u32,
    /// The colour drawn.
    pub rgba: Rgba,
}

impl Stroke {
    /// A stroke `width` pixels wide in `rgba`.
    #[must_use]
    #[inline]
    pub const fn new(width: u32, rgba: Rgba) -> Self {
        Self { width, rgba }
    }

    /// A one-pixel stroke in `rgba`.
    #[must_use]
    #[inline]
    pub const fn thin(rgba: Rgba) -> Self {
        Self { width: 1, rgba }
    }
}

/// An owned `RGBA8` raster with clipped integer drawing primitives.
///
/// Coordinates are `i64` pixels; anything outside the buffer is dropped
/// silently, so callers never clip by hand. The buffer starts as one flat
/// [`Canvas::new`] background; [`Canvas::blend`] is the only compositing
/// primitive, everything else replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    /// Pixels per row, `1..=8192`.
    width: u32,
    /// Rows, `1..=8192`.
    height: u32,
    /// Four bytes per pixel (red, green, blue, alpha), row-major.
    rgba: Vec<u8>,
}

impl Canvas {
    /// A `width` by `height` canvas filled with `background`.
    ///
    /// Dimensions are saturated into `1..=MAX_DIMENSION` (the edge limit
    /// the image constructors accept), so this never fails and the result
    /// always holds exactly `width * height * 4` bytes.
    #[must_use]
    #[inline]
    pub fn new(width: u32, height: u32, background: Rgba) -> Self {
        let columns = clamp_dimension(width).min(MAX_DIMENSION);
        let rows = clamp_dimension(height).min(MAX_DIMENSION);
        Self {
            width: columns,
            height: rows,
            rgba: paint_rows(columns, rows, background),
        }
    }

    /// The canvas width in pixels.
    #[must_use]
    #[inline]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The canvas height in pixels.
    #[must_use]
    #[inline]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The pixel at `(x, y)`, or `None` outside the canvas.
    #[must_use]
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = pixel_index(self.width, x, y)?;
        self.rgba
            .chunks_exact(BYTES_PER_PIXEL)
            .nth(index)
            .map(to_pixel)
    }

    /// Writes `rgba` at `(x, y)`; off-canvas coordinates are dropped.
    #[inline]
    pub fn set(&mut self, x: i64, y: i64, rgba: Rgba) {
        set_pixel(&mut self.rgba, self.width, (x, y), rgba);
    }

    /// Composites `rgba` over the pixel at `(x, y)` (src-over alpha);
    /// off-canvas coordinates are dropped.
    #[inline]
    pub fn blend(&mut self, x: i64, y: i64, rgba: Rgba) {
        blend_pixel(&mut self.rgba, self.width, (x, y), rgba);
    }

    /// Draws a one-pixel-wide line from `from` to `to` (both inclusive) in
    /// `rgba`: integer Bresenham, clipped to the canvas.
    #[inline]
    pub fn line(&mut self, from: Pt, to: Pt, rgba: Rgba) {
        paint_stroke(self, from, to, Stroke::thin(rgba));
    }

    /// Draws a line from `from` to `to` in `stroke`.
    ///
    /// The stroke's half-width offsets the one-pixel Bresenham line
    /// horizontally and vertically, so axis-aligned lines come out exactly
    /// `width` pixels thick; other angles get a rounded blob.
    #[inline]
    pub fn thick_line(&mut self, from: Pt, to: Pt, stroke: Stroke) {
        paint_stroke(self, from, to, stroke);
    }

    /// Fills the inclusive rectangle spanning the corners `min` and `max`
    /// (any corner order) with `rgba`.
    #[inline]
    pub fn fill_rect(&mut self, min: Pt, max: Pt, rgba: Rgba) {
        let (x0, x1) = ordered(min.0, max.0);
        let (y0, y1) = ordered(min.1, max.1);
        for y in self.row_range(y0, y1) {
            self.fill_span(&Span { y, x0, x1 }, rgba, false);
        }
    }

    /// Fills the disc centred on `center` with `radius` (inclusive) in
    /// `rgba`. A radius below `1` plots the centre pixel alone.
    #[inline]
    pub fn fill_circle(&mut self, center: Pt, radius: i64, rgba: Rgba) {
        if radius < 1 {
            self.set(center.0, center.1, rgba);
            return;
        }
        for dy in disc_rows(radius) {
            self.fill_span(&disc_run(center, radius, dy), rgba, false);
        }
    }

    /// Fills the even-odd region enclosed by `edges` with `rgba`: a scan
    /// line through each integer pixel row counts the edge crossings and
    /// fills between the 1st and 2nd, 3rd and 4th, ... of them, so nested
    /// rings come out as holes. Sub-pixel polygon detail is lost, as every
    /// integer render must accept.
    #[inline]
    pub fn fill_polygon_evenodd(&mut self, edges: &[Edge], rgba: Rgba) {
        self.walk_polygon(edges, rgba, false);
    }

    /// [`Canvas::fill_polygon_evenodd`]'s scanline walk with alpha-over
    /// blending: every covered pixel is composited instead of replaced.
    /// Used for translucent highlight tints.
    #[inline]
    pub fn blend_polygon_evenodd(&mut self, edges: &[Edge], rgba: Rgba) {
        self.walk_polygon(edges, rgba, true);
    }

    /// Maps every colour channel (not alpha) through `table`, a
    /// [`crate::render::gamma_table`]: renderers apply the session gamma
    /// once, at encode time, so the buffer stays editable before.
    /// Channels the table is too short for keep their value.
    #[inline]
    pub fn apply_gamma(&mut self, table: &[u8]) {
        apply_gamma_bytes(&mut self.rgba, table);
    }

    /// The canvas as an owned [`RgbaImage`] (pixels copied out).
    #[must_use]
    #[inline]
    pub fn to_rgba_image(&self) -> RgbaImage {
        RgbaImage {
            width: self.width,
            height: self.height,
            pixels: self.rgba.clone(),
        }
    }

    /// The last pixel column and row of the canvas.
    fn edge(&self) -> Pt {
        (
            i64::from(self.width.saturating_sub(1)),
            i64::from(self.height.saturating_sub(1)),
        )
    }

    /// The canvas rows spanning `y0..=y1`.
    fn row_range(&self, y0: i64, y1: i64) -> RangeInclusive<i64> {
        y0.max(0)..=y1.min(self.edge().1)
    }

    /// Fills the clipped columns of one row, replacing or blending per
    /// `over`.
    fn fill_span(&mut self, run: &Span, rgba: Rgba, over: bool) {
        let last = run.x1.min(self.edge().0);
        for x in run.x0.max(0)..=last {
            place_pixel(self, (x, run.y), rgba, over);
        }
    }

    /// The shared even-odd scanline fill behind
    /// [`Canvas::fill_polygon_evenodd`] and [`Canvas::blend_polygon_evenodd`].
    fn walk_polygon(&mut self, edges: &[Edge], rgba: Rgba, over: bool) {
        let (Some(min_y), Some(max_y)) = (edges_min_y(edges), edges_max_y(edges)) else {
            return;
        };
        let scan = Scan { edges, rgba, over };
        for y in self.row_range(min_y, max_y) {
            self.polygon_row(y, &scan);
        }
    }

    /// Fills (or tints) the even-odd spans of the scan's edges on row `y`.
    fn polygon_row(&mut self, y: i64, scan: &Scan<'_>) {
        let mut crossings = polygon_crossings(scan.edges, y).into_iter();
        while let (Some(first), Some(second)) = (crossings.next(), crossings.next()) {
            self.fill_span(
                &Span {
                    y,
                    x0: first,
                    x1: second,
                },
                scan.rgba,
                scan.over,
            );
        }
    }
}

/// One scanline of an even-odd polygon fill: the edges, the colour and
/// the blend mode.
struct Scan<'edges> {
    /// The polygon edges to count crossings against.
    edges: &'edges [Edge],
    /// The pixel colour to draw.
    rgba: Rgba,
    /// Whether to composite over instead of replacing.
    over: bool,
}

/// Allocates a `width` by `height` pixel buffer filled with `background`.
fn paint_rows(width: u32, height: u32, background: Rgba) -> Vec<u8> {
    let count = pixel_count(width, height).unwrap_or(0);
    let mut buffer = Vec::with_capacity(count.saturating_mul(BYTES_PER_PIXEL));
    for _pixel in 0..count {
        buffer.extend_from_slice(&background);
    }
    buffer
}

/// The byte offset of the pixel at `(x, y)` in a `width`-wide buffer,
/// `None` when it leaves the canvas.
fn pixel_start(rgba: &[u8], width: u32, at: Pt) -> Option<usize> {
    let column = u32::try_from(at.0).ok()?;
    let row = u32::try_from(at.1).ok()?;
    if column >= width {
        return None;
    }
    let index = pixel_index(width, column, row)?;
    let start = index.checked_mul(BYTES_PER_PIXEL)?;
    if rgba.len() >= start.checked_add(BYTES_PER_PIXEL)? {
        return Some(start);
    }
    None
}

/// The four bytes of the pixel at `at`, `None` off-canvas.
fn pixel_window(rgba: &[u8], width: u32, at: Pt) -> Option<&[u8]> {
    let start = pixel_start(rgba, width, at)?;
    rgba.get(start..start.checked_add(BYTES_PER_PIXEL)?)
}

/// Replaces the pixel at `at`, dropping off-canvas writes.
fn set_pixel(rgba: &mut [u8], width: u32, at: Pt, color: Rgba) {
    let Some(start) = pixel_start(rgba, width, at) else {
        return;
    };
    let Some(end) = start.checked_add(BYTES_PER_PIXEL) else {
        return;
    };
    if let Some(slot) = rgba.get_mut(start..end) {
        slot.copy_from_slice(&color);
    }
}

/// Composites `color` over the pixel at `at`, dropping off-canvas writes.
fn blend_pixel(rgba: &mut [u8], width: u32, at: Pt, color: Rgba) {
    let Some(current) = pixel_window(rgba, width, at) else {
        return;
    };
    let merged = blend_over(to_pixel(current), color);
    set_pixel(rgba, width, at, merged);
}

/// Draws `color` at `at`, replacing or blending per `over`.
fn place_pixel(canvas: &mut Canvas, at: Pt, color: Rgba, over: bool) {
    if over {
        canvas.blend(at.0, at.1, color);
    } else {
        canvas.set(at.0, at.1, color);
    }
}

/// One axis pass of a [`Canvas::thick_line`] stroke: the base endpoints
/// shifted perpendicular to one axis.
struct Pass {
    /// The line's start.
    from: Pt,
    /// The line's end.
    to: Pt,
    /// This pass's perpendicular shift in pixels.
    offset: i64,
    /// The stroke being painted.
    stroke: Stroke,
}

/// Paints a whole stroke: for each half-width offset, the pass shifted
/// along `x` and the pass shifted along `y`.
fn paint_stroke(canvas: &mut Canvas, from: Pt, to: Pt, stroke: Stroke) {
    let half = i64::from(stroke.width.saturating_sub(1).checked_div(2).unwrap_or(0));
    for offset in half.saturating_neg()..=half {
        let pass = Pass {
            from,
            to,
            offset,
            stroke,
        };
        stroke_pass(canvas, &pass, true);
        stroke_pass(canvas, &pass, false);
    }
}

/// Draws one [`Pass`] of a stroke, clipped to the canvas.
fn stroke_pass(canvas: &mut Canvas, pass: &Pass, along_x: bool) {
    let zero = 0_i64;
    let dx = if along_x { pass.offset } else { zero };
    let dy = if along_x { zero } else { pass.offset };
    let edge = canvas.edge();
    let from = shift(pass.from, dx, dy);
    let to = shift(pass.to, dx, dy);
    if let Some((begin, finish)) = clip(from, to, edge) {
        draw_line(canvas, begin, finish, pass.stroke.rgba);
    }
}

/// `+1` while `rising`, `-1` otherwise: a [`draw_line`] axis step.
const fn direction(rising: bool) -> i64 {
    if rising { 1_i64 } else { -1_i64 }
}

/// The integer Bresenham walk between two points, both inclusive. The
/// buffer's own clipping makes any rounding overshoot harmless.
fn draw_line(canvas: &mut Canvas, from: Pt, to: Pt, color: Rgba) {
    let (mut x, mut y) = from;
    let (end_x, end_y) = to;
    let delta_x = end_x.saturating_sub(x).abs();
    let delta_y = end_y.saturating_sub(y).abs().saturating_neg();
    let step_x = direction(x < end_x);
    let step_y = direction(y < end_y);
    let mut error = delta_x.saturating_add(delta_y);
    for _ in 0..=delta_x.max(delta_y.saturating_neg()) {
        canvas.set(x, y, color);
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error.saturating_mul(2);
        if doubled >= delta_y {
            error = error.saturating_add(delta_y);
            x = x.saturating_add(step_x);
        }
        if doubled <= delta_x {
            error = error.saturating_add(delta_x);
            y = y.saturating_add(step_y);
        }
    }
}

/// The [`Cohen-Sutherland`] outcode of `point`: which sides of the box
/// `0..=edge` it lies outside.
///
/// [`Cohen-Sutherland`]: https://en.wikipedia.org/wiki/Cohen%E2%80%93Sutherland
const fn outcode(point: Pt, edge: Pt) -> u32 {
    let mut code = 0_u32;
    if point.0 < 0 {
        code |= OUT_LEFT;
    }
    if point.0 > edge.0 {
        code |= OUT_RIGHT;
    }
    if point.1 < 0 {
        code |= OUT_TOP;
    }
    if point.1 > edge.1 {
        code |= OUT_BOTTOM;
    }
    code
}

/// Clips the segment `from`-`to` to the box `0..=edge`, `None` when no
/// part of it is visible. Intersection coordinates round down; an
/// unconverged degenerate segment is dropped too.
fn clip(from: Pt, to: Pt, edge: Pt) -> Option<(Pt, Pt)> {
    let mut start = from;
    let mut end = to;
    let mut code_start = outcode(start, edge);
    let mut code_end = outcode(end, edge);
    for _ in 0..CLIP_ROUNDS {
        if code_start | code_end == 0 {
            return Some((start, end));
        }
        if code_start & code_end != 0 {
            return None;
        }
        let (moving_start, outside, inside) = if code_start != 0 {
            (true, start, end)
        } else {
            (false, end, start)
        };
        let outside_code = if moving_start { code_start } else { code_end };
        let moved = clip_to_edge(outside, inside, outside_code, edge);
        if moving_start {
            start = moved;
            code_start = outcode(start, edge);
        } else {
            end = moved;
            code_end = outcode(end, edge);
        }
    }
    None
}

/// Moves `outside` onto the box edge its `code_out` names, along the
/// `outside`-`inside` line.
fn clip_to_edge(outside: Pt, inside: Pt, code_out: u32, edge: Pt) -> Pt {
    if code_out & OUT_LEFT != 0 {
        return (0, intersect_y(outside, inside, 0));
    }
    if code_out & OUT_RIGHT != 0 {
        return (edge.0, intersect_y(outside, inside, edge.0));
    }
    if code_out & OUT_TOP != 0 {
        return (intersect_x(outside, inside, 0), 0);
    }
    if code_out & OUT_BOTTOM != 0 {
        return (intersect_x(outside, inside, edge.1), edge.1);
    }
    outside
}

/// The dy rows a filled disc of `radius` spans (inclusive).
fn disc_rows(radius: i64) -> RangeInclusive<i64> {
    let r = radius.min(MAX_RADIUS);
    r.saturating_neg()..=r
}

/// One row of a filled disc: the row's `y` and its inclusive columns.
struct Span {
    /// The row to fill.
    y: i64,
    /// The first covered column.
    x0: i64,
    /// The last covered column.
    x1: i64,
}

/// The row `center.y + dy` of the disc at `center`: the integer circle
/// equation `x0..=x1 = center.x ± isqrt(r² - dy²)`.
fn disc_run(center: Pt, radius: i64, dy: i64) -> Span {
    let r = radius.min(MAX_RADIUS);
    let squared = r.saturating_mul(r);
    let inside = squared.saturating_sub(dy.saturating_mul(dy));
    let span = i64::try_from(isqrt(u64::try_from(inside).unwrap_or(0))).unwrap_or(MAX_RADIUS);
    Span {
        y: center.1.saturating_add(dy),
        x0: center.0.saturating_sub(span),
        x1: center.0.saturating_add(span),
    }
}

/// Maps every colour byte of `buffer` through `table` (alpha aside).
fn apply_gamma_bytes(buffer: &mut [u8], table: &[u8]) {
    for chunk in buffer.chunks_exact_mut(BYTES_PER_PIXEL) {
        gamma_chunk(chunk, table);
    }
}

/// Gamma-maps a pixel's three colour channels, leaving alpha alone.
fn gamma_chunk(chunk: &mut [u8], table: &[u8]) {
    for slot in chunk.iter_mut().take(RGB_CHANNELS) {
        let corrected = table.get(usize::from(*slot)).copied().unwrap_or(*slot);
        *slot = corrected;
    }
}

/// The lowest endpoint row over `edges`, `None` for an empty slice.
fn edges_min_y(edges: &[Edge]) -> Option<i64> {
    edges
        .iter()
        .flat_map(|&(start, end)| [start.1, end.1])
        .min()
}

/// The highest endpoint row over `edges`, `None` for an empty slice.
fn edges_max_y(edges: &[Edge]) -> Option<i64> {
    edges
        .iter()
        .flat_map(|&(start, end)| [start.1, end.1])
        .max()
}

/// The pixel columns where `edges` cross row `y`, sorted; the classic
/// half-open `(y0 > y) != (y1 > y)` rule counts joins once.
fn polygon_crossings(edges: &[Edge], y: i64) -> Vec<i64> {
    let mut found: Vec<i64> = edges
        .iter()
        .filter(|&&(start, end)| (start.1 > y) != (end.1 > y))
        .map(|&(start, end)| intersect_x(start, end, y))
        .collect();
    found.sort_unstable();
    found
}

/// Shifts a point by an `i64` offset on each axis (saturating).
const fn shift(point: Pt, dx: i64, dy: i64) -> Pt {
    (point.0.saturating_add(dx), point.1.saturating_add(dy))
}

/// The two values ordered as `(smaller, larger)`.
fn ordered(a: i64, b: i64) -> (i64, i64) {
    (a.min(b), a.max(b))
}

/// `base + delta * num / den` in exact `i128`, rounded down, saturating
/// back to `i64`; a zero `den` (a degenerate segment) answers `base`.
fn scale_down(base: i64, delta: i64, num: i128, den: i128) -> i64 {
    if den == 0 {
        return base;
    }
    let (n, d) = if den < 0 {
        (
            i128::from(delta).saturating_neg().saturating_mul(num),
            den.saturating_neg(),
        )
    } else {
        (i128::from(delta).saturating_mul(num), den)
    };
    let drawn = i128::from(base)
        .saturating_mul(d)
        .saturating_add(n)
        .div_euclid(d);
    narrow_i64(drawn)
}

/// The column where the `start`-`end` line crosses row `y`, floor-rounded.
pub(crate) fn intersect_x(start: Pt, end: Pt, y: i64) -> i64 {
    let rise = i128::from(y).saturating_sub(i128::from(start.1));
    let span = i128::from(end.1).saturating_sub(i128::from(start.1));
    scale_down(start.0, end.0.saturating_sub(start.0), rise, span)
}

/// The row where the `start`-`end` line crosses column `x`, floor-rounded.
fn intersect_y(start: Pt, end: Pt, x: i64) -> i64 {
    let run = i128::from(x).saturating_sub(i128::from(start.0));
    let span = i128::from(end.0).saturating_sub(i128::from(start.0));
    scale_down(start.1, end.1.saturating_sub(start.1), run, span)
}

/// One alpha-over channel mix, rounded by [`ALPHA_HALF`].
fn blend_channel(dst: u8, src: u8, alpha: u8) -> u8 {
    let weight = u32::from(alpha);
    let inverse = CHANNEL_FULL.saturating_sub(weight);
    let mixed = u32::from(src)
        .saturating_mul(weight)
        .saturating_add(u32::from(dst).saturating_mul(inverse))
        .saturating_add(ALPHA_HALF)
        .checked_div(CHANNEL_FULL)
        .unwrap_or(0);
    u8::try_from(mixed).unwrap_or(u8::MAX)
}

/// `src` composited over `dst` with src-over alpha.
fn blend_over(dst: Rgba, src: Rgba) -> Rgba {
    let mut out = [0_u8; BYTES_PER_PIXEL];
    for ((slot, s), d) in out.iter_mut().zip(src).zip(dst).take(RGB_CHANNELS) {
        *slot = blend_channel(d, s, src[RGB_CHANNELS]);
    }
    let inverse = CHANNEL_FULL.saturating_sub(u32::from(src[RGB_CHANNELS]));
    let kept = u32::from(dst[RGB_CHANNELS])
        .saturating_mul(inverse)
        .checked_div(CHANNEL_FULL)
        .unwrap_or(0);
    let total = u32::from(src[RGB_CHANNELS])
        .saturating_add(kept)
        .min(CHANNEL_FULL);
    out[RGB_CHANNELS] = u8::try_from(total).unwrap_or(u8::MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Edge, Pt, Rgba, Stroke, blend_over};

    /// An opaque test red.
    const RED: Rgba = [255, 0, 0, 255];
    /// An opaque test blue.
    const BLUE: Rgba = [0, 0, 255, 255];
    /// The black an untouched test canvas holds.
    const CLEAR: Rgba = [0, 0, 0, 255];

    /// A `size` by `size` black canvas.
    fn square(size: u32) -> Canvas {
        Canvas::new(size, size, CLEAR)
    }

    /// Reads a pixel through `i64` coordinates, background for misses.
    fn at(canvas: &Canvas, px: i64, py: i64) -> Rgba {
        let wide = usize::try_from(px)
            .ok()
            .and_then(|value| u32::try_from(value).ok());
        let deep = usize::try_from(py)
            .ok()
            .and_then(|value| u32::try_from(value).ok());
        let (Some(column), Some(row)) = (wide, deep) else {
            return CLEAR;
        };
        canvas.pixel(column, row).unwrap_or(CLEAR)
    }

    /// The pixels of `canvas` holding exactly `color`.
    fn painted(canvas: &Canvas, color: Rgba) -> Vec<Pt> {
        let mut found = Vec::new();
        for row in 0..i64::from(canvas.height()) {
            let marks = (0..i64::from(canvas.width())).filter(|col| at(canvas, *col, row) == color);
            found.extend(marks.map(|col| (col, row)));
        }
        found
    }

    /// The four edges of the box from `(x0, y0)` to `(x1, y1)`, closed.
    fn box_edges(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Edge> {
        let corners = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let rotated = corners
            .iter()
            .skip(1)
            .chain(corners.first())
            .copied()
            .collect::<Vec<Pt>>();
        corners.into_iter().zip(rotated).collect()
    }

    #[test]
    fn new_fills_the_whole_buffer_with_the_background() {
        let canvas = Canvas::new(3, 2, RED);
        assert_eq!(canvas.width(), 3, "width kept");
        assert_eq!(canvas.height(), 2, "height kept");
        assert_eq!(painted(&canvas, RED).len(), 6, "every pixel is background");
        let undersized = Canvas::new(0, 0, RED);
        assert_eq!(
            (undersized.width(), undersized.height()),
            (1, 1),
            "zero edges saturate up"
        );
    }

    #[test]
    fn set_writes_and_clips() {
        let mut canvas = square(4);
        canvas.set(1, 2, RED);
        assert_eq!(at(&canvas, 1, 2), RED, "in-bounds write lands");
        canvas.set(-1, 0, BLUE);
        canvas.set(0, -1, BLUE);
        canvas.set(4, 0, BLUE);
        canvas.set(0, 4, BLUE);
        assert!(
            painted(&canvas, BLUE).is_empty(),
            "off-canvas writes clip out"
        );
        assert_eq!(at(&canvas, 1, 2), RED, "the clipped writes touched nothing");
    }

    #[test]
    fn line_draws_both_endpoints_and_the_diagonal() {
        let mut dot = square(5);
        dot.line((1, 1), (1, 1), RED);
        assert_eq!(painted(&dot, RED), vec![(1_i64, 1_i64)], "a dot");
        let mut diagonal = square(5);
        diagonal.line((0, 0), (4, 4), RED);
        assert_eq!(
            painted(&diagonal, RED),
            vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            "exactly the diagonal, both endpoints included"
        );
        let mut vertical = square(5);
        vertical.line((4, 0), (4, 4), RED);
        assert_eq!(painted(&vertical, RED).len(), 5, "a full column");
        let mut reversed = square(5);
        reversed.line((4, 4), (0, 0), RED);
        assert_eq!(
            painted(&reversed, RED),
            painted(&diagonal, RED),
            "the walk ignores endpoint order"
        );
    }

    #[test]
    fn line_clips_to_the_canvas() {
        let mut crossing = square(4);
        crossing.line((-3, 1), (6, 1), RED);
        assert_eq!(
            painted(&crossing, RED),
            vec![(0_i64, 1_i64), (1, 1), (2, 1), (3, 1)],
            "only the crossing columns are drawn"
        );
        let mut missed = square(4);
        missed.line((-9, -9), (-1, -9), RED);
        missed.line((4, 0), (9, 5), RED);
        assert!(
            painted(&missed, RED).is_empty(),
            "fully outside draws nothing"
        );
    }

    #[test]
    fn thick_line_widens_axis_aligned_strokes() {
        let mut canvas = square(7);
        canvas.thick_line((3, 0), (3, 6), Stroke::new(3, RED));
        assert_eq!(painted(&canvas, RED).len(), 21, "three full columns");
        assert_eq!(at(&canvas, 2, 3), RED, "left of centre");
        assert_eq!(at(&canvas, 4, 3), RED, "right of centre");
        assert_eq!(at(&canvas, 1, 3), CLEAR, "one past the half width");
    }

    #[test]
    fn fill_rect_covers_the_inclusive_box_either_corner_order() {
        let mut canvas = square(6);
        canvas.fill_rect((4, 4), (1, 1), RED);
        assert_eq!(painted(&canvas, RED).len(), 16, "a 4x4 box");
        assert_eq!(at(&canvas, 1, 1), RED, "top-left corner");
        assert_eq!(at(&canvas, 4, 4), RED, "bottom-right corner");
        assert_eq!(at(&canvas, 5, 5), CLEAR, "outside the box");
    }

    #[test]
    fn fill_circle_covers_the_disc_and_clips() {
        let mut canvas = square(9);
        canvas.fill_circle((4, 4), 2, RED);
        assert_eq!(at(&canvas, 4, 4), RED, "centre");
        assert_eq!(at(&canvas, 6, 4), RED, "right edge of the disc");
        assert_eq!(at(&canvas, 2, 2), CLEAR, "corner outside the disc");
        let mut edge = square(4);
        edge.fill_circle((0, 0), 3, BLUE);
        assert_eq!(at(&edge, 3, 0), BLUE, "clipped row still filled");
        assert_eq!(at(&edge, 3, 3), CLEAR, "diagonal corner outside");
        let mut dot = square(3);
        dot.fill_circle((1, 1), 0, RED);
        assert_eq!(
            painted(&dot, RED),
            vec![(1_i64, 1_i64)],
            "radius 0 is the centre"
        );
    }

    #[test]
    fn polygon_fill_of_a_square_fills_the_interior() {
        let mut canvas = square(8);
        canvas.fill_polygon_evenodd(&box_edges(1, 1, 5, 5), RED);
        let interior = (1..5_i64)
            .flat_map(|row| (1..=5_i64).map(move |col| (col, row)))
            .collect::<Vec<Pt>>();
        let missed = interior
            .iter()
            .copied()
            .filter(|point| at(&canvas, point.0, point.1) != RED)
            .collect::<Vec<Pt>>();
        assert!(
            missed.is_empty(),
            "the interior is filled, misses {missed:?}"
        );
        assert_eq!(at(&canvas, 0, 3), CLEAR, "left of the box");
        assert_eq!(at(&canvas, 6, 3), CLEAR, "right of the box");
        assert_eq!(at(&canvas, 3, 5), CLEAR, "the half-open top row");
    }

    #[test]
    fn polygon_even_odd_rule_holes_an_inner_square() {
        let mut canvas = square(10);
        let mut edges = box_edges(1, 1, 8, 8);
        edges.extend(box_edges(4, 4, 6, 6));
        canvas.fill_polygon_evenodd(&edges, RED);
        assert_eq!(at(&canvas, 2, 2), RED, "outer ring is filled");
        assert_eq!(at(&canvas, 5, 5), CLEAR, "the hole stays clear");
        assert_eq!(at(&canvas, 7, 7), RED, "outer ring again");
    }

    #[test]
    fn blend_composites_alpha_over_the_background() {
        let mut canvas = Canvas::new(2, 1, RED);
        canvas.blend(0, 0, [0, 0, 255, 128]);
        let [red, _, blue, alpha] = at(&canvas, 0, 0);
        assert!(
            blue > red,
            "blue shows through the half alpha: {red} vs {blue}"
        );
        assert_eq!(alpha, 255, "an opaque background stays opaque");
        assert_eq!(
            at(&canvas, 1, 0),
            RED,
            "the untouched pixel keeps the background"
        );
        assert_eq!(
            blend_over([10, 20, 30, 255], [200, 200, 200, 255]),
            [200, 200, 200, 255],
            "a full-alpha source replaces"
        );
        assert_eq!(
            blend_over([10, 20, 30, 255], [200, 200, 200, 0]),
            [10, 20, 30, 255],
            "a zero-alpha source keeps the target"
        );
    }

    #[test]
    fn blend_polygon_tints_without_replacing() {
        let mut canvas = Canvas::new(8, 8, RED);
        canvas.blend_polygon_evenodd(&box_edges(1, 1, 5, 5), [0, 0, 255, 128]);
        let inside = at(&canvas, 3, 3);
        assert!(inside[2] > inside[0], "the tint blended over the red");
        assert_eq!(at(&canvas, 0, 0), RED, "outside untouched");
    }

    #[test]
    fn gamma_maps_the_colour_channels_only() {
        let mut canvas = Canvas::new(1, 1, [10, 20, 30, 40]);
        let table: Vec<u8> = (0_u8..=255_u8)
            .map(|value| value.saturating_add(1))
            .collect();
        canvas.apply_gamma(&table);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([11, 21, 31, 40]),
            "rgb lifted through the table, alpha kept"
        );
        canvas.apply_gamma(&[5_u8]);
        assert_eq!(
            canvas.pixel(0, 0),
            Some([11, 21, 31, 40]),
            "a table too short for a channel leaves it alone"
        );
    }

    #[test]
    fn to_rgba_image_copies_the_buffer_out() {
        let mut canvas = square(3);
        canvas.set(1, 1, RED);
        let image = canvas.to_rgba_image();
        assert_eq!((image.width, image.height), (3, 3), "shape kept");
        assert_eq!(image.get(1, 1), Some(RED), "pixel kept");
    }
}
