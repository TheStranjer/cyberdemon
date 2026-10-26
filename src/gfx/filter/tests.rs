//! The shared window's and the shared sampler's checks: the running totals
//! against freshly summed windows, the separable passes against a naive
//! double loop, and the bilinear kernel, blend and sample against hand-worked
//! pixels.

use super::{
    Kernel, Sum, Tap, WEIGHT_ONE, Window, bilinear, bilinear_kernel, blend, box_filter, box_sums,
    last_centre, window_line, window_span,
};
use crate::gfx::image::RgbaImage;

/// Half of [`WEIGHT_ONE`]: the weight of a position exactly between two
/// pixel centres, and three quarters of it besides.
const HALF: u64 = 0x8000;
/// Three quarters of [`WEIGHT_ONE`].
const THREE_QUARTERS: u64 = 0xC000;
/// One quarter of [`WEIGHT_ONE`].
const QUARTER: u64 = 0x4000;

/// The row stride the grid tests share.
const WIDTH: usize = 5_usize;
/// The row count the grid tests share.
const HEIGHT: usize = 4_usize;

/// The grid's value at `(x, y)`: `x + 2 * y`, stepped up on every third
/// column so no window sum comes out round.
fn grid_value(x: usize, y: usize) -> u64 {
    let doubled = y.checked_mul(2).unwrap_or(0);
    let stepped = if x.checked_rem(3).unwrap_or(0) == 0 {
        5
    } else {
        0
    };
    let value = x
        .checked_add(doubled)
        .and_then(|sum| sum.checked_add(stepped))
        .unwrap_or(0);
    u64::try_from(value).unwrap_or(0_u64)
}

/// A [`WIDTH`] x [`HEIGHT`] grid of [`grid_value`]s, row-major.
fn grid() -> Vec<u64> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| grid_value(x, y)))
        .collect()
}

/// The same grid as set bits: ink wherever a value is not a multiple of
/// three, so the two rules split the picture in a visible way.
fn bits() -> Vec<bool> {
    grid()
        .iter()
        .map(|&value| value.checked_rem(3).unwrap_or(0) > 0)
        .collect()
}

/// The row-major index of `(x, y)` in a [`WIDTH`]-wide grid.
fn cell(x: usize, y: usize) -> usize {
    y.checked_mul(WIDTH)
        .and_then(|row| row.checked_add(x))
        .unwrap_or(0)
}

/// A freshly summed window `[i - radius, i + radius]`, clipped to `line`.
fn fresh_sum(line: &[u64], i: usize, radius: usize) -> u64 {
    line.iter()
        .enumerate()
        .filter(|&(position, _)| position.abs_diff(i) <= radius)
        .map(|(_, &value)| value)
        .sum()
}

/// The clipped `(2r + 1) x (2r + 1)` box sum at `(x, y)`, straight from the
/// definition: the reference the separable pass must reproduce.
fn fresh_box(grid: &[u64], x: usize, y: usize, radius: usize) -> u64 {
    let columns = covered(x, radius, WIDTH);
    covered(y, radius, HEIGHT)
        .iter()
        .flat_map(|&row| columns.iter().map(move |&column| (row, column)))
        .map(|(row, column)| grid.get(cell(column, row)).copied().unwrap_or(0))
        .sum()
}

/// The in-grid positions along an axis of `len` that a window centred on
/// `centre` covers.
fn covered(centre: usize, radius: usize, len: usize) -> Vec<usize> {
    (0..len)
        .filter(|&position| position.abs_diff(centre) <= radius)
        .collect()
}

/// The rule one [`crate::sketch::morph`]-style pass applies to a window: set
/// when it holds some ink, or only ink.
fn decide(ink: usize, span: usize, some: bool) -> bool {
    if some { ink > 0 } else { ink == span }
}

