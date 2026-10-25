//! Pure stairs core: divide a straight path into step rectangles and
//! paint them over the map with the overlay engine.
//!
//! [`plan_stairs`] turns a [`StairSpec`] — the points `from` (in the
//! lower area) and `to` (in the higher), a `width`, a `step_height`, an
//! optional fixed `step_depth` and optional explicit endpoint floors —
//! into a [`StairPlan`]: `ceil(|floor_to - floor_from| / step_height)`
//! steps, their floors rising evenly from `floor_from` to `floor_to`,
//! each with the [`Sector`] record to paint (the base sector under the
//! step's centre: its flats and light, the step's heights, inert and
//! untagged), its rectangle polygon and its headroom test (the base
//! ceiling must sit at least [`MIN_HEADROOM`] units over the step's
//! new floor). [`build_stairs`] paints that plan with
//! [`crate::wad::map::overlay::apply_overlay`] (`Paint::Sector`,
//! [`crate::wad::map::overlay::Outside::Original`]) in ascending floor
//! order, so every step's riser faces are dressed with the caller's
//! `riser` texture, and reports the created sectors in path order.
//! Every refusal is a [`CyberError::InvalidArgument`] naming the fix,
//! and no refusal paints anything. Like the rest of [`crate::wad`]
//! this is pure model arithmetic: no sessions, no archives, no MCP.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::hypot;
use crate::geom::Pt;
use crate::geom::segment::{Delta, round_quotient};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::{Map, Sector};
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{Outside, OverlayRequest, Paint, WallTextures, apply_overlay};

/// The gentleest step height the builder accepts, in map units.
pub const MIN_STEP_HEIGHT: i32 = 1_i32;
/// The steepest step height the builder accepts: the tallest step a
/// `Doom` player climbs in one motion, shared from
/// [`consts`](crate::wad::map::consts).
pub use crate::wad::map::consts::MAX_STEP_HEIGHT;
/// The clear space a player needs above a step's floor.
pub const MIN_HEADROOM: i32 = 72_i32;
/// How far the path may miss `steps * step_depth` when a fixed
/// `step_depth` is asked for, in map units.
pub const DEPTH_TOLERANCE: i64 = 8_i64;
/// Divisor of the two-endpoint sum that gives a midpoint coordinate.
const MIDPOINT_DIVISOR: i128 = 2_i128;

/// Everything one staircase needs, as the tool layer resolved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StairSpec {
    /// The path's lower end, inside the area the stairs climb from.
    pub from: Point,
    /// The path's higher end, inside the area they climb into.
    pub to: Point,
    /// The width of every step, across the path, in map units.
    pub width: i32,
    /// The climb of one step, in map units (`MIN_STEP_HEIGHT..=MAX_STEP_HEIGHT`).
    pub step_height: i32,
    /// The fixed tread depth asked for, if any: the path must be
    /// `steps * depth` long (± [`DEPTH_TOLERANCE`]).
    pub step_depth: Option<i32>,
    /// The lower floor override; the sector at `from` answers by default.
    pub floor_from: Option<i32>,
    /// The higher floor override; the sector at `to` answers by default.
    pub floor_to: Option<i32>,
}

/// One planned step: its rectangle, its heights and the record to paint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StairStep {
    /// The step rectangle on the arrangement grid, corners in order.
    pub polygon: Vec<Pt>,
    /// The centre of the rectangle's path span, where the base sector
    /// was probed.
    pub centre: Point,
    /// The step's rising floor height.
    pub floor: i32,
    /// The step's ceiling: the base sector's ceiling, at least
    /// [`MIN_HEADROOM`] units over the floor.
    pub ceiling: i32,
    /// The sector record to paint: the base's flats and light, these
    /// heights, no effect and no tag.
    pub record: Sector,
}

/// A whole staircase as [`plan_stairs`] divided it, in path order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StairPlan {
    /// The resolved lower floor.
    pub floor_from: i32,
    /// The resolved higher floor.
    pub floor_to: i32,
    /// The steps, from `from` to `to`.
    pub steps: Vec<StairStep>,
}

/// One painted step: the sector it became and its heights.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct PaintedStep {
    /// Index in [`Map::sectors`] of the created step sector.
    pub sector: usize,
    /// The step's floor height.
    pub floor_height: i32,
    /// The step's ceiling height.
    pub ceiling_height: i32,
}

