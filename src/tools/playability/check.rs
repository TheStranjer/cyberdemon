//! `check_playability`'s side of the playability group: one walk of the map
//! folded into the report a calling model acts on.
//!
//! [`load`](super::load) reads the map, the session's definitions and the
//! render settings under one lock, the pure [`crate::wad::map::playability`]
//! module walks them and explains the result, [`super::hints`] words each
//! fix and [`viewed`] folds both into [`PlayabilityReport`]: `ok`, the plain
//! `summary`, the sector counts, the blocker, exit, key, lock, stuck and
//! unreachable rows, the highlights [`marks`] hands the render group's
//! overview — painted from those very loaded inputs — and the first three
//! `next_steps` hints. A map with no player start is a normal `ok: false`
//! report, never an error.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::Definitions;
use crate::error::CyberError;
use crate::named::parse_named;
use crate::state::AppState;
use crate::tools::render::{OverviewMarks, playability_overview};
use crate::wad::map::Map;
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::playability::{
    self, Blocker, LockInfo, Playability, Stuck, StuckReason, analyze_playability, blockers,
    stuck_things,
};

use super::hints::{blocker_hint, stuck_rows, unreachable_rows};
use super::load;

/// Arguments for [`crate::server::Cyberdemon::check_playability`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CheckPlayabilityParams {
    /// The map to check, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Which difficulty to walk the map at: `"easy"`, `"medium"` (the
    /// default) or `"hard"`. Thing presence bits decide which starts, keys
    /// and monsters exist at the skill.
    #[serde(default)]
    pub(super) skill: Option<String>,
    /// Also publish a top-down overview PNG with the unreachable sectors,
    /// blocker lines and stuck things highlighted (default `false`).
    #[serde(default)]
    pub(super) render: Option<bool>,
}

/// The skill `skill` names, [`Skill::Medium`] when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the three accepted values.
pub(super) fn parse_skill(text: Option<&str>) -> Result<Skill, CyberError> {
    let Some(name) = text else {
        return Ok(Skill::Medium);
    };
    parse_named("skill", name)
}

/// Everything one report is folded out of: the read inputs and the three
/// pure analyses over them.
pub(super) struct Analysis<'ctx> {
    /// The map that was walked.
    pub(super) map: &'ctx Map,
    /// The map's per-sector geometry, indexed once for the report: the
    /// unreachable rows answer every thing's home sector from it.
    pub(super) geometry: SectorIndex,
    /// The session's definitions the body tests read.
    pub(super) defs: &'ctx Definitions,
    /// The flag family the map's lines are stored in.
    pub(super) family: FlagFamily,
    /// The skill the walk was run at.
    pub(super) skill: Skill,
    /// The reachability walk's answer.
    pub(super) found: Playability,
    /// Why each unreachable sector stayed out of reach.
    pub(super) held_out: Vec<Blocker>,
    /// Which things cannot stand where they are.
    pub(super) stuck: Vec<Stuck>,
    /// The player 1 start the walk began from, if one spawns.
    pub(super) start: Option<usize>,
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
pub(super) struct StuckRow {
    /// The stuck thing.
    #[serde(flatten)]
    pub(super) stuck: Stuck,
    /// The definition name of the thing's type.
    pub(super) name: String,
    /// How to unstick it, naming the tool that does it.
    pub(super) hint: String,
}

/// One `unreachable_things` row.
#[derive(Debug, Serialize)]
pub(super) struct UnreachableRow {
    /// The index into `THINGS`.
    pub(super) thing: usize,
    /// The definition name of the thing's type.
    pub(super) name: String,
    /// The definition category the thing stands under.
    pub(super) category: String,
    /// The unreached sector it stands in.
    pub(super) sector: usize,
}

/// Structured result of [`crate::server::Cyberdemon::check_playability`].
#[derive(Debug, Serialize)]
pub(super) struct PlayabilityReport {
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

/// `check_playability`'s work: resolve the skill before the session is
/// touched, walk the map under one lock, and publish the optional overview
/// from the inputs that same lock loaded, through the render group's own
/// machinery.
///
/// # Errors
///
/// [`parse_skill`]'s [`CyberError::InvalidArgument`], [`load`]'s errors and
/// - with `render` - the [`playability_overview`] refusals.
pub(super) async fn check_run(
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
        geometry: SectorIndex::new(&prepared.map),
        defs: &prepared.defs,
        family,
        skill,
        found,
        held_out,
        stuck,
        start,
    };
    let render = if args.render.unwrap_or(false) {
        Some(playability_overview(state, &prepared, marks(&analysis)).await?)
    } else {
        None
    };
    Ok(viewed(&analysis, render))
}
