//! Transparent pass: the masked middle textures and the thing sprites,
//! painted far to near, depth-tested against the column caster's buffer.
//!
//! [`crate::render::view`] walks every screen column near to far, filling
//! the raster with opaque faces and planes and recording each painted
//! pixel's perpendicular depth. Two kinds of layer are left to this
//! module: [`MaskedMiddle`] items the walk collected behind two-sided
//! lines, and [`SpriteItem`]s a caller builds from the patch of a thing's
//! sprite frame. Both carry everything needed to draw them without
//! re-walking the column, so the pass is one depth-ordered loop:
//! [`crate::render::view::render_view`] hands both lists to this module's
//! `draw_transparent`, which sorts them far to near, folds them into a
//! single sequence and paints each item through the caster's buffer, so a
//! pixel takes the layer's colour only when nothing nearer already owns
//! it. The buffer is read here and written nowhere: a transparent pixel
//! keeps the depth of whatever stands behind it, and a later, nearer
//! layer can still cover it.
//!
//! A masked middle fills one view column's window rows, sampling one
//! texture column; the texture row of a screen row is the caster's own
//! projection (`v = tex_top + y_off - height`). A sprite is a whole patch
//! projected as a billboard: with `k = (focal2 / 2) / tz` pixels per map
//! unit — an exact `i128` pair, never rounded to whole pixels — the
//! patch's centre column is `W / 2 + tx·k`, its left edge
//! `left_offset·k` to the left of that and its top row
//! `horizon - (base + top_offset - eye)·k`, which is the caster's own
//! `Projection::row` formula at `t = tz / focal2`, because
//! `1 / (2t) = focal2 / (2·tz)` is exactly `k`; a patch therefore scales
//! with distance in both axes. Each screen pixel reads the *last* texel
//! whose span it touches, which keeps a magnified sprite's pixels stable
//! and a minified one monotonic. Neither kind of layer is tiled
//! vertically: rows that fall outside the image keep whatever stands
//! behind them, because a patch draws only its own rows, and texels an
//! image marks non-opaque are holes through which the painted geometry
//! shows.
//!
//! What the pass covered comes back as counts: the screen columns that
//! gained at least one masked pixel and the sprites that gained at least
//! one pixel, landed in [`crate::render::view::Rendered`] as
//! [`crate::render::view::Rendered::masked_columns`] and
//! [`crate::render::view::Rendered::sprites_drawn`].

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cmp::Reverse;

use crate::gfx::image::IndexedImage;
use crate::render::canvas::Canvas;
use crate::render::light::{WallDir, colormap_index, shade};
use crate::render::scene::Scene;
use crate::render::view::{MaskedMiddle, ONE_16, Projection};

/// One thing sprite, prepared by a caller for the transparent pass.
///
/// The pass is a pure function of the caster's projection and this item:
/// it reads no lumps and looks at no map. A caller decodes the thing's
/// sprite frame to a patch image, already flipped when the rotation is a
/// mirror pair, resolves the world height it stands or hangs from into
/// [`SpriteItem::base`], and projects the thing's map position into the
/// camera-relative [`SpriteItem::tx16`] and [`SpriteItem::tz16`]. Those
/// are depths of the same kind the [`MaskedMiddle`] items carry, so the
/// two lists sort and paint together. Skipping is the caller's job: an
/// item behind the camera, or too near it to place, simply draws nothing
/// here.
#[derive(Clone, Debug)]
pub struct SpriteItem {
    /// The decoded patch, already mirrored when the frame says so.
    pub image: IndexedImage,
    /// Texels from the patch's left edge to the sprite's centre, raw
    /// 16.16 map units: the patch's `leftoffset` scaled up.
    pub left_offset16: i128,
    /// Texels from the patch's top edge down to the height it is anchored
    /// at, raw 16.16 map units: the patch's `topoffset` scaled up. The
    /// patch's top therefore stands at world `base + top_offset`.
    pub top_offset16: i128,
    /// The thing's map `x` in whole map units: the position `tx16` and
    /// `tz16` were measured from, kept so a caller can match an item back
    /// to its thing.
    pub x: i64,
    /// The thing's map `y` in whole map units.
    pub y: i64,
    /// The world height the patch is anchored at, in whole map units: the
    /// resolved floor plus the thing's `z`, or the hanging ceiling minus
    /// the patch's height, whichever the caller chose.
    pub base: i64,
    /// The light level shading every pixel of the sprite.
    pub light: i32,
    /// The camera-relative offset along the camera's right, raw 16.16 map
    /// units: `dot((x - cam.x, y - cam.y), (sin, -cos))`.
    pub tx16: i128,
    /// The camera-relative offset along the camera's forward, raw 16.16
    /// map units: `dot((x - cam.x, y - cam.y), (cos, sin))`, positive in
    /// front of the camera. One depth for the whole sprite: the number the
    /// sort, the depth test and the lighting fade all read.
    pub tz16: i128,
}

/// The transparent pass's read-only context: the scene whose palette,
/// lighting tables and textures the items draw from, and the caster's
/// projection that places them on the raster.
pub(crate) struct Pass<'scene> {
    /// The prepared scene: textures, palette, `COLORMAP` tables and
    /// lighting settings.
    scene: &'scene Scene,
    /// The camera projection shared with the column caster.
    proj: &'scene Projection,
}

impl<'scene> Pass<'scene> {
    /// The pass drawing `scene` through the caster's `proj`.
    #[must_use]
    pub(crate) const fn new(scene: &'scene Scene, proj: &'scene Projection) -> Self {
        Self { scene, proj }
    }

