//! Arrangement to map emission: the compiled plan becomes a fresh [`Map`].
//!
//! [`emit`] stacks a [`Plan`]'s [`final_layers`], cuts the stack with
//! [`arrange`](fn@crate::geom::arrange), gives every surviving region
//! label one [`Sector`] — rooms in plan order, then connector layers and
//! stair steps in allocator order — deduplicates the piece endpoints into
//! [`Vertex`] rows and turns every piece into exactly one [`Linedef`]
//! whose front (`Doom` right) side faces a sector. Texture roles are out
//! of scope: every side texture is `"-"`, room flats are the room's own
//! overrides or `"-"` (`outdoor` rooms fall back to `"F_SKY1"`), and the
//! textures slice fills the role names later. [`compiled_plan`] wraps an
//! emission together with the room and connector bookkeeping later
//! slices report. Nothing here touches archives, sessions or `MCP`
//! types.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::geom::label::{Label, LayerLabeler};
use crate::geom::{Piece, Pt, arrange};
use crate::plan::connectors::{ConnectorGeometry, final_layers};
use crate::plan::model::{ConnectionKind, Plan, Room};
use crate::plan::walls::Walls;
use crate::wad::classify::MapFormat;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The `Doom` linedef `blocking` bit: engines stop motion at the line.
const FLAG_BLOCKING: u32 = 1_u32;

/// The `Doom` linedef `twosided` bit: the line has a sector behind it.
const FLAG_TWOSIDED: u32 = 4_u32;

/// The model's name for "draw nothing here" on a texture or flat face.
const NO_NAME: &str = "-";

/// The ceiling flat [`emit`] gives an `outdoor` room's sector.
const SKY_FLAT: &str = "F_SKY1";

/// The identity of the map [`emit`] produces: the block-header name and
/// the layout its rows will be written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapIdentity {
    /// The `LumpName` of the emitted map block.
    pub name: LumpName,
    /// The layout class of the emitted map.
    pub format: MapFormat,
}

/// What [`emit`] built: the fresh map, the label-to-sector index and the
/// raw pieces the linedefs were cut from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The emitted map: no things, no nodes, no `GL` run, `other` empty.
    pub map: Map,
    /// The sector owning each region label that survived the arrangement.
    /// Rooms come first in plan order, then connector layers and stair
    /// steps in allocator order; [`Label::Void`] owns no sector.
    pub sector_of_label: BTreeMap<Label, usize>,
    /// The arranged pieces in [`Map::linedefs`] order: one linedef per
    /// piece, so the later slices can match a line back to its faces.
    pub pieces: Vec<Piece>,
}

/// Everything [`compiled_plan`] reports about one placed connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// The connection's id, or its `from->to` label when unnamed.
    pub id: String,
    /// The kind of connection, from the model through the placement.
    pub kind: ConnectionKind,
    /// The sectors of the connector's layers that survived, in allocator
    /// (stair step) order. Empty for an opening the plan opened across a
    /// whole shared edge: [`ConnectorGeometry::joined`] took the wall
    /// there down, so the rooms share their own boundary line and the
    /// connection owns no sector.
    pub sectors: Vec<usize>,
    /// The [`Map::linedefs`] indices whose two sides are one of
    /// [`ConnectorInfo::sectors`] and one of the connector's two rooms'
    /// sectors: the lines a door or switch could hang on. A rectangular
    /// crossing through a wall produces exactly two of them, the jambs
    /// where the band meets each wall face; a stair run or a
    /// zero-thickness band produces more as its outline meets the rooms
    /// along extra cuts, and a [`joined`](ConnectorGeometry::joined)
    /// opening produces none.
    pub door_lines: Vec<usize>,
}

/// A whole compiled plan: the [`Map`] to write and the bookkeeping to
/// report it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledPlan {
    /// The emitted map.
    pub map: Map,
    /// Room id to the sectors that room owns. Each list holds exactly one
    /// sector today; it stays a `Vec` so later slices can give one room
    /// several sectors (pillar rooms, separated wings) without breaking
    /// callers. A room fully painted over by a later one owns none and
    /// holds no entry.
    pub room_sectors: BTreeMap<String, Vec<usize>>,
    /// One [`ConnectorInfo`] per placed connection, in plan order.
    pub connector_sectors: Vec<ConnectorInfo>,
    /// Advisory notes for the caller. [`compiled_plan`] always leaves
    /// this empty: the final reporting slice fills it with the plan
    /// quirks (clamped heights, refused connections) worth surfacing.
    pub warnings: Vec<String>,
}

/// The sectors allocated so far and the labels they answer.
#[derive(Default)]
struct Allocation {
    /// The sector rows, in [`Map::sectors`] order.
    sectors: Vec<Sector>,
    /// Region label to its sector index.
    sector_of_label: BTreeMap<Label, usize>,
}

