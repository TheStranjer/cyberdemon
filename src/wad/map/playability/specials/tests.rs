//! The special tables' own checks: the rows' door letters, colours and
//! argument slots, the presence rule, and the empty and tagged reads a
//! [`Reader`](super::Reader) starts from.

use super::Reader;
use super::argument;
use super::hexen_trigger;
use super::key_color;
use super::lock_color;
use super::spawns;
use super::tables::DOOM_EXITS;
use super::tables::DOOM_ROWS;
use super::tables::DoomRow;
use super::tables::HEXEN_IGNORED;
use super::tables::HEXEN_ROWS;
use super::tables::RowEffect;
use super::tables::Trigger;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::name;
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::playability::KeyColor;

/// The `Doom` row for `special`, for the table spot checks.
fn row(special: i32) -> Option<&'static DoomRow> {
    DOOM_ROWS.iter().find(|entry| entry.special == special)
}

#[test]
fn the_doom_table_spells_out_its_door_letters() {
    let manual = row(1).expect("DR open door");
    assert_eq!(
        manual.effect,
        RowEffect::ManualDoor,
        "the wall opens itself"
    );
    assert_eq!(manual.trigger, Trigger::Use, "and wants a press");
    let walk = row(2).expect("W1 open door");
    assert_eq!(walk.effect, RowEffect::TaggedDoor, "a tagged door");
    assert_eq!(walk.trigger, Trigger::Walk, "worked from either side");
    assert_eq!(
        row(46).expect("G1 open door").trigger,
        Trigger::Walk,
        "a shot reaches either side too"
    );
    assert_eq!(row(62).expect("SR lift").trigger, Trigger::Use);
    assert_eq!(row(88).expect("WR lift").trigger, Trigger::Walk);
    assert_eq!(
        row(10).expect("W1 lift").effect,
        RowEffect::Lift,
        "the same lift family"
    );
    assert_eq!(row(97).expect("WR teleport").effect, RowEffect::Teleport);
    assert_eq!(row(1).and_then(|entry| entry.lock), None, "no key");
}

#[test]
fn the_doom_locks_answer_to_their_colours() {
    for special in [26_i32, 32_i32, 99_i32, 133_i32] {
        assert_eq!(
            row(special).and_then(|entry| entry.lock),
            Some(KeyColor::Blue),
            "special {special} is blue"
        );
    }
    for special in [28_i32, 33_i32, 134_i32, 135_i32] {
        assert_eq!(
            row(special).and_then(|entry| entry.lock),
            Some(KeyColor::Red),
            "special {special} is red"
        );
    }
    for special in [27_i32, 34_i32, 136_i32, 137_i32] {
        assert_eq!(
            row(special).and_then(|entry| entry.lock),
            Some(KeyColor::Yellow),
            "special {special} is yellow"
        );
    }
}

#[test]
fn the_exit_tables_name_the_secret_ends() {
    for special in [51_i32, 124_i32, 198_i32] {
        assert!(
            DOOM_EXITS
                .iter()
                .any(|exit| exit.special == special && exit.secret),
            "{special} is the secret exit"
        );
    }
    for special in [11_i32, 52_i32, 197_i32] {
        assert!(
            DOOM_EXITS
                .iter()
                .any(|exit| exit.special == special && !exit.secret),
            "{special} ends the level"
        );
    }
}

#[test]
fn the_hexen_table_puts_its_arguments_in_the_right_slots() {
    let locked = HEXEN_ROWS
        .iter()
        .find(|entry| entry.special == 13_i32)
        .expect("Door_LockedRaise");
    assert_eq!(locked.tag_arg, 0_usize, "arg0 is the tag");
    assert_eq!(locked.lock_arg, Some(3_usize), "arg3 is the lock");
    assert_eq!(
        HEXEN_ROWS
            .iter()
            .find(|entry| entry.special == 202_i32)
            .expect("Generic_Door")
            .lock_arg,
        Some(4_usize),
        "arg4 is its lock"
    );
    assert!(
        !HEXEN_ROWS.iter().any(|entry| entry.special == 10_i32),
        "Door_Close is not an action row at all"
    );
    assert!(
        HEXEN_IGNORED.contains(&10_i32),
        "and it is named as known-but-unmodelled, so it is never counted unhandled"
    );
}

