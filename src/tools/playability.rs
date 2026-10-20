//! Playability tools: [`Cyberdemon::check_playability`], the can-the-player-
//! actually-finish-this-map report, and [`Cyberdemon::describe_map`], the
//! measured per-sector picture of a whole map.
//!
//! The analysis is the pure [`crate::wad::map::playability`] module's; this
//! file resolves the parameters, reads the map and the session's definitions
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
//! [`SectorFacts`]: crate::wad::map::describe::SectorFacts
//! [`MapOverview`]: crate::wad::map::describe::MapOverview
//! [`playability_overview`]: crate::tools::render::playability_overview

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::render::{OverviewMarks, playability_overview};
use crate::tools::{fail, ok};
use crate::wad::map::Map;
use crate::wad::map::describe::{Describe, MapOverview, SectorFacts};
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::playability::{
    self, Blocker, BlockerReason, LockInfo, Playability, Stuck, StuckReason, analyze_playability,
    blockers, stuck_things,
};
use crate::wad::name::LumpName;

/// The half-side assumed for thing types the definitions do not know, for
/// the in-wall hint's distance: the pure stuck test's own monster default.
const UNKNOWN_THING_RADIUS: i64 = 20;

/// Arguments for [`Cyberdemon::check_playability`].
#[derive(Debug, Deserialize, JsonSchema)]
struct CheckPlayabilityParams {
    /// The map to check, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which difficulty to walk the map at: `"easy"`, `"medium"` (the
    /// default) or `"hard"`. Thing presence bits decide which starts, keys
    /// and monsters exist at the skill.
    #[serde(default)]
    skill: Option<String>,
    /// Also publish a top-down overview PNG with the unreachable sectors,
    /// blocker lines and stuck things highlighted (default `false`).
    #[serde(default)]
    render: Option<bool>,
}

/// The skill `skill` names, [`Skill::Medium`] when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the three accepted values.
fn parse_skill(text: Option<&str>) -> Result<Skill, CyberError> {
    let Some(name) = text else {
        return Ok(Skill::Medium);
    };
    Skill::parse(name).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "skill {name:?} is not a difficulty: pass \"easy\", \"medium\" or \"hard\", or \
             leave the parameter out for medium"
        ))
    })
}

/// Everything one report is folded out of: the read inputs and the three
/// pure analyses over them.
struct Analysis<'ctx> {
    /// The map that was walked.
    map: &'ctx Map,
    /// The session's definitions the body tests read.
    defs: &'ctx Definitions,
    /// The flag family the map's lines are stored in.
    family: FlagFamily,
    /// The skill the walk was run at.
    skill: Skill,
    /// The reachability walk's answer.
    found: Playability,
    /// Why each unreachable sector stayed out of reach.
    held_out: Vec<Blocker>,
    /// Which things cannot stand where they are.
    stuck: Vec<Stuck>,
    /// The player 1 start the walk began from, if one spawns.
    start: Option<usize>,
}

impl Analysis<'_> {
    /// Whether a non-secret exit's action can be worked from a reached side.
    fn exit_reachable(&self) -> bool {
        self.found
            .exits
            .iter()
            .any(|exit| !exit.secret && exit.reachable)
    }

    /// Whether the player start itself is one of the stuck things (its own
    /// row, or an overlap row naming it as the other body).
    fn start_stuck(&self) -> bool {
        let Some(start) = self.start else {
            return false;
        };
        self.stuck.iter().any(|stuck| {
            stuck.thing == start
                || matches!(stuck.reason, StuckReason::Overlaps { other } if other == start)
        })
    }

    /// Whether the map is playable as it stands.
    fn is_ok(&self) -> bool {
        self.found.start_sector.is_some() && self.exit_reachable() && !self.start_stuck()
    }
}

/// The `start` object of a [`PlayabilityReport`].
#[derive(Debug, Serialize)]
struct StartReport {
    /// The player 1 start's index into `THINGS`.
    thing: usize,
    /// The sector it stands in; `null` when it stands in the void.
    sector: Option<usize>,
}

/// The `sectors` object of a [`PlayabilityReport`].
#[derive(Debug, Serialize)]
struct SectorCounts {
    /// The map's sector count.
    total: usize,
    /// How many of them the walk reached.
    reachable: usize,
    /// The unreached sector indices, in order.
    unreachable: Vec<usize>,
}

/// One `exits` row: an exit line as the walk saw it.
#[derive(Debug, Serialize)]
struct ExitRow {
    /// The linedef carrying the action.
    line: usize,
    /// Whether this is the map's secret exit.
    secret: bool,
    /// Whether the walk reached a side the action works from.
    reachable: bool,
}

