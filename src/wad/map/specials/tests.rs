//! The classification's own checks, for both flag families: the tables'
//! door letters, colours and argument slots, the family-aware
//! [`special_class`] and [`is_door_action`] answers, and the exit rule of
//! [`exit_for`]. The playability consumer's read is tested in
//! [`crate::wad::map::playability::inventory::tests`].

use super::SpecialClass;
use super::doom_row;
use super::exit_for;
use super::hexen_trigger;
use super::is_door_action;
use super::lock_color;
use super::special_class;
use super::tables::DOOM_EXITS;
use super::tables::DOOM_ROWS;
use super::tables::DoomRow;
use super::tables::ExitRow;
use super::tables::HEXEN_EXITS;
use super::tables::HEXEN_ROWS;
use super::tables::RowEffect;
use super::tables::Trigger;
use crate::wad::map::flags::{FlagFamily, PLAYER_USE_BITS};
use crate::wad::map::model::Linedef;
use crate::wad::map::playability::KeyColor;

/// The `Doom` row for `special`, for the table spot checks.
fn row(special: i32) -> Option<&'static DoomRow> {
    DOOM_ROWS.iter().find(|entry| entry.special == special)
}

/// A whole `Doom` row, for the [`doom_row`] equality checks.
const fn row_of(
    special: i32,
    effect: RowEffect,
    trigger: Trigger,
    lock: Option<KeyColor>,
) -> DoomRow {
    DoomRow {
        special,
        effect,
        trigger,
        lock,
    }
}

/// A line carrying `special` with the flag bits `flags`.
fn line(special: i32, flags: u32) -> Linedef {
    Linedef {
        flags,
        special,
        ..Default::default()
    }
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
        Trigger::Shoot,
        "a shot works the face it sees, never the side behind"
    );
    assert_eq!(row(62).expect("SR lift").trigger, Trigger::Use);
    assert_eq!(row(88).expect("WR lift").trigger, Trigger::Walk);
    assert_eq!(
        row(87).expect("WR perpetual plat").effect,
        RowEffect::Lift,
        "the perpetual plats drop and stay down"
    );
    assert_eq!(row(87).expect("WR perpetual plat").trigger, Trigger::Walk);
    assert_eq!(row(89).expect("SR perpetual plat").trigger, Trigger::Use);
    assert_eq!(
        row(10).expect("W1 lift").effect,
        RowEffect::Lift,
        "the same lift family"
    );
    assert_eq!(row(97).expect("WR teleport").effect, RowEffect::Teleport);
    assert_eq!(row(1).and_then(|entry| entry.lock), None, "no key");
}

#[test]
fn the_doom_table_names_its_closing_doors() {
    for special in [3_i32, 16_i32, 42_i32, 50_i32, 75_i32, 76_i32] {
        let close = row(special).expect("a Doom close special");
        assert_eq!(
            close.effect,
            RowEffect::CloseDoor,
            "special {special} shuts doors"
        );
    }
    for special in [107_i32, 110_i32, 113_i32, 116_i32] {
        assert_eq!(
            row(special).expect("a DOOM II fast close special").effect,
            RowEffect::CloseDoor,
            "special {special} shuts doors fast"
        );
    }
    assert!(
        is_door_action(50, FlagFamily::Doom),
        "a shutter is still a door action for the texture pass"
    );
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
    for (special, secret) in HEXEN_EXITS.iter().copied() {
        assert_eq!(
            special_class(FlagFamily::Hexen, special),
            Some(SpecialClass::Exit {
                secret,
                trigger: None,
            }),
            "the Hexen portal {special} classifies as its own exit row"
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
    assert_eq!(
        HEXEN_ROWS
            .iter()
            .find(|entry| entry.special == 10_i32)
            .expect("Door_Close")
            .effect,
        RowEffect::CloseDoor,
        "the closing door is a named shutter row, never counted unhandled"
    );
}

#[test]
fn the_hexen_lock_codes_answer_to_their_colours() {
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
}

#[test]
fn the_doom_family_classes_its_own_numbers() {
    assert_eq!(
        special_class(FlagFamily::Doom, 1),
        Some(SpecialClass::Door),
        "DR Door"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 29),
        Some(SpecialClass::Door),
        "S1 Door"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 21),
        Some(SpecialClass::Lift),
        "S1 lift"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 97),
        Some(SpecialClass::Teleport),
        "WR teleport"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 52),
        Some(SpecialClass::Exit {
            secret: false,
            trigger: Some(Trigger::Walk),
        }),
        "the walk-over end"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 197),
        Some(SpecialClass::Exit {
            secret: false,
            trigger: Some(Trigger::Shoot),
        }),
        "the G1 exit is shot at from the front"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 198),
        Some(SpecialClass::Exit {
            secret: true,
            trigger: Some(Trigger::Shoot),
        }),
        "and so is its secret twin"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 12),
        None,
        "12 is nobody's action in the Doom table"
    );
    assert_eq!(special_class(FlagFamily::Doom, 0), None, "no special");
}

