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
//!
//! The `units` object and the `grid` are not model fields: [`BuildPlanParams::to_units`]
//! builds the [`Units`] the caller carries the finished model plan
//! through (`crate::plan::transform_plan`) before compiling it.
//!
//! The `things` rows and the `player_start` are not model fields either:
//! [`BuildPlanParams::to_things`] validates their shape (type-or-name,
//! coordinates-or-room, angle range, skill names) into
//! [`ThingRequest`] rows; the doomednum, the presence flags and the
//! placement itself need the session and the compiled map and live in
//! [`super`] and [`crate::plan::things`].
//!
//! The `features` rows are model fields kept flat the way
//! [`ConnectionParam`] keeps a [`ConnectionKind`]: one `kind` name plus
//! the kind's extra fields beside it, mapped onto the tagged [`Feature`]
//! by [`BuildPlanParams::to_model`]. The reference checks a feature
//! needs (known rooms, a switch's `opens`) stay with [`Plan::validate`].

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::plan::Units;
use crate::plan::model::{
    Connection, ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Plan, Room, Shape, Side,
};
use crate::plan::things::{Skill, ThingSpot};
use crate::wad::map::features::{LiftStyle, LightEffect};

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
/// The [`ExitStyle`] a plain `"exit"` feature row asks for.
const DEFAULT_EXIT_STYLE: &str = "switch";
/// The [`LiftStyle`] a plain `"lift"` feature row asks for.
const DEFAULT_LIFT_STYLE: &str = "use_wall";
/// The [`Feature::Switch`] `repeatable` the wire default carries.
const DEFAULT_SWITCH_REPEATABLE: bool = false;
/// The [`Feature::Lift`] `repeatable` the wire default carries.
const DEFAULT_LIFT_REPEATABLE: bool = true;
/// The [`Feature::Teleporter`] arrival facing the wire default carries.
const DEFAULT_ANGLE: i32 = 0_i32;
/// The [`Units::scale_num`] the wire default carries.
const DEFAULT_SCALE_NUM: i64 = 1_i64;
/// The [`Units::scale_den`] the wire default carries.
const DEFAULT_SCALE_DEN: i64 = 1_i64;
/// The snap grid the wire default carries, map units.
const DEFAULT_GRID: i64 = 8_i64;
/// The [`ThingParam::count`] the wire default carries.
const DEFAULT_COUNT: u32 = 1_u32;

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
    /// How the plan's coordinates become map units: an integer scale,
    /// an origin and an optional vertical mirror, applied to every room
    /// footprint and connection placement before compiling. Sizes
    /// (`wall_thickness`, connection `width`, `sill`, `lintel`,
    /// `step_height`, `step_depth`) are map units already and never
    /// scale. Unset (or `null`) is the identity (`scale_num: 1`,
    /// `scale_den: 1`, origins `0`, `flip_y: false`).
    #[serde(default)]
    pub(super) units: Option<UnitsParam>,
    /// Snap grid in map units every transformed coordinate is pulled to
    /// the nearest line of (default `8`; `1` switches snapping off, `0`
    /// and negatives are refused).
    #[serde(default = "default_grid")]
    pub(super) grid: i64,
    /// The rooms to lay out, in painter's order (bottom first).
    pub(super) rooms: Vec<RoomParam>,
    /// Openings, doors, windows and stairs joining the rooms.
    pub(super) connections: Vec<ConnectionParam>,
    /// Game structures the plan carries: `{"kind": "exit" | "switch" |
    /// "lift" | "teleporter", …}` rows. They parse, transform and
    /// validate; installing them on the map arrives with the follow-up
    /// wiring updates.
    #[serde(default)]
    pub(super) features: Vec<FeatureParam>,
    /// The things to stand in the compiled map. Each row gives exactly
    /// one of `type` (a doomednum) and `name` (resolved against the
    /// session's definitions), and exactly one of an `x`/`y` plan-point
    /// pair and a `room` id to fill.
    #[serde(default)]
    pub(super) things: Vec<ThingParam>,
    /// The player 1 start: exactly one of an `x`/`y` plan-point pair and
    /// a `room` id (whose sector's bounding-box centre it lands on).
    /// Every built map needs a player start here or as a thing of type
    /// `1`.
    #[serde(default)]
    pub(super) player_start: Option<PlayerStartParam>,
    /// Texture theme to dress this map with, for this call only
    /// (case-insensitive; the session's theme stands when unset).
    /// Unknown names fail `invalid_argument` listing the themes the
    /// session's game offers; the session itself never changes.
    #[serde(default)]
    pub(super) theme: Option<String>,
    /// Build the node run into the block before the write and validate
    /// the finished map (default `false`). `true` runs the whole
    /// `build_all_nodes` set on the compiled model as part of this one
    /// undoable edit — the classic `SEGS`/`SSECTORS`/`NODES` run plus a
    /// `zero` `REJECT` and a fresh `BLOCKMAP` for a binary map, the
    /// compressed extended `ZNODES` lump for a `UDMF` one — and fills
    /// the result's `nodes` and `validation` fields. Unset, both stay
    /// `null` and the block ships without node data.
    #[serde(default)]
    pub(super) finalize: bool,
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
    /// Whether the room counts as a secret area (default `false`). The
    /// follow-up wiring updates are the first to act on it.
    #[serde(default)]
    pub(super) secret: bool,
    /// A light effect for the room's sector: `"none"`, `"flicker"`,
    /// `"strobe_fast"`, `"strobe_slow"`, `"glow"` or `"random_off"`
    /// (case-insensitive; unset keeps the room's static `light`).
    #[serde(default)]
    pub(super) light_effect: Option<String>,
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