/// One `keys` row: a key item as the walk saw it.
#[derive(Debug, Serialize)]
struct KeyRow {
    /// The index into `THINGS`.
    thing: usize,
    /// The colour of lock this key opens.
    color: playability::KeyColor,
    /// Whether the walk reached the sector it stands in.
    reachable: bool,
}

/// One `blockers` row: the pure [`Blocker`] plus the tool-named fix.
#[derive(Debug, Serialize)]
struct BlockerRow {
    /// The explained blocker.
    #[serde(flatten)]
    blocker: Blocker,
    /// How to open this way in, naming the tool that does it.
    hint: String,
}

/// One `stuck_things` row: the pure [`Stuck`] plus the thing's name and fix.
#[derive(Debug, Serialize)]
struct StuckRow {
    /// The stuck thing.
    #[serde(flatten)]
    stuck: Stuck,
    /// The definition name of the thing's type.
    name: String,
    /// How to unstick it, naming the tool that does it.
    hint: String,
}

/// One `unreachable_things` row.
#[derive(Debug, Serialize)]
struct UnreachableRow {
    /// The index into `THINGS`.
    thing: usize,
    /// The definition name of the thing's type.
    name: String,
    /// The definition category the thing stands under.
    category: String,
    /// The unreached sector it stands in.
    sector: usize,
}

/// Structured result of [`Cyberdemon::check_playability`].
#[derive(Debug, Serialize)]
struct PlayabilityReport {
    /// Whether a player at this skill can start and finish the map.
    ok: bool,
    /// One or two plain sentences built from the counts.
    summary: String,
    /// The player 1 start, `null` when none spawns at this skill.
    start: Option<StartReport>,
    /// The reachable and unreachable sector counts.
    sectors: SectorCounts,
    /// Why each unreachable sector stayed out of reach, sorted by sector.
    blockers: Vec<BlockerRow>,
    /// Every exit line with whether the walk could work it.
    exits: Vec<ExitRow>,
    /// Every key that spawns, with whether the walk reached it.
    keys: Vec<KeyRow>,
    /// Every locked line, with whether its door was opened.
    locks: Vec<LockInfo>,
    /// The sectors behind a door no reachable trigger opened.
    doors_never_opened: Vec<usize>,
    /// The two-sided lines too short to walk through that join reached
    /// sectors.
    narrow_lines: Vec<usize>,
    /// The things that cannot stand where they are.
    stuck_things: Vec<StuckRow>,
    /// The things standing in a sector the walk never reached.
    unreachable_things: Vec<UnreachableRow>,
    /// Everything the analysis could not read, from the walk.
    warnings: Vec<String>,
    /// The highlighted overview, `null` unless `render` was asked for.
    render: Option<AssetRef>,
    /// What to do next: the look-check note when `ok`, else the first
    /// three problem hints.
    next_steps: Vec<String>,
}

/// The `sectors` row: totals and the unreached indices, in sector order.
fn sector_counts(a: &Analysis<'_>) -> SectorCounts {
    SectorCounts {
        total: a.map.sectors.len(),
        reachable: a.found.reachable.len(),
        unreachable: (0..a.map.sectors.len())
            .filter(|sector| !a.found.reachable.contains(sector))
            .collect(),
    }
}

/// The `exits` rows, in linedef order.
fn exit_rows(a: &Analysis<'_>) -> Vec<ExitRow> {
    a.found
        .exits
        .iter()
        .map(|exit| ExitRow {
            line: exit.line,
            secret: exit.secret,
            reachable: exit.reachable,
        })
        .collect()
}

/// The `keys` rows, in thing order.
fn key_rows(a: &Analysis<'_>) -> Vec<KeyRow> {
    a.found
        .keys
        .iter()
        .map(|key| KeyRow {
            thing: key.thing,
            color: key.color,
            reachable: key.reachable,
        })
        .collect()
}

/// The hint that fixes one blocker, per the fixed template for its reason.
fn blocker_hint(blocker: &Blocker) -> String {
    match blocker.reason {
        BlockerReason::Impassable => blocker.line.map_or_else(
            || "clear the impassable flag on this line (edit_linedef)".to_owned(),
            |line| format!("clear the impassable flag on line {line} (edit_linedef)"),
        ),
        BlockerReason::Narrow { .. } => {
            "widen the opening to at least 32 units (move_vertex)".to_owned()
        }
        BlockerReason::Locked { color } => format!(
            "place a {} key in a reachable area (add_thing)",
            color.as_str()
        ),
        BlockerReason::DoorNeverOpened => {
            "make the switch reachable or use add_door style normal".to_owned()
        }
        BlockerReason::StepTooHigh { .. } => step_hint(blocker),
        BlockerReason::TooLowSector { .. } | BlockerReason::NoHeadroom { .. } => {
            "raise the ceiling (edit_sector) to at least floor + 56".to_owned()
        }
        BlockerReason::Isolated => "connect it (create_sector_from_loop / add_door / \
                                    build_map_from_plan connection)"
            .to_owned(),
    }
}

