//! The wire `things` rows and the `player_start`: [`ThingParam`] and
//! [`PlayerStartParam`] shape-validated into [`ThingRequest`] rows under
//! their wire paths. The doomednum behind a [`KindSource::Name`], the
//! presence flags and the placement itself need the session and the
//! compiled map, so they live in [`super`] and [`crate::plan::things`].

use schemars::JsonSchema;
use serde::Deserialize;

use super::{DEFAULT_COUNT, default_count, invalid_at};
use crate::error::CyberError;
use crate::plan::things::{Skill, ThingSpot};

/// One wire thing: a type or name, a pinned point or a room to fill,
/// and the placement details.
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::plan) struct ThingParam {
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
pub(in crate::tools::plan) struct PlayerStartParam {
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
pub(in crate::tools::plan) enum KindSource {
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
pub(in crate::tools::plan) struct ThingRequest {
    /// The type or the name to resolve.
    pub(in crate::tools::plan) kind: KindSource,
    /// Where the thing goes.
    pub(in crate::tools::plan) spot: ThingSpot,
    /// How many copies a room fill may place.
    pub(in crate::tools::plan) count: u32,
    /// Facing, validated to `0..=359`.
    pub(in crate::tools::plan) angle: i32,
    /// The requested skills (`None` means every skill).
    pub(in crate::tools::plan) skills: Option<Vec<Skill>>,
    /// Whether the thing stays out of single-player.
    pub(in crate::tools::plan) multiplayer_only: bool,
    /// The wire path error messages name.
    pub(in crate::tools::plan) path: String,
}

impl ThingParam {
    /// The validated request row for the `things[index]` path.
    pub(super) fn to_request(&self, index: usize) -> Result<ThingRequest, CyberError> {
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
    pub(super) fn to_request(&self) -> Result<ThingRequest, CyberError> {
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
