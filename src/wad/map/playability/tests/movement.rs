//! The ways a player can and cannot take: openings, steps, headroom and
//! blocking bits, and the manual, locked, tagged and remote doors with the
//! keys that open them, read through a hand-built `hall`.

use alloc::vec;

use super::super::{BlockerReason, KeyColor, KeyInfo, LockInfo, analyze_playability, blockers};
use crate::wad::map::common::fixtures::spawn;
use crate::wad::map::flags::{
    BLOCK_MONSTERS, BLOCKING, FlagFamily, HEXEN_BLOCK_PLAYERS, PLAYER_USE_BITS, Skill, TWO_SIDED,
};

use super::{
    BLUE_CARD, HEXEN_PRESENCE, RED_SKULL, cell, hall, hexen_start, middle, notes_about,
    paired_red_door, sectors, slab, stand, wire,
};

#[test]
fn an_opening_between_two_rooms_reaches_both() {
    let map = hall(&[cell(0_i32), cell(0_i32)]);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(found.start_sector, Some(0), "the start stands in the hall");
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the gap between the rooms is a way through"
    );
    assert!(found.warnings.is_empty(), "nothing to complain about");
    assert!(found.doors_never_opened.is_empty(), "there are no doors");
    assert!(found.narrow_lines.is_empty(), "a 256-unit divider fits");
    assert!(found.exits.is_empty(), "and the map ends nowhere yet");
}

#[test]
fn a_step_above_the_players_legs_is_a_way_down_only() {
    let map = hall(&[cell(32_i32), cell(0_i32), cell(32_i32)]);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the player drops 32 units into the pit and cannot climb back out of it"
    );
}

#[test]
fn a_ledge_at_the_step_limit_is_still_walkable() {
    let map = hall(&[cell(0_i32), cell(24_i32)]);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(found.reachable, sectors(&[0, 1]), "24 units is the limit");
    let taller = hall(&[cell(0_i32), cell(25_i32)]);
    let stuck = analyze_playability(&taller, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        stuck.reachable,
        sectors(&[0]),
        "and one unit past it is not"
    );
}

#[test]
fn forty_eight_units_of_headroom_stop_the_player() {
    let map = hall(&[cell(0_i32), (0_i32, 48_i32)]);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "a 48-unit room is shallower than the player"
    );
}

#[test]
fn the_blocking_flag_ends_a_connection_whatever_the_heights() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCKING));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(found.reachable, sectors(&[0]), "a blocked line is no way");

    let mut hexen = map;
    hexen_start(&mut hexen);
    let blocked = analyze_playability(&hexen, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        blocked.reachable,
        sectors(&[0]),
        "the same bit blocks Hexen"
    );
}

#[test]
fn the_blockplayers_bit_is_a_hexen_wall_and_a_doom_nop() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | HEXEN_BLOCK_PLAYERS));
    let doom = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        doom.reachable,
        sectors(&[0, 1]),
        "bit 14 means nothing to the Doom tables"
    );
    let mut at_hexen = map.clone();
    hexen_start(&mut at_hexen);
    let hexen = analyze_playability(&at_hexen, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        hexen.reachable,
        sectors(&[0]),
        "and means blockplayers to the Hexen ones"
    );
}

#[test]
fn the_monster_flag_alone_still_lets_the_player_through() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCK_MONSTERS));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "flag 2 keeps monsters, not players"
    );
}

#[test]
fn a_manual_door_opens_the_sector_behind_it() {
    let slab = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    let shut = analyze_playability(&slab, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        shut.reachable,
        sectors(&[0]),
        "a slab of a sector is nobody's way through"
    );
    assert!(
        shut.doors_never_opened.is_empty(),
        "with no special on its lines there is no door to report either"
    );

    let mut map = slab;
    wire(&mut map, 0, (1_i32, 0_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "the use line in front of a player raises the ceiling over their head"
    );
    assert!(found.doors_never_opened.is_empty(), "the door opened");
    assert!(found.locks.is_empty(), "and it asked for no key");
}

#[test]
fn a_locked_door_holds_until_its_key_is_reached() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
    stand(&mut map, BLUE_CARD, middle(2), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "the blue card behind the blue door cannot be fetched"
    );
    assert_eq!(
        found.locks,
        vec![LockInfo {
            line: 0,
            color: KeyColor::Blue,
            opened: false,
        }],
        "the locked line is named and stays shut"
    );
    assert_eq!(found.doors_never_opened, vec![1_usize], "the band");
    assert_eq!(
        found.keys,
        vec![KeyInfo {
            thing: 1,
            color: KeyColor::Blue,
            sector: Some(2),
            reachable: false,
        }],
        "the key is where the player is not"
    );
}

#[test]
fn a_key_in_the_reachable_room_opens_its_door() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
    stand(&mut map, BLUE_CARD, middle(0), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "the card in hand walks the player through"
    );
    assert_eq!(
        found.locks,
        vec![LockInfo {
            line: 0,
            color: KeyColor::Blue,
            opened: true,
        }]
    );
    assert!(found.doors_never_opened.is_empty(), "the band opened");
    assert_eq!(
        found.keys.first().map(|entry| entry.reachable),
        Some(true),
        "and the player walked onto it"
    );
}

