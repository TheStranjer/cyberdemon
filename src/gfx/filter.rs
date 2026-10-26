//! The separable box window and the bilinear tap blend every image op shares.
//!
//! Two pieces of integer arithmetic the `gfx` samplers needed more than
//! once:
//!
//! * the clipped `(2r + 1)` sliding [`Window`] over a line ([`window_line`]),
//!   run along every row of a grid and then along every column of the values
//!   the row pass wrote ([`box_filter`]; [`box_sums`] is the same pass whose
//!   fold keeps the running total). [`crate::gfx::ops::blur`] averages its
//!   premultiplied channels with it, [`crate::gfx::ops::adaptive`] flattens a
//!   background with it and [`crate::sketch::morph`] counts set bits with it,
//!   so all three cost `O(width * height)` regardless of the radius.
//! * the two-tap bilinear kernel and the alpha-weighted blend it feeds
//!   ([`bilinear_kernel`], [`blend`] and [`bilinear`]), which
//!   [`crate::gfx::ops::resample`] scales through and
//!   [`crate::gfx::ops::perspective`] samples at a warped position.
//!
//! Border windows are clipped, never replicated: a window at an edge simply
//! holds fewer samples, and [`window_span`] answers that count so a caller can
//! divide by the area it really covered. Colours blend weighted by alpha, so
//! transparent pixels never darken an edge. Everything here is pure integer
//! code: no floats, no MCP, no archive I/O.

use crate::fixed::FRACBITS;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{pixel_at, rounded_div};
use crate::gfx::pixel_index;

/// One whole pixel in the `16.16` position and weight units the sampling
/// kernels use (`65536`).
pub const WEIGHT_ONE: u64 = 0x1_0000;
/// The fractional bits of a `16.16` sampling position.
const FRAC_MASK: i64 = 0xFFFF;

/// What a running [`window_line`] folds.
///
/// A window that one sample enters and another leaves as the walk slides one
/// step along the line. Every implementation is a sum, so a window's content
/// never depends on the order it was built in.
pub trait Window<S>: Clone + Default
where
    S: Copy,
{
    /// Folds one sample entering the window.
    fn enter(&mut self, sample: S);
    /// Takes one sample out of the window again.
    fn exit(&mut self, sample: S);
}

/// A [`Window`] of plain totals: the background sums of
/// [`crate::gfx::ops::adaptive`] and the set-bit counts of
/// [`crate::sketch::morph`]'s square element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sum(pub u64);

impl Window<u64> for Sum {
    #[inline]
    fn enter(&mut self, sample: u64) {
        self.0 = self.0.saturating_add(sample);
    }

    #[inline]
    fn exit(&mut self, sample: u64) {
        self.0 = self.0.saturating_sub(sample);
    }
}

impl Window<u8> for Sum {
    #[inline]
    fn enter(&mut self, sample: u8) {
        self.0 = self.0.saturating_add(u64::from(sample));
    }

    #[inline]
    fn exit(&mut self, sample: u8) {
        self.0 = self.0.saturating_sub(u64::from(sample));
    }
}

impl Window<bool> for Sum {
    #[inline]
    fn enter(&mut self, sample: bool) {
        if sample {
            self.0 = self.0.saturating_add(1_u64);
        }
    }

    #[inline]
    fn exit(&mut self, sample: bool) {
        if sample {
            self.0 = self.0.saturating_sub(1_u64);
        }
    }
}

/// How many samples the window at position `i` really covers in a line of
/// `len`: `2 * radius + 1` inside, fewer at the two borders, and the whole
/// line once the radius reaches past both of them.
#[must_use]
#[inline]
pub fn window_span(i: usize, radius: usize, len: usize) -> usize {
    let left = i.saturating_sub(radius);
    let right = i.saturating_add(radius).min(len.saturating_sub(1));
    right.saturating_sub(left).saturating_add(1)
}

/// For every position of `line`, its clipped window folded into `W`.
///
/// The window `[i - radius, i + radius]` is clipped to the line and carried
/// forward one sample at a time, so the pass costs `O(line length)` whatever
/// the radius; an empty line answers nothing.
#[must_use]
#[inline]
pub fn window_line<W, S>(line: &[S], radius: usize) -> Vec<W>
where
    W: Window<S>,
    S: Copy,
{
    let len = line.len();
    let mut windows = Vec::with_capacity(len);
    if len == 0 {
        return windows;
    }
    let reach = radius.min(len.saturating_sub(1));
    let mut window = W::default();
    for sample in line.iter().take(reach.saturating_add(1)) {
        window.enter(*sample);
    }
    for position in 0..len {
        windows.push(window.clone());
        slide(&mut window, line, position, reach);
    }
    windows
}

