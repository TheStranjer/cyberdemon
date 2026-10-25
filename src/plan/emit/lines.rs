//! Pieces to lines and the bookkeeping read back off them: every piece
//! becomes one [`Linedef`](crate::wad::map::Linedef), flipped so its
//! front [`Sidedef`] faces a sector, with the far side kept only against
//! another region; the shared endpoint table folds coincident piece ends
//! into one [`Vertex`](crate::wad::map::Vertex) row. The
//! [`compiled_plan`](super::compiled_plan) lookups live here too: the
//! room-to-sector map, one connector's surviving sectors and the
//! two-sided door lines that join them to its rooms.

use alloc::collections::BTreeMap;

use super::{ConnectorInfo, Emitted, NO_NAME, region_label};
use crate::error::CyberError;
use crate::geom::label::Label;
use crate::geom::{Piece, Pt};
use crate::plan::connectors::ConnectorGeometry;
use crate::plan::model::Plan;
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::{Linedef, Map, Sidedef, Vertex};

/// One linedef (and its sides' [`Sidedef`] rows) per piece, sharing one
/// endpoint table so coincident piece ends become one [`Vertex`].
pub(super) fn push_linedefs(
    map: &mut Map,
    pieces: &[Piece],
    sector_of_label: &BTreeMap<Label, usize>,
    seen: &mut BTreeMap<Pt, usize>,
) -> Result<(), CyberError> {
    for piece in pieces {
        push_piece(map, piece, sector_of_label, seen)?;
    }
    Ok(())
}

/// Append the line `piece` becomes: flipped so the non-void face is the
/// front (the `Doom` right side), one-sided and blocking against the
/// void, two-sided against the sector on the far face. Both sides get
/// their own empty placeholder [`Sidedef`].
fn push_piece(
    map: &mut Map,
    piece: &Piece,
    sector_of_label: &BTreeMap<Label, usize>,
    seen: &mut BTreeMap<Pt, usize>,
) -> Result<(), CyberError> {
    let (start, stop, front_label, back_label) = oriented(piece);
    let front = sector_for(sector_of_label, front_label)?;
    let back = match back_label {
        Label::Void => None,
        Label::Region(_) => Some(sector_for(sector_of_label, back_label)?),
    };
    let v1 = vertex_at(map, start, seen)?;
    let v2 = vertex_at(map, stop, seen)?;
    let front_side = map.sidedefs.len();
    map.sidedefs.push(side(front));
    let back_side = match back {
        Some(sector) => {
            map.sidedefs.push(side(sector));
            Some(front_side.saturating_add(1))
        }
        None => None,
    };
    let flags = match back_side {
        Some(_) => TWO_SIDED,
        None => BLOCKING,
    };
    map.linedefs.push(Linedef {
        v1,
        v2,
        flags,
        special: 0_i32,
        tag: 0_i32,
        args: [0_i32; 5_usize],
        front: Some(front_side),
        back: back_side,
        extra: BTreeMap::new(),
    });
    Ok(())
}

/// The `Doom` orientation rule for one piece: the front side must be on
/// the right, so a piece whose right side is void runs `b`→`a` and its
/// left face becomes the front. Pieces with both sides void cannot come
/// from [`arrange`](fn@crate::geom::arrange), which drops them.
const fn oriented(piece: &Piece) -> (Pt, Pt, Label, Label) {
    if matches!(piece.right, Label::Void) {
        (piece.b, piece.a, piece.left, piece.right)
    } else {
        (piece.a, piece.b, piece.right, piece.left)
    }
}

/// The sector a surviving face label owns.
///
/// # Errors
///
/// [`emit`](super::emit)'s `Internal` when the label owns no sector: unreachable for
/// a plan whose layers and sectors came from the same [`emit`](super::emit) call,
/// since every surviving region label got one.
fn sector_for(sector_of_label: &BTreeMap<Label, usize>, label: Label) -> Result<usize, CyberError> {
    sector_of_label.get(&label).copied().ok_or_else(|| {
        CyberError::Internal(format!(
            "no sector was allocated for {}; both faces of a piece cannot be void",
            label.describe()
        ))
    })
}

