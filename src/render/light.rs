//! 3D-view lighting: choosing the `COLORMAP` table and shading a pixel.
//!
//! [`colormap_index`] turns a sector light level, a view depth and the
//! wall's orientation into one of the 32 lighting tables of the scene's
//! `COLORMAP` (see [`crate::gfx::colormap`]); [`shade`] looks a palette
//! index up through that table and the settings' gamma curve and answers
//! with the `RGBA8` pixel to plot. Both are pure: they read no lumps and
//! hold no state, so the column renderer only does maths.

use crate::render::gamma_table;
use crate::render::scene::Scene;
use crate::render::settings::Lighting;

/// Lowest lighting table (full bright).
const MIN_LEVEL: i32 = 0;
/// Highest lighting table: the 32nd of the `COLORMAP` is not a level.
const MAX_LEVEL: i32 = 31;
/// The largest `usize` index [`colormap_index`] can answer with.
const MAX_LEVEL_INDEX: usize = 31;
/// The brightest sector light level.
const FULL_LIGHT: i32 = 255;
/// Shift of the vanilla light-to-table ramp: `table = (255 - light) >> 3`.
const LEVEL_SHIFT: u32 = 3;
/// Shift of the distance fade: `depth >> 7` map units per table step.
const DEPTH_SHIFT: u32 = 7;
/// Tables the `doom` fade starts from: at depth `0` the sector's own
/// light is lifted by this many steps (Doom's light source sits at the
/// viewer, so the closest surfaces read brighter).
const DEPTH_BIAS: i32 = 4;

/// A wall's orientation, for `Doom`'s fake contrast: walls are shaded a
/// step brighter or darker by which way they face, because the engine
/// never computes real light direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WallDir {
    /// A slanted or curved wall: no contrast.
    #[default]
    Other,
    /// A linedef with `dy == 0`: drawn one step brighter.
    EastWest,
    /// A linedef with `dx == 0`: drawn one step darker.
    NorthSouth,
}

/// The `COLORMAP` table for a surface lit at `sector_light` seen at
/// `depth` map units, as a fake-contrasted wall facing `wall_dir`.
///
/// `fullbright` is always table `0` before contrast; `flat` maps the
/// sector light onto the 32 levels; `doom` adds the distance fade
/// (`depth >> 7`) on top, biased so a surface at the camera sits four
/// tables brighter than its sector light alone. The answer is clamped to
/// `0..=31`, before and after the contrast step, so it always indexes a
/// lighting table.
#[must_use]
#[inline]
pub fn colormap_index(mode: Lighting, sector_light: i32, depth: i64, wall_dir: WallDir) -> usize {
    let level = match mode {
        Lighting::Fullbright => MIN_LEVEL,
        Lighting::Flat => light_level(sector_light),
        Lighting::Doom => light_level(sector_light)
            .saturating_add(depth_level(depth))
            .saturating_sub(DEPTH_BIAS),
    };
    let with_contrast = clamp_level(level).saturating_add(contrast(wall_dir));
    usize::try_from(clamp_level(with_contrast)).unwrap_or(MAX_LEVEL_INDEX)
}

/// The `RGBA8` of palette entry `palette_index` under lighting table
/// `map`: `palette[colormaps[map][index]]`, each channel passed through
/// the settings' gamma table, fully opaque.
///
/// A `map` past the scene's tables uses the first table (full bright),
/// and an index the tables or palette cannot serve falls back to identity,
/// so this never fails on the pixel being shaded.
#[must_use]
#[inline]
pub fn shade(scene: &Scene, palette_index: u8, map: usize) -> [u8; 4] {
    let mapped = scene
        .colormaps
        .get(map)
        .or_else(|| scene.colormaps.first())
        .and_then(|table| table.get(usize::from(palette_index)).copied())
        .unwrap_or(palette_index);
    let gamma = gamma_table(scene.settings.gamma);
    let [red, green, blue] = scene
        .palette
        .color(usize::from(mapped))
        .unwrap_or([0_u8; 3]);
    let fix = |channel| gamma.get(usize::from(channel)).copied().unwrap_or(channel);
    [fix(red), fix(green), fix(blue), 255_u8]
}

/// The light-only table of `sector_light`: `(255 - light) >> 3`.
fn light_level(sector_light: i32) -> i32 {
    FULL_LIGHT
        .saturating_sub(sector_light)
        .checked_shr(LEVEL_SHIFT)
        .unwrap_or(0)
}

/// The distance fade of `depth` map units, as table steps.
fn depth_level(depth: i64) -> i32 {
    depth
        .checked_shr(DEPTH_SHIFT)
        .and_then(|fade| i32::try_from(fade).ok())
        .unwrap_or(i32::MAX)
}

/// The contrast step a facing adds to a table index.
const fn contrast(wall_dir: WallDir) -> i32 {
    match wall_dir {
        WallDir::Other => 0,
        WallDir::EastWest => -1,
        WallDir::NorthSouth => 1,
    }
}

