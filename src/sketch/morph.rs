//! Square-structuring-element morphology on [`Mask`]s: [`dilate`],
//! [`erode`] and [`close`].
//!
//! Every operation uses the `(2r + 1) x (2r + 1)` square and runs in
//! `O(width * height)` regardless of the radius: a separable two-pass
//! scan keeps a running count of set bits in each row's window, then in
//! each column's window of the row result.
//!
//! Image borders follow the morphology convention the sketch pipeline
//! needs: outside the image counts as paper for [`dilate`] (so dilation
//! cannot grow ink from nowhere) and as ink for [`erode`] (so erosion
//! does not nibble ink at the border). Consequently [`close`] leaves an
//! ink band that already touches the frame intact while bridging breaks
//! up to `2r` pixels wide inside the image.

use crate::sketch::mask::Mask;

/// What one [`morph`] pass requires of a window: some set bit (dilation)
/// or only set bits (erosion).
type Rule = fn(count: usize, span: usize) -> bool;

/// The union rule: set where the window contains at least one set bit.
const fn some(count: usize, _span: usize) -> bool {
    count > 0
}

/// The intersection rule: set where every in-image window pixel is set.
const fn every(count: usize, span: usize) -> bool {
    count == span
}

/// Grows the ink by radius `r`: a pixel is set when any pixel within
/// `r` (Chebyshev distance, square element) is set. Outside the image
/// counts as paper.
#[must_use]
#[inline]
pub fn dilate(mask: &Mask, r: u32) -> Mask {
    morph(mask, r, some)
}

/// Shrinks the ink by radius `r`: a pixel is set when every pixel within
/// `r` (Chebyshev distance, square element) is set. Outside the image
/// counts as ink, so border ink survives.
#[must_use]
#[inline]
pub fn erode(mask: &Mask, r: u32) -> Mask {
    morph(mask, r, every)
}

/// [`erode`] after [`dilate`] with the same radius: bridges breaks in ink
/// up to `2r` pixels wide without removing ink.
#[must_use]
#[inline]
pub fn close(mask: &Mask, r: u32) -> Mask {
    erode(&dilate(mask, r), r)
}

/// Runs the separable row-then-column scan for one rule. A mask with no
/// pixels passes through unchanged.
fn morph(mask: &Mask, r: u32, rule: Rule) -> Mask {
    let radius = usize::try_from(r).unwrap_or(usize::MAX);
    let width = match usize::try_from(mask.width) {
        Ok(width) if width > 0 && !mask.bits.is_empty() => width,
        _ => return mask.clone(),
    };
    let rows_done = scan_rows(mask, width, radius, rule);
    scan_columns(&rows_done, width, radius, rule)
}

/// Applies the row pass: window counts along each row, rule per pixel.
fn scan_rows(mask: &Mask, width: usize, radius: usize, rule: Rule) -> Mask {
    let mut bits = Vec::with_capacity(mask.bits.len());
    for row in mask.bits.chunks_exact(width) {
        let counts = window_counts(row, radius);
        for (position, &count) in counts.iter().enumerate() {
            bits.push(rule(count, window_span(position, radius, row.len())));
        }
    }
    Mask {
        width: mask.width,
        height: mask.height,
        bits,
    }
}

/// Applies the column pass: window counts down each column, rule per
/// pixel, writing into the same row-major layout.
fn scan_columns(mask: &Mask, width: usize, radius: usize, rule: Rule) -> Mask {
    let height = match mask.bits.len().checked_div(width) {
        Some(height) if height > 0 => height,
        _ => return mask.clone(),
    };
    let mut bits = vec![false; mask.bits.len()];
    for x in 0..width {
        let counts = window_counts(&column_of(mask, width, height, x), radius);
        for (y, &count) in counts.iter().enumerate() {
            let kept = rule(count, window_span(y, radius, height));
            write_kept(&mut bits, width, (x, y), kept);
        }
    }
    Mask {
        width: mask.width,
        height: u32::try_from(height).unwrap_or(mask.height),
        bits,
    }
}

/// The pixels of column `x`, top to bottom.
fn column_of(mask: &Mask, width: usize, height: usize, x: usize) -> Vec<bool> {
    (0..height)
        .map(|y| {
            mask.bits
                .get(y.saturating_mul(width).saturating_add(x))
                .copied()
                .unwrap_or_default()
        })
        .collect()
}

/// Writes one kept bit at row-major `(x, y)`, ignoring out-of-bounds.
fn write_kept(bits: &mut [bool], width: usize, (x, y): (usize, usize), kept: bool) {
    if let Some(slot) = bits.get_mut(y.saturating_mul(width).saturating_add(x)) {
        *slot = kept;
    }
}

/// For every position of `line`, the number of set bits in the window
/// `[i - r, i + r]` clipped to the line, via a running count.
fn window_counts(line: &[bool], radius: usize) -> Vec<usize> {
    let len = line.len();
    let mut counts = Vec::with_capacity(len);
    if len == 0 {
        return counts;
    }
    let r = radius.min(len.saturating_sub(1));
    let mut running = line
        .iter()
        .take(r.saturating_add(1))
        .filter(|bit| **bit)
        .count();
    for i in 0..len {
        counts.push(running);
        if line
            .get(i.saturating_add(r).saturating_add(1))
            .copied()
            .unwrap_or_default()
        {
            running = running.saturating_add(1);
        }
        if i >= r && line.get(i.saturating_sub(r)).copied().unwrap_or_default() {
            running = running.saturating_sub(1);
        }
    }
    counts
}

