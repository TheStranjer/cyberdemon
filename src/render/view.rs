//! Column ray caster for the 3D view: textured walls, world-aligned
//! textured floors and ceilings and a view-angled sky.
//!
//! The camera stands at an explicit `x`/`y`/`z` and looks `angle` degrees
//! counter-clockwise from east, `pitch` degrees up from level, through a
//! `fov` degree field onto a `width` by `height` raster. No `BSP` is
//! needed: every column ray is intersected with every linedef, the hits
//! are sorted by ray parameter and walked near-to-far (a portal caster).
//! The walk keeps the window of rows still uncovered; each hit first
//! fills that window with the faced sector's ceiling and floor, sampled
//! from the flat at the world point the row's exact ray parameter lands
//! on (`Doom`'s world-grid alignment, shaded by that row's plane depth),
//! and then paints the wall face its facing sidedef carries. An `F_SKY1`
//! plane draws the sky texture instead, bucketed by the column's ray
//! angle and placed with `Doom`'s 320x200 sky rows, full bright (or the
//! background when the sky switch is off). Masked middle textures seen
//! through two-sided lines are collected, fully keyed, mapped, placed,
//! lit and stamped with the exact ray parameter, in [`Rendered::masked`]
//! and then painted by the render module's transparent pass far to near,
//! in one ordering with the [`crate::render::sprites::SpriteItem`]s a
//! caller has prepared for it. Every painted pixel also records its
//! perpendicular depth in map units in [`Rendered::depth`] (background
//! and sky keep `i64::MAX`), the buffer that pass depth-tests against
//! without ever writing it.
//!
//! All maths is integer. The camera's `cos`/`sin` are 16.16
//! ([`crate::fixed::Angle`]), column directions are scaled by `2^32`, and
//! ray parameters stay exact `i128` fractions until a screen row or map
//! depth is floor-rounded out of them.
//!
//! The module splits by job: `projection` holds [`Fraction`] and the
//! camera's `Projection`, `walk` the shapes one column moves through (its
//! prepared `Edge`s and `Side`s, the `Plane` facts of a sector, the `Hit`
//! and `Visit` of a crossing, the `Window` of rows still uncovered and the
//! `FlatRun`, `FacePlan` and `Part` rows painted from them), `caster` the
//! `Caster` that sorts and visits one column's hits, and `texels` the
//! mapping helpers that turn a crossing into the texel, depth and colour
//! each row reads. This module root is all the rest of the crate names:
//! the pitch limits, the camera, the collected `MaskedMiddle`, the
//! [`Rendered`] frame and the two entry points.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::CyberError;
use crate::render::canvas::{Canvas, Rgba};
use crate::render::light::WallDir;
use crate::render::scene::Scene;
use crate::render::sprites;

/// Lowest accepted pitch, in degrees.
pub const MIN_PITCH: i32 = -60;
/// Highest accepted pitch, in degrees.
pub const MAX_PITCH: i32 = 60;
/// The texture name that means "draw nothing here".
const TEXTURE_NONE: &str = "-";
/// The flat that means "sky" for a ceiling or a floor;
/// [`crate::render::scene::SKY_FLAT`] spells the same constant for scene
/// builders.
const SKY_CEILING: &str = "F_SKY1";
/// One whole map unit or pixel in raw 16.16. Also the divisor the
/// transparent pass uses to floor its carried depths and texture rows
/// out of raw positions.
pub(crate) const ONE_16: i128 = 0x1_0000;
/// Half of a raw 16.16 unit.
const HALF_16: i128 = 0x8000;
/// Column directions are scaled by `2^32` (`cos * focal2` in 16.16), so
/// a raw intersection numerator must take that shift before it is a ray
/// parameter whose perpendicular depth is `t * focal2` map units.
const DIRECTION_SCALE: i128 = ONE_16.saturating_mul(ONE_16);
/// The opaque black fallback for an unparsable background colour.
const BLACK: Rgba = [0, 0, 0, 255];

/// Refuses a pitch outside `MIN_PITCH..=MAX_PITCH`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the range.
#[inline]
pub fn check_pitch(pitch: i32) -> Result<(), CyberError> {
    if (MIN_PITCH..=MAX_PITCH).contains(&pitch) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render_view pitch must be within {MIN_PITCH}..={MAX_PITCH} degrees, got {pitch}"
    )))
}

/// The camera of one `render_view` render.
#[derive(Clone, Copy, Debug)]
pub struct ViewCamera {
    /// World `x` in map units.
    pub x: i32,
    /// World `y` in map units.
    pub y: i32,
    /// Eye height in map units.
    pub z: i32,
    /// Facing in degrees, counter-clockwise from east.
    pub angle: i32,
    /// Pitch in degrees, positive looking up, `MIN_PITCH..=MAX_PITCH`.
    pub pitch: i32,
    /// Horizontal field of view in degrees, `MIN_FOV..=MAX_FOV`.
    pub fov: u32,
}