/// Keeps a table index inside the 32 lighting tables.
const fn clamp_level(value: i32) -> i32 {
    if value < MIN_LEVEL {
        MIN_LEVEL
    } else if value > MAX_LEVEL {
        MAX_LEVEL
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{WallDir, colormap_index, shade};
    use crate::gfx::colormap::TABLE_BYTES;
    use crate::gfx::palette::Palette;
    use crate::render::{Lighting, RenderSettings, scene::Scene};
    use crate::wad::map::{Map, MapFormat};

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`.
    fn fixture_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
            *color = [byte, 255_u8.wrapping_sub(byte), byte];
        }
        Palette::from(colors)
    }

    /// A `[u8; 256]` table filled by `make(index)`.
    fn table(make: impl Fn(usize) -> u8) -> [u8; TABLE_BYTES] {
        let mut table = [0_u8; TABLE_BYTES];
        for (index, entry) in table.iter_mut().enumerate() {
            *entry = make(index);
        }
        table
    }

    /// A scene with hand-built lighting tables: an identity map, a map
    /// that sends every index to entry `8` and a all-`1` dark map.
    fn shaded_scene(gamma: u8, colormaps: Vec<[u8; TABLE_BYTES]>) -> Scene {
        let map = Map::new_empty(
            crate::wad::name::LumpName::parse("MAP01").expect("test names are valid"),
            MapFormat::Doom,
        );
        Scene {
            map,
            palette: fixture_palette(),
            colormaps,
            textures: BTreeMap::new(),
            flats: BTreeMap::new(),
            sky: fixture_sky(),
            sky_flat: "F_SKY1".to_owned(),
            settings: RenderSettings {
                gamma,
                ..RenderSettings::default()
            },
            problems: Vec::new(),
        }
    }

    /// A one-pixel sky stands in for the real image in these tests.
    fn fixture_sky() -> crate::gfx::image::IndexedImage {
        crate::gfx::image::IndexedImage::new(1, 1, vec![0_u8], vec![true])
            .expect("a 1x1 image builds")
    }

    /// The identity lighting table.
    fn identity() -> [u8; TABLE_BYTES] {
        table(|index| u8::try_from(index).expect("the table covers exactly the u8 range"))
    }

    #[test]
    fn flat_mode_maps_the_sector_light_alone() {
        assert_eq!(
            colormap_index(Lighting::Flat, 255, 0, WallDir::Other),
            0,
            "full light is table 0"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 192, 9999, WallDir::Other),
            7,
            "(255 - 192) >> 3 = 7, depth ignored"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 0, 0, WallDir::Other),
            31,
            "dark"
        );
    }

    #[test]
    fn doom_mode_adds_the_distance_fade_and_bias() {
        assert_eq!(
            colormap_index(Lighting::Doom, 192, 0, WallDir::Other),
            3,
            "flat level 7 minus the camera-side bias of 4"
        );
        assert_eq!(
            colormap_index(Lighting::Doom, 192, 1024, WallDir::Other),
            11,
            "1024 units fade 8 tables"
        );
        assert_eq!(
            colormap_index(Lighting::Doom, 255, 0, WallDir::Other),
            0,
            "a bright sector near the camera clamps at full bright"
        );
    }

    #[test]
    fn fullbright_ignores_light_and_depth() {
        assert_eq!(
            colormap_index(Lighting::Fullbright, 0, 8_192, WallDir::Other),
            0
        );
        assert_eq!(
            colormap_index(Lighting::Fullbright, 255, -8_192, WallDir::Other),
            0
        );
    }

    #[test]
    fn indices_clamp_at_both_ends() {
        assert_eq!(
            colormap_index(Lighting::Doom, 0, 20_000, WallDir::Other),
            31,
            "a far dark surface clamps at the darkest table"
        );
        assert_eq!(
            colormap_index(Lighting::Doom, 255, -4_096, WallDir::Other),
            0,
            "a negative depth cannot go below full bright"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, -100, 0, WallDir::Other),
            31,
            "an over-bright light saturates instead of wrapping"
        );
    }

    #[test]
    fn fake_contrast_steps_the_index_and_clamps() {
        assert_eq!(
            colormap_index(Lighting::Flat, 120, 0, WallDir::EastWest),
            15,
            "-1"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 120, 0, WallDir::NorthSouth),
            17,
            "+1"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 120, 0, WallDir::Other),
            16,
            "an unaligned wall keeps the level"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 255, 0, WallDir::EastWest),
            0,
            "0 - 1 clamps"
        );
        assert_eq!(
            colormap_index(Lighting::Flat, 0, 0, WallDir::NorthSouth),
            31,
            "31 + 1 clamps"
        );
        assert_eq!(
            colormap_index(Lighting::Fullbright, 200, 0, WallDir::NorthSouth),
            1,
            "even full bright takes the darker step"
        );
    }

    #[test]
    fn shade_maps_colormap_then_palette_then_gamma() {
        let dark = table(|_| 8);
        let scene = shaded_scene(0, vec![identity(), dark]);
        assert_eq!(
            shade(&scene, 3, 0),
            [3_u8, 252_u8, 3_u8, 255_u8],
            "the identity table keeps the palette colour"
        );
        assert_eq!(
            shade(&scene, 3, 1),
            [8_u8, 247_u8, 8_u8, 255_u8],
            "table 1 sends every index to colour 8"
        );
        assert_eq!(
            shade(&scene, 3, 99),
            [3_u8, 252_u8, 3_u8, 255_u8],
            "a table past the end falls back to the first"
        );
        let lifted = crate::render::gamma_table(2);
        let gamma_scene = shaded_scene(2, vec![dark]);
        let lift = |channel: u8| {
            *lifted
                .get(usize::from(channel))
                .expect("gamma tables cover the u8 range")
        };
        assert_eq!(
            shade(&gamma_scene, 3, 0),
            [lift(8), lift(247), lift(8), 255_u8],
            "gamma is applied to the shaded colour, not the index"
        );
    }

    #[test]
    fn shade_survives_missing_tables_with_identity() {
        let scene = shaded_scene(0, Vec::new());
        assert_eq!(
            shade(&scene, 3, 0),
            [3_u8, 252_u8, 3_u8, 255_u8],
            "no tables means the index is used directly"
        );
    }
}
