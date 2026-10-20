//! Pure thing-placement core: the clearance primitives the plan builder's
//! room fills share with the `place_things` distribution engine.
//!
//! Nothing here touches archives, sessions or `MCP` types. [`fits`] answers
//! whether a thing of one clearance may stand in a sector; [`keeps_distance`]
//! is the exact `i128` point-to-segment test every clearance check runs on;
//! [`axis_points`] walks a grid. On top of them [`distribute`] spreads whole
//! areas: for each [`Area`] it lays a square grid spaced twice the clearance
//! (at least 16 units) centred in the area's bounding box, keeps
//! the points inside the area and clear of every one-sided or blocking
//! line and of every existing thing, orders what is kept by distance from
//! the area centre (ties by `y` then `x`) and spreads the picks by taking
//! every k-th candidate. [`split_proportional`] answers how many of a total
//! belong to each sector: by bounding-box area, largest-remainder, ties to
//! the lower sector index. [`Facing`] chooses each pick's angle, snapped to
//! multiples of 45 degrees; [`Facing::Seeded`] is the engine's only allowed
//! randomness: a xorshift32 advanced deterministically from the caller's
//! `seed`.

use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::fixed::atan2;
use crate::geom::Pt;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::geometry::{point_in_sector, sector_bbox, sector_boundary};
use crate::wad::map::model::{Linedef, Map};

/// The thing type of the player 1 start.
const PLAYER_ONE_START: i32 = 1_i32;
/// The blocking radius assumed for a type the definitions do not list,
/// as the plan builder's room fills use it.
pub const DEFAULT_RADIUS: i64 = 16_i64;
/// Extra clearance every placed thing keeps from the sector boundary,
/// on top of its blocking radius, map units.
pub const CLEARANCE: i64 = 8_i64;
/// The smallest grid spacing [`distribute`] ever lays, map units.
const MIN_SPACING: i64 = 16_i64;
/// The largest number of grid points one area axis contributes, so a
/// huge bounding box cannot spin the fill forever.
const MAX_AXIS_POINTS: usize = 4_096_usize;
/// The largest candidate grid one area contributes; the spacing doubles
/// until the product of the two axes fits.
const MAX_CANDIDATES: usize = 0x1_0000_usize;
/// Divisor of the two-endpoint sum that gives a midpoint coordinate.
const MIDPOINT_DIVISOR: i64 = 2_i64;
/// Divisor turning an axis slack into the centring offset.
const AXIS_SLACK_DIVISOR: i64 = 2_i64;
/// Degrees in a full circle; facings normalise into `0..=359`.
const DEGREES: i32 = 360_i32;
/// Every facing snaps to a multiple of this many degrees.
const ANGLE_STEP: i32 = 45_i32;
/// Half the snap step: the rounding bias of [`snap_angle`].
const HALF_ANGLE_STEP: i32 = 22_i32;
/// The seeded facing draws one of this many compass points.
const OCTANTS: u32 = 8_u32;
/// Degrees between two compass points of the seeded facing.
const OCTANT_DEGREES: u32 = 45_u32;
/// The `blocking` linedef bit, the same value in both flag families.
const BLOCKING: u32 = 1_u32;
/// The `Hexen` `blockplayers` bit: the player cannot cross, monsters can.
const BLOCK_PLAYERS: u32 = 0x4000;
/// The `Hexen` `blockeverything` bit.
const BLOCK_EVERYTHING: u32 = 0x8000;
/// The xorshift32 shifts of [`next_u32`].
const XOR_SHIFT_A: u32 = 13_u32;
/// Second xorshift32 shift.
const XOR_SHIFT_B: u32 = 17_u32;
/// Third xorshift32 shift.
const XOR_SHIFT_C: u32 = 5_u32;

/// A map point widened to the exact `i128` space of the distance tests.
type End = (i128, i128);

/// The whole midpoint of an axis, `i64`-safe for any `i32` pair.
#[must_use]
#[inline]
pub const fn mid(low: i64, high: i64) -> i64 {
    low.saturating_add(high).saturating_div(MIDPOINT_DIVISOR)
}

/// The blocking radius of thing type `kind`: its definition's radius, or
/// `unknown_radius` when the definitions do not list the type.
#[must_use]
#[inline]
pub fn thing_radius(definitions: &Definitions, kind: u32, unknown_radius: i64) -> i64 {
    definitions
        .thing(kind)
        .map_or(unknown_radius, |row| i64::from(row.radius))
}

/// The wall clearance of thing type `kind`: [`thing_radius`] with
/// `unknown_radius` as the fallback, plus [`CLEARANCE`] map units.
#[must_use]
#[inline]
pub fn thing_clearance(definitions: &Definitions, kind: u32, unknown_radius: i64) -> i64 {
    thing_radius(definitions, kind, unknown_radius).saturating_add(CLEARANCE)
}

/// Whether `point` keeps at least `min` distance from the segment
/// `from`-`to`, exact in `i128`: squared distances are compared as
/// integers, the perpendicular case as `cross^2 >= min^2 * len^2`.
#[must_use]
#[inline]
pub const fn keeps_distance(from: End, to: End, point: End, min: i128) -> bool {
    let (ax, ay) = from;
    let (bx, by) = to;
    let (px, py) = point;
    let dx = bx.saturating_sub(ax);
    let dy = by.saturating_sub(ay);
    let wx = px.saturating_sub(ax);
    let wy = py.saturating_sub(ay);
    let len2 = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
    let near2 = wx.saturating_mul(wx).saturating_add(wy.saturating_mul(wy));
    let need = min.saturating_mul(min);
    let dot = wx.saturating_mul(dx).saturating_add(wy.saturating_mul(dy));
    if len2 == 0_i128 || dot <= 0_i128 {
        return near2 >= need;
    }
    if dot >= len2 {
        let ex = px.saturating_sub(bx);
        let ey = py.saturating_sub(by);
        return ex.saturating_mul(ex).saturating_add(ey.saturating_mul(ey)) >= need;
    }
    let cross = dx.saturating_mul(wy).saturating_sub(dy.saturating_mul(wx));
    cross.saturating_mul(cross) >= need.saturating_mul(len2)
}

