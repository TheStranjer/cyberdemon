//! The 5x7 bitmap font every render draws text with.
//!
//! A pure table of [`Glyph`]s, seven row bitmasks of five pixels per
//! character, covering digits, capital letters, space and the punctuation
//! `- _ . : # ( ) / ? !`. [`draw_text`] centres a string on an anchor
//! point, magnifies it by an integer scale, and paints a one-pixel
//! outline under the fill so labels stay legible on any background.
//! Lower-case input draws upper-case; characters the table lacks draw as
//! `?`. Nothing here knows about maps or tools: callers map their own
//! coordinates into pixels and pick the colours.

use crate::render::canvas::{Canvas, Pt, Rgba};

/// Pixels across one glyph.
pub const GLYPH_WIDTH: usize = 5;
/// Pixels down one glyph.
pub const GLYPH_HEIGHT: usize = 7;
/// Blank pixels kept between neighbouring glyphs.
pub const LETTER_GAP: usize = 1;
/// One glyph plus its gap: the columns advanced per character.
const GLYPH_STEP: usize = GLYPH_WIDTH + LETTER_GAP;

/// One glyph: [`GLYPH_HEIGHT`] row bitmasks, bit `4` the leftmost pixel.
pub type Glyph = [u8; GLYPH_HEIGHT];

/// The five column bitmasks of a [`Glyph`], left to right.
const COLUMN_BITS: [u8; GLYPH_WIDTH] = [16_u8, 8_u8, 4_u8, 2_u8, 1_u8];

/// The blank every fallback lands on and the shape space draws as.
const SPACE: Glyph = [0_u8; GLYPH_HEIGHT];

/// The whole table: each supported character with its seven rows.
const GLYPHS: [(char, Glyph); 47] = [
    (' ', SPACE),
    ('!', [4, 4, 4, 4, 4, 0, 4]),
    ('#', [10, 31, 10, 10, 31, 10, 0]),
    ('(', [2, 4, 8, 8, 8, 4, 2]),
    (')', [8, 4, 2, 2, 2, 4, 8]),
    ('-', [0, 0, 0, 31, 0, 0, 0]),
    ('.', [0, 0, 0, 0, 0, 12, 12]),
    ('/', [1, 2, 2, 4, 8, 8, 16]),
    (':', [0, 12, 12, 0, 12, 12, 0]),
    ('?', [14, 17, 1, 2, 4, 0, 4]),
    ('_', [0, 0, 0, 0, 0, 0, 31]),
    ('0', [14, 17, 19, 21, 25, 17, 14]),
    ('1', [4, 12, 4, 4, 4, 4, 14]),
    ('2', [14, 17, 1, 2, 4, 8, 31]),
    ('3', [31, 2, 4, 2, 1, 17, 14]),
    ('4', [2, 6, 10, 18, 31, 2, 2]),
    ('5', [31, 16, 30, 1, 1, 17, 14]),
    ('6', [6, 8, 16, 30, 17, 17, 14]),
    ('7', [31, 1, 2, 4, 8, 8, 8]),
    ('8', [14, 17, 17, 14, 17, 17, 14]),
    ('9', [14, 17, 17, 15, 1, 2, 12]),
    ('A', [14, 17, 17, 31, 17, 17, 17]),
    ('B', [30, 17, 17, 30, 17, 17, 30]),
    ('C', [14, 17, 16, 16, 16, 17, 14]),
    ('D', [28, 18, 17, 17, 17, 18, 28]),
    ('E', [31, 16, 16, 30, 16, 16, 31]),
    ('F', [31, 16, 16, 30, 16, 16, 16]),
    ('G', [14, 17, 16, 23, 17, 17, 15]),
    ('H', [17, 17, 17, 31, 17, 17, 17]),
    ('I', [14, 4, 4, 4, 4, 4, 14]),
    ('J', [7, 2, 2, 2, 2, 18, 12]),
    ('K', [17, 18, 20, 24, 20, 18, 17]),
    ('L', [16, 16, 16, 16, 16, 16, 31]),
    ('M', [17, 27, 21, 21, 17, 17, 17]),
    ('N', [17, 25, 21, 19, 17, 17, 17]),
    ('O', [14, 17, 17, 17, 17, 17, 14]),
    ('P', [30, 17, 17, 30, 16, 16, 16]),
    ('Q', [14, 17, 17, 17, 21, 18, 13]),
    ('R', [30, 17, 17, 30, 20, 18, 17]),
    ('S', [15, 16, 16, 14, 1, 1, 30]),
    ('T', [31, 4, 4, 4, 4, 4, 4]),
    ('U', [17, 17, 17, 17, 17, 17, 14]),
    ('V', [17, 17, 17, 17, 17, 10, 4]),
    ('W', [17, 17, 17, 21, 21, 27, 17]),
    ('X', [17, 17, 10, 4, 10, 17, 17]),
    ('Y', [17, 17, 10, 4, 4, 4, 4]),
    ('Z', [31, 1, 2, 4, 8, 16, 31]),
];

