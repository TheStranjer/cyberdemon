//! The switch and exit tests: the action and kind names, the wall
//! split and panel dressing of [`build_switch`], the tag minting and
//! the walk-over boundary sweep of [`build_walk_over_exit`].

use serde_json::from_str;

use crate::themes::Role;
use crate::tools::features::switches::{
    AddExitParams, AddSwitchParams, ExitKind, action_names, action_of, kind_name, kind_of,
    switch_role,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::SwitchAction;
use crate::wad::map::features::switch_special;
use crate::wad::map::features::{
    SkippedLine, SwitchSpec, action_name, build_switch, build_walk_over_exit, dress_switch,
    is_level_end, isolate_chunk, switch_cut, switch_span, switch_tag, walk_over_special,
};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::ops::Point;

use super::{hall_and_two_rooms, missing, one_room, refuse, two_rooms};

/// A `add_switch` request parsed from `json`.
fn switch_args(json: &str) -> AddSwitchParams {
    from_str(json).expect("test switch requests parse")
}

/// A 32-unit open-door switch spec on linedef `linedef`.
fn switch_spec(linedef: usize) -> SwitchSpec {
    SwitchSpec {
        linedef,
        at: None,
        width: 32_i32,
        action: SwitchAction::OpenDoor,
        target_sector: Some(1_usize),
        repeatable: false,
        texture: "SW1STRTN".to_owned(),
    }
}

/// The coordinates of the vertex at [`Linedef`]'s `end` of `line`
/// (`"v1"` or `"v2"`).
fn line_vertex(map: &Map, line: usize, end: &str) -> Point {
    let def = map.linedefs.get(line).expect("the linedef");
    let index = if end == "v1" { def.v1 } else { def.v2 };
    let vertex = map.vertices.get(index).expect("the vertex");
    Point {
        x: vertex.x,
        y: vertex.y,
    }
}

/// The front sidedef's `middle` texture of linedef `line`.
fn front_middle(map: &Map, line: usize) -> String {
    let def = map.linedefs.get(line).expect("the linedef");
    let side = def.front.expect("a front side");
    map.sidedefs
        .get(side)
        .expect("the front sidedef")
        .middle
        .clone()
}

#[test]
fn switch_actions_round_trip_and_refuse_unknown_names() {
    for action in SwitchAction::all() {
        assert_eq!(action_of(action_name(action)).expect("canonical"), action);
        assert_eq!(
            action_of(&action_name(action).to_ascii_uppercase()).expect("case-insensitive"),
            action
        );
    }
    let message = refuse(&action_of("press").unwrap_err());
    assert!(
        message.contains("open_door, exit, secret_exit, raise_floor, lower_floor"),
        "the refusal lists every name: {message}"
    );
    assert_eq!(
        action_names().len(),
        SwitchAction::all().len(),
        "one name each"
    );
}

#[test]
fn exit_actions_wear_the_exit_panel() {
    assert_eq!(switch_role(SwitchAction::Exit), Role::ExitSwitch);
    assert_eq!(switch_role(SwitchAction::SecretExit), Role::ExitSwitch);
    for action in [
        SwitchAction::OpenDoor,
        SwitchAction::RaiseFloor,
        SwitchAction::LowerFloor,
    ] {
        assert_eq!(switch_role(action), Role::Switch, "{action:?}");
    }
}

#[test]
fn add_switch_params_read_the_wire_shape() {
    let args = switch_args(
        r#"{"linedef": 3, "action": "raise_floor", "target_sector": 1,
               "at": {"x": 64, "y": 0}, "width": 48, "repeatable": true}"#,
    );
    assert_eq!(args.linedef, 3_usize);
    assert_eq!(args.action, "raise_floor");
    assert_eq!(args.target_sector, Some(1_usize));
    assert_eq!(args.width, Some(48_i32));
    assert!(args.repeatable, "the flag arrives");
    let minimal = switch_args(r#"{"linedef": 0, "action": "exit"}"#);
    assert_eq!(minimal.at, None, "no centre asked");
    assert_eq!(minimal.width, None, "the default width stands");
    assert!(!minimal.repeatable, "one-shot by default");
    assert!(
        from_str::<AddSwitchParams>(r#"{"action": "exit"}"#).is_err(),
        "the wall is required"
    );
    assert!(
        from_str::<AddSwitchParams>(r#"{"linedef": 0}"#).is_err(),
        "the action is required"
    );
}

#[test]
fn a_switch_splits_the_wall_and_installs_the_action() {
    let mut map = two_rooms();
    let placed = build_switch(&mut map, &switch_spec(0)).expect("the west wall takes it");
    assert_eq!(placed.linedef, 7_usize, "the first split is the chunk");
    assert!(placed.split, "the long wall was cut");
    assert_eq!(placed.action, "open_door", "echoed canonically");
    assert_eq!(placed.special, 103_i32, "the S1 door action");
    assert_eq!(placed.tag, 1_i32, "sector 1's own tag");
    assert_eq!(placed.texture, "SW1STRTN", "what it wears");
    assert_eq!(map.linedefs.len(), 9_usize, "two outer pieces");
    assert_eq!(line_vertex(&map, 7, "v1"), Point { x: 0, y: 112 });
    assert_eq!(line_vertex(&map, 7, "v2"), Point { x: 0, y: 144 });
    let chunk = map.linedefs.get(7).expect("the chunk");
    assert_eq!(chunk.special, 103_i32);
    assert_eq!(chunk.tag, 1_i32, "the Doom line carries the tag");
    assert_eq!(chunk.args, [0_i32; 5]);
    assert_eq!(chunk.flags, 1_u32, "a Doom switch keeps the wall's flags");
    assert_eq!(front_middle(&map, 7), "SW1STRTN", "the panel on the front");
    assert_eq!(
        front_middle(&map, 0),
        "WALL1",
        "the remaining wall is plain"
    );
    assert_eq!(front_middle(&map, 8), "WALL1", "and so is the other piece");
}

#[test]
fn an_exact_line_takes_the_switch_without_splitting() {
    let mut map = two_rooms();
    let spec = SwitchSpec {
        width: 256_i32,
        action: SwitchAction::Exit,
        target_sector: None,
        texture: "SW1EXIT".to_owned(),
        ..switch_spec(0)
    };
    let placed = build_switch(&mut map, &spec).expect("the whole wall is the panel");
    assert_eq!(placed.linedef, 0_usize, "the line stands as it is");
    assert!(!placed.split, "nothing was cut");
    assert_eq!(placed.special, 11_i32, "the S1 exit");
    assert_eq!(placed.tag, 0_i32, "an exit acts on no tag");
    assert_eq!(map.linedefs.len(), 7_usize, "no linedef was appended");
    assert_eq!(front_middle(&map, 0), "SW1EXIT");
}

#[test]
fn an_untagged_target_sector_gets_a_fresh_tag() {
    let mut map = two_rooms();
    let spec = SwitchSpec {
        target_sector: Some(0_usize),
        ..switch_spec(0)
    };
    let placed = build_switch(&mut map, &spec).expect("sector 0 is untagged");
    assert_eq!(placed.tag, 2_i32, "room 1's tag 1 was outgrown");
    assert_eq!(
        map.sectors.first().expect("target").tag,
        2_i32,
        "the tag was stamped onto the sector"
    );
    assert_eq!(
        map.linedefs.get(7).expect("the chunk").tag,
        2_i32,
        "and answered by the line"
    );
}

#[test]
fn targeted_actions_demand_their_sector() {
    for action in [
        SwitchAction::OpenDoor,
        SwitchAction::RaiseFloor,
        SwitchAction::LowerFloor,
    ] {
        let mut map = two_rooms();
        let spec = SwitchSpec {
            action,
            target_sector: None,
            ..switch_spec(0)
        };
        let message = refuse(&build_switch(&mut map, &spec).unwrap_err());
        assert!(message.contains("target_sector"), "{action:?}: {message}");
    }
    let mut map = two_rooms();
    let spec = SwitchSpec {
        target_sector: Some(99_usize),
        ..switch_spec(0)
    };
    let message = missing(&build_switch(&mut map, &spec).unwrap_err());
    assert!(message.contains("sector 99 does not exist"), "{message}");
}

#[test]
fn exits_ignore_their_parameters() {
    let mut map = two_rooms();
    let spec = SwitchSpec {
        action: SwitchAction::SecretExit,
        target_sector: Some(1_usize),
        ..switch_spec(0)
    };
    let placed = build_switch(&mut map, &spec).expect("a secret exit switch");
    assert_eq!(placed.special, 51_i32, "the S1 secret exit");
    assert_eq!(placed.tag, 0_i32, "the given target is ignored");
    assert_eq!(map.sectors.get(1).expect("sector 1").tag, 1_i32);
}

#[test]
fn a_switch_names_the_wall_it_cannot_take() {
    let mut far_map = two_rooms();
    let far = missing(&build_switch(&mut far_map, &switch_spec(99)).unwrap_err());
    assert!(far.contains("linedef 99 does not exist"), "{far}");
    let mut broken_map = two_rooms();
    broken_map.linedefs.get_mut(0).expect("the wall").v1 = 99;
    let broken = refuse(&build_switch(&mut broken_map, &switch_spec(0)).unwrap_err());
    assert!(broken.contains("references a vertex"), "{broken}");
    let mut map = two_rooms();
    let wide = SwitchSpec {
        width: 300_i32,
        ..switch_spec(0)
    };
    let over = refuse(&build_switch(&mut map, &wide).unwrap_err());
    assert!(over.contains("256 map units long"), "{over}");
    let thin = SwitchSpec {
        width: 0_i32,
        ..switch_spec(0)
    };
    let none = refuse(&build_switch(&mut map, &thin).unwrap_err());
    assert!(none.contains("positive"), "{none}");
    assert_eq!(map.linedefs.len(), 7_usize, "no refusal cut anything");
}

#[test]
fn switch_cut_and_span_check_the_wall_before_the_door_engine() {
    let map = two_rooms();
    let message = missing(&switch_span(&map, 99).unwrap_err());
    assert!(
        message.contains("linedef 99 does not exist") && !message.contains("door"),
        "switch words: {message}"
    );
    let spec = switch_spec(2);
    let ((start, finish), available) = switch_cut(&map, &spec).expect("the step line");
    assert_eq!(
        start,
        Point { x: 256, y: 144 },
        "centred on the midpoint, running from v1 (256, 256) down"
    );
    assert_eq!(finish, Point { x: 256, y: 112 }, "toward v2");
    assert_eq!(available, 256_i32, "the wall's length");
}

#[test]
fn isolate_chunk_cuts_only_where_the_chunk_does_not_meet_an_endpoint() {
    let mut map = two_rooms();
    let flush = isolate_chunk(
        &mut map,
        0_usize,
        Point { x: 0, y: 0 },
        Point { x: 0, y: 64 },
    )
    .expect("a chunk flush with v1");
    assert_eq!(flush.line, 0_usize, "the original keeps the chunk");
    assert!(flush.split, "one cut at finish");
    assert_eq!(map.linedefs.len(), 8_usize, "only one piece was appended");
}

#[test]
fn a_frontless_line_refuses_the_panel() {
    let mut map = two_rooms();
    map.linedefs.get_mut(0).expect("the wall").front = None;
    let message = refuse(&build_switch(&mut map, &switch_spec(0)).unwrap_err());
    assert!(message.contains("no front sidedef"), "{message}");
}

#[test]
fn a_hexen_switch_acts_through_arguments_and_use_bits() {
    let mut map = two_rooms();
    map.format = MapFormat::Hexen;
    let placed = build_switch(&mut map, &switch_spec(0)).expect("the west wall takes it");
    assert_eq!(placed.special, 11_i32, "Door_Open");
    assert_eq!(placed.tag, 1_i32, "the tag reported is the sector's");
    let chunk = map.linedefs.get(7).expect("the chunk");
    assert_eq!(chunk.tag, 0_i32, "a Hexen line carries no tag");
    assert_eq!(
        chunk.args,
        [1_i32, 16_i32, 0_i32, 0_i32, 0_i32],
        "tag in args[0], speed 16 in args[1]"
    );
    assert_eq!(chunk.flags, 1_u32 | 0x400_u32, "the wall stands, playeruse");
    let mut again = two_rooms();
    again.format = MapFormat::Hexen;
    let spec = SwitchSpec {
        repeatable: true,
        ..switch_spec(0)
    };
    build_switch(&mut again, &spec).expect("a repeating switch");
    assert_eq!(
        again.linedefs.get(7).expect("the chunk").flags,
        1_u32 | 0x600_u32,
        "playeruse plus repeatspecial"
    );
}

#[test]
fn switch_tag_mints_only_for_tagged_actions() {
    let mut map = two_rooms();
    let exit = SwitchSpec {
        action: SwitchAction::Exit,
        target_sector: None,
        ..switch_spec(0)
    };
    assert_eq!(switch_tag(&mut map, &exit).expect("no tag needed"), 0_i32);
    assert_eq!(
        switch_tag(&mut map, &switch_spec(0)).expect("sector 1's own tag"),
        1_i32
    );
}

#[test]
fn dress_switch_refuses_a_line_the_map_does_not_hold() {
    let mut map = two_rooms();
    let setup = switch_special(FlagFamily::Doom, SwitchAction::OpenDoor, false, 1_i32);
    let message = refuse(&dress_switch(&mut map, 99, &setup, "SW1STRTN").unwrap_err());
    assert!(message.contains("linedef 99"), "{message}");
}

/// An `add_exit` request parsed from `json`.
fn exit_args(json: &str) -> AddExitParams {
    from_str(json).expect("test exit requests parse")
}

#[test]
fn exit_kinds_round_trip_and_refuse_unknown_names() {
    for kind in [ExitKind::Switch, ExitKind::WalkOver] {
        assert_eq!(kind_of(kind_name(kind)).expect("canonical"), kind);
        assert_eq!(
            kind_of(&kind_name(kind).to_ascii_uppercase()).expect("case-insensitive"),
            kind
        );
    }
    let message = refuse(&kind_of("stairs").unwrap_err());
    assert!(
        message.contains("\"switch\"") && message.contains("\"walk_over\""),
        "the refusal names both triggers: {message}"
    );
}

#[test]
fn add_exit_params_read_the_wire_shape() {
    let mounted = exit_args(r#"{"kind": "switch", "linedef": 3, "secret": true}"#);
    assert_eq!(mounted.linedef, Some(3_usize));
    assert!(mounted.secret, "the flag arrives");
    assert_eq!(mounted.sector, None, "walk_over's selector stands aside");
    assert_eq!(mounted.map, None, "the map defaults like the other tools");
    let walked = exit_args(r#"{"kind": "walk_over", "sector": 1}"#);
    assert_eq!(walked.sector, Some(1_usize));
    assert!(!walked.secret, "the normal exit by default");
    assert!(
        from_str::<AddExitParams>(r#"{"sector": 1}"#).is_err(),
        "the kind is required"
    );
}

#[test]
fn walk_over_specials_follow_the_family_and_secrecy() {
    assert_eq!(walk_over_special(FlagFamily::Doom, false), 52_i32);
    assert_eq!(walk_over_special(FlagFamily::Doom, true), 124_i32);
    assert_eq!(walk_over_special(FlagFamily::Hexen, false), 243_i32);
    assert_eq!(walk_over_special(FlagFamily::Hexen, true), 244_i32);
}

#[test]
fn a_walk_over_exit_dresses_every_boundary_of_the_sector() {
    let mut map = two_rooms();
    if let Some(line) = map.linedefs.get_mut(2) {
        line.tag = 3_i32;
    }
    let built = build_walk_over_exit(&mut map, 0_usize, false).expect("sector 0's boundary");
    assert_eq!(built.lines, vec![2_usize], "the one shared two-sided line");
    assert_eq!(built.special, 52_i32, "the W1 exit");
    let line = map.linedefs.get(2).expect("the boundary");
    assert_eq!(line.special, 52_i32, "the line carries it");
    assert_eq!(line.tag, 3_i32, "the vanilla destination tag survives");
    assert_eq!(line.flags, 4_u32, "a Doom line's flags stay as they were");
    let from_behind =
        build_walk_over_exit(&mut map, 1_usize, true).expect("sector 1 shares the line");
    assert_eq!(from_behind.lines, vec![2_usize], "either side counts");
    assert_eq!(from_behind.special, 124_i32, "the secret W1");
}

#[test]
fn a_hexen_walk_over_exit_sets_the_cross_activation() {
    let mut map = two_rooms();
    map.format = MapFormat::Hexen;
    if let Some(line) = map.linedefs.get_mut(2) {
        line.flags = 4_u32 | 0x400_u32 | 0x200_u32;
    }
    let built = build_walk_over_exit(&mut map, 0_usize, true).expect("a Hexen exit");
    assert_eq!(built.special, 244_i32, "the secret Portal");
    let line = map.linedefs.get(2).expect("the boundary");
    assert_eq!(line.special, 244_i32);
    assert_eq!(
        line.flags,
        4_u32 | 0x200_u32,
        "the activation field clears to playercross (its zero value); twosided and \
             repeatspecial keep their bits"
    );
}

#[test]
fn walk_over_exits_refuse_bad_sectors_and_bare_rooms() {
    let mut map = two_rooms();
    let gone = missing(&build_walk_over_exit(&mut map, 99_usize, false).unwrap_err());
    assert!(gone.contains("sector 99 does not exist"), "{gone}");
    let mut lonely = one_room();
    let bare = refuse(&build_walk_over_exit(&mut lonely, 0_usize, false).unwrap_err());
    assert!(
        bare.contains("sector 0 has no two-sided boundary"),
        "{bare}"
    );
    assert_eq!(lonely.linedefs.len(), 4, "a refusal wrote nothing");
    assert_eq!(lonely.sectors.len(), 1, "and drew nothing");
}

#[test]
fn one_sided_lines_never_take_the_walk_over_action() {
    let mut map = two_rooms();
    let solid = 6_usize;
    let built = build_walk_over_exit(&mut map, 1_usize, false).expect("room 1's boundary");
    assert_eq!(
        built.lines,
        vec![2_usize],
        "the hall-facing step line only, not room 1's own walls"
    );
    assert_eq!(map.linedefs.get(solid).expect("a wall").special, 0_i32);
}

/// Give linedef `index` the action `special` answering `tag`: how a door,
/// a lift or a teleporter line looks to the exit step.
fn dress_with_action(map: &mut Map, index: usize, special: i32, tag: i32) {
    if let Some(line) = map.linedefs.get_mut(index) {
        line.special = special;
        line.tag = tag;
    }
}

/// The special, tag and activation of linedef `index`.
fn line_action(map: &Map, index: usize) -> (i32, i32, u32) {
    let line = map.linedefs.get(index).expect("the linedef");
    (line.special, line.tag, line.flags)
}

#[test]
fn is_level_end_reads_the_familys_own_walk_over_ends() {
    for special in [52_i32, 124_i32] {
        assert!(
            is_level_end(FlagFamily::Doom, special),
            "the Doom {special}"
        );
        assert!(
            !is_level_end(FlagFamily::Hexen, special),
            "{special} is the other family's"
        );
    }
    for special in [243_i32, 244_i32] {
        assert!(
            is_level_end(FlagFamily::Hexen, special),
            "the Hexen {special}"
        );
        assert!(
            !is_level_end(FlagFamily::Doom, special),
            "{special} is the other family's"
        );
    }
    for special in [0_i32, 1_i32, 26_i32, 62_i32, 97_i32, 103_i32] {
        assert!(
            !is_level_end(FlagFamily::Doom, special),
            "{special} is another feature's"
        );
        assert!(
            !is_level_end(FlagFamily::Hexen, special),
            "{special} is another feature's"
        );
    }
}

#[test]
fn a_walk_over_exit_leaves_a_door_line_its_action() {
    let mut map = hall_and_two_rooms();
    dress_with_action(&mut map, 2_usize, 1_i32, 1_i32);
    let built = build_walk_over_exit(&mut map, 0_usize, false).expect("the hall's plain side");
    assert_eq!(
        built.lines,
        vec![6_usize],
        "only the plain boundary ends the level"
    );
    assert_eq!(built.special, 52_i32, "the W1 exit");
    assert_eq!(
        built.skipped_lines,
        vec![SkippedLine {
            line: 2_usize,
            special: 1_i32
        }],
        "the door line is reported with the action it kept"
    );
    assert_eq!(
        line_action(&map, 2),
        (1_i32, 1_i32, 4_u32),
        "the door still opens on its own tag"
    );
    assert_eq!(
        line_action(&map, 6).0,
        52_i32,
        "the plain line took the exit"
    );
    let note = built.warning().expect("skipped lines are warned about");
    assert!(note.contains("one boundary line"), "{note}");
    assert!(note.contains("line 2 (special 1)"), "{note}");
    assert!(note.contains("does not end the level"), "{note}");
}

#[test]
fn a_walk_over_exit_warns_only_when_it_skipped_something() {
    let mut plain = hall_and_two_rooms();
    let dressed = build_walk_over_exit(&mut plain, 0_usize, false).expect("both lines are plain");
    assert_eq!(dressed.lines, vec![2_usize, 6_usize], "both boundaries");
    assert!(dressed.skipped_lines.is_empty(), "nothing stood aside");
    assert_eq!(dressed.warning(), None, "so there is nothing to say");
    let mut busy = hall_and_two_rooms();
    dress_with_action(&mut busy, 2_usize, 26_i32, 1_i32);
    dress_with_action(&mut busy, 6_usize, 62_i32, 2_i32);
    let one = build_walk_over_exit(&mut busy, 0_usize, false);
    assert!(
        one.is_err(),
        "a hall of only busy lines ends the level nowhere"
    );
}

#[test]
fn an_exit_redresses_the_line_that_already_ends_the_level() {
    let mut map = hall_and_two_rooms();
    dress_with_action(&mut map, 2_usize, 1_i32, 1_i32);
    let hall = build_walk_over_exit(&mut map, 0_usize, false).expect("the plain side stands");
    assert_eq!(hall.lines, vec![6_usize]);
    let from_b = build_walk_over_exit(&mut map, 2_usize, true).expect("room 2 shares line 6");
    assert_eq!(
        from_b.lines,
        vec![6_usize],
        "an exit already on the line is the exit's own to move"
    );
    assert!(
        from_b.skipped_lines.is_empty(),
        "the level end it carries is no reason to skip"
    );
    assert_eq!(from_b.special, 124_i32, "the same threshold, now secret");
    assert_eq!(line_action(&map, 6), (124_i32, 0_i32, 4_u32), "re-dressed");
}

#[test]
fn a_sector_of_only_busy_boundaries_is_refused_by_name() {
    let mut map = hall_and_two_rooms();
    dress_with_action(&mut map, 2_usize, 26_i32, 1_i32);
    let message = refuse(&build_walk_over_exit(&mut map, 1_usize, false).unwrap_err());
    assert!(
        message.contains("sector 1 has no plain two-sided boundary"),
        "{message}"
    );
    assert!(message.contains("line 2 (special 26)"), "{message}");
    assert!(message.contains("switch"), "{message}");
    assert_eq!(
        line_action(&map, 2),
        (26_i32, 1_i32, 4_u32),
        "the locked door the refusal refused to touch"
    );
    assert_eq!(
        map.linedefs.get(6).expect("the other line").special,
        0_i32,
        "and nothing else was written"
    );
}

#[test]
fn a_hexen_walk_over_exit_leaves_a_scripted_line_alone() {
    let mut map = hall_and_two_rooms();
    map.format = MapFormat::Hexen;
    dress_with_action(&mut map, 2_usize, 12_i32, 0_i32);
    if let Some(line) = map.linedefs.get_mut(6) {
        line.flags = 4_u32 | 0x400_u32;
    }
    let built = build_walk_over_exit(&mut map, 0_usize, false).expect("the plain side stands");
    assert_eq!(built.special, 243_i32, "the Portal exit");
    assert_eq!(built.lines, vec![6_usize]);
    assert_eq!(
        built.skipped_lines,
        vec![SkippedLine {
            line: 2_usize,
            special: 12_i32
        }],
        "Door_Raise keeps its line"
    );
    assert_eq!(line_action(&map, 2), (12_i32, 0_i32, 4_u32));
    assert_eq!(
        line_action(&map, 6),
        (243_i32, 0_i32, 4_u32),
        "the dressed line clears to playercross"
    );
}
