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
//!
//! The maths that places a sprite — the `k` scale, the two projected axes
//! and the `Placement` they answer with — lives in the private
//! `placement` module, which this module reads through the one
//! `Placement::of` call its `Pass` makes; the pass's own tests are the
//! private `tests` module, its fixture builders shared, the placement
//! cases and the texel-reading cases one topic file apiece.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cmp::Reverse;

use crate::gfx::image::IndexedImage;
use crate::render::canvas::Canvas;
use crate::render::light::{WallDir, colormap_index, shade};
use crate::render::scene::Scene;
use crate::render::view::{MaskedMiddle, ONE_16, Projection};
use placement::Placement;

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

mod placement;

#[cfg(test)]
mod tests;