/// One wire `units` object: the coordinate half of [`Units`] as JSON.
/// The snap grid arrives beside it as the request's top-level `grid`;
/// [`BuildPlanParams::to_units`] carries it in.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(default)]
pub(super) struct UnitsParam {
    /// Numerator of the scale: `4` with `scale_den: 1` makes one plan
    /// unit four map units (default `1`; `0` is refused).
    pub(super) scale_num: i64,
    /// Denominator of the scale (default `1`; `0` is refused).
    pub(super) scale_den: i64,
    /// Map `x` the plan origin lands on (default `0`).
    pub(super) origin_x: i64,
    /// Map `y` the plan origin lands on (default `0`) and the line
    /// `flip_y` mirrors about.
    pub(super) origin_y: i64,
    /// Mirror `y` about `origin_y` (default `false`): set it for
    /// image-space drawings, whose rows grow down while map `y` grows up.
    pub(super) flip_y: bool,
}

impl Default for UnitsParam {
    /// The documented identity defaults: scale `1/1`, both origins `0`,
    /// no mirror.
    fn default() -> Self {
        Self {
            scale_num: DEFAULT_SCALE_NUM,
            scale_den: DEFAULT_SCALE_DEN,
            origin_x: 0_i64,
            origin_y: 0_i64,
            flip_y: false,
        }
    }
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

/// One wire feature row: a `kind` name plus that kind's extra fields
/// beside it. The reference checks (known rooms, an `opens` that names
/// a connection) belong to [`Plan::validate`], not here.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct FeatureParam {
    /// Which feature: `"exit"`, `"switch"`, `"lift"` or `"teleporter"`
    /// (case-insensitive).
    pub(super) kind: String,
    /// Exit, switch and lift only: the id of the room the feature
    /// stands in.
    #[serde(default)]
    pub(super) room: Option<String>,
    /// Exit or lift only: the exit's style, `switch` (default) or
    /// `walk_over`; the lift's style, `use_wall` (default) or
    /// `walk_trigger`. Case-insensitive.
    #[serde(default)]
    pub(super) style: Option<String>,
    /// Exit only: whether the room is a secret area (default `false`).
    #[serde(default)]
    pub(super) secret: bool,
    /// Exit and switch only: which wall carries it — `"north"`,
    /// `"south"`, `"east"` or `"west"` (case-insensitive; unset lets
    /// the follow-up wiring pick).
    #[serde(default)]
    pub(super) wall: Option<String>,
    /// Switch only: the `id` of the connection this switch operates.
    #[serde(default)]
    pub(super) opens: Option<String>,
    /// Switch and lift only: whether the action keeps working after
    /// use (default `false` for a switch, `true` for a lift).
    #[serde(default)]
    pub(super) repeatable: Option<bool>,
    /// Lift only: the floor the platform lowers to, map units (the
    /// lowest neighbouring floor by default).
    #[serde(default)]
    pub(super) low_floor: Option<i32>,
    /// Teleporter only: the id of the room the pad stands in.
    #[serde(default)]
    pub(super) from_room: Option<String>,
    /// Teleporter only: the id of the room the destination stands in.
    #[serde(default)]
    pub(super) to_room: Option<String>,
    /// Teleporter only: the pad's centre as a plan point `[x, y]`
    /// (transformed and snapped with the rooms; the room centre by
    /// default).
    #[serde(default)]
    pub(super) pad: Option<[i64; 2]>,
    /// Teleporter only: the destination's centre as a plan point
    /// `[x, y]` (transformed and snapped with the rooms; the room
    /// centre by default).
    #[serde(default)]
    pub(super) destination: Option<[i64; 2]>,
    /// Teleporter only: arrival facing in degrees (default `0`).
    #[serde(default)]
    pub(super) angle: Option<i32>,
    /// Teleporter only: whether the destination teleports back
    /// (default `false`).
    #[serde(default)]
    pub(super) two_way: bool,
}

/// The `wall_thickness` serde default, so the wire documents it.
const fn default_wall_thickness() -> i64 {
    DEFAULT_WALL_THICKNESS
}

/// The `grid` serde default, so the wire documents it.
const fn default_grid() -> i64 {
    DEFAULT_GRID
}

/// The `count` serde default, so the wire documents it.
const fn default_count() -> u32 {
    DEFAULT_COUNT
}

/// One wire thing: a type or name, a pinned point or a room to fill,
/// and the placement details.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ThingParam {
    /// The thing type number (a doomednum). Exactly one of `type` and
    /// `name`.
    #[serde(default, rename = "type")]
    pub(super) thing_type: Option<u32>,
    /// The thing's editor name, resolved against the session's
    /// definitions: an exact case-insensitive name first, else a
    /// unique case-insensitive substring. Exactly one of `type` and
    /// `name`.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Plan-space `X` (transformed and snapped with the rooms). Exactly
    /// one of the `x`/`y` pair and `room`.
    #[serde(default)]
    pub(super) x: Option<i64>,
    /// Plan-space `Y`, always beside `x`.
    #[serde(default)]
    pub(super) y: Option<i64>,
    /// The id of a plan room to fill on the clearance grid. Exactly one
    /// of `room` and the `x`/`y` pair.
    #[serde(default)]
    pub(super) room: Option<String>,
    /// How many copies a room fill places (default `1`; a pinned `x`/`y`
    /// spot holds exactly one thing whatever this says).
    #[serde(default = "default_count")]
    pub(super) count: u32,
    /// Facing in degrees, `0..=359` (default `0`).
    #[serde(default)]
    pub(super) angle: i32,
    /// Skills the thing appears on: some of `"easy"`, `"medium"`,
    /// `"hard"` (case-insensitive). Unset means every skill.
    #[serde(default)]
    pub(super) skills: Option<Vec<String>>,
    /// Spawn the thing out of single-player (default `false`): the
    /// `notsingle` bit in the `Doom` family, the `coop` and `dm` mode
    /// bits in the `Hexen` one.
    #[serde(default)]
    pub(super) multiplayer_only: bool,
}