/// The rule over one line, from the definition: every position's clipped
/// window counted and decided.
fn line_rule(line: &[bool], radius: usize, some: bool) -> Vec<bool> {
    (0..line.len())
        .map(|position| {
            let span = window_span(position, radius, line.len());
            let window: Vec<bool> = line
                .iter()
                .skip(position.saturating_sub(radius))
                .take(span)
                .copied()
                .collect();
            decide(
                window.iter().filter(|set| **set).count(),
                window.len(),
                some,
            )
        })
        .collect()
}

/// Column `x` of a row-major [`WIDTH`]-wide boolean grid, top to bottom.
fn column_of_bits(values: &[bool], x: usize) -> Vec<bool> {
    values
        .chunks_exact(WIDTH)
        .map(|row| row.get(x).copied().unwrap_or_default())
        .collect()
}

/// The morphological pass straight from the definition: every row's clipped
/// window counted and decided, then the same down every column of the
/// decided bits.
fn naive_pass(bits: &[bool], radius: usize, some: bool) -> Vec<bool> {
    let middle: Vec<bool> = bits
        .chunks_exact(WIDTH)
        .flat_map(|row| line_rule(row, radius, some))
        .collect();
    let down: Vec<Vec<bool>> = (0..WIDTH)
        .map(|x| line_rule(&column_of_bits(&middle, x), radius, some))
        .collect();
    (0..HEIGHT)
        .flat_map(|y| {
            down.iter()
                .map(move |column| column.get(y).copied().unwrap_or_default())
        })
        .collect()
}

/// `line`'s running window totals at `radius`.
fn totals(line: &[u64], radius: usize) -> Vec<u64> {
    window_line::<Sum, u64>(line, radius)
        .iter()
        .map(|window| window.0)
        .collect()
}

/// `line`'s running window bit counts at `radius`.
fn bit_counts(line: &[bool], radius: usize) -> Vec<u64> {
    window_line::<Sum, bool>(line, radius)
        .iter()
        .map(|window| window.0)
        .collect()
}

#[test]
fn window_spans_clamp_at_both_borders() {
    assert_eq!(window_span(0, 1, 5), 2, "clipped at the left edge");
    assert_eq!(window_span(2, 1, 5), 3, "full window");
    assert_eq!(window_span(4, 1, 5), 2, "clipped at the right edge");
    assert_eq!(window_span(1, 9, 3), 3, "a huge radius covers the line");
}

#[test]
fn running_sums_match_a_fresh_window_everywhere() {
    let line = [5_u64, 250, 30, 100, 7];
    for radius in 0_usize..6 {
        let expected: Vec<u64> = (0..line.len())
            .map(|position| fresh_sum(&line, position, radius))
            .collect();
        assert_eq!(
            totals(&line, radius),
            expected,
            "radius {radius} matches a fresh clipped window at every position"
        );
    }
    let empty: Vec<u64> = Vec::new();
    assert!(
        window_line::<Sum, u64>(&empty, 2).is_empty(),
        "an empty line answers nothing"
    );
}

#[test]
fn a_wider_radius_than_the_line_keeps_the_whole_line_everywhere() {
    let line = [7_u64, 9, 4];
    let whole = line.iter().sum::<u64>();
    for radius in [2_usize, 3, 64, usize::MAX] {
        assert_eq!(totals(&line, radius), vec![whole; 3], "radius {radius}");
    }
}

#[test]
fn the_same_window_folds_bytes_and_set_bits() {
    let bytes = [200_u8, 0, 255, 30];
    let widened: Vec<u64> = bytes.iter().map(|&value| u64::from(value)).collect();
    let as_bytes: Vec<u64> = window_line::<Sum, u8>(&bytes, 1)
        .iter()
        .map(|window| window.0)
        .collect();
    assert_eq!(as_bytes, totals(&widened, 1), "a byte enters as its width");
    let line = [true, false, true, true, false];
    assert_eq!(
        bit_counts(&line, 1),
        vec![1_u64, 2, 2, 2, 1],
        "a bool window counts the bits it covers"
    );
    assert_eq!(
        bit_counts(&line, 0),
        vec![1_u64, 0, 1, 1, 0],
        "radius 0 is the line itself"
    );
    assert_eq!(
        bit_counts(&line, 10),
        vec![3_u64; 5],
        "a huge radius covers the whole line"
    );
}