/// The step hint: the concrete sector pair when the row has one.
fn step_hint(blocker: &Blocker) -> String {
    blocker.from.map_or_else(
        || "add_stairs / add_lift or change floor heights (edit_sector)".to_owned(),
        |from| {
            format!(
                "lower sector {}'s floor to no more than 24 above sector {from}, or add_stairs \
                 / add_lift",
                blocker.sector
            )
        },
    )
}

/// The definition name and category of thing type `kind`, for rows that
/// quote a thing.
fn thing_label(defs: &Definitions, kind: i32) -> (String, String) {
    u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
        .map_or_else(
            || (format!("Unknown ({kind})"), "Unknown".to_owned()),
            |row| (row.name.clone(), row.category.clone()),
        )
}

/// The body radius the stuck test used for `kind`: the definition's, or
/// the monster default for unknown types.
fn body_radius(defs: &Definitions, kind: i32) -> i64 {
    u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
        .map_or(UNKNOWN_THING_RADIUS, |row| i64::from(row.radius))
}

/// The hint that fixes one stuck thing, per the template for its reason.
fn stuck_hint(a: &Analysis<'_>, stuck: &Stuck) -> String {
    let kind = a.map.things.get(stuck.thing).map_or(0_i32, |row| row.kind);
    match stuck.reason {
        StuckReason::OutsideMap => "move it inside a sector (edit_thing)".to_owned(),
        StuckReason::InWall { line } => format!(
            "move it at least {} units away from line {line} (edit_thing)",
            body_radius(a.defs, kind)
        ),
        StuckReason::LowCeiling { .. } => {
            "raise the ceiling (edit_sector) to at least floor + 56, or move it (edit_thing)"
                .to_owned()
        }
        StuckReason::Overlaps { other } => format!(
            "move thing {} or thing {other} apart (edit_thing)",
            stuck.thing
        ),
    }
}

/// The `stuck_things` rows: the pure report with each thing's name and
/// tool-named fix.
fn stuck_rows(a: &Analysis<'_>) -> Vec<StuckRow> {
    a.stuck
        .iter()
        .map(|stuck| StuckRow {
            name: thing_label(
                a.defs,
                a.map.things.get(stuck.thing).map_or(0_i32, |row| row.kind),
            )
            .0,
            stuck: *stuck,
            hint: stuck_hint(a, stuck),
        })
        .collect()
}

/// The `unreachable_things` rows: every thing that spawns at this skill,
/// stands in a resolvable sector, and whose sector the walk never reached.
fn unreachable_rows(a: &Analysis<'_>) -> Vec<UnreachableRow> {
    let mut rows: Vec<UnreachableRow> = Vec::new();
    for (index, row) in a.map.things.iter().enumerate() {
        if !playability::thing_spawns(row.flags, a.family, a.skill) {
            continue;
        }
        let Some(sector) = point_in_sector(a.map, i64::from(row.x), i64::from(row.y)) else {
            continue;
        };
        if a.found.reachable.contains(&sector) {
            continue;
        }
        let (name, category) = thing_label(a.defs, row.kind);
        rows.push(UnreachableRow {
            thing: index,
            name,
            category,
            sector,
        });
    }
    rows
}

/// The plain sentences the report leads with, built from the counts.
fn summary(a: &Analysis<'_>, sectors: &SectorCounts, keys: &[KeyRow]) -> String {
    let mut sentences: Vec<String> = Vec::new();
    if a.found.start_sector.is_none() {
        sentences.push("No player 1 start: nothing can be reached.".to_owned());
    } else if a.exit_reachable() {
        sentences.push("Exit reachable.".to_owned());
    } else if a.found.exits.is_empty() {
        sentences.push("The map has no exit.".to_owned());
    } else {
        sentences.push("No exit is reachable.".to_owned());
    }
    sentences.push(format!(
        "{} of {} sectors reachable.",
        sectors.reachable, sectors.total
    ));
    if let Some(sentence) = count_sentence(a.stuck.len(), "monster", "stuck") {
        sentences.push(sentence);
    }
    if let Some(sentence) = count_sentence(
        keys.iter().filter(|key| !key.reachable).count(),
        "key",
        "unreachable",
    ) {
        sentences.push(sentence);
    }
    sentences.join(" ")
}

/// The "N nouns states." sentence for a nonzero `count`, `None` when the
/// count earns no sentence at all.
fn count_sentence(count: usize, noun: &str, state: &str) -> Option<String> {
    match count {
        0 => None,
        many => Some(format!(
            "{many} {noun}{} {state}.",
            if many == 1_usize { "" } else { "s" }
        )),
    }
}