    /// Paints one masked middle's window rows; answers whether it painted
    /// at least one pixel. An item whose texture key the scene does not
    /// have draws nothing at all.
    fn paint_masked(&self, frame: &mut Frame<'_>, item: &MaskedMiddle) -> bool {
        let Some(image) = self.scene.textures.get(&item.texture) else {
            return false;
        };
        let depth = item_depth(item.depth16);
        let run = MaskedRun {
            item,
            image,
            column: usize::try_from(item.column).unwrap_or(usize::MAX),
            depth,
            map: colormap_index(self.scene.settings.lighting, item.light, depth, item.dir),
        };
        let mut painted = false;
        for row in item.top..=item.bottom {
            painted = self.paint_texel(frame, &run, row) || painted;
        }
        painted
    }

    /// Paints one window row of `run`: first the depth test against the
    /// caster's buffer, then the texture row the screen row projects to
    /// (nothing when it lands outside the image, which is never tiled
    /// vertically), then the texel's own opacity, and only then the
    /// pixel. The depth buffer is never written.
    fn paint_texel(&self, frame: &mut Frame<'_>, run: &MaskedRun<'_>, row: i64) -> bool {
        if !frame.nearer(run.column, row, run.depth) {
            return false;
        }
        let v16 = run
            .item
            .tex_top16
            .saturating_add(run.item.y_off16)
            .saturating_sub(self.proj.height_at(row, run.item.t));
        let Some(texrow) = tex_row(v16, run.image.height) else {
            return false;
        };
        let Some((index, visible)) = run.image.get(run.item.texcol, texrow) else {
            return false;
        };
        if !visible {
            return false;
        }
        frame.canvas.set(
            i64::from(run.item.column),
            row,
            shade(self.scene, index, run.map),
        );
        true
    }

    /// Paints one sprite: every screen pixel its projection touches, each
    /// depth-tested and each skipped when its texel is a hole. Answers
    /// whether it painted at least one pixel; an item whose depth is not
    /// in front of the camera, and so has no projection, draws nothing.
    fn paint_sprite(&self, frame: &mut Frame<'_>, item: &SpriteItem) -> bool {
        let Some(place) = Placement::of(self.proj, item) else {
            return false;
        };
        let run = SpriteRun {
            image: &item.image,
            rows: &place.rows,
            depth: place.depth,
            map: colormap_index(
                self.scene.settings.lighting,
                item.light,
                place.depth,
                WallDir::Other,
            ),
        };
        let mut painted = false;
        for &column in &place.columns {
            painted = self.paint_sprite_column(frame, &run, column) || painted;
        }
        painted
    }

    /// Paints one projected column of a sprite, top to bottom.
    fn paint_sprite_column(
        &self,
        frame: &mut Frame<'_>,
        run: &SpriteRun<'_>,
        column: (i64, u32),
    ) -> bool {
        let mut painted = false;
        for &(row, texrow) in run.rows {
            let pixel = SpritePixel {
                column: column.0,
                row,
                texcol: column.1,
                texrow,
            };
            painted = self.paint_sprite_pixel(frame, run, pixel) || painted;
        }
        painted
    }

    /// Paints one pixel of a sprite: the depth test first, then the
    /// texel's own opacity, and only then the pixel. The depth buffer is
    /// never written.
    fn paint_sprite_pixel(
        &self,
        frame: &mut Frame<'_>,
        run: &SpriteRun<'_>,
        pixel: SpritePixel,
    ) -> bool {
        let column = usize::try_from(pixel.column).unwrap_or(usize::MAX);
        if !frame.nearer(column, pixel.row, run.depth) {
            return false;
        }
        let Some((index, visible)) = run.image.get(pixel.texcol, pixel.texrow) else {
            return false;
        };
        if !visible {
            return false;
        }
        frame
            .canvas
            .set(pixel.column, pixel.row, shade(self.scene, index, run.map));
        true
    }
}

/// The pass's mutable target: the pixels to overwrite and the caster's
/// recorded depths to test against. The buffer is read here and written
/// nowhere: a transparent pixel never claims its own depth.
pub(crate) struct Frame<'pass> {
    /// The frame the caster filled with walls, planes and sky.
    canvas: &'pass mut Canvas,
    /// The caster's per-pixel perpendicular depths, row-major.
    depth: &'pass [i64],
    /// Pixels per depth-buffer row, the canvas width.
    width: usize,
}

impl<'pass> Frame<'pass> {
    /// A target painting `canvas` against the caster's `depth` buffer.
    #[must_use]
    pub(crate) fn new(canvas: &'pass mut Canvas, depth: &'pass [i64]) -> Self {
        let width = usize::try_from(canvas.width()).unwrap_or(0);
        Self {
            canvas,
            depth,
            width,
        }
    }

    /// Whether map depth `depth` is strictly nearer than what the buffer
    /// holds at `(column, row)`; outside the frame nothing is nearer.
    fn nearer(&self, column: usize, row: i64, depth: i64) -> bool {
        let Ok(line) = usize::try_from(row) else {
            return false;
        };
        let slot = line
            .checked_mul(self.width)
            .and_then(|base| base.checked_add(column));
        slot.and_then(|index| self.depth.get(index))
            .is_some_and(|seen| depth < *seen)
    }
}

/// What one transparent pass covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Screen columns that gained at least one masked pixel; a column
    /// carrying several masked items counts once.
    pub(crate) masked_columns: usize,
    /// Sprites that gained at least one pixel; a sprite counts once
    /// however many pixels it covers.
    pub(crate) sprites_drawn: usize,
}

