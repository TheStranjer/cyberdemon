//! `add_switch` and `add_exit`: the wall panel, its action and the
//! level end.
//!
//! `add_switch` runs the pure [`crate::wad::map::features::switches`]
//! core as one [`crate::session::Session::edit_map`] step:
//! [`crate::wad::map::features::switches`] fits the panel chunk to the
//! wall, isolates it as its own linedef, resolves the tag its target
//! sector answers with and dresses the line from the
//! [`crate::wad::map::features::switch_special`] table.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::named::parse_named;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{edit_result_fragment, validate_texture_name};
use crate::tools::themes::theme_texture;
use crate::wad::map::features::{
    DEFAULT_SWITCH_WIDTH, PlacedSwitch, SwitchAction, SwitchSpec, action_name, build_switch,
    build_walk_over_exit,
};

use super::door::point_of;
use crate::tools::maps::params::PointParam;

/// Arguments for [`Cyberdemon::add_switch`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddSwitchParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The wall to mount the switch on, as a linedef index.
    pub(super) linedef: usize,
    /// Where along the wall to centre the panel, `{x, y}` (optional: it
    /// centres on the wall's midpoint; a point off the wall is projected
    /// onto it).
    #[serde(default)]
    pub(super) at: Option<PointParam>,
    /// The switch face's width in map units (default 32).
    #[serde(default)]
    pub(super) width: Option<i32>,
    /// What the switch does: `open_door`, `raise_floor`, `lower_floor`,
    /// `exit` or `secret_exit`.
    pub(super) action: String,
    /// The sector the switch acts on (required by the door and floor
    /// actions, ignored by the exits). Its existing tag is the one the
    /// line answers with; an untagged sector gets a fresh one.
    #[serde(default)]
    pub(super) target_sector: Option<usize>,
    /// Whether the line keeps working after use (default false: the
    /// `S1` specials and plain `playeruse`; true picks the `SR` specials
    /// and adds `repeatspecial`. Ignored by the exit actions).
    #[serde(default)]
    pub(super) repeatable: bool,
}

/// Arguments for [`Cyberdemon::add_exit`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddExitParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// How the level ends: `switch` (an exit switch on `linedef`) or
    /// `walk_over` (every two-sided boundary of `sector`).
    pub(super) kind: String,
    /// Whether this is the map's secret exit (default false).
    #[serde(default)]
    pub(super) secret: bool,
    /// The wall to mount the exit switch on (required by `switch`,
    /// ignored by `walk_over`).
    #[serde(default)]
    pub(super) linedef: Option<usize>,
    /// The sector whose boundaries trigger the exit (required by
    /// `walk_over`, ignored by `switch`).
    #[serde(default)]
    pub(super) sector: Option<usize>,
}

/// The [`SwitchAction`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
pub(super) fn action_of(text: &str) -> Result<SwitchAction, CyberError> {
    parse_named("action", text)
}

/// The texture theme role the switch panel wears: the exit switch for
/// the level-changing actions, the plain panel for the sector-moving
/// ones.
pub(super) const fn switch_role(action: SwitchAction) -> Role {
    match action {
        SwitchAction::Exit | SwitchAction::SecretExit => Role::ExitSwitch,
        SwitchAction::OpenDoor | SwitchAction::RaiseFloor | SwitchAction::LowerFloor => {
            Role::Switch
        }
    }
}

/// Structured result of [`Cyberdemon::add_switch`].
#[derive(Serialize)]
pub(super) struct SwitchReport {
    /// What the step installed, inlined next to the edit fragment.
    #[serde(flatten)]
    placed: PlacedSwitch,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The shared switch step's outcome: what was placed and the standard
/// `map`/`nodes_invalidated`/`copied_to_working` edit fragment.
type Switched = (PlacedSwitch, JsonMap<String, Value>);

/// The body both switch-mounting tools share: resolve the map, the
/// format-validated panel texture and the target tag, then run the
/// whole switch as one [`crate::session::Session::edit_map`] step.
fn run_switch_step(
    session: &mut Session,
    args: &AddSwitchParams,
    action: SwitchAction,
) -> Result<Switched, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = common::current_map(session, name)?.format;
    let texture = validate_texture_name(&theme_texture(session, switch_role(action))?, format)?;
    let spec = SwitchSpec {
        linedef: args.linedef,
        at: args.at.map(point_of),
        width: args.width.unwrap_or(DEFAULT_SWITCH_WIDTH),
        action,
        target_sector: args.target_sector,
        repeatable: args.repeatable,
        texture,
    };
    let label = format!("add {} switch", action_name(action));
    let (placed, report) = session.edit_map(name, &label, |map| build_switch(map, &spec))?;
    Ok((placed, edit_result_fragment(&report)))
}