/// The hints of every problem in fix-priority order: the missing start,
/// the blockers, the stuck things, the unreachable exit.
fn problem_hints(a: &Analysis<'_>, held_out: &[BlockerRow], stuck: &[StuckRow]) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();
    if a.found.start_sector.is_none() {
        hints.push(format!(
            "no player 1 start spawns at {} skill: place one (add_thing type 1)",
            a.skill.as_str()
        ));
    }
    hints.extend(held_out.iter().map(|row| row.hint.clone()));
    hints.extend(stuck.iter().map(|row| row.hint.clone()));
    if !a.exit_reachable() {
        hints.push(
            "the map has no reachable level exit: end a reached sector (add_exit)".to_owned(),
        );
    }
    hints
}

/// The `start` row: the walk's thing and the sector it stands in.
fn start_row(a: &Analysis<'_>) -> Option<StartReport> {
    a.start.map(|index| StartReport {
        thing: index,
        sector: a.found.start_sector,
    })
}

/// Folds the walk, the blockers and the stuck tests into the report.
fn viewed(a: &Analysis<'_>, render: Option<AssetRef>) -> PlayabilityReport {
    let sectors = sector_counts(a);
    let held_out: Vec<BlockerRow> = a
        .held_out
        .iter()
        .map(|blocker| BlockerRow {
            hint: blocker_hint(blocker),
            blocker: blocker.clone(),
        })
        .collect();
    let stuck: Vec<StuckRow> = stuck_rows(a);
    let keys = key_rows(a);
    let next_steps: Vec<String> = if a.is_ok() {
        vec!["render_view from the player start to check the look".to_owned()]
    } else {
        problem_hints(a, &held_out, &stuck)
            .into_iter()
            .take(3_usize)
            .collect()
    };
    PlayabilityReport {
        ok: a.is_ok(),
        summary: summary(a, &sectors, &keys),
        start: start_row(a),
        sectors,
        blockers: held_out,
        exits: exit_rows(a),
        keys,
        locks: a.found.locks.clone(),
        doors_never_opened: a.found.doors_never_opened.clone(),
        narrow_lines: a.found.narrow_lines.clone(),
        stuck_things: stuck,
        unreachable_things: unreachable_rows(a),
        warnings: a.found.warnings.clone(),
        render,
        next_steps,
    }
}

/// The highlighted sector, line and thing indices of one report.
fn marks(a: &Analysis<'_>) -> OverviewMarks {
    let mut lines: BTreeSet<usize> = BTreeSet::new();
    for blocker in &a.held_out {
        lines.extend(blocker.line);
    }
    let mut things: BTreeSet<usize> = BTreeSet::new();
    for stuck in &a.stuck {
        things.insert(stuck.thing);
        if let StuckReason::Overlaps { other } = stuck.reason {
            things.insert(other);
        }
    }
    OverviewMarks {
        sectors: sector_counts(a).unreachable,
        lines: lines.into_iter().collect(),
        things: things.into_iter().collect(),
    }
}

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

/// `check_playability`'s work: resolve the skill before the session is
/// touched, walk the map under one lock, and publish the optional overview
/// through the render group's own machinery.
///
/// # Errors
///
/// [`parse_skill`]'s [`CyberError::InvalidArgument`], [`load`]'s errors and
/// - with `render` - the [`playability_overview`] refusals.
async fn check_run(
    state: &AppState,
    args: &CheckPlayabilityParams,
) -> Result<PlayabilityReport, CyberError> {
    let skill = parse_skill(args.skill.as_deref())?;
    let prepared = load(state, args.map.as_deref())?;
    let family = prepared.map.flag_family();
    let found = analyze_playability(&prepared.map, family, skill);
    let held_out = blockers(&prepared.map, family, &found);
    let stuck = stuck_things(&prepared.map, &prepared.defs, skill);
    let start = playability::start_thing(&prepared.map, family, skill);
    let analysis = Analysis {
        map: &prepared.map,
        defs: &prepared.defs,
        family,
        skill,
        found,
        held_out,
        stuck,
        start,
    };
    let render = if args.render.unwrap_or(false) {
        Some(playability_overview(state, args.map.as_deref(), marks(&analysis)).await?)
    } else {
        None
    };
    Ok(viewed(&analysis, render))
}

/// Default sector-row cap for [`Cyberdemon::describe_map`].
const DESCRIBE_DEFAULT_LIMIT: usize = 150;
/// Largest sector-row cap [`Cyberdemon::describe_map`] accepts.
const DESCRIBE_MAX_LIMIT: usize = 500;

