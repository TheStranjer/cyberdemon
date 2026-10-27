//! Arrangement to map emission: the compiled plan becomes a fresh [`Map`].
//!
//! [`emit`] stacks a [`Plan`]'s [`final_layers`], cuts the stack with
//! [`arrange`](fn@crate::geom::arrange), gives every surviving region
//! label one [`Sector`] — rooms in plan order, then connector layers and
//! stair steps in allocator order — deduplicates the piece endpoints
//! into [`Vertex`](crate::wad::map::Vertex) rows and turns every piece into
//! exactly one [`Linedef`](crate::wad::map::Linedef) whose front (`Doom`
//! right) side faces a sector. Texture roles are out
//! of scope: every side texture is `"-"`, room flats are the room's own
//! overrides or `"-"` (`outdoor` rooms fall back to the [`MapIdentity`]'s
//! sky flat), and the textures slice fills the role names later.
//! [`compiled_plan`] wraps an emission together with the room and
//! connector bookkeeping later slices report. Nothing here touches
//! archives, sessions or `MCP` types.

mod lines;
mod sectors;

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::geom::label::{Label, LayerLabeler};
use crate::geom::{Piece, arrange};
use crate::plan::connectors::{ConnectorGeometry, final_layers};
use crate::plan::model::{ConnectionKind, Plan};
use crate::plan::walls::Walls;
use crate::wad::classify::MapFormat;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::{Map, Sector};
use crate::wad::name::LumpName;
use lines::{connector_info, push_linedefs, room_sector_map};
use sectors::{connector_sector, room_sector};

/// The identity of the map [`emit`] produces: the block-header name, the
/// layout its rows will be written in, and the sky flat `outdoor` rooms
/// fall back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapIdentity {
    /// The `LumpName` of the emitted map block.
    pub name: LumpName,
    /// The layout class of the emitted map.
    pub format: MapFormat,
    /// The sky flat of the caller's game
    /// ([`crate::wad::game::sky_flat`]): `outdoor` rooms without a ceiling
    /// override wear this name, `F_SKY1` in the `Doom` family and `F_SKY`
    /// in `Hexen`.
    pub sky_flat: &'static str,
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
/// endpoint becomes one deduplicated
/// [`Vertex`](crate::wad::map::Vertex) and every piece becomes exactly one
/// [`Linedef`](crate::wad::map::Linedef), flipped so its front side faces its sector.
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
    let allocation = new_sectors(plan, connectors, &surviving, identity.sky_flat)?;
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
/// order first, then every connector's layers in allocator order. `sky`
/// is the ceiling default of an `outdoor` room without an override.
///
/// # Errors
///
/// [`emit`]'s `Internal` refusals: see the two add passes and
/// [`check_allocated`].
fn new_sectors(
    plan: &Plan,
    connectors: &[ConnectorGeometry],
    surviving: &BTreeSet<Label>,
    sky: &str,
) -> Result<Allocation, CyberError> {
    let mut allocation = Allocation::default();
    add_room_sectors(plan, surviving, sky, &mut allocation)?;
    add_connector_sectors(plan, connectors, surviving, &mut allocation)?;
    check_allocated(surviving, &allocation)?;
    Ok(allocation)
}

/// One sector per surviving room label, in plan order.
fn add_room_sectors(
    plan: &Plan,
    surviving: &BTreeSet<Label>,
    sky: &str,
    allocation: &mut Allocation,
) -> Result<(), CyberError> {
    for (index, room) in plan.rooms.iter().enumerate() {
        let label = region_label(index);
        if surviving.contains(&label) {
            allocation.take(label, room_sector(room, sky))?;
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

#[cfg(test)]
mod tests;
