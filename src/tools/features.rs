//! Map-feature tools: `add_door`, `add_window`, `add_stairs`, `add_lift`,
//! `add_teleporter`, `add_switch`, `add_exit`, `mark_secret`,
//! `set_light_effect` and `place_things` (see the `features` router on
//! [`crate::server::Cyberdemon`]).
//!
//! The `features` group builds complete game structures on an already
//! drawn map: every tool runs its pure engine inside one
//! [`crate::session::Session::edit_map`] step, so one undo removes the
//! whole feature again. The tag a `remote` door mints with
//! [`crate::wad::map::features::next_free_tag`] is exactly what
//! `add_switch` looks for: pointing a switch's `target_sector` at that
//! door closes the remote loop. `add_exit` finishes the level with the
//! same machinery: its `switch` kind hands the shared
//! [`switches::run_switch_step`] body the `exit`/`secret_exit` action,
//! and its `walk_over` kind dresses every two-sided boundary of a sector
//! with the family's walk-over level-end special as one
//! [`crate::wad::map::features::build_walk_over_exit`] edit step.
//!
//! The work is split by tool: [`wall`] holds the wall choice and
//! void-strip machinery `add_door` and `add_window` share, [`door`] the
//! door pipeline beside the shared wall names ([`door::Target`]),
//! resolutions ([`door::Chosen`]) and placements ([`door::Placed`]) the
//! other feature tools build on, [`window`] the window one, [`stairs`],
//! [`lift`] and [`teleporter`] the movers, [`switches`] the switch and
//! exit tools, [`specials`] the two sector-special steps, and
//! [`place_params`] and [`place`] the `place_things` wire parsing and
//! distribution run. This root keeps the shared refusal helpers, the map
//! read ([`current_map`]), the theme helpers ([`theme_wall`], [`skin`]
//! and [`theme_skin`]), the result handoff ([`reply`]) and the
//! router; the group's unit tests live in `features/tests.rs`.

mod door;
mod lift;
mod place;
mod place_params;
mod specials;
mod stairs;
mod switches;
mod teleporter;
mod wall;
mod window;

#[cfg(test)]
mod tests;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::themes::Role;
use crate::tools::features::door::{AddDoorParams, DoorSkins, MAX_DEPTH, add_door_run, track_role};
use crate::tools::features::lift::{AddLiftParams, add_lift_run};
use crate::tools::features::place::place_things_run;
use crate::tools::features::place_params::PlaceThingsParams;
use crate::tools::features::specials::{
    MarkSecretParams, SetLightParams, mark_secret_run, set_light_run,
};
use crate::tools::features::stairs::{AddStairsParams, add_stairs_run};
use crate::tools::features::switches::{
    AddExitParams, AddSwitchParams, add_exit_run, add_switch_run,
};
use crate::tools::features::teleporter::{AddTeleporterParams, add_teleporter_run};
use crate::tools::features::window::{AddWindowParams, add_window_run};
use crate::tools::maps::common;
use crate::tools::maps::params::validate_texture_name;
use crate::tools::reply;
use crate::tools::themes::theme_texture;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::features::DoorStyle;
use crate::wad::name::LumpName;

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// The refusal for a wall with no room within reach behind it.
fn no_room() -> CyberError {
    invalid(&format!(
        "no room behind this wall within {MAX_DEPTH} units; move the rooms closer or give \
         a larger depth"
    ))
}

/// The server-bug refusal for a paint that lost the door wall.
fn wall_lost(wall: usize) -> CyberError {
    CyberError::Internal(format!(
        "the door wall linedef {wall} vanished while the recess was drawn (server bug)"
    ))
}

/// The server-bug refusal for a paint that reported no sector.
fn sector_lost() -> CyberError {
    CyberError::Internal("the door recess paint reported no sector (server bug)".to_owned())
}

/// The wall texture a carved strip's bare edges fall back to.
const DEFAULT_WALL_TEXTURE: &str = "STARTAN3";