#[test]
fn entering_and_leaving_a_window_round_trips() {
    let mut window = Sum::default();
    Window::enter(&mut window, 40_u64);
    Window::enter(&mut window, 2_u8);
    Window::enter(&mut window, true);
    assert_eq!(window, Sum(43), "bytes, numbers and bits all add up");
    Window::exit(&mut window, 3_u8);
    Window::exit(&mut window, false);
    assert_eq!(window, Sum(40), "and all take away again");
    Window::exit(&mut window, u64::MAX);
    assert_eq!(window, Sum(0), "a total never drops below zero");
}

#[test]
fn box_sums_answer_the_clipped_window_area_everywhere() {
    let values = grid();
    for radius in 1_usize..4 {
        let sums = box_sums(&values, WIDTH, radius);
        assert_eq!(sums.len(), values.len(), "radius {radius} keeps the shape");
        for (position, &got) in sums.iter().enumerate() {
            let x = position.checked_rem(WIDTH).unwrap_or(0);
            let y = position.checked_div(WIDTH).unwrap_or(0);
            assert_eq!(
                got,
                fresh_box(&values, x, y, radius),
                "the window at ({x}, {y}) at radius {radius}"
            );
        }
    }
}

#[test]
fn box_filter_folds_between_the_two_passes() {
    let ink = bits();
    for (some, name) in [(true, "union"), (false, "intersection")] {
        let folded = box_filter(&ink, WIDTH, 1, |window: &Sum, span| {
            decide(usize::try_from(window.0).unwrap_or(0), span, some)
        });
        assert_eq!(
            folded,
            naive_pass(&ink, 1, some),
            "the {name} rule over a row pass and a column pass"
        );
    }
}

#[test]
fn a_grid_without_rows_or_without_width_folds_to_nothing() {
    let empty: Vec<u64> = Vec::new();
    assert!(box_sums(&empty, WIDTH, 1).is_empty(), "no samples");
    let one_row = [3_u64, 4, 5];
    assert!(
        box_sums(&one_row, 0, 1).is_empty(),
        "a zero width answers nothing"
    );
    assert_eq!(
        box_sums(&one_row, 3, 0),
        one_row.to_vec(),
        "radius 0 is the grid itself"
    );
    assert_eq!(
        box_sums(&one_row, 4, 1),
        Vec::<u64>::new(),
        "a partial row is dropped"
    );
}

#[test]
fn the_bilinear_kernel_taps_between_pixel_centres() {
    let mid = bilinear_kernel(0x0001_8000, 8);
    assert_eq!(
        mid.taps,
        vec![
            Tap {
                index: 1,
                weight: HALF,
            },
            Tap {
                index: 2,
                weight: HALF,
            },
        ],
        "halfway between pixel centres"
    );
    assert_eq!(mid.total, WEIGHT_ONE, "the weights are in 16.16 units");
    let first = bilinear_kernel(0, 4);
    assert_eq!(
        first.taps,
        vec![
            Tap {
                index: 0,
                weight: WEIGHT_ONE,
            },
            Tap {
                index: 1,
                weight: 0,
            },
        ],
        "the first centre is the first pixel alone"
    );
}

#[test]
fn the_bilinear_kernel_clamps_its_upper_tap_at_the_last_centre() {
    let edge = bilinear_kernel(last_centre(8), 8);
    assert_eq!(
        edge.taps,
        vec![
            Tap {
                index: 7,
                weight: WEIGHT_ONE,
            },
            Tap {
                index: 7,
                weight: 0,
            },
        ],
        "the last centre clamps both taps in place"
    );
    assert_eq!(
        edge.taps.iter().map(|tap| tap.weight).sum::<u64>(),
        edge.total,
        "tap weights sum to the total"
    );
    assert_eq!(
        bilinear_kernel(0x0002_4000, 3).taps,
        vec![
            Tap {
                index: 2,
                weight: THREE_QUARTERS,
            },
            Tap {
                index: 2,
                weight: QUARTER,
            },
        ],
        "a position past the last centre keeps its taps on the last pixel"
    );
}