#[test]
fn the_hexen_family_classes_its_own_numbers() {
    assert_eq!(
        special_class(FlagFamily::Hexen, 12),
        Some(SpecialClass::Door),
        "Door_Raise"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 11),
        Some(SpecialClass::Door),
        "Door_Open"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 13),
        Some(SpecialClass::Door),
        "Door_LockedRaise"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 10),
        Some(SpecialClass::Door),
        "Door_Close is a door action for the texture pass"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 62),
        Some(SpecialClass::Lift),
        "Plat_DownWaitUpStay"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 71),
        Some(SpecialClass::Teleport),
        "Teleport_NoFog"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 74),
        Some(SpecialClass::Exit {
            secret: false,
            trigger: None,
        }),
        "Teleport_NewMap ends the level by whatever activation the line carries"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 1),
        None,
        "Polyobj_StartLine is nobody's door"
    );
    assert_eq!(
        special_class(FlagFamily::Hexen, 2),
        None,
        "and so is rotation"
    );
}

#[test]
fn the_families_do_not_borrow_each_other_s_actions() {
    assert!(
        !is_door_action(1, FlagFamily::Hexen),
        "the Hexen Polyobj_StartLine is not a Doom DR door"
    );
    assert!(
        !is_door_action(12, FlagFamily::Doom),
        "the Doom speed-12 row (light flash) is not a Hexen Door_Raise"
    );
    assert!(
        !is_door_action(21, FlagFamily::Hexen),
        "a Doom lift call is nobody's action in the Hexen table"
    );
    assert!(
        !is_door_action(60, FlagFamily::Doom),
        "a Hexen plat is only a Doom sky-transfer ceiling"
    );
}

#[test]
fn door_actions_keep_generalized_doors_in_the_doom_family_only() {
    assert!(
        is_door_action(1, FlagFamily::Doom),
        "a plain table row is a door action"
    );
    assert!(
        is_door_action(0x3C00_i32 + 16, FlagFamily::Doom),
        "a generalized door bit pattern"
    );
    assert!(
        !is_door_action(18, FlagFamily::Doom),
        "a pillar build is not a door"
    );
    assert!(
        !is_door_action(0, FlagFamily::Doom),
        "no special, no action"
    );
    assert!(
        !is_door_action(-1, FlagFamily::Doom),
        "a negative number is no action either"
    );
    assert!(
        !is_door_action(0x3C00 + 16, FlagFamily::Hexen),
        "the generalized layout belongs to the Doom family"
    );
    assert!(
        !is_door_action(0x2F_81, FlagFamily::Doom),
        "a generalized floor move is not a door"
    );
}

#[test]
fn each_hexen_activation_field_works_the_line_its_own_way() {
    assert_eq!(
        hexen_trigger(0_u32),
        Trigger::Walk,
        "field 0 is playercross"
    );
    assert_eq!(
        hexen_trigger(0x400_u32),
        Trigger::Use,
        "field 1 is playeruse"
    );
    assert_eq!(
        hexen_trigger(0x800_u32),
        Trigger::Monster,
        "field 2 is monstercross: no player sets it off"
    );
    assert_eq!(
        hexen_trigger(0xC00_u32),
        Trigger::Shoot,
        "field 3 is impact: a shot works the front face"
    );
    assert_eq!(
        hexen_trigger(0x1000_u32),
        Trigger::Shoot,
        "field 4 is playerpush: a shove works the front face"
    );
    assert_eq!(
        hexen_trigger(0x1_400),
        Trigger::Shoot,
        "field 5 is missilecross: worked by a missile the player can send over"
    );
    assert_eq!(
        hexen_trigger(0x2600_u32),
        Trigger::Use,
        "use plus repeatspecial and monsteractivate is still use"
    );
    assert_eq!(
        hexen_trigger(0x2800_u32),
        Trigger::Monster,
        "monsteractivate beside monstercross lets monsters work it too, not the player"
    );
}

