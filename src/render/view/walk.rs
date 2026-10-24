//! The rows one column's walk moves through: the [`Edge`]s and [`Side`]s
//! prepared from the map, the [`Plane`] facts of a sector and the
//! [`Surface`] each of its two planes draws, the [`Hit`] of a ray on an
//! edge and the [`Visit`] that carries it, the [`Window`] of rows still
//! uncovered in the [`Walk`] state, and the [`FlatRun`], [`FacePlan`] and
//! [`Part`] rows painted from them.
//!
//! Nothing here casts a ray or picks a texel: the geometry and the
//! texturing live in the sibling `caster` and `texels` modules. These are
//! the shapes both halves hand each other, together with the two moves a
//! column's window makes ([`Window::closed`] and [`Window::seal`]) and
//! [`Walk::paint`], the one place a pixel and its depth land together.

use alloc::string::String;
use alloc::vec::Vec;

use super::projection::Fraction;
use super::texels::picture_key;
use super::{MaskedMiddle, SKY_CEILING};
use crate::gfx::image::IndexedImage;
use crate::render::canvas::{Canvas, Rgba};
use crate::render::light::WallDir;
use crate::render::scene::Scene;
use crate::wad::map::Map;

/// One linedef prepared for casting.
pub(super) struct Edge {
    /// The linedef index.
    pub(super) line: usize,
    /// `v1` x.
    pub(super) x0: i64,
    /// `v1` y.
    pub(super) y0: i64,
    /// `v1 -> v2` delta x.
    pub(super) ex: i64,
    /// `v1 -> v2` delta y.
    pub(super) ey: i64,
    /// Segment length in raw 16.16 map units.
    pub(super) len16: i128,
    /// Fake-contrast orientation.
    pub(super) dir: WallDir,
    /// The front sidedef index (resolved at preparation).
    pub(super) front: usize,
    /// The back sidedef index when the line carries a resolvable one.
    pub(super) back: Option<usize>,
    /// Whether the upper texture is unpegged (flag bit 8).
    pub(super) upper_unpegged: bool,
    /// Whether the lower texture is unpegged (flag bit 16).
    pub(super) lower_unpegged: bool,
}

/// One sidedef prepared for casting: its sector's planes and its usable
/// texture keys.
pub(super) struct Side<'scene> {
    /// The sector this side opens into.
    pub(super) plane: Plane<'scene>,
    /// Upper texture key, `None` for `-` or empty.
    pub(super) upper: Option<String>,
    /// Lower texture key.
    pub(super) lower: Option<String>,
    /// Middle texture key.
    pub(super) middle: Option<String>,
    /// Horizontal texture shift in map units.
    pub(super) x_offset: i64,
    /// Vertical texture shift in map units.
    pub(super) y_offset: i64,
}

/// What one ceiling or floor draws.
#[derive(Clone, Copy, Debug)]
pub(super) enum Surface<'scene> {
    /// The flat's image, sampled at the world point the row lands on.
    Flat(&'scene IndexedImage),
    /// The sky texture: an `F_SKY1` plane with the sky switch on.
    Sky,
}

/// The view-relevant facts of one sector.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plane<'scene> {
    /// Floor height in map units.
    pub(super) floor: i64,
    /// Ceiling height in map units.
    pub(super) ceil: i64,
    /// Sector light level.
    pub(super) light: i32,
    /// The floor's picture, `None` when there is nothing to draw (no
    /// flat, or the sky switch off on an `F_SKY1` floor).
    pub(super) floor_surface: Option<Surface<'scene>>,
    /// The ceiling's picture, the same rules; an `F_SKY1` ceiling with
    /// the sky on draws [`Surface::Sky`].
    pub(super) ceil_surface: Option<Surface<'scene>>,
    /// Whether the ceiling flat names the sky; the upper-wall sky hack
    /// reads this even when the sky switch is off.
    pub(super) ceil_sky: bool,
}

impl<'scene> Plane<'scene> {
    /// The plane facts of sector `index`; a sector the map lacks reads
    /// as an empty void (zero heights, nothing to draw).
    pub(super) fn of(
        map: &Map,
        index: usize,
        scene: &'scene Scene,
        sky: Option<Surface<'scene>>,
    ) -> Self {
        let Some(sector) = map.sectors.get(index) else {
            return Self {
                floor: 0,
                ceil: 0,
                light: 0,
                floor_surface: None,
                ceil_surface: None,
                ceil_sky: false,
            };
        };
        let floor_surface = plane_surface(scene, &sector.floor_flat, sky);
        let ceiling_flat = &sector.ceiling_flat;
        Self {
            floor: i64::from(sector.floor_height),
            ceil: i64::from(sector.ceiling_height),
            light: sector.light,
            floor_surface,
            ceil_surface: plane_surface(scene, ceiling_flat, sky),
            ceil_sky: picture_key(ceiling_flat) == SKY_CEILING,
        }
    }
}

/// The surface a plane named `name` draws: the sky for an `F_SKY1` name
/// (only when the sky switch offers `sky`), the named flat's image when
/// the scene loaded one, and nothing otherwise.
fn plane_surface<'scene>(
    scene: &'scene Scene,
    name: &str,
    sky: Option<Surface<'scene>>,
) -> Option<Surface<'scene>> {
    let key = picture_key(name);
    if key == SKY_CEILING {
        return sky;
    }
    scene.flats.get(&key).map(Surface::Flat)
}