/// One masked item readied for painting: its looked-up image, the
/// whole-unit depth the test and the `Doom` lighting fade read, the one
/// lighting table for the whole item and the depth-buffer column index.
struct MaskedRun<'item> {
    /// The masked middle being drawn.
    item: &'item MaskedMiddle,
    /// The image named by the item's texture key.
    image: &'item IndexedImage,
    /// The item's screen column as a buffer index, `usize::MAX` when the
    /// column names no pixel.
    column: usize,
    /// The item's depth in whole map units, saturated into `i64`.
    depth: i64,
    /// The `COLORMAP` table shading every pixel of the item.
    map: usize,
}

/// One sprite readied for painting: the patch to read, the projected rows
/// to walk under every column, and the depth and single lighting table
/// the whole sprite shares.
struct SpriteRun<'run> {
    /// The decoded patch.
    image: &'run IndexedImage,
    /// The screen rows the patch touches, with the texel row each reads.
    rows: &'run [(i64, u32)],
    /// The sprite's depth in whole map units, saturated into `i64`.
    depth: i64,
    /// The `COLORMAP` table shading every pixel of the sprite.
    map: usize,
}

/// One pixel of a projected sprite: where it lands on the screen and
/// which texel it reads.
#[derive(Clone, Copy, Debug)]
struct SpritePixel {
    /// The screen column.
    column: i64,
    /// The screen row.
    row: i64,
    /// The texel column within the patch.
    texcol: u32,
    /// The texel row within the patch.
    texrow: u32,
}

/// One transparent layer, from whichever list it came.
#[derive(Clone, Copy, Debug)]
enum Layer<'item> {
    /// A masked middle the column walk collected.
    Masked(&'item MaskedMiddle),
    /// A thing sprite a caller built.
    Sprite(&'item SpriteItem),
}

impl Layer<'_> {
    /// The layer's perpendicular depth, raw 16.16 map units: the number
    /// both kinds carry, so one ordering covers the merged pass.
    const fn depth16(self) -> i128 {
        match self {
            Self::Masked(item) => item.depth16,
            Self::Sprite(item) => item.tz16,
        }
    }

    /// Paints the layer, answering whether it gained at least one pixel.
    fn paint(self, pass: &Pass<'_>, frame: &mut Frame<'_>) -> bool {
        match self {
            Self::Masked(item) => pass.paint_masked(frame, item),
            Self::Sprite(item) => pass.paint_sprite(frame, item),
        }
    }
}

/// Draws the collected masked middles and the caller's sprites, far to
/// near: both lists are depth sorted and folded into one sequence, so a
/// nearer sprite covers a farther masked column and a nearer masked
/// column keeps its pixel against a farther sprite. Every pixel is
/// depth-tested against the caster's buffer, which this pass only ever
/// reads.
///
/// `masked` and `sprites` are sorted in place, by [`MaskedMiddle::depth16`]
/// and [`SpriteItem::tz16`] descending, so after the call each list reads
/// in the order it was drawn.
#[must_use]
pub(crate) fn draw_transparent(
    pass: &Pass<'_>,
    frame: &mut Frame<'_>,
    masked: &mut [MaskedMiddle],
    sprites: &mut [SpriteItem],
) -> Counts {
    masked.sort_by_key(|item| Reverse(item.depth16));
    sprites.sort_by_key(|item| Reverse(item.tz16));
    let mut columns = BTreeSet::new();
    let mut drawn = 0_usize;
    for layer in merged(masked, sprites) {
        if !layer.paint(pass, frame) {
            continue;
        }
        match layer {
            Layer::Masked(item) => {
                columns.insert(item.column);
            }
            Layer::Sprite(_) => drawn = drawn.saturating_add(1),
        }
    }
    Counts {
        masked_columns: columns.len(),
        sprites_drawn: drawn,
    }
}

/// The two lists folded into one far-to-near sequence. The sort is stable
/// and the masked middles go in first, so at an equal depth the masked
/// middle is drawn first and the sprite lands on top of it.
fn merged<'item>(masked: &'item [MaskedMiddle], sprites: &'item [SpriteItem]) -> Vec<Layer<'item>> {
    let mut layers: Vec<Layer<'item>> = masked.iter().map(Layer::Masked).collect();
    layers.extend(sprites.iter().map(Layer::Sprite));
    layers.sort_by_key(|layer| Reverse(layer.depth16()));
    layers
}

/// The item's perpendicular depth in whole map units, floor-rounded and
/// saturated into `i64`: the number the depth test compares against the
/// caster's buffer and the lighting fade reads, the same reading
/// `paint_row` takes for an opaque face.
fn item_depth(depth16: i128) -> i64 {
    i64::try_from(depth16.div_euclid(ONE_16)).unwrap_or(i64::MAX)
}

/// The integer texture row a raw-16.16 vertical texture position reads,
/// floor-rounded; `None` outside `0..height`. Masked textures are never
/// tiled vertically, so a position past either edge draws nothing rather
/// than wrapping the way the horizontal texel column does.
fn tex_row(v16: i128, height: u32) -> Option<u32> {
    let row = u32::try_from(v16.div_euclid(ONE_16)).ok()?;
    (row < height).then_some(row)
}

