//! The window tests: the trim heights that move the opening, the too
//! small and too wide refusals, the one-sided span and the dressed
//! blocking lines of [`build_window`].

use serde_json::from_str;

use crate::tools::features::door::{Target, target_of};
use crate::tools::features::wall::choose_between;
use crate::tools::features::window::{
    AddWindowParams, BuiltWindow, WindowSkins, WindowSpec, build_window,
};
use crate::tools::maps::params::PointParam;
use crate::wad::classify::MapFormat;
use crate::wad::map::features::WindowHeights;
use crate::wad::map::{Linedef, Map};

use super::{missing, one_room, refuse, room_and_hall, two_rooms};

/// The skins the tests dress windows with.
fn window_skins() -> WindowSkins {
    WindowSkins {
        sill: "STEP1".to_owned(),
        wall: "WALL1".to_owned(),
    }
}

/// A `add_window` request parsed from `json`.
fn window_args(json: &str) -> AddWindowParams {
    from_str(json).expect("test window requests parse")
}

/// A default-trim window spec over `target`.
fn window_spec(target: Target) -> WindowSpec {
    WindowSpec {
        target,
        at: None,
        width: 64_i32,
        trim: WindowHeights {
            sill: 32_i32,
            lintel: 24_i32,
        },
        skins: window_skins(),
    }
}

/// The sidedef index of the room-facing side of a window view line:
/// the side that does not belong to window sector `window`.
fn room_side_of(map: &Map, line: &Linedef, window: usize) -> usize {
    if map.side_sector(line.front) == Some(window) {
        line.back.expect("the room side")
    } else {
        line.front.expect("the room side")
    }
}

/// The sidedef index of the strip-facing side of a window view line:
/// the side that belongs to window sector `window`.
fn strip_side_of(map: &Map, line: &Linedef, window: usize) -> usize {
    if map.side_sector(line.front) == Some(window) {
        line.front.expect("the strip side")
    } else {
        line.back.expect("the strip side")
    }
}

/// Requires every reported window line to be a two-sided blocking
/// view line with the sill below and the wall above on its room
/// side, and the strip side clear; returns the room sectors.
fn expect_window_lines(map: &Map, built: &BuiltWindow, window: usize) -> Vec<usize> {
    assert!(!built.lines.is_empty(), "the window has view lines");
    let mut rooms = Vec::new();
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a view line");
        assert!(line.front.is_some() && line.back.is_some(), "two-sided");
        assert_ne!(line.flags & 4_u32, 0_u32, "the two-sided bit stands");
        assert_ne!(line.flags & 1_u32, 0_u32, "blocking: nobody walks through");
        let room = map
            .sidedefs
            .get(room_side_of(map, line, window))
            .expect("the room side");
        assert_eq!(room.lower, "STEP1", "the sill under the opening");
        assert_eq!(room.upper, "WALL1", "the wall over it");
        let strip = map
            .sidedefs
            .get(strip_side_of(map, line, window))
            .expect("the strip side");
        assert_eq!(strip.upper, "-", "from inside the window the line is open");
        assert_eq!(strip.lower, "-", "and stays open below");
        rooms.push(room.sector);
    }
    rooms
}

#[test]
fn a_window_between_touching_rooms_is_spanned_and_blocked() {
    let mut map = two_rooms();
    let built = build_window(&mut map, &window_spec(Target::Between { a: 0, b: 1 }))
        .expect("a window between the rooms");
    assert_eq!(built.window_sector, 2, "the third sector");
    let window = map.sectors.get(2).expect("the window sector");
    assert_eq!(window.floor_height, 64_i32, "max(0, 32) + 32");
    assert_eq!(window.ceiling_height, 104_i32, "min(128, 128) - 24");
    assert_eq!(window.special, 0_i32, "no effect");
    assert_eq!(window.tag, 0_i32, "untagged");
    assert_eq!(window.floor_flat, "FLOOR1", "room A's flats");
    assert_eq!(window.ceiling_flat, "CEIL1", "and ceiling flat");
    assert_eq!(window.light, 176_i32, "and light");
    assert!(
        built
            .notes
            .iter()
            .any(|note| note.contains("void strip") && note.contains("window strip")),
        "the strip note: {:?}",
        built.notes
    );
    let rooms = expect_window_lines(&map, &built, 2_usize);
    assert_eq!(built.lines.len(), 2, "one view line per room");
    assert!(
        rooms.contains(&0_usize) && rooms.contains(&1_usize),
        "both rooms see the glass: {rooms:?}"
    );
}

