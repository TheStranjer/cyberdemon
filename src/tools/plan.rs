//! Plan tools: `build_map_from_plan` (see the `plan` router on
//! [`crate::server::Cyberdemon`]).
//!
//! One declarative call: rooms with footprints and sector properties,
//! connections joining them, a wall thickness and a theme. The wire
//! shape and its mapping onto [`crate::plan::model`] live in
//! [`params`]; the pure [`crate::plan::compile`] engine grows the thick
//! walls, places every connector band, emits the fresh [`Map`] and
//! dresses it in the [`TextureRoles`] this module resolves for the call: a
//! plain `opening` wide enough to span a whole shared edge takes the wall
//! down there and owns no sector (the rooms simply join and its result
//! row reports empty `sectors`), and every other band wears the adjoining
//! rooms' flats and the dimmer of their lights.
//! The [`crate::plan::things`] engine then stands the requested things
//! in: names resolve against the session's game definitions (the same
//! overlay the validate tools read), pinned points travel with the
//! rooms through [`crate::plan::units`], room fills land row-major on
//! the clearance grid, and every built map keeps a player 1 start. The
//! [`crate::plan::features`] step runs right after the compile and
//! installs the plan's game structures in fixed pipeline order —
//! doors, switches, lifts, teleporters, exits, lights, secrets: door
//! connectors take a fresh tag on their band sector and the style's
//! linedef specials on their room-facing lines (locked doors re-dress
//! their jambs in the theme's coloured track), `switch` rows mount
//! panels firing a remote door's tag — on the wall `wall` names and,
//! among several matching ones, the one nearest the row's plan `at`
//! point, which also centres the panel there — `lift` rows dress the
//! room's own sector as a tagged platform whose lower-facing edges carry
//! the ride,
//! `teleporter` rows stand the pad and destination squares (with their
//! own type-14 things), `exit` rows end the level — a `walk_over` row
//! dresses only the room's plain boundary lines, warning about the ones
//! it leaves to a door, a lift or a teleporter — and the room
//! `light_effect` and `secret` flags paint the family's sector
//! specials — lights before secrets, so a lit secret keeps both. Every
//! installed feature is reported as a `features` row, each line it names
//! beside the map-unit coordinates of its two endpoints. After the things
//! stand in, [`crate::plan::playable_checks`] adds its advisory warnings
//! — a locked door whose key colour stands nowhere among the placed
//! things, a plan with no `exit` feature and a remote door no `switch`
//! row opens — never errors, in plan order.
//! The compiled block is written into the working archive as exactly
//! one [`Session::edit`] step through
//! [`crate::tools::maps::write_block`], and the map becomes the
//! session's active map. The `finalize` parameter grows that one step:
//! before the write it runs the [`crate::tools::nodes`] whole-set build
//! (`SEGS`/`SSECTORS`/`NODES` plus `REJECT` and `BLOCKMAP` on a binary
//! map, the extended `ZNODES` lump on a `UDMF` one) on the compiled
//! model, and it reports an errors-only pass of
//! [`crate::wad::map::validate`] over the finished map — validation
//! findings never roll the build back; by then the doors are wired, so
//! the pass judges the finished specials and tags. Before the
//! compiler, the pure
//! [`crate::plan::units`] transform carries the
//! drawn plan into map units: the `units` scale/origin/mirror and the
//! `grid` snap, so a sketch measured in pixels lands grid-aligned at
//! map size.
//!
//! Compiler refusals already name the offending id, and the ones that point
//! at a plan element carry it as a [`crate::error::PlanRef`];
//! [`build::with_plan_path`] turns that reference into the plan path
//! (`rooms[0]`, `connections[1]`, `features[2]`) so the caller can point at
//! the wire element that broke.
//!
//! The work is split: this root keeps what the group shares - the
//! constants, the theme resolution ([`plan_theme`] and the dressings it
//! hands the compiler and the feature step) and the naming helpers every
//! plan path reads - while [`build`] carries the run itself, from
//! [`build::plan_path`] through the compile, the feature and thing steps,
//! the finalize pass and [`build::build_run`]'s one edit, answering with
//! [`build::PlanReport`]. The group's unit tests live in `plan/tests.rs`.

