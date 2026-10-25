//! The counting passes: the whole-map folds [`Describe::summary`] asks for
//! — [`element_counts`], [`player_starts`], the [`category_counts`] of
//! monsters and items, the [`wall_tally`] and [`flat_tally`] behind the
//! [`MapOverview`]'s top rows and the [`exit_reachable`], [`start_stuck`]
//! and [`overview_summary`] readings of the playable answer — beside the
//! per-sector folds one [`SectorFacts`] row carries: [`special_info`],
//! [`wall_textures`] and [`thing_counts`].
//!
//! Every pass here is a plain fold over a [`Map`] and the passed
//! [`Definitions`]: it walks stored rows and answers names and counts, and
//! never measures geometry nor reads the map's actions.
//!

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString as _;
use alloc::vec::Vec;

use crate::defs::{Definitions, ThingDef};
use crate::wad::map::consts::{NO_TEXTURE, PLAYER_START_TYPES};
use crate::wad::map::geometry::{point_in_sector, sector_boundary};
use crate::wad::map::model::Map;
use crate::wad::map::playability::{Playability, Stuck, StuckReason};
use crate::wad::map::stats::ElementCounts;

use super::{PlayerStart, SpecialInfo, ThingCount};

/// The rows `MapOverview::top_wall_textures` and `MapOverview::top_flats`
/// answer with.
const TOP_ROWS: usize = 10_usize;
/// The overview's note when no player start resolves to a sector.
const NO_START_SENTENCE: &str = "No player 1 start: nothing can be reached.";
/// The overview's note when a non-secret exit stands on a reached side.
const EXIT_REACHABLE_SENTENCE: &str = "Exit reachable.";
/// The overview's note when the map carries no exit line at all.
const NO_EXIT_SENTENCE: &str = "The map has no exit.";
/// The overview's note when every exit line stayed out of the walk.
const EXIT_UNREACHABLE_SENTENCE: &str = "No exit is reachable.";

/// How many rows each of `map`'s five element blocks holds.
pub(super) fn element_counts(map: &Map) -> ElementCounts {
    ElementCounts {
        things: map.things.len(),
        vertices: map.vertices.len(),
        linedefs: map.linedefs.len(),
        sidedefs: map.sidedefs.len(),
        sectors: map.sectors.len(),
    }
}

/// The things typed [`PLAYER_START_TYPES`], in thing order.
pub(super) fn player_starts(map: &Map) -> Vec<PlayerStart> {
    map.things
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            PLAYER_START_TYPES
                .contains(&row.kind)
                .then_some(PlayerStart {
                    thing: index,
                    kind: row.kind,
                })
        })
        .collect()
}

/// The definition row of thing type `kind`, `None` for a negative type or
/// one the game does not define.
fn thing_def(defs: &Definitions, kind: i32) -> Option<&ThingDef> {
    u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
}

/// The things whose definitions stand under one of `categories` (a
/// category reads as a match when it starts with it), counted by display
/// name; unknown and absent definitions count under neither tally.
pub(super) fn category_counts(
    map: &Map,
    defs: &Definitions,
    categories: &[&str],
) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.things {
        let Some(def) = thing_def(defs, row.kind) else {
            continue;
        };
        if categories.iter().any(|name| def.category.starts_with(name)) {
            bump(&mut counts, &def.name);
        }
    }
    counts
}

/// Every texture name `map`'s sidedefs wear: each side's upper, middle
/// and lower face counts whatever the side opens into; `"-"` never does.
pub(super) fn wall_tally(map: &Map) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.sidedefs {
        for texture in [row.upper.as_str(), row.middle.as_str(), row.lower.as_str()] {
            bump_named(&mut counts, texture);
        }
    }
    counts
}

/// Every flat name `map`'s sectors wear: floor and ceiling alike, `"-"`
/// never counted.
pub(super) fn flat_tally(map: &Map) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.sectors {
        for flat in [row.floor_flat.as_str(), row.ceiling_flat.as_str()] {
            bump_named(&mut counts, flat);
        }
    }
    counts
}

/// The [`TOP_ROWS`] busiest names of `counts`: count descending, ties by
/// name ascending; fewer rows when the map wears fewer names.
pub(super) fn top_rows(counts: BTreeMap<String, u32>) -> Vec<(String, u32)> {
    let mut rows: Vec<(String, u32)> = counts.into_iter().collect();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.truncate(TOP_ROWS);
    rows
}

