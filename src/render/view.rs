//! Column ray caster for the 3D view: textured walls, flat-shaded floors
//! and ceilings and a solid sky.
//!
//! The camera stands at an explicit `x`/`y`/`z` and looks `angle` degrees
//! counter-clockwise from east, `pitch` degrees up from level, through a
//! `fov` degree field onto a `width` by `height` raster. No `BSP` is
//! needed: every column ray is intersected with every linedef, the hits
//! are sorted by ray parameter and walked near-to-far (a portal caster).
//! The walk keeps the window of rows still uncovered; each hit first
//! fills that window with the faced sector's ceiling and floor, drawn in
//! the flat's average palette colour shaded by the row's own plane depth,
//! and then paints the wall face its facing sidedef carries. Floors and
//! ceilings are solid colours and the sky one flat colour until the next
//! ticket textures them; masked middle textures seen through two-sided
//! lines are collected in [`Rendered::masked`] instead of drawn.
//!
//! All maths is integer. The camera's `cos`/`sin` are 16.16
//! ([`crate::fixed::Angle`]), column directions are scaled by `2^32`, and
//! ray parameters stay exact `i128` fractions until a screen row or map
//! depth is floor-rounded out of them.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::error::CyberError;
use crate::fixed::{Angle, FRACUNIT, hypot, mul_div_round};
use crate::gfx::image::IndexedImage;
use crate::gfx::ops::parse_hex_color;
use crate::render::canvas::{Canvas, Rgba};
use crate::render::light::{WallDir, colormap_index, shade};
use crate::render::scene::Scene;
use crate::render::settings::Lighting;
use crate::wad::map::{Linedef, Map};

/// Lowest accepted pitch, in degrees.
pub const MIN_PITCH: i32 = -60;
/// Highest accepted pitch, in degrees.
pub const MAX_PITCH: i32 = 60;
/// The texture name that means "draw nothing here".
const TEXTURE_NONE: &str = "-";
/// The ceiling flat that means "sky"; [`crate::render::scene::SKY_FLAT`]
/// spells the same constant for scene builders.
const SKY_CEILING: &str = "F_SKY1";
/// One whole map unit or pixel in raw 16.16.
const ONE_16: i128 = 0x1_0000;
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

/// A masked middle texture seen through a two-sided line, collected for
/// the transparent pass that will draw it.
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
}

/// The picture one render produced, plus the data the next pass needs.
#[derive(Debug)]
pub struct Rendered {
    /// The painted view (gamma is the caller's step).
    pub canvas: Canvas,
    /// Masked middles collected behind two-sided lines, in
    /// column-then-distance order.
    pub masked: Vec<MaskedMiddle>,
}

/// Renders the 3D view of `scene` from `camera` onto a `width` by
/// `height` raster.
///
/// The caller is expected to have checked that the camera lies inside a
/// sector (`crate::render::camera_sector`); a camera in no sector simply
/// gets no sector data to fill with and renders what the hits still
/// cover. The result carries the [`MaskedMiddle`] list; this ticket
/// draws no transparency, so the view only shows solid walls, flat
/// average-coloured planes and a solid sky.
#[must_use]
#[inline]
pub fn render_view(scene: &Scene, camera: &ViewCamera, width: u32, height: u32) -> Rendered {
    let caster = Caster::new(scene, camera, width, height);
    let mut canvas = Canvas::new(width, height, caster.background);
    let mut masked = Vec::new();
    for column in 0_i64..i64::from(width) {
        caster.draw_column(&mut canvas, &mut masked, column);
    }
    Rendered { canvas, masked }
}

/// An exact fraction `n / d` with a positive denominator: a column ray's
/// parameter `t` and a hit's position `s` along its linedef.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fraction {
    /// Numerator.
    n: i128,
    /// Denominator, always positive.
    d: i128,
}

impl Fraction {
    /// Builds `num / den` from raw cross products, normalising the sign;
    /// `None` for a zero denominator (a parallel ray) or a negation that
    /// cannot be taken.
    fn new(num: i128, den: i128) -> Option<Self> {
        match den.cmp(&0_i128) {
            Ordering::Equal => None,
            Ordering::Less => Some(Self {
                n: num.checked_neg()?,
                d: den.checked_neg()?,
            }),
            Ordering::Greater => Some(Self { n: num, d: den }),
        }
    }

    /// Whether the value is strictly positive (the denominator is).
    const fn is_positive(self) -> bool {
        self.n > 0
    }

    /// `self * m`, saturating.
    const fn times(self, m: i128) -> i128 {
        quot(self.n.saturating_mul(m), self.d)
    }

    /// `2 * m * self`, saturating: the height step of one pixel at ray
    /// parameter `t`.
    const fn double_times(self, m: i128) -> i128 {
        quot(self.n.saturating_mul(2_i128).saturating_mul(m), self.d)
    }

    /// `m / (2 * self)` in raw 16.16, saturating: the projected height
    /// span of a world-height difference `m` at ray parameter `t`.
    const fn recip_half16(self, m: i128) -> i128 {
        quot(m.saturating_mul(HALF_16).saturating_mul(self.d), self.n)
    }

    /// The ordering of two fractions by cross-multiplication.
    fn cmp_value(self, other: Self) -> Ordering {
        self.n
            .saturating_mul(other.d)
            .cmp(&other.n.saturating_mul(self.d))
    }
}

/// `a / b` truncated, saturating past the `i128` range and treating a
/// zero divisor as the saturated quotient of `a`'s sign.
const fn quot(a: i128, b: i128) -> i128 {
    match a.checked_div(b) {
        Some(value) => value,
        None => {
            if (a >= 0) == (b >= 0) {
                i128::MAX
            } else {
                i128::MIN
            }
        }
    }
}

/// The projection constants of one camera and raster.
#[derive(Clone, Copy, Debug)]
struct Projection {
    /// Raster width in pixels.
    width: i64,
    /// Raster height in pixels.
    height: i64,
    /// Eye height in map units.
    z: i64,
    /// Camera angle cosine, raw 16.16.
    cos: i64,
    /// Camera angle sine, raw 16.16.
    sin: i64,
    /// Twice the focal length in pixels, raw 16.16.
    focal2: i128,
    /// The horizon row, raw 16.16.
    horizon: i128,
}