pub(in crate::tools) mod params;

mod build;

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::plan::features::FeatureDress;
use crate::plan::model::Connection;
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::plan::build::build_run;
use crate::tools::plan::params::{BuildPlanParams, ConnectionParam, FeatureParam};
use crate::tools::reply;
use crate::tools::themes::theme_roles;
use crate::wad::map::consts::NO_TEXTURE;

/// Error rows the finalized result lists at most; the counts stay exact.
const VALIDATION_ERROR_CAP: usize = 20_usize;
/// The steps every result hands the caller.
const NEXT_STEPS: [&str; 2] = [
    "render_topdown to compare with the sketch",
    "render_view from the player start",
];
/// Every plan dressing role and the session theme role answering it.
const THEME_ROLES: [(TextureRole, Role); 8] = [
    (TextureRole::Floor, Role::Floor),
    (TextureRole::Ceiling, Role::Ceiling),
    (TextureRole::Wall, Role::Wall),
    (TextureRole::Door, Role::Door),
    (TextureRole::DoorFloor, Role::DoorFloor),
    (TextureRole::DoorTrack, Role::DoorTrack),
    (TextureRole::WindowSill, Role::WindowSill),
    (TextureRole::Step, Role::Step),
];
/// The theme roles the feature step dresses on top of the eight
/// compiler roles: the coloured tracks of the locked door styles and
/// the switch and exit-switch panels.
const FEATURE_ROLES: [Role; 5] = [
    Role::DoorTrackRed,
    Role::DoorTrackBlue,
    Role::DoorTrackYellow,
    Role::Switch,
    Role::ExitSwitch,
];

/// The resolved [`TextureRole`] names one [`crate::plan::compile`] call
/// dresses with: a role the theme answered `"-"` holds no name, so the
/// compiler leaves its faces undressed instead of inventing one.
struct PlanTheme {
    /// The (role, name) pairs the theme answered, in [`THEME_ROLES`]
    /// order.
    names: Vec<(TextureRole, String)>,
}

impl TextureRoles for PlanTheme {
    fn texture(&self, role: TextureRole) -> Option<&str> {
        self.names
            .iter()
            .find_map(|pair| (pair.0 == role).then_some(pair.1.as_str()))
    }
}

/// What one call's theme resolution hands the build: the compiler's
/// [`PlanTheme`], the [`crate::plan::features`] step's [`FeatureDress`]
/// and the notes for the roles that answered `"-"`.
struct ResolvedTheme {
    /// The eight dressing names for the compiler.
    plan: PlanTheme,
    /// The door and track names for the feature step.
    features: FeatureDress,
    /// The advisory notes, dressing roles first, coloured tracks after.
    notes: Vec<String>,
}

/// The compiler's theme for this call, the [`crate::plan::features`]
/// step's dressed names, and the notes for the roles that answered
/// `"-"`. An unknown `theme` override is refused with the `theme` plan
/// path leading, so the caller sees which request field to fix; a stale
/// session theme (no override) keeps the bare refusal, naming the themes
/// that would clear it with `set_texture_theme`.
///
/// # Errors
///
/// [`theme_roles`]' refusals for an unknown `theme` override.
fn plan_theme(session: &Session, theme: Option<&str>) -> Result<ResolvedTheme, CyberError> {
    let mut wanted: Vec<Role> = THEME_ROLES.iter().map(|&(_, role)| role).collect();
    wanted.extend(FEATURE_ROLES);
    let resolved =
        theme_roles(session, theme, &wanted).map_err(|error| with_theme_path(error, theme))?;
    let mut names = Vec::new();
    let mut notes = Vec::new();
    for (plan_role, role) in THEME_ROLES {
        match resolved.get(&role).map(String::as_str) {
            Some(name) if name != NO_TEXTURE => names.push((plan_role, name.to_owned())),
            _ => notes.push(undressed_note(role)),
        }
    }
    let features = feature_dress(&resolved, &mut notes);
    Ok(ResolvedTheme {
        plan: PlanTheme { names },
        features,
        notes,
    })
}

