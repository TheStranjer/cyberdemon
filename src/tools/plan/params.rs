//! Wire parameters of [`Cyberdemon::build_map_from_plan`](super::Cyberdemon::build_map_from_plan)
//! and their one conversion onto [`crate::plan::model`] types.
//!
//! The wire shape mirrors the model flat: a room is `{id, rect | points,
//! …}`, a connection is `{from, to, kind, …}` with the kind's extra
//! fields at the top level, so the JSON an LLM writes reads like the
//! model it becomes. `to_model` only maps names and fills defaults
//! ([`crate::plan::model`]'s); every structural judgment — positive
//! edges, unique ids, touching rooms — stays with [`Plan::validate`] and
//! the compiler, whose refusals [`super::with_plan_path`] then prefixes
//! with the plan path.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, KeyColor, Plan, Room, Shape};

/// The [`Plan::wall_thickness`] the wire default carries.
const DEFAULT_WALL_THICKNESS: i64 = 16_i64;
/// The [`Room::floor`] the wire default carries.
const DEFAULT_FLOOR: i32 = 0_i32;
/// The [`Room::ceiling`] the wire default carries.
const DEFAULT_CEILING: i32 = 128_i32;
/// The [`Room::light`] the wire default carries.
const DEFAULT_LIGHT: i32 = 160_i32;
/// The [`Connection::width`] the wire default carries.
const DEFAULT_WIDTH: i64 = 64_i64;
/// The [`ConnectionKind::Window`] sill the wire default carries.
const DEFAULT_SILL: i32 = 32_i32;
/// The [`ConnectionKind::Window`] lintel the wire default carries.
const DEFAULT_LINTEL: i32 = 24_i32;
/// The [`ConnectionKind::Stairs`] step height the wire default carries.
const DEFAULT_STEP_HEIGHT: i32 = 16_i32;
/// The [`ConnectionKind::Stairs`] step depth the wire default carries.
const DEFAULT_STEP_DEPTH: i32 = 24_i32;
/// The [`DoorStyle`] a plain `"door"` connection asks for.
const DEFAULT_DOOR_STYLE: &str = "normal";

/// Arguments for [`Cyberdemon::build_map_from_plan`](super::Cyberdemon::build_map_from_plan).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildPlanParams {
    /// The new map's header name (1 to 8 printable ASCII characters,
    /// case-insensitive). Anything but `ExMy`/`MAPxx` still builds;
    /// engines only load those two forms unless MAPINFO defines the name.
    pub(super) map: String,
    /// The layout to write: `"doom"` (the default), `"hexen"` or
    /// `"udmf"`.
    #[serde(default)]
    pub(super) format: Option<String>,
    /// The namespace the `UDMF` `TEXTMAP` declares (1 to 32 ASCII
    /// letters, digits or underscores, lowercased; default `"zdoom"`).
    /// `UDMF` only: ignored for the binary formats.
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// Discard the working archive's block of this name (and its GL run)
    /// before building (default `false`; a map only lower layers hold is
    /// always overridable and needs no `replace`).
    #[serde(default)]
    pub(super) replace: bool,
    /// Thickness of the walls grown between rooms, map units; `0` makes
    /// the room polygons touch directly (default `16`).
    #[serde(default = "default_wall_thickness")]
    pub(super) wall_thickness: i64,
    /// The rooms to lay out, in painter's order (bottom first).
    pub(super) rooms: Vec<RoomParam>,
    /// Openings, doors, windows and stairs joining the rooms.
    pub(super) connections: Vec<ConnectionParam>,
    /// Texture theme to dress this map with, for this call only
    /// (case-insensitive; the session's theme stands when unset).
    /// Unknown names fail `invalid_argument` listing the themes the
    /// session's game offers; the session itself never changes.
    #[serde(default)]
    pub(super) theme: Option<String>,
}