/// Both endpoints of `linedef` widened to `i128`, when they resolve.
fn segment_endpoints(map: &Map, linedef: usize) -> Option<(End, End)> {
    let line = map.linedefs.get(linedef)?;
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    Some((
        (i128::from(first.x), i128::from(first.y)),
        (i128::from(second.x), i128::from(second.y)),
    ))
}

/// Whether the line `linedef` keeps at least `clearance` from `at`.
#[must_use]
#[inline]
pub fn line_clear(map: &Map, linedef: usize, at: Pt, clearance: i64) -> bool {
    let Some((from, to)) = segment_endpoints(map, linedef) else {
        return true;
    };
    keeps_distance(from, to, wide(at), i128::from(clearance))
}

/// Whether every boundary line of `sector` keeps at least `clearance`
/// map units from `at`; lines with unresolvable endpoints never bound
/// anything and stay clear.
#[must_use]
#[inline]
pub fn boundary_clear(map: &Map, sector: usize, at: Pt, clearance: i64) -> bool {
    sector_boundary(map, sector)
        .iter()
        .all(|&(index, _)| line_clear(map, index, at, clearance))
}

/// Whether a thing with this `clearance` may stand at `at` in `sector`:
/// the sector contains the point and every boundary line of the sector
/// keeps at least `clearance` map units of distance from it.
#[must_use]
#[inline]
pub fn fits(map: &Map, sector: usize, at: Pt, clearance: i64) -> bool {
    let (x, y) = at;
    point_in_sector(map, x, y) == Some(sector) && boundary_clear(map, sector, at, clearance)
}

/// The grid values from `start` to `last` inclusive, `step` apart,
/// capped at 4 096 points per axis.
#[must_use]
#[inline]
pub fn axis_points(start: i64, last: i64, step: i64) -> Vec<i64> {
    let mut points = Vec::new();
    let mut value = start;
    while value <= last && points.len() < MAX_AXIS_POINTS {
        points.push(value);
        value = value.saturating_add(step);
    }
    points
}

/// One target area [`distribute`] fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// A sector of the map: points the sector itself contains.
    Sector(
        /// The sector index.
        usize,
    ),
    /// An axis-aligned box in map units (inclusive edges).
    Rect {
        /// The lower-left corner `X`.
        min_x: i64,
        /// The lower-left corner `Y`.
        min_y: i64,
        /// The upper-right corner `X`.
        max_x: i64,
        /// The upper-right corner `Y`.
        max_y: i64,
    },
    /// A circle in map units.
    Circle {
        /// The centre point.
        centre: Pt,
        /// The radius, map units.
        radius: i64,
    },
}

impl Area {
    /// The area's bounding box `(min_x, min_y, max_x, max_y)` in map
    /// units, `None` when it bounds nothing (a sector with no resolvable
    /// boundary).
    #[must_use]
    #[inline]
    pub fn bbox(&self, map: &Map) -> Option<(i64, i64, i64, i64)> {
        match *self {
            Self::Sector(sector) => sector_bbox(map, sector).map(|(x1, y1, x2, y2)| {
                (i64::from(x1), i64::from(y1), i64::from(x2), i64::from(y2))
            }),
            Self::Rect {
                min_x,
                min_y,
                max_x,
                max_y,
            } => Some((min_x, min_y, max_x, max_y)),
            Self::Circle { centre, radius } => Some((
                centre.0.saturating_sub(radius),
                centre.1.saturating_sub(radius),
                centre.0.saturating_add(radius),
                centre.1.saturating_add(radius),
            )),
        }
    }

    /// The area's centre: the circle's own, every other area the bounding
    /// box midpoint; `None` when the area bounds nothing.
    #[must_use]
    #[inline]
    pub fn centre(&self, map: &Map) -> Option<Pt> {
        match *self {
            Self::Circle { centre, .. } => Some(centre),
            Self::Sector(_) | Self::Rect { .. } => self
                .bbox(map)
                .map(|(min_x, min_y, max_x, max_y)| (mid(min_x, max_x), mid(min_y, max_y))),
        }
    }

    /// Whether the point lies inside the area. A sector tests its own
    /// containment; the open shapes additionally require the point to sit
    /// in *some* sector of the map, so nothing lands in the void.
    #[must_use]
    #[inline]
    pub fn contains(&self, map: &Map, at: Pt) -> bool {
        match *self {
            Self::Sector(sector) => point_in_sector(map, at.0, at.1) == Some(sector),
            Self::Rect {
                min_x,
                min_y,
                max_x,
                max_y,
            } => {
                let inside = at.0 >= min_x && at.0 <= max_x && at.1 >= min_y && at.1 <= max_y;
                inside && point_in_sector(map, at.0, at.1).is_some()
            }
            Self::Circle { centre, radius } => {
                within_circle(centre, radius, at) && point_in_sector(map, at.0, at.1).is_some()
            }
        }
    }

    /// What warnings call the area.
    #[must_use]
    #[inline]
    pub fn label(&self) -> String {
        match *self {
            Self::Sector(sector) => format!("sector {sector}"),
            Self::Rect { .. } => "the bounding box".to_owned(),
            Self::Circle { .. } => "the circle".to_owned(),
        }
    }
}

/// Whether `at` sits within `radius` of `centre`, exact in `i128`.
fn within_circle(centre: Pt, radius: i64, at: Pt) -> bool {
    let dx = i128::from(at.0).saturating_sub(i128::from(centre.0));
    let dy = i128::from(at.1).saturating_sub(i128::from(centre.1));
    let wide = i128::from(radius);
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) <= wide.saturating_mul(wide)
}