/// One wire `player_start`: exactly one of an `x`/`y` plan-point pair
/// and a room id, plus the facing.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct PlayerStartParam {
    /// Plan-space `X` (transformed and snapped with the rooms). Exactly
    /// one of the `x`/`y` pair and `room`.
    #[serde(default)]
    pub(super) x: Option<i64>,
    /// Plan-space `Y`, always beside `x`.
    #[serde(default)]
    pub(super) y: Option<i64>,
    /// The id of a plan room whose sector's bounding-box centre the
    /// start lands on. Exactly one of `room` and the `x`/`y` pair.
    #[serde(default)]
    pub(super) room: Option<String>,
    /// Facing in degrees, `0..=359` (default `0`).
    #[serde(default)]
    pub(super) angle: i32,
}

/// The type half of a [`ThingRequest`], as the wire spelled it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum KindSource {
    /// An explicit doomednum.
    Type(u32),
    /// A name to resolve against the session's definitions.
    Name(String),
    /// The player 1 start: type `1`, no resolution.
    PlayerOne,
}

/// A validated wire row: everything about a planned thing except the
/// doomednum (which may need the session's definitions) and the flag
/// bits (which need the compiled map's flag family).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ThingRequest {
    /// The type or the name to resolve.
    pub(super) kind: KindSource,
    /// Where the thing goes.
    pub(super) spot: ThingSpot,
    /// How many copies a room fill may place.
    pub(super) count: u32,
    /// Facing, validated to `0..=359`.
    pub(super) angle: i32,
    /// The requested skills (`None` means every skill).
    pub(super) skills: Option<Vec<Skill>>,
    /// Whether the thing stays out of single-player.
    pub(super) multiplayer_only: bool,
    /// The wire path error messages name.
    pub(super) path: String,
}

/// The `invalid_argument` naming the plan path `at`.
fn invalid_at(at: &str, problem: &str) -> CyberError {
    CyberError::InvalidArgument(format!("{at}: {problem}"))
}

impl BuildPlanParams {
    /// The wire rooms, connections and features as one model [`Plan`]
    /// with the requested `wall_thickness`, filling every documented
    /// default.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the plan path (`rooms[2]`,
    /// `connections[0]`, `features[1]`) for a room with neither or both
    /// of `rect` and `points` or an unknown `light_effect` name, an
    /// unknown connection `kind` or door `style`, an unknown feature
    /// `kind`, `style` or `wall`, or a feature row missing its `room`
    /// (its `from_room`/`to_room`, or its `opens`).
    pub(super) fn to_model(&self) -> Result<Plan, CyberError> {
        Ok(Plan {
            rooms: self.to_rooms()?,
            connections: self.to_connections()?,
            features: self.to_features()?,
            wall_thickness: self.wall_thickness,
        })
    }

    /// The [`Units`] this request asks for, carrying its top-level
    /// `grid` in so [`Units::check`] sees one snap grid everywhere. A
    /// missing or `null` `units` object is the identity scale. The
    /// values themselves are only *checked* there; this mapping cannot
    /// fail.
    pub(super) fn to_units(&self) -> Units {
        let wire = self.units.unwrap_or_default();
        Units {
            scale_num: wire.scale_num,
            scale_den: wire.scale_den,
            origin_x: wire.origin_x,
            origin_y: wire.origin_y,
            grid: self.grid,
            flip_y: wire.flip_y,
        }
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

    /// The features, each converted under its own plan path.
    fn to_features(&self) -> Result<Vec<Feature>, CyberError> {
        self.features
            .iter()
            .enumerate()
            .map(|(index, feature)| feature.to_model(index))
            .collect()
    }

    /// The `things` rows and the `player_start`, shape-validated under
    /// their wire paths. The `player_start` row comes last, so room
    /// fills land before the start is appended.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming `things[i]`,
    /// `things[i].skills` or `player_start` for a row with neither or
    /// both of `type`/`name` or of the `x`/`y` pair and `room`, an
    /// angle outside `0..=359`, or a skill name that is not one.
    pub(super) fn to_things(&self) -> Result<Vec<ThingRequest>, CyberError> {
        let mut rows: Vec<ThingRequest> = Vec::with_capacity(
            self.things
                .len()
                .saturating_add(usize::from(self.player_start.is_some())),
        );
        for (index, row) in self.things.iter().enumerate() {
            rows.push(row.to_request(index)?);
        }
        if let Some(start) = self.player_start.as_ref() {
            rows.push(start.to_request()?);
        }
        Ok(rows)
    }
}

impl ThingParam {
    /// The validated request row for the `things[index]` path.
    fn to_request(&self, index: usize) -> Result<ThingRequest, CyberError> {
        let path = format!("things[{index}]");
        Ok(ThingRequest {
            kind: self.kind(&path)?,
            spot: self.spot(&path)?,
            count: self.count,
            angle: check_angle(self.angle, &path)?,
            skills: parse_skills(self.skills.as_deref(), &path)?,
            multiplayer_only: self.multiplayer_only,
            path,
        })
    }

