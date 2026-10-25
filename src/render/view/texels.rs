//! Where every texel comes from: the ray/segment intersection behind a
//! [`super::walk::Hit`], the world-point and texel maths of floors,
//! ceilings and the sky, the mapping and pegging of a wall face, and the
//! preparation pass that turns map rows into the [`Edge`]s and [`Side`]s
//! the caster walks.
//!
//! Every helper here is pure integer maths over the shapes the sibling
//! `walk` module declares: [`pierce`] answers the exact [`Fraction`]s a
//! crossing carries, [`tex_top`] and [`masked_tex_top`] the world height
//! a texture's `v = 0` row sits on, [`texture_u16`] and [`wrap16`] the
//! texel column, [`flat_texel`] and [`sky_column`] the plane texels a row
//! samples, [`part`] the whole mapped run of one face, and [`edge`] and
//! [`side`] the prepared forms of one linedef and one sidedef.

use super::projection::Fraction;
use super::walk::{Edge, FacePlan, Part, Plane, Side, Slot, Surface, Visit};
use super::{DIRECTION_SCALE, ONE_16};
use crate::fixed::{atan2, hypot};
use crate::render::light::WallDir;
use crate::render::scene::Scene;
use crate::wad::map::consts::normalized_picture;
use crate::wad::map::flags::{DONT_PEG_BOTTOM, DONT_PEG_TOP};
use crate::wad::map::{Linedef, Map};