impl Allocation {
    /// Append `sector` for `label`, refusing a label that already owns a
    /// sector: two layers sharing one label can only come from mismatched
    /// earlier slices.
    fn take(&mut self, label: Label, sector: Sector) -> Result<(), CyberError> {
        if self.sector_of_label.contains_key(&label) {
            return Err(CyberError::Internal(format!(
                "{} owns two sectors; every room and connector layer needs its own label",
                label.describe()
            )));
        }
        let index = self.sectors.len();
        self.sectors.push(sector);
        self.sector_of_label.insert(label, index);
        Ok(())
    }
}

/// Emit a fresh `Doom` [`Map`] realising `plan`'s rooms, `walls` and the
/// placed `connectors`, identified by `identity`.
///
/// The [`final_layers`] stack is cut with
/// [`arrange`](fn@crate::geom::arrange); every region label surviving on
/// a piece face gets one [`Sector`] (rooms in plan order first, then
/// connector layers and stair steps in allocator order), every piece
/// endpoint becomes one deduplicated [`Vertex`] and every piece becomes
/// exactly one [`Linedef`], flipped so its front side faces its sector.
/// Sectors and lines carry the plan's heights and lights only: all side
/// textures and un-overridden flats stay `"-"` and the map has no things,
/// specials or tags until the later slices add them.
///
/// `connectors` must be the [`plan_connectors`](crate::plan::connectors::plan_connectors)
/// output for this same `plan` and `walls`; mixing slices is refused as
/// an internal error.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] and [`CyberError::LimitExceeded`] from
/// [`arrange`](fn@crate::geom::arrange) pass through;
/// [`CyberError::LimitExceeded`] also refuses any piece endpoint outside
/// the 32-bit map coordinate range. [`CyberError::Internal`] reports the
/// defensive contract breaks: a piece face labelled by no room and no
/// connector layer, one label owning two sectors, a connector without
/// its recorded height span, or a stair run pointing at rooms the plan
/// does not hold.
#[inline]
pub fn emit(
    plan: &Plan,
    walls: &Walls,
    connectors: &[ConnectorGeometry],
    identity: MapIdentity,
) -> Result<Emitted, CyberError> {
    let layers = final_layers(plan, walls, connectors);
    let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers))?;
    let surviving = surviving_regions(&pieces);
    let allocation = new_sectors(plan, connectors, &surviving)?;
    let mut map = Map::new_empty(identity.name, identity.format);
    map.sectors = allocation.sectors;
    let mut seen = BTreeMap::new();
    push_linedefs(&mut map, &pieces, &allocation.sector_of_label, &mut seen)?;
    Ok(Emitted {
        map,
        sector_of_label: allocation.sector_of_label,
        pieces,
    })
}

/// Wrap `emitted`'s map in the [`CompiledPlan`] bookkeeping for `plan`
/// and its placed `connectors`.
///
/// Every lookup reads the emission's own label index, so rooms fully
/// painted over by later ones simply hold no entry.
#[must_use]
#[inline]
pub fn compiled_plan(
    plan: &Plan,
    connectors: &[ConnectorGeometry],
    emitted: Emitted,
) -> CompiledPlan {
    let room_sectors = room_sector_map(plan, &emitted);
    let connector_sectors = connectors
        .iter()
        .map(|connector| connector_info(connector, &emitted))
        .collect();
    CompiledPlan {
        map: emitted.map,
        room_sectors,
        connector_sectors,
        warnings: Vec::new(),
    }
}

/// The region labels the pieces carry on their faces; void faces are
/// solid rock and own no sector.
fn surviving_regions(pieces: &[Piece]) -> BTreeSet<Label> {
    let mut labels = BTreeSet::new();
    for piece in pieces {
        if matches!(piece.left, Label::Region(_)) {
            labels.insert(piece.left);
        }
        if matches!(piece.right, Label::Region(_)) {
            labels.insert(piece.right);
        }
    }
    labels
}

/// Build the sector rows: one per surviving region label, rooms in plan
/// order first, then every connector's layers in allocator order.
///
/// # Errors
///
/// [`emit`]'s `Internal` refusals: see the two add passes and
/// [`check_allocated`].
fn new_sectors(
    plan: &Plan,
    connectors: &[ConnectorGeometry],
    surviving: &BTreeSet<Label>,
) -> Result<Allocation, CyberError> {
    let mut allocation = Allocation::default();
    add_room_sectors(plan, surviving, &mut allocation)?;
    add_connector_sectors(plan, connectors, surviving, &mut allocation)?;
    check_allocated(surviving, &allocation)?;
    Ok(allocation)
}

/// One sector per surviving room label, in plan order.
fn add_room_sectors(
    plan: &Plan,
    surviving: &BTreeSet<Label>,
    allocation: &mut Allocation,
) -> Result<(), CyberError> {
    for (index, room) in plan.rooms.iter().enumerate() {
        let label = region_label(index);
        if surviving.contains(&label) {
            allocation.take(label, room_sector(room))?;
        }
    }
    Ok(())
}

