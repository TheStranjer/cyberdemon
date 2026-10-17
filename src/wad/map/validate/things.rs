//! Thing checks over a decoded [`Map`]: the player starts a map needs to
//! spawn anyone at all, the coop starts it is better to have, and thing
//! types the session's definitions do not know.
//!
//! The type check uses [`Context::definitions`] - the built-in game table
//! overlaid with the loaded archives' DEHACKED, DECORATE/`ZScript` and
//! MAPINFO customs - so a total conversion's own numbers stay quiet.

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use super::{Context, Issue, Severity, listed};
use crate::wad::map::model::Map;

/// The single-player start: without it no player can enter the map.
const PLAYER1_START: i32 = 1;

/// The cooperative starts a map without them cannot host `coop`.
const COOP_STARTS: [i32; 3] = [2, 3, 4];

/// Whether the map holds a thing of type `kind`.
fn holds_thing(map: &Map, kind: i32) -> bool {
    map.things.iter().any(|placed| placed.kind == kind)
}

/// Maps without any thing of type 1.
pub(super) fn no_player1_start(map: &Map) -> Vec<Issue> {
    if holds_thing(map, PLAYER1_START) {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Error,
        format!(
            "the map holds no thing of type {PLAYER1_START} (player 1 start), so no player can \
             spawn"
        ),
    )]
}

/// Maps missing one or more of the coop starts (types 2 to 4).
pub(super) fn missing_coop_starts(map: &Map) -> Vec<Issue> {
    let missing: Vec<i32> = COOP_STARTS
        .into_iter()
        .filter(|kind| !holds_thing(map, *kind))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Info,
        format!(
            "coop starts are missing: no thing of type {}",
            listed(&missing)
        ),
    )]
}

/// Things whose type number no session definition lists.
pub(super) fn unknown_thing_type(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    map.things
        .iter()
        .enumerate()
        .filter(|&(_, placed)| {
            !u32::try_from(placed.kind).is_ok_and(|kind| context.definitions.thing(kind).is_some())
        })
        .map(|(index, placed)| {
            Issue::new(
                Severity::Warning,
                format!(
                    "thing {index} has type {}, which the session's definitions do not list",
                    placed.kind,
                ),
            )
            .thing(&[index])
            .at(placed.x, placed.y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{missing_coop_starts, no_player1_start, unknown_thing_type};
    use crate::defs::definitions_for;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::model::{Map, Thing};
    use crate::wad::map::validate::Context;
    use crate::wad::name::LumpName;

    /// A thing of `kind` at `x`/`y`.
    const fn thing_at(kind: i32, x: i32, y: i32) -> Thing {
        Thing {
            tid: 0,
            x,
            y,
            z: 0,
            angle: 0,
            kind,
            flags: 7,
            special: 0,
            args: [0; 5],
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A `Doom` map holding only `things`.
    fn map_with(things: Vec<Thing>) -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("test names are valid"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things,
            ..Default::default()
        }
    }

    /// The built-in DOOM II and `Hexen` rows every context test needs.
    fn tables() -> (crate::defs::Definitions, crate::defs::Definitions) {
        (
            definitions_for(Game::Doom2).expect("the embedded Doom II table is valid"),
            definitions_for(Game::Hexen).expect("the embedded Hexen table is valid"),
        )
    }

    #[test]
    fn no_player1_start_reports_maps_without_type_1() {
        let empty = map_with(Vec::new());
        let issues = no_player1_start(&empty);
        let issue = issues.first().expect("no start, one error");
        assert!(issue.things.is_empty(), "nothing to point at");
        assert!(
            issue.message.contains("type 1"),
            "the message names the number: {}",
            issue.message
        );
        let monster = map_with(vec![thing_at(3001, 100, 100)]);
        assert_eq!(no_player1_start(&monster).len(), 1, "an imp is not a start");
        let solo = map_with(vec![thing_at(3001, 100, 100), thing_at(1, 64, 64)]);
        assert!(no_player1_start(&solo).is_empty(), "the start settles it");
    }

    #[test]
    fn missing_coop_starts_lists_only_the_absent_numbers() {
        let solo = map_with(vec![thing_at(1, 64, 64)]);
        let issues = missing_coop_starts(&solo);
        let issue = issues.first().expect("a solo start misses the coop set");
        assert!(
            issue.message.contains("2, 3, 4"),
            "all three numbers are missing: {}",
            issue.message
        );
        let mut partial = solo;
        partial.things.push(thing_at(2, 64, 96));
        partial.things.push(thing_at(4, 64, 128));
        let fewer = missing_coop_starts(&partial);
        let row = fewer.first().expect("type 3 is still missing");
        assert!(
            row.message.contains("no thing of type 3"),
            "only 3 is named: {}",
            row.message
        );
        partial.things.push(thing_at(3, 64, 160));
        assert!(
            missing_coop_starts(&partial).is_empty(),
            "types 1 to 4 present: nothing to say"
        );
    }

    #[test]
    fn unknown_thing_type_names_the_unlisted_numbers() {
        let (doom, hexen) = tables();
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
        };
        let mut map = map_with(vec![thing_at(1, 64, 64), thing_at(3001, 384, 128)]);
        assert!(
            unknown_thing_type(&map, &context).is_empty(),
            "types 1 and 3001 are DOOM II rows"
        );
        map.things.push(thing_at(30_010, 10, 10));
        let issues = unknown_thing_type(&map, &context);
        let issue = issues.first().expect("30010 is not a DOOM II type");
        assert_eq!(issue.things, vec![2], "the offending thing is attached");
        assert_eq!(issue.x, Some(10_i32), "and its position");
        assert_eq!(issue.y, Some(10_i32), "in map units");
        assert!(
            issue.message.contains("30010"),
            "the message names the type: {}",
            issue.message
        );
        map.things.push(thing_at(-5, 20, 20));
        let more = unknown_thing_type(&map, &context);
        assert_eq!(more.len(), 2, "a negative type cannot be a row");
    }
}