/// What one [`build_stairs`] call drew.
#[derive(Debug)]
pub struct StairBuild {
    /// The created steps, in path order (`from` first).
    pub steps: Vec<PaintedStep>,
    /// The resolved lower floor.
    pub floor_from: i32,
    /// The resolved higher floor.
    pub floor_to: i32,
    /// What the paints reported (emptied sectors, dropped lines).
    pub notes: Vec<String>,
}

/// The path's `from` to `to` vector with its floored integer length.
#[derive(Clone, Copy, Debug)]
struct PathRun {
    /// The `x` component, exact.
    dx: i128,
    /// The `y` component, exact.
    dy: i128,
    /// The floored [`hypot`] length in map units, never zero.
    length: i64,
}

/// The resolved climb: its endpoints' floors and the step count.
#[derive(Clone, Copy, Debug)]
struct Climb {
    /// The lower floor.
    floor_from: i32,
    /// The higher floor.
    floor_to: i32,
    /// `ceil(|floor_to - floor_from| / step_height)`.
    count: usize,
}

/// Everything the step loop shares: the request, the path and the climb.
#[derive(Clone, Copy, Debug)]
struct Prep {
    /// The request being planned.
    spec: StairSpec,
    /// The path vector.
    run: PathRun,
    /// The resolved climb.
    climb: Climb,
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// An exact `i128` axis saturated into `i32`.
fn narrow_axis(value: i128) -> i32 {
    match i32::try_from(value) {
        Ok(narrow) => narrow,
        Err(_) if value < 0_i128 => i32::MIN,
        Err(_) => i32::MAX,
    }
}

/// An exact `i128` axis saturated into `i64`.
fn wide_axis(value: i128) -> i64 {
    match i64::try_from(value) {
        Ok(wide) => wide,
        Err(_) if value < 0_i128 => i64::MIN,
        Err(_) => i64::MAX,
    }
}

/// A `usize` widened to exact arithmetic, saturated (defensive).
fn tall(value: usize) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

/// A `usize` widened into `i64`, saturated (defensive).
fn tall64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Refuses a non-positive width, an out-of-range step height and a
/// non-positive step depth.
fn check_shape(spec: &StairSpec) -> Result<(), CyberError> {
    if spec.width <= 0_i32 {
        return Err(invalid(
            "the stair width must be a positive number of map units (the default 64 \
             lets two players pass)",
        ));
    }
    if !(MIN_STEP_HEIGHT..=MAX_STEP_HEIGHT).contains(&spec.step_height) {
        return Err(invalid(&format!(
            "step_height {} is out of range: one step climbs between \
             {MIN_STEP_HEIGHT} and {MAX_STEP_HEIGHT} map units (the player steps up \
             at most {MAX_STEP_HEIGHT}; 16 is gentler)",
            spec.step_height
        )));
    }
    if spec.step_depth.is_some_and(|depth| depth <= 0_i32) {
        return Err(invalid(
            "step_depth must be a positive number of map units (the tread every step \
             gets along the path)",
        ));
    }
    Ok(())
}

/// The `from` to `to` vector, refusing the zero-length path.
fn path_run(spec: &StairSpec) -> Result<PathRun, CyberError> {
    let dx = i128::from(spec.to.x).saturating_sub(i128::from(spec.from.x));
    let dy = i128::from(spec.to.y).saturating_sub(i128::from(spec.from.y));
    let length = i64::try_from(hypot(wide_axis(dx), wide_axis(dy))).unwrap_or(i64::MAX);
    if length == 0_i64 {
        return Err(invalid(
            "the stair path is zero-length: `from` and `to` name the same point; \
             give a path that runs from the lower area into the higher one",
        ));
    }
    Ok(PathRun { dx, dy, length })
}

/// The sector holding `point`, `None` when no sector resolves there.
fn sector_under(map: &Map, point: Point) -> Option<&Sector> {
    point_in_sector(map, i64::from(point.x), i64::from(point.y))
        .and_then(|index| map.sectors.get(index))
}

/// One endpoint's floor: the explicit `given` one, else the sector at
/// `point`; a point in no sector refuses with its `role` named.
fn endpoint_floor(
    map: &Map,
    point: Point,
    role: &str,
    given: Option<i32>,
) -> Result<i32, CyberError> {
    let base = sector_under(map, point).ok_or_else(|| {
        invalid(&format!(
            "the {role} point {}, {} lies in no sector: `from` stands in the lower \
             area and `to` in the higher one; move the point onto a sector \
             (get_map_summary lists the rooms)",
            point.x, point.y
        ))
    })?;
    Ok(given.unwrap_or(base.floor_height))
}

/// The two endpoint floors, checked against the sectors at both points.
fn endpoint_floors(map: &Map, spec: &StairSpec) -> Result<(i32, i32), CyberError> {
    let floor_from = endpoint_floor(map, spec.from, "from", spec.floor_from)?;
    let floor_to = endpoint_floor(map, spec.to, "to", spec.floor_to)?;
    Ok((floor_from, floor_to))
}

/// The step count for one climb: `ceil(|rise| / step_height)`, refusing
/// a flat path and a count past any workable number.
fn climb_count(floor_from: i32, floor_to: i32, step_height: i32) -> Result<usize, CyberError> {
    let rise = i64::from(floor_to).saturating_sub(i64::from(floor_from));
    if rise == 0_i64 {
        return Err(invalid(&format!(
            "both ends of the path sit at floor {floor_from}: there is no step to \
             build; move `to` into the higher area or pass a floor_from/floor_to \
             pair that differs",
        )));
    }
    let climb = rise.unsigned_abs();
    let height = u64::try_from(step_height).unwrap_or(1_u64).max(1_u64);
    let count = climb.div_ceil(height);
    usize::try_from(count).map_err(|_error| {
        CyberError::LimitExceeded(format!(
            "this climb would run {count} steps, past any workable stair"
        ))
    })
}

/// Refuses a fixed `step_depth` the path cannot fit: it wants
/// `steps * depth` length, ± [`DEPTH_TOLERANCE`], and the refusal
/// states that length and the depth that would divide the path evenly.
fn check_depth(run: &PathRun, count: usize, depth: i32) -> Result<(), CyberError> {
    let wanted = i64::from(depth).saturating_mul(tall64(count));
    if run.length.saturating_sub(wanted).abs() <= DEPTH_TOLERANCE {
        return Ok(());
    }
    let length = i128::from(run.length);
    let fit = round_quotient(length, tall(count)).unwrap_or(length);
    Err(invalid(&format!(
        "step_depth {depth} with {count} steps wants a path {wanted} map units long \
         (tolerance {DEPTH_TOLERANCE}): this path is {} units; pass step_depth {} to \
         divide it evenly, or drop step_depth to let the steps fill the path",
        run.length,
        narrow_axis(fit),
    )))
}

/// Every check up front, then the shared plan inputs.
fn prepare(map: &Map, spec: &StairSpec) -> Result<Prep, CyberError> {
    check_shape(spec)?;
    let run = path_run(spec)?;
    let (floor_from, floor_to) = endpoint_floors(map, spec)?;
    let count = climb_count(floor_from, floor_to, spec.step_height)?;
    if let Some(depth) = spec.step_depth {
        check_depth(&run, count, depth)?;
    }
    if run.length < tall64(count) {
        return Err(invalid(&format!(
            "the stair path is only {} map units long: {count} steps need at least a \
             unit of tread each; shorten the climb or lengthen the path",
            run.length
        )));
    }
    Ok(Prep {
        spec: *spec,
        run,
        climb: Climb {
            floor_from,
            floor_to,
            count,
        },
    })
}

/// One path axis at fraction `index / count`, rounded to the nearest
/// whole map unit (halves away from zero).
fn axis_of(base: i32, axis: i128, index: i128, count: i128) -> i32 {
    let shift = round_quotient(axis.saturating_mul(index), count).unwrap_or(0_i128);
    narrow_axis(i128::from(base).saturating_add(shift))
}

/// The point `index / count` of the way along the path.
fn along(prep: &Prep, index: usize) -> Point {
    let count = tall(prep.climb.count);
    let at = i128::try_from(index).unwrap_or(count);
    Point {
        x: axis_of(prep.spec.from.x, prep.run.dx, at, count),
        y: axis_of(prep.spec.from.y, prep.run.dy, at, count),
    }
}

/// The floor of step `index` (1-based up the climb): the even division
/// of the rise, rounded, so the last step lands exactly on `floor_to`.
fn step_floor(climb: Climb, index: usize) -> i32 {
    let count = tall(climb.count);
    let at = i128::try_from(index).unwrap_or(count);
    let rise = i128::from(climb.floor_to).saturating_sub(i128::from(climb.floor_from));
    let shift = round_quotient(rise.saturating_mul(at), count).unwrap_or(0_i128);
    narrow_axis(i128::from(climb.floor_from).saturating_add(shift))
}

/// The path's left perpendicular, `width` wide in total (half each
/// side), rounded to whole units.
fn side_offset(run: &PathRun, width: i32) -> Delta {
    let den = i128::from(run.length).saturating_mul(2_i128).max(2_i128);
    let across = i128::from(width);
    (
        round_quotient(run.dy.saturating_neg().saturating_mul(across), den).unwrap_or(0_i128),
        round_quotient(run.dx.saturating_mul(across), den).unwrap_or(0_i128),
    )
}

/// One rectangle corner: `point` moved `sign` half-widths across the path.
fn corner(point: Point, off: &Delta, sign: i128) -> Pt {
    (
        i64::from(point.x).saturating_add(wide_axis(off.0.saturating_mul(sign))),
        i64::from(point.y).saturating_add(wide_axis(off.1.saturating_mul(sign))),
    )
}

/// The step rectangle: the path span `near` to `far`, widened across
/// the path by `off` on both sides.
fn rectangle(near: Point, far: Point, off: &Delta) -> Vec<Pt> {
    vec![
        corner(near, off, 1_i128),
        corner(far, off, 1_i128),
        corner(far, off, -1_i128),
        corner(near, off, -1_i128),
    ]
}

/// The midpoint of two path points, halves rounded away from zero.
fn middle(from: Point, to: Point) -> Point {
    let half = |first: i32, second: i32| {
        let sum = i128::from(first).saturating_add(i128::from(second));
        narrow_axis(round_quotient(sum, MIDPOINT_DIVISOR).unwrap_or(sum))
    };
    Point {
        x: half(from.x, to.x),
        y: half(from.y, to.y),
    }
}

/// The base sector under a step's centre, refusing a path that crosses
/// solid rock (steps are 1-based in the message).
fn probe_base(map: &Map, centre: Point, index: usize) -> Result<&Sector, CyberError> {
    sector_under(map, centre).ok_or_else(|| {
        invalid(&format!(
            "step {} at {}, {} lies in no sector: the stair path crosses solid rock; \
             reroute it through the rooms",
            index.saturating_add(1),
            centre.x,
            centre.y
        ))
    })
}

/// Refuses a step whose base ceiling sits under `floor + MIN_HEADROOM`.
fn require_headroom(floor: i32, ceiling: i32, index: usize) -> Result<(), CyberError> {
    if ceiling >= floor.saturating_add(MIN_HEADROOM) {
        return Ok(());
    }
    Err(invalid(&format!(
        "step {} would stand at floor {floor} under a ceiling of {ceiling}: not \
         enough headroom (a player needs {MIN_HEADROOM} map units); raise the \
         ceiling over the path or end the climb lower",
        index.saturating_add(1),
    )))
}

/// The step's sector record: the base's flats and light, the step's
/// heights, no effect and no tag.
fn step_record(base: &Sector, floor: i32, ceiling: i32) -> Sector {
    let mut record = base.clone();
    record.floor_height = floor;
    record.ceiling_height = ceiling;
    record.special = 0_i32;
    record.tag = 0_i32;
    record
}

/// One planned step: its span, floor, probed base and dressed record.
fn plan_step(map: &Map, prep: &Prep, index: usize) -> Result<StairStep, CyberError> {
    let near = along(prep, index);
    let far = along(prep, index.saturating_add(1));
    let floor = step_floor(prep.climb, index.saturating_add(1));
    let centre = middle(near, far);
    let base = probe_base(map, centre, index)?;
    let ceiling = base.ceiling_height;
    require_headroom(floor, ceiling, index)?;
    Ok(StairStep {
        polygon: rectangle(near, far, &side_offset(&prep.run, prep.spec.width)),
        centre,
        floor,
        ceiling,
        record: step_record(base, floor, ceiling),
    })
}

/// Divide a staircase path into its steps without touching the map.
///
/// The step count is `ceil(|floor_to - floor_from| / step_height)`; the
/// floors rise evenly (the last step lands exactly on `floor_to`), each
/// step is a `width`-wide rectangle centred on one `count`-th of the
/// straight `from` to `to` path, and every step's ceiling is the base
/// sector's ceiling at the step's centre — at least [`MIN_HEADROOM`]
/// units over the step's floor.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-positive width or
/// `step_depth`, a `step_height` outside `MIN_STEP_HEIGHT..=MAX_STEP_HEIGHT`,
/// a zero-length path, an endpoint in no sector, a flat path, a
/// `step_depth` the path length misses by more than [`DEPTH_TOLERANCE`],
/// a path shorter than its step count, a centre in solid rock or a step
/// without headroom; [`CyberError::LimitExceeded`] for an absurd count.
#[inline]
pub fn plan_stairs(map: &Map, spec: &StairSpec) -> Result<StairPlan, CyberError> {
    let prep = prepare(map, spec)?;
    let mut steps = Vec::with_capacity(prep.climb.count);
    for index in 0..prep.climb.count {
        steps.push(plan_step(map, &prep, index)?);
    }
    Ok(StairPlan {
        floor_from: prep.climb.floor_from,
        floor_to: prep.climb.floor_to,
        steps,
    })
}

/// The overlay request painting one step, risers in `riser`.
fn step_request(step: &StairStep, riser: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: step.polygon.clone(),
        paint: Paint::Sector(step.record.clone()),
        keep_interior: false,
        textures: WallTextures {
            wall: riser.to_owned(),
            upper: riser.to_owned(),
            lower: riser.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// The server-bug refusal for a step paint that reported no sector.
fn paint_lost(index: usize) -> CyberError {
    CyberError::Internal(format!(
        "the stair paint for step {} reported no sector (server bug)",
        index.saturating_add(1)
    ))
}

/// Paint one plan bottom-up: the steps in ascending floor order, so
/// every riser faces the paint that raises over it.
fn paint_plan(map: &mut Map, plan: &StairPlan, riser: &str) -> Result<StairBuild, CyberError> {
    let mut order: Vec<(i32, usize, &StairStep)> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.floor, index, step))
        .collect();
    order.sort_by_key(|entry| (entry.0, entry.1));
    let mut painted: BTreeMap<usize, PaintedStep> = BTreeMap::new();
    let mut notes = Vec::new();
    for (floor, index, step) in order {
        let drawn = apply_overlay(map, &step_request(step, riser))?;
        let sector = drawn.sector.ok_or_else(|| paint_lost(index))?;
        painted.insert(
            index,
            PaintedStep {
                sector,
                floor_height: floor,
                ceiling_height: step.ceiling,
            },
        );
        notes.extend(drawn.notes);
    }
    Ok(StairBuild {
        steps: painted.into_values().collect(),
        floor_from: plan.floor_from,
        floor_to: plan.floor_to,
        notes,
    })
}

/// Plan and paint a whole staircase as one map edit.
///
/// Every check of [`plan_stairs`] runs before a single paint, so a
/// refused call leaves `map` untouched.
///
/// # Errors
///
/// [`plan_stairs`]' refusals and [`apply_overlay`]'s (a degenerate
/// polygon is refused by the arrangement before anything is written).
#[inline]
pub fn build_stairs(
    map: &mut Map,
    spec: &StairSpec,
    riser: &str,
) -> Result<StairBuild, CyberError> {
    let plan = plan_stairs(map, spec)?;
    paint_plan(map, &plan, riser)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_STEP_HEIGHT, MIN_HEADROOM, MIN_STEP_HEIGHT, PaintedStep, StairSpec, build_stairs,
        check_shape, endpoint_floor, plan_stairs, probe_base,
    };
    use crate::error::CyberError;
    use crate::wad::map::common::fixtures::two_room_map;
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::ops::Point;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    fn refuse(error: &CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    /// A point at `x`/`y`.
    const fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// The fixture path: straight east across the step line, y centred.
    fn spec() -> StairSpec {
        StairSpec {
            from: point(64, 128),
            to: point(448, 128),
            width: 64_i32,
            step_height: 16_i32,
            step_depth: None,
            floor_from: None,
            floor_to: None,
        }
    }

    #[test]
    fn the_fixture_climb_divides_into_even_steps() {
        let map = two_room_map();
        let plan = plan_stairs(&map, &spec()).expect("0 to 32 in 16-unit steps");
        assert_eq!((plan.floor_from, plan.floor_to), (0_i32, 32_i32), "probed");
        assert_eq!(plan.steps.len(), 2_usize, "two 16-unit steps");
        let first = plan.steps.first().expect("the low step");
        assert_eq!(first.floor, 16_i32, "the even rise");
        assert_eq!(first.ceiling, 128_i32, "the hall's ceiling");
        assert_eq!(first.centre, point(160, 128), "mid of 64..256");
        assert_eq!(
            first.polygon,
            vec![(64, 160), (256, 160), (256, 96), (64, 96)],
            "the 64-wide band centred on the path"
        );
        assert_eq!(first.record.floor_flat, "FLOOR1", "the base's flat");
        assert_eq!(
            (first.record.special, first.record.tag),
            (0_i32, 0_i32),
            "inert and untagged"
        );
        let second = plan.steps.get(1).expect("the top step");
        assert_eq!(second.floor, 32_i32, "exactly floor_to");
        assert_eq!(second.centre, point(352, 128), "mid of 256..448");
        assert_eq!(
            (first.polygon.get(1), first.polygon.get(2)),
            (second.polygon.first(), second.polygon.get(3)),
            "the steps share their riser line exactly"
        );
    }

    #[test]
    fn explicit_floors_move_the_division() {
        let map = two_room_map();
        let wanted = StairSpec {
            floor_to: Some(48_i32),
            ..spec()
        };
        let plan = plan_stairs(&map, &wanted).expect("an explicit 48-unit climb");
        let floors: Vec<i32> = plan.steps.iter().map(|step| step.floor).collect();
        assert_eq!(floors, vec![16_i32, 32_i32, 48_i32], "three even steps");
    }

    #[test]
    fn odd_widths_round_the_rails_outward() {
        let map = two_room_map();
        let wanted = StairSpec {
            width: 65_i32,
            ..spec()
        };
        let plan = plan_stairs(&map, &wanted).expect("an odd width");
        let corners = &plan.steps.first().expect("the low step").polygon;
        assert_eq!(
            corners.first(),
            Some(&(64, 161)),
            "32.5 rounds away from zero"
        );
        assert_eq!(corners.last(), Some(&(64, 95)), "on both sides");
    }

    #[test]
    fn step_heights_stay_between_one_and_twenty_four() {
        for height in [MIN_STEP_HEIGHT, 16_i32, MAX_STEP_HEIGHT] {
            let wanted = StairSpec {
                step_height: height,
                ..spec()
            };
            check_shape(&wanted).expect("the boundary heights are climbable");
        }
        for height in [0_i32, 25_i32, -16_i32] {
            let wanted = StairSpec {
                step_height: height,
                ..spec()
            };
            let message = refuse(&check_shape(&wanted).unwrap_err());
            assert!(message.contains("out of range"), "{height}: {message}");
        }
    }

    #[test]
    fn widths_and_depths_must_be_positive() {
        let thin = StairSpec {
            width: 0_i32,
            ..spec()
        };
        assert!(refuse(&check_shape(&thin).unwrap_err()).contains("width"));
        let flat = StairSpec {
            step_depth: Some(0_i32),
            ..spec()
        };
        assert!(refuse(&check_shape(&flat).unwrap_err()).contains("step_depth"));
    }

    #[test]
    fn a_flat_path_and_a_standing_point_refuse() {
        let map = two_room_map();
        let flat = StairSpec {
            to: point(200, 64),
            ..spec()
        };
        let message = refuse(&plan_stairs(&map, &flat).unwrap_err());
        assert!(message.contains("both ends"), "the same floor: {message}");
        let void = StairSpec {
            from: point(3000, 3000),
            ..spec()
        };
        let void_note = refuse(&plan_stairs(&map, &void).unwrap_err());
        assert!(void_note.contains("from point 3000, 3000"), "{void_note}");
        let beyond = refuse(&endpoint_floor(&map, point(5000, 128), "to", None).unwrap_err());
        assert!(beyond.contains("to point 5000, 128"), "{beyond}");
    }

    #[test]
    fn a_path_through_rock_refuses_its_step() {
        let map = two_room_map();
        let message = refuse(&probe_base(&map, point(5000, 5000), 2).unwrap_err());
        assert!(message.contains("step 3 at 5000, 5000"), "{message}");
        assert!(message.contains("solid rock"), "{message}");
    }

    #[test]
    fn a_low_ceiling_over_a_high_step_refuses() {
        let map = two_room_map();
        let wanted = StairSpec {
            floor_to: Some(64_i32),
            ..spec()
        };
        let message = refuse(&plan_stairs(&map, &wanted).unwrap_err());
        assert!(message.contains("not enough headroom"), "{message}");
        assert!(
            message.contains(&format!(
                "floor 64 under a ceiling of 128: not enough headroom (a player needs \
                 {MIN_HEADROOM} map units)"
            )),
            "the refusal carries the live constant: {message}"
        );
    }

    #[test]
    fn a_fixed_depth_must_match_the_path() {
        let map = two_room_map();
        let off = StairSpec {
            step_depth: Some(24_i32),
            ..spec()
        };
        let message = refuse(&plan_stairs(&map, &off).unwrap_err());
        assert!(message.contains("step_depth 24 with 2 steps"), "{message}");
        assert!(message.contains("wants a path 48 map units"), "{message}");
        assert!(
            message.contains("step_depth 192"),
            "the fit is named: {message}"
        );
        for depth in [188_i32, 196_i32] {
            let wanted = StairSpec {
                step_depth: Some(depth),
                ..spec()
            };
            assert!(
                plan_stairs(&map, &wanted).is_ok(),
                "{depth} sits within the tolerance of 192"
            );
        }
    }

    #[test]
    fn a_path_shorter_than_its_steps_refuses() {
        let map = two_room_map();
        let cramped = StairSpec {
            to: point(66, 128),
            step_height: 1_i32,
            floor_to: Some(64_i32),
            ..spec()
        };
        let message = refuse(&plan_stairs(&map, &cramped).unwrap_err());
        assert!(message.contains("only 2 map units long"), "{message}");
    }

    #[test]
    fn build_paints_the_steps_bottom_up() {
        let mut map = two_room_map();
        let build = build_stairs(&mut map, &spec(), "STEP1").expect("a drawn staircase");
        assert_eq!(
            build.steps,
            vec![
                PaintedStep {
                    sector: 2_usize,
                    floor_height: 16_i32,
                    ceiling_height: 128_i32,
                },
                PaintedStep {
                    sector: 3_usize,
                    floor_height: 32_i32,
                    ceiling_height: 128_i32,
                },
            ],
            "path order, ascending sectors"
        );
        assert_eq!((build.floor_from, build.floor_to), (0_i32, 32_i32));
        assert!(
            build.notes.is_empty(),
            "no emptied sectors: {:?}",
            build.notes
        );
        assert_eq!(map.sectors.len(), 4_usize, "two rooms and two steps");
        assert_eq!(map.sectors.get(2).expect("step 1").floor_height, 16_i32);
        assert_eq!(map.sectors.first().expect("hall").floor_height, 0_i32);
        assert_eq!(map.sectors.get(1).expect("room").floor_height, 32_i32);
        assert_eq!(point_in_sector(&map, 160, 128), Some(2_usize));
        assert_eq!(point_in_sector(&map, 352, 128), Some(3_usize));
        let riser = map
            .sidedefs
            .iter()
            .any(|side| side.sector == 2_usize && side.lower == "STEP1");
        assert!(riser, "the step wears its riser texture");
    }

    #[test]
    fn a_descending_path_still_paints_the_low_step_first() {
        let mut map = two_room_map();
        let down = StairSpec {
            from: point(352, 128),
            to: point(64, 128),
            ..spec()
        };
        let build = build_stairs(&mut map, &down, "STEP1").expect("a stair down to the hall");
        assert_eq!(
            build
                .steps
                .iter()
                .map(|step| (step.sector, step.floor_height))
                .collect::<Vec<(usize, i32)>>(),
            vec![(3_usize, 16_i32), (2_usize, 0_i32)],
            "path order, but the floor-0 step was appended first"
        );
    }

    #[test]
    fn a_refused_build_touches_the_map_nothing() {
        let mut map = two_room_map();
        let before = map.clone();
        let wanted = StairSpec {
            floor_to: Some(64_i32),
            ..spec()
        };
        assert_eq!(
            code(&build_stairs(&mut map, &wanted, "STEP1").unwrap_err()),
            "invalid_argument"
        );
        assert_eq!(map, before, "the headroom refusal painted nothing");
    }
}
