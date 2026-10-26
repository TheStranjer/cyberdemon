//! The teleporter step of the pipeline: stand every `teleporter` row's
//! pad and destination squares, in plan order.
//!
//! The step runs [`build_teleporter`] on each row: the pad and
//! destination squares (default: the room centres, explicit: transformed
//! plan points) must fit their rooms with [`TELEPORT_CLEARANCE`] units
//! of clearance at the fixed [`DEFAULT_PAD_SIZE`] side, the pad borders
//! carry the family's walk-on teleport to the fresh-tagged destination
//! and the builder appends its own type-14 destination things straight
//! into the map before the plan's things are placed. Each square
//! rebuilds the whole map through
//! [`apply_overlay`](crate::wad::map::overlay::apply_overlay), so
//! [`remap_rows`] follows every earlier row's line indices through the
//! rebuild.

use crate::error::CyberError;
use crate::fixed::{midpoint, narrow_i32};
use crate::geom::Pt;
use crate::plan::emit::CompiledPlan;
use crate::plan::model::{Feature, Plan};
use crate::wad::map::Map;
use crate::wad::map::features::{DEFAULT_PAD_SIZE, TeleportSpec, build_teleporter};
use crate::wad::map::geometry::{point_in_sector, sector_bbox};
use crate::wad::map::ops::Point;

use super::{FeatureDress, FeatureRow, FeatureRun, NO_TEXTURE, invalid, room_sectors_of};

/// The units by which the plan teleporter's fit rule insets the square's
/// corners: every corner of the 64-unit square shrunk by this much must
/// resolve into the room that owns the square.
const TELEPORT_CLEARANCE: i32 = 8_i32;

/// The teleporter step: stand every `teleporter` row's pad and
/// destination squares, in plan order.
pub(super) fn teleporters(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in &plan.features {
        if !matches!(feature, Feature::Teleporter { .. }) {
            continue;
        }
        // A `&Feature` cannot be destructured with bindings under this
        // crate's denied `pattern_type_mismatch` and `ref_patterns`, so
        // only the matching row is cloned, never the whole vector.
        let Feature::Teleporter {
            from_room,
            to_room,
            pad,
            destination,
            angle,
            two_way,
        } = feature.clone()
        else {
            continue;
        };
        let row = TeleporterRow {
            pad: teleport_square("pad", &from_room, pad, compiled)?,
            destination: teleport_square("destination", &to_room, destination, compiled)?,
            angle,
            two_way,
        };
        let spec = teleport_spec(compiled, dress, &row)?;
        let built = build_teleporter(&mut compiled.map, &spec)?;
        remap_rows(&mut run.rows, &built.line_remap);
        compiled.warnings.extend(built.notes.iter().cloned());
        run.rows.push(FeatureRow {
            kind: "teleporter",
            room: Some(from_room),
            sectors: vec![built.pad_sector, built.destination_sector],
            lines: built.pad_lines,
            tag: Some(built.tag),
            line_ends: Vec::new(),
        });
    }
    Ok(())
}

/// Follow every line index reported so far through one teleporter's
/// square rebuilds: the arrangement re-sorts the map's lines, so a
/// row must name its lines as the finished map holds them, and a line
/// a square painted over drops out of the row.
pub(super) fn remap_rows(rows: &mut [FeatureRow], remap: &[Option<usize>]) {
    for row in rows {
        row.lines = row
            .lines
            .iter()
            .copied()
            .filter_map(|old| remap.get(old).copied().flatten())
            .collect();
    }
}

/// One square of a plan teleporter: the room it stands in, the room's
/// sectors in the compiled map, and the transformed centre the wire
/// gave, if any.
struct TeleportSquare<'plan> {
    /// The word naming the square in messages: `pad` or `destination`.
    role: &'static str,
    /// The id of the room the square stands in.
    room: &'plan str,
    /// The wire's transformed, snapped centre, if it gave one.
    given: Option<Pt>,
    /// The indices into the compiled map's sectors that `room` owns.
    sectors: Vec<usize>,
}

/// One square resolved against `compiled`: its room must own sectors
/// the emission kept.
fn teleport_square<'plan>(
    role: &'static str,
    room: &'plan str,
    given: Option<Pt>,
    compiled: &CompiledPlan,
) -> Result<TeleportSquare<'plan>, CyberError> {
    Ok(TeleportSquare {
        role,
        room,
        given,
        sectors: room_sectors_of(compiled, room)?,
    })
}

/// The two squares and the facing of one `teleporter` row.
struct TeleporterRow<'plan> {
    /// The pad square, standing in `from_room`.
    pad: TeleportSquare<'plan>,
    /// The destination square, standing in `to_room`.
    destination: TeleportSquare<'plan>,
    /// The arriving player's facing, degrees (the builder normalises).
    angle: i32,
    /// Whether the destination teleports back to the pad as well.
    two_way: bool,
}

/// The centre of `square`: the wire's point, else the centre of the
/// room sectors' merged bounding box.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a wire point past the map
/// coordinate range, [`CyberError::Internal`] for a room the compiled
/// map bounds nowhere.
fn square_centre(map: &Map, square: &TeleportSquare<'_>) -> Result<Point, CyberError> {
    let given = match square.given {
        Some(point) => map_point(point, square.role)?,
        None => merged_centre(map, &square.sectors).ok_or_else(|| unbounded_room(square))?,
    };
    Ok(given)
}

