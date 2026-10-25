//! Tests for the lift core: the [`lift_specials`] tables for both flag
//! families, [`build_lift`] on an existing platform and on a drawn one,
//! the edge probe, the refusals and the small-drop note.

use super::build::{activation_edges, build_lift, polygon_centre};
use super::{LiftPlatform, LiftSpec, LiftStyle, MIN_LIFT_DROP, lift_specials};
use crate::error::CyberError;
use crate::wad::map::common::fixtures::{name, room, side, solid_line, two_room_map, vertex};
use crate::wad::map::features::DoorLineSetup;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::{Map, MapFormat};

/// The tag the table tests ask `lift_specials` for.
const SAMPLE_TAG: i32 = 42;

/// The error code of `error`, for compact asserts.
fn code(error: &CyberError) -> &str {
    error.code()
}

/// The message of a refusal that must be an `InvalidArgument`.
fn refuse(error: &CyberError) -> String {
    assert_eq!(code(error), "invalid_argument", "refusal code");
    error.to_string()
}

/// A sidedef over `sector` wearing the fixture textures.
fn sector_side(sector: usize) -> crate::wad::map::Sidedef {
    side("-", "-", "WALL1", sector)
}

/// A single-room square: no neighbour on any side.
fn one_room() -> Map {
    Map {
        name: name("MAP01"),
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
        sidedefs: vec![
            sector_side(0),
            sector_side(0),
            sector_side(0),
            sector_side(0),
        ],
        sectors: vec![room(0, 128, 160, 0)],
        ..Default::default()
    }
}

/// A use-wall, repeatable lift spec over `platform`.
fn spec(platform: LiftPlatform) -> LiftSpec {
    LiftSpec {
        platform,
        low_floor: None,
        style: LiftStyle::UseWall,
        repeatable: true,
        side: "LIFTSIDE".to_owned(),
        floor_flat: "FLAT10".to_owned(),
    }
}

#[test]
fn lift_specials_matches_the_doom_table() {
    let expected = [
        (LiftStyle::UseWall, false, 21_i32),
        (LiftStyle::UseWall, true, 62_i32),
        (LiftStyle::WalkTrigger, false, 10_i32),
        (LiftStyle::WalkTrigger, true, 88_i32),
    ];
    assert_eq!(
        expected.len(),
        LiftStyle::all().len() * 2_usize,
        "the table covers every style and repeat"
    );
    for (style, repeatable, special) in expected {
        assert_eq!(
            lift_specials(FlagFamily::Doom, style, repeatable, SAMPLE_TAG),
            DoorLineSetup {
                special,
                tag: SAMPLE_TAG,
                args: [0_i32; 5],
                flags_activation: 0_u32,
            },
            "the Doom row of {style:?} repeatable={repeatable}"
        );
    }
}

#[test]
fn the_serde_names_of_the_styles_are_snake_case() {
    for (style, name) in [
        (LiftStyle::UseWall, "use_wall"),
        (LiftStyle::WalkTrigger, "walk_trigger"),
    ] {
        let written = serde_json::to_value(style).expect("serialising a style");
        assert_eq!(
            written,
            serde_json::Value::String(name.to_owned()),
            "{style:?}"
        );
        let again: LiftStyle = serde_json::from_value(written).expect("re-reading the style");
        assert_eq!(again, style, "the name reads back");
    }
}

#[test]
fn lift_specials_matches_the_hexen_table() {
    let args = [SAMPLE_TAG, 32_i32, 105_i32, 0_i32, 0_i32];
    let expected = [
        (LiftStyle::UseWall, false, 0x400_u32),
        (LiftStyle::UseWall, true, 0x600_u32),
        (LiftStyle::WalkTrigger, false, 0_u32),
        (LiftStyle::WalkTrigger, true, 0x200_u32),
    ];
    for (style, repeatable, activation) in expected {
        assert_eq!(
            lift_specials(FlagFamily::Hexen, style, repeatable, SAMPLE_TAG),
            DoorLineSetup {
                special: 206_i32,
                tag: 0_i32,
                args,
                flags_activation: activation,
            },
            "the Hexen row of {style:?} repeatable={repeatable}"
        );
    }
}

