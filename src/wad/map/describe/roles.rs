//! What a sector *is* and what lies next to it: the boundary and
//! neighbour reads of [`super::SectorFacts`], the `via` and role names, and
//! the per-family sector-special tables behind [`super::SectorFacts::roles`].
//!
//! The helpers here answer two questions the roles and neighbours fields
//! ask. Where does a sector's boundary lie, and which sectors sit across
//! it ([`boundary_edges`], [`neighbour_groups`], with [`Map::side_sector`]
//! for the single-side read)? Both boundary reads take the [`SectorIndex`]
//! the whole map is indexed into once and walk its stored pairs, resolving
//! each line's far side and vertices against the map. And what
//! does a line, a floor difference or a stored special state about the way
//! through it ([`carries_flag`], [`height_via`], [`is_stair_step`],
//! [`walk_over_exit`], [`special_roles`])? The one read that needs the
//! map's door, lift and teleport actions, [`super::Describe`], asks them
//! through this module.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::defs::boom::{SectorFamily, SectorFlag, decode_sector_special};
use crate::fixed::midpoint;
use crate::geom::Pt;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::playability::MAX_STEP_UP;
use crate::wad::map::specials::exit_for;
use crate::wad::map::specials::tables::Trigger;

use super::{
    Edge, NAME_DAMAGING, NAME_LIGHT_EFFECT, NAME_OPENING, NAME_SECRET, NAME_STEP_DOWN, NAME_STEP_UP,
};

/// The largest footprint of a stair-step sector, square map units:
/// 64 x 256.
const STAIR_MAX_AREA: u64 = 0x4000_u64;
/// The smallest rising neighbour floor difference a step counts.
const STEP_UP: i32 = 1_i32;
/// The largest falling neighbour floor difference a step counts.
const STEP_DOWN_TOP: i32 = -1_i32;
/// The smallest rising difference negated: the deepest falling step a
/// stair step counts, mirroring [`MAX_STEP_UP`].
const MAX_STEP_DOWN: i32 = MAX_STEP_UP.saturating_neg();
/// The `Doom` family's vanilla secret sector type.
const DOOM_SECRET_BASE: u32 = 9_u32;
/// The `Doom` family's damaging vanilla sector types.
const DOOM_DAMAGE_BASES: [u32; 5] = [4_u32, 5_u32, 7_u32, 11_u32, 16_u32];
/// The `Doom` family's light-effect vanilla sector types.
const DOOM_LIGHT_BASES: [u32; 7] = [1_u32, 2_u32, 3_u32, 8_u32, 12_u32, 13_u32, 17_u32];
/// The `Hexen` family's secret sector flag bit.
const HEXEN_SECRET_BIT: i32 = 1024_i32;
/// The `Hexen` family's `ZDoom` damage sector specials.
const HEXEN_DAMAGE_SPECIALS: [i32; 7] = [69_i32, 70_i32, 71_i32, 82_i32, 83_i32, 84_i32, 85_i32];
/// The `Hexen` family's light-effect sector specials: the `Doom` numbers
/// shifted by 64.
const HEXEN_LIGHT_SPECIALS: [i32; 7] = [65_i32, 66_i32, 67_i32, 72_i32, 76_i32, 77_i32, 81_i32];
/// The decoder's damage text for sectors that hurt nobody.
const NO_DAMAGE: &str = "none";

/// `sector`'s boundary as directed world-space edges: front sides walk
/// `v1` to `v2`, back sides `v2` to `v1`, linedef order; lines whose
/// vertices do not resolve are skipped. The pairs come from `index`'s
/// boundary read, the endpoints resolved afresh against `map`.
pub(super) fn boundary_edges(map: &Map, index: &SectorIndex, sector: usize) -> Vec<Edge> {
    index
        .boundary(sector)
        .iter()
        .filter_map(|&(line, is_front)| {
            let ((sx, sy), (ex, ey)) = map
                .linedefs
                .get(line)
                .and_then(|definition| map.line_endpoints(definition))?;
            let from = (i64::from(sx), i64::from(sy));
            let to = (i64::from(ex), i64::from(ey));
            Some(if is_front { (from, to) } else { (to, from) })
        })
        .collect()
}

/// The neighbour sectors across `sector`'s two-sided boundary, each with
/// the linedef indices it shares, in sector order and linedef order. A
/// line whose own side resolves to `sector` and whose far side resolves
/// to another sector joins that neighbour's group; lines to the void, to
/// `sector` itself or with a dangling far side group nothing. The pairs
/// come from `index`'s boundary read, the far side resolved afresh
/// against `map`.
pub(super) fn neighbour_groups(
    map: &Map,
    index: &SectorIndex,
    sector: usize,
) -> Vec<(usize, Vec<usize>)> {
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(line, is_front) in index.boundary(sector) {
        let definition = map.linedefs.get(line);
        let across = definition
            .and_then(|def| map.side_sector(if is_front { def.back } else { def.front }))
            .filter(|other| *other != sector);
        if let Some(other) = across {
            groups.entry(other).or_default().push(line);
        }
    }
    groups.into_iter().collect()
}