#[test]
fn a_position_left_of_the_first_centre_sits_on_the_first_pixel() {
    assert_eq!(
        bilinear_kernel(-0x0001_0000, 4).taps,
        vec![
            Tap {
                index: 0,
                weight: WEIGHT_ONE,
            },
            Tap {
                index: 1,
                weight: 0,
            },
        ],
        "the whole-pixel overshoot left of the edge falls back to pixel 0"
    );
    assert_eq!(
        bilinear_kernel(-1, 4).taps,
        bilinear_kernel(0, 4).taps,
        "and so does a one-unit overshoot"
    );
}

#[test]
fn an_edge_centre_is_the_middle_of_a_one_pixel_edge() {
    assert_eq!(last_centre(1), 0, "a one-pixel edge centres on zero");
    assert_eq!(last_centre(0), 0, "an empty edge has no centre");
    assert_eq!(last_centre(3), 0x0002_0000, "the third pixel sits at 2.0");
}

/// A 2x1 image: opaque black on the left, opaque white on the right.
fn black_white() -> RgbaImage {
    RgbaImage::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).expect("2x1 is valid")
}

/// A 2x1 image: an opaque red pixel next to a fully transparent one.
fn red_transparent() -> RgbaImage {
    RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).expect("2x1 is valid")
}

#[test]
fn bilinear_reads_a_pixel_centre_exactly_and_the_middle_between_two() {
    let picture = black_white();
    assert_eq!(
        bilinear(&picture, 0, 0),
        Some([0, 0, 0, 255]),
        "the left centre"
    );
    assert_eq!(
        bilinear(&picture, 0x0001_0000, 0),
        Some([255, 255, 255, 255]),
        "the right centre"
    );
    assert_eq!(
        bilinear(&picture, i64::try_from(HALF).unwrap_or(0), 0),
        Some([128, 128, 128, 255]),
        "the middle of an opaque ramp: 65025 / 510 rounds to 128"
    );
}

#[test]
fn bilinear_outside_the_picture_answers_none() {
    let picture = black_white();
    let last = last_centre(picture.width);
    let outside = last.saturating_add(1);
    assert_eq!(bilinear(&picture, -1, 0), None, "left of the first centre");
    assert_eq!(bilinear(&picture, 0, -1), None, "above the only row");
    assert_eq!(
        bilinear(&picture, outside, 0),
        None,
        "past the last centre on the row"
    );
    assert_eq!(
        bilinear(&picture, 0, last_centre(picture.height).saturating_add(1)),
        None,
        "past the only row's centre"
    );
    assert_eq!(
        bilinear(&picture, last, 0),
        Some([255, 255, 255, 255]),
        "the far centre stands"
    );
}

#[test]
fn the_blend_weights_colours_by_alpha_and_alpha_by_the_footprint() {
    let picture = red_transparent();
    let columns = bilinear_kernel(0x0000_8000, 2);
    let rows = bilinear_kernel(0, 1);
    let blended = blend(&picture, &rows, &columns);
    assert_eq!(blended[0], 255, "the red keeps its purity");
    assert_eq!([blended[1], blended[2]], [0, 0], "and nothing else appears");
    assert_eq!(
        blended[3], 128,
        "half the footprint carries the red's alpha"
    );
    let empty = Kernel {
        taps: Vec::new(),
        total: WEIGHT_ONE,
    };
    assert_eq!(
        blend(&picture, &empty, &columns),
        [0, 0, 0, 0],
        "a kernel with no taps blends nothing"
    );
}