/// The `Internal` refusal for a room whose sectors bound nothing.
fn unbounded_room(square: &TeleportSquare<'_>) -> CyberError {
    CyberError::Internal(format!(
        "the teleporter {} room '{}' owns no sector the compiled map can bound; pass an \
         explicit {} point",
        square.role, square.room, square.role
    ))
}

/// Require every [`TELEPORT_CLEARANCE`]-inset corner of the 64-unit
/// square at `centre` to resolve through [`point_in_sector`] into one
/// of the square's room sectors.
fn assert_square_fits(
    map: &Map,
    centre: Point,
    square: &TeleportSquare<'_>,
) -> Result<(), CyberError> {
    for (x, y) in square_corners(centre) {
        let inside = point_in_sector(map, i64::from(x), i64::from(y))
            .is_some_and(|sector| square.sectors.contains(&sector));
        if !inside {
            return Err(square_too_big(square, x, y));
        }
    }
    Ok(())
}

/// The [`TeleportSpec`] behind one row: both centres resolved and
/// fitted, the fixed [`DEFAULT_PAD_SIZE`] side, the rooms' own floor
/// flats and the theme's `wall` for bare new edges.
///
/// # Errors
///
/// [`square_centre`]'s and [`assert_square_fits`]' refusals for either
/// square.
fn teleport_spec(
    compiled: &CompiledPlan,
    dress: &FeatureDress,
    row: &TeleporterRow<'_>,
) -> Result<TeleportSpec, CyberError> {
    let map = &compiled.map;
    let pad = square_centre(map, &row.pad)?;
    assert_square_fits(map, pad, &row.pad)?;
    let destination = square_centre(map, &row.destination)?;
    assert_square_fits(map, destination, &row.destination)?;
    Ok(TeleportSpec {
        pad,
        destination,
        angle: row.angle,
        pad_size: DEFAULT_PAD_SIZE,
        two_way: row.two_way,
        floor_flat: NO_TEXTURE.to_owned(),
        wall: teleport_wall(dress),
    })
}

/// The texture a plan teleporter's bare new edges fall back to: the
/// theme's `wall` name, `"-"` when the theme left the role unnamed.
pub(super) fn teleport_wall(dress: &FeatureDress) -> String {
    dress.texture("wall").unwrap_or(NO_TEXTURE).to_owned()
}

/// The four corners of the [`DEFAULT_PAD_SIZE`] square centred on
/// `centre`, inset by [`TELEPORT_CLEARANCE`] units.
const fn square_corners(centre: Point) -> [(i32, i32); 4] {
    let reach = DEFAULT_PAD_SIZE
        .saturating_div(2)
        .saturating_sub(TELEPORT_CLEARANCE);
    let (x, y) = (centre.x, centre.y);
    [
        (x.saturating_sub(reach), y.saturating_sub(reach)),
        (x.saturating_add(reach), y.saturating_sub(reach)),
        (x.saturating_add(reach), y.saturating_add(reach)),
        (x.saturating_sub(reach), y.saturating_add(reach)),
    ]
}

/// The centre of the merged bounding box of `sectors`, or `None` when
/// no one of them bounds a resolvable vertex.
fn merged_centre(map: &Map, sectors: &[usize]) -> Option<Point> {
    let (min_x, min_y, max_x, max_y) = merged_bbox(map, sectors)?;
    Some(Point {
        x: mid_axis(min_x, max_x),
        y: mid_axis(min_y, max_y),
    })
}

/// The box covering `sectors`, or `None` when no one of them bounds a
/// resolvable vertex.
fn merged_bbox(map: &Map, sectors: &[usize]) -> Option<(i32, i32, i32, i32)> {
    sectors
        .iter()
        .filter_map(|&index| sector_bbox(map, index))
        .reduce(|(a1, b1, a2, b2), (c1, d1, c2, d2)| {
            (a1.min(c1), b1.min(d1), a2.max(c2), b2.max(d2))
        })
}

/// The centre axis of two box edges, through [`crate::fixed::midpoint`]
/// and saturated back into the plan grid.
fn mid_axis(low: i32, high: i32) -> i32 {
    narrow_i32(i128::from(midpoint(i64::from(low), i64::from(high))))
}

/// The map-unit [`Point`] of a transformed plan coordinate, refused
/// when an axis does not fit the map's `i32` grid.
fn map_point(point: Pt, role: &str) -> Result<Point, CyberError> {
    to_map_point(point).ok_or_else(|| centre_out_of_range(point, role))
}

/// The map-unit [`Point`] of a plan coordinate, `None` when an axis does
/// not fit the map's `i32` grid.
pub(super) fn to_map_point(point: Pt) -> Option<Point> {
    let (x, y) = point;
    i32::try_from(x)
        .ok()
        .zip(i32::try_from(y).ok())
        .map(|(fx, fy)| Point { x: fx, y: fy })
}

/// The refusal for a wire centre past the map coordinate range.
fn centre_out_of_range(point: Pt, role: &str) -> CyberError {
    invalid(&format!(
        "the teleporter {role} point {}, {} is outside the map coordinate range: keep \
         plan points and the units transform inside the map grid",
        point.0, point.1
    ))
}

/// The refusal for a teleporter square its room cannot hold.
fn square_too_big(square: &TeleportSquare<'_>, x: i32, y: i32) -> CyberError {
    invalid(&format!(
        "the teleporter {} does not fit inside room '{}' with {TELEPORT_CLEARANCE} units \
         of clearance: the square corner at {x}, {y} lands outside the room; pass an \
         explicit {} point nearer the room's middle or move the rooms apart",
        square.role, square.room, square.role,
    ))
}