/// The index of `point` in [`Map::vertices`], appending a new row (and
/// recording it in `seen`) when the endpoints have not used it yet.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] naming the point when a coordinate does
/// not fit `i32`.
fn vertex_at(
    map: &mut Map,
    point: Pt,
    seen: &mut BTreeMap<Pt, usize>,
) -> Result<usize, CyberError> {
    if let Some(index) = seen.get(&point).copied() {
        return Ok(index);
    }
    let Ok(x) = i32::try_from(point.0) else {
        return Err(outside_coordinates(point));
    };
    let Ok(y) = i32::try_from(point.1) else {
        return Err(outside_coordinates(point));
    };
    let index = map.vertices.len();
    map.vertices.push(Vertex {
        x,
        y,
        extra: BTreeMap::new(),
    });
    seen.insert(point, index);
    Ok(index)
}

/// The `LimitExceeded` for a piece endpoint the map format cannot store.
fn outside_coordinates(point: Pt) -> CyberError {
    let (x, y) = point;
    CyberError::LimitExceeded(format!(
        "emitted vertex ({x}, {y}) is outside the 32-bit map coordinate range; keep every plan coordinate between {} and {}",
        i32::MIN,
        i32::MAX
    ))
}

/// An empty placeholder side facing `sector`: all textures `"-"`, both
/// offsets `0`, nothing for the later formats.
fn side(sector: usize) -> Sidedef {
    Sidedef {
        upper: NO_NAME.to_owned(),
        lower: NO_NAME.to_owned(),
        middle: NO_NAME.to_owned(),
        sector,
        ..Default::default()
    }
}

/// Room id to its sectors, for [`compiled_plan`](super::compiled_plan).
pub(super) fn room_sector_map(plan: &Plan, emitted: &Emitted) -> BTreeMap<String, Vec<usize>> {
    let mut rooms = BTreeMap::new();
    for (index, room) in plan.rooms.iter().enumerate() {
        if let Some(sector) = emitted.sector_of_label.get(&region_label(index)).copied() {
            rooms.insert(room.id.clone(), vec![sector]);
        }
    }
    rooms
}

/// The [`ConnectorInfo`] for one placed connector: its surviving sectors
/// and the lines that join them to the rooms.
pub(super) fn connector_info(connector: &ConnectorGeometry, emitted: &Emitted) -> ConnectorInfo {
    let sectors: Vec<usize> = connector
        .layers
        .iter()
        .filter_map(|layer| emitted.sector_of_label.get(&layer.label).copied())
        .collect();
    let rooms = connector_room_sectors(connector, emitted);
    let door_lines = door_lines(&emitted.map, &sectors, &rooms);
    ConnectorInfo {
        id: connector.id.clone(),
        kind: connector.kind,
        sectors,
        door_lines,
    }
}

/// The sectors of the two rooms a connector joins that survived
/// emission (in either order, usually two).
fn connector_room_sectors(connector: &ConnectorGeometry, emitted: &Emitted) -> Vec<usize> {
    [connector.from, connector.to]
        .into_iter()
        .filter_map(|index| emitted.sector_of_label.get(&region_label(index)).copied())
        .collect()
}

/// The indices of the two-sided lines joining any of `connector`'s
/// sectors to any of the `rooms`' sectors.
fn door_lines(map: &Map, connector: &[usize], rooms: &[usize]) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_door_line(map, line, connector, rooms).then_some(index))
        .collect()
}

/// Whether `line` faces a connector sector on one side and a connected
/// room's sector on the other.
fn is_door_line(map: &Map, line: &Linedef, connector: &[usize], rooms: &[usize]) -> bool {
    let Some((front, back)) = line_sides(map, line) else {
        return false;
    };
    (connector.contains(&front) && rooms.contains(&back))
        || (connector.contains(&back) && rooms.contains(&front))
}

/// The sectors behind both sides of `line`, `None` when it is one-sided
/// or a side index dangles.
fn line_sides(map: &Map, line: &Linedef) -> Option<(usize, usize)> {
    let front = side_sector(map, line.front?)?;
    let back = side_sector(map, line.back?)?;
    Some((front, back))
}

/// The sector index of one [`Sidedef`] row, `None` when it dangles.
pub(super) fn side_sector(map: &Map, index: usize) -> Option<usize> {
    map.sidedefs.get(index).map(|side| side.sector)
}