/// One wire room: an id, exactly one of `rect`/`points`, and the sector
/// properties its interior carries.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RoomParam {
    /// Name, unique in the plan, that connections refer to.
    pub(super) id: String,
    /// Axis-aligned rectangle `{x, y, w, h}` from `(x, y)` to
    /// `(x + w, y + h)`. Exactly one of `rect` and `points`.
    #[serde(default)]
    pub(super) rect: Option<RectParam>,
    /// A closed ring of `[x, y]` points, implicitly closing to the
    /// first; at least three distinct ones. Exactly one of `rect` and
    /// `points`.
    #[serde(default)]
    pub(super) points: Option<Vec<[i64; 2]>>,
    /// Floor height of the room's sector (default `0`).
    #[serde(default)]
    pub(super) floor: Option<i32>,
    /// Ceiling height of the room's sector (default `128`).
    #[serde(default)]
    pub(super) ceiling: Option<i32>,
    /// Light level of the room's sector (default `160`).
    #[serde(default)]
    pub(super) light: Option<i32>,
    /// Flat name for the floor; the theme's `floor` role answers when
    /// unset.
    #[serde(default)]
    pub(super) floor_flat: Option<String>,
    /// Flat name for the ceiling; the theme's `ceiling` role answers
    /// when unset (an `outdoor` room gets the sky instead).
    #[serde(default)]
    pub(super) ceiling_flat: Option<String>,
    /// Texture name for the walls; the theme's `wall` role answers when
    /// unset.
    #[serde(default)]
    pub(super) wall_texture: Option<String>,
    /// Whether the room is open to the sky (default `false`).
    #[serde(default)]
    pub(super) outdoor: bool,
}

/// One wire `rect`: the axis-aligned rectangle from `(x, y)` to
/// `(x + w, y + h)`.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
pub(super) struct RectParam {
    /// `X` of the bottom-left corner.
    pub(super) x: i64,
    /// `Y` of the bottom-left corner.
    pub(super) y: i64,
    /// Width; must be positive.
    pub(super) w: i64,
    /// Height; must be positive.
    pub(super) h: i64,
}

/// One wire connection: two room ids, a kind name and the kind's extra
/// fields beside it.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ConnectionParam {
    /// Optional name used in reports and messages; defaults to the
    /// `from->to` label.
    #[serde(default)]
    pub(super) id: Option<String>,
    /// Id of the room the connection starts from.
    pub(super) from: String,
    /// Id of the room the connection leads to.
    pub(super) to: String,
    /// What kind of opening it is: `"opening"`, `"door"`, `"window"` or
    /// `"stairs"` (case-insensitive).
    pub(super) kind: String,
    /// Where to place the opening, `[x, y]` (default: centred on the
    /// shared wall).
    #[serde(default)]
    pub(super) at: Option<[i64; 2]>,
    /// Clear width of the opening, map units (default `64`).
    #[serde(default)]
    pub(super) width: Option<i64>,
    /// Door only: `normal` (default), `fast`, `stay_open`,
    /// `locked_red`, `locked_blue`, `locked_yellow` or `remote`.
    /// No action is installed yet — the style only picks the door's
    /// dressing.
    #[serde(default)]
    pub(super) style: Option<String>,
    /// Window only: height of the glass bottom above the lower floor
    /// (default `32`).
    #[serde(default)]
    pub(super) sill: Option<i32>,
    /// Window only: height of the glass top below the higher ceiling
    /// (default `24`).
    #[serde(default)]
    pub(super) lintel: Option<i32>,
    /// Stairs only: height of each step (default `16`, clamped to
    /// `1..=24`).
    #[serde(default)]
    pub(super) step_height: Option<i32>,
    /// Stairs only: depth of each step, map units; must be positive
    /// (default `24`).
    #[serde(default)]
    pub(super) step_depth: Option<i32>,
}

/// The `wall_thickness` serde default, so the wire documents it.
const fn default_wall_thickness() -> i64 {
    DEFAULT_WALL_THICKNESS
}

/// The `invalid_argument` naming the plan path `at`.
fn invalid_at(at: &str, problem: &str) -> CyberError {
    CyberError::InvalidArgument(format!("{at}: {problem}"))
}

impl BuildPlanParams {
    /// The wire rooms and connections as one model [`Plan`] with the
    /// requested `wall_thickness`, filling every documented default.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the plan path (`rooms[2]`,
    /// `connections[0]`) for a room with neither or both of `rect` and
    /// `points`, an unknown connection `kind` or an unknown door
    /// `style`.
    pub(super) fn to_model(&self) -> Result<Plan, CyberError> {
        Ok(Plan {
            rooms: self.to_rooms()?,
            connections: self.to_connections()?,
            wall_thickness: self.wall_thickness,
        })
    }

    /// The rooms, each converted under its own plan path.
    fn to_rooms(&self) -> Result<Vec<Room>, CyberError> {
        self.rooms
            .iter()
            .enumerate()
            .map(|(index, room)| room.to_model(index))
            .collect()
    }

