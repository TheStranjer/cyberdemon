//! Square-structuring-element morphology on [`Mask`]s: [`dilate`],
//! [`erode`] and [`close`].
//!
//! Every operation uses the `(2r + 1) x (2r + 1)` square and runs in
//! `O(width * height)` regardless of the radius through the shared
//! [`crate::gfx::filter::box_filter`] pass: a running count of the set bits
//! in each row's window, then in each column's window of the row results,
//! with [`crate::gfx::filter::window_span`] answering how many in-image
//! pixels each clipped window covers.
//!
//! Image borders follow the morphology convention the sketch pipeline
//! needs: outside the image counts as paper for [`dilate`] (so dilation
//! cannot grow ink from nowhere) and as ink for [`erode`] (so erosion
//! does not nibble ink at the border). Consequently [`close`] leaves an
//! ink band that already touches the frame intact while bridging breaks
//! up to `2r` pixels wide inside the image.

use crate::gfx::filter::{Sum, box_filter};
use crate::sketch::mask::Mask;

/// What one [`morph`] pass requires of a window: some set bit (dilation)
/// or only set bits (erosion), counted over the clipped window's `span`.
type Rule = fn(ink: u64, span: usize) -> bool;

/// The union rule: set where the window contains at least one set bit.
const fn holds_ink(ink: u64, _span: usize) -> bool {
    ink > 0
}

/// The intersection rule: set where every in-image window pixel is set.
fn holds_only_ink(ink: u64, span: usize) -> bool {
    u64::try_from(span).is_ok_and(|total| ink == total)
}

/// Grows the ink by radius `r`: a pixel is set when any pixel within
/// `r` (Chebyshev distance, square element) is set. Outside the image
/// counts as paper.
#[must_use]
#[inline]
pub fn dilate(mask: &Mask, r: u32) -> Mask {
    morph(mask, r, holds_ink)
}

/// Shrinks the ink by radius `r`: a pixel is set when every pixel within
/// `r` (Chebyshev distance, square element) is set. Outside the image
/// counts as ink, so border ink survives.
#[must_use]
#[inline]
pub fn erode(mask: &Mask, r: u32) -> Mask {
    morph(mask, r, holds_only_ink)
}

/// [`erode`] after [`dilate`] with the same radius: bridges breaks in ink
/// up to `2r` pixels wide without removing ink.
#[must_use]
#[inline]
pub fn close(mask: &Mask, r: u32) -> Mask {
    erode(&dilate(mask, r), r)
}