/// `add_switch`'s work: parse the action and run the shared body.
pub(super) fn add_switch_run(
    session: &mut Session,
    args: &AddSwitchParams,
) -> Result<SwitchReport, CyberError> {
    let action = action_of(&args.action)?;
    let (placed, edit) = run_switch_step(session, args, action)?;
    Ok(SwitchReport { placed, edit })
}

/// How [`Cyberdemon::add_exit`] triggers the level end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExitKind {
    /// A wall switch carrying the exit action (the shared switch body).
    Switch,
    /// Every two-sided boundary of a sector, crossed by the player.
    WalkOver,
}

/// The canonical wire name of `kind`.
pub(super) const fn kind_name(kind: ExitKind) -> &'static str {
    match kind {
        ExitKind::Switch => "switch",
        ExitKind::WalkOver => "walk_over",
    }
}

/// The [`ExitKind`] `text` names (case-insensitive), or the refusal
/// listing both.
pub(super) fn kind_of(text: &str) -> Result<ExitKind, CyberError> {
    let wanted = text.to_ascii_lowercase();
    [ExitKind::Switch, ExitKind::WalkOver]
        .into_iter()
        .find(|kind| kind_name(*kind) == wanted)
        .ok_or_else(|| {
            CyberError::invalid(&format!(
                "kind {text:?} is not an exit kind: pass \"switch\" (an exit switch on \
                 linedef) or \"walk_over\" (every two-sided boundary of sector)"
            ))
        })
}

/// Structured result of [`Cyberdemon::add_exit`].
#[derive(Serialize)]
pub(super) struct ExitReport {
    /// The canonical trigger name: `switch` or `walk_over`.
    kind: &'static str,
    /// Indices of the linedefs carrying the level-end action.
    lines: Vec<usize>,
    /// The line special installed on them.
    special: i32,
    /// Whether this is the map's secret exit.
    secret: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The `switch` kind: the shared [`run_switch_step`] body with the
/// exit action on the required `linedef`.
fn exit_via_switch(session: &mut Session, args: &AddExitParams) -> Result<ExitReport, CyberError> {
    let Some(linedef) = args.linedef else {
        return Err(CyberError::invalid(
            "kind \"switch\" requires linedef: the index of the wall to mount the exit \
             switch on (assemble_map lists the linedefs)",
        ));
    };
    let action = if args.secret {
        SwitchAction::SecretExit
    } else {
        SwitchAction::Exit
    };
    let wire = AddSwitchParams {
        map: args.map.clone(),
        linedef,
        at: None,
        width: None,
        action: action_name(action).to_owned(),
        target_sector: None,
        repeatable: false,
    };
    let (placed, edit) = run_switch_step(session, &wire, action)?;
    Ok(ExitReport {
        kind: kind_name(ExitKind::Switch),
        lines: vec![placed.linedef],
        special: placed.special,
        secret: args.secret,
        edit,
    })
}

/// The `walk_over` kind: one [`build_walk_over_exit`] edit step over
/// the sector's boundaries.
fn exit_via_walkover(
    session: &mut Session,
    args: &AddExitParams,
) -> Result<ExitReport, CyberError> {
    let Some(sector) = args.sector else {
        return Err(CyberError::invalid(
            "kind \"walk_over\" requires sector: the index of the sector whose two-sided \
             boundaries the player crosses to end the level (get_map_summary lists the \
             sectors)",
        ));
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = if args.secret {
        "add secret walk_over exit"
    } else {
        "add walk_over exit"
    };
    let secret = args.secret;
    let (built, report) =
        session.edit_map(name, label, |map| build_walk_over_exit(map, sector, secret))?;
    Ok(ExitReport {
        kind: kind_name(ExitKind::WalkOver),
        lines: built.lines,
        special: built.special,
        secret,
        edit: edit_result_fragment(&report),
    })
}

/// `add_exit`'s work: parse the trigger kind and run its path, each a
/// single [`crate::session::Session::edit_map`] step.
pub(super) fn add_exit_run(
    session: &mut Session,
    args: &AddExitParams,
) -> Result<ExitReport, CyberError> {
    match kind_of(&args.kind)? {
        ExitKind::Switch => exit_via_switch(session, args),
        ExitKind::WalkOver => exit_via_walkover(session, args),
    }
}