/// The rows of the glyph for `ch`, `None` when the font does not carry it.
#[must_use]
#[inline]
pub fn glyph(ch: char) -> Option<&'static Glyph> {
    GLYPHS
        .iter()
        .find(|entry| entry.0 == ch)
        .map(|entry| &entry.1)
}

/// How one [`draw_text`] pass paints: fill, outline and integer scale.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    /// The colour of the glyph pixels themselves.
    pub colour: Rgba,
    /// The colour of the one-pixel outline woven under the glyphs.
    pub outline: Rgba,
    /// Integer magnification: each glyph pixel becomes a square of this
    /// many canvas pixels per edge.
    pub scale: u32,
}

/// Draws `text` centred on `at` in `style`.
///
/// The string upper-cases first and draws characters the font lacks as
/// `?`; every glyph pixel becomes a square of [`scale`](TextStyle::scale)
/// canvas pixels (`0` magnifies like `1`). Nine passes put a one-pixel
/// [`outline`](TextStyle::outline) halo behind the
/// [`colour`](TextStyle::colour) fill. Off-canvas pixels clip like every
/// other canvas draw; an empty string draws nothing.
#[inline]
pub fn draw_text(canvas: &mut Canvas, at: Pt, text: &str, style: TextStyle) {
    let glyphs = layout(text);
    if glyphs.is_empty() {
        return;
    }
    let origin = centred(at, glyphs.len(), style.scale);
    let edge = TextStyle {
        colour: style.outline,
        ..style
    };
    for dy in [-1_i64, 0_i64, 1_i64] {
        for dx in [-1_i64, 0_i64, 1_i64] {
            paint(canvas, &glyphs, shift(origin, dx, dy), edge);
        }
    }
    paint(canvas, &glyphs, origin, style);
}

/// The glyph grid shifted by `(dx, dy)` canvas pixels (saturating).
const fn shift(at: Pt, dx: i64, dy: i64) -> Pt {
    (at.0.saturating_add(dx), at.1.saturating_add(dy))
}

/// The glyphs of `text`, upper-cased, unknown characters drawn as `?`.
fn layout(text: &str) -> Vec<&'static Glyph> {
    text.chars()
        .map(|ch| {
            glyph(ch.to_ascii_uppercase())
                .or_else(|| glyph('?'))
                .unwrap_or(&SPACE)
        })
        .collect()
}

/// The top-left pixel of `count` glyphs centred on `at`.
fn centred(at: Pt, count: usize, scale: u32) -> Pt {
    let (width, height) = extent(count, scale);
    (
        at.0.saturating_sub(width.div_euclid(2)),
        at.1.saturating_sub(height.div_euclid(2)),
    )
}