/// The ray-segment intersection: `cam + t * d` against `edge`, giving
/// the exact `t` and `s` and whether the ray crossed from the front.
///
/// `t > 0` and `0 <= s <= 1`, compared by cross-multiplication in
/// `i128`; `None` when the ray is parallel, points away, or misses the
/// segment. The facing side is the front when `cross(E, d) > 0` with
/// `E = v2 - v1` (the front sidedef bounds the right of `v1 -> v2`).
pub(super) fn pierce(
    cam: (i64, i64),
    d: (i128, i128),
    edge: &Edge,
) -> Option<(Fraction, Fraction, bool)> {
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

/// The world point `cam + t * d` in raw 16.16 for ray parameter `t`
/// along the `2^32`-scaled column direction `d`.
pub(super) fn plane_point(cam: (i64, i64), d: (i128, i128), t: Fraction) -> (i128, i128) {
    (
        i128::from(cam.0)
            .saturating_mul(ONE_16)
            .saturating_add(t.times(shift_down(d.0))),
        i128::from(cam.1)
            .saturating_mul(ONE_16)
            .saturating_add(t.times(shift_down(d.1))),
    )
}

/// A `2^32`-scaled direction component back to raw 16.16 (the shift can
/// only fail for a shift distance the constant never is).
fn shift_down(component: i128) -> i128 {
    component.checked_shr(16_u32).unwrap_or(0)
}

/// The flat texel of the raw-16.16 world point `(x16, y16)`, `Doom`-
/// aligned to the world grid: column `floor(x)` and row `floor(-y)`,
/// each wrapped into the flat, whatever supported shape it has.
pub(super) fn flat_texel(x16: i128, y16: i128, width: u32, height: u32) -> Option<(u32, u32)> {
    let columns = i128::from(width).max(1_i128);
    let rows = i128::from(height).max(1_i128);
    let column = x16.div_euclid(ONE_16).rem_euclid(columns);
    let row = y16.saturating_neg().div_euclid(ONE_16).rem_euclid(rows);
    Some((u32::try_from(column).ok()?, u32::try_from(row).ok()?))
}

/// The sky texture column of a `2^32`-scaled column direction: the ray
/// angle's `BAM` bucketed into `Doom`'s 1024 slices per turn (`>> 22`)
/// and wrapped into the sky's width, so a 256-wide sky repeats four
/// times per full turn.
pub(super) fn sky_column(d: (i128, i128), sky_width: u32) -> u32 {
    let angle = atan2(fit64(d.1), fit64(d.0));
    let slice = i64::from(angle.0.checked_shr(22_u32).unwrap_or(0));
    u32::try_from(slice.rem_euclid(i64::from(sky_width.max(1_u32)))).unwrap_or(0)
}

/// An `i128` saturated into `i64`, for handing raw direction components
/// to [`atan2`].
fn fit64(value: i128) -> i64 {
    let low = i128::from(i64::MIN);
    let high = i128::from(i64::MAX);
    i64::try_from(value.clamp(low, high)).unwrap_or(0)
}

/// The world height whose texture row is `v = 0` for `plan`'s slot,
/// following the pegging rules: a pegged middle hangs from the near
/// ceiling, a pegged upper's bottom sits on the far ceiling, a pegged
/// lower's top sits on the far floor, and the unpegged variants measure
/// from a fixed world height instead.
pub(super) fn tex_top(plan: &FacePlan<'_>) -> i64 {
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

/// The world height whose texture row is `v = 0` for a masked middle.
///
/// Differs from a drawn middle's [`super::walk::Slot::Middle`] pegging:
/// pegged, the texture hangs from the lower of the two sectors' ceilings;
/// with the linedef's [`DONT_PEG_BOTTOM`] bit its bottom stands on the
/// higher of the two floors, `tex_height` below the top.
pub(super) fn masked_tex_top(
    near: &Plane<'_>,
    far: &Plane<'_>,
    tex_height: i64,
    lower_unpegged: bool,
) -> i64 {
    if lower_unpegged {
        near.floor.max(far.floor).saturating_add(tex_height)
    } else {
        near.ceil.min(far.ceil)
    }
}

/// The raw-16.16 horizontal texture position at a hit.
///
/// The distance along `v1 -> v2` at `s`, measured from `v2` for a
/// back-side facing, plus the faced sidedef's x shift: one mapping for
/// drawn faces and collected masked middles alike.
pub(super) fn texture_u16(visit: &Visit<'_>) -> i128 {
    let along = visit.s.times(visit.edge.len16);
    let base = if visit.front {
        along
    } else {
        visit.edge.len16.saturating_sub(along)
    };
    base.saturating_add(i128::from(visit.side.x_offset).saturating_mul(ONE_16))
}

/// Builds the mapped, shaded wall run from `plan`.
pub(super) fn part<'plan>(plan: &'plan FacePlan<'plan>) -> Part<'plan> {
    let visit = plan.visit;
    let side = visit.side;
    Part {
        image: plan.image,
        top: plan.span.0,
        bottom: plan.span.1,
        texcol: wrap16(texture_u16(visit), i128::from(plan.image.width)),
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
pub(super) fn wrap16(value: i128, size: i128) -> u32 {
    if size <= 0 {
        return 0;
    }
    let wrapped = value.rem_euclid(size.saturating_mul(ONE_16));
    u32::try_from(wrapped.div_euclid(ONE_16)).unwrap_or(0)
}

/// The fake-contrast orientation of a linedef vector.
pub(super) const fn wall_dir(ex: i64, ey: i64) -> WallDir {
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
pub(super) fn edge(line: usize, map: &Map, row: &Linedef) -> Option<Edge> {
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
        upper_unpegged: row.flags & DONT_PEG_TOP != 0_u32,
        lower_unpegged: row.flags & DONT_PEG_BOTTOM != 0_u32,
    })
}

/// One sidedef prepared for casting, `None` when its sector dangles.
pub(super) fn side<'scene>(
    index: usize,
    scene: &'scene Scene,
    sky: Option<Surface<'scene>>,
) -> Option<Side<'scene>> {
    let row = scene.map.sidedefs.get(index)?;
    scene.map.sectors.get(row.sector)?;
    Some(Side {
        plane: Plane::of(&scene.map, row.sector, scene, sky),
        upper: normalized_picture(&row.upper),
        lower: normalized_picture(&row.lower),
        middle: normalized_picture(&row.middle),
        x_offset: i64::from(row.x_offset),
        y_offset: i64::from(row.y_offset),
    })
}