impl Projection {
    /// Builds the projection of `camera` over a `width` by `height`
    /// raster: `focal2 = width * cot(fov/2)` in 16.16 and
    /// `horizon = height/2 + (focal2/2) * tan(pitch)`.
    fn new(camera: &ViewCamera, width: u32, height: u32) -> Self {
        let facing = Angle::from_degrees(i64::from(camera.angle));
        let half = Angle::from_millidegrees(i64::from(camera.fov).saturating_mul(500_i64));
        let focal2 = focal_length(i64::from(width), half);
        let tan = Angle::from_degrees(i64::from(camera.pitch))
            .tan()
            .unwrap_or_else(|| Angle::EAST.tan().unwrap_or_default());
        let lift = focal2.saturating_mul(i128::from(tan.0)).checked_shr(17);
        Self {
            width: i64::from(width),
            height: i64::from(height),
            z: i64::from(camera.z),
            cos: i64::from(facing.cos().0),
            sin: i64::from(facing.sin().0),
            focal2,
            horizon: i128::from(
                i64::from(height)
                    .checked_div(2_i64)
                    .unwrap_or_else(|| i64::from(height)),
            )
            .saturating_mul(ONE_16)
            .saturating_add(lift.unwrap_or(0)),
        }
    }

    /// The `2^32`-scaled direction of column `c`:
    /// `d = forward * focal2 + right * u` with `u = 2c + 1 - width`.
    fn direction(&self, c: i64) -> (i128, i128) {
        let u = c
            .saturating_mul(2_i64)
            .saturating_add(1_i64)
            .saturating_sub(self.width);
        let u16 = i128::from(u).saturating_mul(ONE_16);
        let cos = i128::from(self.cos);
        let sin = i128::from(self.sin);
        (
            cos.saturating_mul(self.focal2)
                .saturating_add(sin.saturating_mul(u16)),
            sin.saturating_mul(self.focal2)
                .saturating_sub(cos.saturating_mul(u16)),
        )
    }

    /// The screen row world height `h` projects to at ray parameter `t`,
    /// floored: `horizon - (h - z) / (2t)`.
    fn row(&self, h: i64, t: Fraction) -> i64 {
        let rise = i128::from(h).saturating_sub(i128::from(self.z));
        let raw = self.horizon.saturating_sub(t.recip_half16(rise));
        let floored = raw.div_euclid(ONE_16);
        i64::try_from(floored).unwrap_or(if raw < 0 { i64::MIN } else { i64::MAX })
    }

    /// The world height at screen `row` for ray parameter `t`, raw
    /// 16.16: `z + (horizon - row) * 2t`.
    fn height_at(&self, row: i64, t: Fraction) -> i128 {
        let dy = self
            .horizon
            .saturating_sub(i128::from(row).saturating_mul(ONE_16));
        i128::from(self.z)
            .saturating_mul(ONE_16)
            .saturating_add(t.double_times(dy))
    }

    /// The perpendicular depth in map units, raw 16.16, at `t`.
    const fn depth16(&self, t: Fraction) -> i128 {
        t.times(self.focal2)
    }
}

/// `width * cot(half)` in raw 16.16; the half angle sits inside
/// `5..=85` degrees (the field-of-view range), so its sine is positive.
fn focal_length(width: i64, half: Angle) -> i128 {
    let numerator = width.saturating_mul(i64::from(half.cos().0));
    let denominator = i64::from(half.sin().0).max(1_i64);
    let raw = mul_div_round(numerator, i64::from(FRACUNIT), denominator).unwrap_or(i64::MAX);
    i128::from(raw).max(ONE_16)
}

/// One linedef prepared for casting.
struct Edge {
    /// The linedef index.
    line: usize,
    /// `v1` x.
    x0: i64,
    /// `v1` y.
    y0: i64,
    /// `v1 -> v2` delta x.
    ex: i64,
    /// `v1 -> v2` delta y.
    ey: i64,
    /// Segment length in raw 16.16 map units.
    len16: i128,
    /// Fake-contrast orientation.
    dir: WallDir,
    /// The front sidedef index (resolved at preparation).
    front: usize,
    /// The back sidedef index when the line carries a resolvable one.
    back: Option<usize>,
    /// Whether the upper texture is unpegged (flag bit 8).
    upper_unpegged: bool,
    /// Whether the lower texture is unpegged (flag bit 16).
    lower_unpegged: bool,
}

/// One sidedef prepared for casting: its sector's planes and its usable
/// texture keys.
struct Side {
    /// The sector this side opens into.
    plane: Plane,
    /// Upper texture key, `None` for `-` or empty.
    upper: Option<String>,
    /// Lower texture key.
    lower: Option<String>,
    /// Middle texture key.
    middle: Option<String>,
    /// Horizontal texture shift in map units.
    x_offset: i64,
    /// Vertical texture shift in map units.
    y_offset: i64,
}

/// The view-relevant facts of one sector.
#[derive(Clone, Copy, Debug)]
struct Plane {
    /// Floor height in map units.
    floor: i64,
    /// Ceiling height in map units.
    ceil: i64,
    /// Sector light level.
    light: i32,
    /// Average palette index of the floor flat, `None` for no flat.
    floor_fill: Option<u8>,
    /// Average index filling the ceiling: the sky's for a sky ceiling,
    /// the ceiling flat's otherwise, `None` for nothing to draw (no
    /// flat, or the sky switch off on a sky ceiling).
    ceil_fill: Option<u8>,
    /// Whether the ceiling flat names the sky.
    ceil_sky: bool,
}

impl Plane {
    /// The plane facts of sector `index`; a sector the map lacks reads
    /// as an empty void (zero heights, no fills).
    fn of(map: &Map, index: usize, averages: &BTreeMap<String, u8>, sky: Option<u8>) -> Self {
        let Some(sector) = map.sectors.get(index) else {
            return Self {
                floor: 0,
                ceil: 0,
                light: 0,
                floor_fill: None,
                ceil_fill: None,
                ceil_sky: false,
            };
        };
        let floor_fill = averages.get(&picture_key(&sector.floor_flat)).copied();
        let ceil_sky = picture_key(&sector.ceiling_flat) == SKY_CEILING;
        let ceil_fill = if ceil_sky {
            sky
        } else {
            averages.get(&picture_key(&sector.ceiling_flat)).copied()
        };
        Self {
            floor: i64::from(sector.floor_height),
            ceil: i64::from(sector.ceiling_height),
            light: sector.light,
            floor_fill,
            ceil_fill,
            ceil_sky,
        }
    }
}

