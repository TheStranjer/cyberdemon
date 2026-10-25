//! The doors, switches, lifts and teleporters a [`crate::plan`] build wires
//! are the ones the walk reads back: locked bands locking both room-facing
//! lines, switches raising remote doors, platforms carrying the player to
//! the high room and pads landing them in the far one.

use alloc::vec;

use super::super::{KeyColor, analyze_playability};
use crate::plan::model::{
    ConnectionKind, DoorStyle, Feature as PlanFeature, KeyColor as PlanKeyColor, Plan, Side,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::features::LiftStyle;
use crate::wad::map::flags::{FlagFamily, Skill};

use super::{
    BLUE_CARD, built, cell, clear_specials, door_band, link, middle, plan, room, sectors, stand,
};

#[test]
fn a_plan_built_locked_door_locks_both_of_its_room_facing_lines() {
    let blueprint = plan(
        vec![
            room("a", (0, 256), cell(0_i32)),
            room("b", (256, 256), cell(0_i32)),
        ],
        vec![link(
            "d1",
            ("a", "b"),
            ConnectionKind::Door {
                style: DoorStyle::Locked(PlanKeyColor::Blue),
            },
            64,
        )],
        Vec::new(),
    );
    let (mut map, rows) = built(&blueprint, MapFormat::Doom);
    let door = rows.first().expect("the wired door row");
    assert_eq!(door.kind, "door", "the row is the door");
    let band = *door.sectors.first().expect("the band sector");
    stand(&mut map, 1, middle(0), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "the band is shut and its key is nowhere"
    );
    assert_eq!(found.doors_never_opened, vec![band], "so it never opened");
    assert_eq!(
        found.locks.len(),
        door.lines.len(),
        "every room-facing line of the door is a lock"
    );
    assert!(
        found.locks.iter().all(|lock| lock.color == KeyColor::Blue),
        "and every one of them wants the blue card"
    );
    assert!(
        found.locks.iter().all(|lock| !lock.opened),
        "all of them stayed shut"
    );

    let mut with_key = map.clone();
    stand(&mut with_key, BLUE_CARD, middle(0), 7);
    let walked = analyze_playability(&with_key, FlagFamily::Doom, Skill::Medium);
    assert!(
        walked.reachable.len() > found.reachable.len(),
        "drop the card in the hall and the whole map opens: {:?}",
        walked.reachable
    );
    assert!(walked.doors_never_opened.is_empty(), "the band rose");
}

#[test]
fn a_plan_built_switch_raises_the_remote_door_it_names() {
    let rooms = vec![
        room("a", (0, 256), cell(0_i32)),
        room("b", (256, 256), cell(0_i32)),
    ];
    let remote = link(
        "d1",
        ("a", "b"),
        ConnectionKind::Door {
            style: DoorStyle::Remote,
        },
        64,
    );
    let bare = plan(rooms.clone(), vec![remote.clone()], Vec::new());
    let (bare_map, bare_rows) = built(&bare, MapFormat::Doom);
    let mut handleless = bare_map;
    stand(&mut handleless, 1, middle(0), 7);
    let shut = analyze_playability(&handleless, FlagFamily::Doom, Skill::Medium);
    let band = door_band(&bare_rows);
    assert!(
        shut.doors_never_opened.is_empty(),
        "a remote door writes no special on its own lines, so there is no trigger              to report as unworked"
    );
    assert!(
        !shut.reachable.contains(&band),
        "and the band it leaves shut is still out of reach"
    );

    let wired = plan(
        rooms,
        vec![remote],
        vec![PlanFeature::Switch {
            room: "a".to_owned(),
            opens: "d1".to_owned(),
            wall: Some(Side::West),
            repeatable: true,
            at: None,
        }],
    );
    let (wired_map, rows) = built(&wired, MapFormat::Doom);
    let mut played = wired_map;
    stand(&mut played, 1, middle(0), 7);
    let found = analyze_playability(&played, FlagFamily::Doom, Skill::Medium);
    assert!(
        found.reachable.contains(&band),
        "the switch mounted in the hall raises the band: {:?}",
        found.reachable
    );
    assert!(found.doors_never_opened.is_empty(), "and it opened");
    assert_eq!(door_band(&rows), band, "the same band in both builds");
}

#[test]
fn a_plan_built_lift_reaches_the_high_room_it_serves() {
    let lifts = Plan {
        rooms: vec![
            room("low", (0, 256), cell(0_i32)),
            room("pad", (256, 256), cell(96_i32)),
        ],
        connections: vec![link("c1", ("low", "pad"), ConnectionKind::Opening, 64)],
        features: vec![PlanFeature::Lift {
            room: "pad".to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
        }],
        wall_thickness: 0_i64,
    };
    let (mut map, rows) = built(&lifts, MapFormat::Doom);
    let lift = rows
        .iter()
        .find(|row| row.kind == "lift")
        .expect("the lift row");
    let platform = *lift.sectors.first().expect("the platform sector");
    let edges = lift.lines.clone();
    stand(&mut map, 1, middle(0), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert!(
        found.reachable.contains(&platform),
        "the platform rides the player up to the high room: {:?}",
        found.reachable
    );

    let mut stranded = map.clone();
    clear_specials(&mut stranded, &edges);
    let stuck = analyze_playability(&stranded, FlagFamily::Doom, Skill::Medium);
    assert!(
        !stuck.reachable.contains(&platform),
        "and with the specials stripped the same map leaves them below"
    );
    assert!(
        found.reachable.len() > stuck.reachable.len(),
        "the lift is what widens the map"
    );
}

#[test]
fn a_plan_built_teleporter_reaches_the_far_room() {
    let teleported = plan(
        vec![
            room("a", (0, 256), cell(0_i32)),
            room("b", (512, 256), cell(0_i32)),
        ],
        Vec::new(),
        vec![PlanFeature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "b".to_owned(),
            pad: Some((64, 64)),
            destination: Some((576, 64)),
            angle: 0_i32,
            two_way: false,
        }],
    );
    let (mut map, rows) = built(&teleported, MapFormat::Doom);
    let square = rows
        .iter()
        .find(|row| row.kind == "teleporter")
        .and_then(|row| row.sectors.get(1))
        .copied()
        .expect("the destination square");
    stand(&mut map, 1, middle(0), 7);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert!(
        found.reachable.contains(&square),
        "the pad in room a lands the player in room b: {:?}",
        found.reachable
    );
    assert!(
        found.reachable.len() > 2,
        "and they can walk on from the square: {:?}",
        found.reachable
    );
}