#[test]
fn the_fixture_step_becomes_a_repeating_use_lift() {
    let mut map = two_room_map();
    let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
    assert_eq!(
        build.platform_sector, 1_usize,
        "the raised room is the platform"
    );
    assert_eq!(build.tag, 2_i32, "the fresh tag");
    assert_eq!(build.lines, vec![2_usize], "the step line only");
    assert_eq!(
        (build.floor_height, build.low_floor, build.drop),
        (32_i32, 0_i32, 32_i32),
        "a 32-unit ride"
    );
    assert!(
        build
            .notes
            .iter()
            .any(|note| note.contains("a lift is not needed; stairs or a step will do")),
        "the small-drop note: {:?}",
        build.notes
    );
    let platform = map.sectors.get(1).expect("the platform");
    assert_eq!(platform.tag, 2_i32, "the fresh tag is stamped");
    assert_eq!(platform.floor_flat, "FLAT10", "the theme's lift floor");
    let line = map.linedefs.get(2).expect("the step line");
    assert_eq!(line.special, 62_i32, "the SR lift");
    assert_eq!(line.tag, 2_i32, "the line answers with the platform tag");
    assert_eq!(line.args, [0_i32; 5], "Doom lines carry no args");
    assert_eq!(line.flags, 4_u32, "two-sided and reachable");
    assert_eq!(
        map.side_sector(line.front),
        Some(0_usize),
        "the front already faced the lower hall"
    );
    assert_eq!(
        map.sidedefs.get(2).expect("the low side").lower,
        "LIFTSIDE",
        "the platform's face below the ledge"
    );
}

#[test]
fn the_other_doom_rows_land_on_their_specials() {
    for (style, repeatable, special) in [
        (LiftStyle::UseWall, false, 21_i32),
        (LiftStyle::WalkTrigger, false, 10_i32),
        (LiftStyle::WalkTrigger, true, 88_i32),
    ] {
        let mut map = two_room_map();
        let wanted = LiftSpec {
            style,
            repeatable,
            ..spec(LiftPlatform::Existing(1_usize))
        };
        let build = build_lift(&mut map, &wanted).expect("a Doom lift");
        assert_eq!(
            map.linedefs.get(2).expect("the step line").special,
            special,
            "{style:?} repeatable={repeatable}"
        );
        assert_eq!(build.tag, 2_i32, "the same fresh tag every time");
    }
}

#[test]
fn a_reversed_edge_turns_to_face_the_lower_sector() {
    let mut map = two_room_map();
    if let Some(line) = map.linedefs.get_mut(2) {
        line.v1 = 1_usize;
        line.v2 = 4_usize;
        line.front = Some(3_usize);
        line.back = Some(2_usize);
    }
    let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
    assert_eq!(build.lines, vec![2_usize]);
    let line = map.linedefs.get(2).expect("the step line");
    assert_eq!(line.v1, 4_usize, "the line turned around with its sides");
    assert_eq!(line.v2, 1_usize);
    assert_eq!(
        map.side_sector(line.front),
        Some(0_usize),
        "the front now faces the lower hall"
    );
    assert_eq!(map.side_sector(line.back), Some(1_usize));
    assert_eq!(line.special, 62_i32);
    assert_eq!(map.sidedefs.get(2).expect("the low side").lower, "LIFTSIDE");
}

#[test]
fn a_hexen_platform_runs_the_scripted_lip_plat() {
    let mut map = two_room_map();
    map.format = MapFormat::Hexen;
    let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
    assert_eq!(build.tag, 2_i32, "the minted tag reaches args[0]");
    let line = map.linedefs.get(2).expect("the step line");
    assert_eq!(line.special, 206_i32, "Plat_DownWaitUpStayLip");
    assert_eq!(
        line.args,
        [2_i32, 32_i32, 105_i32, 0_i32, 0_i32],
        "tag, speed, wait"
    );
    assert_eq!(line.tag, 0_i32, "a Hexen line carries no tag");
    assert_eq!(
        line.flags,
        4_u32 | 0x600_u32,
        "playeruse plus repeatspecial, two-sided"
    );
}

