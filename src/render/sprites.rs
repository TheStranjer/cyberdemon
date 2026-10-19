//! Transparent pass: the masked middle textures painted far to near,
//! depth-tested against the column caster's buffer.
//!
//! [`crate::render::view`] walks every screen column near to far and
//! collects each masked middle texture seen through a two-sided line,
//! fully keyed, mapped, placed and lit, in [`MaskedMiddle`] items. This
//! module draws them: sort the items by their perpendicular depth
//! descending, then paint each item's window rows through the caster's
//! depth buffer, so a pixel takes the texture's colour only when nothing
//! nearer already owns it. The texture row of a screen row is the
//! caster's own projection maths (`v = tex_top + y_off - height`); rows
//! that fall outside the image keep whatever stands behind them, because
//! masked textures are never tiled vertically, and texels the image
//! marks non-opaque are holes through which the painted geometry shows.
//! The pass never writes the depth buffer, so everything a masked pixel
//! covers keeps its own depth, and it answers with the number of screen
//! columns that gained at least one pixel,
//! [`crate::render::view::Rendered::masked_columns`]. Later sprite
//! drawing joins the same item list and shares this sort and this depth
//! test.

use alloc::collections::BTreeSet;
use core::cmp::Reverse;

use crate::gfx::image::IndexedImage;
use crate::render::canvas::Canvas;
use crate::render::light::{colormap_index, shade};
use crate::render::scene::Scene;
use crate::render::view::{MaskedMiddle, ONE_16, Projection};

/// The transparent pass's read-only context: the scene whose textures the
/// items key into and the caster's projection that turns a screen row
/// into a world height.
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

    /// Paints one item's window rows; answers whether it painted at
    /// least one pixel. An item whose texture key the scene does not
    /// have draws nothing at all.
    fn paint_item(&self, frame: &mut Frame<'_>, item: &MaskedMiddle) -> bool {
        let Some(image) = self.scene.textures.get(&item.texture) else {
            return false;
        };
        let depth = item_depth(item.depth16);
        let run = Run {
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
    fn paint_texel(&self, frame: &mut Frame<'_>, run: &Run<'_>, row: i64) -> bool {
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

/// One item readied for painting: its looked-up image, the whole-unit
/// depth the test and the `Doom` lighting fade read, the one lighting
/// table for the whole item and the depth-buffer column index.
struct Run<'item> {
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

/// Draws the collected masked middles, far to near: the items are depth
/// sorted, then each one's window rows are depth-tested against the
/// caster's buffer and painted from the scene's textures. Answers with
/// the number of distinct screen columns that gained at least one
/// pixel: a column with several masked items counts once when any of
/// them paints.
///
/// `masked` is sorted in place, by [`MaskedMiddle::depth16`] descending,
/// so after the call the list reads in the order it was drawn.
#[must_use]
pub(crate) fn draw_masked(
    pass: &Pass<'_>,
    frame: &mut Frame<'_>,
    masked: &mut [MaskedMiddle],
) -> usize {
    masked.sort_by_key(|item| Reverse(item.depth16));
    let mut columns = BTreeSet::new();
    for item in masked {
        if pass.paint_item(frame, item) {
            columns.insert(item.column);
        }
    }
    columns.len()
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

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{Frame, item_depth, tex_row};
    use crate::render::canvas::Canvas;
    use crate::render::view::ONE_16;

    /// `value` scaled into raw 16.16.
    fn raw(value: i64) -> i128 {
        i128::from(value).saturating_mul(ONE_16)
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
}
