//! The build side of the lift core: drawing a new platform, resolving
//! the floor it lowers to, finding the edges that face the lower area
//! and dressing them with the action [`lift_specials`] names.
//!
//! [`build_lift`] folds these steps into one map edit over the
//! [`LiftSpec`] and the platform, side and probe shapes the parent
//! module defines; the refusal and note messages those steps report
//! live beside them in [`super`].

use crate::error::CyberError;
use crate::geom::Pt;
use crate::geom::segment::round_quotient;
use crate::wad::map::flags::{BLOCKING, FlagFamily};
use crate::wad::map::geometry::{neighbors, point_in_sector};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::overlay::{Outside, OverlayRequest, Paint, WallTextures, apply_overlay};

use super::super::doors::{DoorLineSetup, next_free_tag};
use super::{
    Edge, EdgeProbe, LiftBuild, LiftPlatform, LiftSpec, MIN_LIFT_DROP, MIN_PLATFORM_CORNERS,
    NO_TEXTURE, Side, TRIGGER_MASK, centre_lost, invalid, isolated, lift_specials, no_edges,
    no_way_down, paint_lost, range_error, shrink_note,
};

/// The mean of one axis of the polygon's corners, rounded to the
/// nearest whole unit and saturated into the grid.
fn mean_axis(sum: i128, count: i128) -> i64 {
    let exact = round_quotient(sum, count).unwrap_or(sum);
    match i64::try_from(exact) {
        Ok(axis) => axis,
        Err(_error) if exact < 0_i128 => i64::MIN,
        Err(_error) => i64::MAX,
    }
}

/// The corner-average centre of a platform polygon.
pub(super) fn polygon_centre(polygon: &[Pt]) -> Pt {
    let mut sum_x = 0_i128;
    let mut sum_y = 0_i128;
    for &(x, y) in polygon {
        sum_x = sum_x.saturating_add(i128::from(x));
        sum_y = sum_y.saturating_add(i128::from(y));
    }
    let count = i128::try_from(polygon.len()).unwrap_or(1);
    (mean_axis(sum_x, count), mean_axis(sum_y, count))
}

/// The sector record for a drawn platform: the base's heights, flats and
/// light, inert and untagged (the tag is stamped after the paint).
fn platform_record(map: &Map, base: usize) -> Sector {
    let mut record = map.sectors.get(base).cloned().unwrap_or_default();
    record.special = 0_i32;
    record.tag = 0_i32;
    record
}

