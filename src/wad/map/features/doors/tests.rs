//! Shared builders for the door core's tests: the sidedef, line and sector
//! makers, the `Map` doorway both topics dress, the [`DoorSides`],
//! [`DoorTextures`] and [`DoorLineSetup`] the calls pass, and the refusal
//! reader they assert on.
//!
//! [`apply`] runs [`apply_door`] and [`face_room_line`] over the doorway;
//! [`specials`] checks the two [`door_specials`] tables, the track roles and
//! [`next_free_tag`].

mod apply;
mod specials;

use super::{DoorLineSetup, DoorSides, DoorStyle, DoorTextures, door_specials};
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

/// The tag the table tests ask `door_specials` for.
pub(super) const SAMPLE_TAG: i32 = 42;

/// A sidedef over `sector` with the model's empty-face names.
pub(super) fn side(sector: usize) -> Sidedef {
    Sidedef {
        upper: "-".to_owned(),
        lower: "-".to_owned(),
        middle: "-".to_owned(),
        sector,
        ..Default::default()
    }
}

/// A two-sided line between sidedefs `front` and `back`, running
/// from vertex 0 to vertex 1.
pub(super) fn two_sided(front: usize, back: usize) -> Linedef {
    Linedef {
        v1: 0_usize,
        v2: 1_usize,
        front: Some(front),
        back: Some(back),
        flags: 4_u32,
        ..Default::default()
    }
}

/// A blocking one-sided line with front sidedef `front`, running
/// from vertex 0 to vertex 1.
pub(super) fn one_sided(front: usize) -> Linedef {
    Linedef {
        v1: 0_usize,
        v2: 1_usize,
        front: Some(front),
        flags: 1_u32,
        ..Default::default()
    }
}

/// A sector at the given heights, with the named flats and tag.
pub(super) fn sector(floor_height: i32, ceiling_height: i32, tag: i32) -> Sector {
    Sector {
        floor_height,
        ceiling_height,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "CEIL1".to_owned(),
        light: 176_i32,
        tag,
        ..Default::default()
    }
}

/// A doorway: room sector 0, door sector 1, and
/// - linedef 0: two-sided, front (sidedef 0) facing the room — correct;
/// - linedef 1: two-sided, front (sidedef 2) facing the door — backwards;
/// - linedef 2: two-sided jamb, front (sidedef 4) room, back (5) door;
/// - linedef 3: jamb already one-sided with front (sidedef 6) over the door;
/// - linedef 4: a plain wall of the room, not part of the door.
///
/// Lines 0 and 1 start top-unpegged (bit 8) to check it is cleared.
pub(super) fn doorway(format: MapFormat) -> Map {
    Map {
        format,
        sectors: vec![sector(0_i32, 128_i32, 0_i32), sector(0_i32, 128_i32, 0_i32)],
        vertices: vec![Vertex::default(), Vertex::default()],
        linedefs: vec![
            Linedef {
                flags: 12_u32,
                ..two_sided(0_usize, 1_usize)
            },
            Linedef {
                flags: 12_u32,
                ..two_sided(2_usize, 3_usize)
            },
            two_sided(4_usize, 5_usize),
            one_sided(6_usize),
            one_sided(7_usize),
        ],
        sidedefs: vec![
            side(0_usize), // 0: line 0 front, room
            side(1_usize), // 1: line 0 back, door
            side(1_usize), // 2: line 1 front, door
            side(0_usize), // 3: line 1 back, room
            side(0_usize), // 4: line 2 front, room
            side(1_usize), // 5: line 2 back, door
            side(1_usize), // 6: line 3 front, door
            side(0_usize), // 7: line 4 front, room wall
        ],
        ..Map::default()
    }
}

/// The doorway indices of [`doorway`].
pub(super) fn sides() -> DoorSides {
    DoorSides {
        door_sector: 1_usize,
        room_lines: vec![0_usize, 1_usize],
        jamb_lines: vec![2_usize, 3_usize],
    }
}

/// Door textures with distinctive names.
pub(super) fn textures() -> DoorTextures {
    DoorTextures {
        door: "DOOR7".to_owned(),
        track: "DOORTRAK".to_owned(),
        floor_flat: "FLOOR6_8".to_owned(),
    }
}

/// The `Doom` use-door action for the doorway tests.
pub(super) fn doom_setup() -> DoorLineSetup {
    door_specials(FlagFamily::Doom, DoorStyle::Normal, SAMPLE_TAG)
}

/// Whether `error` is the `InvalidArgument` mentioning `needle`.
pub(super) fn refuses(error: CyberError, needle: &str) -> bool {
    let CyberError::InvalidArgument(message) = error else {
        panic!("not an InvalidArgument: {error:?}");
    };
    message.contains(needle)
}
