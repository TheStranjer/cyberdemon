//! The whole-map overview from [`crate::wad::map::describe::MapOverview`]:
//! the one [`Describe`] that answers every sector like the wrapper does,
//! and the map-wide summary the overview tests build — counts, bounds,
//! sizes and tallies, the walk's exit, key and lock rows, the secret and
//! largest sectors and the playable sentences — over the two-room hall,
//! the keyed-exit plan and the empty map.

use alloc::vec;

use serde_json::{json, to_value};

use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::spawn;
use crate::wad::map::describe::{Describe, PlayerStart, sector_facts};
use crate::wad::map::playability::{KeyColor, KeyInfo};

use super::plans::{built, crewed_plan, keyed_exit_plan, plan_rooms};
use super::{cell, doom_defs, doom2_defs, hall, row, slab, stand};

#[test]
fn one_describe_answers_every_sector_like_the_wrapper() {
    let map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs);
    assert_eq!(
        describe.sector(9),
        None,
        "an index past the sectors still answers nothing"
    );
    for index in [0_usize, 1, 2] {
        assert_eq!(
            describe.sector(index),
            sector_facts(&map, &doom_defs(), index),
            "the wrapper and the shared read agree on sector {index}"
        );
    }
}

#[test]
fn the_two_room_hall_overviews_counts_sizes_tallies_and_the_missing_exit() {
    let map = hall(&[cell(0_i32), cell(0_i32)]);
    let overview = Describe::new(&map, &doom2_defs()).summary();
    assert_eq!(overview.format, MapFormat::Doom, "the stored format");
    assert_eq!(
        overview.bounds,
        Some([0_i32, 0_i32, 512_i32, 256_i32]),
        "the vertex box"
    );
    assert_eq!(overview.counts.vertices, 6, "two rows of columns");
    assert_eq!(overview.counts.linedefs, 7, "one divider and six walls");
    assert_eq!(
        overview.counts.sidedefs, 8,
        "two on the divider, one per wall"
    );
    assert_eq!(overview.counts.sectors, 2, "the two rooms");
    assert_eq!(overview.counts.things, 1, "just the start");
    assert_eq!(
        overview.player_starts,
        vec![PlayerStart { thing: 0, kind: 1 }],
        "the type-1 row, in thing order"
    );
    assert!(overview.exits.is_empty(), "the hall ends nowhere");
    assert!(overview.keys.is_empty(), "and keeps no keys");
    assert!(overview.locked_doors.is_empty(), "and no locks");
    assert!(overview.teleporters.is_empty(), "and no pads");
    assert!(overview.secret_sectors.is_empty(), "and hides nothing");
    assert!(overview.monsters.is_empty(), "only the player stands here");
    assert!(overview.items.is_empty(), "and the start is no item");
    assert_eq!(
        overview.top_wall_textures,
        vec![("WALL1".to_owned(), 6_u32)],
        "six WALL1 faces and nothing else"
    );
    assert_eq!(
        overview.top_flats,
        vec![("FLOOR1".to_owned(), 4_u32)],
        "two floors and two ceilings"
    );
    assert_eq!(
        overview.largest_sectors,
        vec![0_usize, 1],
        "equal rooms tie and fall back to index order"
    );
    assert!(!overview.playable.ok, "nothing ends the level");
    assert_eq!(
        overview.playable.summary, "The map has no exit. 2 of 2 sectors reachable.",
        "the tool's own wording, both sentences"
    );
}

#[test]
fn the_hall_overview_serialises_to_the_tickets_shape() {
    let map = hall(&[cell(0_i32), cell(0_i32)]);
    let value =
        to_value(Describe::new(&map, &doom2_defs()).summary()).expect("the overview serialises");
    assert_eq!(
        value,
        json!({
            "format": "doom",
            "bounds": [0_i64, 0_i64, 512_i64, 256_i64],
            "counts": {
                "vertices": 6_u64,
                "linedefs": 7_u64,
                "sidedefs": 8_u64,
                "sectors": 2_u64,
                "things": 1_u64,
            },
            "player_starts": [{ "thing": 0_u64, "kind": 1_i64 }],
            "exits": [],
            "keys": [],
            "locked_doors": [],
            "teleporters": [],
            "secret_sectors": [],
            "monsters": {},
            "items": {},
            "top_wall_textures": [["WALL1", 6_u64]],
            "top_flats": [["FLOOR1", 4_u64]],
            "largest_sectors": [0_u64, 1_u64],
            "playable": {
                "ok": false,
                "summary": "The map has no exit. 2 of 2 sectors reachable.",
            },
        }),
        "every field the ticket named, in its names"
    );
}

