//! The distribution engine: spreads whole [`Area`]s over a map and splits
//! a count across sectors.
//!
//! [`distribute`] fills every [`Area`] of a [`DistributeSpec`]: for each
//! one it lays a square grid spaced twice the clearance (at least 16
//! units) centred in the area's bounding box, keeps the points inside the
//! area and clear of every wall and existing body, orders what is kept by
//! distance from the area centre (ties by `y` then `x`) and spreads the
//! picks by taking every k-th candidate. [`split_proportional`] answers
//! how many of a total belong to each sector: by bounding-box area,
//! largest-remainder, ties to the lower sector index. The facings the
//! picks carry are chosen by the sibling `facing` module; the exact
//! measuring is done by the primitives in the parent module.

use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::geom::Pt;
use crate::wad::map::geometry::sector_bbox;
use crate::wad::map::model::Map;

use super::facing::{Facing, FacingRun};
use super::{
    Area, End, MAX_AXIS_POINTS, Occupant, axis_points, distance_squared, keeps_distance, occupants,
    wall_segments, wide,
};

/// The smallest grid spacing [`distribute`] ever lays, map units.
const MIN_SPACING: i64 = 16_i64;
/// The largest candidate grid one area contributes; the spacing doubles
/// until the product of the two axes fits.
const MAX_CANDIDATES: usize = 0x1_0000_usize;
/// Divisor turning an axis slack into the centring offset.
const AXIS_SLACK_DIVISOR: i64 = 2_i64;

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
pub(super) fn sector_area(map: &Map, sector: usize) -> i128 {
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