/// One sorted ray/linedef intersection.
struct Hit {
    /// The edge's index in [`Caster::edges`].
    edge: usize,
    /// The hit's ray parameter, exact.
    t: Fraction,
    /// The position along `v1 -> v2`, exact and inside `0..=1`.
    s: Fraction,
    /// Whether the ray crossed the linedef's front side.
    front: bool,
    /// The sidedef the ray faces.
    side: usize,
    /// The sidedef on the far side of a transparent crossing.
    opposite: Option<usize>,
}

/// The hit under visit: prepared facts and the projections that drive
/// the column window.
struct Visit<'visit> {
    /// The crossed edge.
    edge: &'visit Edge,
    /// The faced sidedef index.
    side_index: usize,
    /// The faced sidedef.
    side: &'visit Side,
    /// The far sidedef for a transparent crossing.
    far_side: Option<&'visit Side>,
    /// The ray parameter.
    t: Fraction,
    /// The position along the segment.
    s: Fraction,
    /// Whether the faced side is the linedef's front.
    front: bool,
    /// Where the faced sector's ceiling projects.
    y_ceil: i64,
    /// Where the faced sector's floor projects.
    y_floor: i64,
    /// Where the far sector's ceiling projects.
    y_far_ceil: Option<i64>,
    /// Where the far sector's floor projects.
    y_far_floor: Option<i64>,
}

/// The rows of one column still waiting to be covered.
#[derive(Clone, Copy, Debug)]
struct Window {
    /// First uncovered row.
    top: i64,
    /// Last uncovered row.
    bottom: i64,
}

impl Window {
    /// The whole `height`-row column.
    const fn open(height: i64) -> Self {
        Self {
            top: 0,
            bottom: height.saturating_sub(1),
        }
    }

    /// Whether nothing is left to draw.
    const fn closed(self) -> bool {
        self.top > self.bottom
    }

    /// Ends the column.
    const fn seal(&mut self) {
        self.top = 1_i64;
        self.bottom = 0_i64;
    }
}

/// The mutable state of one column's walk.
struct Walk<'walk> {
    /// Pixels so far.
    canvas: &'walk mut Canvas,
    /// Masked middles collected this call.
    masked: &'walk mut Vec<MaskedMiddle>,
    /// The screen column being walked.
    column: i64,
    /// The rows still uncovered.
    window: Window,
}

/// Which texture slot of the faced side a wall part draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// The upper texture band.
    Upper,
    /// The lower texture band.
    Lower,
    /// The middle texture (solid on one-sided lines).
    Middle,
}

/// One ceiling or floor run to fill: a column's rows of one plane.
struct FlatRun {
    /// Screen column.
    column: i64,
    /// First row.
    top: i64,
    /// Last row.
    bottom: i64,
    /// Palette index of the plane's average colour.
    index: u8,
    /// Plane height in map units.
    height: i64,
    /// Sector light level.
    light: i32,
}

/// Everything one textured face needs: geometry, texture and mapping.
struct FacePlan<'plan> {
    /// The hit under visit.
    visit: &'plan Visit<'plan>,
    /// Which slot is drawing.
    slot: Slot,
    /// The texture image.
    image: &'plan IndexedImage,
    /// The window rows the face covers.
    span: (i64, i64),
    /// The perpendicular depth, raw 16.16 map units.
    depth16: i128,
}

/// One textured wall run: mapped, shaded and ready to paint row by row.
struct Part<'tex> {
    /// Screen column the face paints.
    column: i64,
    /// The texture image.
    image: &'tex IndexedImage,
    /// First row.
    top: i64,
    /// Last row.
    bottom: i64,
    /// Texture column, already wrapped, in texels.
    texcol: u32,
    /// World height (raw 16.16) whose texture row is `v = 0`.
    tex_top16: i128,
    /// Vertical texture shift, raw 16.16.
    y_off16: i128,
    /// Sector light shading the face.
    light: i32,
    /// Fake contrast of the linedef.
    dir: WallDir,
    /// Perpendicular depth, raw 16.16 map units.
    depth16: i128,
    /// The ray parameter, for the height at each row.
    t: Fraction,
}

/// The column caster: the scene, the projection and the per-call
/// preparations the column walk reads.
struct Caster<'scene> {
    /// The prepared scene.
    scene: &'scene Scene,
    /// The camera position in map units.
    cam: (i64, i64),
    /// Projection constants.
    proj: Projection,
    /// Castable linedefs.
    edges: Vec<Edge>,
    /// Prepared sidedefs, indexed by sidedef.
    sides: Vec<Option<Side>>,
    /// The background colour.
    background: Rgba,
    /// The lighting model.
    lighting: Lighting,
}

