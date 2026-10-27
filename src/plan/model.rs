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

use serde::{Deserialize, Serialize};

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

/// Default [`ConnectionKind::Window::lintel`]: the shared
/// [`DEFAULT_LINTEL`](crate::wad::map::consts::DEFAULT_LINTEL).
const fn default_lintel() -> i32 {
    crate::wad::map::consts::DEFAULT_LINTEL
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
    /// Game structures the feature run installs once the map is
    /// compiled: exits, wall switches, lifts and teleporters. The
    /// compiler itself acts on none of them.
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
    /// feature run's secret step (always after the lights step) flags
    /// the room's sectors with the family's secret special.
    #[serde(default)]
    pub secret: bool,
    /// A light effect for the room's sector, if any; the feature run's
    /// lights step installs it on every sector of the room, keeping the
    /// light level and the bits above the family's basic-type field.
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

impl crate::named::Named for ConnectionKind {
    /// One representative per kind: the wire names a connection's `kind`
    /// word, and the plan tools fill the row's fields from the parameter,
    /// not from these sentinel payloads.
    const ALL: &'static [Self] = &[
        Self::Opening,
        Self::Door {
            style: DoorStyle::Normal,
        },
        Self::Window { sill: 0, lintel: 0 },
        Self::Stairs {
            step_height: 0,
            step_depth: 0,
        },
    ];

    #[inline]
    fn name(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Door { .. } => "door",
            Self::Window { .. } => "window",
            Self::Stairs { .. } => "stairs",
        }
    }
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

impl crate::named::Named for DoorStyle {
    const ALL: &'static [Self] = &[
        Self::Normal,
        Self::Fast,
        Self::StayOpen,
        Self::Locked(KeyColor::Red),
        Self::Locked(KeyColor::Blue),
        Self::Locked(KeyColor::Yellow),
        Self::Remote,
    ];

    #[inline]
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Fast => "fast",
            Self::StayOpen => "stay_open",
            Self::Locked(color) => match color {
                KeyColor::Red => "locked_red",
                KeyColor::Blue => "locked_blue",
                KeyColor::Yellow => "locked_yellow",
            },
            Self::Remote => "remote",
        }
    }
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
/// `exit` and `switch` rows, dresses `lift` rooms as platforms and
/// stands `teleporter` pads and destinations once the map is compiled;
/// [`compile`](fn@crate::plan::compile) itself validates every row and
/// draws no geometry for it. The two wall-mounted kinds carry an
/// optional plan `at` point, transformed with the rooms like a
/// [`Connection::at`], that picks the wall among several matching
/// `wall` sides and centres the panel there.
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
        /// Plan point that picks the wall (and centres the panel on it)
        /// when several match `wall`; the longest matching wall wins
        /// without it. The `switch` style only: a `walk_over` exit
        /// dresses every boundary of the room and mounts no panel.
        at: Option<Pt>,
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
        /// Plan point that picks the wall (and centres the panel on it)
        /// when several match `wall`; the longest matching wall wins
        /// without it.
        at: Option<Pt>,
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

impl crate::named::Named for Side {
    const ALL: &'static [Self] = &[Self::North, Self::South, Self::East, Self::West];

    #[inline]
    fn name(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }
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

impl crate::named::Named for ExitStyle {
    const ALL: &'static [Self] = &[Self::Switch, Self::WalkOver];

    #[inline]
    fn name(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::WalkOver => "walk_over",
        }
    }
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
        // The rectangle's corners are `Copy` and the ring's `Vec` is needed
        // as the owned return value, so the shape is cloned rather than
        // borrowed: binding a field out of a `&Shape` needs a pattern whose
        // shape differs from the expression's, which this crate's denied
        // `pattern_type_mismatch` and `ref_patterns` forbid.
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

mod validate;

#[cfg(test)]
mod tests;