/// The overlay request painting the platform, its new walls in `side`.
fn platform_request(map: &Map, polygon: &[Pt], base: usize, side: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: polygon.to_vec(),
        paint: Paint::Sector(platform_record(map, base)),
        keep_interior: false,
        textures: WallTextures {
            wall: side.to_owned(),
            upper: side.to_owned(),
            lower: side.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// Draw a new platform: the `polygon` folded over the map as a sector
/// copying the heights of the sector under its centre.
///
/// Returns the new sector's index and the paint's notes.
fn draw_platform(
    map: &mut Map,
    polygon: &[Pt],
    side: &str,
) -> Result<(usize, Vec<String>), CyberError> {
    if polygon.len() < MIN_PLATFORM_CORNERS {
        return Err(invalid(
            "platform.points needs at least three corners: pass the ordered outline of \
             the new platform (its last corner closes it)",
        ));
    }
    let centre = polygon_centre(polygon);
    let base = point_in_sector(map, centre.0, centre.1).ok_or_else(|| centre_lost(centre))?;
    let drawn = apply_overlay(map, &platform_request(map, polygon, base, side))?;
    let sector = drawn.sector.ok_or_else(paint_lost)?;
    Ok((sector, drawn.notes))
}

/// The floor the platform lowers to: the explicit `given` one, else the
/// lowest neighbouring sector's floor.
fn resolve_low_floor(map: &Map, sector: usize, given: Option<i32>) -> Result<i32, CyberError> {
    given.map_or_else(|| lowest_neighbour(map, sector), Ok)
}

/// The lowest floor among `sector`'s neighbouring sectors.
fn lowest_neighbour(map: &Map, sector: usize) -> Result<i32, CyberError> {
    neighbors(map, sector)
        .into_iter()
        .filter_map(|index| map.sectors.get(index).map(|row| row.floor_height))
        .min()
        .ok_or_else(|| isolated(sector))
}

/// The resolved side of a line, `None` when the sidedef or its sector
/// does not resolve.
fn resolve_side(map: &Map, side: Option<usize>) -> Option<Side> {
    let sidedef_index = side?;
    let sidedef = map.sidedefs.get(sidedef_index)?;
    let sector = sidedef.sector;
    let row = map.sectors.get(sector)?;
    Some(Side {
        sidedef: sidedef_index,
        sector,
        floor: row.floor_height,
    })
}

/// Whether `line` is a platform edge facing a lower sector, and which of
/// its sides must end up front.
fn edge_against_low(map: &Map, line: &Linedef, index: usize, probe: EdgeProbe) -> Option<Edge> {
    let front = resolve_side(map, line.front)?;
    let back = resolve_side(map, line.back)?;
    match (
        front.sector == probe.platform,
        back.sector == probe.platform,
    ) {
        (true, false) if back.floor < probe.floor => Some(Edge {
            line: index,
            low_side: back.sidedef,
            flip: true,
        }),
        (false, true) if front.floor < probe.floor => Some(Edge {
            line: index,
            low_side: front.sidedef,
            flip: false,
        }),
        _ => None,
    }
}

/// The platform's activation edges, in linedef order.
pub(super) fn activation_edges(map: &Map, platform: usize, floor: i32) -> Vec<Edge> {
    let probe = EdgeProbe { platform, floor };
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| edge_against_low(map, line, index, probe))
        .collect()
}

/// Write the platform's own row: the fresh tag and the lift floor flat
/// (kept when the caller named no texture).
fn stamp_platform(map: &mut Map, sector: usize, tag: i32, floor_flat: &str) {
    if let Some(row) = map.sectors.get_mut(sector) {
        row.tag = tag;
        if floor_flat != NO_TEXTURE {
            floor_flat.clone_into(&mut row.floor_flat);
        }
    }
}

/// Dress one activation edge: turn it so its front faces the lower
/// area, write the family's lift action on it (the `Hexen` family
/// replaces the whole trigger — activation field and `repeatspecial` —
/// the `Doom` family keeps its flags except the blocking bit), and hang
/// the side texture below the platform on the low-facing side.
fn dress_edge(map: &mut Map, edge: &Edge, setup: &DoorLineSetup, side: &str) {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    if let Some(line) = map.linedefs.get_mut(edge.line) {
        if edge.flip {
            core::mem::swap(&mut line.v1, &mut line.v2);
            core::mem::swap(&mut line.front, &mut line.back);
        }
        line.special = setup.special;
        line.tag = setup.tag;
        line.args = setup.args;
        line.flags &= !BLOCKING;
        if hexen {
            line.flags = (line.flags & !TRIGGER_MASK) | setup.flags_activation;
        }
    }
    if let Some(worn) = map.sidedefs.get_mut(edge.low_side) {
        side.clone_into(&mut worn.lower);
    }
}

/// Dress every activation edge and report the line indices, ascending.
fn dress_edges(map: &mut Map, edges: &[Edge], setup: &DoorLineSetup, side: &str) -> Vec<usize> {
    let mut lines: Vec<usize> = edges.iter().map(|edge| edge.line).collect();
    lines.sort_unstable();
    for edge in edges {
        dress_edge(map, edge, setup, side);
    }
    lines
}

/// Build a working lift platform as one map edit.
///
/// The platform is either the named existing sector or the polygon
/// painted by [`LiftPlatform::Draw`]; it lowers from its floor to
/// [`LiftSpec::low_floor`] (default: the lowest neighbouring floor), so
/// it must stand above that floor and at least one of its two-sided
/// boundary edges must face a sector below it — those edges carry the
/// [`lift_specials`] action, front-turned at the lower area. The sector
/// is stamped with a fresh [`next_free_tag`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an out-of-range platform sector,
/// a polygon with fewer than three corners or a
/// centre in no sector, a platform with no neighbour and no explicit
/// `low_floor`, a low floor not below the platform's, or a platform no
/// edge of which faces a lower sector; [`CyberError::Internal`] when a
/// platform paint reports no sector. A refusal on an existing platform
/// writes nothing.
///
/// # Panics
///
/// Never: every map access is checked.
#[inline]
pub fn build_lift(map: &mut Map, spec: &LiftSpec) -> Result<LiftBuild, CyberError> {
    let (platform_sector, mut notes) = match spec.platform.clone() {
        LiftPlatform::Existing(index) => {
            if map.sectors.get(index).is_none() {
                return Err(range_error("sector", index, map.sectors.len()));
            }
            (index, Vec::new())
        }
        LiftPlatform::Draw(polygon) => draw_platform(map, &polygon, &spec.side)?,
    };
    let floor = map
        .sectors
        .get(platform_sector)
        .map(|row| row.floor_height)
        .ok_or_else(|| {
            CyberError::Internal(format!(
                "the lift platform sector {platform_sector} vanished while the lift was \
                 built (server bug)"
            ))
        })?;
    let low_floor = resolve_low_floor(map, platform_sector, spec.low_floor)?;
    if low_floor >= floor {
        return Err(no_way_down(platform_sector, floor, low_floor));
    }
    let edges = activation_edges(map, platform_sector, floor);
    if edges.is_empty() {
        return Err(no_edges(platform_sector));
    }
    let tag = next_free_tag(map);
    stamp_platform(map, platform_sector, tag, &spec.floor_flat);
    let setup = lift_specials(map.flag_family(), spec.style, spec.repeatable, tag);
    let lines = dress_edges(map, &edges, &setup, &spec.side);
    let drop = floor.saturating_sub(low_floor);
    if drop < MIN_LIFT_DROP {
        notes.push(shrink_note(drop));
    }
    Ok(LiftBuild {
        platform_sector,
        tag,
        lines,
        floor_height: floor,
        low_floor,
        drop,
        notes,
    })
}
