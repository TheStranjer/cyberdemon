//! The role names: what [`Describe`] says a sector *is*. The plan-built
//! door band, lift platform, secret room and teleport destination square,
//! the walk-over exit room and the midpoint rule that sweeps a sector in,
//! the `Hexen` activation field, the stair step between two floors and the
//! per-family sector-special readings, against the hand-drawn square room.

use alloc::vec;
use alloc::vec::Vec;

use crate::plan::model::{
    ConnectionKind, DoorStyle, ExitStyle, Feature as PlanFeature, KeyColor as PlanKeyColor,
};
use crate::wad::map::MapFormat;
use crate::wad::map::common::fixtures::room;
use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::describe::{Describe, sector_facts};
use crate::wad::map::features::LiftStyle;
use crate::wad::map::flags::{BLOCKING, PLAYER_USE_BITS};

use super::plans::{built, plan_link, plan_of, plan_room};
use super::{
    cell, described, doom_defs, has_role, neighbour_across, nested_exit_map, retune, room_sector,
    row, square_map,
};

#[test]
fn a_plan_built_door_band_wears_the_door_role_and_offers_its_doors() {
    let blueprint = plan_of(
        vec![
            plan_room("a", (0, 256), cell(0_i32)),
            plan_room("b", (256, 256), cell(0_i32)),
        ],
        vec![plan_link(
            "d1",
            ("a", "b"),
            ConnectionKind::Door {
                style: DoorStyle::Locked(PlanKeyColor::Blue),
            },
            64,
        )],
        Vec::new(),
        16_i64,
    );
    let (map, rows) = built(&blueprint, MapFormat::Doom);
    let band = *row(&rows, "door").sectors.first().expect("the band");
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let band_facts = describe.sector(band).expect("the band exists");
    assert!(has_role(&band_facts, "door"), "the band is a door");
    for column in [0_usize, 1] {
        let room_index = room_sector(&map, column);
        let room_facts = describe.sector(room_index).expect("a room exists");
        assert!(
            !has_role(&room_facts, "door"),
            "room {column} stands full-height, not a door"
        );
        assert_eq!(
            neighbour_across(&room_facts, band).via,
            "door",
            "room {column} reaches the band through a door"
        );
    }
}

#[test]
fn a_plan_built_lift_is_a_lift_and_rides_up_from_its_low_neighbour() {
    let lifts = plan_of(
        vec![
            plan_room("low", (0, 256), cell(0_i32)),
            plan_room("pad", (256, 256), cell(96_i32)),
        ],
        vec![plan_link("c1", ("low", "pad"), ConnectionKind::Opening, 64)],
        vec![PlanFeature::Lift {
            room: "pad".to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
        }],
        0_i64,
    );
    let (map, rows) = built(&lifts, MapFormat::Doom);
    let platform = *row(&rows, "lift").sectors.first().expect("the platform");
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let pad = describe.sector(platform).expect("the platform exists");
    assert!(has_role(&pad, "lift"), "the platform is a lift");
    let low_index = room_sector(&map, 0);
    let low = describe.sector(low_index).expect("the low room exists");
    let across = neighbour_across(&low, platform);
    assert_eq!(across.via, "lift", "the way up is the lift");
    assert_eq!(across.floor_delta, 96_i32, "over the ledge's height");
}

#[test]
fn a_plan_built_secret_room_wears_the_secret_role() {
    let mut hidden = plan_room("b", (256, 256), cell(0_i32));
    hidden.secret = true;
    let blueprint = plan_of(
        vec![plan_room("a", (0, 256), cell(0_i32)), hidden],
        vec![plan_link("c1", ("a", "b"), ConnectionKind::Opening, 64)],
        Vec::new(),
        16_i64,
    );
    let (map, rows) = built(&blueprint, MapFormat::Doom);
    let secret = *row(&rows, "secret")
        .sectors
        .first()
        .expect("the secret room");
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let facts = describe.sector(secret).expect("the secret room exists");
    assert!(has_role(&facts, "secret"), "the flagged room is a secret");
    let open_index = room_sector(&map, 0);
    let open = describe.sector(open_index).expect("the hall exists");
    assert!(!has_role(&open, "secret"), "the hall beside it is not");
}