/// A masked middle texture seen through a two-sided line.
///
/// Carries everything the transparent pass needs to draw it without
/// re-walking the column: the texture key and its wrapped column, the
/// mapped top and vertical shift, the lighting and fake contrast, the
/// exact ray parameter, and the depth and window rows to respect.
#[derive(Clone, Debug)]
pub struct MaskedMiddle {
    /// The screen column the part sits in.
    pub column: u32,
    /// The linedef index the faced side belongs to.
    pub line: usize,
    /// The faced sidedef index.
    pub side: usize,
    /// The first still-uncovered row at the crossing.
    pub top: i64,
    /// The last still-uncovered row at the crossing.
    pub bottom: i64,
    /// The perpendicular depth of the crossing, raw 16.16 map units.
    pub depth16: i128,
    /// The [`Scene::textures`] key of the texture to draw.
    pub texture: String,
    /// The texture column at the crossing, already wrapped, in texels.
    pub texcol: u32,
    /// The world height (raw 16.16) whose texture row is `v = 0`.
    pub tex_top16: i128,
    /// Vertical texture shift, raw 16.16.
    pub y_off16: i128,
    /// The sector light level shading the face.
    pub light: i32,
    /// The fake-contrast orientation of the linedef.
    pub dir: WallDir,
    /// The exact ray parameter of the crossing, for the height at each
    /// row.
    pub t: Fraction,
}

/// The picture one render produced, plus the data the next pass needs.
#[derive(Debug)]
pub struct Rendered {
    /// The painted view (gamma is the caller's step).
    pub canvas: Canvas,
    /// Masked middles seen behind two-sided lines, sorted far to near
    /// by the transparent pass that drew them (see
    /// [`crate::render::sprites`]).
    pub masked: Vec<MaskedMiddle>,
    /// Screen columns the transparent pass painted at least one masked
    /// pixel in; a column carrying several masked items counts once.
    /// Zero when the pass drew nothing.
    pub masked_columns: usize,
    /// Sprites the transparent pass painted at least one pixel of. Zero
    /// unless [`render_frame`] was given [`crate::render::sprites::SpriteItem`]s
    /// that gained pixels.
    pub sprites_drawn: usize,
    /// Per-pixel perpendicular depth in map units, row-major
    /// `width * height`, written for every wall and plane pixel;
    /// background and sky pixels keep `i64::MAX`. Internal to the
    /// renderer: the masked and sprite passes depth-test against it.
    pub depth: Vec<i64>,
}

/// The raster one [`render_frame`] paints, with the caller's sprite
/// layers folded into its transparent pass.
///
/// The column walk collects its own [`MaskedMiddle`] list;
/// [`ViewFrame::sprites`] carries the [`crate::render::sprites::SpriteItem`]s
/// a caller resolved from the map's things before the cast. The plain
/// [`render_view`] is this frame with no sprites.
#[derive(Debug)]
pub struct ViewFrame {
    /// Raster width in pixels.
    pub width: u32,
    /// Raster height in pixels.
    pub height: u32,
    /// The prepared sprite items, folded into the transparent pass.
    pub sprites: Vec<sprites::SpriteItem>,
}

/// Renders the 3D view of `scene` from `camera` onto a `width` by
/// `height` raster, with no sprite layers in the transparent pass.
///
/// The caller is expected to have checked that the camera lies inside a
/// sector (`crate::render::camera_sector`); a camera in no sector simply
/// gets no sector data to fill with and renders what the hits still
/// cover. After the column walk the collected [`MaskedMiddle`] list and
/// the [`Rendered::depth`] buffer go through the render module's
/// transparent pass; [`render_frame`] is the same cast with prepared
/// [`crate::render::sprites::SpriteItem`]s folded into that pass.
#[must_use]
#[inline]
pub fn render_view(scene: &Scene, camera: &ViewCamera, width: u32, height: u32) -> Rendered {
    render_frame(
        scene,
        camera,
        ViewFrame {
            width,
            height,
            sprites: Vec::new(),
        },
    )
}

/// Renders the 3D view of `scene` from `camera` onto `frame`'s raster,
/// folding [`ViewFrame::sprites`] into the transparent pass.
///
/// The same cast as [`render_view`]: after the column walk the collected
/// [`MaskedMiddle`] list, the caller's [`crate::render::sprites::SpriteItem`]s
/// and the [`Rendered::depth`] buffer go through the render module's
/// transparent pass, so the view shows masked middle textures and sprites
/// painted far to near through one ordering and depth-tested against a
/// buffer the result still carries.
#[must_use]
#[inline]
pub fn render_frame(scene: &Scene, camera: &ViewCamera, frame: ViewFrame) -> Rendered {
    let ViewFrame {
        width,
        height,
        mut sprites,
    } = frame;
    let caster = Caster::new(scene, camera, width, height);
    let mut canvas = Canvas::new(width, height, caster.background);
    let rows = i64::from(canvas.height());
    let mut depth = vec![i64::MAX; pixel_area(&canvas)];
    let mut masked = Vec::new();
    for column in 0_i64..i64::from(canvas.width()) {
        let d = caster.proj.direction(column);
        let mut walk = Walk {
            canvas: &mut canvas,
            masked: &mut masked,
            depth: &mut depth,
            column,
            d,
            skycol: sky_column(d, scene.sky.width),
            window: Window::open(rows),
        };
        caster.draw_column(&mut walk);
    }
    let counts = sprites::draw_transparent(
        &sprites::Pass::new(scene, &caster.proj),
        &mut sprites::Frame::new(&mut canvas, &depth),
        &mut masked,
        &mut sprites,
    );
    Rendered {
        canvas,
        masked,
        masked_columns: counts.masked_columns,
        sprites_drawn: counts.sprites_drawn,
        depth,
    }
}

/// The number of pixels (and depth slots) a canvas holds.
fn pixel_area(canvas: &Canvas) -> usize {
    let pixels = u64::from(canvas.width()).saturating_mul(u64::from(canvas.height()));
    usize::try_from(pixels).unwrap_or(0)
}

mod caster;
pub mod projection;
mod texels;
mod walk;

#[cfg(test)]
mod tests;

use caster::Caster;
pub use projection::Fraction;
pub(crate) use projection::Projection;
use texels::sky_column;
use walk::{Walk, Window};