/// One sorted ray/linedef intersection.
pub(super) struct Hit {
    /// The edge's index in [`super::caster::Caster::edges`].
    pub(super) edge: usize,
    /// The hit's ray parameter, exact.
    pub(super) t: Fraction,
    /// The position along `v1 -> v2`, exact and inside `0..=1`.
    pub(super) s: Fraction,
    /// Whether the ray crossed the linedef's front side.
    pub(super) front: bool,
    /// The sidedef the ray faces.
    pub(super) side: usize,
    /// The sidedef on the far side of a transparent crossing.
    pub(super) opposite: Option<usize>,
}

/// The hit under visit: prepared facts and the projections that drive
/// the column window.
pub(super) struct Visit<'visit> {
    /// The crossed edge.
    pub(super) edge: &'visit Edge,
    /// The faced sidedef index.
    pub(super) side_index: usize,
    /// The faced sidedef.
    pub(super) side: &'visit Side<'visit>,
    /// The far sidedef for a transparent crossing.
    pub(super) far_side: Option<&'visit Side<'visit>>,
    /// The ray parameter.
    pub(super) t: Fraction,
    /// The position along the segment.
    pub(super) s: Fraction,
    /// Whether the faced side is the linedef's front.
    pub(super) front: bool,
    /// Where the faced sector's ceiling projects.
    pub(super) y_ceil: i64,
    /// Where the faced sector's floor projects.
    pub(super) y_floor: i64,
    /// Where the far sector's ceiling projects.
    pub(super) y_far_ceil: Option<i64>,
    /// Where the far sector's floor projects.
    pub(super) y_far_floor: Option<i64>,
}

/// The rows of one column still waiting to be covered.
#[derive(Clone, Copy, Debug)]
pub(super) struct Window {
    /// First uncovered row.
    pub(super) top: i64,
    /// Last uncovered row.
    pub(super) bottom: i64,
}

impl Window {
    /// The whole `height`-row column.
    pub(super) const fn open(height: i64) -> Self {
        Self {
            top: 0,
            bottom: height.saturating_sub(1),
        }
    }

    /// Whether nothing is left to draw.
    pub(super) const fn closed(self) -> bool {
        self.top > self.bottom
    }

    /// Ends the column.
    pub(super) const fn seal(&mut self) {
        self.top = 1_i64;
        self.bottom = 0_i64;
    }
}

/// The mutable state of one column's walk.
pub(super) struct Walk<'walk> {
    /// Pixels so far.
    pub(super) canvas: &'walk mut Canvas,
    /// Masked middles collected this call.
    pub(super) masked: &'walk mut Vec<MaskedMiddle>,
    /// The frame's per-pixel depth buffer, written as pixels paint.
    pub(super) depth: &'walk mut Vec<i64>,
    /// The screen column being walked.
    pub(super) column: i64,
    /// The column's `2^32`-scaled ray direction.
    pub(super) d: (i128, i128),
    /// The sky texture column this ray's angle selects.
    pub(super) skycol: u32,
    /// The rows still uncovered.
    pub(super) window: Window,
}

impl Walk<'_> {
    /// Plots one pixel and records its perpendicular depth; rows outside
    /// the frame are dropped by the canvas and the depth index alike.
    pub(super) fn paint(&mut self, row: i64, color: Rgba, depth: i64) {
        self.canvas.set(self.column, row, color);
        let Some(index) = self.depth_index(row) else {
            return;
        };
        if let Some(slot) = self.depth.get_mut(index) {
            *slot = depth;
        }
    }

    /// The flat depth-buffer slot of this column and `row`, `None`
    /// outside the frame.
    fn depth_index(&self, row: i64) -> Option<usize> {
        let column = usize::try_from(self.column).ok()?;
        let line = usize::try_from(row).ok()?;
        let width = usize::try_from(self.canvas.width()).ok()?;
        let index = line.checked_mul(width)?.checked_add(column)?;
        (index < self.depth.len()).then_some(index)
    }
}

/// Which texture slot of the faced side a wall part draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    /// The upper texture band.
    Upper,
    /// The lower texture band.
    Lower,
    /// The middle texture (solid on one-sided lines).
    Middle,
}

/// One ceiling or floor run to fill: a column's rows of one plane.
pub(super) struct FlatRun<'scene> {
    /// First row.
    pub(super) top: i64,
    /// Last row.
    pub(super) bottom: i64,
    /// The picture the plane draws.
    pub(super) surface: Surface<'scene>,
    /// Plane height in map units.
    pub(super) height: i64,
    /// Sector light level.
    pub(super) light: i32,
}

/// Everything one textured face needs: geometry, texture and mapping.
pub(super) struct FacePlan<'plan> {
    /// The hit under visit.
    pub(super) visit: &'plan Visit<'plan>,
    /// Which slot is drawing.
    pub(super) slot: Slot,
    /// The texture image.
    pub(super) image: &'plan IndexedImage,
    /// The window rows the face covers.
    pub(super) span: (i64, i64),
    /// The perpendicular depth, raw 16.16 map units.
    pub(super) depth16: i128,
}

/// One textured wall run: mapped, shaded and ready to paint row by row.
pub(super) struct Part<'tex> {
    /// The texture image.
    pub(super) image: &'tex IndexedImage,
    /// First row.
    pub(super) top: i64,
    /// Last row.
    pub(super) bottom: i64,
    /// Texture column, already wrapped, in texels.
    pub(super) texcol: u32,
    /// World height (raw 16.16) whose texture row is `v = 0`.
    pub(super) tex_top16: i128,
    /// Vertical texture shift, raw 16.16.
    pub(super) y_off16: i128,
    /// Sector light shading the face.
    pub(super) light: i32,
    /// Fake contrast of the linedef.
    pub(super) dir: WallDir,
    /// Perpendicular depth, raw 16.16 map units.
    pub(super) depth16: i128,
    /// The ray parameter, for the height at each row.
    pub(super) t: Fraction,
}
