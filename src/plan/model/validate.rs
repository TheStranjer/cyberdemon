//! Structural validation of a [`Plan`]: the [`Plan::validate`] pass and
//! the size, thickness, room, connection and feature checks it runs one
//! thing at a time, refusing with the first failure that names the
//! offending id.

use std::collections::HashSet;

use super::{
    Connection, ConnectionKind, Feature, MAX_CONNECTIONS, MAX_FEATURES, MAX_ROOMS, Plan, Room,
    Shape,
};
use crate::error::CyberError;
use crate::geom::Pt;

impl Plan {
    /// Checks the plan's structure, one thing at a time, and returns the
    /// first failure naming the offending id: at least one room, counts
    /// within the `u32` region-label budget, unique room ids, usable
    /// shapes, a non-negative `wall_thickness`, connections joining two
    /// distinct rooms with a positive width and, for stairs, a positive
    /// step depth, and features naming rooms that exist and, for
    /// switches, opening a connection that carries an `id`. Room
    /// `ceiling > floor` is *not* checked here; rooms are drawn in
    /// painter's order and may stack.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the offending id for the
    /// first structural problem found, or
    /// [`CyberError::LimitExceeded`] past the room, connection and
    /// feature budgets.
    #[inline]
    pub fn validate(&self) -> Result<(), CyberError> {
        self.validate_size()?;
        self.validate_thickness()?;
        let ids = self.validate_rooms()?;
        self.validate_connections(&ids)?;
        self.validate_features(&ids)
    }

    /// Refuses an empty plan and plans whose counts would push the
    /// region labels past `u32`.
    fn validate_size(&self) -> Result<(), CyberError> {
        if self.rooms.is_empty() {
            return Err(CyberError::InvalidArgument(
                "plan has no rooms; add at least one room".to_owned(),
            ));
        }
        let rooms = self.rooms.len();
        if rooms > MAX_ROOMS {
            return Err(CyberError::LimitExceeded(format!(
                "plan has {rooms} rooms; at most {MAX_ROOMS} are supported"
            )));
        }
        let connections = self.connections.len();
        if connections > MAX_CONNECTIONS {
            return Err(CyberError::LimitExceeded(format!(
                "plan has {connections} connections; at most {MAX_CONNECTIONS} are supported"
            )));
        }
        let features = self.features.len();
        if features > MAX_FEATURES {
            return Err(CyberError::LimitExceeded(format!(
                "plan has {features} features; at most {MAX_FEATURES} are supported"
            )));
        }
        Ok(())
    }

    /// Refuses a negative wall thickness; `0` is allowed.
    fn validate_thickness(&self) -> Result<(), CyberError> {
        let thickness = self.wall_thickness;
        if thickness < 0_i64 {
            return Err(CyberError::InvalidArgument(format!(
                "wall thickness {thickness} is negative; use 0 for rooms that touch"
            )));
        }
        Ok(())
    }

    /// Checks room ids are unique and every room's shape is usable, and
    /// returns the id set for the connection pass.
    fn validate_rooms(&self) -> Result<HashSet<&str>, CyberError> {
        let mut ids: HashSet<&str> = HashSet::with_capacity(self.rooms.len());
        for room in &self.rooms {
            collect_room_id(&mut ids, room)?;
        }
        Ok(ids)
    }

    /// Checks every connection against the known room ids.
    fn validate_connections(&self, ids: &HashSet<&str>) -> Result<(), CyberError> {
        for connection in &self.connections {
            validate_connection(connection, ids)?;
        }
        Ok(())
    }

    /// Checks every feature: the rooms it names exist and, for a
    /// switch, its `opens` names a connection that carries an `id`.
    fn validate_features(&self, ids: &HashSet<&str>) -> Result<(), CyberError> {
        for feature in &self.features {
            validate_feature(feature, ids, &self.connections)?;
        }
        Ok(())
    }
}

/// Registers one room: its id must be new, its shape usable.
fn collect_room_id<'plan>(
    ids: &mut HashSet<&'plan str>,
    room: &'plan Room,
) -> Result<(), CyberError> {
    let id = room.id.as_str();
    if !ids.insert(id) {
        return Err(CyberError::InvalidArgument(format!(
            "room id '{id}' is used by more than one room"
        )));
    }
    validate_shape(id, &room.shape)
}

/// Checks one room's shape: rectangles need positive edges, polygons at
/// least three distinct points. Full ring validity is
/// [`arrange`](crate::geom::arrange)'s job, whose errors already name
/// the layer.
fn validate_shape(id: &str, shape: &Shape) -> Result<(), CyberError> {
    match shape.clone() {
        Shape::Rect { w, h, .. } => validate_rect(id, w, h),
        Shape::Polygon { points } => validate_polygon(id, &points),
    }
}

