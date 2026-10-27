//! The wire `features` rows: [`FeatureParam`], one `kind` name plus that
//! kind's extra fields beside it, mapped onto the tagged [`Feature`] by
//! [`super::BuildPlanParams`]'s `to_features`. The reference checks a
//! feature needs (known rooms, a switch's `opens`) stay with
//! [`crate::plan::model::Plan::validate`], not here.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    DEFAULT_ANGLE, DEFAULT_EXIT_STYLE, DEFAULT_LIFT_REPEATABLE, DEFAULT_LIFT_STYLE,
    DEFAULT_SWITCH_REPEATABLE, invalid_at, named_at, required,
};
use crate::error::CyberError;
use crate::geom::Pt;
use crate::plan::model::{ExitStyle, Feature, Side};
use crate::wad::map::features::LiftStyle;

/// One wire feature row: a `kind` name plus that kind's extra fields
/// beside it. The reference checks (known rooms, an `opens` that names
/// a connection) belong to [`Plan::validate`], not here.
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::plan) struct FeatureParam {
    /// Which feature: `"exit"`, `"switch"`, `"lift"` or `"teleporter"`
    /// (case-insensitive).
    pub(in crate::tools::plan) kind: String,
    /// Exit, switch and lift only: the id of the room the feature
    /// stands in.
    #[serde(default)]
    pub(in crate::tools::plan) room: Option<String>,
    /// Exit or lift only: the exit's style, `switch` (default) or
    /// `walk_over`; the lift's style, `use_wall` (default) or
    /// `walk_trigger`. Case-insensitive.
    #[serde(default)]
    pub(in crate::tools::plan) style: Option<String>,
    /// Exit only: whether the room is a secret area (default `false`).
    #[serde(default)]
    pub(in crate::tools::plan) secret: bool,
    /// Exit and switch only: which wall carries it — `"north"`,
    /// `"south"`, `"east"` or `"west"` (case-insensitive; unset takes
    /// the room's longest one-sided wall).
    #[serde(default)]
    pub(in crate::tools::plan) wall: Option<String>,
    /// Exit and switch only: the plan point `[x, y]` that picks the wall
    /// when several match `wall` (transformed and snapped with the rooms;
    /// the nearest matching wall wins and the panel centres on the point).
    /// Unset takes the longest matching wall. A `walk_over` exit dresses
    /// every boundary and reads neither `wall` nor `at`.
    #[serde(default)]
    pub(in crate::tools::plan) at: Option<[i64; 2]>,
    /// Switch only: the `id` of the connection this switch operates.
    #[serde(default)]
    pub(in crate::tools::plan) opens: Option<String>,
    /// Switch and lift only: whether the action keeps working after
    /// use (default `false` for a switch, `true` for a lift).
    #[serde(default)]
    pub(in crate::tools::plan) repeatable: Option<bool>,
    /// Lift only: the floor the platform lowers to, map units (the
    /// lowest neighbouring floor by default).
    #[serde(default)]
    pub(in crate::tools::plan) low_floor: Option<i32>,
    /// Teleporter only: the id of the room the pad stands in.
    #[serde(default)]
    pub(in crate::tools::plan) from_room: Option<String>,
    /// Teleporter only: the id of the room the destination stands in.
    #[serde(default)]
    pub(in crate::tools::plan) to_room: Option<String>,
    /// Teleporter only: the pad's centre as a plan point `[x, y]`
    /// (transformed and snapped with the rooms; the room centre by
    /// default).
    #[serde(default)]
    pub(in crate::tools::plan) pad: Option<[i64; 2]>,
    /// Teleporter only: the destination's centre as a plan point
    /// `[x, y]` (transformed and snapped with the rooms; the room
    /// centre by default).
    #[serde(default)]
    pub(in crate::tools::plan) destination: Option<[i64; 2]>,
    /// Teleporter only: arrival facing in degrees (default `0`).
    #[serde(default)]
    pub(in crate::tools::plan) angle: Option<i32>,
    /// Teleporter only: whether the destination teleports back
    /// (default `false`).
    #[serde(default)]
    pub(in crate::tools::plan) two_way: bool,
}

impl FeatureParam {
    /// The model feature, its variant resolved from the wire kind name.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming `features[index]` for an
    /// unknown kind, a row missing the room fields its kind needs, or
    /// a bad style or wall name.
    pub(super) fn to_model(&self, index: usize) -> Result<Feature, CyberError> {
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

    /// The `exit` row: its room, style, secret flag, wall and wall point.
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
            at: self.at.map(Pt::from),
        })
    }

    /// The `switch` row: its room, the connection it opens, its wall
    /// with the point that picks it, and whether it repeats.
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
            at: self.at.map(Pt::from),
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
            pad: self.pad.map(Pt::from),
            destination: self.destination.map(Pt::from),
            angle: self.angle.unwrap_or(DEFAULT_ANGLE),
            two_way: self.two_way,
        })
    }

    /// The [`ExitStyle`] the `style` name spells (`switch` by default).
    fn exit_style(&self, path: &str) -> Result<ExitStyle, CyberError> {
        named_at(
            path,
            "style",
            self.style.as_deref().unwrap_or(DEFAULT_EXIT_STYLE),
        )
    }

    /// The [`LiftStyle`] the `style` name spells (`use_wall` by
    /// default).
    fn lift_style(&self, path: &str) -> Result<LiftStyle, CyberError> {
        named_at(
            path,
            "style",
            self.style.as_deref().unwrap_or(DEFAULT_LIFT_STYLE),
        )
    }

    /// The optional `wall` [`Side`], case-insensitively.
    fn side(&self, path: &str) -> Result<Option<Side>, CyberError> {
        let Some(text) = self.wall.as_deref() else {
            return Ok(None);
        };
        named_at(path, "wall", text).map(Some)
    }
}
