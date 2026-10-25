//! The wall chooser's tests: [`choose_switch_wall`] answering the
//! compass side, the longest run, the lowest index on a tie, the point
//! that ranks only the walls it was asked about, and the refusal of a
//! side with no wall.
//!
//! [`choose_switch_wall`]: crate::plan::features::switches::choose_switch_wall

use super::super::{door, two_rooms, wired};
use super::{c_room, ends, stacked_doors, three_in_row};
use crate::plan::features::switches::{WallChoice, choose_switch_wall};
use crate::plan::model::{DoorStyle, Side};
use crate::wad::map::Map;
use crate::wad::map::ops::Point;

/// The wall choice of room `room`: `side` facing walls, nearest to
/// `at` when one is given.
fn choice_in(side: Option<Side>, at: Option<Point>, room: &str) -> WallChoice<'_> {
    WallChoice { side, at, room }
}

/// The same choice inside room `a`, the fixture's first room.
fn choice(side: Option<Side>, at: Option<Point>) -> WallChoice<'static> {
    choice_in(side, at, "a")
}

/// The point at map coordinates `x`, `y`.
const fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

/// Room `a`'s chosen wall in the two-room remote-door fixture.
fn a_wall(map: &Map, side: Option<Side>) -> usize {
    choose_switch_wall(map, &[0], choice(side, None)).expect("a wall on the asked side")
}

#[test]
fn the_wall_choice_follows_the_compass_side() {
    let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
    let (compiled, _) = wired(&plan);
    let map = &compiled.map;
    assert_eq!(
        ends(map, a_wall(map, Some(Side::North))),
        ((0_i32, 128_i32), (120_i32, 128_i32)),
        "the north wall answers north"
    );
    assert_eq!(
        ends(map, a_wall(map, Some(Side::South))),
        ((120_i32, 0_i32), (0_i32, 0_i32)),
        "the south wall answers south"
    );
    assert_eq!(
        ends(map, a_wall(map, Some(Side::West))),
        ((0_i32, 0_i32), (0_i32, 128_i32)),
        "the west wall answers west"
    );
}

#[test]
fn the_longest_line_wins_within_the_chosen_side() {
    let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
    let (compiled, _) = wired(&plan);
    let map = &compiled.map;
    let east = choose_switch_wall(map, &[0], choice(Some(Side::East), None)).expect("an east wall");
    let ((x1, y1), (x2, y2)) = ends(map, east);
    assert_eq!((x1, x2), (120_i32, 120_i32), "the band-cut east wall");
    assert_eq!(
        y1.abs_diff(y2),
        72_u32,
        "the long of the two segments (the door band left an 8-unit stub below)"
    );
}

#[test]
fn without_a_side_the_longest_wall_wins() {
    let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
    let (compiled, _) = wired(&plan);
    let map = &compiled.map;
    assert_eq!(
        ends(map, a_wall(map, None)),
        ((0_i32, 0_i32), (0_i32, 128_i32)),
        "the untouched 128-unit west wall outsides the 120-unit north and south"
    );
}

#[test]
fn equal_walls_fall_to_the_lowest_index() {
    let (compiled, _) = wired(&stacked_doors());
    let map = &compiled.map;
    let tied: Vec<usize> = (0..map.linedefs.len())
        .filter(|&index| {
            let ((x1, y1), (x2, y2)) = ends(map, index);
            y1 == y2 && y1 == 120_i32 && x1.abs_diff(x2) == 40_u32
        })
        .collect();
    assert_eq!(tied.len(), 2, "the centred band leaves two equal segments");
    let chosen = choose_switch_wall(map, &[0], choice(Some(Side::North), None))
        .expect("two equal north segments");
    assert_eq!(
        chosen,
        *tied.iter().min().expect("two tied candidates"),
        "ties go to the lowest linedef index"
    );
}

#[test]
fn a_side_without_a_wall_names_the_room_and_the_side() {
    let (compiled, _) = wired(&three_in_row());
    let error = choose_switch_wall(&compiled.map, &[1], choice_in(Some(Side::East), None, "b"))
        .expect_err("room b meets room c on its east side");
    assert_eq!(error.code(), "invalid_argument", "a wall-less side");
    let message = error.to_string();
    assert!(
        message.contains("'b'") && message.contains("east"),
        "the refusal names room and side: {message}"
    );
}

/// The two west runs of the C fixture, each 48 units, and the choice
/// `at` decides between them.
#[test]
fn equal_walls_on_one_side_answer_a_point_not_their_order() {
    let (compiled, _) = wired(&c_room());
    let map = &compiled.map;
    let lower = choose_switch_wall(map, &[0], choice(Some(Side::West), Some(point(0, 24))))
        .expect("the lower west wall");
    let upper = choose_switch_wall(map, &[0], choice(Some(Side::West), Some(point(0, 104))))
        .expect("the upper west wall");
    assert_eq!(
        ends(map, lower),
        ((0_i32, 0_i32), (0_i32, 48_i32)),
        "the point near the lower run answers it"
    );
    assert_eq!(
        ends(map, upper),
        ((0_i32, 80_i32), (0_i32, 128_i32)),
        "the point near the upper run answers that one"
    );
    assert_eq!(
        choose_switch_wall(map, &[0], choice(Some(Side::West), None)).expect("a west wall"),
        lower.min(upper),
        "without a point the longest tie still falls to the lowest index"
    );
}

/// A point measures to the wall, not into it: the asked side still
/// bounds the candidates (the notch's own inner face counts as a west
/// wall: its void lies to the west), and with no side any one-sided
/// wall competes.
#[test]
fn a_point_ranks_only_the_walls_it_was_asked_about() {
    let (compiled, _) = wired(&c_room());
    let map = &compiled.map;
    let chosen = choose_switch_wall(map, &[0], choice(Some(Side::West), Some(point(64, 64))))
        .expect("a west-facing wall answers a point inside the room");
    assert_eq!(
        ends(map, chosen),
        ((48_i32, 48_i32), (48_i32, 80_i32)),
        "the notch's inner face looks west and stands nearest the room's middle"
    );
    let any = choose_switch_wall(map, &[0], choice(None, Some(point(120, 64))))
        .expect("no side, so the nearest one-sided wall of any");
    let ((x1, y1), (x2, y2)) = ends(map, any);
    assert_eq!(
        (x1, x2, y1.min(y2), y1.max(y2)),
        (128_i32, 128_i32, 0_i32, 128_i32),
        "the untouched east wall is the nearest to that point"
    );
}
