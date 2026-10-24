//! `apply_door` and `face_room_line` tests: closing the recess, dressing the
//! room lines and the jambs, the `Hexen` activation rewrite and every refusal
//! the plan pass raises before a row is written.

use super::{SAMPLE_TAG, doom_setup, doorway, refuses, sides, textures};
use crate::wad::classify::MapFormat;
use crate::wad::map::features::doors::{
    DoorSides, DoorStyle, apply_door, door_specials, face_room_line,
};
use crate::wad::map::flags::FlagFamily;

#[test]
fn apply_door_closes_the_recess_and_dresses_the_room_lines() {
    let mut map = doorway(MapFormat::Doom);
    apply_door(&mut map, &sides(), &doom_setup(), &textures()).expect("a sound doorway");
    let door = map.sectors.get(1).expect("door sector");
    assert_eq!(door.ceiling_height, door.floor_height, "the door is closed");
    assert_eq!(
        door.floor_flat, "FLOOR6_8",
        "the recess floor is re-dressed"
    );
    assert_eq!(door.ceiling_flat, "CEIL1", "the ceiling flat stays");
    let untouched = map.sectors.first().expect("room sector");
    assert_eq!(untouched.ceiling_height, 128_i32, "the room is not closed");
    for index in [0_usize, 1] {
        let line = map.linedefs.get(index).expect("room line");
        assert_eq!(line.special, 1_i32, "the DR action of linedef {index}");
        assert_eq!(line.tag, SAMPLE_TAG, "the shared tag of linedef {index}");
        assert_eq!(line.args, [0_i32; 5], "no arguments of linedef {index}");
        assert_eq!(line.flags, 4_u32, "two-sided, unblocked and top-pegged");
    }
    assert_eq!(
        map.linedefs.first().expect("line 0").front,
        Some(0_usize),
        "the correct front stays the front"
    );
    assert_eq!(
        (
            map.linedefs.first().expect("line 0").v1,
            map.linedefs.first().expect("line 0").v2
        ),
        (0_usize, 1_usize),
        "an unflipped line keeps its vertices"
    );
    assert_eq!(
        map.linedefs.get(1).expect("line 1").front,
        Some(3_usize),
        "the backwards line is flipped to face the room"
    );
    assert_eq!(
        (
            map.linedefs.get(1).expect("line 1").v1,
            map.linedefs.get(1).expect("line 1").v2
        ),
        (1_usize, 0_usize),
        "the flip turns the line around too, so the room sidedef keeps the Doom \
         convention of sitting right of v1 to v2"
    );
    assert_eq!(map.sidedefs.first().expect("side").upper, "DOOR7");
    assert_eq!(map.sidedefs.get(3).expect("side").upper, "DOOR7");
    assert_eq!(
        map.sidedefs.get(1).expect("side").upper,
        "-",
        "the door-side upper clears"
    );
    assert_eq!(map.sidedefs.get(2).expect("side").upper, "-");
    assert_eq!(
        map.sidedefs.get(7).expect("side").upper,
        "-",
        "the plain wall is untouched"
    );
    assert_eq!(map.linedefs.get(4).expect("plain wall").special, 0_i32);
}

#[test]
fn apply_door_turns_the_jambs_into_one_sided_tracks() {
    let mut map = doorway(MapFormat::Doom);
    apply_door(&mut map, &sides(), &doom_setup(), &textures()).expect("a sound doorway");
    let swinging = map.linedefs.get(2).expect("jamb 2");
    assert_eq!(
        swinging.front,
        Some(5_usize),
        "the door-facing side becomes the front"
    );
    assert_eq!(swinging.back, None, "the room side is dropped");
    assert_eq!(
        (swinging.v1, swinging.v2),
        (1_usize, 0_usize),
        "the door side was the back, so the jamb turns around with it"
    );
    assert_eq!(
        swinging.flags,
        1_u32 | 16_u32,
        "one-sided blocking and lower-unpegged"
    );
    assert_eq!(map.sidedefs.get(5).expect("side").middle, "DOORTRAK");
    let standing = map.linedefs.get(3).expect("jamb 3");
    assert_eq!(
        standing.front,
        Some(6_usize),
        "an already correct jamb keeps its front"
    );
    assert_eq!(standing.back, None);
    assert_eq!(standing.flags, 1_u32 | 16_u32);
    assert_eq!(map.sidedefs.get(6).expect("side").middle, "DOORTRAK");
}

