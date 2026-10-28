//! `add_stairs`: a working staircase over a height change.
//!
//! `add_stairs` runs the pure [`crate::wad::map::features::stairs`]
//! engine as one [`crate::session::Session::edit_map`] step:
//! [`crate::wad::map::features::plan_stairs`] divides the straight
//! `from` to `to` path into evenly rising step rectangles (each one's
//! ceiling probed from the sector under it,
//! [`crate::wad::map::features::MIN_HEADROOM`] enforced),
//! [`crate::wad::map::features::build_stairs`] paints them bottom-up
//! with [`crate::wad::map::overlay::apply_overlay`]
//! [`crate::wad::map::overlay::Paint::Sector`], and every riser face
//! wears the theme's `step` role ([`crate::tools::themes::theme_texture`],
//! [`crate::tools::maps::params::validate_texture_name`]). The created
//! step sectors and their floors come back in path order.

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
use crate::wad::map::features::{PaintedStep, StairSpec, build_stairs};

use super::door::point_of;
use crate::tools::maps::params::PointParam;

/// The `add_stairs` default step width in map units.
const DEFAULT_STAIR_WIDTH: i32 = 64_i32;

/// The `add_stairs` default step climb in map units.
const DEFAULT_STEP_HEIGHT: i32 = 16_i32;

/// Arguments for [`Cyberdemon::add_stairs`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddStairsParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The path's lower end, `{x, y}`: a point inside the area the stairs
    /// climb from (its sector's floor is the default `floor_from`).
    pub(super) from: PointParam,
    /// The path's higher end, `{x, y}`: a point inside the area the
    /// stairs climb into (its sector's floor is the default `floor_to`).
    pub(super) to: PointParam,
    /// The width of every step across the path in map units (default 64).
    #[serde(default)]
    pub(super) width: Option<i32>,
    /// The climb of one step in map units, `1..=24` (default 16).
    #[serde(default)]
    pub(super) step_height: Option<i32>,
    /// A fixed tread depth in map units (optional: by default the steps
    /// divide the path evenly); the path must be `steps * step_depth`
    /// long, ± 8 units.
    #[serde(default)]
    pub(super) step_depth: Option<i32>,
    /// The lower floor in map units (optional: the sector at `from`).
    #[serde(default)]
    pub(super) floor_from: Option<i32>,
    /// The higher floor in map units (optional: the sector at `to`).
    #[serde(default)]
    pub(super) floor_to: Option<i32>,
}

/// Structured result of [`Cyberdemon::add_stairs`].
#[derive(Serialize)]
pub(super) struct StairsReport {
    /// The created step sectors with their floors, in path order.
    steps: Vec<PaintedStep>,
    /// How many steps were built.
    count: usize,
    /// The resolved lower floor.
    floor_from: i32,
    /// The resolved higher floor.
    floor_to: i32,
    /// The per-step climb the division used.
    step_height: i32,
    /// The step width in map units.
    width: i32,
    /// What the paints reported (emptied sectors, dropped lines).
    notes: Vec<String>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Everything [`add_stairs_run`] hands the pure stair engine.
pub(super) fn stair_spec(args: &AddStairsParams) -> StairSpec {
    StairSpec {
        from: point_of(args.from),
        to: point_of(args.to),
        width: args.width.unwrap_or(DEFAULT_STAIR_WIDTH),
        step_height: args.step_height.unwrap_or(DEFAULT_STEP_HEIGHT),
        step_depth: args.step_depth,
        floor_from: args.floor_from,
        floor_to: args.floor_to,
    }
}

/// The texture the steps' risers wear: the session theme's `step` role,
/// validated for the map's format.
///
/// # Errors
///
/// [`theme_texture`]'s errors and [`validate_texture_name`]' refusals.
fn stair_riser(session: &Session, format: MapFormat) -> Result<String, CyberError> {
    let themed = theme_texture(session, Role::Step)?;
    validate_texture_name(&themed, format)
}

/// `add_stairs`' work: resolve the map, the riser texture and the spec,
/// then run the whole staircase as one [`crate::session::Session::edit_map`]
/// step.
pub(super) fn add_stairs_run(
    session: &mut Session,
    args: &AddStairsParams,
) -> Result<StairsReport, CyberError> {
    let spec = stair_spec(args);
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = common::current_map(session, name)?.format;
    let riser = stair_riser(session, format)?;
    let (built, report) =
        session.edit_map(name, "add stairs", |map| build_stairs(map, &spec, &riser))?;
    Ok(StairsReport {
        count: built.steps.len(),
        floor_from: built.floor_from,
        floor_to: built.floor_to,
        step_height: spec.step_height,
        width: spec.width,
        steps: built.steps,
        notes: built.notes,
        edit: edit_result_fragment(&report),
    })
}