/// The `(width, height)` in canvas pixels of `count` glyphs at `scale`,
/// the inter-letter gaps included.
fn extent(count: usize, scale: u32) -> (i64, i64) {
    let step = i64::from(scale.max(1));
    let columns = i64::try_from(count.saturating_mul(GLYPH_STEP))
        .unwrap_or(i64::MAX)
        .saturating_sub(i64::try_from(LETTER_GAP).unwrap_or(1));
    (
        columns.saturating_mul(step),
        i64::try_from(GLYPH_HEIGHT)
            .unwrap_or(7)
            .saturating_mul(step),
    )
}

/// One [`draw_text`] pass: every glyph in `style.colour`, the first
/// glyph's top-left pixel at `origin`.
fn paint(canvas: &mut Canvas, glyphs: &[&'static Glyph], origin: Pt, style: TextStyle) {
    let step = i64::from(style.scale.max(1));
    let advance = i64::try_from(GLYPH_STEP).unwrap_or(6).saturating_mul(step);
    for (column, rows) in glyphs.iter().copied().enumerate() {
        let x = origin
            .0
            .saturating_add(i64::try_from(column).unwrap_or(0).saturating_mul(advance));
        draw_glyph(canvas, *rows, (x, origin.1), style);
    }
}

/// The set pixels of one glyph, each a square of
/// [`scale`](TextStyle::scale) canvas pixels, the grid's top left at
/// `at`.
fn draw_glyph(canvas: &mut Canvas, rows: Glyph, at: Pt, style: TextStyle) {
    let step = i64::from(style.scale.max(1));
    for (dy, mask) in rows.iter().copied().enumerate() {
        let row_y =
            at.1.saturating_add(i64::try_from(dy).unwrap_or(0).saturating_mul(step));
        draw_row(canvas, mask, (at.0, row_y), style);
    }
}

/// The set columns of one [`Glyph`] row, each painted as a block in
/// `style.colour`.
fn draw_row(canvas: &mut Canvas, mask: u8, at: Pt, style: TextStyle) {
    if mask == 0_u8 {
        return;
    }
    let step = i64::from(style.scale.max(1));
    for (dx, bit) in COLUMN_BITS.iter().copied().enumerate() {
        if mask & bit == 0_u8 {
            continue;
        }
        let x =
            at.0.saturating_add(i64::try_from(dx).unwrap_or(0).saturating_mul(step));
        let max = (
            x.saturating_add(step).saturating_sub(1),
            at.1.saturating_add(step).saturating_sub(1),
        );
        canvas.fill_rect((x, at.1), max, style.colour);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COLUMN_BITS, GLYPH_HEIGHT, GLYPH_WIDTH, GLYPHS, Glyph, SPACE, TextStyle, draw_text, glyph,
    };
    use crate::render::canvas::{Canvas, Pt, Rgba};

    /// Every character the table promises to carry.
    const SUPPORTED: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ -_.:#()/?!";

    /// An opaque test colour for the glyph fill.
    const FILL: Rgba = [255, 255, 128, 255];
    /// An opaque black for the outline.
    const OUTLINE: Rgba = [0, 0, 0, 255];
    /// The background a test canvas starts from.
    const CLEAR: Rgba = [77, 77, 77, 255];

    /// The test style at `scale`.
    const fn style(scale: u32) -> TextStyle {
        TextStyle {
            colour: FILL,
            outline: OUTLINE,
            scale,
        }
    }

    /// The pixel of `canvas` at an `i64` coordinate, background outside.
    fn at(canvas: &Canvas, point: Pt) -> Rgba {
        let column = u32::try_from(point.0).expect("a fitted column");
        let deep = u32::try_from(point.1).expect("a fitted row");
        canvas.pixel(column, deep).unwrap_or(CLEAR)
    }

    /// Every pixel of `canvas` that is neither the background.
    fn painted(canvas: &Canvas) -> Vec<Pt> {
        (0..canvas.height())
            .flat_map(|row| (0..canvas.width()).map(move |col| (col, row)))
            .map(|(col, row)| (i64::from(col), i64::from(row)))
            .filter(|&point| at(canvas, point) != CLEAR)
            .collect()
    }

    /// The number of pixels of `canvas` holding exactly `colour`.
    fn coloured(canvas: &Canvas, colour: Rgba) -> usize {
        painted(canvas)
            .into_iter()
            .filter(|&point| at(canvas, point) == colour)
            .count()
    }

    #[test]
    fn every_supported_character_has_a_five_wide_glyph() {
        assert_eq!(GLYPH_WIDTH, 5, "the documented glyph width");
        assert_eq!(GLYPH_HEIGHT, 7, "the documented glyph height");
        assert_eq!(
            COLUMN_BITS,
            [16_u8, 8_u8, 4_u8, 2_u8, 1_u8],
            "bit 4 is the leftmost column"
        );
        assert_eq!(SUPPORTED.chars().count(), GLYPHS.len(), "table size");
        for ch in SUPPORTED.chars() {
            let rows: &Glyph = glyph(ch).unwrap_or_else(|| panic!("the font lacks {ch:?}"));
            assert_eq!(rows.len(), GLYPH_HEIGHT, "{ch:?} row count");
            assert!(
                rows.iter().all(|mask| mask & !0x1F_u8 == 0_u8),
                "{ch:?} reaches past the {GLYPH_WIDTH} columns: {rows:?}"
            );
        }
        assert!(
            glyph('@').is_none(),
            "characters outside the set have no glyph"
        );
    }

    #[test]
    fn shapes_are_distinct_and_only_space_is_blank() {
        let mut rows: Vec<Glyph> = GLYPHS.iter().map(|entry| entry.1).collect();
        rows.sort_unstable();
        let shared = rows
            .windows(2)
            .filter(|pair| pair.first() == pair.get(1))
            .count();
        assert_eq!(shared, 0_usize, "no two characters share a shape");
        let blanks = rows.iter().filter(|row| **row == SPACE).count();
        assert_eq!(blanks, 1_usize, "exactly the space is blank");
    }

    #[test]
    fn draw_text_paints_fill_and_outline_in_the_centred_box() {
        let mut canvas = Canvas::new(40, 24, CLEAR);
        draw_text(&mut canvas, (20, 12), "42", style(2));
        // Two glyphs at scale 2: (2 * 6 - 1) * 2 = 22 wide, 7 * 2 = 14
        // tall, centred on (20, 12): columns 9..=30, rows 5..=18, with
        // the one-pixel outline halo reaching 8..=31 and 4..=19.
        let outside: Vec<Pt> = painted(&canvas)
            .into_iter()
            .filter(|&(col, row)| !(8..=31).contains(&col) || !(4..=19).contains(&row))
            .collect();
        assert!(outside.is_empty(), "nothing painted outside: {outside:?}");
        let fill = coloured(&canvas, FILL);
        assert!(fill > 20, "the glyph interiors carry fill: {fill}");
        assert!(
            coloured(&canvas, OUTLINE) > 0,
            "the outline halo is painted too"
        );
    }

    #[test]
    fn unknown_and_lower_case_characters_draw_upper_case_and_question() {
        let mut upper = Canvas::new(24, 12, CLEAR);
        let mut lower = Canvas::new(24, 12, CLEAR);
        let mut unknown = Canvas::new(24, 12, CLEAR);
        let mut question = Canvas::new(24, 12, CLEAR);
        draw_text(&mut upper, (12, 6), "IMP", style(1));
        draw_text(&mut lower, (12, 6), "imp", style(1));
        draw_text(&mut unknown, (12, 6), "@?M", style(1));
        draw_text(&mut question, (12, 6), "??M", style(1));
        assert_eq!(painted(&upper), painted(&lower), "lower case uppercases");
        assert_eq!(
            painted(&unknown),
            painted(&question),
            "a character outside the set draws as ?"
        );
    }

    #[test]
    fn empty_text_draws_nothing() {
        let mut canvas = Canvas::new(8, 8, CLEAR);
        draw_text(&mut canvas, (4, 4), "", style(1));
        assert!(
            painted(&canvas).is_empty(),
            "an empty string paints nothing"
        );
    }
}