#[test]
fn apply_door_writes_the_hexen_activation_over_the_old_field() {
    let mut map = doorway(MapFormat::Hexen);
    map.linedefs.first_mut().expect("room line").flags = 4_u32 | 0x1000_u32;
    let setup = door_specials(FlagFamily::Hexen, DoorStyle::Normal, SAMPLE_TAG);
    apply_door(&mut map, &sides(), &setup, &textures()).expect("a sound doorway");
    let line = map.linedefs.first().expect("room line");
    assert_eq!(line.special, 12_i32, "Door_Raise");
    assert_eq!(
        line.args,
        [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
        "speed and delay"
    );
    assert_eq!(line.tag, 0_i32, "a Hexen line carries no tag");
    assert_eq!(
        line.flags,
        4_u32 | 0x2600_u32,
        "the playerpush field is replaced by use, repeat and monsteractivate"
    );
}

#[test]
fn face_room_line_flips_a_door_facing_line_and_writes_the_action() {
    let mut map = doorway(MapFormat::Doom);
    face_room_line(&mut map, 1_usize, 1_usize, &doom_setup());
    let line = map.linedefs.get(1).expect("room line 1");
    assert_eq!(line.front, Some(3_usize), "the room side becomes the front");
    assert_eq!(
        (line.v1, line.v2),
        (1_usize, 0_usize),
        "the vertices swap with the sides"
    );
    assert_eq!(line.special, 1_i32, "the DR action");
    assert_eq!(line.tag, SAMPLE_TAG, "the shared tag");
    assert_eq!(line.args, [0_i32; 5], "no arguments in the Doom family");
    assert_eq!(line.flags, 4_u32, "two-sided, unblocked and top-pegged");
    assert_eq!(
        map.sidedefs.get(2).expect("door side").upper,
        "-",
        "the untouched sides keep their faces: face_room_line dresses actions only"
    );
    assert_eq!(map.linedefs.first().expect("line 0").special, 0_i32);
}

#[test]
fn face_room_line_keeps_a_faced_line_and_replaces_hexen_activation() {
    let mut map = doorway(MapFormat::Hexen);
    map.linedefs.first_mut().expect("room line").flags = 4_u32 | 0x1000_u32;
    let setup = door_specials(FlagFamily::Hexen, DoorStyle::LockedBlue, SAMPLE_TAG);
    face_room_line(&mut map, 0_usize, 1_usize, &setup);
    let line = map.linedefs.first().expect("room line 0");
    assert_eq!(line.front, Some(0_usize), "a correct front stays the front");
    assert_eq!(
        (line.v1, line.v2),
        (0_usize, 1_usize),
        "and its vertices do"
    );
    assert_eq!(line.special, 13_i32, "Door_LockedRaise");
    assert_eq!(line.tag, 0_i32, "a Hexen line carries no tag");
    assert_eq!(
        line.flags,
        4_u32 | 0x600_u32,
        "the playerpush field is replaced by use and repeat"
    );
}

#[test]
fn face_room_line_leaves_lines_the_map_does_not_hold_alone() {
    let mut map = doorway(MapFormat::Doom);
    let before = map.clone();
    face_room_line(&mut map, 99_usize, 1_usize, &doom_setup());
    assert_eq!(map, before, "a missing row is written nowhere");
}
#[test]
fn apply_door_refuses_indices_the_map_does_not_hold() {
    let outside = |door_sides: DoorSides| {
        let mut map = doorway(MapFormat::Doom);
        let before = map.clone();
        let error = apply_door(&mut map, &door_sides, &doom_setup(), &textures())
            .expect_err("the index is out of range");
        assert_eq!(map, before, "a refused call leaves the map alone");
        error
    };
    let mut missing_sector = sides();
    missing_sector.door_sector = 9_usize;
    assert!(refuses(outside(missing_sector), "sector 9"), "the sector");
    let mut missing_line = sides();
    missing_line.room_lines = vec![0_usize, 77_usize];
    assert!(
        refuses(outside(missing_line), "room linedef 77"),
        "the room line"
    );
    let mut missing_jamb = sides();
    missing_jamb.jamb_lines = vec![88_usize];
    assert!(
        refuses(outside(missing_jamb), "jamb linedef 88"),
        "the jamb line"
    );
}

#[test]
fn apply_door_refuses_lines_that_are_not_doorways() {
    let mut map = doorway(MapFormat::Doom);
    let before = map.clone();
    let mut one_sided_room = sides();
    one_sided_room.room_lines = vec![4_usize];
    let first_error = apply_door(&mut map, &one_sided_room, &doom_setup(), &textures())
        .expect_err("a one-sided line is no use line");
    assert!(refuses(first_error, "not two-sided"), "the hint says why");

    let mut plain_jamb = sides();
    plain_jamb.jamb_lines = vec![4_usize];
    let second_error = apply_door(&mut map, &plain_jamb, &doom_setup(), &textures())
        .expect_err("the wall does not border the recess");
    assert!(refuses(second_error, "jamb linedef 4"), "the jamb is named");
    assert_eq!(map, before, "both refused calls leave the map alone");

    let mut dangling = map.linedefs.remove(0_usize);
    dangling.front = Some(99_usize);
    map.linedefs.insert(0_usize, dangling);
    let third_error = apply_door(&mut map, &sides(), &doom_setup(), &textures())
        .expect_err("the sidedef reference dangles");
    assert!(
        refuses(third_error, "sidedef 99"),
        "the missing side is named"
    );
}