    /// The connections, each converted under its own plan path.
    fn to_connections(&self) -> Result<Vec<Connection>, CyberError> {
        self.connections
            .iter()
            .enumerate()
            .map(|(index, connection)| connection.to_model(index))
            .collect()
    }
}

impl RoomParam {
    /// The model room: the footprint exactly one of `rect`/`points`
    /// gives, the sector properties or their defaults.
    fn to_model(&self, index: usize) -> Result<Room, CyberError> {
        Ok(Room {
            id: self.id.clone(),
            shape: self.shape(index)?,
            floor: self.floor.unwrap_or(DEFAULT_FLOOR),
            ceiling: self.ceiling.unwrap_or(DEFAULT_CEILING),
            light: self.light.unwrap_or(DEFAULT_LIGHT),
            floor_flat: self.floor_flat.clone(),
            ceiling_flat: self.ceiling_flat.clone(),
            wall_texture: self.wall_texture.clone(),
            outdoor: self.outdoor,
        })
    }

    /// The footprint: `rect` or `points`, never both and never neither.
    fn shape(&self, index: usize) -> Result<Shape, CyberError> {
        let at = format!("rooms[{index}]");
        match (self.rect, self.points.clone()) {
            (Some(rect), None) => Ok(Shape::Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            }),
            (None, Some(points)) => Ok(Shape::Polygon {
                points: points.into_iter().map(<(i64, i64)>::from).collect(),
            }),
            (Some(_), Some(_)) => Err(invalid_at(
                &at,
                "give exactly one of rect and points: both were passed",
            )),
            (None, None) => Err(invalid_at(
                &at,
                "give exactly one of rect ({x, y, w, h}) and points ([[x, y], ...])",
            )),
        }
    }
}

impl ConnectionParam {
    /// The model connection, its kind resolved from the wire name.
    fn to_model(&self, index: usize) -> Result<Connection, CyberError> {
        Ok(Connection {
            id: self.id.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            kind: self.kind(index)?,
            at: self.at.map(<(i64, i64)>::from),
            width: self.width.unwrap_or(DEFAULT_WIDTH),
        })
    }

    /// The [`ConnectionKind`] the `kind` name and its extra fields
    /// spell.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming `connections[index]` for
    /// an unknown kind or door style.
    fn kind(&self, index: usize) -> Result<ConnectionKind, CyberError> {
        match self.kind.to_ascii_lowercase().as_str() {
            "opening" => Ok(ConnectionKind::Opening),
            "door" => Ok(ConnectionKind::Door {
                style: self.door_style(index)?,
            }),
            "window" => Ok(ConnectionKind::Window {
                sill: self.sill.unwrap_or(DEFAULT_SILL),
                lintel: self.lintel.unwrap_or(DEFAULT_LINTEL),
            }),
            "stairs" => Ok(ConnectionKind::Stairs {
                step_height: self.step_height.unwrap_or(DEFAULT_STEP_HEIGHT),
                step_depth: self.step_depth.unwrap_or(DEFAULT_STEP_DEPTH),
            }),
            other => Err(invalid_at(
                &format!("connections[{index}]"),
                &format!(
                    "kind {other:?} is not a connection kind: pass one of opening, door, window, stairs"
                ),
            )),
        }
    }

