//! The plan data model: rooms, connections, features, and structural
//! validation.
//!
//! A [`Plan`] is pure data: the room footprints and sector properties a
//! caller draws, the connections between them, the game features
//! ([`Feature`]) the wiring slices install, and the
//! scalar taste knobs (wall thickness, opening widths, sill and step
//! sizes). This module knows nothing about maps or archives: it only
//! checks that a plan is well-formed ([`Plan::validate`]) and turns a
//! [`Room`] into a polygon ring ([`Room::polygon`]) for the geometry
//! engine.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::geom::Pt;
use crate::wad::map::features::{LiftStyle, LightEffect};

/// Upper bound on [`Plan::rooms`]: region labels must fit `u32`
/// ([`Label::Region`](crate::geom::label::Label::Region)) with room to
/// spare for the wall and connector regions later slices add.
const MAX_ROOMS: usize = 4_096;

/// Upper bound on [`Plan::connections`], same `u32` label budget.
const MAX_CONNECTIONS: usize = 4_096;

/// Upper bound on [`Plan::features`], same `u32` label budget.
const MAX_FEATURES: usize = 4_096;

/// Default [`Plan::wall_thickness`].
const fn default_wall_thickness() -> i64 {
    16_i64
}

/// Default [`Room::floor`].
const fn default_floor() -> i32 {
    0_i32
}

/// Default [`Room::ceiling`].
const fn default_ceiling() -> i32 {
    128_i32
}

/// Default [`Room::light`].
const fn default_light() -> i32 {
    160_i32
}

/// Default [`Connection::width`].
const fn default_width() -> i64 {
    64_i64
}

/// Default [`ConnectionKind::Window::sill`].
const fn default_sill() -> i32 {
    32_i32
}

/// Default [`ConnectionKind::Window::lintel`].
const fn default_lintel() -> i32 {
    24_i32
}

/// Default [`ConnectionKind::Stairs::step_height`].
const fn default_step_height() -> i32 {
    16_i32
}

/// Default [`ConnectionKind::Stairs::step_depth`].
const fn default_step_depth() -> i32 {
    24_i32
}

/// Default [`Feature::Lift::style`].
const fn default_lift_style() -> LiftStyle {
    LiftStyle::UseWall
}

/// Default [`Feature::Lift::repeatable`].
const fn default_lift_repeatable() -> bool {
    true
}

/// A whole map plan: the rooms, the connections between them, and the
/// wall material in between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Plan {
    /// The rooms to lay out, in painter's order (bottom first).
    pub rooms: Vec<Room>,
    /// Openings, doors, windows and stairs joining the rooms.
    pub connections: Vec<Connection>,
    /// Game structures the follow-up wiring slices install once the map
    /// is compiled: exits, wall switches, lifts and teleporters. The
    /// compiler itself acts on none of them yet.
    #[serde(default)]
    pub features: Vec<Feature>,
    /// Thickness of the walls grown between rooms, map units; `0` makes
    /// the room polygons touch directly. Default `16`.
    #[serde(default = "default_wall_thickness")]
    pub wall_thickness: i64,
}

/// One room: a footprint ring plus the sector properties its interior
/// should carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Room {
    /// Name, unique in the plan, that connections refer to.
    pub id: String,
    /// The room's footprint.
    pub shape: Shape,
    /// Floor height of the room's sector. Default `0`.
    #[serde(default = "default_floor")]
    pub floor: i32,
    /// Ceiling height of the room's sector. Default `128`.
    #[serde(default = "default_ceiling")]
    pub ceiling: i32,
    /// Light level of the room's sector. Default `160`.
    #[serde(default = "default_light")]
    pub light: i32,
    /// Flat name for the floor, if any.
    pub floor_flat: Option<String>,
    /// Flat name for the ceiling, if any.
    pub ceiling_flat: Option<String>,
    /// Texture name for the walls, if any.
    pub wall_texture: Option<String>,
    /// Whether the room is open to the sky. Default `false`.
    #[serde(default)]
    pub outdoor: bool,
    /// Whether the room counts as a secret area. Default `false`; the
    /// follow-up wiring slices are the first to act on it.
    #[serde(default)]
    pub secret: bool,
    /// A light effect for the room's sector, if any; the follow-up
    /// wiring slices are the first to install it.
    #[serde(default)]
    pub light_effect: Option<LightEffect>,
}