/// One sector per surviving connector layer — the band of an opening,
/// door or window, each riser of a stair run — in allocator order.
fn add_connector_sectors(
    plan: &Plan,
    connectors: &[ConnectorGeometry],
    surviving: &BTreeSet<Label>,
    allocation: &mut Allocation,
) -> Result<(), CyberError> {
    for connector in connectors {
        add_layer_sectors(plan, connector, surviving, allocation)?;
    }
    Ok(())
}

/// The sectors of one connector's surviving layers, step order first.
fn add_layer_sectors(
    plan: &Plan,
    connector: &ConnectorGeometry,
    surviving: &BTreeSet<Label>,
    allocation: &mut Allocation,
) -> Result<(), CyberError> {
    for (step, layer) in connector.layers.iter().enumerate() {
        if surviving.contains(&layer.label) {
            let sector = connector_sector(plan, connector, step)?;
            allocation.take(layer.label, sector)?;
        }
    }
    Ok(())
}

/// Refuse a surviving region label that no room and no connector layer
/// owns: [`final_layers`] paints with exactly those, so a piece carrying
/// anything else means the slices handed to [`emit`] disagree.
fn check_allocated(surviving: &BTreeSet<Label>, allocation: &Allocation) -> Result<(), CyberError> {
    for label in surviving {
        if !allocation.sector_of_label.contains_key(label) {
            return Err(CyberError::Internal(format!(
                "the arrangement carries {}, but no plan room or connector layer owns it",
                label.describe()
            )));
        }
    }
    Ok(())
}

/// The region label of plan index `index`, matching the room numbering
/// of [`room_layers`](crate::plan::walls::room_layers); indices always
/// come from the validated plan's own length.
fn region_label(index: usize) -> Label {
    Label::Region(u32::try_from(index).unwrap_or(u32::MAX))
}

/// The sector row for a room: its heights and light, its flat overrides
/// or the convention default (`outdoor` rooms fall back to the sky), and
/// no special or tag yet.
fn room_sector(room: &Room) -> Sector {
    let ceiling_default = if room.outdoor { SKY_FLAT } else { NO_NAME };
    Sector {
        floor_height: room.floor,
        ceiling_height: room.ceiling,
        floor_flat: flat_name(room.floor_flat.as_deref(), NO_NAME),
        ceiling_flat: flat_name(room.ceiling_flat.as_deref(), ceiling_default),
        light: room.light,
        special: 0_i32,
        tag: 0_i32,
        extra: BTreeMap::new(),
    }
}

/// The flat row for an override name: uppercased to the model's
/// convention, `fallback` when the room names none.
fn flat_name(name: Option<&str>, fallback: &str) -> String {
    name.map_or_else(|| fallback.to_owned(), str::to_uppercase)
}

/// The sector row for connector layer `step`, built purely from the
/// metadata the connectors slice recorded: openings and windows take
/// their stored span, doors close onto their floor, stair steps rise
/// under the higher room's ceiling so the risers face up, and every kind
/// lights at the dimmer of its two rooms.
///
/// # Errors
///
/// [`emit`]'s `Internal` refusals for missing spans or foreign rooms.
fn connector_sector(
    plan: &Plan,
    connector: &ConnectorGeometry,
    step: usize,
) -> Result<Sector, CyberError> {
    let (from, to) = joined_rooms(plan, connector)?;
    let (floor_height, ceiling_height) = match connector.kind {
        ConnectionKind::Opening | ConnectionKind::Window { .. } => span_of(connector)?,
        ConnectionKind::Door { .. } => {
            let (floor, _) = span_of(connector)?;
            (floor, floor)
        }
        ConnectionKind::Stairs { .. } => stair_span(plan, connector, step)?,
    };
    Ok(Sector {
        floor_height,
        ceiling_height,
        floor_flat: NO_NAME.to_owned(),
        ceiling_flat: NO_NAME.to_owned(),
        light: from.light.min(to.light),
        special: 0_i32,
        tag: 0_i32,
        extra: BTreeMap::new(),
    })
}

/// The two [`Room`]s a connector joins, refusing a connector from a
/// different plan.
fn joined_rooms<'plan>(
    plan: &'plan Plan,
    connector: &ConnectorGeometry,
) -> Result<(&'plan Room, &'plan Room), CyberError> {
    let from = plan
        .rooms
        .get(connector.from)
        .ok_or_else(|| mismatched(connector))?;
    let to = plan
        .rooms
        .get(connector.to)
        .ok_or_else(|| mismatched(connector))?;
    Ok((from, to))
}

/// The `Internal` for connectors that do not belong to this plan.
fn mismatched(connector: &ConnectorGeometry) -> CyberError {
    CyberError::Internal(format!(
        "connector '{}' names room indexes {} and {} that this plan does not hold; emit the connectors planned for this plan",
        connector.id, connector.from, connector.to
    ))
}