impl<'scene> Caster<'scene> {
    /// Prepares one view render of `scene` from `camera`.
    fn new(scene: &'scene Scene, camera: &ViewCamera, width: u32, height: u32) -> Self {
        let averages = flat_averages(scene);
        let sky = scene
            .settings
            .sky
            .is_on()
            .then(|| average_index(&scene.sky))
            .flatten();
        let edges = scene
            .map
            .linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| edge(index, &scene.map, line))
            .collect();
        let sides = (0..scene.map.sidedefs.len())
            .map(|index| side(&scene.map, index, &averages, sky))
            .collect();
        Self {
            scene,
            cam: (i64::from(camera.x), i64::from(camera.y)),
            proj: Projection::new(camera, width, height),
            edges,
            sides,
            background: parse_hex_color(&scene.settings.background).unwrap_or(BLACK),
            lighting: scene.settings.lighting,
        }
    }

    /// Casts and walks one screen column; whatever the walk leaves
    /// uncovered keeps the background.
    fn draw_column(&self, canvas: &mut Canvas, masked: &mut Vec<MaskedMiddle>, column: i64) {
        let mut walk = Walk {
            canvas,
            masked,
            column,
            window: Window::open(self.proj.height),
        };
        let d = self.proj.direction(column);
        for hit in self.hits(d) {
            self.visit(&mut walk, &hit);
        }
    }

    /// Every hit of direction `d`, sorted by ray parameter then linedef.
    fn hits(&self, d: (i128, i128)) -> Vec<Hit> {
        let mut found: Vec<Hit> = self
            .edges
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| self.hit(index, edge, d))
            .collect();
        found.sort_unstable_by(|a, b| a.t.cmp_value(b.t).then(a.edge.cmp(&b.edge)));
        found
    }

    /// The hit of `edge` with direction `d`, `None` when the ray passes
    /// through a back side that cannot be faced.
    fn hit(&self, index: usize, edge: &Edge, d: (i128, i128)) -> Option<Hit> {
        let (t, s, front) = pierce(self.cam, d, edge)?;
        let side = if front { edge.front } else { edge.back? };
        let opposite = if front { edge.back } else { Some(edge.front) };
        Some(Hit {
            edge: index,
            t,
            s,
            front,
            side,
            opposite,
        })
    }

    /// Gathers the hit's prepared facts, `None` when a side is missing.
    fn visit_of(&self, hit: &Hit) -> Option<Visit<'_>> {
        let edge = self.edges.get(hit.edge)?;
        let side = self.sides.get(hit.side).and_then(Option::as_ref)?;
        let far_side = hit
            .opposite
            .and_then(|index| self.sides.get(index).and_then(Option::as_ref));
        Some(Visit {
            edge,
            side_index: hit.side,
            side,
            far_side,
            t: hit.t,
            s: hit.s,
            front: hit.front,
            y_ceil: self.proj.row(side.plane.ceil, hit.t),
            y_floor: self.proj.row(side.plane.floor, hit.t),
            y_far_ceil: far_side.map(|far| self.proj.row(far.plane.ceil, hit.t)),
            y_far_floor: far_side.map(|far| self.proj.row(far.plane.floor, hit.t)),
        })
    }

    /// Processes one hit: fills the planes up to it, then draws (or
    /// seals) the wall it faces; a sealed column does nothing.
    fn visit(&self, walk: &mut Walk<'_>, hit: &Hit) {
        if walk.window.closed() {
            return;
        }
        let Some(visit) = self.visit_of(hit) else {
            walk.window.seal();
            return;
        };
        self.fill_planes(walk, &visit);
        walk.window.top = walk.window.top.max(visit.y_ceil);
        walk.window.bottom = walk.window.bottom.min(visit.y_floor);
        if walk.window.closed() {
            return;
        }
        if visit.far_side.is_none() {
            self.draw_face(walk, &visit, Slot::Middle);
            walk.window.seal();
            return;
        }
        self.draw_face(walk, &visit, Slot::Upper);
        self.draw_face(walk, &visit, Slot::Lower);
        if walk.window.closed() {
            return;
        }
        self.collect_masked(walk, &visit);
    }

    /// Fills the window rows the faced sector's planes own up to this
    /// hit: ceiling above, floor below.
    fn fill_planes(&self, walk: &mut Walk<'_>, visit: &Visit<'_>) {
        let plane = &visit.side.plane;
        if let Some(index) = plane.ceil_fill {
            let run = FlatRun {
                column: walk.column,
                top: walk.window.top,
                bottom: visit.y_ceil.min(walk.window.bottom),
                index,
                height: plane.ceil,
                light: plane.light,
            };
            self.fill_flat(walk.canvas, &run);
        }
        if let Some(index) = plane.floor_fill {
            let run = FlatRun {
                column: walk.column,
                top: visit.y_floor.max(walk.window.top),
                bottom: walk.window.bottom,
                index,
                height: plane.floor,
                light: plane.light,
            };
            self.fill_flat(walk.canvas, &run);
        }
    }

    /// Fills the run's rows with the plane's average colour, shaded by
    /// each row's own plane depth; the horizon row is skipped.
    fn fill_flat(&self, canvas: &mut Canvas, run: &FlatRun) {
        for row in run.top..=run.bottom {
            self.fill_flat_row(canvas, run, row);
        }
    }

    /// Draws one row of a plane run; the horizon row (zero `dy`) skips.
    fn fill_flat_row(&self, canvas: &mut Canvas, run: &FlatRun, row: i64) {
        let dy = i128::from(row)
            .saturating_mul(ONE_16)
            .saturating_sub(self.proj.horizon)
            .abs();
        let Some(depth) = plane_depth(self.proj.focal2, self.proj.z, run.height, dy) else {
            return;
        };
        let map = colormap_index(self.lighting, run.light, depth, WallDir::Other);
        canvas.set(run.column, row, shade(self.scene, run.index, map));
    }

    /// Paints `slot`'s texture across the window rows it owns (an empty
    /// `-` name leaves them to the background and later hits).
    fn draw_face(&self, walk: &mut Walk<'_>, visit: &Visit<'_>, slot: Slot) {
        let Some(span) = Self::face_span(walk, visit, slot) else {
            return;
        };
        if span.0 > span.1 {
            return;
        }
        let name = match slot {
            Slot::Upper => &visit.side.upper,
            Slot::Lower => &visit.side.lower,
            Slot::Middle => &visit.side.middle,
        };
        let Some(key) = name.as_deref() else {
            return;
        };
        let Some(image) = self.scene.textures.get(key) else {
            return;
        };
        let plan = FacePlan {
            visit,
            slot,
            image,
            span,
            depth16: self.proj.depth16(visit.t),
        };
        let part = part(&plan, walk.column);
        let canvas = &mut *walk.canvas;
        self.paint_part(canvas, &part);
    }

    /// The rows `slot`'s face covers, shrinking the window past it;
    /// `None` when the face does not exist geometrically.
    fn face_span(walk: &mut Walk<'_>, visit: &Visit<'_>, slot: Slot) -> Option<(i64, i64)> {
        match slot {
            Slot::Middle => Some((walk.window.top, walk.window.bottom)),
            Slot::Upper => Self::upper_span(walk, visit),
            Slot::Lower => Self::lower_span(walk, visit),
        }
    }

    /// The upper band: rows from the window top to the far ceiling,
    /// skipped when both sectors look at the sky.
    fn upper_span(walk: &mut Walk<'_>, visit: &Visit<'_>) -> Option<(i64, i64)> {
        let far = visit.far_side?;
        if far.plane.ceil >= visit.side.plane.ceil {
            return None;
        }
        if far.plane.ceil_sky && visit.side.plane.ceil_sky {
            return None;
        }
        let y = visit.y_far_ceil?;
        let span = (walk.window.top, y.min(walk.window.bottom));
        walk.window.top = walk.window.top.max(y);
        Some(span)
    }

    /// The lower band: rows from the far floor down to the window
    /// bottom, kept when the near ceiling is sky above a plain floor.
    fn lower_span(walk: &mut Walk<'_>, visit: &Visit<'_>) -> Option<(i64, i64)> {
        let far = visit.far_side?;
        if far.plane.floor <= visit.side.plane.floor {
            return None;
        }
        let y = visit.y_far_floor?;
        let span = (y.max(walk.window.top), walk.window.bottom);
        walk.window.bottom = walk.window.bottom.min(y);
        Some(span)
    }

    /// Paints the part's rows: one texture column, heights from the
    /// projection, one lighting table for the whole face.
    fn paint_part(&self, canvas: &mut Canvas, part: &Part<'_>) {
        for row in part.top..=part.bottom {
            self.paint_row(canvas, part, row);
        }
    }

    /// Draws one wall row: the texture row the screen row projects to,
    /// through the whole-face lighting table.
    fn paint_row(&self, canvas: &mut Canvas, part: &Part<'_>, row: i64) {
        let depth = i64::try_from(part.depth16.div_euclid(ONE_16)).unwrap_or(i64::MAX);
        let map = colormap_index(self.lighting, part.light, depth, part.dir);
        let v16 = part
            .tex_top16
            .saturating_sub(self.proj.height_at(row, part.t))
            .saturating_add(part.y_off16);
        let rows = i128::from(part.image.height);
        let Some((index, _)) = part.image.get(part.texcol, wrap16(v16, rows)) else {
            return;
        };
        canvas.set(part.column, row, shade(self.scene, index, map));
    }

    /// Remembers the middle texture seen through this transparent
    /// crossing, for the masked pass that will draw it.
    fn collect_masked(&self, walk: &mut Walk<'_>, visit: &Visit<'_>) {
        if visit.side.middle.is_none() {
            return;
        }
        walk.masked.push(MaskedMiddle {
            column: u32::try_from(walk.column).unwrap_or(u32::MAX),
            line: visit.edge.line,
            side: visit.side_index,
            top: walk.window.top,
            bottom: walk.window.bottom,
            depth16: self.proj.depth16(visit.t),
        });
    }
}

