//! Shared builders for the transparent pass's tests: the fixture palette,
//! `COLORMAP`, scene, camera and projection, the indexed-image, sprite and
//! masked-middle makers, the merged-pass runner and the pixel readers the
//! topic modules reuse.
//!
//! [`projection`] places: where a sprite lands on the raster, how its
//! offsets and the frame's edges clip it, and how the depth test and the
//! merged far-to-near order settle shared pixels. [`sampling`] reads:
//! which texel a minified or a magnified pixel takes, and the integer
//! helpers that projection maths is built from.

mod projection;
mod sampling;

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use super::{Counts, Frame, Pass, SpriteItem, draw_transparent};
use crate::gfx::colormap::TABLE_BYTES;
use crate::gfx::image::IndexedImage;
use crate::gfx::palette::Palette;
use crate::render::canvas::Canvas;
use crate::render::view::{Fraction, MaskedMiddle, ONE_16, Projection, ViewCamera};
use crate::render::{Lighting, RenderSettings, Scene, WallDir};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::{Map, MapFormat};
use crate::wad::name::LumpName;

/// Columns in the fixture raster.
pub(super) const WIDTH: u32 = 32;
/// Rows in the fixture raster.
pub(super) const HEIGHT: u32 = 64;
/// The unfilled fixture background.
pub(super) const BLANK: [u8; 4] = [9_u8, 9, 9, 255];
/// The fixture camera's eye height, in map units.
pub(super) const EYE: i64 = 41;
/// The raster's centre column, `W / 2`, where a sprite straight ahead
/// lands.
pub(super) const CENTRE: u32 = 16;
/// The horizon row of a level camera, `H / 2`.
pub(super) const HORIZON: u32 = 32;

/// `value` scaled into raw 16.16.
pub(super) fn raw(value: i64) -> i128 {
    i128::from(value).saturating_mul(ONE_16)
}

/// A raw test fraction (the denominators here are positive).
fn frac(n: i64, d: i64) -> Fraction {
    Fraction::new(i128::from(n), i128::from(d)).expect("test denominators are non-zero")
}

/// The fixture palette: colour `i` is `(i, 255 - i, i)`.
fn fixture_palette() -> Palette {
    let mut colors = [[0_u8; 3]; 256];
    for (index, color) in colors.iter_mut().enumerate() {
        let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
        *color = [byte, 255_u8.wrapping_sub(byte), byte];
    }
    Palette::from(colors)
}

/// The identity lighting table.
fn identity() -> [u8; TABLE_BYTES] {
    let mut table = [0_u8; TABLE_BYTES];
    for (index, entry) in table.iter_mut().enumerate() {
        *entry = u8::try_from(index).expect("the table covers exactly the u8 range");
    }
    table
}

/// The shaded pixel of palette index `index`: the fixture scene is
/// full bright behind an identity table, so it is its palette colour.
pub(super) fn lit(index: u8) -> [u8; 4] {
    [index, 255_u8.wrapping_sub(index), index, 255_u8]
}

/// A full-bright scene with an identity `COLORMAP` and the one masked
/// texture the tests share. Its map, flats and sky are never read:
/// sprite items carry their own patches.
pub(super) fn fixture_scene() -> Scene {
    let map = Map::new_empty(
        LumpName::parse("MAP01").expect("test names are valid"),
        MapFormat::Doom,
    );
    let geometry = SectorIndex::new(&map);
    Scene {
        map,
        geometry,
        palette: fixture_palette(),
        colormaps: vec![identity()],
        textures: BTreeMap::from([("TESTBAR".to_owned(), solid(100, 128, 64))]),
        flats: BTreeMap::new(),
        sky: solid(1, 1, 0),
        sky_flat: "F_SKY1".to_owned(),
        settings: RenderSettings {
            lighting: Lighting::Fullbright,
            ..RenderSettings::default()
        },
        problems: Vec::new(),
    }
}

/// The fixture camera: the origin, `EYE` units up, due east and level
/// through a 90 degree field, so `focal2` is the raster width.
pub(super) fn camera() -> ViewCamera {
    ViewCamera {
        x: 0,
        y: 0,
        z: i32::try_from(EYE).expect("test heights fit"),
        angle: 0,
        pitch: 0,
        fov: 90,
    }
}

/// The fixture projection over a 32x64 raster, whose horizon row is
/// [`HORIZON`].
pub(super) fn projection() -> Projection {
    Projection::new(&camera(), WIDTH, HEIGHT)
}

/// A `width` by `height` indexed image of solid `index`, fully opaque.
pub(super) fn solid(width: u32, height: u32, index: u8) -> IndexedImage {
    let count = usize::try_from(width.saturating_mul(height)).expect("test images fit");
    IndexedImage::new(width, height, vec![index; count], vec![true; count])
        .expect("test images build")
}

/// An image whose texel `(j, u)` is palette index `j + u * width`, so
/// every painted pixel names the texel it read.
pub(super) fn gradient(width: u32, height: u32) -> IndexedImage {
    let cells = usize::try_from(width.saturating_mul(height)).expect("test images fit");
    let indices = (0..cells)
        .map(|cell| u8::try_from(cell).expect("test gradients stay inside the palette"))
        .collect();
    IndexedImage::new(width, height, indices, vec![true; cells]).expect("test images build")
}