#[test]
fn a_walk_over_exit_room_is_an_exit_room() {
    let blueprint = plan_of(
        vec![
            plan_room("a", (0, 256), cell(0_i32)),
            plan_room("b", (256, 256), cell(0_i32)),
        ],
        vec![plan_link("c1", ("a", "b"), ConnectionKind::Opening, 64)],
        vec![PlanFeature::Exit {
            room: "b".to_owned(),
            style: ExitStyle::WalkOver,
            secret: false,
            wall: None,
            at: None,
        }],
        16_i64,
    );
    let (map, _) = built(&blueprint, MapFormat::Doom);
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let exit_room = describe
        .sector(room_sector(&map, 1))
        .expect("the exit room exists");
    assert!(
        has_role(&exit_room, "exit_room"),
        "walking its boundary ends the level"
    );
    let hall = describe
        .sector(room_sector(&map, 0))
        .expect("the hall exists");
    assert!(
        !has_role(&hall, "exit_room"),
        "and the far room does not share the line"
    );
}

#[test]
fn a_walk_over_exit_line_sweeps_its_midpoint_sector_in() {
    let defs = doom_defs();
    let crossed = nested_exit_map(MapFormat::Doom, 52_i32, BLOCKING);
    let describe = Describe::new(&crossed, &defs, SKY_FLAT);
    let outer = describe.sector(0).expect("the outer sector exists");
    assert!(
        has_role(&outer, "exit_room"),
        "the line's own side reads by the boundary rule"
    );
    let inner = describe.sector(1).expect("the inner sector exists");
    assert!(
        has_role(&inner, "exit_room"),
        "and the walk-over midpoint stands in the inner square"
    );

    let pressed = nested_exit_map(MapFormat::Doom, 11_i32, BLOCKING);
    let by_use = Describe::new(&pressed, &defs, SKY_FLAT);
    assert!(
        !has_role(&by_use.sector(1).expect("inner"), "exit_room"),
        "a use line sweeps nothing into the inner square"
    );
    assert!(
        has_role(&by_use.sector(0).expect("outer"), "exit_room"),
        "while the side rule still holds for it"
    );
}

#[test]
fn hexen_exits_follow_their_activation_field() {
    let defs = doom_defs();
    let portal = nested_exit_map(MapFormat::Hexen, 243_i32, 0_u32);
    let describe = Describe::new(&portal, &defs, SKY_FLAT);
    assert!(
        has_role(&describe.sector(1).expect("inner"), "exit_room"),
        "a playercross portal walks over into the inner square"
    );
    let used = nested_exit_map(MapFormat::Hexen, 243_i32, PLAYER_USE_BITS);
    let pressed = Describe::new(&used, &defs, SKY_FLAT);
    assert!(
        !has_role(&pressed.sector(1).expect("inner"), "exit_room"),
        "and a playeruse one sweeps nothing"
    );
    assert!(
        has_role(&pressed.sector(0).expect("outer"), "exit_room"),
        "while its own side still answers"
    );
}

#[test]
fn stepped_floors_flag_the_middle_step_only() {
    let climb = plan_of(
        vec![
            plan_room("a", (0, 256), cell(0_i32)),
            plan_room("b", (256, 256), cell(32_i32)),
        ],
        vec![plan_link(
            "s1",
            ("a", "b"),
            ConnectionKind::Stairs {
                step_height: 16_i32,
                step_depth: 24_i32,
            },
            64,
        )],
        Vec::new(),
        0_i64,
    );
    let (map, _) = built(&climb, MapFormat::Doom);
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let stepped: Vec<usize> = (0..map.sectors.len())
        .filter_map(|index| describe.sector(index))
        .filter(|facts| has_role(facts, "stair_step"))
        .map(|facts| facts.sector)
        .collect();
    assert_eq!(stepped.len(), 1, "one step between the rooms: {stepped:?}");
    let middle = describe
        .sector(*stepped.first().expect("the step"))
        .expect("it exists");
    assert_eq!(middle.floor, 16_i32, "the 16-unit tread");
    let vias: Vec<&str> = middle.neighbours.iter().map(|n| n.via.as_str()).collect();
    assert!(vias.contains(&"step_up"), "one side rises: {vias:?}");
    assert!(vias.contains(&"step_down"), "the other falls: {vias:?}");
}