/// The squared distance from `at` to `centre`, for the ordering.
fn distance_squared(centre: Pt, at: Pt) -> i128 {
    let dx = i128::from(at.0).saturating_sub(i128::from(centre.0));
    let dy = i128::from(at.1).saturating_sub(i128::from(centre.1));
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

/// The `point` widened into the exact `i128` test space.
fn wide(point: Pt) -> End {
    (i128::from(point.0), i128::from(point.1))
}

/// Whether `line` is a wall to placement: one-sided, or carrying a bit
/// that stops bodies in the map's own flag family.
const fn is_wall(line: &Linedef, family: FlagFamily) -> bool {
    if line.back.is_none() {
        return true;
    }
    if line.flags & BLOCKING == BLOCKING {
        return true;
    }
    matches!(family, FlagFamily::Hexen)
        && (line.flags & BLOCK_PLAYERS == BLOCK_PLAYERS
            || line.flags & BLOCK_EVERYTHING == BLOCK_EVERYTHING)
}

/// The segments placement candidates keep clear of: every wall line of
/// the map whose endpoints resolve.
fn wall_segments(map: &Map) -> Vec<(End, End)> {
    let family = map.flag_family();
    map.linedefs
        .iter()
        .enumerate()
        .filter(|entry| is_wall(entry.1, family))
        .filter_map(|entry| segment_endpoints(map, entry.0))
        .collect()
}

/// A thing already standing where candidates are measured against.
#[derive(Clone, Copy, Debug)]
pub struct Occupant {
    /// Where it stands.
    pub at: Pt,
    /// Its blocking radius, map units.
    pub radius: i64,
}

/// The existing things of `map` as obstacles: each row's position and
/// blocking radius (the definitions' for its type, `unknown_radius` for
/// types the tables do not list).
#[must_use]
#[inline]
pub fn occupants(map: &Map, definitions: &Definitions, unknown_radius: i64) -> Vec<Occupant> {
    map.things
        .iter()
        .map(|row| Occupant {
            at: (i64::from(row.x), i64::from(row.y)),
            radius: thing_radius(
                definitions,
                u32::try_from(row.kind).unwrap_or(u32::MAX),
                unknown_radius,
            ),
        })
        .collect()
}

/// The player 1 start of `map`: the position of its first `type 1` thing.
#[must_use]
#[inline]
pub fn player_start(map: &Map) -> Option<Pt> {
    map.things
        .iter()
        .find(|row| row.kind == PLAYER_ONE_START)
        .map(|row| (i64::from(row.x), i64::from(row.y)))
}

/// How the facing of the things one [`distribute`] run places is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// Every thing faces this angle, already snapped by [`snap_angle`].
    Fixed(i32),
    /// Every thing faces the map's player 1 start.
    PlayerStart,
    /// Every thing faces its area's centre.
    Centre,
    /// A deterministic pseudo-random compass point per thing: a xorshift32
    /// advanced from `seed` in placement order, the only randomness here.
    Seeded(u32),
}

impl Facing {
    /// The seed of [`Self::Seeded`], `None` for every other mode.
    #[must_use]
    #[inline]
    pub const fn seed(self) -> Option<u32> {
        match self {
            Self::Seeded(seed) => Some(seed),
            Self::Fixed(_) | Self::PlayerStart | Self::Centre => None,
        }
    }
}

/// `degrees` normalised into `0..=359` and rounded to the nearest
/// multiple of 45.
#[must_use]
#[inline]
pub const fn snap_angle(degrees: i32) -> i32 {
    let turned = degrees.rem_euclid(DEGREES);
    let steps = turned
        .saturating_add(HALF_ANGLE_STEP)
        .saturating_div(ANGLE_STEP);
    steps.saturating_mul(ANGLE_STEP).rem_euclid(DEGREES)
}

/// The direction from `from` to `to` in whole degrees, `0..=359`.
fn bearing(from: Pt, to: Pt) -> i32 {
    let dx = to.0.saturating_sub(from.0);
    let dy = to.1.saturating_sub(from.1);
    i32::try_from(atan2(dy, dx).to_degrees()).unwrap_or_default()
}

/// The next value of the xorshift32 stream; a zero state is replaced by
/// one so the generator never sticks.
const fn next_u32(state: u32) -> u32 {
    let mut x = if state == 0_u32 { 1_u32 } else { state };
    x ^= x.wrapping_shl(XOR_SHIFT_A);
    x ^= x.wrapping_shr(XOR_SHIFT_B);
    x ^= x.wrapping_shl(XOR_SHIFT_C);
    x
}

/// The facing mode with the state a [`distribute`] run carries along.
struct FacingRun {
    /// The mode the angles answer with.
    mode: Facing,
    /// The player start [`Facing::PlayerStart`] faces, when the map has one.
    start: Option<Pt>,
    /// The xorshift32 state [`Facing::Seeded`] advances.
    random: u32,
}

impl FacingRun {
    /// The run state for `mode` on `map`.
    fn new(mode: Facing, map: &Map) -> Self {
        Self {
            mode,
            start: match mode {
                Facing::PlayerStart => player_start(map),
                Facing::Fixed(_) | Facing::Centre | Facing::Seeded(_) => None,
            },
            random: mode.seed().unwrap_or_default(),
        }
    }

    /// The facing of the thing standing at `at` in an area centred `centre`.
    fn angle(&mut self, at: Pt, centre: Pt) -> i32 {
        match self.mode {
            Facing::Fixed(angle) => angle,
            Facing::PlayerStart => self
                .start
                .map_or(0_i32, |start| snap_angle(bearing(at, start))),
            Facing::Centre => snap_angle(bearing(at, centre)),
            Facing::Seeded(_) => self.next_seeded(),
        }
    }

    /// The next seeded compass point, `0` to `315` in steps of 45.
    fn next_seeded(&mut self) -> i32 {
        self.random = next_u32(self.random);
        let octant = self.random.checked_rem(OCTANTS).unwrap_or_default();
        i32::try_from(octant.saturating_mul(OCTANT_DEGREES)).unwrap_or_default()
    }
}