/// The `theme` plan path on an override refusal: exactly how
/// `with_plan_path` in `build.rs` leads a compiler refusal with its
/// `rooms[i]`, an unknown `theme` name is refused with `theme:` leading,
/// so the caller sees which request field to fix.
/// Refusals that are not the caller's argument error (a broken texture
/// read), and calls that carry no override, keep their own words.
fn with_theme_path(error: CyberError, theme: Option<&str>) -> CyberError {
    if theme.is_some() && error.is_invalid_argument() {
        return error.map_message(|old| format!("theme: {old}"));
    }
    error
}

/// The advisory note for a theme role the session answered `"-"`.
fn undressed_note(role: Role) -> String {
    format!(
        "the theme answers \"-\" for role {}: those faces stay undressed",
        role.as_str()
    )
}

/// The names the feature step dresses with: the plain door face and
/// track ride on the compiler's own answers, the lift's `step` side and
/// the teleporter's `wall` fallback ride on the compiler's `step` and
/// `wall` answers (a role missing there was already noted by the
/// compiler loop), and a coloured track or switch panel the theme left
/// unnamed notes itself like a missing dressing role — its jambs keep
/// the plain track the compiler dressed and its switch panels keep the
/// wall's own middle.
fn feature_dress(resolved: &BTreeMap<Role, String>, notes: &mut Vec<String>) -> FeatureDress {
    FeatureDress {
        door: resolved_name(resolved, Role::Door),
        track: resolved_name(resolved, Role::DoorTrack),
        track_red: unthemed_note(resolved, Role::DoorTrackRed, notes),
        track_blue: unthemed_note(resolved, Role::DoorTrackBlue, notes),
        track_yellow: unthemed_note(resolved, Role::DoorTrackYellow, notes),
        switch: unthemed_note(resolved, Role::Switch, notes),
        exit_switch: unthemed_note(resolved, Role::ExitSwitch, notes),
        step: resolved_name(resolved, Role::Step),
        wall: resolved_name(resolved, Role::Wall),
    }
}

/// `role`'s resolved name, `"-"` when the theme answers nothing, noting
/// itself when it does not.
fn unthemed_note(resolved: &BTreeMap<Role, String>, role: Role, notes: &mut Vec<String>) -> String {
    let name = resolved_name(resolved, role);
    if name == NO_TEXTURE {
        notes.push(undressed_note(role));
    }
    name
}

/// `role`'s resolved name, `"-"` when the theme answers nothing.
fn resolved_name(resolved: &BTreeMap<Role, String>, role: Role) -> String {
    resolved
        .get(&role)
        .cloned()
        .unwrap_or_else(|| NO_TEXTURE.to_owned())
}

/// The label a wire connection row answers to: its `id`, or the
/// derived `from->to` when it carries none. One formatting rule, owned
/// by [`crate::plan::model::Connection`], shared by the model's own
/// messages and the plan tool's `connections[i]` paths.
fn connection_label(connection: &ConnectionParam) -> String {
    Connection::label_for(connection.id.as_deref(), &connection.from, &connection.to)
}

/// The ids a wire feature row names: its room, its teleporter rooms
/// and the connection it opens.
fn feature_names(feature: &FeatureParam) -> [Option<&str>; 4] {
    [
        feature.room.as_deref(),
        feature.from_room.as_deref(),
        feature.to_room.as_deref(),
        feature.opens.as_deref(),
    ]
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = plan_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Builds a whole map block from a declarative rooms-and-connections plan in one undoable edit.
    #[tool(
        description = "Builds a complete map from a declarative plan in one undoable edit: rooms (rect or point-loop footprints with heights, light, flats and theme roles), connections (opening, wired door, window, stairs) grown between rooms that share an edge, thick walls, feature rows (exit, switch, lift, teleporter), things resolved against the session's definitions, and a mandatory player start. units and grid map plan coordinates to map units. finalize also builds nodes and validates. This is the first tool for a new level, not the element editors."
    )]
    fn build_map_from_plan(
        &self,
        Parameters(args): Parameters<BuildPlanParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| build_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