/// Moves a sliding window one step along `line`: the sample `reach` past the
/// new centre enters and the sample `reach` before it leaves, so the two stay
/// exactly `2 * reach + 1` apart.
fn slide<W, S>(window: &mut W, line: &[S], centre: usize, reach: usize)
where
    W: Window<S>,
    S: Copy,
{
    if let Some(next) = line.get(centre.saturating_add(reach).saturating_add(1)) {
        window.enter(*next);
    }
    if let Some(leaving) = centre.checked_sub(reach).and_then(|left| line.get(left)) {
        window.exit(*leaving);
    }
}

/// The separable box pass over a row-major grid `width` samples wide.
///
/// [`window_line`] runs along every row, each window folded by `fold` into one
/// value, and then along every column of *those* values with the same `fold`.
/// The vertical pass reads what the horizontal one wrote, so a caller that
/// decides or rounds between the passes keeps exactly the two-pass result it
/// computed by hand: [`crate::sketch::morph`] applies its set-bit rule,
/// [`crate::gfx::ops::blur`] its average.
///
/// `fold` receives one window and the [`window_span`] of the position it
/// belongs to. A zero width or an empty grid answers nothing, and a trailing
/// partial row is dropped the way `chunks_exact` drops it.
#[must_use]
#[inline]
pub fn box_filter<S, T, W, F>(values: &[S], width: usize, radius: usize, fold: F) -> Vec<T>
where
    W: Window<S> + Window<T>,
    S: Copy,
    T: Copy + Default,
    F: Fn(&W, usize) -> T,
{
    if width == 0 || values.is_empty() {
        return Vec::new();
    }
    let rows = row_pass(values, width, radius, &fold);
    column_pass(&rows, width, radius, &fold)
}

/// The horizontal half of [`box_filter`]: every row's windows, folded, in the
/// grid's own row-major order.
fn row_pass<S, T, W, F>(values: &[S], width: usize, radius: usize, fold: &F) -> Vec<T>
where
    W: Window<S>,
    S: Copy,
    F: Fn(&W, usize) -> T,
{
    values
        .chunks_exact(width)
        .flat_map(|line| fold_line(line, radius, fold))
        .collect()
}

/// The vertical half of [`box_filter`]: the row results taken down every
/// column, folded and laid back out in row-major order.
fn column_pass<T, W, F>(rows: &[T], width: usize, radius: usize, fold: &F) -> Vec<T>
where
    T: Copy + Default,
    W: Window<T>,
    F: Fn(&W, usize) -> T,
{
    let height = rows.len().checked_div(width).unwrap_or(0_usize);
    let mut grid = vec![T::default(); rows.len()];
    for x in 0..width {
        let line = column_of(rows, width, height, x).unwrap_or_default();
        write_column(&mut grid, width, x, &fold_line(&line, radius, fold));
    }
    grid
}

/// One line's box pass: its running windows, each folded with the count of
/// samples that position's clipped window covers.
fn fold_line<S, T, W, F>(line: &[S], radius: usize, fold: &F) -> Vec<T>
where
    W: Window<S>,
    S: Copy,
    F: Fn(&W, usize) -> T,
{
    window_line::<W, S>(line, radius)
        .iter()
        .enumerate()
        .map(|(position, window)| fold(window, window_span(position, radius, line.len())))
        .collect()
}

/// The samples of column `x` of a `width` by `height` row-major grid, top to
/// bottom, or `None` when the grid is too short to hold the whole column.
fn column_of<T>(values: &[T], width: usize, height: usize, x: usize) -> Option<Vec<T>>
where
    T: Copy,
{
    (0..height)
        .map(|y| {
            values
                .get(y.saturating_mul(width).saturating_add(x))
                .copied()
        })
        .collect()
}

/// Writes one column's values back into the row-major grid, ignoring rows
/// past its end.
fn write_column<T: Copy>(grid: &mut [T], width: usize, x: usize, column: &[T]) {
    for (y, value) in column.iter().copied().enumerate() {
        if let Some(slot) = grid.get_mut(y.saturating_mul(width).saturating_add(x)) {
            *slot = value;
        }
    }
}

/// The clipped `(2r + 1) x (2r + 1)` box sum of every grid position.
///
/// [`box_filter`] whose fold keeps the running total itself, so a window at a
/// border holds the samples it really covers and [`window_span`] answers the
/// count each sum divides by.
#[must_use]
#[inline]
pub fn box_sums<S>(values: &[S], width: usize, radius: usize) -> Vec<u64>
where
    Sum: Window<S>,
    S: Copy,
{
    box_filter(values, width, radius, |window: &Sum, _span| window.0)
}

/// One source pixel's contribution along one axis of a tap blend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tap {
    /// The source index on that axis.
    pub index: u32,
    /// Its weight, in the kernel's units.
    pub weight: u64,
}