/// One placed thing: where it stands and which way it faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The map point.
    pub at: Pt,
    /// The facing in degrees, a multiple of 45.
    pub angle: i32,
}

/// Everything one [`distribute`] run asks for beyond the map and its
/// definitions.
#[derive(Clone, Debug)]
pub struct DistributeSpec {
    /// The areas to fill, worked in this order.
    pub areas: Vec<Area>,
    /// How many things each area must hold (parallel to [`Self::areas`]).
    pub needs: Vec<usize>,
    /// The distance every placed thing keeps from walls and bodies.
    pub clearance: i64,
    /// How the facings are chosen.
    pub facing: Facing,
    /// The radius assumed for thing types the definitions do not list.
    pub unknown_radius: i64,
}

/// What a [`distribute`] run produced.
#[derive(Clone, Debug, Default)]
pub struct Distribution {
    /// Every placed thing, in area order and within an area in spread
    /// order (centre-outward, every k-th candidate).
    pub placed: Vec<Placed>,
    /// One shortfall note per area whose grid held fewer candidates than
    /// its need.
    pub warnings: Vec<String>,
}

/// The shared state of one [`distribute`] run.
struct Run<'ctx> {
    /// The map the areas are read on.
    map: &'ctx Map,
    /// The distance every candidate keeps from walls and bodies.
    clearance: i64,
    /// The wall segments candidates are measured against.
    walls: Vec<(End, End)>,
    /// The existing bodies candidates are measured against.
    blockers: Vec<Occupant>,
    /// The facing mode with its run state.
    facing: FacingRun,
    /// Where things landed so far.
    placed: Vec<Placed>,
    /// The shortfall notes.
    warnings: Vec<String>,
}

