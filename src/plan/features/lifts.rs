//! The lift step of the pipeline: dress every `lift` row's room sector
//! as a platform, in plan order.
//!
//! The step runs the [`build_lift`] core on each row's room sector —
//! refused unless every connection from the lift room to a room with a
//! lower floor is a plain opening — stamping a fresh tag, turning the
//! platform's lower-facing edges onto the players below with the
//! family's down-wait-up action and dressing those faces in the theme's
//! `step` texture.
//!
//! [`build_lift`]: crate::wad::map::features::build_lift

use crate::error::CyberError;
use crate::plan::emit::CompiledPlan;
use crate::plan::model::{Connection, ConnectionKind, Feature, Plan};
use crate::wad::map::features::{LiftPlatform, LiftSpec, build_lift};

use super::{FeatureDress, FeatureRow, FeatureRun, NO_TEXTURE, invalid, room_sectors_of};

/// The lift step: dress every `lift` row's room sector as a platform,
/// in plan order.
pub(super) fn lifts(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in &plan.features {
        if !matches!(feature, Feature::Lift { .. }) {
            continue;
        }
        // A `&Feature` cannot be destructured with bindings under this
        // crate's denied `pattern_type_mismatch` and `ref_patterns`, so
        // only the matching row is cloned, never the whole vector.
        let Feature::Lift {
            room,
            low_floor,
            style,
            repeatable,
        } = feature.clone()
        else {
            continue;
        };
        let sectors = room_sectors_of(compiled, &room)?;
        lift_join_rule(plan, &room)?;
        let platform = single_platform_sector(&sectors, &room)?;
        let spec = LiftSpec {
            platform: LiftPlatform::Existing(platform),
            low_floor,
            style,
            repeatable,
            side: lift_side(dress),
            floor_flat: NO_TEXTURE.to_owned(),
        };
        let built = build_lift(&mut compiled.map, &spec)?;
        compiled.warnings.extend(built.notes.iter().cloned());
        run.rows.push(FeatureRow {
            kind: "lift",
            room: Some(room),
            sectors: vec![built.platform_sector],
            lines: built.lines,
            tag: Some(built.tag),
            line_ends: Vec::new(),
        });
    }
    Ok(())
}

/// The single sector of room `room`, which the plan lift dresses as
/// its whole platform: emission gives every plan room exactly one
/// sector today, and a room that ever owns several is refused by name
/// rather than silently lifting one of them.
fn single_platform_sector(sectors: &[usize], room: &str) -> Result<usize, CyberError> {
    match sectors {
        &[only] => Ok(only),
        many => Err(invalid(&format!(
            "lift room '{room}' owns {} sectors: the plan lift dresses a whole room as \
             one platform; give the room a single sector, or lift one of them directly \
             with add_lift",
            many.len()
        ))),
    }
}

/// The lift room's geometry rule: every connection from `room` to a
/// room whose floor is lower must be a plain opening, or the ride down
/// would dead-end in a closed band, a glass sill or a step no trigger
/// reaches.
pub(super) fn lift_join_rule(plan: &Plan, room: &str) -> Result<(), CyberError> {
    let Some(lift) = plan.rooms.iter().find(|entry| entry.id == room) else {
        return Ok(());
    };
    for connection in &plan.connections {
        let Some(neighbour) = joined_room(connection, room) else {
            continue;
        };
        if !joins_lower_room(plan, neighbour, lift.floor) {
            continue;
        }
        if !matches!(connection.kind, ConnectionKind::Opening) {
            return Err(lift_join_refusal(room, neighbour, connection));
        }
    }
    Ok(())
}

/// The room across `connection` from `room`, or `None` when the
/// connection does not touch `room`.
fn joined_room<'conn>(connection: &'conn Connection, room: &str) -> Option<&'conn str> {
    if connection.from == room {
        Some(connection.to.as_str())
    } else if connection.to == room {
        Some(connection.from.as_str())
    } else {
        None
    }
}

/// Whether room `neighbour` sits below `floor`.
fn joins_lower_room(plan: &Plan, neighbour: &str, floor: i32) -> bool {
    plan.rooms
        .iter()
        .find(|entry| entry.id == neighbour)
        .is_some_and(|entry| entry.floor < floor)
}

/// The refusal for a lift room joined to a lower room by anything but
/// an opening.
fn lift_join_refusal(room: &str, neighbour: &str, connection: &Connection) -> CyberError {
    invalid(&format!(
        "lift room '{room}' joins lower room '{neighbour}' through connection '{}' of \
         kind {}: a lift needs openings to the lower rooms; make the connection an \
         opening, raise the lower room's floor, or move the lift",
        connection_name(connection),
        connection_kind_name(connection.kind),
    ))
}

/// The label `connection` answers to in messages: its `id`, or
/// `from->to` when unnamed.
pub(super) fn connection_name(connection: &Connection) -> String {
    connection
        .id
        .clone()
        .unwrap_or_else(|| format!("{}->{}", connection.from, connection.to))
}

/// The wire class of `connection`'s kind.
const fn connection_kind_name(kind: ConnectionKind) -> &'static str {
    match kind {
        ConnectionKind::Opening => "opening",
        ConnectionKind::Door { .. } => "door",
        ConnectionKind::Window { .. } => "window",
        ConnectionKind::Stairs { .. } => "stairs",
    }
}

/// The texture a plan lift's platform side wears: the theme's `step`
/// name, else the `wall` name the compiler's fill pass already dressed
/// the room's height gaps in, else `"-"` (no theme answer at all).
pub(super) fn lift_side(dress: &FeatureDress) -> String {
    dress
        .texture("step")
        .or_else(|| dress.texture("wall"))
        .unwrap_or(NO_TEXTURE)
        .to_owned()
}
