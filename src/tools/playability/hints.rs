//! The tool-named fixes a playability report quotes: every blocker reason and
//! every stuck reason answered with the one short sentence that says what to
//! do, naming the tool that does it.
//!
//! The pure [`crate::wad::map::playability`] module measures, this module
//! translates each [`BlockerReason`] and [`StuckReason`] into that sentence and
//! dresses the [`Stuck`] rows and the unreachable ones with the thing's
//! definition name and category. The shared
//! [`UNKNOWN_THING_RADIUS`](super::UNKNOWN_THING_RADIUS) stands in for the body
//! half-side the definitions do not carry.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::wad::map::consts::{MAX_STEP_HEIGHT, MIN_LINE_WIDTH, PLAYER_HEIGHT};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::playability::{Blocker, BlockerReason, Stuck, StuckReason, thing_spawns};

use super::UNKNOWN_THING_RADIUS;
use super::check::{Analysis, StuckRow, UnreachableRow};

/// The hint that fixes one blocker, per the fixed template for its reason.
pub(super) fn blocker_hint(blocker: &Blocker) -> String {
    match blocker.reason {
        BlockerReason::Impassable => blocker.line.map_or_else(
            || "clear the impassable flag on this line (edit_linedef)".to_owned(),
            |line| format!("clear the impassable flag on line {line} (edit_linedef)"),
        ),
        BlockerReason::Narrow { .. } => {
            format!("widen the opening to at least {MIN_LINE_WIDTH} units (move_vertex)")
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
            format!("raise the ceiling (edit_sector) to at least floor + {PLAYER_HEIGHT}")
        }
        BlockerReason::Isolated => "connect it (create_sector_from_loop / add_door / \
                                    build_map_from_plan connection)"
            .to_owned(),
    }
}

/// The step hint: the concrete sector pair when the row has one.
pub(super) fn step_hint(blocker: &Blocker) -> String {
    blocker.from.map_or_else(
        || "add_stairs / add_lift or change floor heights (edit_sector)".to_owned(),
        |from| {
            format!(
                "lower sector {}'s floor to no more than {MAX_STEP_HEIGHT} above sector {from}, \
                 or add_stairs / add_lift",
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
pub(super) fn stuck_hint(a: &Analysis<'_>, stuck: &Stuck) -> String {
    let kind = a.map.things.get(stuck.thing).map_or(0_i32, |row| row.kind);
    match stuck.reason {
        StuckReason::OutsideMap => "move it inside a sector (edit_thing)".to_owned(),
        StuckReason::InWall { line } => format!(
            "move it at least {} units away from line {line} (edit_thing)",
            body_radius(a.defs, kind)
        ),
        StuckReason::LowCeiling { .. } => format!(
            "raise the ceiling (edit_sector) to at least floor + {PLAYER_HEIGHT}, or move it \
             (edit_thing)"
        ),
        StuckReason::Overlaps { other } => format!(
            "move thing {} or thing {other} apart (edit_thing)",
            stuck.thing
        ),
    }
}

/// The `stuck_things` rows: the pure report with each thing's name and
/// tool-named fix.
pub(super) fn stuck_rows(a: &Analysis<'_>) -> Vec<StuckRow> {
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
pub(super) fn unreachable_rows(a: &Analysis<'_>) -> Vec<UnreachableRow> {
    let mut rows: Vec<UnreachableRow> = Vec::new();
    for (index, row) in a.map.things.iter().enumerate() {
        if !thing_spawns(row.flags, a.family, a.skill) {
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