/// The number of in-line window positions at `i`, i.e. the count a fully
/// set window reaches there.
fn window_span(i: usize, radius: usize, len: usize) -> usize {
    let left = i.saturating_sub(radius);
    let right = i.saturating_add(radius).min(len.saturating_sub(1));
    right.saturating_sub(left).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::{close, dilate, erode, window_counts, window_span};
    use crate::sketch::mask::Mask;

    /// A `width` x 1 mask with the given column bits.
    fn row(bits: &[bool]) -> Mask {
        let width = u32::try_from(bits.len()).expect("fixture fits");
        let mut mask = Mask::blank(width, 1);
        for (position, bit) in bits.iter().enumerate() {
            let x = u32::try_from(position).expect("fixture fits");
            assert!(mask.set(x, 0, *bit), "fixture in bounds");
        }
        mask
    }

    /// A 5x5 mask with only `(2, 2)` set.
    fn single() -> Mask {
        let mut mask = Mask::blank(5, 5);
        assert!(mask.set(2, 2, true), "in bounds");
        mask
    }

    /// A 4x4 mask with the top row and left column set.
    fn l_band() -> Mask {
        let mut mask = Mask::blank(4, 4);
        for (x, y) in [(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (0, 2), (0, 3)] {
            assert!(mask.set(x, y, true), "fixture in bounds");
        }
        mask
    }

    /// The number of set bits in a mask.
    fn ink_count(mask: &Mask) -> usize {
        mask.bits.iter().filter(|&&bit| bit).count()
    }

    #[test]
    fn dilate_grows_a_square_and_stops_at_the_border() {
        let grown = dilate(&single(), 1);
        assert_eq!(
            grown.get(1, 1),
            Some(true),
            "diagonal corners of the element grow"
        );
        assert_eq!(grown.get(3, 3), Some(true), "the other corner grows");
        assert_eq!(grown.get(0, 2), Some(false), "distance 2 stays paper");
        let mut corner = Mask::blank(5, 5);
        assert!(corner.set(0, 0, true), "in bounds");
        let corner_grown = dilate(&corner, 1);
        assert_eq!(
            ink_count(&corner_grown),
            4,
            "outside counts as paper: the corner grows to a 2x2 block"
        );
        assert_eq!(corner_grown.get(0, 0), Some(true), "the seed stays");
        assert_eq!(corner_grown.get(1, 1), Some(true), "inside diagonal grows");
    }

    #[test]
    fn erode_needs_the_whole_window_inside_and_keeps_the_border() {
        let shrunk = erode(&dilate(&single(), 1), 1);
        assert_eq!(ink_count(&shrunk), 1, "erode(dilate) is the single pixel");
        assert_eq!(shrunk.get(2, 2), Some(true), "and it is the seed");
        let solid = dilate(&single(), 4);
        assert_eq!(ink_count(&solid), 25, "radius 4 fills the 5x5");
        let eaten = erode(&solid, 1);
        assert_eq!(
            eaten.get(0, 0),
            Some(true),
            "outside the image counts as ink, so frame ink survives"
        );
        assert_eq!(eaten.get(2, 2), Some(true), "the middle survives too");
        let mut band = Mask::blank(3, 3);
        for y in 0..3 {
            assert!(band.set(0, y, true), "in bounds");
        }
        let gone = erode(&band, 1);
        assert_eq!(
            ink_count(&gone),
            0,
            "a 1-px band erodes away even at the border: its inner neighbours are paper"
        );
    }

    #[test]
    fn close_bridges_breaks_up_to_twice_the_radius() {
        let bridged = close(&row(&[true, true, false, true, true]), 1);
        assert_eq!(
            bridged.bits,
            vec![true; 5],
            "a 1-px break is filled, the frame stays ink"
        );
        let broken = close(&row(&[true, true, false, false, false, true, true]), 1);
        assert_eq!(broken.get(2, 0), Some(false), "a 3-px break exceeds 2r = 2");
        let kept = close(&l_band(), 2);
        assert_eq!(
            kept.get(0, 0),
            Some(true),
            "frame corner ink survives closing"
        );
        assert_eq!(
            kept.get(3, 0),
            Some(true),
            "the far end of the band survives"
        );
        assert_eq!(
            kept.get(2, 1),
            Some(false),
            "the notch is wider than the element can span"
        );
    }

    #[test]
    fn zero_radius_is_the_identity() {
        let mask = single();
        assert_eq!(dilate(&mask, 0).bits, mask.bits, "dilate(_, 0)");
        assert_eq!(erode(&mask, 0).bits, mask.bits, "erode(_, 0)");
        assert_eq!(close(&mask, 0).bits, mask.bits, "close(_, 0)");
        let empty = Mask::blank(0, 0);
        assert_eq!(
            dilate(&empty, 3).bits,
            empty.bits,
            "empty mask passes through"
        );
    }

    #[test]
    fn running_window_counts_match_the_definition() {
        let line = [true, false, true, true, false];
        assert_eq!(
            window_counts(&line, 1),
            vec![1, 2, 2, 2, 1],
            "window sizes follow the clipped square"
        );
        assert_eq!(
            window_counts(&line, 0),
            vec![1, 0, 1, 1, 0],
            "radius 0 is the line"
        );
        assert_eq!(window_counts(&[], 2), Vec::<usize>::new(), "empty line");
        assert_eq!(
            window_counts(&line, 10),
            vec![3; 5],
            "a huge radius covers the whole line everywhere"
        );
        assert_eq!(window_span(0, 1, 5), 2, "clipped at the left edge");
        assert_eq!(window_span(2, 1, 5), 3, "full window");
        assert_eq!(window_span(4, 1, 5), 2, "clipped at the right edge");
    }
}