#[test]
fn the_window_trims_move_the_opening() {
    let mut map = two_rooms();
    let spec = WindowSpec {
        trim: WindowHeights {
            sill: 40_i32,
            lintel: 8_i32,
        },
        ..window_spec(Target::Between { a: 0, b: 1 })
    };
    let built = build_window(&mut map, &spec).expect("a taller window");
    let window = map.sectors.get(built.window_sector).expect("the window");
    assert_eq!(window.floor_height, 72_i32, "max(0, 32) + 40");
    assert_eq!(window.ceiling_height, 120_i32, "min(128, 128) - 8");
}

#[test]
fn a_too_small_opening_refuses_before_anything_is_carved() {
    let mut map = two_rooms();
    let spec = WindowSpec {
        trim: WindowHeights {
            sill: 90_i32,
            lintel: 24_i32,
        },
        ..window_spec(Target::Between { a: 0, b: 1 })
    };
    let message = refuse(&build_window(&mut map, &spec).unwrap_err());
    assert!(message.contains("at least 16"), "{message}");
    assert_eq!(map, two_rooms(), "not even the void strip was carved");
}

#[test]
fn a_window_wider_than_the_wall_names_the_available_length() {
    let mut map = two_rooms();
    let spec = WindowSpec {
        width: 300_i32,
        ..window_spec(Target::Between { a: 0, b: 1 })
    };
    let message = refuse(&build_window(&mut map, &spec).unwrap_err());
    assert!(message.contains("256 map units long"), "{message}");
    assert_eq!(map.sectors.len(), 2, "no sector was drawn");
}

#[test]
fn a_one_sided_window_spans_to_the_room_behind() {
    let mut map = room_and_hall();
    let built =
        build_window(&mut map, &window_spec(Target::Linedef(0))).expect("the ray finds the hall");
    let window = map.sectors.get(built.window_sector).expect("the window");
    assert_eq!(
        (window.floor_height, window.ceiling_height),
        (32_i32, 104_i32),
        "room A's own heights on both sides of the trim"
    );
    let rooms = expect_window_lines(&map, &built, built.window_sector);
    assert_eq!(
        built.lines.len(),
        3,
        "the bay: the hall and both split sides"
    );
    assert!(
        rooms.contains(&1_usize),
        "the hall looks through: {rooms:?}"
    );
}

#[test]
fn a_one_sided_window_wall_with_nothing_behind_refuses() {
    let mut map = one_room();
    let message = refuse(&build_window(&mut map, &window_spec(Target::Linedef(0))).unwrap_err());
    assert!(
        message.contains("no room behind this wall within 256 units"),
        "{message}"
    );
    assert_eq!(map, one_room(), "nothing was drawn");
}

#[test]
fn window_targets_are_validated_like_the_door_s() {
    let checked = two_rooms();
    let message = missing(&choose_between(&checked, 0, 42, None).unwrap_err());
    assert!(message.contains("sector 42 does not exist"), "{message}");
    let mut map = two_rooms();
    map.format = MapFormat::Hexen;
    let built = build_window(&mut map, &window_spec(Target::Between { a: 0, b: 1 }))
        .expect("a Hexen map windows the same way");
    let rooms = expect_window_lines(&map, &built, built.window_sector);
    assert_eq!(rooms.len(), 2, "both rooms face the glass");
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a view line");
        assert_eq!(line.special, 0_i32, "a window carries no action");
    }
}

#[test]
fn add_window_params_read_the_wire_shape() {
    let full = window_args(
        r#"{"between": [0, 1], "at": {"x": 64, "y": 0}, "width": 96,
                "sill": 48, "lintel": 0, "map": "MAP01"}"#,
    );
    assert_eq!(full.between, Some([0_usize, 1_usize]));
    assert_eq!(full.at, Some(PointParam { x: 64, y: 0 }));
    assert_eq!(full.width, Some(96_i32));
    assert_eq!(full.sill, Some(48_i32));
    assert_eq!(full.lintel, Some(0_i32));
    assert_eq!(full.map.as_deref(), Some("MAP01"));
    let minimal = window_args(r#"{"linedef": 2}"#);
    assert_eq!(minimal.width, None, "the defaults stand");
    assert_eq!(minimal.sill, None);
    assert_eq!(minimal.lintel, None);
    assert_eq!(minimal.at, None);
    let neither = window_args("{}");
    let message = refuse(
        &target_of(neither.between, neither.linedef, "the window looks between").unwrap_err(),
    );
    assert!(message.contains("exactly one"), "{message}");
    assert!(
        message.contains("the window looks between"),
        "the window wording: {message}"
    );
    let both = window_args(r#"{"between": [0, 1], "linedef": 2}"#);
    assert!(
        refuse(&target_of(both.between, both.linedef, "the window looks between").unwrap_err())
            .contains("exactly one")
    );
}
