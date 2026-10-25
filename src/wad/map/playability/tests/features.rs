//! The features a walk works beyond the plain lines: lifts that ride the
//! player up, teleporters with and without their destinations, and the exit
//! lines a reached map can end on, `Doom` and `Hexen` alike.

use alloc::vec;

use super::super::{ExitInfo, analyze_playability};
use crate::wad::map::flags::{BLOCKING, FlagFamily, Skill, TWO_SIDED};

use super::{
    HEXEN_PRESENCE, TELEPORT_DEST, cell, hall, hexen_start, middle, notes_about, sectors, stand,
    wire,
};

#[test]
fn a_lift_carries_the_player_up_to_a_ledge_they_cannot_climb() {
    let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the platform").tag = 3_i32;
    wire(&mut map, 0, (21_i32, 3_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "the platform rides them up to the ledge"
    );

    let mut plain = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    plain.sectors.get_mut(1).expect("the platform").tag = 3_i32;
    let stuck = analyze_playability(&plain, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        stuck.reachable,
        sectors(&[0]),
        "without the lift the ledge is 96 units out of reach"
    );
}

#[test]
fn a_walk_on_teleporter_reaches_the_room_it_points_at() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
    wire(&mut map, 0, (97_i32, 5_i32, TWO_SIDED | BLOCKING));
    stand(&mut map, TELEPORT_DEST, middle(1), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the pad carries the player across a line they cannot walk"
    );
}

#[test]
fn a_teleporter_without_its_destination_says_so() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
    wire(&mut map, 0, (97_i32, 5_i32, TWO_SIDED | BLOCKING));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(found.reachable, sectors(&[0]), "there is nowhere to land");
    let notes = notes_about(&found, "no teleport destination");
    assert_eq!(notes.len(), 1, "and the missing thing is named");
}

#[test]
fn a_hexen_teleport_follows_its_destination_tid() {
    let mut map = hall(&[cell(0_i32), cell(96_i32)]);
    wire(&mut map, 0, (70_i32, 0_i32, TWO_SIDED | BLOCKING));
    map.linedefs.get_mut(0).expect("the divider").args = [7_i32, 0_i32, 0_i32, 0_i32, 0_i32];
    hexen_start(&mut map);
    stand(&mut map, TELEPORT_DEST, middle(1), HEXEN_PRESENCE);
    if let Some(thing) = map.things.last_mut() {
        thing.tid = 7_i32;
    }
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the 96-unit step is taken by teleporting over it"
    );

    let mut other_tid = map.clone();
    if let Some(thing) = other_tid.things.last_mut() {
        thing.tid = 9_i32;
    }
    let wrong = analyze_playability(&other_tid, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        wrong.reachable,
        sectors(&[0]),
        "and a thing with another tid is not the destination"
    );
    assert_eq!(
        notes_about(&wrong, "no teleport destination").len(),
        1,
        "which the report says"
    );
}

#[test]
fn an_exit_line_reads_as_reachable_with_its_kind() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (52_i32, 0_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.exits,
        vec![ExitInfo {
            line: 0,
            special: 52,
            secret: false,
            reachable: true,
        }],
        "a walk-over exit on a reached line"
    );
}

#[test]
fn an_exit_behind_an_impassable_line_is_unreachable() {
    let mut map = hall(&[cell(0_i32), cell(128_i32), cell(0_i32)]);
    wire(&mut map, 0, (11_i32, 0_i32, TWO_SIDED));
    wire(&mut map, 1, (51_i32, 0_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.exits,
        vec![
            ExitInfo {
                line: 0,
                special: 11,
                secret: false,
                reachable: true,
            },
            ExitInfo {
                line: 1,
                special: 51,
                secret: true,
                reachable: false,
            },
        ],
        "the use side of the second exit is the room nobody reaches"
    );
}

#[test]
fn the_hexen_portals_are_the_exits_their_numbers_say() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (243_i32, 0_i32, TWO_SIDED));
    hexen_start(&mut map);
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        found.exits.first(),
        Some(&ExitInfo {
            line: 0,
            special: 243,
            secret: false,
            reachable: true,
        }),
        "243 ends the level"
    );
    wire(&mut map, 0, (244_i32, 0_i32, TWO_SIDED));
    let secret = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        secret.exits.first().map(|exit| exit.secret),
        Some(true),
        "244 is the secret one"
    );
    wire(&mut map, 0, (74_i32, 0_i32, TWO_SIDED));
    let newmap = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        newmap.exits.first().map(|exit| exit.special),
        Some(74_i32),
        "and Teleport_NewMap ends it too"
    );
}