    /// The [`DoorStyle`] the `style` name spells (`normal` by default).
    fn door_style(&self, index: usize) -> Result<DoorStyle, CyberError> {
        let wanted = self
            .style
            .as_deref()
            .unwrap_or(DEFAULT_DOOR_STYLE)
            .to_ascii_lowercase();
        let style = match wanted.as_str() {
            "normal" => DoorStyle::Normal,
            "fast" => DoorStyle::Fast,
            "stay_open" => DoorStyle::StayOpen,
            "locked_red" => DoorStyle::Locked(KeyColor::Red),
            "locked_blue" => DoorStyle::Locked(KeyColor::Blue),
            "locked_yellow" => DoorStyle::Locked(KeyColor::Yellow),
            "remote" => DoorStyle::Remote,
            other => {
                return Err(invalid_at(
                    &format!("connections[{index}]"),
                    &format!(
                        "style {other:?} is not a door style: pass one of normal, fast, \
                         stay_open, locked_red, locked_blue, locked_yellow, remote"
                    ),
                ));
            }
        };
        Ok(style)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{BuildPlanParams, DEFAULT_WIDTH};
    use crate::plan::model::{ConnectionKind, DoorStyle, KeyColor, Plan, Shape};

    /// Wire parameters parsed from the two-room-and-door example.
    const FULL: &str = r#"{
        "map": "MAP05",
        "rooms": [
            {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
            {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128},
             "floor": 16, "ceiling": 160, "light": 200,
             "floor_flat": "FLAT10", "ceiling_flat": "F_SKY1",
             "wall_texture": "WALL1", "outdoor": true}
        ],
        "connections": [
            {"id": "d1", "from": "a", "to": "b", "kind": "door",
             "at": [128, 32], "width": 48, "style": "locked_blue"},
            {"from": "b", "to": "a", "kind": "window", "at": [128, 96],
             "sill": 48, "lintel": 20},
            {"from": "a", "to": "b", "kind": "stairs",
             "step_height": 8, "step_depth": 16},
            {"from": "a", "to": "b", "kind": "OPENING", "width": 32}
        ],
        "wall_thickness": 24,
        "format": "udmf",
        "namespace": "ZDoom",
        "replace": true,
        "theme": "hell"
    }"#;