/// A rectangle room must have positive edges.
fn validate_rect(id: &str, width: i64, height: i64) -> Result<(), CyberError> {
    let ok = width > 0_i64 && height > 0_i64;
    if ok {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "room '{id}' has a {width}x{height} rectangle; both edges must be positive"
    )))
}

/// A polygon room must have at least three distinct points.
fn validate_polygon(id: &str, points: &[Pt]) -> Result<(), CyberError> {
    let distinct = points.iter().collect::<HashSet<_>>().len();
    if distinct >= 3 {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "room '{id}' has a polygon with {distinct} distinct points; at least 3 are required"
    )))
}

/// Checks one connection: both endpoints exist and differ, the width is
/// positive, and stairs have a positive step depth.
fn validate_connection(connection: &Connection, ids: &HashSet<&str>) -> Result<(), CyberError> {
    let from = connection.from.as_str();
    let to = connection.to.as_str();
    let label = connection
        .id
        .clone()
        .unwrap_or_else(|| derived_label(connection));
    if !ids.contains(from) {
        return Err(CyberError::InvalidArgument(format!(
            "connection '{label}' starts from unknown room '{from}'"
        )));
    }
    if !ids.contains(to) {
        return Err(CyberError::InvalidArgument(format!(
            "connection '{label}' leads to unknown room '{to}'"
        )));
    }
    if from == to {
        return Err(CyberError::InvalidArgument(format!(
            "connection '{label}' joins room '{from}' to itself"
        )));
    }
    let width = connection.width;
    if width <= 0_i64 {
        return Err(CyberError::InvalidArgument(format!(
            "connection '{label}' has width {width}; it must be positive"
        )));
    }
    validate_kind(connection.kind, &label)
}

/// Kind-specific checks: stairs need a positive step depth.
fn validate_kind(kind: ConnectionKind, label: &str) -> Result<(), CyberError> {
    match kind {
        ConnectionKind::Stairs { step_depth, .. } => {
            if step_depth > 0_i32 {
                Ok(())
            } else {
                Err(CyberError::InvalidArgument(format!(
                    "connection '{label}' has stairs with step depth {step_depth}; it must be positive"
                )))
            }
        }
        ConnectionKind::Opening | ConnectionKind::Door { .. } | ConnectionKind::Window { .. } => {
            Ok(())
        }
    }
}

/// The label a connection answers to when it carries no `id`:
/// `from->to`.
fn derived_label(connection: &Connection) -> String {
    format!("{}->{}", connection.from, connection.to)
}

/// Checks one feature: every room it names exists and, for a switch,
/// its `opens` names a connection that carries an `id`. Whether that
/// connection is a *remote* door needs compilation and is a later
/// slice's check.
fn validate_feature(
    feature: &Feature,
    ids: &HashSet<&str>,
    connections: &[Connection],
) -> Result<(), CyberError> {
    match feature.clone() {
        Feature::Exit { room, .. } => validate_feature_room("exit", &room, ids),
        Feature::Switch { room, opens, .. } => {
            validate_feature_room("switch", &room, ids)?;
            validate_opens(&opens, connections)
        }
        Feature::Lift { room, .. } => validate_feature_room("lift", &room, ids),
        Feature::Teleporter {
            from_room, to_room, ..
        } => {
            validate_feature_room("teleporter", &from_room, ids)?;
            validate_feature_room("teleporter", &to_room, ids)
        }
    }
}

/// One feature's room reference must name a room of the plan.
fn validate_feature_room(kind: &str, room: &str, ids: &HashSet<&str>) -> Result<(), CyberError> {
    if ids.contains(room) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{kind} feature names unknown room '{room}'; point it at a room of the plan"
    )))
}

/// A switch's `opens` must name a connection by its `id`; an unnamed
/// connection cannot be referred to, and is refused by its `from->to`
/// label.
fn validate_opens(opens: &str, connections: &[Connection]) -> Result<(), CyberError> {
    let named = connections
        .iter()
        .any(|connection| connection.id.as_deref() == Some(opens));
    if named {
        return Ok(());
    }
    if let Some(connection) = connections
        .iter()
        .find(|connection| connection.id.is_none() && derived_label(connection) == opens)
    {
        return Err(CyberError::InvalidArgument(format!(
            "switch feature opens connection '{label}', which carries no id; give the \
             connection an id and name it in opens",
            label = derived_label(connection)
        )));
    }
    Err(CyberError::InvalidArgument(format!(
        "switch feature opens unknown connection '{opens}'; pass the id of a connection \
         of the plan"
    )))
}
