//! Wire parameters of [`Cyberdemon::build_map_from_plan`](super::Cyberdemon::build_map_from_plan)
//! and their one conversion onto [`crate::plan::model`] types.
//!
//! The wire shape mirrors the model flat: a room is `{id, rect | points,
//! …}`, a connection is `{from, to, kind, …}` with the kind's extra
//! fields at the top level, so the JSON an LLM writes reads like the
//! model it becomes. `to_model` only maps names and fills defaults
//! ([`crate::plan::model`]'s); every structural judgment — positive
//! edges, unique ids, touching rooms — stays with [`Plan::validate`] and
//! the compiler, whose refusals [`super::build::with_plan_path`] then prefixes
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
//! [`ConnectionParam`] keeps a [`crate::plan::model::ConnectionKind`]: one
//! `kind` name plus the kind's extra fields beside it, mapped onto the
//! tagged [`Feature`] by [`BuildPlanParams::to_model`]. The reference
//! checks a feature needs (known rooms, a switch's `opens`) stay with
//! [`Plan::validate`].
//!
//! The rows live in their own modules: [`RoomParam`] and
//! [`ConnectionParam`] in `rooms`, [`FeatureParam`] in `features`,
//! [`ThingParam`], [`PlayerStartParam`] and [`ThingRequest`] in `things`.
//! This root keeps the request itself, [`UnitsParam`], the wire defaults
//! and the conversion that walks the three row kinds.

mod features;
mod rooms;
mod things;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::named::{Named, parse_named};
use crate::plan::Units;
use crate::plan::model::{Connection, Feature, Plan, Room};
pub(in crate::tools::plan) use features::FeatureParam;
pub(in crate::tools::plan) use rooms::ConnectionParam;
use rooms::RoomParam;
pub(in crate::tools::plan) use things::{KindSource, ThingRequest};
use things::{PlayerStartParam, ThingParam};

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
/// The [`crate::plan::model::ConnectionKind::Window`] sill the wire default carries.
const DEFAULT_SILL: i32 = 32_i32;
/// The [`crate::plan::model::ConnectionKind::Stairs`] step height the wire default carries.
const DEFAULT_STEP_HEIGHT: i32 = 16_i32;
/// The [`crate::plan::model::ConnectionKind::Stairs`] step depth the wire default carries.
const DEFAULT_STEP_DEPTH: i32 = 24_i32;
/// The [`crate::plan::model::DoorStyle`] a plain `"door"` connection asks for.
const DEFAULT_DOOR_STYLE: &str = "normal";
/// The [`crate::plan::model::ExitStyle`] a plain `"exit"` feature row asks for.
const DEFAULT_EXIT_STYLE: &str = "switch";
/// The [`crate::wad::map::features::LiftStyle`] a plain `"lift"` feature row asks for.
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
    /// validate, and [`crate::plan::features`] installs them on the
    /// compiled map in fixed pipeline order (doors, switches, lifts,
    /// teleporters, exits, lights, secrets).
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
    /// Unknown names fail `invalid_argument` with the `theme` plan path
    /// leading, listing the themes the session's game offers (see
    /// `get_texture_theme`); the session itself never changes.
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

/// One wire `units` object: the coordinate half of [`Units`] as `JSON`.
/// The snap grid arrives beside it as the request's top-level `grid`;
/// [`BuildPlanParams::to_units`] carries it in. `render_topdown`'s
/// `underlay.units` is the same object, so a sketch and the map built
/// from it line up exactly.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(default)]
pub(in crate::tools) struct UnitsParam {
    /// Numerator of the scale: `4` with `scale_den: 1` makes one plan
    /// unit four map units (default `1`; `0` is refused).
    pub(in crate::tools) scale_num: i64,
    /// Denominator of the scale (default `1`; `0` is refused).
    pub(in crate::tools) scale_den: i64,
    /// Map `x` the plan origin lands on (default `0`).
    pub(in crate::tools) origin_x: i64,
    /// Map `y` the plan origin lands on (default `0`) and the line
    /// `flip_y` mirrors about.
    pub(in crate::tools) origin_y: i64,
    /// Mirror `y` about `origin_y` (default `false`): set it for
    /// image-space drawings, whose rows grow down while map `y` grows up.
    pub(in crate::tools) flip_y: bool,
}

impl UnitsParam {
    /// The [`Units`] this wire object stands for under the caller's snap
    /// `grid`: the one field the `units` object does not carry. The values
    /// themselves are only *checked* by [`Units::check`]; this mapping
    /// cannot fail.
    #[must_use]
    pub(in crate::tools) const fn to_units(self, grid: i64) -> Units {
        Units {
            scale_num: self.scale_num,
            scale_den: self.scale_den,
            origin_x: self.origin_x,
            origin_y: self.origin_y,
            grid,
            flip_y: self.flip_y,
        }
    }
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

/// The `invalid_argument` naming the plan path `at`.
fn invalid_at(at: &str, problem: &str) -> CyberError {
    CyberError::InvalidArgument(format!("{at}: {problem}"))
}

/// The [`Named`] value a plan row's `field` names, its refusal attributed
/// to the row path `at` so the caller can see which row asked.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`]: `{at}: ` + [`parse_named`]'s uniform
/// refusal listing the accepted names.
fn named_at<T: Named>(at: &str, field: &str, text: &str) -> Result<T, CyberError> {
    parse_named::<T>(field, text).map_err(|error| invalid_at(at, &error.message()))
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
        self.units.unwrap_or_default().to_units(self.grid)
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

/// The required wire field of a feature row, refused at `path` with the
/// `what`-worded instruction when the row left it out.
fn required(value: Option<&str>, path: &str, what: &str) -> Result<String, CyberError> {
    value
        .map(str::to_owned)
        .ok_or_else(|| invalid_at(path, what))
}

#[cfg(test)]
mod tests;