    /// Exactly one of `type` and `name`.
    fn kind(&self, path: &str) -> Result<KindSource, CyberError> {
        match (self.thing_type, self.name.as_deref()) {
            (Some(kind), None) => Ok(KindSource::Type(kind)),
            (None, Some(name)) => Ok(KindSource::Name(name.to_owned())),
            (Some(_), Some(_)) => Err(invalid_at(
                path,
                "give exactly one of type and name: both were passed",
            )),
            (None, None) => Err(invalid_at(
                path,
                "give one of type (a doomednum) and name (a thing name)",
            )),
        }
    }

    /// The `x`/`y` pair or the `room` id, never both and never half.
    fn spot(&self, path: &str) -> Result<ThingSpot, CyberError> {
        match (self.x, self.y, self.room.as_deref()) {
            (Some(x), Some(y), None) => Ok(ThingSpot::Point((x, y))),
            (None, None, Some(id)) => Ok(ThingSpot::Room { id: id.to_owned() }),
            (_, _, Some(_)) => Err(invalid_at(
                path,
                "a room and coordinates are exclusive: give x/y or room",
            )),
            (Some(_), None, None) => Err(invalid_at(
                path,
                "x needs y: give both coordinates or use room",
            )),
            (None, Some(_), None) => Err(invalid_at(
                path,
                "y needs x: give both coordinates or use room",
            )),
            (None, None, None) => Err(invalid_at(
                path,
                "give an x/y plan point or a room id: where the thing goes",
            )),
        }
    }
}

impl PlayerStartParam {
    /// The validated request row on the `player_start` path: the player
    /// 1 type, one copy, default presence flags.
    fn to_request(&self) -> Result<ThingRequest, CyberError> {
        let path = "player_start".to_owned();
        Ok(ThingRequest {
            kind: KindSource::PlayerOne,
            spot: self.spot(&path)?,
            count: DEFAULT_COUNT,
            angle: check_angle(self.angle, &path)?,
            skills: None,
            multiplayer_only: false,
            path,
        })
    }