/// The ray-segment intersection: `cam + t * d` against `edge`, giving
/// the exact `t` and `s` and whether the ray crossed from the front.
///
/// `t > 0` and `0 <= s <= 1`, compared by cross-multiplication in
/// `i128`; `None` when the ray is parallel, points away, or misses the
/// segment. The facing side is the front when `cross(E, d) > 0` with
/// `E = v2 - v1` (the front sidedef bounds the right of `v1 -> v2`).
fn pierce(cam: (i64, i64), d: (i128, i128), edge: &Edge) -> Option<(Fraction, Fraction, bool)> {
    let px = i128::from(edge.x0).saturating_sub(i128::from(cam.0));
    let py = i128::from(edge.y0).saturating_sub(i128::from(cam.1));
    let ex = i128::from(edge.ex);
    let ey = i128::from(edge.ey);
    let den =
        d.0.saturating_mul(ey)
            .saturating_sub(d.1.saturating_mul(ex));
    let raw_t = px.saturating_mul(ey).saturating_sub(py.saturating_mul(ex));
    let raw_s = px
        .saturating_mul(d.1)
        .saturating_sub(py.saturating_mul(d.0));
    let t = Fraction::new(raw_t.saturating_mul(DIRECTION_SCALE), den)?;
    let s = Fraction::new(raw_s, den)?;
    if !t.is_positive() || s.n < 0 || s.n > s.d {
        return None;
    }
    Some((t, s, den < 0))
}

/// The depth in map units of a plane row `dy` raw-16.16 pixels from the
/// horizon: `|plane_h - z| * (focal2 / 2) / |dy|`; `None` at the horizon
/// row itself, which the walk skips.
fn plane_depth(focal2: i128, z: i64, height: i64, dy: i128) -> Option<i64> {
    if dy == 0 {
        return None;
    }
    let rise = i128::from(height)
        .saturating_sub(i128::from(z))
        .saturating_abs();
    let raw = rise
        .saturating_mul(focal2)
        .checked_div(dy.saturating_mul(2_i128))?;
    Some(i64::try_from(raw).unwrap_or(i64::MAX))
}

/// The world height whose texture row is `v = 0` for `plan`'s slot,
/// following the pegging rules: a pegged middle hangs from the near
/// ceiling, a pegged upper's bottom sits on the far ceiling, a pegged
/// lower's top sits on the far floor, and the unpegged variants measure
/// from a fixed world height instead.
fn tex_top(plan: &FacePlan<'_>) -> i64 {
    let near = &plan.visit.side.plane;
    let far = plan.visit.far_side.map(|side| &side.plane);
    let tex_height = i64::from(plan.image.height);
    let unpegged = match plan.slot {
        Slot::Upper => plan.visit.edge.upper_unpegged,
        Slot::Middle | Slot::Lower => plan.visit.edge.lower_unpegged,
    };
    let (pegged, unpegged_top) = match plan.slot {
        Slot::Upper => (
            far.map_or(near.ceil, |f| f.ceil.saturating_add(tex_height)),
            near.ceil,
        ),
        Slot::Middle => (near.ceil, near.floor.saturating_add(tex_height)),
        Slot::Lower => (far.map_or(near.floor, |f| f.floor), near.ceil),
    };
    if unpegged { unpegged_top } else { pegged }
}

/// Builds the mapped, shaded wall run from `plan`, painting `column`.
fn part<'plan>(plan: &'plan FacePlan<'plan>, column: i64) -> Part<'plan> {
    let visit = plan.visit;
    let side = visit.side;
    let along = visit.s.times(visit.edge.len16);
    let u16 = if visit.front {
        along
    } else {
        visit.edge.len16.saturating_sub(along)
    }
    .saturating_add(i128::from(side.x_offset).saturating_mul(ONE_16));
    Part {
        column,
        image: plan.image,
        top: plan.span.0,
        bottom: plan.span.1,
        texcol: wrap16(u16, i128::from(plan.image.width)),
        tex_top16: i128::from(tex_top(plan)).saturating_mul(ONE_16),
        y_off16: i128::from(side.y_offset).saturating_mul(ONE_16),
        light: side.plane.light,
        dir: visit.edge.dir,
        depth16: plan.depth16,
        t: visit.t,
    }
}

/// The texel coordinate of a raw-16.16 texture position wrapped into
/// `size` texels.
fn wrap16(value: i128, size: i128) -> u32 {
    if size <= 0 {
        return 0;
    }
    let wrapped = value.rem_euclid(size.saturating_mul(ONE_16));
    u32::try_from(wrapped.div_euclid(ONE_16)).unwrap_or(0)
}