/// A `width` by `height` patch of `index`, transparent from texel
/// column `clear_from` rightwards.
pub(super) fn clear_columns(width: u32, height: u32, index: u8, clear_from: u32) -> IndexedImage {
    let columns = usize::try_from(width).expect("test widths fit");
    let rows = usize::try_from(height).expect("test heights fit");
    let keep = usize::try_from(clear_from).expect("test widths fit");
    let mut indices = Vec::new();
    let mut opaque = Vec::new();
    for _row in 0..rows {
        indices.extend(vec![index; columns]);
        opaque.extend((0..columns).map(|column| column < keep));
    }
    IndexedImage::new(width, height, indices, opaque).expect("test images build")
}

/// A sprite of `patch` standing on world height `base`, `tz` map units
/// straight east of the fixture camera and so centred on the view axis
/// (`tx16` zero), with its `topoffset` at the patch's full height.
pub(super) fn standing(patch: IndexedImage, tz: i64, base: i64) -> SpriteItem {
    let half_width = u64::from(patch.width)
        .checked_div(2_u64)
        .expect("test widths divide");
    let height = i64::from(patch.height);
    SpriteItem {
        image: patch,
        left_offset16: i128::from(half_width).saturating_mul(ONE_16),
        top_offset16: raw(height),
        x: tz,
        y: 0,
        base,
        light: 255,
        tx16: 0_i128,
        tz16: raw(tz),
    }
}

/// A single-row masked middle at `column`/`row`, `tz` map units deep,
/// drawing texel column 0 of the fixture scene's `TESTBAR` (palette
/// index 64). With `t = 1` and a 128-unit `tex_top` its one row reads
/// a texture row inside the bar.
pub(super) fn masked_at(column: u32, row: i64, tz: i64) -> MaskedMiddle {
    MaskedMiddle {
        column,
        line: 0,
        side: 0,
        top: row,
        bottom: row,
        depth16: raw(tz),
        texture: "TESTBAR".to_owned(),
        texcol: 0,
        tex_top16: raw(128),
        y_off16: 0,
        light: 255,
        dir: WallDir::Other,
        t: frac(1, 1),
    }
}

/// Runs the merged pass over `masked` and `sprites` on a fresh
/// fixture frame whose buffer holds `depth` everywhere.
pub(super) fn draw(
    masked: &mut [MaskedMiddle],
    sprites: &mut [SpriteItem],
    depth: i64,
) -> (Canvas, Counts) {
    let scene = fixture_scene();
    let proj = projection();
    let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
    let buffer = vec![depth; usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("fits")];
    let counts = draw_transparent(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &buffer),
        masked,
        sprites,
    );
    (canvas, counts)
}

/// Paints one sprite into a fixture frame with the open (infinitely
/// deep) buffer a sky pixel carries, and answers with the canvas.
pub(super) fn paint_sprite(item: SpriteItem) -> Canvas {
    let (canvas, counts) = draw(&mut [], &mut [item], i64::MAX);
    assert_eq!(
        counts.sprites_drawn, 1,
        "the sprite gained at least one pixel"
    );
    assert_eq!(counts.masked_columns, 0, "no masked items were given");
    canvas
}

/// Whether `canvas` holds something other than the background at
/// `(column, row)`.
pub(super) fn painted(canvas: &Canvas, column: u32, row: u32) -> bool {
    canvas
        .pixel(column, row)
        .is_some_and(|pixel| pixel != BLANK)
}

/// Whether `canvas` holds any painted pixel in screen `column`.
pub(super) fn column_painted(canvas: &Canvas, column: u32) -> bool {
    (0..HEIGHT).any(|row| painted(canvas, column, row))
}

/// Whether `canvas` holds any painted pixel in screen `row`.
pub(super) fn row_painted(canvas: &Canvas, row: u32) -> bool {
    (0..WIDTH).any(|column| painted(canvas, column, row))
}

/// The screen columns `canvas` painted over the fixture background.
pub(super) fn columns_of(canvas: &Canvas) -> Vec<u32> {
    (0..WIDTH)
        .filter(|column| column_painted(canvas, *column))
        .collect()
}

/// The screen rows `canvas` painted over the fixture background.
pub(super) fn rows_of(canvas: &Canvas) -> Vec<u32> {
    (0..HEIGHT)
        .filter(|row| row_painted(canvas, *row))
        .collect()
}

/// Asserts the fixture sprite's 64 pixels hold, one apiece, the
/// colours of the gradient texels they project from: texel `(j, u)`
/// sits at column `12 + j`, row `24 + u`.
pub(super) fn assert_each_texel_lands_on_its_own_pixel(canvas: &Canvas) {
    for cell in 0_u32..64_u32 {
        let texcol = cell.checked_rem(8_u32).expect("test divisor");
        let texrow = cell.checked_div(8_u32).expect("test divisor");
        let column = 12_u32.saturating_add(texcol);
        let row = 24_u32.saturating_add(texrow);
        let index = u8::try_from(cell).expect("test gradients stay inside the palette");
        assert_eq!(
            canvas.pixel(column, row),
            Some(lit(index)),
            "texel ({texcol}, {texrow}) projects to column {column}, row {row}"
        );
    }
}
