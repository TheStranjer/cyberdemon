//! The [`Sector`](crate::wad::map::Sector) rows the emission allocates:
//! one per surviving room with its heights, light and flat overrides,
//! and one per surviving connector layer, opened to its stored span,
//! closed for a door or rising under the higher room's ceiling for a
//! stair step. The room-pair, height-span and error builders the
//! connector sector reads its metadata through live here too.

use alloc::collections::BTreeMap;

use super::{NO_TEXTURE, SKY_FLAT};
use crate::error::CyberError;
use crate::plan::connectors::ConnectorGeometry;
use crate::plan::model::{ConnectionKind, Plan, Room};
use crate::wad::map::Sector;

/// The sector row for a room: its heights and light, its flat overrides
/// or the convention default (`outdoor` rooms fall back to the sky), and
/// no special or tag yet.
pub(super) fn room_sector(room: &Room) -> Sector {
    let ceiling_default = if room.outdoor { SKY_FLAT } else { NO_TEXTURE };
    Sector {
        floor_height: room.floor,
        ceiling_height: room.ceiling,
        floor_flat: flat_name(room.floor_flat.as_deref(), NO_TEXTURE),
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
/// [`emit`](super::emit)'s `Internal` refusals for missing spans or foreign rooms.
pub(super) fn connector_sector(
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
        floor_flat: NO_TEXTURE.to_owned(),
        ceiling_flat: NO_TEXTURE.to_owned(),
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