/// The scene key of a texture or flat name: uppercase and trimmed.
fn picture_key(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

/// The texture key of a sidedef name, `None` for the empty name and the
/// `-` none-marker.
fn texture_key(name: &str) -> Option<String> {
    let key = picture_key(name);
    if key.is_empty() || key == TEXTURE_NONE {
        return None;
    }
    Some(key)
}

/// The average palette index of an indexed image, `None` when it holds
/// no pixels.
fn average_index(image: &IndexedImage) -> Option<u8> {
    let count = u64::try_from(image.indices.len()).ok().filter(|&n| n > 0)?;
    let total = image
        .indices
        .iter()
        .fold(0_u64, |sum, &index| sum.saturating_add(u64::from(index)));
    u8::try_from(total.checked_div(count)?).ok()
}

/// The average palette index of every flat the scene loaded.
fn flat_averages(scene: &Scene) -> BTreeMap<String, u8> {
    scene
        .flats
        .iter()
        .filter_map(|(name, image)| average_index(image).map(|avg| (name.clone(), avg)))
        .collect()
}

/// The fake-contrast orientation of a linedef vector.
const fn wall_dir(ex: i64, ey: i64) -> WallDir {
    if ey == 0 {
        WallDir::EastWest
    } else if ex == 0 {
        WallDir::NorthSouth
    } else {
        WallDir::Other
    }
}

/// The sidedef index when it exists and opens into a real sector.
fn usable_side(map: &Map, side: Option<usize>) -> Option<usize> {
    let index = side?;
    let row = map.sidedefs.get(index)?;
    map.sectors.get(row.sector).is_some().then_some(index)
}

/// One linedef prepared for casting, `None` when it cannot bound a
/// visible face (missing vertices, zero length, dangling front side).
fn edge(line: usize, map: &Map, row: &Linedef) -> Option<Edge> {
    let start = map.vertices.get(row.v1)?;
    let end = map.vertices.get(row.v2)?;
    let (x0, y0) = (i64::from(start.x), i64::from(start.y));
    let ex = i64::from(end.x).saturating_sub(x0);
    let ey = i64::from(end.y).saturating_sub(y0);
    if ex == 0 && ey == 0 {
        return None;
    }
    let front = usable_side(map, row.front)?;
    let back = row
        .back
        .filter(|&index| usable_side(map, Some(index)).is_some());
    Some(Edge {
        line,
        x0,
        y0,
        ex,
        ey,
        len16: i128::from(hypot(ex, ey)).saturating_mul(ONE_16),
        dir: wall_dir(ex, ey),
        front,
        back,
        upper_unpegged: row.flags & 8_u32 != 0_u32,
        lower_unpegged: row.flags & 16_u32 != 0_u32,
    })
}

/// One sidedef prepared for casting, `None` when its sector dangles.
fn side(map: &Map, index: usize, averages: &BTreeMap<String, u8>, sky: Option<u8>) -> Option<Side> {
    let row = map.sidedefs.get(index)?;
    map.sectors.get(row.sector)?;
    Some(Side {
        plane: Plane::of(map, row.sector, averages, sky),
        upper: texture_key(&row.upper),
        lower: texture_key(&row.lower),
        middle: texture_key(&row.middle),
        x_offset: i64::from(row.x_offset),
        y_offset: i64::from(row.y_offset),
    })
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::ToString as _;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cmp::Ordering;

    use super::{
        DIRECTION_SCALE, Edge, FacePlan, Fraction, MAX_PITCH, MIN_PITCH, ONE_16, Plane, Projection,
        Slot, ViewCamera, average_index, check_pitch, focal_length, pierce, plane_depth,
        render_view, tex_top, texture_key, wall_dir, wrap16,
    };
    use crate::fixed::Angle;
    use crate::gfx::image::IndexedImage;
    use crate::gfx::palette::Palette;
    use crate::render::{Lighting, RenderSettings, Scene, WallDir};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;

    /// `value` scaled into raw 16.16, for expected positions.
    fn raw(value: i64) -> i128 {
        i128::from(value).saturating_mul(ONE_16)
    }

    /// A raw test fraction by value (denominators are positive).
    fn frac(n: i128, d: i128) -> Fraction {
        Fraction::new(n, d).expect("test denominators are non-zero")
    }

    /// A unit-length east-pointing column direction.
    fn east() -> (i128, i128) {
        (DIRECTION_SCALE, 0_i128)
    }

    /// A prepared horizontal edge from `y0` `span` units east of `x0`.
    fn horizontal(x0: i64, y0: i64, span: i64) -> Edge {
        Edge {
            line: 0,
            x0,
            y0,
            ex: span,
            ey: 0,
            len16: raw(span),
            dir: WallDir::EastWest,
            front: 0,
            back: Some(1),
            upper_unpegged: false,
            lower_unpegged: false,
        }
    }

    #[test]
    fn pierce_reports_exact_fractions_and_the_facing_side() {
        let edge = horizontal(0, 0, 10);
        // From (0,5) heading north-east: the segment is behind the ray.
        let away = pierce((0, 5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge);
        assert!(away.is_none(), "a crossing at t < 0 is no hit");
        // From (0,5) heading south-east: (5,0) at s = 1/2, back side.
        let (t, s, front) =
            pierce((0, 5), (DIRECTION_SCALE, -DIRECTION_SCALE), &edge).expect("a crossing");
        assert_eq!(t.times(ONE_16), raw(5), "t = 5");
        assert_eq!(
            s.times(ONE_16),
            raw(1).checked_div(2_i128).expect("test divisor"),
            "s = 1/2"
        );
        assert!(!front, "the ray crosses the back side from above");
        // From (0,-5) heading north-east: the same line from the front.
        let (_, _, faced_front) =
            pierce((0, -5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge).expect("a crossing");
        assert!(faced_front, "the front side bounds the right of v1 -> v2");
    }

    #[test]
    fn pierce_misses_parallel_and_outside_segments() {
        let edge = horizontal(0, 0, 10);
        let parallel = pierce((0, 5), east(), &edge);
        assert!(parallel.is_none(), "an east ray never meets an east edge");
        // Crossing the segment's line far beyond v2: s > 1 rejects it.
        let short = horizontal(0, 10, 2);
        let beyond = pierce((0, 0), (DIRECTION_SCALE, DIRECTION_SCALE), &short);
        assert!(beyond.is_none(), "the crossing at t = 10 lands at s = 5");
        let behind = pierce((20, 5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge);
        assert!(behind.is_none(), "the segment lies behind this ray");
    }

    #[test]
    fn fractions_order_by_cross_multiplication() {
        let half = frac(1, 2);
        let fifth = frac(1, 5);
        let big = frac(12, 10);
        assert_eq!(fifth.cmp_value(half), Ordering::Less);
        assert_eq!(half.cmp_value(frac(2, 4)), Ordering::Equal);
        assert_eq!(big.cmp_value(half), Ordering::Greater);
        let mut values = vec![big, fifth, half];
        values.sort_unstable_by(|a, b| a.cmp_value(*b));
        assert_eq!(values, vec![fifth, half, big], "near to far");
    }

    #[test]
    fn heights_project_around_the_horizon() {
        let camera = ViewCamera {
            x: 64,
            y: 128,
            z: 41,
            angle: 0,
            pitch: 0,
            fov: 90,
        };
        let proj = Projection::new(&camera, 160, 100);
        assert_eq!(proj.horizon, raw(50), "level pitch centres the view");
        assert_eq!(proj.focal2, raw(160), "cot(45) leaves focal2 = W");
        let t = frac(12, 10);
        assert_eq!(proj.row(41, t), 50, "the eye height lands on the horizon");
        assert_eq!(proj.row(32, t), 53, "9 below the eye projects 3.75 down");
        assert_eq!(proj.row(128, t), 13, "87 above the eye projects 36.25 up");
        assert_eq!(proj.depth16(t), raw(192), "t * focal2 is the depth");
        let h = proj.height_at(53, t);
        assert!(
            h.saturating_sub(raw(32)).abs() < raw(2),
            "the step row measures back to about 32: {h}"
        );
    }

    #[test]
    fn pitch_shears_the_horizon_downwards() {
        let flat = ViewCamera {
            x: 0,
            y: 0,
            z: 0,
            angle: 0,
            pitch: 0,
            fov: 90,
        };
        let up = ViewCamera { pitch: 30, ..flat };
        let proj = Projection::new(&up, 160, 100);
        let lift = raw(50).saturating_add(
            raw(160)
                .saturating_mul(37_837_i128)
                .checked_shr(17)
                .unwrap_or(0),
        );
        assert!(
            proj.horizon.saturating_sub(lift).abs() < raw(2),
            "tan(30) pushes the horizon about 46 rows down: {}",
            proj.horizon
        );
        assert!(
            proj.horizon > raw(50),
            "a positive pitch moves the horizon down the screen"
        );
    }

    #[test]
    fn the_focal_length_spans_the_field_of_view_range() {
        let wide = focal_length(160, Angle::from_millidegrees(85_000));
        assert!(
            wide.saturating_sub(raw(14)).abs() < raw(2),
            "cot(85) shrinks focal2: {wide}"
        );
        let narrow = focal_length(160, Angle::from_millidegrees(5_000));
        assert!(
            narrow.saturating_sub(raw(1_829)).abs() < raw(20),
            "cot(5) stretches focal2: {narrow}"
        );
    }

    #[test]
    fn the_pitch_range_is_the_documented_one() {
        for pitch in [MIN_PITCH, 0_i32, MAX_PITCH] {
            assert!(check_pitch(pitch).is_ok(), "{pitch} is legal");
        }
        for pitch in [
            MIN_PITCH.saturating_sub(1),
            MAX_PITCH.saturating_add(1),
            90_i32,
        ] {
            let error = check_pitch(pitch).expect_err("outside the range");
            assert_eq!(error.code(), "invalid_argument", "pitch {pitch} code");
            assert!(
                error.to_string().contains("-60..=60"),
                "the message shows the range: {error}"
            );
        }
    }

    #[test]
    fn plane_depth_uses_the_row_geometry_and_skips_the_horizon() {
        let focal2 = raw(160);
        assert_eq!(
            plane_depth(focal2, 41, 0, raw(49)),
            Some(66),
            "floor 0 seen 49 rows down: 41 * 80 / 49 = 66.9"
        );
        assert_eq!(
            plane_depth(focal2, 41, 41, raw(1)),
            Some(0),
            "an eye-height plane has no depth"
        );
        assert_eq!(
            plane_depth(focal2, 41, 0, 0),
            None,
            "the horizon row is skipped"
        );
    }

    #[test]
    fn texture_tops_follow_the_pegging_rules() {
        let image = solid(64, 32, 7);
        let cases = [
            (Slot::Upper, false, 128, "pegged to the far ceiling"),
            (
                Slot::Upper,
                true,
                128,
                "unpegged measures from the near ceiling",
            ),
            (
                Slot::Middle,
                false,
                128,
                "a solid middle hangs from the ceiling",
            ),
            (Slot::Middle, true, 32, "unpegged stands on the floor"),
            (
                Slot::Lower,
                false,
                32,
                "a pegged lower tops out at the far floor",
            ),
            (Slot::Lower, true, 128, "unpegged scrolls from the ceiling"),
        ];
        for (slot, unpegged, expected, why) in cases {
            let (edge, near, far) = step_parts(unpegged);
            let visit = super::Visit {
                edge: &edge,
                side_index: 0,
                side: &near,
                far_side: Some(&far),
                t: frac(12, 10),
                s: frac(1, 2),
                front: true,
                y_ceil: 0,
                y_floor: 0,
                y_far_ceil: None,
                y_far_floor: None,
            };
            let plan = FacePlan {
                visit: &visit,
                slot,
                image: &image,
                span: (0, 10),
                depth16: raw(100),
            };
            assert_eq!(tex_top(&plan), expected, "{why}");
        }
    }

    /// A near/far pair split by a 32-unit step under a 96-unit far
    /// ceiling, on an edge carrying the asked-for unpegged bits. Near
    /// floor 0 ceiling 128, far floor 32 ceiling 96, texture 32 tall:
    /// the pegged cases differ from the unpegged ones by the table above.
    fn step_parts(unpegged: bool) -> (Edge, super::Side, super::Side) {
        let near = Plane {
            floor: 0,
            ceil: 128,
            light: 255,
            floor_fill: None,
            ceil_fill: None,
            ceil_sky: false,
        };
        let far = Plane {
            floor: 32,
            ceil: 96,
            ..near
        };
        (
            Edge {
                line: 0,
                x0: 0,
                y0: 0,
                ex: 0,
                ey: -256,
                len16: raw(256),
                dir: WallDir::NorthSouth,
                front: 0,
                back: Some(1),
                upper_unpegged: unpegged,
                lower_unpegged: unpegged,
            },
            super::Side {
                plane: near,
                upper: Some("WALL1".to_owned()),
                lower: Some("STEP1".to_owned()),
                middle: None,
                x_offset: 0,
                y_offset: 0,
            },
            super::Side {
                plane: far,
                upper: None,
                lower: None,
                middle: None,
                x_offset: 0,
                y_offset: 0,
            },
        )
    }

    #[test]
    fn wrapped_texture_coordinates_stay_in_the_image() {
        assert_eq!(wrap16(raw(3), 64), 3, "a plain column");
        assert_eq!(wrap16(raw(67), 64), 3, "one lap");
        assert_eq!(wrap16(-raw(1), 64), 63, "below zero wraps up");
        assert_eq!(wrap16(-raw(64), 64), 0, "exactly one lap back");
        assert_eq!(wrap16(raw(1), 0), 0, "an empty texture reads column zero");
    }

    #[test]
    fn averages_and_keys_are_the_picture_side() {
        let flat = IndexedImage::new(2, 2, vec![1_u8, 3, 5, 7], vec![true; 4]).expect("2x2 builds");
        assert_eq!(average_index(&flat), Some(4), "the mean index");
        let nothing = IndexedImage::new(1, 1, vec![9_u8], vec![true]).expect("1x1 builds");
        assert_eq!(average_index(&nothing), Some(9), "a single pixel");
        assert_eq!(texture_key(" wall1 "), Some("WALL1".to_owned()));
        assert_eq!(texture_key("-"), None, "the none-marker");
        assert_eq!(texture_key("  "), None, "an empty name");
    }

    #[test]
    fn wall_directions_split_the_cardinals() {
        assert_eq!(wall_dir(64, 0), WallDir::EastWest);
        assert_eq!(wall_dir(0, -64), WallDir::NorthSouth);
        assert_eq!(wall_dir(3, 4), WallDir::Other);
    }

    /// A solid indexed image of `index`.
    fn solid(width: u32, height: u32, index: u8) -> IndexedImage {
        let count = usize::try_from(width.saturating_mul(height)).expect("test sizes fit");
        IndexedImage::new(width, height, vec![index; count], vec![true; count])
            .expect("test images build")
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
    fn identity_table() -> [u8; 256] {
        let mut table = [0_u8; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            *entry = u8::try_from(index).expect("the table covers exactly the u8 range");
        }
        table
    }

    /// The box's geometry: a 512x256 one-sector footprint, all four
    /// sidedefs `WALL1`-middle facing inward.
    fn box_map() -> Map {
        let name = LumpName::parse("MAP01").expect("test names are valid");
        let mut map = Map::new_empty(name, MapFormat::Doom);
        map.vertices = [
            (0_i32, 0_i32),
            (0_i32, 256_i32),
            (512_i32, 256_i32),
            (512_i32, 0_i32),
            (0_i32, 0_i32),
        ]
        .into_iter()
        .map(|(x, y)| Vertex {
            x,
            y,
            ..Default::default()
        })
        .collect();
        map.linedefs = (0..4_usize)
            .map(|index| Linedef {
                v1: index,
                v2: index.saturating_add(1),
                flags: 1_u32,
                front: Some(index),
                ..Default::default()
            })
            .collect();
        map.sidedefs = (0..4_usize)
            .map(|_| Sidedef {
                upper: "-".to_owned(),
                lower: "-".to_owned(),
                middle: "WALL1".to_owned(),
                sector: 0,
                ..Default::default()
            })
            .collect();
        map.sectors = vec![Sector {
            floor_height: 0,
            ceiling_height: 128,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "CEIL3".to_owned(),
            light: 255,
            ..Default::default()
        }];
        map
    }

    /// A fullbright scene over [`box_map`]: `WALL1` middles (index 77),
    /// `FLOOR1` (index 1), `CEIL3` (index 200) and an identity `COLORMAP`.
    fn box_scene() -> Scene {
        let mut textures = BTreeMap::new();
        textures.insert("WALL1".to_owned(), solid(64, 128, 77));
        let mut flats = BTreeMap::new();
        flats.insert("FLOOR1".to_owned(), solid(64, 64, 1));
        flats.insert("CEIL3".to_owned(), solid(64, 64, 200));
        Scene {
            map: box_map(),
            palette: fixture_palette(),
            colormaps: vec![identity_table()],
            textures,
            flats,
            sky: solid(64, 64, 0),
            sky_flat: "F_SKY1".to_owned(),
            settings: RenderSettings {
                lighting: Lighting::Fullbright,
                ..RenderSettings::default()
            },
            problems: Vec::new(),
        }
    }

    /// The east-facing camera the box is checked from: 32x64, fov 90,
    /// eye 41 at `(64, 96)`.
    fn box_camera() -> ViewCamera {
        ViewCamera {
            x: 64,
            y: 96,
            z: 41,
            angle: 0,
            pitch: 0,
            fov: 90,
        }
    }

    #[test]
    fn a_box_render_lands_walls_floor_and_ceiling_where_the_math_says() {
        let scene = box_scene();
        let rendered = render_view(&scene, &box_camera(), 32, 64);
        assert!(rendered.masked.is_empty(), "one-sided lines mask nothing");
        let canvas = &rendered.canvas;
        assert_eq!(
            canvas.pixel(0, 63),
            Some([1_u8, 254, 1, 255]),
            "the bottom row is the FLOOR1 average"
        );
        assert_eq!(
            canvas.pixel(0, 0),
            Some([200_u8, 55, 200, 255]),
            "the top of the north-leaning column is the CEIL3 average"
        );
        assert_eq!(
            canvas.pixel(0, 30),
            Some([77_u8, 178, 77, 255]),
            "between its plane rows the north wall paints"
        );
        assert_eq!(
            canvas.pixel(16, 32),
            Some([77_u8, 178, 77, 255]),
            "the centre column at the horizon is the east wall"
        );
        assert_eq!(
            canvas.pixel(16, 0),
            Some([200_u8, 55, 200, 255]),
            "the centre column's ceiling projects inside the frame"
        );
    }

    #[test]
    fn the_background_seals_the_frame_outside_every_wall() {
        let mut scene = box_scene();
        scene.map.linedefs.clear();
        scene.map.sidedefs.clear();
        let rendered = render_view(&scene, &box_camera(), 32, 64);
        assert_eq!(
            rendered.canvas.pixel(16, 32),
            Some([0_u8, 0, 0, 255]),
            "no lines means the plain background colour"
        );
    }
}