/// Whether the folded walk can work some non-secret exit from a side it
/// reached: the `check_playability` tool's rule, read here because this
/// layer may not import the tool layer.
pub(super) fn exit_reachable(play: &Playability) -> bool {
    play.exits.iter().any(|exit| !exit.secret && exit.reachable)
}

/// Whether the player start thing is one of the stuck rows (its own row,
/// or an overlap row naming it as the other body).
pub(super) fn start_stuck(start: Option<usize>, stuck: &[Stuck]) -> bool {
    let Some(index) = start else {
        return false;
    };
    stuck.iter().any(|row| {
        row.thing == index
            || matches!(row.reason, StuckReason::Overlaps { other } if other == index)
    })
}

/// The overview's pair of plain sentences: the exit note (or the no-start
/// note) and the reachable-sector count, worded as `check_playability`
/// words its own summary.
pub(super) fn overview_summary(play: &Playability, sectors: usize, reached_exit: bool) -> String {
    let lead = if play.start_sector.is_none() {
        NO_START_SENTENCE
    } else if reached_exit {
        EXIT_REACHABLE_SENTENCE
    } else if play.exits.is_empty() {
        NO_EXIT_SENTENCE
    } else {
        EXIT_UNREACHABLE_SENTENCE
    };
    let reached = play.reachable.len();
    format!("{lead} {reached} of {sectors} sectors reachable.")
}

/// The [`SpecialInfo`] of sector special `value`: `None` for the plain `0`
/// that means "no effect", the table's name for a number
/// [`Definitions::sector_type`] knows, and the plain number as text for
/// everything else.
pub(super) fn special_info(value: i32, defs: &Definitions) -> Option<SpecialInfo> {
    if value == 0_i32 {
        return None;
    }
    let name = u32::try_from(value)
        .ok()
        .and_then(|number| defs.sector_type(number))
        .map_or_else(|| value.to_string(), |row| row.name.clone());
    Some(SpecialInfo { value, name })
}

/// The texture tally of the sides opening into `sector`: for every
/// boundary line, each of its two sides whose own sector resolves to
/// `sector` contributes its upper, lower and middle names; `"-"` never
/// counts and a dangling side reference contributes nothing.
pub(super) fn wall_textures(map: &Map, sector: usize) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (index, _) in sector_boundary(map, sector) {
        let Some(line) = map.linedefs.get(index) else {
            continue;
        };
        for side in [line.front, line.back].into_iter().flatten() {
            count_side(&mut counts, map, side, sector);
        }
    }
    counts
}

/// Record the three texture names of `map`'s sidedef `side` in `counts`,
/// when that side opens into `sector`.
fn count_side(counts: &mut BTreeMap<String, u32>, map: &Map, side: usize, sector: usize) {
    let Some(row) = map.sidedefs.get(side) else {
        return;
    };
    if row.sector != sector {
        return;
    }
    for texture in [row.upper.as_str(), row.lower.as_str(), row.middle.as_str()] {
        bump_named(counts, texture);
    }
}

/// The display name of thing type `kind`: the table's name for a type
/// [`Definitions::thing`] knows, `Unknown <kind>` for anything else
/// (including a negative type number).
fn thing_name(defs: &Definitions, kind: i32) -> String {
    thing_def(defs, kind).map_or_else(|| format!("Unknown {kind}"), |row| row.name.clone())
}

/// Every thing that stands in `sector` - skill-independent, whatever its
/// presence bits say - counted by definition name and sorted by count
/// descending, then by name ascending.
pub(super) fn thing_counts(map: &Map, defs: &Definitions, sector: usize) -> Vec<ThingCount> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.things {
        let at = point_in_sector(map, i64::from(row.x), i64::from(row.y));
        if at != Some(sector) {
            continue;
        }
        bump(&mut counts, &thing_name(defs, row.kind));
    }
    let mut rows: Vec<ThingCount> = counts
        .into_iter()
        .map(|(name, count)| ThingCount { name, count })
        .collect();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// Record `name` once more in `counts`.
fn bump(counts: &mut BTreeMap<String, u32>, name: &str) {
    let slot = counts.entry(name.to_owned()).or_insert(0_u32);
    *slot = slot.saturating_add(1);
}

/// Record the texture or flat name `name` once more in `counts`; the
/// model's `"-"` (and nothing else) never counts.
fn bump_named(counts: &mut BTreeMap<String, u32>, name: &str) {
    if name != NO_TEXTURE {
        bump(counts, name);
    }
}