/// The projected size of one map unit at a sprite's depth: pixels per map
/// unit `k = (focal2 / 2) / tz`, kept as an exact `i128` pair and never
/// rounded to whole pixels.
///
/// `k` is `num / den`, so a raw-16.16 map-unit distance times `num` over
/// `den` is the same distance in raw-16.16 pixels. To keep every step
/// exact, projected positions are carried in *scale units* — raw-16.16
/// pixels multiplied by `den` — so [`Scale::units`] and [`Scale::pixels`]
/// are plain saturating multiplications and only the final whole-number
/// screen index divides.
#[derive(Clone, Copy, Debug)]
struct Scale {
    /// Numerator: twice the focal length, raw 16.16 pixels.
    num: i128,
    /// Denominator: twice the item's depth, raw 16.16 map units, always
    /// positive.
    den: i128,
    /// Scale units in one screen pixel: `2^16 * den`.
    pixel: i128,
}

impl Scale {
    /// The scale of an item at perpendicular depth `tz16` (raw 16.16 map
    /// units) under `proj`: `focal2 / (2 * tz16)` pixels per map unit.
    /// `None` for an item at or behind the camera, and for the depths
    /// whose double does not fit: an item the caller has not filtered out
    /// simply draws nothing.
    fn of(proj: &Projection, tz16: i128) -> Option<Self> {
        let den = tz16.checked_mul(2_i128)?;
        (den > 0_i128).then_some(Self {
            num: proj.focal2(),
            den,
            pixel: ONE_16.saturating_mul(den),
        })
    }

    /// A raw-16.16 map-unit distance in scale units: `value * num`, which
    /// is `value * k * den` raw-16.16 pixels.
    const fn units(self, value16: i128) -> i128 {
        value16.saturating_mul(self.num)
    }

    /// A raw-16.16 pixel distance in scale units.
    const fn pixels(self, value16: i128) -> i128 {
        value16.saturating_mul(self.den)
    }

    /// Scale units in one map unit, which is one texel of a patch: `k`
    /// raw-16.16 pixels, carried in scale units.
    const fn texel(self) -> i128 {
        ONE_16.saturating_mul(self.num)
    }
}

/// One projected axis of a sprite patch: where its texel edges land on
/// the screen and which texel a screen pixel reads.
///
/// The patch's span runs from [`Axis::start`] to [`Axis::end`] in scale
/// units, along the axis' own screen direction — columns left to right,
/// rows top to bottom — and texel `j` occupies the span's `j`-th `step`,
/// so a pixel touching the span reads the last texel it overlaps.
#[derive(Clone, Copy, Debug)]
struct Axis {
    /// Scale units from the frame's edge to texel 0's near edge.
    start: i128,
    /// Scale units per texel: one map unit at the item's depth.
    step: i128,
    /// Scale units per screen pixel: `k`'s denominator scaled by `2^16`.
    pixel: i128,
    /// Texels along this axis: the patch's width or height, always at
    /// least 1.
    count: i128,
}

impl Axis {
    /// The screen pixels the span touches inside `0..limit`, each with
    /// the texel it reads, in ascending screen order. Empty when the patch
    /// misses the frame.
    fn pixels(self, limit: i64) -> Vec<(i64, u32)> {
        let low = self
            .start
            .div_euclid(self.pixel)
            .clamp(0_i128, i128::from(i64::MAX));
        let high = strict_last_below(self.end(), self.pixel)
            .min(i128::from(limit.saturating_sub(1_i64)))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        let first = i64::try_from(low).unwrap_or(i64::MAX);
        let last = i64::try_from(high).unwrap_or(-1_i64);
        let mut touched = Vec::new();
        for screen in first..=last {
            touched.push((screen, self.texel_at(screen)));
        }
        touched
    }

    /// Scale units just past the span's far edge.
    const fn end(self) -> i128 {
        self.start
            .saturating_add(self.count.saturating_mul(self.step))
    }

    /// The texel screen pixel `screen` reads: the last one whose span
    /// touches it, clamped into the patch.
    fn texel_at(self, screen: i64) -> u32 {
        let far_edge = i128::from(screen)
            .saturating_add(1_i128)
            .saturating_mul(self.pixel)
            .saturating_sub(self.start);
        clamped_texel(strict_last_below(far_edge, self.step), self.count)
    }
}

/// A sprite item projected onto the raster, ready to paint.
struct Placement {
    /// The screen columns the patch touches, with the texel column each
    /// reads, ascending.
    columns: Vec<(i64, u32)>,
    /// The screen rows the patch touches, with the texel row each reads,
    /// ascending.
    rows: Vec<(i64, u32)>,
    /// The item's perpendicular depth in whole map units: the number the
    /// depth test compares against the caster's buffer and the lighting
    /// fade reads.
    depth: i64,
}

impl Placement {
    /// Projects `item` through `proj`. The centre column is
    /// `W / 2 + tx·k`, the patch's left edge `left_offset·k` to its left
    /// and its top row `horizon - (base + top_offset - eye)·k`: the
    /// caster's own row formula at the item's depth, since
    /// `1 / (2t) = focal2 / (2 * tz)` is exactly `k`. `None` when the
    /// item's depth is not in front of the camera.
    fn of(proj: &Projection, item: &SpriteItem) -> Option<Self> {
        let scale = Scale::of(proj, item.tz16)?;
        let centre = scale
            .pixels(view_centre(proj))
            .saturating_add(scale.units(item.tx16));
        let columns = Axis {
            start: centre.saturating_sub(scale.units(item.left_offset16)),
            step: scale.texel(),
            pixel: scale.pixel,
            count: i128::from(item.image.width),
        }
        .pixels(proj.width());
        let top = i128::from(item.base)
            .saturating_mul(ONE_16)
            .saturating_add(item.top_offset16);
        let rows = Axis {
            start: scale
                .pixels(proj.horizon16())
                .saturating_sub(scale.units(top.saturating_sub(proj.eye16()))),
            step: scale.texel(),
            pixel: scale.pixel,
            count: i128::from(item.image.height),
        }
        .pixels(proj.height());
        Some(Self {
            columns,
            rows,
            depth: item_depth(item.tz16),
        })
    }
}

