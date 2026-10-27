//! Unit tests of the `features` group: the door pipeline in [`door`],
//! the window one in [`window`], the switch and exit tools in
//! [`switches`], the two sector-special steps in [`specials`], the
//! stairs/lift/teleporter movers in [`movers`] and the `place_things`
//! parsing and distribution in [`place`].
//!
//! [`two_rooms`], [`one_room`], [`room_and_hall`] and
//! [`hall_and_two_rooms`] build the fixture maps, [`vertex`], [`side`],
//! [`solid_line`], [`open_line`] and
//! [`room`] the records they are wound from, [`code`] and [`refuse`]
//! read the refusals.

mod door;
mod movers;
mod place;
mod specials;
mod switches;
mod window;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// The message of a refusal that must be an `InvalidArgument`.
pub(super) fn refuse(error: &CyberError) -> String {
    assert_eq!(code(error), "invalid_argument", "refusal code");
    error.to_string()
}

/// The message of a refusal that must be a `NotFound`, as the shared
/// index guard phrases its refusals.
pub(super) fn missing(error: &CyberError) -> String {
    assert_eq!(code(error), "not_found", "index-refusal code");
    error.to_string()
}

/// A vertex at `x`/`y`.
pub(super) fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Default::default()
    }
}

/// A sidedef over `sector` wearing `WALL1` in the middle.
pub(super) fn side(sector: usize) -> Sidedef {
    Sidedef {
        upper: "-".to_owned(),
        lower: "-".to_owned(),
        middle: "WALL1".to_owned(),
        sector,
        ..Default::default()
    }
}

/// A one-sided blocking line from `v1` to `v2` over `front`.
pub(super) fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1_u32,
        front: Some(front),
        ..Default::default()
    }
}

/// A two-sided line from `v1` to `v2` over `front`/`back`.
pub(super) fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4_u32,
        front: Some(front),
        back: Some(back),
        ..Default::default()
    }
}

/// A sector with the given heights and tag.
pub(super) fn room(floor_height: i32, ceiling_height: i32, tag: i32) -> Sector {
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

/// The two-room test map: a hall (sector 0) and a raised room (sector
/// 1, tag 1) sharing the line 2, wound like the repo's fixtures (the
/// front side bounds the line's right).
pub(super) fn two_rooms() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("a valid name"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            solid_line(0, 5, 0),
            solid_line(5, 4, 1),
            open_line(4, 1, 2, 3),
            solid_line(1, 0, 4),
            solid_line(4, 3, 5),
            solid_line(3, 2, 6),
            solid_line(2, 1, 7),
        ],
        sidedefs: vec![
            side(0),
            side(0),
            side(0),
            side(1),
            side(0),
            side(1),
            side(1),
            side(1),
        ],
        sectors: vec![room(0, 128, 0), room(32, 128, 1)],
        ..Default::default()
    }
}

/// The single-room test map: a 128 square, every wall one-sided,
/// nothing behind any of them.
pub(super) fn one_room() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("a valid name"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(0, 128),
            vertex(128, 128),
            vertex(128, 0),
        ],
        linedefs: vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 0),
            solid_line(2, 3, 0),
            solid_line(3, 0, 0),
        ],
        sidedefs: vec![side(0), side(0), side(0), side(0)],
        sectors: vec![room(0, 128, 0)],
        ..Default::default()
    }
}

/// Room A (sector 0, `x 0..128`) touching a hall (sector 1, east of
/// `x 128`); A's west wall is the one-sided door wall, its east line
/// the two-sided boundary the outward ray stops at 128 units.
pub(super) fn room_and_hall() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("a valid name"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(0, 128),
            vertex(128, 128),
            vertex(128, 0),
            vertex(192, 128),
            vertex(192, 0),
        ],
        linedefs: vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            open_line(2, 3, 2, 3),
            solid_line(3, 0, 4),
            solid_line(2, 4, 5),
            solid_line(4, 5, 6),
            solid_line(5, 3, 7),
        ],
        sidedefs: vec![
            side(0),
            side(0),
            side(0),
            side(1),
            side(0),
            side(1),
            side(1),
            side(1),
        ],
        sectors: vec![room(0, 128, 0), room(0, 128, 0)],
        ..Default::default()
    }
}

/// The hall-and-two-rooms test map: a hall (sector 0, `x 128..384`)
/// between room A (sector 1, west of it) and room B (sector 2, east of
/// it). The hall owns two two-sided boundaries — line 2 with room A and
/// line 6 with room B — so a test can dress one of them with another
/// feature's action and leave the other plain.
pub(super) fn hall_and_two_rooms() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("a valid name"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(128, 0),
            vertex(384, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(384, 256),
            vertex(128, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            solid_line(0, 7, 0),
            solid_line(7, 6, 1),
            open_line(6, 1, 2, 3),
            solid_line(1, 0, 4),
            solid_line(2, 1, 5),
            solid_line(6, 5, 6),
            open_line(2, 5, 7, 8),
            solid_line(3, 2, 9),
            solid_line(5, 4, 10),
            solid_line(4, 3, 11),
        ],
        sidedefs: vec![
            side(1),
            side(1),
            side(1),
            side(0),
            side(1),
            side(0),
            side(0),
            side(2),
            side(0),
            side(2),
            side(2),
            side(2),
        ],
        sectors: vec![room(0, 128, 0), room(0, 128, 1), room(0, 128, 2)],
        ..Default::default()
    }
}
