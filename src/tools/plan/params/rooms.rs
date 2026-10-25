//! The wire `rooms` and `connections` rows: [`RoomParam`] with the
//! [`RectParam`] or point-ring footprint it turns into a [`Shape`],
//! [`ConnectionParam`] with its `kind` name resolved to a
//! [`ConnectionKind`], and the room's `light_effect` name resolved to a
//! [`LightEffect`]. Every `to_model` answers under its own plan path
//! (`rooms[2]`, `connections[0]`); the structural judgments — positive
//! edges, unique ids, touching rooms — stay with the model, as
//! [`super::BuildPlanParams`]'s overview spells out.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    DEFAULT_CEILING, DEFAULT_DOOR_STYLE, DEFAULT_FLOOR, DEFAULT_LIGHT, DEFAULT_SILL,
    DEFAULT_STEP_DEPTH, DEFAULT_STEP_HEIGHT, DEFAULT_WIDTH, invalid_at,
};
use crate::error::CyberError;
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, KeyColor, Room, Shape};
use crate::wad::map::consts::DEFAULT_LINTEL;
use crate::wad::map::features::LightEffect;

/// One wire room: an id, exactly one of `rect`/`points`, and the sector
/// properties its interior carries.
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::plan) struct RoomParam {
    /// Name, unique in the plan, that connections refer to.
    pub(in crate::tools::plan) id: String,
    /// Axis-aligned rectangle `{x, y, w, h}` from `(x, y)` to
    /// `(x + w, y + h)`. Exactly one of `rect` and `points`.
    #[serde(default)]
    pub(in crate::tools::plan) rect: Option<RectParam>,
    /// A closed ring of `[x, y]` points, implicitly closing to the
    /// first; at least three distinct ones. Exactly one of `rect` and
    /// `points`.
    #[serde(default)]
    pub(in crate::tools::plan) points: Option<Vec<[i64; 2]>>,
    /// Floor height of the room's sector (default `0`).
    #[serde(default)]
    pub(in crate::tools::plan) floor: Option<i32>,
    /// Ceiling height of the room's sector (default `128`).
    #[serde(default)]
    pub(in crate::tools::plan) ceiling: Option<i32>,
    /// Light level of the room's sector (default `160`).
    #[serde(default)]
    pub(in crate::tools::plan) light: Option<i32>,
    /// Flat name for the floor; the theme's `floor` role answers when
    /// unset.
    #[serde(default)]
    pub(in crate::tools::plan) floor_flat: Option<String>,
    /// Flat name for the ceiling; the theme's `ceiling` role answers
    /// when unset (an `outdoor` room gets the sky instead).
    #[serde(default)]
    pub(in crate::tools::plan) ceiling_flat: Option<String>,
    /// Texture name for the walls; the theme's `wall` role answers when
    /// unset.
    #[serde(default)]
    pub(in crate::tools::plan) wall_texture: Option<String>,
    /// Whether the room is open to the sky (default `false`).
    #[serde(default)]
    pub(in crate::tools::plan) outdoor: bool,
    /// Whether the room counts as a secret area (default `false`). The
    /// feature run's secret step flags the room's sectors with the
    /// family's secret special, after the lights step.
    #[serde(default)]
    pub(in crate::tools::plan) secret: bool,
    /// A light effect for the room's sector: `"none"`, `"flicker"`,
    /// `"strobe_fast"`, `"strobe_slow"`, `"glow"` or `"random_off"`
    /// (case-insensitive; unset keeps the room's static `light`). The
    /// feature run's lights step installs it on every sector of the
    /// room, keeping the light level.
    #[serde(default)]
    pub(in crate::tools::plan) light_effect: Option<String>,
}

/// One wire `rect`: the axis-aligned rectangle from `(x, y)` to
/// `(x + w, y + h)`.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
pub(in crate::tools::plan) struct RectParam {
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
pub(in crate::tools::plan) struct ConnectionParam {
    /// Optional name used in reports and messages; defaults to the
    /// `from->to` label.
    #[serde(default)]
    pub(in crate::tools::plan) id: Option<String>,
    /// Id of the room the connection starts from.
    pub(in crate::tools::plan) from: String,
    /// Id of the room the connection leads to.
    pub(in crate::tools::plan) to: String,
    /// What kind of opening it is: `"opening"`, `"door"`, `"window"` or
    /// `"stairs"` (case-insensitive).
    pub(in crate::tools::plan) kind: String,
    /// Where to place the opening, `[x, y]` (default: centred on the
    /// shared wall).
    #[serde(default)]
    pub(in crate::tools::plan) at: Option<[i64; 2]>,
    /// Clear width of the opening, map units (default `64`).
    #[serde(default)]
    pub(in crate::tools::plan) width: Option<i64>,
    /// Door only: `normal` (default), `fast`, `stay_open`,
    /// `locked_red`, `locked_blue`, `locked_yellow` or `remote`. It
    /// picks the door's action (installed on the band's room-facing
    /// lines) and its dressing.
    #[serde(default)]
    pub(in crate::tools::plan) style: Option<String>,
    /// Window only: height of the glass bottom above the lower floor
    /// (default `32`).
    #[serde(default)]
    pub(in crate::tools::plan) sill: Option<i32>,
    /// Window only: height of the glass top below the higher ceiling
    /// (default `24`).
    #[serde(default)]
    pub(in crate::tools::plan) lintel: Option<i32>,
    /// Stairs only: height of each step (default `16`, clamped to
    /// `1..=24`).
    #[serde(default)]
    pub(in crate::tools::plan) step_height: Option<i32>,
    /// Stairs only: depth of each step, map units; must be positive
    /// (default `24`).
    #[serde(default)]
    pub(in crate::tools::plan) step_depth: Option<i32>,
}

impl RoomParam {
    /// The model room: the footprint exactly one of `rect`/`points`
    /// gives, the sector properties or their defaults.
    pub(super) fn to_model(&self, index: usize) -> Result<Room, CyberError> {
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
    pub(super) fn to_model(&self, index: usize) -> Result<Connection, CyberError> {
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
