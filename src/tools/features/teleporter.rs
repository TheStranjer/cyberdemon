//! `add_teleporter`: a floor pad and the square it delivers to.
//!
//! `add_teleporter` runs the pure [`crate::wad::map::features::teleport`]
//! core as one [`crate::session::Session::edit_map`] step:
//! [`crate::wad::map::features::build_teleporter`] folds the pad and
//! destination squares over the map with two
//! [`crate::wad::map::overlay::apply_overlay`]
//! [`crate::wad::map::overlay::Paint::Sector`] paints (each copying the
//! sector under its centre, the pad's floor wearing the theme's
//! `teleport_floor` role), pushes a type-14 teleport-destination
//! [`crate::wad::map::Thing`] at the destination answering with a fresh
//! [`crate::wad::map::features::next_free_tag`] - through the tagged
//! destination sector in the `Doom` family, through the thing's own TID
//! in the `Hexen` one - and dresses every two-sided pad border with
//! [`crate::wad::map::features::teleport_specials`], fronts turned to
//! face out of the pad so players walking on from the surrounding rooms
//! make the jump. `two_way` runs the same pattern back with its own
//! tag/TID, all inside a single undo.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{edit_result_fragment, validate_texture_name};
use crate::tools::themes::{FALLBACK_WALL_TEXTURE, role_texture, theme_roles};
use crate::wad::classify::MapFormat;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::features::{DEFAULT_PAD_SIZE, TeleportBuild, TeleportSpec, build_teleporter};
use crate::wad::map::ops::Point;

use super::current_map;
use super::door::point_of;
use crate::tools::maps::params::PointParam;

/// Arguments for [`Cyberdemon::add_teleporter`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddTeleporterParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The centre of the pad the player walks onto, `{x, y}`.
    #[serde(default)]
    pub(super) pad: Option<PointParam>,
    /// The centre of the destination the player arrives in, `{x, y}`,
    /// with the facing `angle` in degrees (default 0).
    #[serde(default)]
    pub(super) destination: Option<TeleportDestinationParam>,
    /// The side length of both the pad and the destination square in map
    /// units (default 64).
    #[serde(default)]
    pub(super) pad_size: Option<i32>,
    /// Whether the destination sector leads back to the pad as well
    /// (default false).
    #[serde(default)]
    pub(super) two_way: bool,
}

/// The `destination` point of [`Cyberdemon::add_teleporter`]: where the
/// player arrives and which way they face there.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, JsonSchema)]
pub(super) struct TeleportDestinationParam {
    /// `X` coordinate in map units.
    pub(super) x: i32,
    /// `Y` coordinate in map units.
    pub(super) y: i32,
    /// Facing direction in degrees (default 0; any value is wrapped into
    /// `0..=359`).
    #[serde(default)]
    pub(super) angle: i32,
}

/// The theme-resolved textures a teleporter wears.
#[derive(Debug)]
pub(super) struct TeleportSkins {
    /// The pad's floor flat (kept as the base's when the theme names none).
    pub(super) floor_flat: String,
    /// The plain wall texture bare new edges fall back to.
    pub(super) wall: String,
}

/// Structured result of [`Cyberdemon::add_teleporter`].
#[derive(Serialize)]
pub(super) struct TeleporterReport {
    /// What the step built, inlined next to the size echo and the edit
    /// fragment.
    #[serde(flatten)]
    built: TeleportBuild,
    /// The square side both squares got, in map units.
    pad_size: i32,
    /// Whether the destination leads back.
    two_way: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The textures a teleporter wears: the theme's `teleport_floor` flat and
/// `wall` fallback in one [`theme_roles`] pass, validated for the map's
/// format (the same resolution pattern [`lift_skins`] runs).
///
/// # Errors
///
/// [`theme_roles`]' errors and [`validate_texture_name`]' refusals.
fn teleport_skins(session: &Session, format: MapFormat) -> Result<TeleportSkins, CyberError> {
    let wanted = [Role::TeleportFloor, Role::Wall];
    let resolved = theme_roles(session, None, &wanted)?;
    let floor_flat = role_texture(&resolved, Role::TeleportFloor, NO_TEXTURE);
    let wall = role_texture(&resolved, Role::Wall, FALLBACK_WALL_TEXTURE);
    Ok(TeleportSkins {
        floor_flat: validate_texture_name(&floor_flat, format)?,
        wall: validate_texture_name(&wall, format)?,
    })
}

/// The [`TeleportSpec`] the wire parameters and the theme resolve to; a
/// missing `pad` or `destination` is `invalid_argument` naming it.
pub(super) fn teleport_spec(
    args: &AddTeleporterParams,
    skins: &TeleportSkins,
) -> Result<TeleportSpec, CyberError> {
    let pad_param = args.pad.ok_or_else(|| {
        CyberError::invalid("pad is required: the {x, y} centre of the pad the player walks onto")
    })?;
    let wanted = args.destination.as_ref().ok_or_else(|| {
        CyberError::invalid(
            "destination is required: the {x, y, angle} centre of the sector the \
             player arrives in",
        )
    })?;
    Ok(TeleportSpec {
        pad: point_of(pad_param),
        destination: Point {
            x: wanted.x,
            y: wanted.y,
        },
        angle: wanted.angle,
        pad_size: args.pad_size.unwrap_or(DEFAULT_PAD_SIZE),
        two_way: args.two_way,
        floor_flat: skins.floor_flat.clone(),
        wall: skins.wall.clone(),
    })
}

/// `add_teleporter`'s work: resolve the parameters, then run the whole
/// teleporter as one [`crate::session::Session::edit_map`] step.
pub(super) fn add_teleporter_run(
    session: &mut Session,
    args: &AddTeleporterParams,
) -> Result<TeleporterReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = current_map(session, name)?.format;
    let skins = teleport_skins(session, format)?;
    let spec = teleport_spec(args, &skins)?;
    let label = if args.two_way {
        "add two-way teleporter"
    } else {
        "add teleporter"
    };
    let (built, report) = session.edit_map(name, label, |map| build_teleporter(map, &spec))?;
    Ok(TeleporterReport {
        pad_size: spec.pad_size,
        two_way: spec.two_way,
        built,
        edit: edit_result_fragment(&report),
    })
}