#[test]
fn arguments_read_by_slot_and_default_to_no_tag() {
    let line = Linedef {
        special: 202,
        args: [9_i32, 1_i32, 16_i32, 150_i32, 130_i32],
        ..Default::default()
    };
    assert_eq!(argument(&line, 0), 9_i32, "the tag slot reads");
    assert_eq!(argument(&line, 4), 130_i32, "and the lock slot");
    assert_eq!(argument(&line, 9), 0_i32, "past the end is no tag");
    assert_eq!(
        argument(&Linedef::default(), 0),
        0_i32,
        "and an empty line has none"
    );
}

#[test]
fn only_playeruse_pins_a_hexen_line_to_its_front_side() {
    assert_eq!(hexen_trigger(0x400_u32), Trigger::Use, "field 1 is use");
    assert_eq!(hexen_trigger(0_u32), Trigger::Walk, "field 0 is cross");
    assert_eq!(
        hexen_trigger(0x1_400),
        Trigger::Walk,
        "field 5 is missilecross"
    );
    assert_eq!(
        hexen_trigger(0x600_u32),
        Trigger::Use,
        "use plus repeatspecial and monsteractivate"
    );
}

#[test]
fn the_lock_and_key_tables_answer_for_each_other() {
    for code in [1_i32, 4_i32, 129_i32] {
        assert_eq!(lock_color(code), Some(KeyColor::Red), "code {code}");
    }
    for code in [2_i32, 5_i32, 130_i32] {
        assert_eq!(lock_color(code), Some(KeyColor::Blue), "code {code}");
    }
    for code in [3_i32, 6_i32, 131_i32] {
        assert_eq!(lock_color(code), Some(KeyColor::Yellow), "code {code}");
    }
    assert_eq!(lock_color(7_i32), None, "the extended list is not modelled");
    assert_eq!(lock_color(0_i32), None, "and zero is no lock at all");
    for kind in [5_i32, 40_i32] {
        assert_eq!(key_color(kind), Some(KeyColor::Blue), "type {kind}");
    }
    assert_eq!(key_color(13_i32), Some(KeyColor::Red));
    assert_eq!(key_color(38_i32), Some(KeyColor::Red));
    assert_eq!(key_color(6_i32), Some(KeyColor::Yellow));
    assert_eq!(key_color(39_i32), Some(KeyColor::Yellow));
    assert_eq!(key_color(2_005_i32), None, "a medikit is no key");
}

#[test]
fn presence_reads_the_skill_bit_and_the_game_mode() {
    assert!(
        spawns(4_u32, FlagFamily::Doom, Skill::Hard),
        "the named skill"
    );
    assert!(
        !spawns(2_u32, FlagFamily::Doom, Skill::Hard),
        "a medium-only thing does not spawn on hard"
    );
    assert!(
        !spawns(8_u32, FlagFamily::Doom, Skill::Medium),
        "an ambush-only thing carries no skill bit and so never spawns"
    );
    assert!(
        !spawns(4_u32 | 16_u32, FlagFamily::Doom, Skill::Hard),
        "a notsingle thing is out of the single-player game"
    );
    assert!(
        spawns(4_u32 | 0x100_u32, FlagFamily::Hexen, Skill::Hard),
        "a Hexen thing needs its mode bit"
    );
    assert!(
        !spawns(4_u32, FlagFamily::Hexen, Skill::Hard),
        "and no mode bit means no spawn"
    );
}

#[test]
fn reading_an_empty_map_invents_nothing() {
    let map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    let reader = Reader::new(&map, FlagFamily::Doom, Skill::Medium);
    assert!(!reader.spawns(8_u32), "no skill bit is never present");
    assert!(reader.tagged.is_empty(), "an empty map tags nothing");
}

#[test]
fn a_tagged_action_finds_the_sectors_answering_its_tag() {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.sectors = vec![
        Sector {
            tag: 7_i32,
            ..Default::default()
        },
        Sector {
            tag: 7_i32,
            ..Default::default()
        },
        Sector::default(),
    ];
    let reader = Reader::new(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        reader.tagged.get(&7_i32),
        Some(&[0_usize, 1].into_iter().collect()),
        "both tagged sectors answer"
    );
    assert!(
        reader.tagged.contains_key(&0_i32),
        "and the untagged ones group under 0"
    );
}
