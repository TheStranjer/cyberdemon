//! The inventory read's own checks: the presence rule, the argument and
//! colour helpers, and the empty and tagged starts a
//! [`Reader`](super::Reader) works from. The tables and the trigger read
//! they share with every other classification consumer are checked in
//! [`crate::wad::map::specials::tests`].

use super::Effect;
use super::Reader;
use super::argument;
use super::key_color;
use super::openable;
use super::spawns;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::name;
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::playability::KeyColor;
use crate::wad::map::specials::tables::RowEffect;

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
fn the_key_things_answer_for_their_colours() {
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
fn only_a_closing_door_has_no_openable_effect() {
    assert_eq!(
        openable(RowEffect::ManualDoor),
        Some(Effect::Door),
        "the wall opens itself"
    );
    assert_eq!(
        openable(RowEffect::TaggedDoor),
        Some(Effect::Door),
        "a tagged door opens too"
    );
    assert_eq!(openable(RowEffect::Lift), Some(Effect::Lift));
    assert_eq!(openable(RowEffect::Teleport), Some(Effect::Teleport));
    assert_eq!(
        openable(RowEffect::CloseDoor),
        None,
        "a shutter is worked into nothing: known, never a feature"
    );
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
