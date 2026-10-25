//! Playability tools: [`Cyberdemon::check_playability`], the can-the-player-
//! actually-finish-this-map report, and [`Cyberdemon::describe_map`], the
//! measured per-sector picture of a whole map.
//!
//! The analysis is the pure [`crate::wad::map::playability`] module's; the
//! group resolves the parameters, reads the map and the session's definitions
//! (custom `DECORATE`/`DEHACKED` things included) under one lock, and folds
//! the walk, its [`blockers`] explanations and the [`stuck_things`] body
//! tests into the one answer the calling model acts on: `ok`, a plain
//! `summary`, the reachable/unreachable sector counts, per-blocker `reason`
//! and `hint` rows naming the tool that fixes each, the exits, keys and
//! locks as the walk saw them, the stuck and unreachable things, and
//! `next_steps` with the first three hints. `render: true` additionally
//! publishes a [`render_topdown`](Cyberdemon::render_topdown) overview with
//! the unreachable sectors, blocker lines and stuck things highlighted and
//! the sector labels on. A map with no player start is a normal
//! `ok: false` result, never an error.
//!
//! `describe_map` folds the pure [`crate::wad::map::describe`] module's
//! answer into the one reading picture an agent orients on before editing:
//! the same [`load`] under one lock, the per-sector [`SectorFacts`] rows
//! deduplicated, filtered and paged as the parameters ask, the whole-map
//! [`MapOverview`] summary beside them whatever the paging says, and the
//! same labelled [`playability_overview`] PNG with plain marks as the
//! `render` flag asks for (default on).
//!
//! The work itself is in three private submodules: `check` holds
//! `check_playability`'s parameters, the analysis a report is folded out of,
//! its row types and the fold; `describe` holds `describe_map`'s parameters,
//! its limit and selection checks, its paging and its report; `hints` words
//! every blocker and stuck thing as the tool-named fix those reports quote.
//! This root keeps the constant they share, the read inputs and [`load`], and
//! the two router methods.
//!
//! [`SectorFacts`]: crate::wad::map::describe::SectorFacts
//! [`MapOverview`]: crate::wad::map::describe::MapOverview
//! [`playability_overview`]: crate::tools::render::playability_overview
//! [`blockers`]: crate::wad::map::playability::blockers
//! [`stuck_things`]: crate::wad::map::playability::stuck_things

mod check;
mod describe;
mod hints;

#[cfg(test)]
mod tests;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::playability::check::{CheckPlayabilityParams, check_run};
use crate::tools::playability::describe::{DescribeMapParams, describe_run};
use crate::tools::{fail, ok};
use crate::wad::map::Map;
use crate::wad::map::consts::UNKNOWN_THING_RADIUS;

/// The map and definitions one check reads, out of the session lock.
struct Prepared {
    /// The map to walk, from the merged view.
    map: Map,
    /// The session's definitions: custom `DECORATE`/`DEHACKED` things
    /// included, exactly what the sprite renderers dress from.
    defs: Definitions,
}

/// Read the check's inputs under the session lock.
///
/// # Errors
///
/// [`common::resolve_map_name`] and [`common::load_map`]'s errors, and the
/// definition-collection errors of [`session_definitions`].
fn load(state: &AppState, requested: Option<&str>) -> Result<Prepared, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let (sources, _notes) = custom_sources(session)?;
        let defs = session_definitions(session_game(session), &sources)?;
        Ok(Prepared { map, defs })
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = playability_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports whether a player can actually finish the map, and why not.
    #[tool(
        description = "Walks one map the way a player at skill (easy, medium default, hard) would: from the player 1 start through every line the player fits (step, headroom and width tests), opening the doors, lifts and teleporters the walk reaches and collecting keys. Reports ok plus blockers (every unreachable sector with the reason), exits, keys and locks, stuck things, and an overview render. Run it before shipping a map."
    )]
    async fn check_playability(
        &self,
        Parameters(args): Parameters<CheckPlayabilityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match check_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Describes a map's sectors as measured rows of facts, roles and
    /// neighbours, beside a whole-map summary and a labelled overview.
    #[tool(
        description = "Describes one map the way its sectors can be read: per-sector rows (area, bbox, label centre, heights, light, flats, sky, named special, tag, roles, wall-texture tally, counted things, neighbours with their connecting lines) beside a whole-map summary, paged by offset and limit. Use it to learn what a sector is before editing it; find_elements answers index questions."
    )]
    async fn describe_map(
        &self,
        Parameters(args): Parameters<DescribeMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match describe_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}