/// Arguments for [`Cyberdemon::describe_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct DescribeMapParams {
    /// The map to describe, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Describe only these sectors, by index; unset describes every sector.
    /// The list is deduplicated and read in ascending order; an index at or
    /// above the sector count fails with `not_found` naming the valid range.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// Skip this many rows of the (possibly filtered) sector list before
    /// returning (default 0). An offset past the end is never an error: the
    /// rows simply come back empty.
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum sector rows returned (default 150, at most 500);
    /// `next_offset` names the offset that continues where this page
    /// stopped.
    #[serde(default)]
    limit: Option<usize>,
    /// Also publish the labelled top-down overview PNG (default `true`):
    /// sector fills, sector numbers and things drawn, the picture
    /// `check_playability`'s `render` draws.
    #[serde(default)]
    render: Option<bool>,
}

/// The sector-row cap `describe_map`'s `limit` asks for,
/// [`DESCRIBE_DEFAULT_LIMIT`] when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above
/// [`DESCRIBE_MAX_LIMIT`].
fn parse_limit(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(limit) = value else {
        return Ok(DESCRIBE_DEFAULT_LIMIT);
    };
    if limit == 0 || limit > DESCRIBE_MAX_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "limit {limit} is out of range: pass a value between 1 and {DESCRIBE_MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

/// The sectors `describe_map` will report, ascending and deduplicated:
/// every stored index when `wanted` is unset, else `wanted` with each
/// index checked against the `count` of sectors `map` holds.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the offending index, the valid
/// `0..count` range and the `sectors` parameter.
fn sector_selection(
    wanted: Option<&[usize]>,
    count: usize,
    map: LumpName,
) -> Result<Vec<usize>, CyberError> {
    let Some(indices) = wanted else {
        return Ok((0..count).collect());
    };
    let mut chosen: BTreeSet<usize> = BTreeSet::new();
    for index in indices {
        if *index >= count {
            return Err(CyberError::NotFound(format!(
                "sector index {index} is out of range for map {map}: it holds {count} sectors, \
                 so `sectors` accepts indices 0..{count} (get_map_summary shows the count)"
            )));
        }
        chosen.insert(*index);
    }
    Ok(chosen.into_iter().collect())
}

/// The `offset`/`limit` window over the selected `indices`: this page's
/// indices and the `next_offset` that continues them (`None` when the page
/// took everything left, which an offset past the end always does).
fn page_rows(indices: &[usize], offset: usize, limit: usize) -> (Vec<usize>, Option<usize>) {
    let start = offset.min(indices.len());
    let stop = start.saturating_add(limit).min(indices.len());
    let rows = indices.get(start..stop).unwrap_or_default().to_vec();
    let next = (stop < indices.len()).then(|| offset.saturating_add(rows.len()));
    (rows, next)
}

/// Structured result of [`Cyberdemon::describe_map`].
#[derive(Debug, Serialize)]
struct DescribeReport {
    /// The map that was described.
    map: LumpName,
    /// The map-wide overview, computed in full whatever the `sectors`
    /// filter and the paging say.
    summary: MapOverview,
    /// The described sector rows, in ascending index order, one page.
    sectors: Vec<SectorFacts>,
    /// The `offset` that returns the rows this page left behind, `null`
    /// when the page took everything.
    next_offset: Option<usize>,
    /// The labelled overview image, `null` unless `render` was asked for.
    image: Option<AssetRef>,
}

/// `describe_map`'s work: check the limit before the session is touched,
/// read the map and the session's definitions under one lock exactly like
/// [`check_run`] does, select and page the sector rows, fold the always-
/// whole map summary, and publish the labelled overview through the render
/// group's own machinery.
///
/// # Errors
///
/// [`parse_limit`]'s and [`sector_selection`]'s refusals, [`load`]'s
/// errors and - with `render` - the [`playability_overview`] refusals.
async fn describe_run(
    state: &AppState,
    args: &DescribeMapParams,
) -> Result<DescribeReport, CyberError> {
    let limit = parse_limit(args.limit)?;
    let prepared = load(state, args.map.as_deref())?;
    let selected = sector_selection(
        args.sectors.as_deref(),
        prepared.map.sectors.len(),
        prepared.map.name,
    )?;
    let (page, next_offset) = page_rows(&selected, args.offset.unwrap_or(0), limit);
    let describe = Describe::new(&prepared.map, &prepared.defs);
    let sectors: Vec<SectorFacts> = page
        .iter()
        .filter_map(|index| describe.sector(*index))
        .collect();
    let summary = describe.summary();
    let image = if args.render.unwrap_or(true) {
        Some(playability_overview(state, args.map.as_deref(), OverviewMarks::default()).await?)
    } else {
        None
    };
    Ok(DescribeReport {
        map: prepared.map.name,
        summary,
        sectors,
        next_offset,
        image,
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
        description = "Walks one map the way a player at `skill` (\"easy\", \"medium\" default or \"hard\") would: from the player 1 start, across every line the player fits through (step up to 24 units, 56 units of headroom, a 32-unit-wide opening), opening the doors, lifts and teleporters whose triggers and keys the walk reaches, and reports {ok, summary, start, sectors, blockers, exits, keys, locks, doors_never_opened, narrow_lines, stuck_things, unreachable_things, warnings, render, next_steps}. ok means a player start stands in a sector, some non-secret exit can be worked from a reached side, and the start itself is not stuck; map works as in the other map tools (merged view). start is {thing, sector} or null when no type-1 thing spawns at the skill; sectors is {total, reachable, unreachable}. blockers names, per unreachable sector and bordering line, the first failing rule as a reason: impassable (the blocking flag), narrow {length}, locked {color}, door_never_opened, step_too_high {delta}, too_low_sector {height}, no_headroom {gap}, or isolated (nothing reachable borders it); each carries a `hint` naming the fix (edit_sector, add_stairs, add_lift, edit_linedef, add_thing, add_door, create_sector_from_loop). exits/keys/locks are the walk's rows (reachable/reached/opened booleans), stuck_things lists monsters, player starts and unknown types (body square 2*radius) that sit in a wall (in_wall {line}), under a ceiling below 56 units (low_ceiling {height}), on top of another body (overlaps {other}) or outside every sector (outside_map); unreachable_things lists spawning things standing in unreached sectors. next_steps carries the hints of the first three problems, or the look-check note when ok. A map without a player start is a normal ok:false result, never an error; unknown maps fail not_found, a bad skill invalid_argument. render: true also publishes a top-down PNG with the unreachable sectors, blocker lines and stuck things highlighted and sector labels on, in the `render` field. Read it after validate_map; fix blockers with the hinted tools and check again."
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
        description = "Describes one map the way its sectors can be read: per-sector rows in ascending index order beside a whole-map `summary`, over the merged view like the other map tools. Each row is {sector, area (even-odd floor square map units), bbox [min_x, min_y, max_x, max_y], center (a label point inside), floor, ceiling, height, light, floor_flat, ceiling_flat, sky, special {value, name} or null, tag, roles, wall_textures (texture name to side count), neighbours ({sector, via, lines, floor_delta} per sector across its two-sided boundary), things ({name, count} by definition name, count descending then name)}. roles name what a sector is: door (a door action raises it, or a shut band at most 8 units tall with two distinct neighbours), lift, teleport_destination, secret, damaging, light_effect, exit_room (a level-end line touches it), stair_step, sky (an F_SKY1 ceiling). A neighbour's via names the way through: door or lift when the neighbour wears that role, window or impassable for a blocking boundary with or without 16 units of light between the higher floor and the lower ceiling, else opening, step_up or step_down by the neighbour's floor. summary is the whole-map picture whatever the rows say: {format, bounds, counts, player_starts, exits, keys, locked_doors, teleporters, secret_sectors, monsters, items, top_wall_textures, top_flats, largest_sectors, playable {ok, summary}}. sectors keeps only those indices (deduplicated, sorted; one at or above the sector count fails not_found naming the valid 0..N range); offset (default 0) and limit (default 150, at most 500; anything else fails invalid_argument) page the list and next_offset names where the rows left behind continue (null when none do). To answer \"the room with the shotgun\", scan the rows' things for that definition name (or wall_textures and roles for a look), take that row's sector index, and confirm on the labelled image: render (default true) publishes the top-down PNG with sector fills, sector numbers and things in image, and a render failure fails the call; render: false answers image null."
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

#[cfg(test)]
mod tests {
    use super::{
        Analysis, CheckPlayabilityParams, DESCRIBE_DEFAULT_LIMIT, DESCRIBE_MAX_LIMIT,
        DescribeMapParams, blocker_hint, check_run, describe_run, page_rows, parse_limit,
        parse_skill, sector_selection, step_hint, stuck_hint,
    };
    use crate::defs::definitions_for;
    use crate::state::AppState;
    use crate::wad::game::Game;
    use crate::wad::map::flags::{FlagFamily, Skill};
    use crate::wad::map::playability::{
        self, Blocker, BlockerReason, KeyColor, Stuck, StuckReason,
    };
    use crate::wad::map::{Map, Thing};
    use crate::wad::name::LumpName;

    /// A `check_playability` call over just these parameter texts.
    fn args(skill: Option<&str>) -> CheckPlayabilityParams {
        CheckPlayabilityParams {
            map: None,
            skill: skill.map(str::to_owned),
            render: None,
        }
    }

    /// A `describe_map` call over just these parameter values, unpinned
    /// fields at their defaults (`render` off so no call ever publishes).
    fn describe_args(sectors: Option<Vec<usize>>, limit: Option<usize>) -> DescribeMapParams {
        DescribeMapParams {
            map: None,
            sectors,
            offset: None,
            limit,
            render: Some(false),
        }
    }

    /// A parsed test map name.
    fn test_name() -> LumpName {
        LumpName::parse("MAP01").expect("test names are valid")
    }

    /// A border blocker on line 17 from sector 2 into sector 5.
    fn border(reason: BlockerReason) -> Blocker {
        Blocker {
            sector: 5,
            from: Some(2),
            line: Some(17),
            reason,
        }
    }

    #[test]
    fn parse_skill_defaults_to_medium_and_names_the_three() {
        assert_eq!(parse_skill(None).expect("absent"), Skill::Medium);
        assert_eq!(
            parse_skill(Some("HARD")).expect("case-insensitive"),
            Skill::Hard
        );
        assert_eq!(parse_skill(Some("easy")).expect("easy"), Skill::Easy);
        let error = parse_skill(Some("nightmare")).expect_err("not a skill");
        assert_eq!(error.code(), "invalid_argument", "bad skill code");
        let message = error.to_string();
        assert!(
            message.contains("\"easy\"")
                && message.contains("\"medium\"")
                && message.contains("\"hard\""),
            "the message names all three values: {message}"
        );
    }

    #[test]
    fn blocker_hints_name_their_fix_tools_per_reason() {
        assert_eq!(
            blocker_hint(&border(BlockerReason::Impassable)),
            "clear the impassable flag on line 17 (edit_linedef)",
        );
        assert_eq!(
            blocker_hint(&border(BlockerReason::Narrow {
                length: Some(24_u64)
            })),
            "widen the opening to at least 32 units (move_vertex)",
        );
        assert_eq!(
            blocker_hint(&border(BlockerReason::Locked {
                color: KeyColor::Blue
            })),
            "place a blue key in a reachable area (add_thing)",
        );
        assert_eq!(
            blocker_hint(&border(BlockerReason::DoorNeverOpened)),
            "make the switch reachable or use add_door style normal",
        );
        assert_eq!(
            blocker_hint(&border(BlockerReason::TooLowSector { height: 40_i32 })),
            "raise the ceiling (edit_sector) to at least floor + 56",
        );
        assert_eq!(
            blocker_hint(&border(BlockerReason::Isolated)),
            "connect it (create_sector_from_loop / add_door / build_map_from_plan connection)",
        );
    }

    #[test]
    fn the_step_hint_names_the_sector_pair_when_there_is_one() {
        assert_eq!(
            step_hint(&border(BlockerReason::StepTooHigh { delta: 40_i64 })),
            "lower sector 5's floor to no more than 24 above sector 2, or add_stairs / add_lift",
        );
        let lonely = Blocker {
            sector: 5,
            from: None,
            line: None,
            reason: BlockerReason::StepTooHigh { delta: 40_i64 },
        };
        assert_eq!(
            step_hint(&lonely),
            "add_stairs / add_lift or change floor heights (edit_sector)",
        );
    }

    /// The stuck hint for a thing of `kind` at the head of a two-row map,
    /// read against the built-in `DOOM II` definitions.
    fn stuck_hint_for(kind: i32, reason: StuckReason) -> String {
        let map = Map {
            things: vec![
                Thing {
                    kind,
                    ..Default::default()
                },
                Thing {
                    kind: 3001,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let defs = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
        let a = Analysis {
            map: &map,
            defs: &defs,
            family: FlagFamily::Doom,
            skill: Skill::Medium,
            found: playability::Playability::default(),
            held_out: Vec::new(),
            stuck: Vec::new(),
            start: None,
        };
        stuck_hint(&a, &Stuck { thing: 0, reason })
    }

    #[test]
    fn stuck_hints_quote_the_line_radius_and_the_partner() {
        assert_eq!(
            stuck_hint_for(3001, StuckReason::InWall { line: 12 }),
            "move it at least 20 units away from line 12 (edit_thing)",
            "the imp's own radius"
        );
        assert_eq!(
            stuck_hint_for(8, StuckReason::InWall { line: 12 }),
            "move it at least 16 units away from line 12 (edit_thing)",
            "the backpack's own radius rides along"
        );
        assert_eq!(
            stuck_hint_for(3001, StuckReason::Overlaps { other: 1 }),
            "move thing 0 or thing 1 apart (edit_thing)",
        );
        assert_eq!(
            stuck_hint_for(3001, StuckReason::OutsideMap),
            "move it inside a sector (edit_thing)",
        );
        assert!(
            stuck_hint_for(3001, StuckReason::LowCeiling { height: 30_i32 })
                .contains("edit_sector"),
            "the low-ceiling hint names both fixes"
        );
    }

    #[tokio::test]
    async fn check_run_refuses_a_bad_skill_and_the_empty_session() {
        let state = AppState::default();
        let refused = check_run(&state, &args(Some("nightmare")))
            .await
            .expect_err("not a skill");
        assert_eq!(
            refused.code(),
            "invalid_argument",
            "a bad skill is refused even with no WAD open"
        );
        assert_eq!(
            check_run(&state, &args(None))
                .await
                .expect_err("no maps at all")
                .code(),
            "invalid_argument",
            "the empty-session no-map refusal comes from the shared resolver"
        );
    }

    #[test]
    fn parse_limit_defaults_to_a_page_and_names_the_range_when_refusing() {
        assert_eq!(
            parse_limit(None).expect("absent"),
            DESCRIBE_DEFAULT_LIMIT,
            "the default pages a hundred and fifty rows"
        );
        assert_eq!(parse_limit(Some(1)).expect("one"), 1, "one row is legal");
        assert_eq!(
            parse_limit(Some(DESCRIBE_MAX_LIMIT)).expect("max"),
            DESCRIBE_MAX_LIMIT,
            "the cap itself is legal"
        );
        for bad in [0_usize, DESCRIBE_MAX_LIMIT.checked_add(1).expect("small")] {
            let error = parse_limit(Some(bad)).expect_err("this cap is out of range");
            assert_eq!(error.code(), "invalid_argument", "bad limit code");
            assert!(
                error.to_string().contains("between 1 and 500"),
                "the message names the range: {error}"
            );
        }
    }

    #[test]
    fn sector_selection_sorts_dedupes_and_defaults_to_every_sector() {
        assert_eq!(
            sector_selection(None, 3, test_name()).expect("no filter"),
            vec![0_usize, 1, 2],
            "unset describes every stored sector"
        );
        assert!(
            sector_selection(None, 0, test_name())
                .expect("no sectors")
                .is_empty(),
            "an empty map selects nothing"
        );
        let wanted = vec![2_usize, 0, 2];
        assert_eq!(
            sector_selection(Some(&wanted), 5, test_name()).expect("two rows asked"),
            vec![0_usize, 2],
            "deduplicated and ascending"
        );
    }

    #[test]
    fn sector_selection_refuses_past_the_end_with_the_valid_range() {
        let error =
            sector_selection(Some(&[5_usize]), 5, test_name()).expect_err("5 is not below 5");
        assert_eq!(error.code(), "not_found", "a past-the-end index refuses");
        let message = error.to_string();
        assert!(
            message.contains("0..5") && message.contains("`sectors`"),
            "the message names the valid range and the parameter: {message}"
        );
        let empty =
            sector_selection(Some(&[0_usize]), 0, test_name()).expect_err("nothing to index into");
        assert!(
            empty.to_string().contains("0..0"),
            "an empty map's range is empty too: {empty}"
        );
    }

    #[test]
    fn page_rows_windows_the_selection_and_points_at_the_rest() {
        let all: Vec<usize> = (0..5).collect();
        let (first, next) = page_rows(&all, 0, 2);
        assert_eq!(first, vec![0_usize, 1], "the head of the list");
        assert_eq!(next, Some(2), "and where the rest continues");
        let (tail, done) = page_rows(&all, 3, 500);
        assert_eq!(
            tail,
            vec![3_usize, 4],
            "a page longer than the rest stops at it"
        );
        assert_eq!(done, None, "with nothing left behind");
        let (nothing, past) = page_rows(&all, 9, 2);
        assert!(nothing.is_empty(), "an offset past the end is no error");
        assert_eq!(past, None, "and it has no next page");
        let (empty, empty_next) = page_rows(&[], 0, DESCRIBE_DEFAULT_LIMIT);
        assert!(empty.is_empty(), "nothing selected, nothing paged");
        assert_eq!(empty_next, None, "and no next offset to chase");
    }

    #[tokio::test]
    async fn describe_run_refuses_a_bad_limit_and_the_empty_session() {
        let state = AppState::default();
        let refused = describe_run(&state, &describe_args(None, Some(0)))
            .await
            .expect_err("zero rows");
        assert_eq!(
            refused.code(),
            "invalid_argument",
            "a bad limit is refused even with no WAD open"
        );
        assert_eq!(
            describe_run(&state, &describe_args(None, Some(501)))
                .await
                .expect_err("rows over the cap")
                .code(),
            "invalid_argument",
            "and so is one above the maximum page"
        );
        assert_eq!(
            describe_run(&state, &describe_args(None, None))
                .await
                .expect_err("no maps at all")
                .code(),
            "invalid_argument",
            "the empty-session no-map refusal comes from the shared resolver"
        );
    }
}