    /// A one-room request with `body` spliced in as its `rooms`.
    fn rooms_request(body: &str) -> BuildPlanParams {
        let text = format!(r#"{{"map": "MAP01", "rooms": {body}, "connections": []}}"#);
        from_str(&text).expect("the request parses")
    }

    /// A one-connection request with `body` spliced in as its
    /// `connections`.
    fn connections_request(body: &str) -> BuildPlanParams {
        let text = format!(
            r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": {body}}}"#
        );
        from_str(&text).expect("the request parses")
    }

    /// The error code of `error`, for compact asserts.
    fn code(error: &crate::error::CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    #[track_caller]
    fn refuse(error: &crate::error::CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    #[test]
    fn the_documented_wire_shape_lands_in_the_model() {
        let args: BuildPlanParams = from_str(FULL).expect("the full example parses");
        assert_eq!(args.map, "MAP05");
        assert_eq!(args.format.as_deref(), Some("udmf"));
        assert_eq!(args.namespace.as_deref(), Some("ZDoom"));
        assert!(args.replace, "the flag arrives");
        assert_eq!(args.wall_thickness, 24_i64);
        assert_eq!(args.theme.as_deref(), Some("hell"));
        let plan = args.to_model().expect("the plan converts");
        let b = plan.rooms.get(1).expect("room b");
        assert_eq!(b.floor, 16_i32, "explicit sector properties");
        assert_eq!(b.ceiling, 160_i32);
        assert_eq!(b.light, 200_i32);
        assert_eq!(b.floor_flat.as_deref(), Some("FLAT10"));
        assert_eq!(b.wall_texture.as_deref(), Some("WALL1"));
        assert!(b.outdoor, "the sky flag rides along");
        let door = plan.connections.first().expect("the door");
        assert_eq!(
            door.kind,
            ConnectionKind::Door {
                style: DoorStyle::Locked(KeyColor::Blue),
            },
            "the locked-blue style"
        );
        assert_eq!(door.at, Some((128_i64, 32_i64)), "the placement point");
        assert_eq!(door.width, 48_i64, "the explicit width");
        assert_eq!(
            plan.connections.get(1).expect("the window").kind,
            ConnectionKind::Window {
                sill: 48_i32,
                lintel: 20_i32
            },
            "the window band"
        );
        assert_eq!(
            plan.connections.get(2).expect("the stairs").kind,
            ConnectionKind::Stairs {
                step_height: 8_i32,
                step_depth: 16_i32
            },
            "the stair run"
        );
        assert_eq!(
            plan.connections.get(3).expect("the opening").kind,
            ConnectionKind::Opening,
            "kinds are case-insensitive"
        );
        assert_eq!(plan.connections.get(3).expect("the opening").width, 32_i64);
    }

    #[test]
    fn omissions_materialise_the_documented_defaults() {
        let args = rooms_request(r#"[{"id": "a", "rect": {"x": 1, "y": 2, "w": 3, "h": 4}}]"#);
        let plan: Plan = args.to_model().expect("defaults fill in");
        assert_eq!(plan.wall_thickness, 16_i64, "the default wall");
        let room = plan.rooms.first().expect("the one room");
        assert_eq!(room.floor, 0_i32);
        assert_eq!(room.ceiling, 128_i32);
        assert_eq!(room.light, 160_i32);
        assert!(!room.outdoor);
        assert_eq!(room.floor_flat, None, "the theme answers the roles");
        assert_eq!(
            room.shape,
            Shape::Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4
            }
        );
        let window_args = connections_request(r#"[{"from": "a", "to": "a", "kind": "window"}]"#);
        let model = window_args.to_model().expect("kind defaults fill in");
        let window = model.connections.first().expect("the window");
        assert_eq!(window.width, DEFAULT_WIDTH, "the default opening width");
        assert_eq!(window.at, None, "centred on the shared wall");
        assert_eq!(window.id, None, "the label is derived by the model");
        assert_eq!(
            window.kind,
            ConnectionKind::Window {
                sill: 32_i32,
                lintel: 24_i32
            },
            "the default glass band"
        );
    }

    #[test]
    fn points_rooms_carry_the_ring_as_given() {
        let args = rooms_request(r#"[{"id": "a", "points": [[0, 0], [64, 0], [32, 64]]}]"#);
        let plan = args.to_model().expect("a polygon room converts");
        assert_eq!(
            plan.rooms.first().expect("the room").shape,
            Shape::Polygon {
                points: vec![(0_i64, 0_i64), (64, 0), (32, 64)]
            }
        );
    }

    #[test]
    fn a_room_with_neither_or_both_footprints_names_its_path() {
        let neither =
            rooms_request(r#"[{"id": "a"}, {"id": "b", "rect": {"x":0,"y":0,"w":1,"h":1}}]"#);
        let bare = refuse(&neither.to_model().expect_err("no footprint"));
        assert!(bare.contains("rooms[0]"), "{bare}");

        let second = rooms_request(
            r#"[{"id": "a", "rect": {"x":0,"y":0,"w":8,"h":8}},
                {"id": "b", "rect": {"x":0,"y":0,"w":8,"h":8},
                 "points": [[0,0],[8,0],[8,8]]}]"#,
        );
        let message = refuse(&second.to_model().expect_err("both footprints"));
        assert!(message.contains("rooms[1]"), "{message}");
        assert!(message.contains("exactly one"), "{message}");
    }

    #[test]
    fn unknown_kind_and_style_names_mention_their_paths() {
        let args = connections_request(r#"[{"from": "a", "to": "a", "kind": "portal"}]"#);
        let kind = refuse(&args.to_model().expect_err("no such kind"));
        assert!(kind.contains("connections[0]"), "{kind}");
        assert!(
            kind.contains("opening, door, window, stairs"),
            "the refusal lists the kinds: {kind}"
        );

        let styled =
            connections_request(r#"[{"from": "a", "to": "a", "kind": "door", "style": "slide"}]"#);
        let message = refuse(&styled.to_model().expect_err("no such style"));
        assert!(message.contains("connections[0]"), "{message}");
        assert!(
            message.contains(
                "normal, fast, stay_open, locked_red, locked_blue, locked_yellow, remote"
            ),
            "the refusal lists the styles: {message}"
        );
    }

    #[test]
    fn the_door_style_defaults_to_normal_and_reads_every_name() {
        let plain = connections_request(r#"[{"from": "a", "to": "a", "kind": "door"}]"#);
        let model = plain.to_model().expect("the plain door converts");
        assert_eq!(
            model.connections.first().expect("door").kind,
            ConnectionKind::Door {
                style: DoorStyle::Normal
            }
        );
        for (text, style) in [
            ("normal", DoorStyle::Normal),
            ("FAST", DoorStyle::Fast),
            ("stay_open", DoorStyle::StayOpen),
            ("locked_red", DoorStyle::Locked(KeyColor::Red)),
            ("locked_blue", DoorStyle::Locked(KeyColor::Blue)),
            ("locked_yellow", DoorStyle::Locked(KeyColor::Yellow)),
            ("remote", DoorStyle::Remote),
        ] {
            let probe = connections_request(&format!(
                r#"[{{"from":"a","to":"a","kind":"door","style":"{text}"}}]"#
            ));
            let converted = probe.to_model().expect("styles are case-insensitive");
            assert_eq!(
                converted.connections.first().expect("door").kind,
                ConnectionKind::Door { style },
                "{text}"
            );
        }
    }
}