/// The clear span the connectors slice stored for a band connector.
fn span_of(connector: &ConnectorGeometry) -> Result<(i32, i32), CyberError> {
    connector.height.ok_or_else(|| {
        CyberError::Internal(format!(
            "connector '{}' carries no height span; only stair runs may lack one",
            connector.id
        ))
    })
}

/// The `(floor, ceiling)` of stair step `step`: its own recorded floor
/// under the higher room's ceiling.
fn stair_span(
    plan: &Plan,
    connector: &ConnectorGeometry,
    step: usize,
) -> Result<(i32, i32), CyberError> {
    let floor = *connector.step_floors.get(step).ok_or_else(|| {
        CyberError::Internal(format!(
            "connector '{}' records no floor for step {step}",
            connector.id
        ))
    })?;
    let lower = connector.lower_room.ok_or_else(|| {
        CyberError::Internal(format!(
            "connector '{}' is a stair run with no lower room recorded",
            connector.id
        ))
    })?;
    let (from, to) = joined_rooms(plan, connector)?;
    let higher = if lower == connector.from {
        to
    } else if lower == connector.to {
        from
    } else {
        return Err(mismatched(connector));
    };
    Ok((floor, higher.ceiling))
}

/// One linedef (and its sides' [`Sidedef`] rows) per piece, sharing one
/// endpoint table so coincident piece ends become one [`Vertex`].
fn push_linedefs(
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
        Some(_) => FLAG_TWOSIDED,
        None => FLAG_BLOCKING,
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
/// [`emit`]'s `Internal` when the label owns no sector: unreachable for
/// a plan whose layers and sectors came from the same [`emit`] call,
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

/// Room id to its sectors, for [`compiled_plan`].
fn room_sector_map(plan: &Plan, emitted: &Emitted) -> BTreeMap<String, Vec<usize>> {
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
fn connector_info(connector: &ConnectorGeometry, emitted: &Emitted) -> ConnectorInfo {
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
fn side_sector(map: &Map, index: usize) -> Option<usize> {
    map.sidedefs.get(index).map(|side| side.sector)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{
        Emitted, FLAG_BLOCKING, FLAG_TWOSIDED, MapIdentity, compiled_plan, emit, side_sector,
    };
    use crate::defs::definitions_for;
    use crate::error::CyberError;
    use crate::geom::{Label, Piece};
    use crate::plan::connectors::{ConnectorGeometry, plan_connectors};
    use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Plan, Room, Shape};
    use crate::plan::walls::build_walls;
    use crate::wad::game::Game;
    use crate::wad::map::validate::{Context, Issue, ResourceIndex, run};
    use crate::wad::map::{Linedef, Map, MapFormat};
    use crate::wad::name::LumpName;

    /// A parsed map name for test data.
    fn map_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test map names are valid")
    }

    /// The identity every test emits into.
    fn identity() -> MapIdentity {
        MapIdentity {
            name: map_name("MAP01"),
            format: MapFormat::Doom,
        }
    }

    /// A rectangular room with default sector properties.
    fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: left_bottom.0,
                y: left_bottom.1,
                w: width_height.0,
                h: width_height.1,
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A default-width opening between two rooms by id.
    fn opening(from: &str, to: &str) -> Connection {
        Connection {
            id: None,
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width: 64,
        }
    }

    /// A styled door between two rooms by id.
    fn door(from: &str, to: &str, style: DoorStyle) -> Connection {
        let mut passage = opening(from, to);
        passage.id = Some("d1".to_owned());
        passage.kind = ConnectionKind::Door { style };
        passage
    }

    /// Two 128 x 128 rooms sharing the line x = 128, `thickness` apart.
    fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
            ],
            connections,
            features: Vec::new(),
            wall_thickness: thickness,
        }
    }

    /// A plan of `rooms` at `thickness` with no connections.
    fn rooms_plan(rooms: Vec<Room>, thickness: i64) -> Plan {
        Plan {
            rooms,
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness: thickness,
        }
    }

    /// Build, place and emit a plan that must succeed, keeping the
    /// placement for the bookkeeping asserts.
    #[track_caller]
    fn compile(plan: &Plan) -> (Emitted, Vec<ConnectorGeometry>) {
        let walls = build_walls(plan).expect("the fixture walls build");
        let connectors = plan_connectors(plan, &walls).expect("the connectors place");
        let emitted = emit(plan, &walls, &connectors, identity()).expect("the plan emits");
        (emitted, connectors)
    }

    /// Build, place and emit a plan whose emission must fail.
    #[track_caller]
    fn emit_error(plan: &Plan) -> CyberError {
        let walls = build_walls(plan).expect("the fixture walls build");
        let connectors = plan_connectors(plan, &walls).expect("the connectors place");
        emit(plan, &walls, &connectors, identity()).expect_err("the plan must not emit")
    }

    /// The sectors behind both sides of `line`, `None` when one-sided.
    fn side_sectors(map: &Map, line: &Linedef) -> Option<(usize, usize)> {
        let front = side_sector(map, line.front?)?;
        let back = side_sector(map, line.back?)?;
        Some((front, back))
    }

    /// The two sectors of the `index`th line, ordered, asserting the line
    /// is flagged two-sided.
    fn door_pair(map: &Map, index: usize) -> (usize, usize) {
        let line = map.linedefs.get(index).expect("recorded index");
        assert_eq!(line.flags, FLAG_TWOSIDED, "door lines are two-sided");
        let (front, back) = side_sectors(map, line).expect("door lines have two sides");
        (front.min(back), front.max(back))
    }

    /// The indices of the two-sided lines joining sectors `one` and
    /// `two`, in either order.
    fn lines_between(map: &Map, one: usize, two: usize) -> Vec<usize> {
        map.linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| joins_sectors(map, line, one, two).then_some(index))
            .collect()
    }

    /// Whether `line` is two-sided between sectors `one` and `two`.
    fn joins_sectors(map: &Map, line: &Linedef, one: usize, two: usize) -> bool {
        let Some((front, back)) = side_sectors(map, line) else {
            return false;
        };
        (front == one && back == two) || (front == two && back == one)
    }

    /// The flags of the `index`th line.
    fn line_flags(map: &Map, index: usize) -> u32 {
        map.linedefs.get(index).expect("recorded index").flags
    }

    /// The shared `x` of a vertical line, `None` otherwise.
    fn vertical_x(map: &Map, line: &Linedef) -> Option<i32> {
        let first = map.vertices.get(line.v1)?;
        let second = map.vertices.get(line.v2)?;
        (first.x == second.x).then_some(first.x)
    }

    /// Whether the `index`th line is vertical at coordinate `x`.
    fn vertical_at(map: &Map, index: usize, x: i32) -> bool {
        let line = map.linedefs.get(index).expect("recorded index");
        vertical_x(map, line) == Some(x)
    }

    /// Whether both ends of `line` sit on the same edge of the map's
    /// vertex bounds.
    fn on_map_border(map: &Map, line: &Linedef) -> bool {
        let (min_x, min_y, max_x, max_y) = map.bounds().expect("the map has vertices");
        let first = map.vertices.get(line.v1).expect("test indices");
        let second = map.vertices.get(line.v2).expect("test indices");
        (first.x == min_x && second.x == min_x)
            || (first.x == max_x && second.x == max_x)
            || (first.y == min_y && second.y == min_y)
            || (first.y == max_y && second.y == max_y)
    }

    /// The stored coordinate of the `index`th vertex, widened to `i64`.
    fn point_of(map: &Map, index: usize) -> (i64, i64) {
        let vertex = map.vertices.get(index).expect("test indices");
        (i64::from(vertex.x), i64::from(vertex.y))
    }

    /// Whether `line` is one-sided and its front faces sector `sector`.
    fn faces_sector(map: &Map, line: &Linedef, sector: usize) -> bool {
        line.back.is_none()
            && line
                .front
                .is_some_and(|front| side_sector(map, front) == Some(sector))
    }

    /// Sidedness and flags agree with the piece faces: void-facing pieces
    /// are the blocking one-sided lines, region-vs-region pieces are
    /// flagged two-sided with two distinct sidedef rows.
    fn assert_sidedness_matches_faces(emitted: &Emitted) {
        assert_eq!(
            emitted.map.linedefs.len(),
            emitted.pieces.len(),
            "one line per piece"
        );
        for (piece, line) in emitted.pieces.iter().zip(&emitted.map.linedefs) {
            assert_piece_facing(piece, line);
        }
    }

    /// The sidedness and flags of one piece's line.
    fn assert_piece_facing(piece: &Piece, line: &Linedef) {
        let touches_void = piece.left == Label::Void || piece.right == Label::Void;
        assert_eq!(
            touches_void,
            line.back.is_none(),
            "void-facing pieces are the one-sided lines: {piece:?}"
        );
        let expected = if line.back.is_some() {
            FLAG_TWOSIDED
        } else {
            FLAG_BLOCKING
        };
        assert_eq!(line.flags, expected, "flags match sidedness");
        let front = line.front.expect("every emitted line has a front");
        assert_ne!(line.back, Some(front), "the two sides never share a row");
    }

    /// One piece becomes exactly its linedef: the front side carries the
    /// non-void face's sector and the endpoints follow the flip rule.
    fn assert_line_orientation(emitted: &Emitted, piece: &Piece, line: &Linedef) {
        assert_ne!(piece.left, piece.right, "arrange drops same-label pieces");
        let flipped = piece.right == Label::Void;
        let front_label = if flipped { piece.left } else { piece.right };
        let expected = emitted
            .sector_of_label
            .get(&front_label)
            .copied()
            .expect("every surviving face owns a sector");
        let front = line.front.expect("every emitted line has a front");
        let row = side_sector(&emitted.map, front).expect("the front row exists");
        assert_eq!(row, expected, "the front side faces the piece's region");
        let (start, stop) = if flipped {
            (piece.b, piece.a)
        } else {
            (piece.a, piece.b)
        };
        assert_eq!(
            point_of(&emitted.map, line.v1),
            start,
            "v1 is the flipped start"
        );
        assert_eq!(
            point_of(&emitted.map, line.v2),
            stop,
            "v2 is the flipped stop"
        );
    }

    /// Every emitted side is an empty placeholder: the textures slice
    /// dresses them later.
    fn assert_sides_are_placeholders(map: &Map) {
        for side in &map.sidedefs {
            assert_eq!(
                (
                    side.upper.as_str(),
                    side.lower.as_str(),
                    side.middle.as_str()
                ),
                ("-", "-", "-"),
                "textures wait for the textures slice"
            );
            assert_eq!(side.x_offset, 0_i32, "no offsets yet");
            assert_eq!(side.y_offset, 0_i32, "no offsets yet");
        }
    }

    /// Sectors and lines carry no specials, tags, args or extras yet.
    fn assert_rows_are_plain(map: &Map) {
        for sector in &map.sectors {
            assert_eq!(sector.special, 0_i32, "no sector specials yet");
            assert_eq!(sector.tag, 0_i32, "no sector tags yet");
            assert!(sector.extra.is_empty(), "no sector extras");
        }
        for line in &map.linedefs {
            assert_eq!(line.special, 0_i32, "no line specials yet");
            assert_eq!(line.tag, 0_i32, "no line tags yet");
            assert!(line.args.iter().all(|&arg| arg == 0_i32), "no line args");
        }
    }

    /// The issues the geometry and structure validation checks report for
    /// `map`, filtered to the arrangement's contract.
    fn geometry_issues(map: &Map) -> Vec<Issue> {
        let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
        let resources = ResourceIndex::default();
        let context = Context {
            definitions: &definitions,
            hexen_specials: &definitions,
            resources: &resources,
        };
        let enabled: BTreeSet<&'static str> = [
            "crossing_lines",
            "overlapping_lines",
            "two_sided_flag_mismatch",
            "zero_length_line",
            "duplicate_line",
        ]
        .into_iter()
        .collect();
        run(map, &context, Some(&enabled)).issues
    }

    #[test]
    fn an_opening_walled_between_two_rooms_emits_three_clean_sectors() {
        let plan = two_rooms(16, vec![opening("a", "b")]);
        plan.validate().expect("the plan validates");
        let (emitted, _) = compile(&plan);
        let map = &emitted.map;
        assert_eq!(map.name, map_name("MAP01"), "the map carries its identity");
        assert_eq!(map.format, MapFormat::Doom, "in the requested layout");
        assert!(map.things.is_empty(), "the emitter draws no things");
        assert_eq!(map.sectors.len(), 3, "two rooms plus the connector band");
        assert_eq!(
            emitted.sector_of_label.get(&Label::Region(0)),
            Some(&0),
            "room a, first in plan order"
        );
        assert_eq!(
            emitted.sector_of_label.get(&Label::Region(1)),
            Some(&1),
            "room b, second"
        );
        assert_eq!(
            emitted.sector_of_label.get(&Label::Region(2)),
            Some(&2),
            "the connector follows the rooms in allocator order"
        );
        let band = map.sectors.get(2).expect("the band sector");
        assert_eq!(band.floor_height, 0_i32, "the higher floor");
        assert_eq!(band.ceiling_height, 128_i32, "the lower ceiling");
        assert_eq!(band.light, 160_i32, "the dimmer light");
        assert_sides_are_placeholders(map);
        assert_sidedness_matches_faces(&emitted);
        assert!(geometry_issues(map).is_empty(), "the emission is clean");
    }

    #[test]
    fn every_line_fronts_its_pieces_region_and_keeps_the_flip() {
        let plan = two_rooms(16, vec![opening("a", "b")]);
        let (emitted, _) = compile(&plan);
        for (piece, line) in emitted.pieces.iter().zip(&emitted.map.linedefs) {
            assert_line_orientation(&emitted, piece, line);
        }
        assert_eq!(
            emitted.map.linedefs.len(),
            emitted.pieces.len(),
            "one line per piece"
        );
    }

    #[test]
    fn rooms_touching_at_a_corner_share_one_vertex_row() {
        let plan = rooms_plan(
            vec![
                rect_room("a", (0, 0), (64, 64)),
                rect_room("b", (64, 64), (64, 64)),
            ],
            0,
        );
        let (emitted, _) = compile(&plan);
        let map = &emitted.map;
        assert_eq!(map.sectors.len(), 2, "one sector per room");
        assert_eq!(
            map.vertices.len(),
            7,
            "the corner both rooms share is emitted once"
        );
        let shared = map
            .vertices
            .iter()
            .filter(|vertex| vertex.x == 64_i32 && vertex.y == 64_i32)
            .count();
        assert_eq!(shared, 1, "exactly one vertex row at (64, 64)");
        assert_eq!(map.linedefs.len(), 8, "four walls per room, none shared");
        assert!(
            map.linedefs
                .iter()
                .all(|line| line.back.is_none() && line.flags == FLAG_BLOCKING),
            "point-touching rooms share no line and every wall blocks"
        );
        assert!(geometry_issues(map).is_empty(), "the emission is clean");
    }

    #[test]
    fn a_door_closes_onto_its_floor_and_books_two_door_lines() {
        let plan = two_rooms(16, vec![door("a", "b", DoorStyle::Fast)]);
        let (emitted, connectors) = compile(&plan);
        let compiled = compiled_plan(&plan, &connectors, emitted);
        assert!(
            compiled.warnings.is_empty(),
            "warnings stay empty until the reporting slice"
        );
        assert_eq!(
            compiled.room_sectors.get("a"),
            Some(&vec![0_usize]),
            "one sector per room, as a list for later slices"
        );
        assert_eq!(
            compiled.room_sectors.get("b"),
            Some(&vec![1_usize]),
            "and for the other room too"
        );
        let info = compiled.connector_sectors.first().expect("one placed door");
        assert_eq!(info.id, "d1", "the connection's own id");
        assert_eq!(
            info.kind,
            ConnectionKind::Door {
                style: DoorStyle::Fast
            },
            "the kind rides along"
        );
        assert_eq!(info.sectors, vec![2], "the band's sector");
        let band = compiled.map.sectors.get(2).expect("the band sector row");
        assert_eq!(band.floor_height, 0_i32, "the higher floor");
        assert_eq!(
            band.ceiling_height, band.floor_height,
            "a closed door starts flat with its floor"
        );
        assert_eq!(
            info.door_lines.len(),
            2,
            "the band meets each wall face once"
        );
        let mut joins: Vec<(usize, usize)> = info
            .door_lines
            .iter()
            .map(|&index| door_pair(&compiled.map, index))
            .collect();
        joins.sort_unstable();
        assert_eq!(
            joins,
            vec![(0, 2), (1, 2)],
            "each door line joins the band to one room"
        );
    }

    #[test]
    fn a_joined_opening_books_no_sector_and_no_door_line() {
        let mut arch = opening("a", "b");
        arch.id = Some("arch".to_owned());
        arch.width = 128_i64;
        let plan = two_rooms(16, vec![arch]);
        let (emitted, connectors) = compile(&plan);
        assert_eq!(emitted.map.sectors.len(), 2, "the rooms alone own the map");
        assert_eq!(
            emitted
                .sector_of_label
                .values()
                .copied()
                .collect::<Vec<usize>>(),
            vec![0_usize, 1],
            "no connector label is allocated"
        );
        let compiled = compiled_plan(&plan, &connectors, emitted);
        let info = compiled.connector_sectors.first().expect("the arch");
        assert_eq!(info.id, "arch", "the connection still reports");
        assert_eq!(info.kind, ConnectionKind::Opening, "as the opening it is");
        assert!(info.sectors.is_empty(), "and it owns no sector");
        assert!(
            info.door_lines.is_empty(),
            "nor a line to hang a special on"
        );
        assert!(
            geometry_issues(&compiled.map).is_empty(),
            "the join is clean"
        );
    }

    #[test]
    fn a_doors_void_facing_reveals_are_one_sided() {
        let plan = two_rooms(16, vec![door("a", "b", DoorStyle::Fast)]);
        let (emitted, _) = compile(&plan);
        let reveals: Vec<&Linedef> = emitted
            .map
            .linedefs
            .iter()
            .filter(|line| faces_sector(&emitted.map, line, 2))
            .collect();
        assert_eq!(
            reveals.len(),
            4,
            "the band's four reveal edges face the wall void, split where the rooms' own line ran"
        );
        assert!(
            reveals.iter().all(|line| line.flags & FLAG_BLOCKING != 0),
            "the void-facing door reveals block"
        );
    }

    #[test]
    fn stairs_emit_one_riser_facing_sector_per_step() {
        let mut plan = two_rooms(16, Vec::new());
        if let Some(room) = plan.rooms.get_mut(0) {
            room.light = 96_i32;
        }
        if let Some(room) = plan.rooms.get_mut(1) {
            room.floor = 64_i32;
            room.ceiling = 160_i32;
            room.light = 192_i32;
        }
        let mut climb = opening("a", "b");
        climb.kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32,
        };
        plan.connections = vec![climb];
        let (emitted, connectors) = compile(&plan);
        assert_eq!(emitted.map.sectors.len(), 6, "two rooms plus four steps");
        let mut floors = Vec::new();
        for id in [2_u32, 3_u32, 4_u32, 5_u32] {
            let index = *emitted
                .sector_of_label
                .get(&Label::Region(id))
                .expect("every step keeps its label");
            assert_eq!(
                index,
                usize::try_from(id).expect("small ids"),
                "the steps take the sectors right after the rooms"
            );
            let step = emitted.map.sectors.get(index).expect("the step sector");
            floors.push(step.floor_height);
            assert_eq!(
                step.ceiling_height, 160_i32,
                "risers face up under the higher room's ceiling"
            );
            assert_eq!(step.light, 96_i32, "the dimmer of the two room lights");
        }
        assert_eq!(
            floors,
            vec![16_i32, 32_i32, 48_i32, 64_i32],
            "four steps, strictly increasing, lower room first"
        );
        let compiled = compiled_plan(&plan, &connectors, emitted);
        let info = compiled.connector_sectors.first().expect("the stair run");
        assert_eq!(info.sectors, vec![2, 3, 4, 5], "steps in allocator order");
        assert!(
            geometry_issues(&compiled.map).is_empty(),
            "the stair map is clean"
        );
    }

    #[test]
    fn outdoor_rooms_fall_back_to_sky_and_flat_overrides_win() {
        let mut court = rect_room("court", (0, 0), (128, 128));
        court.outdoor = true;
        court.floor_flat = Some("FLOOR4_8".to_owned());
        let mut lab = rect_room("lab", (160, 0), (128, 128));
        lab.ceiling_flat = Some("CEIL5_1".to_owned());
        lab.floor_flat = Some("ceiling1".to_owned());
        let plan = rooms_plan(vec![court, lab], 16);
        let (emitted, _) = compile(&plan);
        assert_eq!(
            emitted.map.sectors.len(),
            2,
            "no connector, no band sectors"
        );
        let sky = emitted.map.sectors.first().expect("the courtyard");
        assert_eq!(sky.ceiling_flat, "F_SKY1", "an outdoor room sees the sky");
        assert_eq!(sky.floor_flat, "FLOOR4_8", "the room's own flat is kept");
        let lab_sector = emitted.map.sectors.get(1).expect("the lab");
        assert_eq!(
            lab_sector.ceiling_flat, "CEIL5_1",
            "an explicit ceiling wins"
        );
        assert_eq!(
            lab_sector.floor_flat, "CEILING1",
            "override names follow the model's uppercase convention"
        );
        assert_sides_are_placeholders(&emitted.map);
        assert_rows_are_plain(&emitted.map);
        assert!(
            geometry_issues(&emitted.map).is_empty(),
            "the emission is clean"
        );
    }

    #[test]
    fn vertices_beyond_the_i32_range_are_refused() {
        let far = rooms_plan(vec![rect_room("field", (0, 0), (3_000_000_000, 64))], 16);
        let high = emit_error(&far);
        assert_eq!(
            high.code(),
            "limit_exceeded",
            "an unmappable point is a limit"
        );
        let message = high.to_string();
        assert!(
            message.contains("3000000000"),
            "the message names the offending x: {message}"
        );
        let deep = rooms_plan(vec![rect_room("pit", (-3_000_000_000, 0), (128, 64))], 16);
        let low = emit_error(&deep);
        assert_eq!(low.code(), "limit_exceeded", "and below the range too");
        assert!(
            low.to_string().contains("-3000000000"),
            "the message names the point: {low}"
        );
    }

    #[test]
    fn zero_thickness_rooms_share_two_sided_lines_through_the_band() {
        let plan = two_rooms(0, vec![opening("a", "b")]);
        let (emitted, _) = compile(&plan);
        let map = &emitted.map;
        assert_eq!(map.sectors.len(), 3, "the band still gets its own sector");
        let shared = lines_between(map, 0, 1);
        assert_eq!(
            shared.len(),
            2,
            "the rooms meet directly on x=128 below and above the band"
        );
        assert!(
            shared
                .iter()
                .all(|&index| line_flags(map, index) == FLAG_TWOSIDED),
            "the shared room lines are two-sided"
        );
        let west = lines_between(map, 0, 2);
        let east = lines_between(map, 2, 1);
        assert_eq!(
            west.iter()
                .filter(|&&index| vertical_at(map, index, 120_i32))
                .count(),
            1,
            "the depth-16 band cuts room a's side at x=120"
        );
        assert_eq!(
            east.iter()
                .filter(|&&index| vertical_at(map, index, 136_i32))
                .count(),
            1,
            "and room b's side at x=136"
        );
        for line in map.linedefs.iter().filter(|line| line.back.is_none()) {
            assert!(
                on_map_border(map, line),
                "the only void walls sit on the map's outer border: {line:?}"
            );
        }
        assert!(geometry_issues(map).is_empty(), "the emission is clean");
    }
}