    /// The `x`/`y` pair or the room centre, never both and never half.
    fn spot(&self, path: &str) -> Result<ThingSpot, CyberError> {
        match (self.x, self.y, self.room.as_deref()) {
            (Some(x), Some(y), None) => Ok(ThingSpot::Point((x, y))),
            (None, None, Some(id)) => Ok(ThingSpot::RoomCentre { id: id.to_owned() }),
            (_, _, Some(_)) => Err(invalid_at(
                path,
                "a room and coordinates are exclusive: give x/y or room",
            )),
            (None, None, None) => Err(invalid_at(
                path,
                "give an x/y plan point or a room id: where the player starts",
            )),
            (_, _, None) => Err(invalid_at(
                path,
                "x and y travel together: give both coordinates or use room",
            )),
        }
    }
}

/// The angle of a planned row, refused outside `0..=359`.
fn check_angle(angle: i32, path: &str) -> Result<i32, CyberError> {
    if (0_i32..=359_i32).contains(&angle) {
        return Ok(angle);
    }
    Err(invalid_at(
        path,
        &format!("angle {angle} is outside 0..=359; give a compass bearing in degrees"),
    ))
}

/// The `skills` list parsed to [`Skill`]s, on the row's
/// `things[i].skills` path (`None` stays every skill).
fn parse_skills(wanted: Option<&[String]>, path: &str) -> Result<Option<Vec<Skill>>, CyberError> {
    let Some(texts) = wanted else {
        return Ok(None);
    };
    let skills_path = format!("{path}.skills");
    texts
        .iter()
        .map(|text| {
            Skill::parse(text).ok_or_else(|| {
                invalid_at(
                    &skills_path,
                    &format!(
                        "skill {text:?} is not a skill: pass easy, medium or hard, or leave \
                         skills out for every skill"
                    ),
                )
            })
        })
        .collect::<Result<Vec<Skill>, CyberError>>()
        .map(Some)
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
            secret: self.secret,
            light_effect: parse_light_effect(
                self.light_effect.as_deref(),
                &format!("rooms[{index}]"),
            )?,
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

/// The room's `light_effect` name resolved case-insensitively against
/// [`LightEffect::name`]; unset asks for no effect.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `path` for a name that is not
/// one.
fn parse_light_effect(name: Option<&str>, path: &str) -> Result<Option<LightEffect>, CyberError> {
    let Some(text) = name else {
        return Ok(None);
    };
    let found = LightEffect::all()
        .into_iter()
        .find(|effect| effect.name().eq_ignore_ascii_case(text));
    found.map_or_else(
        || {
            Err(invalid_at(
                path,
                &format!(
                    "light effect {text:?} is not one: pass none, flicker, strobe_fast, \
                     strobe_slow, glow or random_off"
                ),
            ))
        },
        |effect| Ok(Some(effect)),
    )
}

/// The required wire field of a feature row, refused at `path` with the
/// `what`-worded instruction when the row left it out.
fn required(value: Option<&str>, path: &str, what: &str) -> Result<String, CyberError> {
    value
        .map(str::to_owned)
        .ok_or_else(|| invalid_at(path, what))
}

impl FeatureParam {
    /// The model feature, its variant resolved from the wire kind name.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming `features[index]` for an
    /// unknown kind, a row missing the room fields its kind needs, or
    /// a bad style or wall name.
    fn to_model(&self, index: usize) -> Result<Feature, CyberError> {
        let path = format!("features[{index}]");
        match self.kind.to_ascii_lowercase().as_str() {
            "exit" => self.exit_feature(&path),
            "switch" => self.switch_feature(&path),
            "lift" => self.lift_feature(&path),
            "teleporter" => self.teleporter_feature(&path),
            other => Err(invalid_at(
                &path,
                &format!(
                    "kind {other:?} is not a feature kind: pass one of exit, switch, lift, \
                     teleporter"
                ),
            )),
        }
    }

    /// The `exit` row: its room, style, secret flag and wall.
    fn exit_feature(&self, path: &str) -> Result<Feature, CyberError> {
        Ok(Feature::Exit {
            room: required(
                self.room.as_deref(),
                path,
                "an exit feature needs a room: the id of the room it stands in",
            )?,
            style: self.exit_style(path)?,
            secret: self.secret,
            wall: self.side(path)?,
        })
    }

    /// The `switch` row: its room, the connection it opens, its wall
    /// and whether it repeats.
    fn switch_feature(&self, path: &str) -> Result<Feature, CyberError> {
        Ok(Feature::Switch {
            room: required(
                self.room.as_deref(),
                path,
                "a switch feature needs a room: the id of the room it is mounted in",
            )?,
            opens: required(
                self.opens.as_deref(),
                path,
                "a switch feature needs opens: the id of the connection it operates",
            )?,
            wall: self.side(path)?,
            repeatable: self.repeatable.unwrap_or(DEFAULT_SWITCH_REPEATABLE),
        })
    }

    /// The `lift` row: its room, low floor, style and whether the ride
    /// repeats.
    fn lift_feature(&self, path: &str) -> Result<Feature, CyberError> {
        Ok(Feature::Lift {
            room: required(
                self.room.as_deref(),
                path,
                "a lift feature needs a room: the id of the room the platform stands in",
            )?,
            low_floor: self.low_floor,
            style: self.lift_style(path)?,
            repeatable: self.repeatable.unwrap_or(DEFAULT_LIFT_REPEATABLE),
        })
    }

    /// The `teleporter` row: its two rooms, squares, arrival facing and
    /// return leg.
    fn teleporter_feature(&self, path: &str) -> Result<Feature, CyberError> {
        Ok(Feature::Teleporter {
            from_room: required(
                self.from_room.as_deref(),
                path,
                "a teleporter feature needs a from_room: the id of the room the pad stands in",
            )?,
            to_room: required(
                self.to_room.as_deref(),
                path,
                "a teleporter feature needs a to_room: the id of the room the destination \
                 stands in",
            )?,
            pad: self.pad.map(<(i64, i64)>::from),
            destination: self.destination.map(<(i64, i64)>::from),
            angle: self.angle.unwrap_or(DEFAULT_ANGLE),
            two_way: self.two_way,
        })
    }

    /// The [`ExitStyle`] the `style` name spells (`switch` by default).
    fn exit_style(&self, path: &str) -> Result<ExitStyle, CyberError> {
        let wanted = self
            .style
            .as_deref()
            .unwrap_or(DEFAULT_EXIT_STYLE)
            .to_ascii_lowercase();
        match wanted.as_str() {
            "switch" => Ok(ExitStyle::Switch),
            "walk_over" => Ok(ExitStyle::WalkOver),
            other => Err(invalid_at(
                path,
                &format!("style {other:?} is not an exit style: pass one of switch, walk_over"),
            )),
        }
    }

    /// The [`LiftStyle`] the `style` name spells (`use_wall` by
    /// default).
    fn lift_style(&self, path: &str) -> Result<LiftStyle, CyberError> {
        let wanted = self
            .style
            .as_deref()
            .unwrap_or(DEFAULT_LIFT_STYLE)
            .to_ascii_lowercase();
        match wanted.as_str() {
            "use_wall" => Ok(LiftStyle::UseWall),
            "walk_trigger" => Ok(LiftStyle::WalkTrigger),
            other => Err(invalid_at(
                path,
                &format!("style {other:?} is not a lift style: pass one of use_wall, walk_trigger"),
            )),
        }
    }

    /// The optional `wall` [`Side`], case-insensitively.
    fn side(&self, path: &str) -> Result<Option<Side>, CyberError> {
        let Some(text) = self.wall.as_deref() else {
            return Ok(None);
        };
        let side = match text.to_ascii_lowercase().as_str() {
            "north" => Side::North,
            "south" => Side::South,
            "east" => Side::East,
            "west" => Side::West,
            other => {
                return Err(invalid_at(
                    path,
                    &format!(
                        "wall {other:?} is not a side: pass one of north, south, east, west, \
                         or leave wall out"
                    ),
                ));
            }
        };
        Ok(Some(side))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{BuildPlanParams, DEFAULT_WIDTH, KindSource};
    use crate::plan::model::{
        ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Plan, Shape, Side,
    };
    use crate::plan::things::{Skill, ThingSpot};
    use crate::plan::{Units, transform_plan};
    use crate::wad::map::features::{LiftStyle, LightEffect};

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

    /// A request with `body` spliced in as its `features`.
    fn features_request(body: &str) -> BuildPlanParams {
        let text = format!(
            r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": [], "features": {body}}}"#
        );
        from_str(&text).expect("the request parses")
    }

    #[test]
    fn every_feature_variant_lands_in_the_model_with_its_explicit_fields() {
        let args = features_request(
            r#"[
                {"kind": "exit", "room": "a", "style": "WALK_OVER", "secret": true, "wall": "North"},
                {"kind": "switch", "room": "a", "opens": "d1", "wall": "west", "repeatable": true},
                {"kind": "lift", "room": "a", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
                {"kind": "teleporter", "from_room": "a", "to_room": "a", "pad": [40, 40],
                 "destination": [120, 8], "angle": 90, "two_way": true}
            ]"#,
        );
        let plan = args.to_model().expect("all four variants convert");
        assert_eq!(
            plan.features.first().expect("four features"),
            &Feature::Exit {
                room: "a".to_owned(),
                style: ExitStyle::WalkOver,
                secret: true,
                wall: Some(Side::North),
            },
            "kind, style and wall names are case-insensitive"
        );
        assert_eq!(
            plan.features.get(1).expect("four features"),
            &Feature::Switch {
                room: "a".to_owned(),
                opens: "d1".to_owned(),
                wall: Some(Side::West),
                repeatable: true,
            },
            "the switch row"
        );
        assert_eq!(
            plan.features.get(2).expect("four features"),
            &Feature::Lift {
                room: "a".to_owned(),
                low_floor: Some(-64_i32),
                style: LiftStyle::WalkTrigger,
                repeatable: false,
            },
            "the lift row"
        );
        assert_eq!(
            plan.features.get(3).expect("four features"),
            &Feature::Teleporter {
                from_room: "a".to_owned(),
                to_room: "a".to_owned(),
                pad: Some((40_i64, 40_i64)),
                destination: Some((120_i64, 8_i64)),
                angle: 90_i32,
                two_way: true,
            },
            "the teleporter row"
        );
    }

    #[test]
    fn feature_rows_fill_their_documented_defaults() {
        let args = features_request(
            r#"[
                {"kind": "exit", "room": "a"},
                {"kind": "switch", "room": "a", "opens": "d1"},
                {"kind": "lift", "room": "a"},
                {"kind": "teleporter", "from_room": "a", "to_room": "a"}
            ]"#,
        );
        let plan = args.to_model().expect("the defaults fill in");
        assert_eq!(
            plan.features.first().expect("four features"),
            &Feature::Exit {
                room: "a".to_owned(),
                style: ExitStyle::Switch,
                secret: false,
                wall: None,
            },
            "the exit defaults"
        );
        assert_eq!(
            plan.features.get(1).expect("four features"),
            &Feature::Switch {
                room: "a".to_owned(),
                opens: "d1".to_owned(),
                wall: None,
                repeatable: false,
            },
            "a switch defaults to one use"
        );
        assert_eq!(
            plan.features.get(2).expect("four features"),
            &Feature::Lift {
                room: "a".to_owned(),
                low_floor: None,
                style: LiftStyle::UseWall,
                repeatable: true,
            },
            "a lift defaults to use_wall and repeating"
        );
        assert_eq!(
            plan.features.get(3).expect("four features"),
            &Feature::Teleporter {
                from_room: "a".to_owned(),
                to_room: "a".to_owned(),
                pad: None,
                destination: None,
                angle: 0_i32,
                two_way: false,
            },
            "the teleporter defaults"
        );
    }

    #[test]
    fn a_missing_features_list_stays_empty() {
        let args = request_with("");
        let plan = args.to_model().expect("the plan converts");
        assert!(plan.features.is_empty(), "features default to nothing");
    }

    #[test]
    fn feature_refusals_name_their_row_and_offer_the_vocabulary() {
        for (row, needle) in [
            (
                r#"{"kind": "trap", "room": "a"}"#,
                "exit, switch, lift, teleporter",
            ),
            (r#"{"kind": "exit"}"#, "needs a room"),
            (r#"{"kind": "lift"}"#, "needs a room"),
            (r#"{"kind": "switch", "room": "a"}"#, "needs opens"),
            (
                r#"{"kind": "teleporter", "to_room": "a"}"#,
                "needs a from_room",
            ),
            (
                r#"{"kind": "teleporter", "from_room": "a"}"#,
                "needs a to_room",
            ),
            (
                r#"{"kind": "exit", "room": "a", "style": "lever"}"#,
                "switch, walk_over",
            ),
            (
                r#"{"kind": "lift", "room": "a", "style": "jump"}"#,
                "use_wall, walk_trigger",
            ),
            (
                r#"{"kind": "exit", "room": "a", "wall": "up"}"#,
                "north, south, east, west",
            ),
        ] {
            let args = features_request(&format!("[{row}]"));
            let message = refuse(&args.to_model().expect_err("the row is refused"));
            assert!(message.contains("features[0]"), "{message} names the row");
            assert!(message.contains(needle), "{message} names {needle}");
        }
    }

    #[test]
    fn a_rooms_secret_and_light_effect_land_in_the_model() {
        let args = rooms_request(
            r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64},
                "secret": true, "light_effect": "GLOW"}]"#,
        );
        let plan = args.to_model().expect("the room converts");
        let room = plan.rooms.first().expect("one room");
        assert!(room.secret, "the secret flag arrives");
        assert_eq!(
            room.light_effect,
            Some(LightEffect::Glow),
            "case-insensitive"
        );
        let unset = rooms_request(r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}]"#);
        let defaults = unset.to_model().expect("the plain room converts");
        let bare = defaults.rooms.first().expect("one room");
        assert!(!bare.secret, "the default room is no secret");
        assert_eq!(bare.light_effect, None, "no effect asks for nothing");
        let bad = rooms_request(
            r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64},
                "light_effect": "pulse"}]"#,
        );
        let message = refuse(&bad.to_model().expect_err("no such effect"));
        assert!(message.contains("rooms[0]"), "{message} names the room");
        assert!(
            message.contains("flicker, strobe_fast, strobe_slow, glow or random_off"),
            "the refusal lists the effects: {message}"
        );
    }

    /// A request with `extra` spliced in beside the required fields.
    fn request_with(extra: &str) -> BuildPlanParams {
        let text = format!(
            r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": []{extra}}}"#
        );
        from_str(&text).expect("the request parses")
    }

    #[test]
    fn omitted_units_stay_the_identity_and_grid_defaults_to_eight() {
        let args = request_with("");
        assert_eq!(args.grid, 8_i64, "the documented snap grid");
        assert_eq!(
            args.to_units(),
            Units {
                grid: 8_i64,
                ..Units::default()
            },
            "identity scale and origins, the request grid carried in"
        );
        let explicit_null = request_with(r#", "units": null"#);
        assert_eq!(
            explicit_null.to_units(),
            Units {
                grid: 8_i64,
                ..Units::default()
            },
            "an explicit null units object is the identity too"
        );
    }

    #[test]
    fn a_partial_units_object_fills_the_documented_defaults() {
        let args = request_with(r#", "units": {"scale_num": 4, "flip_y": true}"#);
        assert_eq!(
            args.to_units(),
            Units {
                scale_num: 4_i64,
                scale_den: 1_i64,
                origin_x: 0_i64,
                origin_y: 0_i64,
                grid: 8_i64,
                flip_y: true,
            },
            "the sketch recipe: unset fields keep their defaults"
        );
    }

    #[test]
    fn a_full_units_object_and_grid_lands_on_the_transform() {
        let args = request_with(
            r#", "units": {"scale_num": 2, "scale_den": 4, "origin_x": 10,
                 "origin_y": 20, "flip_y": true}, "grid": 16"#,
        );
        assert_eq!(args.grid, 16_i64, "the wire grid");
        let units = args.to_units();
        assert_eq!(
            units,
            Units {
                scale_num: 2_i64,
                scale_den: 4_i64,
                origin_x: 10_i64,
                origin_y: 20_i64,
                grid: 16_i64,
                flip_y: true,
            }
        );
        let moved = transform_plan(
            &args.to_model().expect("the plan converts"),
            &units,
            args.grid,
        )
        .expect("the wired transform runs");
        assert_eq!(
            moved.rooms.first().expect("the room").shape,
            Shape::Rect {
                x: 16,
                y: -16,
                w: 32,
                h: 32
            },
            "half scale, mirrored y, 16 grid: 10 snaps to 16, 42 to 48, 20 to 16, -12 to -16"
        );
    }

    #[test]
    fn the_grid_snaps_untransformed_coordinates_too() {
        let text = r#"{"map": "MAP01",
            "rooms": [{"id": "a", "rect": {"x": 3, "y": 0, "w": 10, "h": 8}}],
            "connections": []}"#;
        let args: BuildPlanParams = from_str(text).expect("the request parses");
        let moved = transform_plan(
            &args.to_model().expect("the plan converts"),
            &args.to_units(),
            args.grid,
        )
        .expect("the default 8 grid runs");
        assert_eq!(
            moved.rooms.first().expect("the room").shape,
            Shape::Rect {
                x: 0,
                y: 0,
                w: 16,
                h: 8
            },
            "identity scale still snaps: 3 goes to 0, 13 to 16"
        );
    }

    #[test]
    fn a_grid_aligned_plan_passes_the_default_snap_untouched() {
        let args = request_with("");
        let moved = transform_plan(
            &args.to_model().expect("the plan converts"),
            &args.to_units(),
            args.grid,
        )
        .expect("the default 8 grid runs");
        assert_eq!(
            moved.rooms.first().expect("the room").shape,
            Shape::Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64
            },
            "the existing grid-aligned plans compile exactly as before"
        );
    }

    #[test]
    fn unusable_units_are_refused_naming_the_parameter() {
        for (extra, needle) in [
            (r#", "units": {"scale_den": 0}"#, "scale_den"),
            (r#", "units": {"scale_num": 0}"#, "scale_num"),
            (r#", "grid": 0"#, "grid"),
            (r#", "grid": -8"#, "grid"),
        ] {
            let args = request_with(extra);
            let error = args
                .to_units()
                .check()
                .expect_err("the refusal must surface at the tool");
            assert_eq!(error.code(), "invalid_argument", "{extra}");
            assert!(error.to_string().contains(needle), "{error} names {needle}");
        }
    }

    /// A one-room request with `body` spliced in beside the required
    /// fields.
    fn plan_request(body: &str) -> BuildPlanParams {
        let text = format!(
            r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": []{body}}}"#
        );
        from_str(&text).expect("the request parses")
    }

    #[test]
    fn thing_rows_and_the_player_start_keep_their_wire_shape() {
        let args = plan_request(
            r#", "things": [
                {"type": 2035, "x": 40, "y": 40},
                {"name": "imp", "room": "a", "count": 4, "angle": 90,
                 "skills": ["easy", "HARD"], "multiplayer_only": true}
            ],
            "player_start": {"room": "a", "angle": 270}"#,
        );
        let rows = args.to_things().expect("both rows and the start convert");
        assert_eq!(rows.len(), 3, "things first, the player start last");
        let pinned = rows.first().expect("three rows");
        assert_eq!(pinned.kind, KindSource::Type(2035_u32));
        assert_eq!(pinned.spot, ThingSpot::Point((40_i64, 40_i64)));
        assert_eq!(pinned.count, 1_u32, "the documented count default");
        assert_eq!(pinned.angle, 0_i32, "the documented angle default");
        assert_eq!(pinned.skills, None, "unset skills mean every skill");
        assert!(!pinned.multiplayer_only);
        assert_eq!(pinned.path, "things[0]");
        let filled = rows.get(1).expect("three rows");
        assert_eq!(filled.kind, KindSource::Name("imp".to_owned()));
        assert_eq!(filled.spot, ThingSpot::Room { id: "a".to_owned() });
        assert_eq!(filled.count, 4_u32);
        assert_eq!(filled.angle, 90_i32);
        assert_eq!(
            filled.skills,
            Some(vec![Skill::Easy, Skill::Hard]),
            "skills are case-insensitive"
        );
        assert!(filled.multiplayer_only);
        let start = rows.get(2).expect("three rows");
        assert_eq!(start.kind, KindSource::PlayerOne);
        assert_eq!(
            start.spot,
            ThingSpot::RoomCentre { id: "a".to_owned() },
            "the room form centres"
        );
        assert_eq!(start.angle, 270_i32);
        assert_eq!(start.path, "player_start");
    }

    #[test]
    fn an_omitted_things_list_and_player_start_ask_for_a_start_at_the_tool() {
        let args = plan_request("");
        let rows = args.to_things().expect("nothing to convert");
        assert!(rows.is_empty(), "no rows to place means no rows to convert");
    }

    #[test]
    fn a_type_or_name_that_is_both_or_neither_names_its_row() {
        let both =
            plan_request(r#", "things": [{"type": 2035, "name": "barrel", "x": 8, "y": 8}]"#);
        let message = refuse(&both.to_things().expect_err("type and name are exclusive"));
        assert!(message.contains("things[0]"), "{message}");
        assert!(message.contains("exactly one"), "{message}");
        let neither = plan_request(r#", "things": [{"room": "a"}]"#);
        let bare = refuse(&neither.to_things().expect_err("no type or name"));
        assert!(bare.contains("things[0]"), "{bare}");
    }

    #[test]
    fn half_a_coordinate_or_a_room_and_a_point_names_its_row() {
        for body in [
            r#"{"type": 1, "x": 8, "room": "a"}"#,
            r#"{"type": 1, "y": 8}"#,
            r#"{"type": 1}"#,
            r#"{"type": 1, "x": 8, "y": 8, "room": "a"}"#,
        ] {
            let args = plan_request(&format!(r#", "things": [{body}]"#));
            let message = refuse(&args.to_things().expect_err("the row is malformed"));
            assert!(message.contains("things[0]"), "{message} names things[0]");
        }
    }

    #[test]
    fn angles_outside_the_compass_and_unknown_skills_are_refused() {
        for (extra, needle) in [
            (r#""x": 8, "y": 8, "angle": 360"#, "0..=359"),
            (r#""x": 8, "y": 8, "angle": -1"#, "0..=359"),
            (
                r#""room": "a", "skills": ["nightmare"]"#,
                "things[0].skills",
            ),
        ] {
            let args = plan_request(&format!(
                r#", "things": [{{"type": 2035, {extra}}}],
                    "player_start": {{"room": "a"}}"#
            ));
            let error = args.to_things().expect_err("the row is refused");
            assert_eq!(error.code(), "invalid_argument", "{extra}");
            assert!(error.to_string().contains(needle), "{error} names {needle}");
        }
    }

    #[test]
    fn the_player_start_shape_mirrors_the_thing_rows() {
        let point = plan_request(
            r#", "player_start": {"x": 16, "y": 24, "angle": 45},
                "things": [{"type": 1, "room": "a"}]"#,
        );
        let rows = point.to_things().expect("a pinned start converts");
        assert_eq!(
            rows.first().expect("two rows").spot,
            ThingSpot::Room { id: "a".to_owned() },
            "the thing rows keep their order"
        );
        assert_eq!(
            rows.get(1).expect("two rows").spot,
            ThingSpot::Point((16_i64, 24_i64))
        );
        assert_eq!(rows.get(1).expect("two rows").angle, 45_i32);
        for (start, needle) in [
            (r#"{"x": 8}"#, "player_start"),
            (r#"{"x": 8, "y": 8, "room": "a"}"#, "player_start"),
            ("{}", "player_start"),
            (r#"{"room": "a", "angle": 900}"#, "0..=359"),
        ] {
            let args = plan_request(&format!(r#", "player_start": {start}"#));
            let message = refuse(&args.to_things().expect_err("the start is malformed"));
            assert!(message.contains(needle), "{message} names {needle}");
        }
    }
}
