//! Rendering core: the session's render defaults and the shared curves the
//! render tools use.
//!
//! [`settings`] holds [`RenderSettings`], the validated, session-level
//! defaults (`640x400`, `Doom` lighting, a 90 degree field of view) that
//! `render_view`, `render_topdown` and `render_minimap` start from, and
//! [`canvas`] the integer raster they all draw onto. This module root adds
//! the one piece of maths every renderer needs and none of the settings
//! themselves: [`gamma_table`]. [`scene`] prepares a [`Scene`] (map,
//! palette, `COLORMAP` tables, textures, flats, sky) from a session and
//! [`light`] turns sector lights, depths and wall orientations into
//! `COLORMAP` tables and shaded pixels for the 3D view. [`topdown`] holds
//! the shared line map: the `Doom` automap classification and colours,
//! thing visuals and the square [`Window`] the minimap frames with.
//! [`view`] casts the columns of the 3D view: exact `i128` ray
//! parameters, a sorted portal walk per column, textured walls,
//! world-aligned textured planes and a sky texture, plus the per-pixel
//! depth buffer the masked and sprite passes will need. Nothing here
//! knows about MCP; the `render` tool
//! group is the only caller that talks to clients.

pub mod canvas;
pub mod light;
pub mod scene;
pub mod settings;
pub mod topdown;
pub mod view;

use crate::render::settings::MAX_GAMMA;

pub use canvas::{Canvas, Edge, Pt, Rgba, Stroke};
pub use light::{WallDir, colormap_index, shade};
pub use scene::{Scene, build_scene, camera_sector, default_eye_z};
pub use settings::{Lighting, RenderSettings, Switch, TopdownStyle};
pub use topdown::{
    AutomapColors, PLAYER_ARROW_PX, PlayerMark, THING_DOT_PX, UNKNOWN_THING_RADIUS, Window,
    automap_color, category_color, paint_automap_lines, paint_player_arrow, paint_thing_dots,
    scale_down, side_sector, thing_visual,
};
pub use view::{
    MAX_PITCH, MIN_PITCH, MaskedMiddle, Rendered, ViewCamera, check_pitch, render_view,
};

/// The brightest input value of an 8-bit channel.
const GAMMA_MAX_INPUT: i32 = 255;

/// Doom's gamma divisor: `255 * 8`, the denominator of the boost term.
const GAMMA_DIVISOR: i32 = 255 * 8;

/// Number of input levels an 8-bit channel has, and so the length of every
/// gamma table.
const GAMMA_ENTRIES: usize = 256;

/// An integer approximation of Doom's gamma correction for `level`.
///
/// `level 0` is the identity and every table keeps both endpoints fixed
/// (`0` maps to `0`, `255` maps to `255`) while mid-tones are lifted; the
/// curve is non-decreasing, so the table can be applied as a per-channel
/// lookup without inverting ordering. `level` is clamped to
/// [`MAX_GAMMA`]: any higher value behaves like the top level.
///
/// This is an *approximation*, not Doom's data: the game ships precomputed
/// `COLORMAP`s per gamma level, and Doom's own gamma command-line levels
/// raise the palette mid-tones with a table this code never reads. The
/// formula here is
/// `out = in + (255 - in) * in * level / (255 * 8)` in integer arithmetic,
/// a brightening curve that peaks in the middle of the range. Renderers
/// apply the result to every output channel.
#[must_use]
#[inline]
pub fn gamma_table(level: u8) -> [u8; 256] {
    let strength = level.min(MAX_GAMMA);
    let mut table = [0_u8; GAMMA_ENTRIES];
    for (slot, input) in table.iter_mut().zip(0_u8..=255_u8) {
        *slot = gamma_point(input, strength);
    }
    table
}

/// The corrected value of one 8-bit `input` at gamma `level`.
///
/// `level` is expected to already be clamped to [`MAX_GAMMA`]; `0` returns
/// `input` unchanged. Every step is checked and saturates, so no input can
/// overflow or wrap.
fn gamma_point(input: u8, level: u8) -> u8 {
    if level == 0 {
        return input;
    }
    let value = i32::from(input);
    let boost = i32::from(level);
    let lift = GAMMA_MAX_INPUT
        .checked_sub(value)
        .and_then(|span| span.checked_mul(value))
        .and_then(|area| area.checked_mul(boost))
        .and_then(|numerator| numerator.checked_div(GAMMA_DIVISOR))
        .unwrap_or_default();
    let lifted = value.saturating_add(lift);
    u8::try_from(lifted).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::{GAMMA_ENTRIES, gamma_table};
    use crate::render::settings::MAX_GAMMA;

    /// The table entry for `input` at `level`, index-checked.
    fn at(level: u8, input: u8) -> u8 {
        *gamma_table(level)
            .get(usize::from(input))
            .unwrap_or_else(|| panic!("a 256-entry table covers every u8"))
    }

    #[test]
    fn level_zero_is_the_identity() {
        let table = gamma_table(0);
        assert_eq!(table.len(), GAMMA_ENTRIES, "one entry per input");
        for (index, output) in table.iter().enumerate() {
            let input = u8::try_from(index).expect("the table covers exactly the u8 range");
            assert_eq!(*output, input, "identity at input {input}");
        }
    }

    #[test]
    fn every_level_fixes_both_endpoints() {
        for level in 0_u8..=MAX_GAMMA {
            assert_eq!(at(level, 0), 0, "black stays black at level {level}");
            assert_eq!(at(level, 255), 255, "white stays white at level {level}");
        }
    }

    /// Asserts that `table` never darkens as the input grows.
    fn assert_monotonic(level: u8, table: &[u8]) {
        for (input, (previous, next)) in table.iter().zip(table.iter().skip(1)).enumerate() {
            assert!(
                previous <= next,
                "level {level} darkens between input {input} and the next: {previous} then \
                 {next}"
            );
        }
    }

    #[test]
    fn every_level_is_monotonic() {
        for level in 0_u8..=MAX_GAMMA {
            assert_monotonic(level, &gamma_table(level));
        }
    }

    #[test]
    fn higher_levels_brighten_mid_tones_more() {
        for input in [32_u8, 64, 128, 192] {
            assert!(
                at(1, input) > input,
                "level 1 lifts input {input} above itself"
            );
            assert!(
                at(2, input) >= at(1, input),
                "level 2 never darkens what level 1 lifted"
            );
            assert!(
                at(MAX_GAMMA, input) > input,
                "the top level lifts input {input}"
            );
        }
        assert_eq!(at(1, 128), 135, "the documented mid-grey of level 1");
    }

    #[test]
    fn a_level_above_the_top_behaves_like_the_top_level() {
        assert_eq!(
            gamma_table(5_u8),
            gamma_table(MAX_GAMMA),
            "an overshot level is clamped, not extrapolated"
        );
    }
}
