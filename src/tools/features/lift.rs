//! `add_lift`: a platform floor that drops, waits and rises back.
//!
//! `add_lift` runs the pure [`crate::wad::map::features::lift`] core as
//! one [`crate::session::Session::edit_map`] step: the platform is an
//! existing sector or a new one drawn from its polygon with
//! [`crate::wad::map::overlay::apply_overlay`]
//! [`crate::wad::map::overlay::Paint::Sector`] (copying the sector under
//! its centre), [`crate::wad::map::features::build_lift`] finds its
//! edges that face a lower neighbour, turns each so its front looks down
//! at the player who triggers the ride, stamps a fresh
//! [`crate::wad::map::features::next_free_tag`] on the sector and
//! installs the [`crate::wad::map::features::lift_specials`]
//! down-wait-up action on those lines. The platform's face and floor
//! wear the theme's `lift_side` and `lift_floor` roles
//! ([`crate::tools::themes::theme_texture`],
//! [`crate::tools::maps::params::validate_texture_name`]), and a drop
//! under [`crate::wad::map::features::MIN_LIFT_DROP`] is reported as a
//! note (use stairs) rather than a refusal.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{edit_result_fragment, validate_texture_name};
use crate::tools::themes::theme_texture;
use crate::wad::classify::MapFormat;
use crate::wad::map::features::{LiftBuild, LiftPlatform, LiftSpec, LiftStyle, build_lift};

use super::{current_map, invalid};
use crate::tools::maps::params::PointParam;

/// The `add_lift` default trigger style name.
const DEFAULT_LIFT_STYLE: &str = "use_wall";

/// Arguments for [`Cyberdemon::add_lift`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddLiftParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The outline of a new platform to draw, `{points: [{x, y}, ...]}`:
    /// at least three corners, implicitly closed, and the sector under
    /// its centre lends it the heights. Exactly one of `platform` and
    /// `sector`.
    #[serde(default)]
    pub(super) platform: Option<LiftPlatformParam>,
    /// The index of the existing platform sector. Exactly one of
    /// `platform` and `sector`.
    #[serde(default)]
    pub(super) sector: Option<usize>,
    /// The floor the platform lowers to in map units (optional: the
    /// lowest neighbouring floor; it must sit below the platform's
    /// floor).
    #[serde(default)]
    pub(super) low_floor: Option<i32>,
    /// How the ride is triggered from below (default `"use_wall"`,
    /// case-insensitive): `use_wall` presses the platform's side wall or
    /// `walk_trigger` crosses its edge.
    #[serde(default)]
    pub(super) style: Option<String>,
    /// Whether the activation lines keep working after use (default
    /// true: the `SR`/`WR` specials and `repeatspecial`; false picks the
    /// one-shot `S1`/`W1` specials and drops `repeatspecial`).
    #[serde(default = "default_lift_repeatable")]
    pub(super) repeatable: bool,
}

/// Default [`AddLiftParams::repeatable`]: a lift rides back up on its
/// own, so its lines repeat unless asked otherwise.
const fn default_lift_repeatable() -> bool {
    true
}

/// The `platform` parameter of [`Cyberdemon::add_lift`]: a new lift
/// platform's outline.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct LiftPlatformParam {
    /// The polygon's corners, `{x, y}`, in order (at least three; the
    /// last corner closes the ring).
    pub(super) points: Vec<PointParam>,
}

/// The theme-resolved textures a lift wears.
#[derive(Debug)]
struct LiftSkins {
    /// The platform's side: the `lower` band every activation edge shows
    /// the lower area.
    side: String,
    /// The platform's floor flat.
    floor_flat: String,
}

/// Structured result of [`Cyberdemon::add_lift`].
#[derive(Serialize)]
pub(super) struct LiftReport {
    /// What the step built, inlined next to the style echo and the edit
    /// fragment.
    #[serde(flatten)]
    built: LiftBuild,
    /// The canonical style name the lift got.
    style: &'static str,
    /// Whether the activation lines keep working after use.
    repeatable: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The canonical wire name of `style`.
pub(super) const fn lift_style_name(style: LiftStyle) -> &'static str {
    match style {
        LiftStyle::UseWall => "use_wall",
        LiftStyle::WalkTrigger => "walk_trigger",
    }
}

/// Every style name, in [`LiftStyle::all`] order.
pub(super) fn lift_style_names() -> Vec<&'static str> {
    LiftStyle::all().map(lift_style_name).to_vec()
}

/// The [`LiftStyle`] `text` names (case-insensitive), or the refusal
/// listing both.
pub(super) fn lift_style_of(text: &str) -> Result<LiftStyle, CyberError> {
    let wanted = text.to_ascii_lowercase();
    LiftStyle::all()
        .into_iter()
        .find(|style| lift_style_name(*style) == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "style {text:?} is not a lift style: pass one of {}",
                lift_style_names().join(", ")
            ))
        })
}

/// The `platform`/`sector` parameter pair as one [`LiftPlatform`]:
/// refuses unless exactly one of the two was passed.
pub(super) fn lift_platform_of(
    platform: Option<&LiftPlatformParam>,
    sector: Option<usize>,
) -> Result<LiftPlatform, CyberError> {
    match (platform, sector) {
        (Some(shape), None) => Ok(LiftPlatform::Draw(
            shape
                .points
                .iter()
                .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
                .collect(),
        )),
        (None, Some(index)) => Ok(LiftPlatform::Existing(index)),
        _ => Err(invalid(
            "pass exactly one of platform ({points: [{x, y}, ...]}: the outline of a \
             new platform to draw) and sector (the index of the existing platform \
             sector)",
        )),
    }
}

/// The textures a lift wears: the theme's `lift_side` and `lift_floor`
/// roles, validated for the map's format (the same resolution pattern
/// [`window_skins`] runs).
///
/// # Errors
///
/// [`theme_texture`]'s errors and [`validate_texture_name`]' refusals.
fn lift_skins(session: &Session, format: MapFormat) -> Result<LiftSkins, CyberError> {
    let side = theme_texture(session, Role::LiftSide)?;
    let floor_flat = theme_texture(session, Role::LiftFloor)?;
    Ok(LiftSkins {
        side: validate_texture_name(&side, format)?,
        floor_flat: validate_texture_name(&floor_flat, format)?,
    })
}

/// `add_lift`'s work: resolve the parameters, then run the whole lift as
/// one [`crate::session::Session::edit_map`] step.
pub(super) fn add_lift_run(
    session: &mut Session,
    args: &AddLiftParams,
) -> Result<LiftReport, CyberError> {
    let style = lift_style_of(args.style.as_deref().unwrap_or(DEFAULT_LIFT_STYLE))?;
    let platform = lift_platform_of(args.platform.as_ref(), args.sector)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = current_map(session, name)?.format;
    let skins = lift_skins(session, format)?;
    let spec = LiftSpec {
        platform,
        low_floor: args.low_floor,
        style,
        repeatable: args.repeatable,
        side: skins.side,
        floor_flat: skins.floor_flat,
    };
    let label = format!("add {} lift", lift_style_name(style));
    let (built, report) = session.edit_map(name, &label, |map| build_lift(map, &spec))?;
    Ok(LiftReport {
        built,
        style: lift_style_name(style),
        repeatable: args.repeatable,
        edit: edit_result_fragment(&report),
    })
}