/// The source pixels contributing along one axis of a tap blend, with the
/// total weight that divides their blend.
#[derive(Clone, Debug)]
pub struct Kernel {
    /// The taps, in increasing source-index order.
    pub taps: Vec<Tap>,
    /// The sum of the tap weights.
    pub total: u64,
}

/// The `16.16` centre of the last pixel of an edge `length` wide.
///
/// The highest sampling position that still sits over the picture; saturating
/// for an edge too wide to shift.
#[must_use]
#[inline]
pub fn last_centre(length: u32) -> i64 {
    i64::from(length.saturating_sub(1))
        .checked_shl(FRACBITS)
        .unwrap_or(i64::MAX)
}

/// The two bilinear taps of the `16.16` `position` on an edge of `limit`.
///
/// The centre at or below the position and the next one up, weighted by how
/// far the position sits between the two. The weights always add up to
/// [`WEIGHT_ONE`], the upper tap never passes the edge's last pixel, and a
/// position left of the first centre reads as the first pixel alone.
#[must_use]
#[inline]
pub fn bilinear_kernel(position: i64, limit: u32) -> Kernel {
    let whole = position.max(0);
    let low = whole
        .checked_shr(FRACBITS)
        .and_then(|shifted| u32::try_from(shifted).ok())
        .unwrap_or(0_u32);
    let high = low.saturating_add(1).min(limit.saturating_sub(1));
    let fraction = u64::try_from(whole & FRAC_MASK).unwrap_or(0_u64);
    Kernel {
        taps: vec![
            Tap {
                index: low,
                weight: WEIGHT_ONE.saturating_sub(fraction),
            },
            Tap {
                index: high,
                weight: fraction,
            },
        ],
        total: WEIGHT_ONE,
    }
}

/// The bilinear, alpha-weighted sample of `img` at `(x, y)`, in `16.16`.
///
/// `None` when the position lies outside the picture's pixel centres.
#[must_use]
#[inline]
pub fn bilinear(img: &RgbaImage, x: i64, y: i64) -> Option<[u8; 4]> {
    if x < 0 || y < 0 || x > last_centre(img.width) || y > last_centre(img.height) {
        return None;
    }
    let columns = bilinear_kernel(x, img.width);
    let rows = bilinear_kernel(y, img.height);
    Some(blend(img, &rows, &columns))
}

/// Blends the source pixels the `rows` x `columns` kernel pair reaches.
///
/// Colours are weighted by their alpha and by both axes' weights, alpha by the
/// axis weights alone divided by the whole footprint. A tap off the buffer
/// reads as transparent black.
#[must_use]
#[inline]
pub fn blend(img: &RgbaImage, rows: &Kernel, columns: &Kernel) -> [u8; 4] {
    let mut channels = [0_u64; 3];
    let mut premultiplied = 0_u64;
    for row in &rows.taps {
        for column in &columns.taps {
            let position = pixel_index(img.width, column.index, row.index).unwrap_or(usize::MAX);
            let pixel = pixel_at(&img.pixels, position).unwrap_or([0_u8; 4]);
            let weight = row.weight.saturating_mul(column.weight);
            accumulate(&mut channels, &mut premultiplied, pixel, weight);
        }
    }
    finish_pixel(
        channels,
        premultiplied,
        rows.total.saturating_mul(columns.total),
    )
}

/// Adds one source pixel to a blend: colours weighted by alpha and the tap
/// weight, alpha weighted by the tap weight alone.
fn accumulate(channels: &mut [u64; 3], premultiplied: &mut u64, pixel: [u8; 4], weight: u64) {
    let [red, green, blue, alpha] = pixel;
    let scaled = weight.saturating_mul(u64::from(alpha));
    for (channel, value) in channels.iter_mut().zip([red, green, blue]) {
        *channel = channel.saturating_add(u64::from(value).saturating_mul(scaled));
    }
    *premultiplied = premultiplied.saturating_add(scaled);
}

/// Turns a blend's accumulators into a pixel: alpha over the whole footprint,
/// colour over the covered alpha; a fully transparent result gets colour `0`.
fn finish_pixel(channels: [u64; 3], premultiplied: u64, footprint: u64) -> [u8; 4] {
    let alpha = u8::try_from(rounded_div(premultiplied, footprint)).unwrap_or(255_u8);
    let color: [u8; 3] = if premultiplied == 0 {
        [0_u8; 3]
    } else {
        channels.map(|sum| u8::try_from(rounded_div(sum, premultiplied)).unwrap_or(255_u8))
    };
    let [red, green, blue] = color;
    [red, green, blue, alpha]
}

#[cfg(test)]
mod tests;