/// The footprint of a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Axis-aligned rectangle from `(x, y)` to `(x + w, y + h)`.
    Rect {
        /// `X` of the bottom-left corner.
        x: i64,
        /// `Y` of the bottom-left corner.
        y: i64,
        /// Width; must be positive.
        w: i64,
        /// Height; must be positive.
        h: i64,
    },
    /// A closed ring of points, implicitly closing to the first.
    Polygon {
        /// The vertices in order; at least three distinct ones.
        points: Vec<Pt>,
    },
}

/// An intervention between two rooms: a hole in the wall between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Connection {
    /// Optional name used in messages and later tooling.
    pub id: Option<String>,
    /// Id of the room the connection starts from.
    pub from: String,
    /// Id of the room the connection leads to.
    pub to: String,
    /// What kind of opening it is.
    pub kind: ConnectionKind,
    /// Where to place the opening; `None` centres it on the shared wall.
    pub at: Option<(i64, i64)>,
    /// Clear width of the opening, map units. Default `64`.
    #[serde(default = "default_width")]
    pub width: i64,
}

/// How two rooms are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// A plain gap in the wall.
    Opening,
    /// A door with a linedef special.
    Door {
        /// Which door special.
        style: DoorStyle,
    },
    /// A window band in the wall.
    Window {
        /// Height of the glass bottom above the lower floor. Default `32`.
        #[serde(default = "default_sill")]
        sill: i32,
        /// Height of the glass top (the lintel). Default `24`.
        #[serde(default = "default_lintel")]
        lintel: i32,
    },
    /// A stepped floor transition.
    Stairs {
        /// Height of each step. Default `16`.
        #[serde(default = "default_step_height")]
        step_height: i32,
        /// Depth of each step, map units; must be positive. Default `24`.
        #[serde(default = "default_step_depth")]
        step_depth: i32,
    },
}

/// The door special behind [`ConnectionKind::Door`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoorStyle {
    /// Manual door: opens on use and closes again.
    Normal,
    /// Opens on touch and closes again.
    Fast,
    /// Opens on touch and stays open.
    StayOpen,
    /// Needs the named card to open.
    Locked(KeyColor),
    /// Opens by a remote switch.
    Remote,
}

/// A key card colour for [`DoorStyle::Locked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyColor {
    /// The blue card.
    Blue,
    /// The yellow card.
    Yellow,
    /// The red key.
    Red,
}

/// A game structure the plan carries and the wiring steps install.
///
/// [`apply_features`](crate::plan::features::apply_features) mounts the
/// `exit` and `switch` rows once the map is compiled;
/// [`compile`](fn@crate::plan::compile) itself validates every row and
/// draws no geometry for it, and the `lift` and `teleporter` kinds wait
/// for their slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Feature {
    /// The level's end trigger, standing in a room.
    Exit {
        /// Id of the room the exit stands in.
        room: String,
        /// How crossing or using it ends the level.
        style: ExitStyle,
        /// Whether the room is a secret area. Default `false`.
        #[serde(default)]
        secret: bool,
        /// Which wall of the room carries it, if any.
        wall: Option<Side>,
    },
    /// A wall switch in a room that operates a remote connection.
    Switch {
        /// Id of the room the switch is mounted in.
        room: String,
        /// Id of the connection (a remote door) this switch operates.
        opens: String,
        /// Which wall of the room carries it, if any.
        wall: Option<Side>,
        /// Whether the switch keeps working after use. Default `false`.
        #[serde(default)]
        repeatable: bool,
    },
    /// A lift platform lowering to the ground of a room.
    Lift {
        /// Id of the room the platform stands in.
        room: String,
        /// The floor the platform lowers to, map units; the lowest
        /// neighbouring floor by default.
        low_floor: Option<i32>,
        /// How the ride is triggered from below. Default
        /// [`LiftStyle::UseWall`].
        #[serde(default = "default_lift_style")]
        style: LiftStyle,
        /// Whether the ride keeps working after use. Default `true`.
        #[serde(default = "default_lift_repeatable")]
        repeatable: bool,
    },
    /// A teleporter pad in one room, its destination in another.
    Teleporter {
        /// Id of the room the pad stands in.
        from_room: String,
        /// Id of the room the destination stands in.
        to_room: String,
        /// Plan point for the pad's centre; the room centre by default.
        pad: Option<(i64, i64)>,
        /// Plan point for the destination's centre; the room centre by
        /// default.
        destination: Option<(i64, i64)>,
        /// Facing of the arrival, degrees. Default `0`.
        #[serde(default)]
        angle: i32,
        /// Whether the return leg teleports back. Default `false`.
        #[serde(default)]
        two_way: bool,
    },
}