#[test]
fn the_keyed_exit_plan_overviews_its_walk_and_playable_rows() {
    let (map, _) = crewed_plan();
    let overview = Describe::new(&map, &doom2_defs()).summary();
    assert!(
        overview.playable.ok,
        "the card in hand walks the whole map: {:?}",
        overview.playable
    );
    assert_eq!(
        overview.playable.summary, "Exit reachable. 7 of 7 sectors reachable.",
        "start room, band, hall, exit room, secret and both opening strips stand on the path"
    );
    assert!(
        overview
            .exits
            .iter()
            .any(|exit| !exit.secret && exit.reachable),
        "the walk-over exit is worked from a reached side: {:?}",
        overview.exits
    );
    assert_eq!(
        overview.keys,
        vec![KeyInfo {
            thing: 1,
            color: KeyColor::Blue,
            sector: Some(plan_rooms(&map)[0]),
            reachable: true,
        }],
        "the card waits in the start room"
    );
    assert!(
        !overview.locked_doors.is_empty()
            && overview
                .locked_doors
                .iter()
                .all(|lock| lock.color == KeyColor::Blue && lock.opened),
        "every door lock rose for the card: {:?}",
        overview.locked_doors
    );
    assert!(
        overview.teleporters.is_empty(),
        "the plan teleports nowhere"
    );
}

#[test]
fn the_keyed_exit_plan_overviews_its_crew_and_sizes() {
    let (map, rows) = crewed_plan();
    let overview = Describe::new(&map, &doom2_defs()).summary();
    let secret = *row(&rows, "secret")
        .sectors
        .first()
        .expect("the secret room");
    assert_eq!(
        overview.secret_sectors,
        vec![secret],
        "and the secret room alone"
    );
    assert_eq!(
        to_value(&overview.monsters).expect("monsters serialise"),
        json!({ "Former Human": 1_u64, "Imp": 1_u64 }),
        "counted by definition name"
    );
    assert_eq!(
        to_value(&overview.items).expect("items serialise"),
        json!({
            "Backpack": 1_u64,
            "Blue Keycard": 1_u64,
            "Shotgun": 1_u64,
            "Stimpack": 1_u64,
        }),
        "weapons, ammunition, the key and health alike"
    );
    let band = *row(&rows, "door").sectors.first().expect("the band");
    let rooms = plan_rooms(&map);
    // Sector areas: the outer rooms `a`/`s` (63488 square units)
    // outrank the inner `b`/`c` (61440); the door band and both
    // opening strips tie at 1024, so the lowest index wins the
    // fifth row.
    assert_eq!(
        overview.largest_sectors,
        vec![rooms[0], rooms[3], rooms[1], rooms[2], band],
        "biggest first, ties by index, the band claiming the fifth row"
    );
    assert_eq!(
        overview.counts.sectors, 7,
        "four rooms, the band, two strips"
    );
    assert!(overview.bounds.is_some(), "and the built map bounds itself");
    assert!(
        !overview.top_wall_textures.is_empty() && !overview.top_flats.is_empty(),
        "the dressed walls and flats tally: {:?} {:?}",
        overview.top_wall_textures,
        overview.top_flats
    );
}

#[test]
fn the_plan_without_its_key_stays_locked_and_lists_the_locks_anyway() {
    let (mut map, rows) = built(&keyed_exit_plan(), MapFormat::Doom);
    stand(&mut map, spawn(1, 128, 128, 0));
    stand(&mut map, spawn(5, 640, 64, 0));
    let overview = Describe::new(&map, &doom2_defs()).summary();
    assert!(
        !overview.locked_doors.is_empty() && overview.locked_doors.iter().all(|lock| !lock.opened),
        "the key in the far room opens none of them: {:?}",
        overview.locked_doors
    );
    assert_eq!(
        overview.keys.first().map(|key| key.reachable),
        Some(false),
        "and that card stands behind the door"
    );
    assert!(
        overview.exits.iter().any(|exit| !exit.reachable),
        "the exit lines are there and out of reach: {:?}",
        overview.exits
    );
    assert!(
        !overview.playable.ok,
        "a shut door is no way to the exit: {:?}",
        overview.playable
    );
    assert!(
        overview
            .playable
            .summary
            .starts_with("No exit is reachable. "),
        "the tool's own note: {}",
        overview.playable.summary
    );
    let secret = *row(&rows, "secret")
        .sectors
        .first()
        .expect("the secret room");
    assert_eq!(
        overview.secret_sectors,
        vec![secret],
        "locked out or not, the secret reads the same"
    );
}

#[test]
fn the_empty_map_overviews_with_zeroes_and_the_no_start_note() {
    let map = Map::default();
    let overview = Describe::new(&map, &doom_defs()).summary();
    assert_eq!(overview.bounds, None, "no vertices, no box");
    assert_eq!(overview.counts.vertices, 0, "and no elements at all");
    assert_eq!(overview.counts.things, 0, "not even a start");
    assert!(overview.player_starts.is_empty(), "nobody spawns");
    assert!(overview.exits.is_empty(), "nothing ends");
    assert!(overview.teleporters.is_empty(), "nothing lands");
    assert!(overview.secret_sectors.is_empty(), "nothing hides");
    assert!(
        overview.monsters.is_empty() && overview.items.is_empty(),
        "nobody and nothing to tally"
    );
    assert!(
        overview.top_wall_textures.is_empty(),
        "no sidedefs, no textures"
    );
    assert!(overview.top_flats.is_empty(), "no sectors, no flats");
    assert!(overview.largest_sectors.is_empty(), "no sectors to rank");
    assert!(!overview.playable.ok, "nobody can start, let alone finish");
    assert_eq!(
        overview.playable.summary,
        "No player 1 start: nothing can be reached. 0 of 0 sectors reachable.",
        "the no-start note and the zero count"
    );
}