/// The wall texture a carved strip's bare edges fall back to: the
/// session theme's `Wall`, else [`DEFAULT_WALL_TEXTURE`].
fn theme_wall(session: &Session) -> Result<String, CyberError> {
    let themed = theme_texture(session, Role::Wall)?;
    Ok(if themed == NO_TEXTURE {
        DEFAULT_WALL_TEXTURE.to_owned()
    } else {
        themed
    })
}

/// One skin entry: the explicit name when given, else the theme's,
/// validated for the map's format.
fn skin(given: Option<&str>, themed: &str, format: MapFormat) -> Result<String, CyberError> {
    validate_texture_name(given.unwrap_or(themed), format)
}

/// The textures the door wears: theme roles by default (the coloured
/// track for the locked styles), explicit `texture`/`track_texture`
/// overrides validated for the map's format.
///
/// # Errors
///
/// [`theme_texture`]'s errors and [`validate_texture_name`]' refusals.
fn theme_skin(
    session: &Session,
    style: DoorStyle,
    args: &AddDoorParams,
    format: MapFormat,
) -> Result<DoorSkins, CyberError> {
    let door = theme_texture(session, Role::Door)?;
    let track = theme_texture(session, track_role(style))?;
    let floor = theme_texture(session, Role::DoorFloor)?;
    let wall = theme_wall(session)?;
    Ok(DoorSkins {
        door: skin(args.texture.as_deref(), &door, format)?,
        track: skin(args.track_texture.as_deref(), &track, format)?,
        floor_flat: skin(None, &floor, format)?,
        wall: skin(None, &wall, format)?,
    })
}