/// A compass side of a room, naming the wall a feature is mounted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The wall facing `+y`.
    North,
    /// The wall facing `-y`.
    South,
    /// The wall facing `+x`.
    East,
    /// The wall facing `-x`.
    West,
}

/// How a [`Feature::Exit`] ends the level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitStyle {
    /// A use switch: pressing the exit wall ends the level.
    Switch,
    /// Walking over the exit line ends the level.
    WalkOver,
}

impl Shape {
    /// The corner ring of a `width` x `height` rectangle based at
    /// `(left, bottom)`, counter-clockwise starting at that corner. Sums
    /// saturate at the `i64` extremes so the ring is always produced;
    /// [`Plan::validate`] rejects the bad sizes before anything is
    /// compiled.
    #[must_use]
    #[inline]
    pub fn rect_points(left: i64, bottom: i64, width: i64, height: i64) -> Vec<Pt> {
        let right = left.saturating_add(width);
        let top = bottom.saturating_add(height);
        vec![(left, bottom), (right, bottom), (right, top), (left, top)]
    }
}

impl Room {
    /// The room's outline as a [`geom`](crate::geom) polygon ring: the
    /// four corners of a `Rect` counter-clockwise from `(x, y)`, or a
    /// `Polygon`'s points exactly as given.
    #[must_use]
    #[inline]
    pub fn polygon(&self) -> Vec<Pt> {
        match self.shape.clone() {
            Shape::Rect {
                x: left,
                y: bottom,
                w: width,
                h: height,
            } => Shape::rect_points(left, bottom, width, height),
            Shape::Polygon { points } => points,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A room with default sector properties and the given shape.
    fn room_with(id: &str, shape: Shape) -> Room {
        Room {
            id: id.to_owned(),
            shape,
            floor: default_floor(),
            ceiling: default_ceiling(),
            light: default_light(),
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A rectangular room at the origin, `64` x `64`.
    fn square_room(id: &str) -> Room {
        room_with(
            id,
            Shape::Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            },
        )
    }

    /// A default-width opening between two rooms.
    fn opening(from: &str, to: &str) -> Connection {
        Connection {
            id: None,
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width: default_width(),
        }
    }

    /// A plan over the given rooms and connections.
    fn plan(rooms: Vec<Room>, connections: Vec<Connection>) -> Plan {
        Plan {
            rooms,
            connections,
            features: Vec::new(),
            wall_thickness: default_wall_thickness(),
        }
    }

    /// A plan over the given rooms, connections and features.
    fn plan_with_features(
        rooms: Vec<Room>,
        connections: Vec<Connection>,
        features: Vec<Feature>,
    ) -> Plan {
        Plan {
            rooms,
            connections,
            features,
            wall_thickness: default_wall_thickness(),
        }
    }

    /// A plain exit in the named room.
    fn exit_in(room: &str) -> Feature {
        Feature::Exit {
            room: room.to_owned(),
            style: ExitStyle::Switch,
            secret: false,
            wall: None,
        }
    }

    /// A switch in the named room opening the named connection.
    fn switch_in(room: &str, opens: &str) -> Feature {
        Feature::Switch {
            room: room.to_owned(),
            opens: opens.to_owned(),
            wall: None,
            repeatable: false,
        }
    }

    /// A plain lift in the named room.
    fn lift_in(room: &str) -> Feature {
        Feature::Lift {
            room: room.to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
        }
    }

    /// A plain teleporter from `from` to `to`.
    fn teleporter(from: &str, to: &str) -> Feature {
        Feature::Teleporter {
            from_room: from.to_owned(),
            to_room: to.to_owned(),
            pad: None,
            destination: None,
            angle: 0_i32,
            two_way: false,
        }
    }

    /// Asserts `plan` is rejected with `invalid_argument` and a message
    /// mentioning `needle`.
    #[track_caller]
    fn assert_invalid(plan: &Plan, needle: &str) {
        let error = plan.validate().expect_err("the plan must be rejected");
        assert_eq!(error.code(), "invalid_argument", "code for: {error}");
        assert!(
            error.to_string().contains(needle),
            "message '{error}' must mention '{needle}'"
        );
    }

    #[test]
    fn a_full_plan_round_trips_through_json() {
        let text = r#"{
            "rooms": [
                {
                    "id": "courtyard",
                    "shape": {"polygon": {"points": [[0, 0], [128, 0], [128, 128], [0, 128]]}},
                    "ceiling": 512,
                    "light": 192,
                    "ceiling_flat": "F_SKY1",
                    "outdoor": true
                },
                {
                    "id": "lab",
                    "shape": {"rect": {"x": 144, "y": 0, "w": 128, "h": 128}},
                    "floor": 8,
                    "floor_flat": "FLAT10",
                    "wall_texture": "STONE6"
                }
            ],
            "connections": [
                {"from": "courtyard", "to": "lab", "kind": "opening"},
                {"id": "d1", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "normal"}}, "at": [160, 64], "width": 48},
                {"id": "d2", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "fast"}}},
                {"id": "d3", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "stay_open"}}},
                {"id": "d4", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "red"}}}},
                {"id": "d5", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "blue"}}}},
                {"id": "d6", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "yellow"}}}},
                {"id": "d7", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "remote"}}},
                {"id": "w1", "from": "courtyard", "to": "lab", "kind": {"window": {"sill": 48, "lintel": 80}}},
                {"id": "w2", "from": "courtyard", "to": "lab", "kind": {"window": {}}},
                {"id": "s1", "from": "lab", "to": "courtyard", "kind": {"stairs": {"step_height": 8, "step_depth": 16}}, "at": [0, 0]},
                {"id": "s2", "from": "lab", "to": "courtyard", "kind": {"stairs": {}}}
            ],
            "features": [
                {"kind": "exit", "room": "courtyard", "style": "walk_over", "secret": true, "wall": "north"},
                {"kind": "switch", "room": "lab", "opens": "d7", "wall": "west", "repeatable": true},
                {"kind": "lift", "room": "lab", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
                {"kind": "teleporter", "from_room": "courtyard", "to_room": "lab", "pad": [64, 64], "destination": [200, 64], "angle": 90, "two_way": true}
            ],
            "wall_thickness": 24
        }"#;
        let parsed: Plan = serde_json::from_str(text).expect("the documented JSON layout");
        assert_eq!(parsed.wall_thickness, 24_i64, "explicit wall thickness");
        let courtyard = parsed.rooms.first().expect("first room");
        assert_eq!(courtyard.floor, 0_i32, "default floor");
        assert_eq!(courtyard.ceiling, 512_i32, "explicit ceiling");
        assert!(courtyard.outdoor, "explicit outdoor");
        assert_eq!(courtyard.ceiling_flat.as_deref(), Some("F_SKY1"), "flat");
        let written = serde_json::to_string(&parsed).expect("serialising a plan");
        let again: Plan = serde_json::from_str(&written).expect("re-reading the plan");
        assert_eq!(parsed, again, "round trip must preserve the plan");
    }

    #[test]
    fn feature_rows_keep_their_tagged_wire_names() {
        let text = r#"{
            "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
            "connections": [],
            "features": [
                {"kind": "exit", "room": "a", "style": "walk_over", "secret": true, "wall": "north"},
                {"kind": "switch", "room": "a", "opens": "d1", "wall": "west", "repeatable": true},
                {"kind": "lift", "room": "a", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
                {"kind": "teleporter", "from_room": "a", "to_room": "a", "pad": [64, 64], "destination": [200, 64], "angle": 90, "two_way": true}
            ]
        }"#;
        let parsed: Plan = serde_json::from_str(text).expect("the tagged feature rows");
        assert_eq!(
            parsed.features.first().expect("four features"),
            &Feature::Exit {
                room: "a".to_owned(),
                style: ExitStyle::WalkOver,
                secret: true,
                wall: Some(Side::North),
            },
            "the exit row is tagged by kind"
        );
        assert_eq!(
            parsed.features.get(1).expect("four features"),
            &Feature::Switch {
                room: "a".to_owned(),
                opens: "d1".to_owned(),
                wall: Some(Side::West),
                repeatable: true,
            },
            "the switch row"
        );
        assert_eq!(
            parsed.features.get(2).expect("four features"),
            &Feature::Lift {
                room: "a".to_owned(),
                low_floor: Some(-64_i32),
                style: LiftStyle::WalkTrigger,
                repeatable: false,
            },
            "the lift row"
        );
        assert_eq!(
            parsed.features.get(3).expect("four features"),
            &Feature::Teleporter {
                from_room: "a".to_owned(),
                to_room: "a".to_owned(),
                pad: Some((64_i64, 64_i64)),
                destination: Some((200_i64, 64_i64)),
                angle: 90_i32,
                two_way: true,
            },
            "the teleporter row keeps its squares"
        );
        let written = serde_json::to_string(&parsed).expect("serialising features");
        let again: Plan = serde_json::from_str(&written).expect("re-reading the features");
        assert_eq!(parsed, again, "the tagged rows round trip");
    }

    #[test]
    fn omitted_feature_fields_materialise_their_defaults() {
        let text = r#"{
            "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
            "connections": [],
            "features": [
                {"kind": "exit", "room": "a", "style": "switch"},
                {"kind": "switch", "room": "a", "opens": "d1"},
                {"kind": "lift", "room": "a"},
                {"kind": "teleporter", "from_room": "a", "to_room": "a"}
            ]
        }"#;
        let parsed: Plan = serde_json::from_str(text).expect("a plan with minimal features");
        assert_eq!(
            parsed.features.first().expect("four features"),
            &Feature::Exit {
                room: "a".to_owned(),
                style: ExitStyle::Switch,
                secret: false,
                wall: None,
            },
            "default exit secret and wall"
        );
        assert_eq!(
            parsed.features.get(1).expect("four features"),
            &Feature::Switch {
                room: "a".to_owned(),
                opens: "d1".to_owned(),
                wall: None,
                repeatable: false,
            },
            "default switch repeatable and wall"
        );
        assert_eq!(
            parsed.features.get(2).expect("four features"),
            &Feature::Lift {
                room: "a".to_owned(),
                low_floor: None,
                style: LiftStyle::UseWall,
                repeatable: true,
            },
            "default lift style and repeatable"
        );
        assert_eq!(
            parsed.features.get(3).expect("four features"),
            &Feature::Teleporter {
                from_room: "a".to_owned(),
                to_room: "a".to_owned(),
                pad: None,
                destination: None,
                angle: 0_i32,
                two_way: false,
            },
            "default teleporter squares, angle and return leg"
        );
    }

    #[test]
    fn omitted_fields_materialise_their_defaults() {
        let text = r#"{
            "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
            "connections": [
                {"from": "a", "to": "a", "kind": {"window": {}}},
                {"from": "a", "to": "a", "kind": {"stairs": {}}}
            ]
        }"#;
        let parsed: Plan = serde_json::from_str(text).expect("a minimal plan");
        assert_eq!(parsed.wall_thickness, 16_i64, "default wall thickness");
        let room = parsed.rooms.first().expect("one room");
        assert_eq!(room.floor, 0_i32, "default floor");
        assert_eq!(room.ceiling, 128_i32, "default ceiling");
        assert_eq!(room.light, 160_i32, "default light");
        assert!(!room.outdoor, "default outdoor");
        assert!(!room.secret, "default secret");
        assert_eq!(room.light_effect, None, "default light effect");
        assert_eq!(room.floor_flat, None, "default floor flat");
        assert_eq!(room.ceiling_flat, None, "default ceiling flat");
        assert_eq!(room.wall_texture, None, "default wall texture");
        assert!(parsed.features.is_empty(), "default feature list");
        let window = parsed.connections.first().expect("first connection");
        assert_eq!(window.width, 64_i64, "default connection width");
        assert_eq!(window.id, None, "default connection id");
        assert_eq!(window.at, None, "default placement");
        assert_eq!(
            window.kind,
            ConnectionKind::Window {
                sill: 32_i32,
                lintel: 24_i32
            },
            "default sill and lintel"
        );
        let stairs = parsed.connections.get(1).expect("second connection");
        assert_eq!(
            stairs.kind,
            ConnectionKind::Stairs {
                step_height: 16_i32,
                step_depth: 24_i32
            },
            "default step height and depth"
        );
    }

    #[test]
    fn a_rooms_light_effect_names_its_snake_case_word() {
        let text = r#"{
            "rooms": [{
                "id": "a",
                "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}},
                "secret": true,
                "light_effect": "strobe_slow"
            }],
            "connections": []
        }"#;
        let parsed: Plan = serde_json::from_str(text).expect("a lit room");
        let room = parsed.rooms.first().expect("one room");
        assert!(room.secret, "explicit secret");
        assert_eq!(room.light_effect, Some(LightEffect::StrobeSlow));
        let written = serde_json::to_string(room).expect("serialising a room");
        assert!(written.contains("\"strobe_slow\""), "{written}");
    }

    #[test]
    fn rect_polygon_is_counter_clockwise_from_the_base_corner() {
        let room = room_with(
            "r",
            Shape::Rect {
                x: 10,
                y: 20,
                w: 64,
                h: 32,
            },
        );
        assert_eq!(
            room.polygon(),
            vec![(10, 20), (74, 20), (74, 52), (10, 52)],
            "counter-clockwise corners from (x, y)"
        );
    }

    #[test]
    fn polygon_room_returns_the_points_as_given() {
        let points = vec![(0_i64, 0_i64), (128, 0), (96, 128), (0, 96)];
        let room = room_with(
            "r",
            Shape::Polygon {
                points: points.clone(),
            },
        );
        assert_eq!(room.polygon(), points, "the ring passes through");
    }

    #[test]
    fn an_empty_plan_is_refused() {
        let plan = plan(vec![], vec![]);
        assert_invalid(&plan, "rooms");
    }

    #[test]
    fn duplicate_room_ids_are_refused() {
        let plan = plan(
            vec![square_room("a"), square_room("a"), square_room("b")],
            vec![],
        );
        assert_invalid(&plan, "room id 'a'");
    }

    #[test]
    fn degenerate_rectangles_are_refused() {
        let flat = room_with(
            "cellar",
            Shape::Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 0,
            },
        );
        let plan = plan(vec![square_room("a"), flat], vec![]);
        assert_invalid(&plan, "cellar");
    }

    #[test]
    fn polygons_with_fewer_than_three_distinct_points_are_refused() {
        let bent = Shape::Polygon {
            points: vec![(0_i64, 0_i64), (64, 0), (0, 0)],
        };
        let plan = plan(vec![room_with("attic", bent)], vec![]);
        assert_invalid(&plan, "attic");
    }

    #[test]
    fn connections_must_reference_existing_rooms() {
        let unknown_target = plan(vec![square_room("a")], vec![opening("a", "ghost")]);
        assert_invalid(&unknown_target, "'ghost'");
        let unknown_source = plan(vec![square_room("a")], vec![opening("ghost", "a")]);
        assert_invalid(&unknown_source, "'ghost'");
    }

    #[test]
    fn self_connections_are_refused() {
        let plan = plan(vec![square_room("a")], vec![opening("a", "a")]);
        assert_invalid(&plan, "room 'a' to itself");
    }

    #[test]
    fn non_positive_widths_are_refused() {
        let mut narrow = opening("a", "b");
        narrow.id = Some("c1".to_owned());
        narrow.width = 0_i64;
        let plan = plan(vec![square_room("a"), square_room("b")], vec![narrow]);
        assert_invalid(&plan, "c1");
    }

    #[test]
    fn negative_wall_thickness_is_refused() {
        let mut plan = plan(vec![square_room("a")], vec![]);
        plan.wall_thickness = -1_i64;
        assert_invalid(&plan, "thickness");
    }

    #[test]
    fn stairs_must_have_a_positive_step_depth() {
        let mut steep = opening("a", "b");
        steep.id = Some("s1".to_owned());
        steep.kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 0_i32,
        };
        let plan = plan(vec![square_room("a"), square_room("b")], vec![steep]);
        assert_invalid(&plan, "s1");
    }

    #[test]
    fn every_feature_kind_refuses_an_unknown_room_by_name() {
        let rooms = vec![square_room("a"), square_room("b")];
        for (feature, kind) in [
            (exit_in("ghost"), "exit"),
            (switch_in("ghost", "d1"), "switch"),
            (lift_in("ghost"), "lift"),
            (teleporter("ghost", "a"), "teleporter"),
            (teleporter("a", "ghost"), "teleporter"),
        ] {
            let plan = plan_with_features(rooms.clone(), vec![], vec![feature]);
            assert_invalid(&plan, &format!("{kind} feature names unknown room 'ghost'"));
        }
    }

    #[test]
    fn a_switch_must_open_a_connection_by_id() {
        let mut named = opening("a", "b");
        named.id = Some("d1".to_owned());
        let rooms = vec![square_room("a"), square_room("b")];
        let good = plan_with_features(
            rooms.clone(),
            vec![named.clone()],
            vec![switch_in("a", "d1")],
        );
        good.validate().expect("the switch names the connection");
        let unknown = plan_with_features(rooms, vec![named], vec![switch_in("a", "nope")]);
        assert_invalid(&unknown, "opens unknown connection 'nope'");
    }

    #[test]
    fn a_switch_opening_an_unnamed_connection_is_refused_by_its_label() {
        let plan = plan_with_features(
            vec![square_room("a"), square_room("b")],
            vec![opening("a", "b")],
            vec![switch_in("a", "a->b")],
        );
        assert_invalid(&plan, "opens connection 'a->b', which carries no id");
    }

    #[test]
    fn a_feature_bearing_plan_validates() {
        let rooms = vec![square_room("a"), square_room("b")];
        let mut named = opening("a", "b");
        named.id = Some("d1".to_owned());
        let plan = plan_with_features(
            rooms,
            vec![named],
            vec![
                exit_in("b"),
                switch_in("a", "d1"),
                lift_in("b"),
                teleporter("a", "b"),
            ],
        );
        plan.validate()
            .expect("features are reference-checked only");
    }

    #[test]
    fn plans_past_the_room_budget_are_refused() {
        let rooms = (0_i64..4_097)
            .map(|index| square_room(&format!("room-{index}")))
            .collect();
        let plan = plan(rooms, vec![]);
        let error = plan.validate().expect_err("4097 rooms must be refused");
        assert_eq!(error.code(), "limit_exceeded", "room budget code");
        assert!(
            error.to_string().contains("4097"),
            "message names the count: {error}"
        );
    }

    #[test]
    fn plans_past_the_connection_budget_are_refused() {
        let connections = (0_i64..4_097).map(|_| opening("a", "b")).collect();
        let plan = plan(vec![square_room("a"), square_room("b")], connections);
        let error = plan
            .validate()
            .expect_err("4097 connections must be refused");
        assert_eq!(error.code(), "limit_exceeded", "connection budget code");
        assert!(
            error.to_string().contains("4097"),
            "message names the count: {error}"
        );
    }

    #[test]
    fn plans_past_the_feature_budget_are_refused() {
        let features = (0_i64..4_097).map(|_| exit_in("a")).collect();
        let plan = plan_with_features(vec![square_room("a")], vec![], features);
        let error = plan.validate().expect_err("4097 features must be refused");
        assert_eq!(error.code(), "limit_exceeded", "feature budget code");
        assert!(
            error.to_string().contains("4097"),
            "message names the count: {error}"
        );
    }

    #[test]
    fn a_happy_plan_validates() {
        let courtyard = room_with(
            "courtyard",
            Shape::Polygon {
                points: vec![(0_i64, 0_i64), (256, 0), (256, 256), (0, 256)],
            },
        );
        let lab = square_room("lab");
        let mut door = opening("courtyard", "lab");
        door.id = Some("d1".to_owned());
        door.kind = ConnectionKind::Door {
            style: DoorStyle::Locked(KeyColor::Blue),
        };
        let mut stairs = opening("lab", "courtyard");
        stairs.kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32,
        };
        let mut plan = plan(vec![courtyard, lab], vec![door, stairs]);
        plan.wall_thickness = 0_i64;
        plan.features = vec![
            exit_in("courtyard"),
            switch_in("lab", "d1"),
            lift_in("lab"),
            teleporter("courtyard", "lab"),
        ];
        plan.validate().expect("the plan is well-formed");
    }
}