/// The raw-16.16 pixel column at the middle of the raster: `W / 2`, the
/// edge the caster's centre ray (`u = 0`) passes through.
fn view_centre(proj: &Projection) -> i128 {
    i128::from(proj.width().div_euclid(2)).saturating_mul(ONE_16)
}

/// The largest integer whose `step`-sized multiple still falls strictly
/// below `value` (`step` positive): the number of whole spans a pixel's
/// far edge has passed.
const fn strict_last_below(value: i128, step: i128) -> i128 {
    value.saturating_sub(1_i128).div_euclid(step)
}

/// A texel index clamped into a `count`-texel axis, as a `u32`.
fn clamped_texel(index: i128, count: i128) -> u32 {
    let last = count.saturating_sub(1_i128);
    u32::try_from(index.clamp(0_i128, last)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        Counts, Frame, Layer, Pass, SpriteItem, draw_transparent, item_depth, merged,
        strict_last_below, tex_row,
    };
    use crate::gfx::colormap::TABLE_BYTES;
    use crate::gfx::image::IndexedImage;
    use crate::gfx::palette::Palette;
    use crate::render::canvas::Canvas;
    use crate::render::view::{Fraction, MaskedMiddle, ONE_16, Projection, ViewCamera};
    use crate::render::{Lighting, RenderSettings, Scene, WallDir};
    use crate::wad::map::{Map, MapFormat};
    use crate::wad::name::LumpName;

    /// Columns in the fixture raster.
    const WIDTH: u32 = 32;
    /// Rows in the fixture raster.
    const HEIGHT: u32 = 64;
    /// The unfilled fixture background.
    const BLANK: [u8; 4] = [9_u8, 9, 9, 255];
    /// The fixture camera's eye height, in map units.
    const EYE: i64 = 41;
    /// The raster's centre column, `W / 2`, where a sprite straight ahead
    /// lands.
    const CENTRE: u32 = 16;
    /// The horizon row of a level camera, `H / 2`.
    const HORIZON: u32 = 32;

    /// `value` scaled into raw 16.16.
    fn raw(value: i64) -> i128 {
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
    fn lit(index: u8) -> [u8; 4] {
        [index, 255_u8.wrapping_sub(index), index, 255_u8]
    }

    /// A full-bright scene with an identity `COLORMAP` and the one masked
    /// texture the tests share. Its map, flats and sky are never read:
    /// sprite items carry their own patches.
    fn fixture_scene() -> Scene {
        Scene {
            map: Map::new_empty(
                LumpName::parse("MAP01").expect("test names are valid"),
                MapFormat::Doom,
            ),
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
    fn camera() -> ViewCamera {
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
    fn projection() -> Projection {
        Projection::new(&camera(), WIDTH, HEIGHT)
    }

    /// A `width` by `height` indexed image of solid `index`, fully opaque.
    fn solid(width: u32, height: u32, index: u8) -> IndexedImage {
        let count = usize::try_from(width.saturating_mul(height)).expect("test images fit");
        IndexedImage::new(width, height, vec![index; count], vec![true; count])
            .expect("test images build")
    }

    /// An image whose texel `(j, u)` is palette index `j + u * width`, so
    /// every painted pixel names the texel it read.
    fn gradient(width: u32, height: u32) -> IndexedImage {
        let cells = usize::try_from(width.saturating_mul(height)).expect("test images fit");
        let indices = (0..cells)
            .map(|cell| u8::try_from(cell).expect("test gradients stay inside the palette"))
            .collect();
        IndexedImage::new(width, height, indices, vec![true; cells]).expect("test images build")
    }

    /// A `width` by `height` patch of `index`, transparent from texel
    /// column `clear_from` rightwards.
    fn clear_columns(width: u32, height: u32, index: u8, clear_from: u32) -> IndexedImage {
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
    fn standing(patch: IndexedImage, tz: i64, base: i64) -> SpriteItem {
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
    fn masked_at(column: u32, row: i64, tz: i64) -> MaskedMiddle {
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
    fn draw(
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
    fn paint_sprite(item: SpriteItem) -> Canvas {
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
    fn painted(canvas: &Canvas, column: u32, row: u32) -> bool {
        canvas
            .pixel(column, row)
            .is_some_and(|pixel| pixel != BLANK)
    }

    /// Whether `canvas` holds any painted pixel in screen `column`.
    fn column_painted(canvas: &Canvas, column: u32) -> bool {
        (0..HEIGHT).any(|row| painted(canvas, column, row))
    }

    /// Whether `canvas` holds any painted pixel in screen `row`.
    fn row_painted(canvas: &Canvas, row: u32) -> bool {
        (0..WIDTH).any(|column| painted(canvas, column, row))
    }

    /// The screen columns `canvas` painted over the fixture background.
    fn columns_of(canvas: &Canvas) -> Vec<u32> {
        (0..WIDTH)
            .filter(|column| column_painted(canvas, *column))
            .collect()
    }

    /// The screen rows `canvas` painted over the fixture background.
    fn rows_of(canvas: &Canvas) -> Vec<u32> {
        (0..HEIGHT)
            .filter(|row| row_painted(canvas, *row))
            .collect()
    }

    #[test]
    fn a_sprite_straight_ahead_is_placed_and_mapped_exactly() {
        // Depth 16 under focal2 32 puts k at one pixel per map unit, so
        // the 8x8 patch spans columns 12..20 and rows 24..32, each pixel
        // reading its own texel.
        let canvas = paint_sprite(standing(gradient(8, 8), 16, EYE));
        assert_eq!(
            columns_of(&canvas),
            (12_u32..20_u32).collect::<Vec<u32>>(),
            "centred on column 16"
        );
        assert_eq!(
            rows_of(&canvas),
            (24_u32..32_u32).collect::<Vec<u32>>(),
            "from the eye up by the patch's own height"
        );
        assert_each_texel_lands_on_its_own_pixel(&canvas);
    }

    /// Asserts the fixture sprite's 64 pixels hold, one apiece, the
    /// colours of the gradient texels they project from: texel `(j, u)`
    /// sits at column `12 + j`, row `24 + u`.
    fn assert_each_texel_lands_on_its_own_pixel(canvas: &Canvas) {
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

    #[test]
    fn doubling_the_depth_halves_the_columns_and_the_height() {
        let near = paint_sprite(standing(gradient(8, 8), 16, EYE));
        let far = paint_sprite(standing(gradient(8, 8), 32, EYE));
        assert_eq!(columns_of(&near).len(), 8, "k = 1 eight texels across");
        assert_eq!(
            columns_of(&far),
            (14_u32..18_u32).collect::<Vec<u32>>(),
            "k = 1/2 half as many columns, still centred on 16"
        );
        assert_eq!(rows_of(&near).len(), 8, "eight rows near");
        assert_eq!(rows_of(&far).len(), 4, "half the height far");
    }

    #[test]
    fn an_odd_raster_centres_the_sprite_on_its_middle_column() {
        // 33 columns: the centre is column 16 = 33 / 2 and, at depth 16,
        // k = 33/32, so the 8-unit patch spans 8.25 pixels symmetric about
        // that column and touches 11..=20.
        let scene = fixture_scene();
        let proj = Projection::new(&camera(), 33_u32, HEIGHT);
        let mut canvas = Canvas::new(33_u32, HEIGHT, BLANK);
        let slots = usize::try_from(33_u32.saturating_mul(HEIGHT)).expect("test sizes fit");
        let buffer = vec![i64::MAX; slots];
        let mut sprites = vec![standing(gradient(8, 8), 16, EYE)];
        let counts = draw_transparent(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &buffer),
            &mut [],
            &mut sprites,
        );
        assert_eq!(counts.sprites_drawn, 1, "the sprite paints");
        let columns: Vec<u32> = (0..33_u32)
            .filter(|column| column_painted(&canvas, *column))
            .collect();
        assert_eq!(
            columns,
            (11_u32..21_u32).collect::<Vec<u32>>(),
            "the pixels it touches are symmetric about column 16"
        );
    }

    #[test]
    fn the_top_offset_places_the_patch_top_at_its_world_height() {
        // A 64-unit topoffset on base 0 tops the patch at world height 64;
        // at k = 1 that is row 32 - (64 - 41) = 9 and the 8-tall patch
        // runs down to row 17, never tiling past its own rows.
        let mut item = standing(solid(8, 8, 200), 16, 0);
        item.top_offset16 = raw(64);
        let canvas = paint_sprite(item);
        let rows = rows_of(&canvas);
        assert_eq!(rows.first(), Some(&9_u32), "the projected top row");
        assert_eq!(rows.last(), Some(&16_u32), "eight rows down");
        assert!(
            !painted(&canvas, CENTRE, HORIZON - 24),
            "row 8 is above the patch and keeps the background"
        );
        assert!(
            !painted(&canvas, CENTRE, HORIZON - 15),
            "row 17 is past the patch's last row"
        );
    }

    #[test]
    fn a_sprite_behind_a_closer_wall_pixel_paints_nothing() {
        let scene = fixture_scene();
        let proj = projection();
        let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
        let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
        let wall = vec![10_i64; slots];
        let mut behind = vec![standing(solid(8, 8, 200), 20, EYE)];
        let counts = draw_transparent(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &wall),
            &mut [],
            &mut behind,
        );
        assert_eq!(
            counts,
            Counts::default(),
            "the recorded 10 is nearer than the sprite's 20, everywhere"
        );
        assert!(
            columns_of(&canvas).is_empty(),
            "behind an opaque wall nothing paints"
        );
    }

    #[test]
    fn a_sprite_in_front_of_the_buffer_paints_and_leaves_it_untouched() {
        let scene = fixture_scene();
        let proj = projection();
        let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
        let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
        let open = vec![i64::MAX; slots];
        let before = open.clone();
        let mut in_front = vec![standing(solid(8, 8, 200), 20, EYE)];
        let counts = draw_transparent(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &open),
            &mut [],
            &mut in_front,
        );
        assert_eq!(
            counts.sprites_drawn, 1,
            "sky and background are `i64::MAX` deep, so the sprite is nearer"
        );
        assert!(
            painted(&canvas, CENTRE, 30),
            "in front of the wall the patch paints"
        );
        assert_eq!(open, before, "the pass never writes the buffer");
    }

    #[test]
    fn depth_test_against_a_wall_pixel_keeps_the_wall_and_counts_both_layers() {
        // One pixel stands 15 units deep behind an opaque face: the masked
        // bar at 20 and the sprite at 30 both lose it, and both still
        // paint the open pixels around it.
        let scene = fixture_scene();
        let proj = projection();
        let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
        let mut buffer = vec![i64::MAX; slots];
        let wall =
            usize::try_from(30_u32.saturating_mul(WIDTH).saturating_add(CENTRE)).expect("fits");
        if let Some(slot) = buffer.get_mut(wall) {
            *slot = 15;
        }
        let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
        let mut bar = masked_at(CENTRE, 30, 20);
        bar.top = 29;
        let sprite = standing(solid(8, 8, 111), 30, EYE);
        let counts = draw_transparent(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &buffer),
            &mut [bar],
            &mut [sprite],
        );
        assert_eq!(
            canvas.pixel(CENTRE, 30),
            Some(BLANK),
            "the nearer wall keeps its own pixel"
        );
        assert_eq!(
            counts,
            Counts {
                masked_columns: 1,
                sprites_drawn: 1
            },
            "both layers painted their other pixels"
        );
    }

    #[test]
    fn a_nearer_sprite_covers_a_masked_column_on_the_same_pixel() {
        let masked = masked_at(CENTRE, 30, 40);
        let sprite = standing(solid(8, 8, 111), 30, EYE);
        let (canvas, counts) = draw(&mut [masked], &mut [sprite], i64::MAX);
        assert_eq!(
            counts,
            Counts {
                masked_columns: 1,
                sprites_drawn: 1
            },
            "both layers gained pixels"
        );
        assert_eq!(
            canvas.pixel(CENTRE, 30),
            Some(lit(111)),
            "the nearer sprite covers the farther bar"
        );
    }

    #[test]
    fn a_nearer_masked_column_covers_a_farther_sprite() {
        let masked = masked_at(CENTRE, 30, 20);
        let sprite = standing(solid(8, 8, 111), 30, EYE);
        let (canvas, counts) = draw(&mut [masked], &mut [sprite], i64::MAX);
        assert_eq!(
            canvas.pixel(CENTRE, 30),
            Some(lit(64)),
            "the nearer bar wins the shared pixel"
        );
        assert_eq!(counts.masked_columns, 1, "the masked column painted");
        assert_eq!(counts.sprites_drawn, 1, "and the sprite painted elsewhere");
    }

    #[test]
    fn clear_texels_leave_the_pixels_behind_a_sprite() {
        // The 8-wide patch at k = 1 covers columns 12..20 and is opaque
        // over its first two texel columns, so only 12 and 13 paint.
        let canvas = paint_sprite(standing(clear_columns(8, 8, 111, 2), 16, EYE));
        assert_eq!(
            canvas.pixel(12, 24),
            Some(lit(111)),
            "texel column 0 is opaque and paints"
        );
        assert_eq!(canvas.pixel(13, 24), Some(lit(111)), "so is texel column 1");
        assert_eq!(
            canvas.pixel(14, 24),
            Some(BLANK),
            "texel column 2 is a hole: what stands behind shows through"
        );
        assert_eq!(
            canvas.pixel(19, 24),
            Some(BLANK),
            "and so are the rest of them"
        );
        assert_eq!(
            rows_of(&canvas),
            (24_u32..32_u32).collect::<Vec<u32>>(),
            "the opaque band runs the patch's full height"
        );
    }

    #[test]
    fn a_sprite_wider_than_the_frame_is_clipped_to_it() {
        // The 40-unit patch at k = 1 spans columns -4..36: the frame sees
        // 0..32 of them, and each visible column still reads the texel its
        // own span ends in.
        let canvas = paint_sprite(standing(gradient(40, 4), 16, EYE));
        let all: Vec<u32> = (0..WIDTH).collect();
        assert_eq!(columns_of(&canvas), all, "the whole width");
        assert_eq!(
            rows_of(&canvas),
            (28_u32..32_u32).collect::<Vec<u32>>(),
            "four rows up from the eye"
        );
        assert_eq!(
            canvas.pixel(0, 28),
            Some(lit(4)),
            "column 0 shows texel 4: the patch starts four pixels off-screen"
        );
        assert_eq!(
            canvas.pixel(31, 28),
            Some(lit(35)),
            "the last column ends inside texel 35"
        );
    }

    #[test]
    fn a_sprite_outside_the_frame_paints_nothing() {
        let far_right = SpriteItem {
            tx16: raw(1_000),
            ..standing(solid(8, 8, 200), 16, EYE)
        };
        let under_the_floor = standing(solid(8, 8, 200), 16, 1);
        let (canvas, counts) = draw(&mut [], &mut [far_right, under_the_floor], i64::MAX);
        assert_eq!(
            counts,
            Counts::default(),
            "neither a sprite off to the side nor one below the frame draws"
        );
        assert!(
            columns_of(&canvas).is_empty(),
            "and no pixel of the frame changed"
        );
    }

    #[test]
    fn items_at_or_behind_the_camera_draw_nothing() {
        let behind = standing(solid(8, 8, 200), -16, EYE);
        let mut on_plane = behind.clone();
        on_plane.tz16 = 0_i128;
        let mut past_the_range = behind.clone();
        past_the_range.tz16 = i128::MAX;
        let (canvas, counts) = draw(&mut [], &mut [behind, on_plane, past_the_range], i64::MAX);
        assert_eq!(
            counts,
            Counts::default(),
            "nothing in the list gained a pixel"
        );
        assert!(
            columns_of(&canvas).is_empty(),
            "and no pixel changed either"
        );
    }

    #[test]
    fn a_minified_sprite_reads_the_texel_its_pixel_ends_in() {
        // k = 1/8 at depth 128: the 8-wide patch is one pixel across and
        // the 8-tall patch one row, and each pixel takes the last texel
        // whose span it touches.
        let canvas = paint_sprite(standing(gradient(8, 8), 128, EYE));
        assert_eq!(
            columns_of(&canvas),
            vec![15_u32, CENTRE],
            "one pixel across, and it straddles the centre column 16"
        );
        assert_eq!(
            rows_of(&canvas),
            vec![HORIZON - 1],
            "one row tall, ending at the eye's row"
        );
        assert_eq!(
            canvas.pixel(CENTRE, HORIZON - 1),
            Some(lit(63)),
            "the pixel's far edge has passed every texel boundary but the last"
        );
        assert_eq!(
            canvas.pixel(15, HORIZON - 1),
            Some(lit(59)),
            "and its neighbour ends inside texel column 3"
        );
    }

    #[test]
    fn a_magnified_sprite_repeats_one_texel_over_several_pixels() {
        // k = 2 at depth 8: one texel is two pixels wide, and a pixel that
        // straddles a boundary takes the texel it ends in.
        let canvas = paint_sprite(standing(gradient(8, 8), 8, EYE));
        assert_eq!(
            columns_of(&canvas),
            (8_u32..24_u32).collect::<Vec<u32>>(),
            "sixteen pixels across"
        );
        assert_eq!(
            canvas.pixel(8, 16),
            Some(lit(0)),
            "the first pixel ends inside texel 0"
        );
        assert_eq!(
            canvas.pixel(9, 16),
            Some(lit(0)),
            "and so does the one after it"
        );
        assert_eq!(
            canvas.pixel(10, 16),
            Some(lit(1)),
            "then texel 1 takes over"
        );
        assert_eq!(
            canvas.pixel(23, 16),
            Some(lit(7)),
            "the last pixel ends in the last texel"
        );
    }

    #[test]
    fn the_merged_order_is_far_to_near_with_masked_first_on_ties() {
        let masked = vec![masked_at(1, 1, 10), masked_at(0, 0, 90)];
        let sprite = standing(solid(1, 1, 1), 40, EYE);
        let sprites = vec![sprite];
        let order: Vec<i128> = merged(&masked, &sprites)
            .into_iter()
            .map(Layer::depth16)
            .collect();
        assert_eq!(
            order,
            vec![raw(90), raw(40), raw(10)],
            "one descending ordering over both kinds"
        );
        let tied = vec![masked_at(2, 2, 40)];
        let kinds: Vec<bool> = merged(&tied, &sprites)
            .iter()
            .map(|layer| matches!(layer, Layer::Sprite(_)))
            .collect();
        assert_eq!(
            kinds,
            vec![false, true],
            "an equal depth keeps the sprite on top"
        );
    }

    #[test]
    fn texture_rows_round_down_and_are_never_tiled() {
        assert_eq!(tex_row(raw(3), 128), Some(3), "a plain row");
        assert_eq!(
            tex_row(raw(4).saturating_sub(1), 128),
            Some(3),
            "the last fraction of a unit rounds down"
        );
        assert_eq!(tex_row(raw(4), 4), None, "one past the last row is no row");
        assert_eq!(tex_row(-raw(1), 4), None, "above the first row is no row");
        assert_eq!(tex_row(raw(0), 0), None, "an empty image has no rows");
    }

    #[test]
    fn depths_floor_to_whole_map_units_and_saturate() {
        assert_eq!(item_depth(raw(50)), 50, "exact units");
        assert_eq!(
            item_depth(raw(51).saturating_sub(1)),
            50,
            "a partial unit rounds down"
        );
        assert_eq!(item_depth(-raw(1)), -1, "below zero keeps the floor");
        assert_eq!(
            item_depth(raw(i64::MAX).saturating_add(ONE_16)),
            i64::MAX,
            "a depth past i64::MAX units saturates"
        );
    }

    #[test]
    fn nearer_reads_the_buffer_in_row_major_order() {
        let mut canvas = Canvas::new(4, 2, [0_u8, 0, 0, 255]);
        let depth = vec![i64::MAX; 8_usize];
        let frame = Frame::new(&mut canvas, &depth);
        assert!(frame.nearer(3, 1, 100), "anything beats the open buffer");
        assert!(
            !frame.nearer(3, 1, i64::MAX),
            "never strictly nearer than i64::MAX"
        );
        assert!(
            !frame.nearer(4, 1, 100),
            "a column past the row's end names no slot"
        );
        assert!(!frame.nearer(0, 2, 100), "below the frame is nothing");
        assert!(!frame.nearer(0, -1, 100), "above the frame is nothing");
    }

    #[test]
    fn a_span_edge_belongs_to_the_span_it_has_not_reached() {
        assert_eq!(
            strict_last_below(raw(4), raw(1)),
            3,
            "four whole spans: the last one starts at index 3"
        );
        assert_eq!(
            strict_last_below(raw(4).saturating_sub(1), raw(1)),
            3,
            "one fraction short of a boundary has passed the previous one"
        );
        assert_eq!(
            strict_last_below(raw(1), raw(4)),
            0,
            "a quarter of the way in is still index 0"
        );
        assert_eq!(
            strict_last_below(0, raw(1)),
            -1,
            "at the very start no span lies wholly behind"
        );
        assert_eq!(
            strict_last_below(raw(-1), raw(1)),
            -2,
            "left of the start the index keeps counting down"
        );
    }
}