#[test]
fn a_hexen_walk_trigger_clears_the_activation_field() {
    let mut map = two_room_map();
    map.format = MapFormat::Hexen;
    if let Some(line) = map.linedefs.get_mut(2) {
        line.flags = 4_u32 | 0x1000_u32;
    }
    let wanted = LiftSpec {
        style: LiftStyle::WalkTrigger,
        repeatable: false,
        ..spec(LiftPlatform::Existing(1_usize))
    };
    build_lift(&mut map, &wanted).expect("a cross lift");
    let line = map.linedefs.get(2).expect("the step line");
    assert_eq!(
        line.flags, 4_u32,
        "the playerpush field clears to playercross (its zero value), twosided stays"
    );
    assert_eq!(line.special, 206_i32);
    let repeating = LiftSpec {
        repeatable: true,
        ..wanted
    };
    build_lift(&mut map, &repeating).expect("a repeating cross lift");
    assert_eq!(
        map.linedefs.get(2).expect("the step line").flags,
        4_u32 | 0x200_u32,
        "repeatable adds repeatspecial on top"
    );
}

#[test]
fn the_explicit_low_floor_moves_the_drop_and_the_warning() {
    for (low_floor, warns) in [(-32_i32, false), (-31_i32, true)] {
        let mut map = two_room_map();
        let wanted = LiftSpec {
            low_floor: Some(low_floor),
            ..spec(LiftPlatform::Existing(1_usize))
        };
        let build = build_lift(&mut map, &wanted).expect("an explicit ride");
        assert_eq!(
            (build.low_floor, build.drop),
            (low_floor, 32_i32 - low_floor)
        );
        let small = build
            .notes
            .iter()
            .any(|note| note.contains("a lift is not needed"));
        assert_eq!(small, warns, "the warning at drop {}", 32_i32 - low_floor);
    }
    assert_eq!(MIN_LIFT_DROP, 64_i32, "the guide's threshold");
}

#[test]
fn a_platform_that_is_not_the_high_floor_refuses() {
    let mut map = two_room_map();
    let before = map.clone();
    let message =
        refuse(&build_lift(&mut map, &spec(LiftPlatform::Existing(0_usize))).unwrap_err());
    assert!(message.contains("sector 0 stands at floor 0"), "{message}");
    assert!(message.contains("not above its lift floor 32"), "{message}");
    assert_eq!(map, before, "the refusal wrote nothing");
}

#[test]
fn an_isolated_platform_names_its_missing_neighbour() {
    let mut lonely = one_room();
    let before = lonely.clone();
    let missing =
        refuse(&build_lift(&mut lonely, &spec(LiftPlatform::Existing(0_usize))).unwrap_err());
    assert!(missing.contains("no neighbouring sector"), "{missing}");
    let wanted = LiftSpec {
        low_floor: Some(-64_i32),
        ..spec(LiftPlatform::Existing(0_usize))
    };
    let bare = refuse(&build_lift(&mut lonely, &wanted).unwrap_err());
    assert!(bare.contains("no edge of sector 0 faces a lower"), "{bare}");
    assert_eq!(lonely, before, "both refusals wrote nothing");
}

#[test]
fn an_out_of_range_platform_refuses_with_the_current_count() {
    let mut map = two_room_map();
    let message =
        refuse(&build_lift(&mut map, &spec(LiftPlatform::Existing(99_usize))).unwrap_err());
    assert!(message.contains("sector 99 is out of range"), "{message}");
    assert!(message.contains("holds 2 of them"), "{message}");
    assert_eq!(map, two_room_map(), "the refusal wrote nothing");
}