#[test]
fn an_unpressed_partner_line_of_a_raised_door_reads_as_opened() {
    let map = paired_red_door();
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the band stands open; the room above it is a step too high",
    );
    assert_eq!(
        found.locks,
        vec![
            LockInfo {
                line: 0,
                color: KeyColor::Red,
                opened: true,
            },
            LockInfo {
                line: 1,
                color: KeyColor::Red,
                opened: true,
            },
        ],
        "both room-facing lines of the raised band read as opened",
    );
    assert!(found.doors_never_opened.is_empty(), "the band opened");
    let held_out = blockers(&map, FlagFamily::Doom, &found);
    assert_eq!(
        held_out
            .first()
            .map(|blocker| (&blocker.reason, blocker.sector)),
        Some((&BlockerReason::StepTooHigh { delta: 32 }, 2_usize)),
        "the room is held by its step, not by the lock: {held_out:?}",
    );
}

#[test]
fn a_card_and_a_skull_answer_for_the_same_colour() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (28_i32, 0_i32, TWO_SIDED));
    stand(&mut map, RED_SKULL, middle(0), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert!(
        found.reachable.contains(&2),
        "the red skull opens the red door"
    );
    assert_eq!(found.keys.first().map(|key| key.color), Some(KeyColor::Red));
}

#[test]
fn a_skill_that_hides_the_key_hides_the_way_through() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
    stand(&mut map, BLUE_CARD, middle(0), 4);
    let medium = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        medium.reachable,
        sectors(&[0]),
        "a hard-skill key does not spawn on medium"
    );
    assert!(medium.keys.is_empty(), "and so it is no key at all");
    let hard = analyze_playability(&map, FlagFamily::Doom, Skill::Hard);
    assert!(
        hard.reachable.contains(&2),
        "on hard the card is right there"
    );
    assert_eq!(hard.keys.first().map(|key| key.color), Some(KeyColor::Blue));
}

#[test]
fn a_multiplayer_only_key_is_no_key() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
    stand(&mut map, BLUE_CARD, middle(0), 7 | 16);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert!(found.keys.is_empty(), "a notsingle thing never spawns");
    assert_eq!(found.reachable, sectors(&[0]), "and so the door stays shut");
}

#[test]
fn a_hexen_locked_door_wants_the_colour_of_its_lock_argument() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (13_i32, 0_i32, TWO_SIDED | PLAYER_USE_BITS));
    map.linedefs.get_mut(0).expect("the divider").args = [0_i32, 16_i32, 150_i32, 130_i32, 0_i32];
    hexen_start(&mut map);
    let shut = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(shut.reachable, sectors(&[0]), "no key, no way through");
    assert_eq!(
        shut.locks,
        vec![LockInfo {
            line: 0,
            color: KeyColor::Blue,
            opened: false,
        }],
        "arg3 130 is the blue any-key lock"
    );
    let mut with_key = map;
    stand(&mut with_key, BLUE_CARD, middle(0), HEXEN_PRESENCE);
    let open = analyze_playability(&with_key, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        open.reachable,
        sectors(&[0, 1, 2]),
        "arg0 0 leaves the door acting on the sector behind the line"
    );
    assert!(open.locks.iter().all(|lock| lock.opened), "and it opened");
}

#[test]
fn an_unknown_lock_code_is_read_as_openable_and_says_so() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (13_i32, 0_i32, TWO_SIDED | PLAYER_USE_BITS));
    map.linedefs.get_mut(0).expect("the divider").args = [0_i32, 16_i32, 150_i32, 77_i32, 0_i32];
    hexen_start(&mut map);
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    let notes = notes_about(&found, "unknown lock 77");
    assert_eq!(
        notes.len(),
        1,
        "the odd code is named once: {:?}",
        found.warnings
    );
    assert!(
        notes
            .first()
            .is_some_and(|note| note.contains("assumed openable")),
        "and called what it is: {notes:?}"
    );
    assert!(
        found.reachable.contains(&2),
        "the door is walked through for want of a better guess"
    );
    assert!(found.locks.is_empty(), "and it is no longer a colour lock");
}

#[test]
fn a_use_line_needs_its_front_side_and_a_walk_line_either() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the band").tag = 4_i32;
    map.things = vec![spawn(1, 640, 128, 0)];
    wire(&mut map, 1, (103_i32, 4_i32, TWO_SIDED));
    let use_line = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        use_line.reachable,
        sectors(&[2]),
        "an SR door faces the band it opens, and the player is behind it"
    );

    wire(&mut map, 1, (2_i32, 4_i32, TWO_SIDED));
    let walk_line = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        walk_line.reachable,
        sectors(&[0, 1, 2]),
        "a W1 door works from the side the player is standing on"
    );
}

#[test]
fn a_remote_door_needs_its_switch_on_the_reachable_side() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the band").tag = 7_i32;
    let mut with_switch = map.clone();
    wire(&mut with_switch, 2, (103_i32, 7_i32, BLOCKING));
    let worked = analyze_playability(&with_switch, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        worked.reachable,
        sectors(&[0, 1, 2]),
        "the switch on a hall wall raises the tagged band"
    );
    assert!(worked.doors_never_opened.is_empty(), "and it opened");

    let mut behind = map;
    wire(&mut behind, 6, (103_i32, 7_i32, BLOCKING));
    let stuck = analyze_playability(&behind, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        stuck.reachable,
        sectors(&[0]),
        "the same switch on the far side of the door is never pressed"
    );
    assert_eq!(
        stuck.doors_never_opened,
        vec![1_usize],
        "so the band is a door that never opened"
    );
}