/// Whether linedef `line` carries the flag bit `flag`; a dangling index
/// carries none.
pub(super) fn carries_flag(map: &Map, line: usize, flag: u32) -> bool {
    map.linedefs
        .get(line)
        .is_some_and(|def| def.flags & flag == flag)
}

/// The way through an unblocked boundary whose neighbour stands `delta`
/// map units above: level is an `opening`, higher a `step_up`, lower a
/// `step_down`.
pub(super) const fn height_via(delta: i32) -> &'static str {
    if delta > 0_i32 {
        NAME_STEP_UP
    } else if delta < 0_i32 {
        NAME_STEP_DOWN
    } else {
        NAME_OPENING
    }
}

/// Whether an `area` sector wedged between neighbours standing at the
/// `floor_deltas` reads as a stair step: a footprint of at most
/// [`STAIR_MAX_AREA`] square units with one delta [`STEP_UP`] to
/// [`MAX_STEP_UP`] units above and another [`MAX_STEP_DOWN`] to
/// [`STEP_DOWN_TOP`] units below. The deltas are all a neighbour row
/// contributes to this read, so the light [`super::SectorShape`] path can
/// answer them off the neighbour groups without dressing the rows.
pub(super) fn is_stair_step(area: u64, floor_deltas: &[i32]) -> bool {
    area <= STAIR_MAX_AREA
        && floor_deltas
            .iter()
            .any(|delta| (STEP_UP..=MAX_STEP_UP).contains(delta))
        && floor_deltas
            .iter()
            .any(|delta| (MAX_STEP_DOWN..=STEP_DOWN_TOP).contains(delta))
}

/// Whether the exit special on `line` ends the level by walking over it:
/// the shared exit rule of [`crate::wad::map::specials`] answered with a
/// [`Trigger::Walk`] side - the `Doom` family's touch specials and the
/// `Hexen` family's portals on any activation but `playeruse`.
pub(super) fn walk_over_exit(family: FlagFamily, line: &Linedef) -> bool {
    exit_for(family, line).is_some_and(|exit| exit.trigger == Trigger::Walk)
}

/// The world-space middle of `line`'s span, `None` when either vertex
/// fails to resolve.
pub(super) fn line_midpoint(map: &Map, line: &Linedef) -> Option<Pt> {
    let ((sx, sy), (ex, ey)) = map.line_endpoints(line)?;
    Some((
        midpoint(i64::from(sx), i64::from(ex)),
        midpoint(i64::from(sy), i64::from(ey)),
    ))
}

/// The roles a sector special of the flag family `family` states.
pub(super) fn special_roles(special: i32, family: FlagFamily) -> Vec<&'static str> {
    match family {
        FlagFamily::Doom => doom_special_roles(special),
        FlagFamily::Hexen => hexen_special_roles(special),
    }
}

/// The `Doom` family's read of one sector special, every bit field taken
/// through [`decode_sector_special`] and every bit outside them left
/// alone: the vanilla `9` or the `Boom` bit 128 hides, the vanilla
/// damaging numbers or any `Boom` damage level hurts, the vanilla light
/// numbers flicker; a value the decoder takes none of reads as nothing.
fn doom_special_roles(special: i32) -> Vec<&'static str> {
    let Some(decoded) = u32::try_from(special)
        .ok()
        .and_then(|value| decode_sector_special(value, SectorFamily::Doom))
    else {
        return Vec::new();
    };
    let mut roles = Vec::new();
    if decoded.base == DOOM_SECRET_BASE || decoded.flags.contains(&SectorFlag::Secret) {
        roles.push(NAME_SECRET);
    }
    if decoded.damage != NO_DAMAGE || DOOM_DAMAGE_BASES.contains(&decoded.base) {
        roles.push(NAME_DAMAGING);
    }
    if DOOM_LIGHT_BASES.contains(&decoded.base) {
        roles.push(NAME_LIGHT_EFFECT);
    }
    roles
}

/// The `Hexen` family's read of one sector special: bit 1024 hides, the
/// `ZDoom` damage specials hurt and the `Doom` light numbers shifted by
/// 64 flicker.
fn hexen_special_roles(special: i32) -> Vec<&'static str> {
    let mut roles = Vec::new();
    if special & HEXEN_SECRET_BIT == HEXEN_SECRET_BIT {
        roles.push(NAME_SECRET);
    }
    if HEXEN_DAMAGE_SPECIALS.contains(&special) {
        roles.push(NAME_DAMAGING);
    }
    if HEXEN_LIGHT_SPECIALS.contains(&special) {
        roles.push(NAME_LIGHT_EFFECT);
    }
    roles
}