#[test]
fn a_plan_built_teleporter_marks_its_destination_square() {
    let teleported = plan_of(
        vec![
            plan_room("a", (0, 256), cell(0_i32)),
            plan_room("b", (512, 256), cell(0_i32)),
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
        16_i64,
    );
    let (map, rows) = built(&teleported, MapFormat::Doom);
    let squares = row(&rows, "teleporter").sectors;
    let pad = *squares.first().expect("the pad square");
    let square = *squares.get(1).expect("the destination square");
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let landing = describe.sector(square).expect("the square exists");
    assert!(
        has_role(&landing, "teleport_destination"),
        "a teleport action lands here"
    );
    let start = describe.sector(pad).expect("the pad exists");
    assert!(
        !has_role(&start, "teleport_destination"),
        "the pad itself is nobody's landing"
    );
}

#[test]
fn doom_sector_specials_state_their_vanilla_and_boom_roles() {
    let mut hurt = square_map();
    let mut damaging = room(0, 128, 160, 0);
    damaging.special = 4_i32;
    retune(&mut hurt, damaging);
    let facts = sector_facts(&hurt, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert_eq!(facts.roles, vec!["damaging".to_owned()], "20% and strobe");

    let mut lit = square_map();
    let mut flickering = room(0, 128, 160, 0);
    flickering.special = 1_i32;
    retune(&mut lit, flickering);
    let lit_facts = sector_facts(&lit, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert_eq!(
        lit_facts.roles,
        vec!["light_effect".to_owned()],
        "light on and off randomly"
    );

    let mut boom = square_map();
    let mut flagged = room(0, 128, 160, 0);
    flagged.special = 133_i32;
    retune(&mut boom, flagged);
    let boom_facts = sector_facts(&boom, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert_eq!(
        boom_facts.roles,
        vec!["damaging".to_owned(), "secret".to_owned()],
        "the Boom read: base 5 hurts and bit 128 hides"
    );

    let mut plain_secret = square_map();
    let mut hidden = room(0, 128, 160, 0);
    hidden.special = 128_i32;
    retune(&mut plain_secret, hidden);
    let hidden_facts =
        sector_facts(&plain_secret, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert_eq!(
        hidden_facts.roles,
        vec!["secret".to_owned()],
        "bit 128 alone hides a plain sector"
    );

    let mut negative = square_map();
    let mut below = room(0, 128, 160, 0);
    below.special = -3_i32;
    retune(&mut negative, below);
    let off_table = sector_facts(&negative, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert!(
        off_table.roles.is_empty(),
        "a negative special states nothing"
    );
}

#[test]
fn hexen_sector_specials_state_their_own_numbers() {
    let mut map = square_map();
    map.format = MapFormat::Hexen;
    let mut hidden = room(0, 128, 160, 0);
    hidden.special = 1024_i32;
    retune(&mut map, hidden);
    let described_map = described(&map, 0);
    assert_eq!(
        described_map.roles,
        vec!["secret".to_owned()],
        "bit 1024 hides a Hexen-family sector"
    );

    let mut nine = square_map();
    nine.format = MapFormat::Hexen;
    let mut doomish = room(0, 128, 160, 0);
    doomish.special = 9_i32;
    retune(&mut nine, doomish);
    assert!(
        described(&nine, 0).roles.is_empty(),
        "and the Doom family's 9 states nothing here"
    );

    for (special, role) in [
        (69_i32, "damaging"),
        (85_i32, "damaging"),
        (65_i32, "light_effect"),
        (81_i32, "light_effect"),
    ] {
        let mut one = square_map();
        one.format = MapFormat::Hexen;
        let mut row_special = room(0, 128, 160, 0);
        row_special.special = special;
        retune(&mut one, row_special);
        assert_eq!(
            described(&one, 0).roles,
            vec![role.to_owned()],
            "special {special} states {role}"
        );
    }
}