/// The map as the next edit sees it: the working archive's block when it
/// holds one, else the merged view's (which is what the edit copies in).
///
/// # Errors
///
/// [`common::load_map`]'s errors for the merged scope when the working
/// archive holds no such block.
fn current_map(session: &Session, name: LumpName) -> Result<Map, CyberError> {
    match common::load_map(session, name, Scope::Working) {
        Ok(map) => Ok(map),
        Err(error) if error.code() == "not_found" => common::load_map(session, name, Scope::Merged),
        Err(error) => Err(error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = features_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Cuts a working door into a map in one undoable edit.
    #[tool(
        description = "Cuts a working door into a map in one undoable edit: the doorway, the recess it slides into, the use action and the theme dressing. Name the wall as between: [sectorA, sectorB] or linedef: N, optionally centred at {x, y}; width defaults to 64 and style picks the action (normal, fast, stay_open, locked_red/blue/yellow, remote). Returns the band sector, the dressed lines and the tag. add_switch can trigger it from elsewhere."
    )]
    fn add_door(
        &self,
        Parameters(args): Parameters<AddDoorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_door_run(session, &args)),
        )
    }

    /// Cuts a working window into a map in one undoable edit.
    #[tool(
        description = "Cuts a working window into a map in one undoable edit: a raised view strip between two rooms that nobody can walk through. Placement mirrors add_door exactly (between: [sectorA, sectorB] or linedef: N, with optional at and width); sill (default 32) lifts the strip's floor and lintel (default 24) drops its ceiling, and under 16 units of glass the call is refused. One undo removes the window again."
    )]
    fn add_window(
        &self,
        Parameters(args): Parameters<AddWindowParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_window_run(session, &args)),
        )
    }

    /// Builds a working staircase over a height change in one undoable edit.
    #[tool(
        description = "Builds a working staircase over a height change in one undoable edit: the straight path from: {x, y} (the lower end) to: {x, y} (the higher) divides into evenly rising step rectangles painted bottom-up, each becoming its own sector. step_height defaults to 16 and step_depth to 24. Returns the step sectors in climb order. Use it where floors differ by more than a step; build_map_from_plan can include stairs as a connector kind."
    )]
    fn add_stairs(
        &self,
        Parameters(args): Parameters<AddStairsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_stairs_run(session, &args)),
        )
    }

    /// Installs a working lift platform in one undoable edit.
    #[tool(
        description = "Installs a working lift in one undoable edit: a platform floor that drops to a lower level, waits, and rises back, triggered by players below. Name the platform as sector: N or draw a new one with platform: {points: [...]}; low_floor defaults to the lowest neighbouring floor, style use_wall or walk_trigger and repeatable pick the action. Returns the platform sector, its lines and the fresh tag."
    )]
    fn add_lift(
        &self,
        Parameters(args): Parameters<AddLiftParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_lift_run(session, &args)),
        )
    }

    /// Places a working teleporter pad and destination in one undoable edit.
    #[tool(
        description = "Places a working teleporter in one undoable edit: a floor pad a player walks onto and a destination square they arrive in. pad: {x, y} and destination: {x, y, angle} are both required points inside sectors; pad_size (default 64) squares both, the pad's borders carry the family's teleport action, a type-14 destination thing stands at the target, and two_way: true builds the return leg."
    )]
    fn add_teleporter(
        &self,
        Parameters(args): Parameters<AddTeleporterParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_teleporter_run(session, &args)),
        )
    }

    /// Mounts a working switch on a wall chunk in one undoable edit.
    #[tool(
        description = "Mounts a working switch on a wall in one undoable edit: a panel chunk (width 32 by default) isolated on its own linedef, wearing the theme's switch texture and a use action. action is open_door, raise_floor, lower_floor, exit or secret_exit; target_sector names the sector the door and floor actions work on (its tag answers, minted when untagged); at centres the panel and repeatable keeps it live."
    )]
    fn add_switch(
        &self,
        Parameters(args): Parameters<AddSwitchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_switch_run(session, &args)),
        )
    }

    /// Ends the level from a wall switch or a walk-over line in one undoable edit.
    #[tool(
        description = "Places a level end in one undoable edit. kind switch mounts an exit panel on linedef (the add_switch mount with the exit action, the theme's exit_switch texture); kind walk_over ends the level on every two-sided boundary of sector. secret: true picks the secret-exit action. Returns the dressed lines. Use it to finish a map you built; check_playability then proves a player can reach it."
    )]
    fn add_exit(
        &self,
        Parameters(args): Parameters<AddExitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| add_exit_run(session, &args)),
        )
    }

    /// Marks whole sectors secret in one undoable edit.
    #[tool(
        description = "Marks whole sectors secret (or unmarks them) in a single undoable edit: a pure Sector::special edit that keeps every bit outside the field - the vanilla family's secret type or the Boom bit, the Hexen flag over its own special. A Boom secret bit on a vanilla Doom map warns because vanilla Doom ignores Boom bits. Prefer it over edit_sector's raw special for this one job."
    )]
    fn mark_secret(
        &self,
        Parameters(args): Parameters<MarkSecretParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| mark_secret_run(session, &args)),
        )
    }

    /// Installs a vanilla light effect on whole sectors in one undoable edit.
    #[tool(
        description = "Installs a light effect on whole sectors in a single undoable edit: effect none, random_off, strobe_fast, strobe_slow, glow or flicker picks the map family's sector type, keeping the light level and every bit outside the type field; light optionally sets the level. Prefer it over edit_sector's raw special - the action tables do the bit math."
    )]
    fn set_light_effect(
        &self,
        Parameters(args): Parameters<SetLightParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| set_light_run(session, &args)),
        )
    }

    /// Distributes monsters and items across sectors with clearance.
    #[tool(
        description = "Distributes monsters, items and decorations across sectors, a bounding box or a circle in one undoable edit: thing by doomednum or definition name, count split by area or per_sector each, wall and neighbour clearance kept, skills/ambush flags, snapped facings (a number, face_player_start, face_center or random_seeded with seed) and dry_run preview. Use it to populate rooms; add_thing places one exact thing."
    )]
    fn place_things(
        &self,
        Parameters(args): Parameters<PlaceThingsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| place_things_run(session, &args)),
        )
    }
}
