//! Action-table tests: what [`door_specials`] installs for each style in each
//! flag family, how those installs read in the shared specials
//! classification, the track roles the locked styles name, and how
//! [`next_free_tag`] counts the tags a map already carries.

use super::{SAMPLE_TAG, one_sided, sector, two_sided};
use crate::wad::classify::MapFormat;
use crate::wad::map::features::doors::{DoorLineSetup, DoorStyle, door_specials, next_free_tag};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::specials::is_door_action;

/// Checks one style's written special of `family` against the shared
/// classification: the remote style's zero is the only door without an
/// action row, and every written action must read as a door.
fn check_door_row(family: FlagFamily, style: DoorStyle) {
    let setup = door_specials(family, style, SAMPLE_TAG);
    if setup.special == 0_i32 {
        assert!(
            matches!(style, DoorStyle::Remote),
            "{family:?} only zeroes the action for the remote style, got {style:?}"
        );
        assert!(
            !is_door_action(setup.special, family),
            "special 0 is never a door action"
        );
        return;
    }
    assert!(
        is_door_action(setup.special, family),
        "the {family:?} row of {style:?} (special {}) must classify as a door action, or \
         auto_texture will repaint the door it just built",
        setup.special
    );
}

#[test]
fn every_written_door_action_is_a_door_in_the_shared_classification() {
    for family in [FlagFamily::Doom, FlagFamily::Hexen] {
        for style in DoorStyle::all() {
            check_door_row(family, style);
        }
    }
}

#[test]
fn door_specials_matches_the_doom_table() {
    let expected = [
        (DoorStyle::Normal, 1_i32),
        (DoorStyle::Fast, 117_i32),
        (DoorStyle::StayOpen, 31_i32),
        (DoorStyle::LockedRed, 28_i32),
        (DoorStyle::LockedBlue, 26_i32),
        (DoorStyle::LockedYellow, 27_i32),
        (DoorStyle::Remote, 0_i32),
    ];
    assert_eq!(
        expected.len(),
        DoorStyle::all().len(),
        "the table covers every style"
    );
    for (style, special) in expected {
        assert_eq!(
            door_specials(FlagFamily::Doom, style, SAMPLE_TAG),
            DoorLineSetup {
                special,
                tag: SAMPLE_TAG,
                args: [0_i32; 5],
                flags_activation: 0_u32,
            },
            "the Doom row of {style:?}"
        );
    }
}

#[test]
fn door_specials_matches_the_hexen_table() {
    let expected = [
        (
            DoorStyle::Normal,
            12_i32,
            [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
            0x2600_u32,
        ),
        (
            DoorStyle::Fast,
            12_i32,
            [0_i32, 64_i32, 150_i32, 0_i32, 0_i32],
            0x2600_u32,
        ),
        (
            DoorStyle::StayOpen,
            11_i32,
            [0_i32, 16_i32, 0_i32, 0_i32, 0_i32],
            0x400_u32,
        ),
        (
            DoorStyle::LockedRed,
            13_i32,
            [0_i32, 16_i32, 150_i32, 129_i32, 0_i32],
            0x600_u32,
        ),
        (
            DoorStyle::LockedBlue,
            13_i32,
            [0_i32, 16_i32, 150_i32, 130_i32, 0_i32],
            0x600_u32,
        ),
        (
            DoorStyle::LockedYellow,
            13_i32,
            [0_i32, 16_i32, 150_i32, 131_i32, 0_i32],
            0x600_u32,
        ),
        (DoorStyle::Remote, 0_i32, [0_i32; 5], 0_u32),
    ];
    for (style, special, args, activation) in expected {
        assert_eq!(
            door_specials(FlagFamily::Hexen, style, SAMPLE_TAG),
            DoorLineSetup {
                special,
                tag: 0_i32,
                args,
                flags_activation: activation,
            },
            "the Hexen row of {style:?}: use 0x400, repeat 0x200, monster 0x2000"
        );
    }
}
#[test]
fn locked_styles_name_their_coloured_track_role() {
    assert_eq!(DoorStyle::LockedRed.track_role_name(), "door_track_red");
    assert_eq!(DoorStyle::LockedBlue.track_role_name(), "door_track_blue");
    assert_eq!(
        DoorStyle::LockedYellow.track_role_name(),
        "door_track_yellow"
    );
    for style in [
        DoorStyle::Normal,
        DoorStyle::Fast,
        DoorStyle::StayOpen,
        DoorStyle::Remote,
    ] {
        assert_eq!(style.track_role_name(), "door_track", "{style:?}");
    }
}
#[test]
fn next_free_tag_starts_at_one_on_an_empty_map() {
    assert_eq!(
        next_free_tag(&Map::default()),
        1_i32,
        "nothing carries a tag yet"
    );
}

#[test]
fn next_free_tag_counts_sectors_and_doom_line_tags_of_special_lines() {
    let map = Map {
        sectors: vec![sector(0_i32, 128_i32, 3_i32), sector(0_i32, 128_i32, 9_i32)],
        linedefs: vec![
            Linedef {
                special: 1_i32,
                tag: 7_i32,
                ..two_sided(0_usize, 1_usize)
            },
            Linedef {
                tag: 100_i32,
                ..one_sided(0_usize)
            },
        ],
        ..Map::default()
    };
    assert_eq!(
        next_free_tag(&map),
        10_i32,
        "the special-carrying line tag 7 and the sector tags count, the \
         plain line's 100 does not"
    );
}

#[test]
fn next_free_tag_counts_hexen_arguments_and_saturates_at_the_limit() {
    let hexen = Map {
        format: MapFormat::Hexen,
        sectors: vec![sector(0_i32, 128_i32, 2_i32)],
        linedefs: vec![
            Linedef {
                special: 12_i32,
                args: [6_i32, 16_i32, 150_i32, 0_i32, 0_i32],
                ..two_sided(0_usize, 1_usize)
            },
            Linedef {
                args: [50_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                ..one_sided(0_usize)
            },
        ],
        ..Map::default()
    };
    assert_eq!(
        next_free_tag(&hexen),
        7_i32,
        "args[0] of the tagged special counts, the plain line's does not"
    );
    let full = Map {
        sectors: vec![sector(0_i32, 128_i32, i32::MAX)],
        ..Map::default()
    };
    assert_eq!(
        next_free_tag(&full),
        i32::MAX,
        "nothing is above the maximum"
    );
}