impl<'ctx> Run<'ctx> {
    /// Prepares the run: walls, blockers and facing read off `map`.
    fn new(map: &'ctx Map, definitions: &Definitions, spec: &DistributeSpec) -> Self {
        Self {
            map,
            clearance: spec.clearance,
            walls: wall_segments(map),
            blockers: occupants(map, definitions, spec.unknown_radius),
            facing: FacingRun::new(spec.facing, map),
            placed: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// The grid spacing: twice the clearance, never under [`MIN_SPACING`].
    fn spacing(&self) -> i64 {
        self.clearance.saturating_mul(2_i64).max(MIN_SPACING)
    }

    /// Whether `at` is a usable candidate of `area`.
    fn accepts(&self, area: &Area, at: Pt) -> bool {
        area.contains(self.map, at)
            && self.wall_clear(at)
            && self.blockers.iter().all(|occupant| {
                let need = i128::from(self.clearance.saturating_add(occupant.radius));
                keeps_distance(wide(occupant.at), wide(occupant.at), wide(at), need)
            })
    }

    /// Whether every wall keeps the clearance from `at`.
    fn wall_clear(&self, at: Pt) -> bool {
        let need = i128::from(self.clearance);
        self.walls
            .iter()
            .all(|&(from, to)| keeps_distance(from, to, wide(at), need))
    }

    /// The usable candidates of `area`, ordered by distance from the area
    /// centre (ties by `y` then `x`); empty when the area bounds nothing.
    fn candidates(&self, area: &Area) -> Vec<Pt> {
        let Some((min_x, min_y, max_x, max_y)) = area.bbox(self.map) else {
            return Vec::new();
        };
        let Some(centre) = area.centre(self.map) else {
            return Vec::new();
        };
        let (rows, columns) = centred_axes((min_x, max_x, min_y, max_y), self.spacing());
        let mut kept: Vec<Pt> = Vec::new();
        for &y in &rows {
            kept.extend(self.row(area, y, &columns));
        }
        kept.sort_by(|a, b| {
            distance_squared(centre, *a)
                .cmp(&distance_squared(centre, *b))
                .then(a.1.cmp(&b.1))
                .then(a.0.cmp(&b.0))
        });
        kept
    }

    /// The usable candidates on grid row `y`, one per accepted column.
    fn row(&self, area: &Area, y: i64, columns: &[i64]) -> Vec<Pt> {
        columns
            .iter()
            .filter(|&&x| self.accepts(area, (x, y)))
            .map(|&x| (x, y))
            .collect()
    }

    /// Fill one area: spread its picks over the kept candidates and note a
    /// shortfall.
    fn fill(&mut self, area: &Area, need: usize) {
        if need == 0 {
            return;
        }
        let kept = self.candidates(area);
        let centre = area.centre(self.map).unwrap_or_default();
        let taken = spread(&kept, need);
        if taken.len() < need {
            let label = area.label();
            let got = taken.len();
            self.warnings
                .push(format!("{label} fits only {got} of {need} things"));
        }
        for at in taken {
            let angle = self.facing.angle(at, centre);
            self.placed.push(Placed { at, angle });
        }
    }
}

/// Every k-th candidate, `k = max(1, kept / need)`, up to `need` of them.
fn spread(kept: &[Pt], need: usize) -> Vec<Pt> {
    let step = kept.len().checked_div(need).unwrap_or(0).max(1);
    kept.iter().step_by(step).take(need).copied().collect()
}

/// The centred grid of one area: `(rows, columns)` on the `spacing`, each
/// axis centred in its bounds, the spacing doubled until the whole grid
/// holds at most [`MAX_CANDIDATES`] points (a degenerate one when the
/// spacing saturates, which always fits).
fn centred_axes(
    (min_x, max_x, min_y, max_y): (i64, i64, i64, i64),
    spacing: i64,
) -> (Vec<i64>, Vec<i64>) {
    let mut step = spacing;
    loop {
        let columns = centred_axis(min_x, max_x, step);
        let rows = centred_axis(min_y, max_y, step);
        if columns.len().saturating_mul(rows.len()) <= MAX_CANDIDATES {
            return (rows, columns);
        }
        let doubled = step.saturating_mul(2_i64);
        if doubled == step {
            return (rows, columns);
        }
        step = doubled;
    }
}

/// The grid values along one axis, spaced `step` apart and centred in
/// `min..=max`: the whole span is walked and the leftover slack is split
/// evenly below and above it.
fn centred_axis(min: i64, max: i64, step: i64) -> Vec<i64> {
    let span = max.saturating_sub(min);
    let steps = usize::try_from(span.checked_div(step).unwrap_or_default())
        .unwrap_or(MAX_AXIS_POINTS)
        .min(MAX_AXIS_POINTS);
    let used = i64::try_from(steps)
        .unwrap_or(i64::MAX)
        .saturating_mul(step);
    let start = min.saturating_add(span.saturating_sub(used).saturating_div(AXIS_SLACK_DIVISOR));
    axis_points(start, start.saturating_add(used), step)
}

/// Spread the areas of `spec` over `map`: one [`distribute`] pass fills
/// every area with its need.
#[must_use]
#[inline]
pub fn distribute(map: &Map, definitions: &Definitions, spec: &DistributeSpec) -> Distribution {
    let mut run = Run::new(map, definitions, spec);
    for (index, area) in spec.areas.iter().enumerate() {
        let need = spec.needs.get(index).copied().unwrap_or_default();
        run.fill(area, need);
    }
    Distribution {
        placed: run.placed,
        warnings: run.warnings,
    }
}

/// The bounding-box area of `sector` in square map units, `0` when the
/// sector bounds nothing. Exact in `i128`.
fn sector_area(map: &Map, sector: usize) -> i128 {
    let Some((min_x, min_y, max_x, max_y)) = sector_bbox(map, sector) else {
        return 0_i128;
    };
    let width = i128::from(max_x)
        .checked_sub(i128::from(min_x))
        .unwrap_or(i128::MAX);
    let height = i128::from(max_y)
        .checked_sub(i128::from(min_y))
        .unwrap_or(i128::MAX);
    width.checked_mul(height).unwrap_or(i128::MAX)
}

/// One sector's running share of a [`split_proportional`] split.
struct Share {
    /// The sector the share belongs to.
    sector: usize,
    /// The whole part of the proportional share.
    base: usize,
    /// What the whole part dropped, for the remainder round.
    remainder: i128,
}

/// Split `count` across `sectors` in proportion to their bounding-box areas.
///
/// Largest-remainder method: each sector first gets the whole part of its
/// share, then the sectors with the biggest dropped remainders get one
/// more each, ties going to the lower sector index. Sectors that bound
/// nothing split the count evenly. The result is parallel to `sectors`.
#[must_use]
#[inline]
pub fn split_proportional(map: &Map, sectors: &[usize], count: usize) -> Vec<usize> {
    let wanted = i128::try_from(count).unwrap_or_default();
    let areas: Vec<i128> = sectors
        .iter()
        .map(|&sector| sector_area(map, sector))
        .collect();
    let counted = areas
        .iter()
        .fold(0_i128, |acc, &area| acc.saturating_add(area));
    let (weighted, divisor) = if counted == 0_i128 {
        let even = i128::try_from(areas.len()).unwrap_or(1_i128).max(1_i128);
        (vec![1_i128; areas.len()], even)
    } else {
        (areas, counted)
    };
    let mut shares: Vec<Share> = weighted
        .iter()
        .zip(sectors.iter())
        .map(|(&area, &sector)| {
            let product = wanted.saturating_mul(area);
            let base = product
                .checked_div(divisor)
                .unwrap_or_default()
                .try_into()
                .unwrap_or(count);
            let remainder = product.checked_rem(divisor).unwrap_or_default();
            Share {
                sector,
                base,
                remainder,
            }
        })
        .collect();
    let used = shares
        .iter()
        .fold(0_usize, |acc, share| acc.saturating_add(share.base));
    let leftover = count.saturating_sub(used);
    let mut order: Vec<Order> = shares
        .iter()
        .enumerate()
        .map(|(position, share)| Order {
            remainder: share.remainder,
            sector: share.sector,
            position,
        })
        .collect();
    order.sort_unstable_by(|a, b| b.remainder.cmp(&a.remainder).then(a.sector.cmp(&b.sector)));
    for entry in order.iter().take(leftover) {
        if let Some(share) = shares.get_mut(entry.position) {
            share.base = share.base.saturating_add(1);
        }
    }
    shares.iter().map(|share| share.base).collect()
}

/// One sector's place in the remainder round of a [`split_proportional`]
/// split: its dropped remainder, its sector index (the tie-break) and its
/// position in the result.
struct Order {
    /// The remainder that competes for the extra copy.
    remainder: i128,
    /// The sector index, for the tie-break.
    sector: usize,
    /// The index into the shares (and the result) this entry names.
    position: usize,
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{
        Area, CLEARANCE, DEFAULT_RADIUS, DistributeSpec, Distribution, Facing, Placed, axis_points,
        bearing, distribute, fits, keeps_distance, mid, next_u32, occupants, player_start,
        sector_area, snap_angle, split_proportional, thing_clearance,
    };
    use crate::defs::{Definitions, definitions_for};
    use crate::geom::Pt;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{side, solid_line, vertex};
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// A valid parsed test name.
    fn map_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A map holding `squares` as separate one-sector rooms, each
    /// `(min, max)`-cornered box bounded by four blocking lines.
    fn square_rooms(squares: &[(i32, i32)]) -> Map {
        let mut map = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
        for square in squares {
            add_square(&mut map, square.0, square.1);
        }
        map
    }

    /// Append one square sector from (`min`, `min`) to (`max`, `max`).
    fn add_square(map: &mut Map, min: i32, max: i32) {
        let sector = map.sectors.len();
        let base = map.vertices.len();
        map.sectors.push(Sector::default());
        let corners = [(min, min), (max, min), (max, max), (min, max)];
        for corner in corners {
            map.vertices.push(Vertex {
                x: corner.0,
                y: corner.1,
                extra: alloc::collections::BTreeMap::new(),
            });
        }
        let sides: Vec<usize> = (0_usize..4_usize)
            .map(|_| {
                let side = map.sidedefs.len();
                map.sidedefs.push(Sidedef {
                    sector,
                    ..Default::default()
                });
                side
            })
            .collect();
        let edges = [(0_usize, 1_usize), (1, 2), (2, 3), (3, 0)];
        for edge in edges {
            map.linedefs.push(Linedef {
                v1: base.saturating_add(edge.0),
                v2: base.saturating_add(edge.1),
                flags: 1_u32,
                front: sides.get(edge.0).copied(),
                ..Default::default()
            });
        }
    }

    /// The L-shaped single-sector room: the `0..256` box with its
    /// upper-right quadrant removed, bounded by six one-sided lines.
    fn l_room() -> Map {
        let mut map = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
        let ring: [(i32, i32); 6] = [
            (0, 256),
            (128, 256),
            (128, 128),
            (256, 128),
            (256, 0),
            (0, 0),
        ];
        for point in &ring {
            map.vertices.push(vertex(point.0, point.1));
        }
        map.sectors.push(Sector::default());
        map.sidedefs.push(side("-", "-", "WALL1", 0));
        let indices: Vec<usize> = (0_usize..ring.len()).collect();
        let back = indices.iter().skip(1).chain(indices.iter().take(1));
        for (&from, &to) in indices.iter().zip(back) {
            map.linedefs.push(solid_line(from, to, 0_usize));
        }
        map
    }

    /// The session's `DOOM` definitions for the radius tests.
    fn doom_definitions() -> Definitions {
        definitions_for(Game::Doom).expect("the embedded tables parse")
    }

    /// A one-area spec filling `area` with `need` copies at `clearance`.
    fn spec(area: Area, need: usize, clearance: i64) -> DistributeSpec {
        DistributeSpec {
            areas: vec![area],
            needs: vec![need],
            clearance,
            facing: Facing::Fixed(0_i32),
            unknown_radius: 20_i64,
        }
    }

    /// The points a [`distribute`] result landed on.
    fn points(out: &Distribution) -> Vec<Pt> {
        out.placed.iter().map(|placed| placed.at).collect()
    }

    /// The angles a [`distribute`] result carries, in placement order.
    fn angles(out: &Distribution) -> Vec<i32> {
        out.placed.iter().map(|placed| placed.angle).collect()
    }

    #[test]
    fn fits_requires_the_sector_and_the_clearance_from_every_wall() {
        let map = square_rooms(&[(0_i32, 100_i32), (400_i32, 500_i32)]);
        assert!(fits(&map, 0, (50_i64, 50_i64), CLEARANCE));
        assert!(
            fits(&map, 0, (20_i64, 50_i64), 20_i64),
            "exactly the clearance fits"
        );
        assert!(
            !fits(&map, 0, (19_i64, 50_i64), 20_i64),
            "one short does not"
        );
        assert!(
            !fits(&map, 0, (50_i64, 450_i64), CLEARANCE),
            "a point in the other room's sector is not this one's"
        );
        assert!(
            !fits(&map, 0, (5000_i64, 5000_i64), CLEARANCE),
            "a point in no sector at all"
        );
    }

    #[test]
    fn the_distance_test_measures_perpendicular_and_endpoint_cases() {
        assert!(keeps_distance(
            (0_i128, 0_i128),
            (100_i128, 0_i128),
            (50_i128, 28_i128),
            28_i128
        ));
        assert!(!keeps_distance(
            (0_i128, 0_i128),
            (100_i128, 0_i128),
            (50_i128, 27_i128),
            28_i128
        ));
        assert!(
            !keeps_distance(
                (0_i128, 0_i128),
                (100_i128, 0_i128),
                (-20_i128, 20_i128),
                29_i128
            ),
            "beyond the endpoint the distance is to the corner: sqrt(800) < 29"
        );
        assert!(
            keeps_distance(
                (0_i128, 0_i128),
                (100_i128, 0_i128),
                (-20_i128, 20_i128),
                28_i128
            ),
            "and sqrt(800) >= 28"
        );
    }

    #[test]
    fn axis_points_includes_the_far_bound_and_is_capped() {
        assert_eq!(
            axis_points(20_i64, 100_i64, 40_i64),
            vec![20_i64, 60_i64, 100_i64]
        );
        assert_eq!(axis_points(50_i64, 40_i64, 40_i64), Vec::<i64>::new());
        assert_eq!(
            axis_points(0_i64, i64::MAX, 1_i64).len(),
            4_096_usize,
            "a huge axis stops at the cap"
        );
    }

    #[test]
    fn clearance_uses_the_defined_radius_or_the_given_fallback() {
        let definitions = doom_definitions();
        assert_eq!(
            thing_clearance(&definitions, 3001_u32, 20_i64),
            20_i64.checked_add(CLEARANCE).expect("imp radius"),
            "the Imp's 20-unit radius"
        );
        assert_eq!(
            thing_clearance(&definitions, 999_999_u32, DEFAULT_RADIUS),
            DEFAULT_RADIUS
                .checked_add(CLEARANCE)
                .expect("default radius"),
            "an unlisted type gets the caller's fallback"
        );
    }

    #[test]
    fn the_centred_grid_lays_evenly_in_a_rectangle_and_orders_centre_outward() {
        let map = square_rooms(&[(0_i32, 180_i32)]);
        let definitions = doom_definitions();
        let out = distribute(&map, &definitions, &spec(Area::Sector(0), 9_usize, 20_i64));
        assert_eq!(
            points(&out),
            vec![
                (90_i64, 90_i64),
                (90_i64, 50_i64),
                (50_i64, 90_i64),
                (130_i64, 90_i64),
                (90_i64, 130_i64),
                (50_i64, 50_i64),
                (130_i64, 50_i64),
                (50_i64, 130_i64),
                (130_i64, 130_i64)
            ],
            "step 40 centred in the 0..180 box: the slack 20 splits 10/10 \
             and the 10/170 rails fall inside the wall clearance"
        );
        assert!(out.warnings.is_empty(), "the room held everything asked");
    }

    #[test]
    fn the_spread_walks_every_kth_candidate() {
        let map = square_rooms(&[(0_i32, 180_i32)]);
        let definitions = doom_definitions();
        let out = distribute(&map, &definitions, &spec(Area::Sector(0), 3_usize, 20_i64));
        assert_eq!(
            points(&out),
            vec![(90_i64, 90_i64), (130_i64, 90_i64), (130_i64, 50_i64)],
            "nine kept, three needed: every third of the centre-outward order"
        );
    }

    #[test]
    fn the_grid_drops_points_outside_an_l_shaped_area() {
        let map = l_room();
        let definitions = doom_definitions();
        let out = distribute(&map, &definitions, &spec(Area::Sector(0), 16_usize, 28_i64));
        assert_eq!(
            points(&out),
            vec![
                (128_i64, 72_i64),
                (72_i64, 128_i64),
                (72_i64, 72_i64),
                (184_i64, 72_i64),
                (72_i64, 184_i64)
            ],
            "the five points the L leaves at least 28 clear of every wall, \
             ordered by distance from the centre (128, 128)"
        );
        assert_eq!(
            out.warnings,
            vec!["sector 0 fits only 5 of 16 things".to_owned()],
            "the shortfall is a note, not an error"
        );
    }

    #[test]
    fn existing_bodies_keep_their_clearance_off_the_grid() {
        let mut map = square_rooms(&[(0_i32, 200_i32)]);
        let definitions = doom_definitions();
        map.things.push(Thing {
            x: 120_i32,
            y: 80_i32,
            kind: 3001_i32,
            ..Default::default()
        });
        let out = distribute(&map, &definitions, &spec(Area::Sector(0), 16_usize, 20_i64));
        assert!(
            !points(&out).contains(&(120_i64, 80_i64)),
            "the imp's own spot is gone: 0 < 20 + 20"
        );
        assert!(
            points(&out).contains(&(80_i64, 80_i64)),
            "the 40-unit neighbour is exactly 20 + 20 away and stays"
        );
        assert_eq!(points(&out).len(), 15, "sixteen grid points, one taken");
        assert_eq!(
            out.warnings,
            vec!["sector 0 fits only 15 of 16 things".to_owned()]
        );
    }

    #[test]
    fn an_open_box_must_also_sit_in_some_sector() {
        let map = square_rooms(&[(0_i32, 200_i32)]);
        let definitions = doom_definitions();
        let inside = distribute(
            &map,
            &definitions,
            &spec(
                Area::Rect {
                    min_x: 40_i64,
                    min_y: 40_i64,
                    max_x: 160_i64,
                    max_y: 160_i64,
                },
                2_usize,
                20_i64,
            ),
        );
        assert_eq!(
            points(&inside),
            vec![(80_i64, 80_i64), (40_i64, 120_i64)],
            "the box centred in the room keeps its points, spread every eighth"
        );
        let outside = distribute(
            &map,
            &definitions,
            &spec(
                Area::Rect {
                    min_x: 1000_i64,
                    min_y: 1000_i64,
                    max_x: 1200_i64,
                    max_y: 1200_i64,
                },
                2_usize,
                20_i64,
            ),
        );
        assert!(
            outside.placed.is_empty(),
            "a box in the void holds no sector point"
        );
        assert_eq!(
            outside.warnings,
            vec!["the bounding box fits only 0 of 2 things".to_owned()]
        );
    }

    #[test]
    fn a_circle_area_keeps_its_radius_and_orders_its_picks() {
        let map = square_rooms(&[(0_i32, 200_i32)]);
        let definitions = doom_definitions();
        let out = distribute(
            &map,
            &definitions,
            &spec(
                Area::Circle {
                    centre: (100_i64, 100_i64),
                    radius: 50_i64,
                },
                2_usize,
                20_i64,
            ),
        );
        assert_eq!(
            points(&out),
            vec![(100_i64, 100_i64), (60_i64, 100_i64)],
            "five points sit within the radius; every second, from the centre"
        );
        assert!(out.warnings.is_empty(), "five kept for two needed");
    }

    #[test]
    fn proportional_needs_follow_the_bounding_box_areas() {
        let map = square_rooms(&[(0_i32, 100_i32), (200_i32, 400_i32)]);
        assert_eq!(sector_area(&map, 0), 10_000_i128);
        assert_eq!(
            split_proportional(&map, &[0_usize, 1], 5_usize),
            vec![1_usize, 4_usize],
            "the 1:4 area ratio splits 5 exactly"
        );
        assert_eq!(
            split_proportional(&map, &[0_usize, 1], 4_usize),
            vec![1_usize, 3_usize],
            "0.8 and 3.2: the bigger remainder rounds the small sector up"
        );
        assert_eq!(
            split_proportional(&map, &[1, 0], 4_usize),
            vec![3_usize, 1_usize],
            "and the result stays parallel to the requested order"
        );
    }

    #[test]
    fn remainder_ties_go_to_the_lower_sector_index() {
        let map = square_rooms(&[(0_i32, 100_i32), (200_i32, 300_i32)]);
        assert_eq!(
            split_proportional(&map, &[0_usize, 1], 3_usize),
            vec![2_usize, 1_usize],
            "equal areas: the extra copy goes to the lower index"
        );
        assert_eq!(
            split_proportional(&map, &[1], 2_usize),
            vec![2_usize],
            "one area takes the whole count"
        );
        let empty = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
        assert_eq!(
            split_proportional(&empty, &[0_usize, 1], 3_usize),
            vec![2_usize, 1_usize],
            "sectors bounding nothing split evenly, lower index first"
        );
    }

    #[test]
    fn angles_snap_to_the_nearest_compass_point() {
        assert_eq!(snap_angle(0_i32), 0_i32);
        assert_eq!(snap_angle(22_i32), 0_i32);
        assert_eq!(snap_angle(23_i32), 45_i32);
        assert_eq!(snap_angle(70_i32), 90_i32);
        assert_eq!(snap_angle(350_i32), 0_i32, "350 rounds up to 360 = 0");
        assert_eq!(snap_angle(-23_i32), 315_i32, "-23 turns to 337");
        assert_eq!(snap_angle(400_i32), 45_i32, "400 turns to 40");
    }

    #[test]
    fn bearings_point_where_the_facing_modes_follow() {
        assert_eq!(bearing((0_i64, 0_i64), (100_i64, 0_i64)), 0_i32);
        assert_eq!(bearing((0_i64, 0_i64), (0_i64, 100_i64)), 90_i32);
        assert_eq!(
            snap_angle(bearing((0_i64, 0_i64), (100_i64, 90_i64))),
            45_i32
        );
    }

    #[test]
    fn the_seeded_facing_is_a_deterministic_compass_walk() {
        let map = square_rooms(&[(0_i32, 400_i32)]);
        let definitions = doom_definitions();
        let mut seeded = spec(Area::Sector(0), 6_usize, 20_i64);
        seeded.facing = Facing::Seeded(1_u32);
        let first = distribute(&map, &definitions, &seeded);
        let second = distribute(&map, &definitions, &seeded);
        assert_eq!(
            first.placed, second.placed,
            "the same seed places the same angles on the same points"
        );
        let mut state = 1_u32;
        let mut expected: Vec<i32> = Vec::new();
        for _ in 0..6_usize {
            state = next_u32(state);
            let octant = state.checked_rem(8_u32).expect("eight is non-zero");
            expected.push(i32::try_from(octant.checked_mul(45_u32).expect("small")).expect("fit"));
        }
        assert_eq!(angles(&first), expected, "the documented xorshift32 walk");
        seeded.facing = Facing::Seeded(2_u32);
        let third = distribute(&map, &definitions, &seeded);
        assert_ne!(
            angles(&first),
            angles(&third),
            "another seed walks elsewhere"
        );
        seeded.facing = Facing::Seeded(0_u32);
        let zero = distribute(&map, &definitions, &seeded);
        assert_eq!(
            angles(&zero),
            angles(&first),
            "the zero state is replaced by one: seed 0 walks like seed 1"
        );
    }

    #[test]
    fn the_face_modes_follow_the_start_and_the_centre() {
        let mut map = square_rooms(&[(0_i32, 400_i32)]);
        let definitions = doom_definitions();
        map.things.push(Thing {
            x: 20_i32,
            y: 200_i32,
            kind: 1_i32,
            ..Default::default()
        });
        let start = player_start(&map).expect("the map carries a start");
        assert_eq!(start, (20_i64, 200_i64));
        let mut to_start = spec(Area::Sector(0), 1_usize, 20_i64);
        to_start.facing = Facing::PlayerStart;
        let out = distribute(&map, &definitions, &to_start);
        assert_eq!(
            out.placed,
            vec![Placed {
                at: (200_i64, 200_i64),
                angle: 180_i32
            }],
            "the centre pick sits due east of the start and faces back west"
        );
        let mut to_centre = spec(Area::Sector(0), 1_usize, 20_i64);
        to_centre.facing = Facing::Centre;
        let centred = distribute(&map, &definitions, &to_centre);
        assert_eq!(
            centred.placed,
            vec![Placed {
                at: (200_i64, 200_i64),
                angle: 0_i32
            }],
            "the centre faces itself: a zero bearing from its own point"
        );
    }

    #[test]
    fn blockers_widen_with_the_occupant_radius() {
        let map = square_rooms(&[(0_i32, 200_i32)]);
        let definitions = doom_definitions();
        let blockers = occupants(&map, &definitions, 20_i64);
        assert!(blockers.is_empty(), "the bare rooms hold no things");
        let mut crowded = map;
        crowded.things.push(Thing {
            x: 100_i32,
            y: 100_i32,
            kind: 999_999_i32,
            ..Default::default()
        });
        crowded.things.push(Thing {
            x: 20_i32,
            y: 20_i32,
            kind: 3001_i32,
            ..Default::default()
        });
        let armed = occupants(&crowded, &definitions, 20_i64);
        assert_eq!(
            armed
                .iter()
                .map(|blocker| blocker.radius)
                .collect::<Vec<i64>>(),
            vec![20_i64, 20_i64],
            "the unknown type takes the fallback, the Imp its defined radius"
        );
    }

    #[test]
    fn the_midpoint_and_area_shape_helpers_stay_exact() {
        assert_eq!(mid(0_i64, 100_i64), 50_i64);
        assert_eq!(mid(-300_i64, 300_i64), 0_i64);
        let circle = Area::Circle {
            centre: (0_i64, 0_i64),
            radius: 64_i64,
        };
        let bounds = square_rooms(&[(0_i32, 100_i32)]);
        assert_eq!(
            circle.bbox(&bounds),
            Some((-64_i64, -64_i64, 64_i64, 64_i64))
        );
        assert_eq!(circle.centre(&bounds), Some((0_i64, 0_i64)));
        assert_eq!(circle.label(), "the circle");
        assert_eq!(Area::Sector(7).label(), "sector 7");
        assert_eq!(
            Area::Rect {
                min_x: 0_i64,
                min_y: 0_i64,
                max_x: 1_i64,
                max_y: 1_i64,
            }
            .label(),
            "the bounding box"
        );
        assert_eq!(next_u32(0_u32), next_u32(1_u32), "zero starts at one");
    }

    #[test]
    fn an_area_with_no_need_places_nothing() {
        let map = square_rooms(&[(0_i32, 200_i32)]);
        let definitions = doom_definitions();
        let out = distribute(&map, &definitions, &spec(Area::Sector(0), 0_usize, 20_i64));
        assert!(out.placed.is_empty(), "need 0 asks for nothing");
        assert!(out.warnings.is_empty(), "and gets no note");
    }
}