#[test]
fn doom_row_passes_the_table_and_decodes_the_generalized_bits() {
    assert_eq!(
        doom_row(2),
        Some(row_of(2, RowEffect::TaggedDoor, Trigger::Walk, None)),
        "a classic row comes out of the table"
    );
    assert_eq!(doom_row(0), None, "no special, no row");
    assert_eq!(doom_row(-1), None, "and a negative number is no pattern");
    assert_eq!(
        doom_row(0x3C01),
        Some(row_of(0x3C01, RowEffect::TaggedDoor, Trigger::Walk, None)),
        "WR generalized door, open-wait-close"
    );
    assert_eq!(
        doom_row(0x3C27),
        Some(row_of(0x3C27, RowEffect::TaggedDoor, Trigger::Use, None)),
        "DR generalized door, open and stay: the manual use door"
    );
    assert_eq!(
        doom_row(0x3C61),
        Some(row_of(0x3C61, RowEffect::CloseDoor, Trigger::Walk, None)),
        "WR generalized door, close and stay: a known shutter, never a way"
    );
    assert_eq!(
        doom_row(0x3401),
        Some(row_of(0x3401, RowEffect::Lift, Trigger::Walk, None)),
        "WR generalized lift"
    );
    assert_eq!(
        doom_row(0x3404),
        Some(row_of(0x3404, RowEffect::Lift, Trigger::Shoot, None)),
        "G1 generalized lift: shot at from the front"
    );
}

#[test]
fn doom_row_threads_generalized_locks_through_the_table_read() {
    assert_eq!(
        doom_row(0x38A3),
        Some(row_of(
            0x38A3,
            RowEffect::TaggedDoor,
            Trigger::Use,
            Some(KeyColor::Blue)
        )),
        "SR generalized door locked to the blue card"
    );
    assert_eq!(
        doom_row(0x3900),
        Some(row_of(
            0x3900,
            RowEffect::TaggedDoor,
            Trigger::Walk,
            Some(KeyColor::Red)
        )),
        "W1 generalized door locked to the red skull"
    );
    assert_eq!(
        special_class(FlagFamily::Doom, 0x3C01),
        None,
        "special_class stays the pure table read even where doom_row decodes"
    );
    assert!(
        is_door_action(0x3C01, FlagFamily::Doom),
        "and the texture pass still sees the generalized door"
    );
}

#[test]
fn doom_row_leaves_unmodelled_generalized_kinds_and_locks_unhandled() {
    assert_eq!(
        doom_row(0x2F_80),
        None,
        "a generalized crusher is nobody's door"
    );
    assert_eq!(
        doom_row(0x60_01),
        None,
        "a generalized floor move is not modelled"
    );
    assert_eq!(doom_row(0x40_01), None, "nor a generalized ceiling");
    assert_eq!(doom_row(0x30_01), None, "nor a generalized stair builder");
    assert_eq!(
        doom_row(0x3801),
        None,
        "a locked door keyed \"any\" names no single colour"
    );
    assert_eq!(
        doom_row(0x39C1),
        None,
        "nor does one that wants all six keys"
    );
}

#[test]
fn exits_read_their_trigger_where_the_family_puts_it() {
    assert_eq!(
        exit_for(FlagFamily::Doom, &line(52, 0_u32)),
        Some(ExitRow {
            special: 52,
            secret: false,
            trigger: Trigger::Walk,
        }),
        "the Doom row carries its fixed trigger"
    );
    assert_eq!(
        exit_for(FlagFamily::Doom, &line(26, 0_u32)),
        None,
        "a Doom door line is no exit"
    );
    let use_portal =
        exit_for(FlagFamily::Hexen, &line(243, PLAYER_USE_BITS)).expect("Portal is a Hexen exit");
    assert_eq!(use_portal.trigger, Trigger::Use, "playeruse pins the side");
    let cross_portal =
        exit_for(FlagFamily::Hexen, &line(243, 0_u32)).expect("Portal is a Hexen exit");
    assert_eq!(cross_portal.trigger, Trigger::Walk, "crossing reaches it");
    let monster_portal =
        exit_for(FlagFamily::Hexen, &line(243, 0x800_u32)).expect("Portal is a Hexen exit");
    assert_eq!(
        monster_portal.trigger,
        Trigger::Monster,
        "a monstercross portal ends nothing for a player"
    );
    let impact_portal =
        exit_for(FlagFamily::Hexen, &line(243, 0xC00_u32)).expect("Portal is a Hexen exit");
    assert_eq!(
        impact_portal.trigger,
        Trigger::Shoot,
        "an impact portal is shot at from the front"
    );
    assert_eq!(
        exit_for(FlagFamily::Doom, &line(197, 0_u32)).map(|exit| exit.trigger),
        Some(Trigger::Shoot),
        "the Doom family's G1 exit carries the gun trigger"
    );
    assert_eq!(
        exit_for(FlagFamily::Hexen, &line(244, 0_u32)).map(|exit| exit.secret),
        Some(true),
        "Instant teleporter 244 is the secret end"
    );
    assert_eq!(
        exit_for(FlagFamily::Hexen, &line(12, 0_u32)),
        None,
        "Door_Raise does not end the level"
    );
}