#[test]
fn the_centre_of_a_platform_polygon_is_the_corner_average() {
    assert_eq!(
        polygon_centre(&[(0, 0), (64, 0), (64, 64), (0, 64)]),
        (32_i64, 32_i64),
        "the rectangle's middle"
    );
}

#[test]
fn a_drawn_platform_straddles_the_step() {
    let mut map = two_room_map();
    let wanted = spec(LiftPlatform::Draw(vec![
        (224_i64, 64_i64),
        (320_i64, 64_i64),
        (320_i64, 192_i64),
        (224_i64, 192_i64),
    ]));
    let build = build_lift(&mut map, &wanted).expect("a drawn ledge");
    assert_eq!(build.platform_sector, 2_usize, "the third sector");
    assert_eq!(
        (build.floor_height, build.low_floor, build.drop),
        (32_i32, 0_i32, 32_i32)
    );
    assert!(!build.lines.is_empty(), "the hall-facing edges activate");
    assert!(
        build
            .notes
            .iter()
            .any(|note| note.contains("a lift is not needed")),
        "the 32-unit drop warns: {:?}",
        build.notes
    );
    let platform = map.sectors.get(2).expect("the platform");
    assert_eq!(
        (platform.floor_height, platform.ceiling_height),
        (32_i32, 128_i32)
    );
    assert_eq!(platform.tag, build.tag, "stamped with the fresh tag");
    assert_eq!(platform.floor_flat, "FLAT10", "the theme's lift floor");
    for index in &build.lines {
        let line = map.linedefs.get(*index).expect("an activation line");
        assert_eq!(
            map.side_sector(line.front),
            Some(0_usize),
            "edge {index} faces the lower hall"
        );
        assert_eq!(
            map.side_sector(line.back),
            Some(2_usize),
            "and the platform behind"
        );
        assert_eq!(line.special, 62_i32, "the SR lift on edge {index}");
        assert_eq!(line.tag, build.tag, "the shared tag on edge {index}");
        let front = map
            .sidedefs
            .get(line.front.expect("a front side"))
            .expect("side");
        assert_eq!(front.lower, "LIFTSIDE", "the lift's face on edge {index}");
    }
}

#[test]
fn a_small_or_misplaced_platform_polygon_refuses() {
    let mut thin = two_room_map();
    let before = thin.clone();
    let tiny = refuse(
        &build_lift(
            &mut thin,
            &spec(LiftPlatform::Draw(vec![
                (224_i64, 64_i64),
                (320_i64, 64_i64),
            ])),
        )
        .unwrap_err(),
    );
    assert!(tiny.contains("at least three corners"), "{tiny}");

    let mut void = two_room_map();
    let lost = refuse(
        &build_lift(
            &mut void,
            &spec(LiftPlatform::Draw(vec![
                (1000_i64, 1000_i64),
                (1064_i64, 1000_i64),
                (1064_i64, 1064_i64),
                (1000_i64, 1064_i64),
            ])),
        )
        .unwrap_err(),
    );
    assert!(
        lost.contains("centre 1032, 1032 lies in no sector"),
        "{lost}"
    );

    let mut flat = two_room_map();
    let flush = refuse(
        &build_lift(
            &mut flat,
            &spec(LiftPlatform::Draw(vec![
                (64_i64, 64_i64),
                (192_i64, 64_i64),
                (192_i64, 192_i64),
                (64_i64, 192_i64),
            ])),
        )
        .unwrap_err(),
    );
    assert!(flush.contains("not above its lift floor"), "{flush}");
    assert_eq!(thin, before, "the short refusal wrote nothing");
}

#[test]
fn the_edge_probe_skips_walls_that_are_not_lower_edges() {
    let map = two_room_map();
    assert_eq!(
        activation_edges(&map, 0_usize, 0_i32).len(),
        0_usize,
        "nothing is under the hall floor"
    );
    let edges = activation_edges(&map, 1_usize, 32_i32);
    assert_eq!(edges.len(), 1_usize, "the step line only");
    assert_eq!(edges.first().expect("the edge").line, 2_usize);
}