/// Runs the shared separable row-then-column window for one rule, folding
/// every window into the bit the rule asks for. A mask with no pixels
/// passes through unchanged.
fn morph(mask: &Mask, r: u32, rule: Rule) -> Mask {
    let radius = usize::try_from(r).unwrap_or(usize::MAX);
    let width = match usize::try_from(mask.width) {
        Ok(width) if width > 0 && !mask.bits.is_empty() => width,
        _ => return mask.clone(),
    };
    Mask {
        width: mask.width,
        height: mask.height,
        bits: box_filter(&mask.bits, width, radius, |window: &Sum, span| {
            rule(window.0, span)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{close, dilate, erode};
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

    /// A mask from rows of `0`/`1` text.
    fn from_rows(rows: &[&str]) -> Mask {
        let width = u32::try_from(rows.first().map_or(0, |row| row.len())).expect("fixture fits");
        let height = u32::try_from(rows.len()).expect("fixture fits");
        let bits: Vec<bool> = rows
            .iter()
            .flat_map(|row| row.chars().map(|letter| letter == '1'))
            .collect();
        Mask {
            width,
            height,
            bits,
        }
    }

    /// The `9 x 7` morphology fixture.
    const SEED: [&str; 7] = [
        "000000000",
        "011100000",
        "011101110",
        "011111110",
        "111101111",
        "011111110",
        "000011000",
    ];
    /// [`SEED`] dilated at radius `1`.
    const SEED_DILATE_ONE: [&str; 7] = [
        "111110000",
        "111111111",
        "111111111",
        "111111111",
        "111111111",
        "111111111",
        "111111111",
    ];
    /// [`SEED`] eroded at radius `1`.
    const SEED_ERODE_ONE: [&str; 7] = [
        "000000000",
        "000000000",
        "001000000",
        "001000100",
        "001000100",
        "000000000",
        "000000000",
    ];
    /// [`SEED`] dilated at radius `2`.
    const SEED_DILATE_TWO: [&str; 7] = ["111111111"; 7];
    /// [`SEED`] eroded at radius `2`.
    const SEED_ERODE_TWO: [&str; 7] = ["000000000"; 7];

    /// The `8 x 8` ring fixture: a filled plate with a one-pixel notch.
    const RING: [&str; 8] = [
        "00000000", "00111100", "01111110", "01111110", "01110110", "01111110", "00111100",
        "00000000",
    ];
    /// [`RING`] dilated at radius `1`.
    const RING_DILATE_ONE: [&str; 8] = [
        "01111110", "11111111", "11111111", "11111111", "11111111", "11111111", "11111111",
        "01111110",
    ];
    /// [`RING`] eroded at radius `1`.
    const RING_ERODE_ONE: [&str; 8] = [
        "00000000", "00000000", "00011000", "00100000", "00100000", "00000000", "00000000",
        "00000000",
    ];
    /// [`RING`] closed at radius `1`: the notch is bridged, the plate grows.
    const RING_CLOSE_ONE: [&str; 8] = [
        "00111100", "00111100", "11111111", "11111111", "11111111", "11111111", "00111100",
        "00111100",
    ];

    /// The `11 x 7` plate fixture, ink everywhere but the frame rows.
    const PLATE: [&str; 7] = [
        "00000000000",
        "01111111110",
        "01111111110",
        "01111111110",
        "01111111110",
        "01111111110",
        "00000000000",
    ];
    /// [`PLATE`] eroded at radius `1`.
    const PLATE_ERODE_ONE: [&str; 7] = [
        "00000000000",
        "00000000000",
        "00111111100",
        "00111111100",
        "00111111100",
        "00000000000",
        "00000000000",
    ];
    /// [`PLATE`] eroded at radius `2`.
    const PLATE_ERODE_TWO: [&str; 7] = [
        "00000000000",
        "00000000000",
        "00000000000",
        "00011111000",
        "00000000000",
        "00000000000",
        "00000000000",
    ];

    #[test]
    fn the_square_element_holds_its_golden_masks() {
        let seed = from_rows(&SEED);
        assert_eq!(
            dilate(&seed, 1).bits,
            from_rows(&SEED_DILATE_ONE).bits,
            "the seed dilated at radius 1"
        );
        assert_eq!(
            erode(&seed, 1).bits,
            from_rows(&SEED_ERODE_ONE).bits,
            "the seed eroded at radius 1"
        );
        assert_eq!(
            dilate(&seed, 2).bits,
            from_rows(&SEED_DILATE_TWO).bits,
            "the seed dilated at radius 2"
        );
        assert_eq!(
            erode(&seed, 2).bits,
            from_rows(&SEED_ERODE_TWO).bits,
            "the seed eroded at radius 2"
        );
        let ring = from_rows(&RING);
        assert_eq!(
            dilate(&ring, 1).bits,
            from_rows(&RING_DILATE_ONE).bits,
            "the ring dilated at radius 1"
        );
        assert_eq!(
            erode(&ring, 1).bits,
            from_rows(&RING_ERODE_ONE).bits,
            "the ring eroded at radius 1"
        );
        assert_eq!(
            close(&ring, 1).bits,
            from_rows(&RING_CLOSE_ONE).bits,
            "the ring closed at radius 1"
        );
        let plate = from_rows(&PLATE);
        assert_eq!(
            erode(&plate, 1).bits,
            from_rows(&PLATE_ERODE_ONE).bits,
            "the plate eroded at radius 1"
        );
        assert_eq!(
            erode(&plate, 2).bits,
            from_rows(&PLATE_ERODE_TWO).bits,
            "the plate eroded at radius 2"
        );
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
}
