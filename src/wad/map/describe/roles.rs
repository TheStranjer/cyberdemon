//! What a sector *is* and what lies next to it: the boundary and
//! neighbour reads of [`super::SectorFacts`], the `via` and role names, and
//! the per-family sector-special tables behind [`super::SectorFacts::roles`].
//!
//! The helpers here answer two questions the roles and neighbours fields
//! ask. Where does a sector's boundary lie, and which sectors sit across
//! it ([`boundary_edges`], [`neighbour_groups`], [`side_sector`])? And what
//! does a line, a floor difference or a stored special state about the way
//! through it ([`carries_flag`], [`height_via`], [`is_stair_step`],
//! [`walk_over_exit`], [`special_roles`])? The one read that needs the
//! map's door, lift and teleport actions, [`super::Describe`], asks them
//! through this module.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::defs::boom::{SectorFamily, SectorFlag, decode_sector_special};
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily, PLAYER_USE_BITS};
use crate::wad::map::geometry::sector_boundary;
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::playability::MAX_STEP_UP;

use super::shape::midpoint;
use super::{
    Edge, NAME_DAMAGING, NAME_LIGHT_EFFECT, NAME_OPENING, NAME_SECRET, NAME_STEP_DOWN,
    NAME_STEP_UP, Neighbour, Point,
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
/// The `Doom` family's walk-over level ends, as the playability exit
/// table reads them.
const DOOM_WALK_EXITS: [i32; 4] = [52_i32, 124_i32, 197_i32, 198_i32];
/// The decoder's damage text for sectors that hurt nobody.
const NO_DAMAGE: &str = "none";

/// `sector`'s boundary as directed world-space edges: front sides walk
/// `v1` to `v2`, back sides `v2` to `v1`, linedef order; lines whose
/// vertices do not resolve are skipped.
pub(super) fn boundary_edges(map: &Map, sector: usize) -> Vec<Edge> {
    sector_boundary(map, sector)
        .into_iter()
        .filter_map(|(index, is_front)| {
            let line = map.linedefs.get(index)?;
            let start = map.vertices.get(line.v1)?;
            let end = map.vertices.get(line.v2)?;
            let from = (i64::from(start.x), i64::from(start.y));
            let to = (i64::from(end.x), i64::from(end.y));
            Some(if is_front { (from, to) } else { (to, from) })
        })
        .collect()
}

/// The sector the sidedef `side` opens into: `None` when the side is
/// absent, its sidedef index dangles, or that sidedef's sector index
/// dangles.
pub(super) fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sidedef = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
}

/// The neighbour sectors across `sector`'s two-sided boundary, each with
/// the linedef indices it shares, in sector order and linedef order. A
/// line whose own side resolves to `sector` and whose far side resolves
/// to another sector joins that neighbour's group; lines to the void, to
/// `sector` itself or with a dangling far side group nothing.
pub(super) fn neighbour_groups(map: &Map, sector: usize) -> Vec<(usize, Vec<usize>)> {
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, is_front) in sector_boundary(map, sector) {
        let across = map
            .linedefs
            .get(index)
            .and_then(|line| side_sector(map, if is_front { line.back } else { line.front }))
            .filter(|other| *other != sector);
        if let Some(other) = across {
            groups.entry(other).or_default().push(index);
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

/// Whether an `area` sector wedged between `neighbours` reads as a stair
/// step: a footprint of at most [`STAIR_MAX_AREA`] square units with one
/// neighbour [`STEP_UP`] to [`MAX_STEP_UP`] units above and another
/// [`MAX_STEP_DOWN`] to [`STEP_DOWN_TOP`] units below.
pub(super) fn is_stair_step(area: u64, neighbours: &[Neighbour]) -> bool {
    area <= STAIR_MAX_AREA
        && neighbours
            .iter()
            .any(|row| (STEP_UP..=MAX_STEP_UP).contains(&row.floor_delta))
        && neighbours
            .iter()
            .any(|row| (MAX_STEP_DOWN..=STEP_DOWN_TOP).contains(&row.floor_delta))
}

/// Whether the exit special on `line` ends the level by walking over it:
/// a `Doom`-family touch special, or a `Hexen`-family portal on any
/// activation but `playeruse`.
pub(super) fn walk_over_exit(family: FlagFamily, line: &Linedef) -> bool {
    match family {
        FlagFamily::Doom => DOOM_WALK_EXITS.contains(&line.special),
        FlagFamily::Hexen => line.flags & ACTIVATION_MASK != PLAYER_USE_BITS,
    }
}

/// The world-space middle of `line`'s span, `None` when either vertex
/// fails to resolve.
pub(super) fn line_midpoint(map: &Map, line: &Linedef) -> Option<Point> {
    let start = map.vertices.get(line.v1)?;
    let end = map.vertices.get(line.v2)?;
    Some((
        midpoint(i64::from(start.x), i64::from(end.x)),
        midpoint(i64::from(start.y), i64::from(end.y)),
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
