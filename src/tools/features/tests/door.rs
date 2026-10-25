//! The door pipeline's tests: the style names and wall targets, the
//! `choose` resolutions, the touching-rooms carve, the recess record and
//! its dressing, and the whole [`build_door`] runs on the fixture maps.

use serde_json::from_str;

use crate::themes::Role;
use crate::tools::features::door::{
    AddDoorParams, Chosen, DoorSkins, DoorSpec, Dress, Groups, Placed, Target, build_door,
    door_groups, door_record, dress_door, style_name, style_names, style_of, target_of, track_role,
};
use crate::tools::features::out_of_range;
use crate::tools::features::wall::{
    centroid, choose_between, choose_linedef, half_axis, recess_depth, side_sector,
    strip_face_lost, strip_wall,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle;
use crate::wad::map::features::door_wall;
use crate::wad::map::ops::Point;

use super::{code, one_room, open_line, refuse, room_and_hall, solid_line, two_rooms};

/// A request parsed from `json`.
fn args(json: &str) -> AddDoorParams {
    from_str(json).expect("test requests parse")
}

/// The skins the tests dress doors with.
fn skins() -> DoorSkins {
    DoorSkins {
        door: "DOOR1".to_owned(),
        track: "DOORTRAK".to_owned(),
        floor_flat: "FLAT10".to_owned(),
        wall: "WALL1".to_owned(),
    }
}

/// A full spec over `target`.
fn spec(target: Target) -> DoorSpec {
    DoorSpec {
        target,
        at: None,
        width: 64_i32,
        depth: None,
        style: DoorStyle::Normal,
        skins: skins(),
    }
}

#[test]
fn styles_round_trip_and_refuse_unknown_names() {
    for style in DoorStyle::all() {
        assert_eq!(style_of(style_name(style)).expect("canonical"), style);
        assert_eq!(
            style_of(&style_name(style).to_ascii_uppercase()).expect("case-insensitive"),
            style
        );
    }
    let message = refuse(&style_of("sliding").unwrap_err());
    assert!(
        message.contains("normal, fast, stay_open, locked_red, locked_blue, locked_yellow, remote"),
        "the refusal lists every name: {message}"
    );
    assert_eq!(style_names().len(), DoorStyle::all().len(), "one name each");
}

#[test]
fn locked_styles_get_their_coloured_track_roles() {
    assert_eq!(track_role(DoorStyle::LockedRed), Role::DoorTrackRed);
    assert_eq!(track_role(DoorStyle::LockedBlue), Role::DoorTrackBlue);
    assert_eq!(track_role(DoorStyle::LockedYellow), Role::DoorTrackYellow);
    for style in [
        DoorStyle::Normal,
        DoorStyle::Fast,
        DoorStyle::StayOpen,
        DoorStyle::Remote,
    ] {
        assert_eq!(track_role(style), Role::DoorTrack, "{style:?}");
    }
}

#[test]
fn exactly_one_of_between_and_linedef_resolves() {
    let pair = args(r#"{"between": [0, 1]}"#);
    assert!(matches!(
        target_of(pair.between, pair.linedef, "the door joins").expect("between"),
        Target::Between { a: 0, b: 1 }
    ));
    let single = args(r#"{"linedef": 4}"#);
    assert!(matches!(
        target_of(single.between, single.linedef, "the door joins").expect("linedef"),
        Target::Linedef(4)
    ));
    let both = args(r#"{"between": [0, 1], "linedef": 4}"#);
    assert!(
        refuse(&target_of(both.between, both.linedef, "the door joins").unwrap_err())
            .contains("exactly one")
    );
    let neither = args("{}");
    assert!(
        refuse(&target_of(neither.between, neither.linedef, "the door joins").unwrap_err())
            .contains("exactly one")
    );
}

#[test]
fn choose_validates_sector_and_linedef_indices() {
    let map = two_rooms();
    let second = refuse(&choose_between(&map, 0, 99, None).unwrap_err());
    assert!(second.contains("sector 99 is out of range"), "{second}");
    let first = refuse(&choose_between(&map, 99, 0, None).unwrap_err());
    assert!(first.contains("sector 99 is out of range"), "{first}");
    let wall = refuse(&choose_linedef(&map, 99, None).unwrap_err());
    assert!(wall.contains("linedef 99 is out of range"), "{wall}");
    let self_pair = refuse(&choose_between(&map, 0, 0, None).unwrap_err());
    assert!(self_pair.contains("no two-sided boundary"), "{self_pair}");
    assert_eq!(
        code(&out_of_range("sector", 9, 2)),
        "invalid_argument",
        "the shared refusal's code"
    );
}

#[test]
fn choose_linedef_reads_sidedness() {
    let map = two_rooms();
    let touching = choose_linedef(&map, 2, None).expect("the step line is two-sided");
    assert!(touching.touching, "its sectors differ");
    assert_eq!((touching.room_a, touching.room_b), (0_usize, 1_usize));
    assert_eq!(
        touching.centre,
        Point { x: 256, y: 128 },
        "the line's midpoint"
    );
    let solid = choose_linedef(&map, 0, None).expect("the west wall is one-sided");
    assert!(!solid.touching, "the hall owns its only side");
    assert_eq!((solid.room_a, solid.wall), (0_usize, 0_usize));
    let back_room = choose_linedef(&map, 5, None).expect("the east wall is one-sided");
    assert_eq!(back_room.room_a, 1_usize, "room 1 owns it");
}

#[test]
fn choose_linedef_refuses_useless_walls() {
    let mut map = two_rooms();
    map.linedefs.push(open_line(0, 1, 0, 0));
    let doubled = map.linedefs.len() - 1;
    let same = refuse(&choose_linedef(&map, doubled, None).unwrap_err());
    assert!(same.contains("both sides of linedef"), "{same}");
    map.linedefs.push(solid_line(1, 2, 0));
    let frontless = map.linedefs.len() - 1;
    if let Some(line) = map.linedefs.get_mut(frontless) {
        line.front = None;
        line.back = Some(3);
    }
    let only_back = refuse(&choose_linedef(&map, frontless, None).unwrap_err());
    assert!(only_back.contains("only a back side"), "{only_back}");
    map.linedefs.push(solid_line(2, 3, 0));
    let naked = map.linedefs.len() - 1;
    if let Some(line) = map.linedefs.get_mut(naked) {
        line.front = None;
    }
    let sideless = refuse(&choose_linedef(&map, naked, None).unwrap_err());
    assert!(sideless.contains("no usable sidedef"), "{sideless}");
}

#[test]
fn touching_doors_are_cut_apart_spanned_and_closed() {
    let mut map = two_rooms();
    let built = build_door(&mut map, &spec(Target::Between { a: 0, b: 1 }))
        .expect("a door between the rooms");
    assert_eq!(built.door_sector, 2, "the third sector");
    assert_eq!(built.lines.len(), 2, "one use line per room");
    assert_eq!(built.jambs.len(), 2, "a track wall on each side");
    assert_eq!(built.tag, None, "plain doors carry no tag");
    assert!(
        built.notes.iter().any(|note| note.contains("void strip")),
        "the strip note: {:?}",
        built.notes
    );
    let door = map.sectors.get(2).expect("the door sector");
    assert_eq!(door.ceiling_height, door.floor_height, "the door is closed");
    assert_eq!(door.floor_height, 0_i32, "room A's floor");
    assert_eq!(door.floor_flat, "FLAT10", "the theme's recess floor");
    assert_eq!(door.tag, 0_i32, "untagged");
    let mut fronts = Vec::new();
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a room line");
        assert_eq!(line.special, 1_i32, "the DR action");
        assert_eq!(line.tag, 0_i32, "tag 0 acts on the sector behind");
        let room_side = map
            .sidedefs
            .get(line.front.expect("a front side"))
            .expect("front");
        assert_eq!(room_side.upper, "DOOR1", "the face wears the door");
        fronts.push(side_sector(&map, line.front).expect("the front sector"));
    }
    assert!(
        fronts.contains(&0_usize) && fronts.contains(&1_usize),
        "fronts face their rooms: {fronts:?}"
    );
}

#[test]
fn jambs_are_one_sided_tracks() {
    let mut map = two_rooms();
    let built =
        build_door(&mut map, &spec(Target::Between { a: 0, b: 1 })).expect("a drawn doorway");
    for index in &built.jambs {
        let line = map.linedefs.get(*index).expect("a jamb");
        assert_eq!(line.back, None, "the jamb is one-sided");
        assert_eq!(line.flags & 1_u32, 1_u32, "blocking");
        let front = map
            .sidedefs
            .get(line.front.expect("a front side"))
            .expect("side");
        assert_eq!(front.middle, "DOORTRAK", "the track");
    }
}

#[test]
fn remote_doors_carry_a_fresh_tag_and_no_action() {
    let mut map = two_rooms();
    let spec = DoorSpec {
        style: DoorStyle::Remote,
        ..spec(Target::Between { a: 0, b: 1 })
    };
    let built = build_door(&mut map, &spec).expect("a remote door");
    assert_eq!(built.tag, Some(2_i32), "room 1's tag 1 was outgrown");
    let door = map.sectors.get(built.door_sector).expect("the door");
    assert_eq!(door.tag, 2_i32, "the sector carries the minted tag");
    assert_eq!(door.ceiling_height, door.floor_height, "closed and inert");
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a line");
        assert_eq!(line.special, 0_i32, "no action on the door's lines");
        assert_eq!(line.tag, 2_i32, "the Doom family pre-tags the use lines");
    }
}

#[test]
fn a_too_wide_door_names_the_available_length() {
    let mut map = two_rooms();
    let spec = DoorSpec {
        width: 300_i32,
        ..spec(Target::Between { a: 0, b: 1 })
    };
    let message = refuse(&build_door(&mut map, &spec).unwrap_err());
    assert!(message.contains("256 map units long"), "{message}");
    assert!(message.contains("300-unit door"), "{message}");
    assert_eq!(map.sectors.len(), 2, "nothing was drawn");
}

#[test]
fn a_one_sided_wall_with_nothing_behind_refuses() {
    let mut map = one_room();
    let message = refuse(&build_door(&mut map, &spec(Target::Linedef(0))).unwrap_err());
    assert_eq!(
        message,
        "no room behind this wall within 256 units; move the rooms closer or give a \
             larger depth",
        "the exact instruction"
    );
    assert_eq!(map.sectors.len(), 1, "nothing was drawn");
}

#[test]
fn an_explicit_depth_must_land_inside_a_sector() {
    let mut map = one_room();
    let spec = DoorSpec {
        depth: Some(400_i32),
        ..spec(Target::Linedef(0))
    };
    let message = refuse(&build_door(&mut map, &spec).unwrap_err());
    assert!(message.contains("no room behind this wall"), "{message}");
    assert_eq!(map.sectors.len(), 1, "nothing was drawn");
}

#[test]
fn a_one_sided_door_opens_a_passage_across_the_room() {
    let mut map = room_and_hall();
    let built = build_door(&mut map, &spec(Target::Linedef(0))).expect("the ray finds the hall");
    assert_eq!(built.lines.len(), 3, "the bay: the hall and both sides");
    assert_eq!(built.jambs.len(), 1, "the old west wall seals the bay");
    let door = map.sectors.get(built.door_sector).expect("the door");
    assert_eq!(door.ceiling_height, door.floor_height, "closed");
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a line");
        assert!(line.front.is_some() && line.back.is_some(), "two-sided");
        assert_eq!(line.special, 1_i32, "every opening carries the action");
    }
    assert_eq!(
        map.sectors.get(built.door_sector).expect("door").floor_flat,
        "FLAT10"
    );
}

#[test]
fn recess_depth_validates_the_explicit_number() {
    let map = one_room();
    let wall = door_wall(&map, 0, None, 64_i32).expect("the west wall carries 64");
    let mut probe = spec(Target::Linedef(0));
    probe.depth = Some(0_i32);
    assert!(
        refuse(&recess_depth(&map, &wall, &probe).unwrap_err()).contains("positive"),
        "zero is no depth"
    );
    probe.depth = Some(-8_i32);
    assert!(
        refuse(&recess_depth(&map, &wall, &probe).unwrap_err()).contains("positive"),
        "neither is a negative one"
    );
    probe.depth = Some(64_i32);
    assert_eq!(recess_depth(&map, &wall, &probe).expect("kept"), 64_i32);
    probe.depth = None;
    assert_eq!(
        code(&recess_depth(&map, &wall, &probe).unwrap_err()),
        "invalid_argument",
        "the ray finds nothing behind the room"
    );
}

#[test]
fn strip_wall_needs_a_one_sided_room_face() {
    let map = two_rooms();
    assert_eq!(
        strip_wall(&map, 9, Point { x: 0, y: 0 }),
        None,
        "sector 9's walls are nobody's"
    );
    let west = strip_wall(&map, 0, Point { x: 0, y: 128 }).expect("the hall's west wall");
    assert_eq!(west, 0_usize, "the one-sided face at the point");
    let east = strip_wall(&map, 1, Point { x: 256, y: 128 }).expect("room 1's far wall");
    assert_eq!(
        east, 4_usize,
        "the nearest one-sided face (two-sided step lines never count, and the \
             north/south ties go to the lowest index)"
    );
}

#[test]
fn the_strip_face_refusal_names_the_room() {
    let message = refuse(&strip_face_lost(3));
    assert!(message.contains("sector 3"), "{message}");
}

#[test]
fn an_empty_map_finds_no_groups() {
    let map = Map::default();
    let groups = door_groups(&map, 0_usize);
    assert!(groups.lines.is_empty(), "nothing borders");
    assert!(groups.jambs.is_empty(), "and nothing frames it");
}

#[test]
fn centroids_and_halves_stay_whole() {
    assert_eq!(
        centroid(&[
            Point { x: 0, y: 0 },
            Point { x: 3, y: 0 },
            Point { x: 3, y: 3 },
            Point { x: 0, y: 3 },
        ]),
        Point { x: 2, y: 2 },
        "the mean of the corners"
    );
    assert_eq!(half_axis(-1, 0), -1_i32, "halves round away from zero");
    assert_eq!(half_axis(0, 1), 1_i32, "up too");
    assert_eq!(half_axis(i32::MAX, i32::MAX), i32::MAX, "saturated");
}

#[test]
fn the_recess_record_copies_the_room_and_tags_remote() {
    let map = two_rooms();
    let placed = Placed {
        wall: 0_usize,
        room_a: 1_usize,
        rect: vec![Point { x: 0, y: 0 }],
        tag: Some(7_i32),
    };
    let record = door_record(&map, &placed, &spec(Target::Linedef(0)));
    assert_eq!(record.floor_height, 32_i32, "room 1's floor");
    assert_eq!(record.ceiling_height, 128_i32, "and ceiling");
    assert_eq!(record.tag, 7_i32, "the minted tag");
    assert_eq!(record.special, 0_i32, "no effect");
    assert_eq!(record.floor_flat, "FLAT10", "the theme's recess floor");
    let plain = door_record(
        &map,
        &Placed {
            tag: None,
            ..placed
        },
        &spec(Target::Linedef(0)),
    );
    assert_eq!(plain.tag, 0_i32, "untagged without a tag");
}

#[test]
fn dressing_an_emptied_recess_changes_nothing_more() {
    let mut map = two_rooms();
    let spec = DoorSpec {
        style: DoorStyle::Fast,
        ..spec(Target::Between { a: 0, b: 1 })
    };
    let built = build_door(&mut map, &spec).expect("a fast door");
    for index in &built.lines {
        assert_eq!(
            map.linedefs.get(*index).expect("line").special,
            117_i32,
            "the DR fast action"
        );
    }
    let before = map.clone();
    let dress = Dress {
        door: built.door_sector,
        groups: Groups {
            lines: Vec::new(),
            jambs: Vec::new(),
        },
        tag: 0_i32,
    };
    dress_door(&mut map, &spec, &dress).expect("an empty dressing is allowed");
    assert_eq!(map.sectors, before.sectors, "the door stays closed anyway");
}

#[test]
fn a_hexen_map_takes_the_script_door() {
    let mut map = two_rooms();
    map.format = MapFormat::Hexen;
    let built =
        build_door(&mut map, &spec(Target::Between { a: 0, b: 1 })).expect("a Hexen-family door");
    for index in &built.lines {
        let line = map.linedefs.get(*index).expect("a room line");
        assert_eq!(line.special, 12_i32, "Door_Raise");
        assert_eq!(line.args, [0_i32, 16_i32, 150_i32, 0_i32, 0_i32]);
        assert_ne!(line.flags & 0x400_u32, 0_u32, "playeruse");
        assert_ne!(line.flags & 0x2000_u32, 0_u32, "monsteractivate");
    }
}

#[test]
fn an_off_centre_door_projects_onto_the_wall() {
    let mut map = two_rooms();
    let chosen: Chosen =
        choose_between(&map, 0, 1, Some(Point { x: 300, y: 64 })).expect("near the south end");
    assert_eq!(
        chosen.centre,
        Point { x: 256, y: 64 },
        "projected onto the shared line"
    );
    let mut spec = spec(Target::Between { a: 0, b: 1 });
    spec.at = Some(Point { x: 300, y: 64 });
    let built = build_door(&mut map, &spec).expect("the off-centre door builds");
    assert_eq!(built.lines.len(), 2);
    assert_eq!(built.jambs.len(), 2);
}
