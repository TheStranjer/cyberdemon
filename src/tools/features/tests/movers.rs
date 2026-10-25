//! The movers' tests: the stairs spec and its run over the fixture
//! map, the lift styles and platform choice, and the teleporter spec
//! that demands both points.

use serde_json::from_str;

use crate::tools::features::lift::{
    AddLiftParams, lift_platform_of, lift_style_name, lift_style_names, lift_style_of,
};
use crate::tools::features::stairs::{AddStairsParams, stair_spec};
use crate::tools::features::teleporter::{
    AddTeleporterParams, TeleportDestinationParam, TeleportSkins, teleport_spec,
};
use crate::tools::maps::params::PointParam;
use crate::wad::map::features::{
    DEFAULT_PAD_SIZE, LiftPlatform, LiftStyle, StairSpec, build_stairs,
};
use crate::wad::map::ops::Point;

use super::{refuse, two_rooms};

/// An `add_stairs` request parsed from `json`.
fn stairs_args(json: &str) -> AddStairsParams {
    from_str(json).expect("test stairs requests parse")
}

/// An `add_lift` request parsed from `json`.
fn lift_args(json: &str) -> AddLiftParams {
    from_str(json).expect("test lift requests parse")
}

/// An `add_teleporter` request parsed from `json`.
fn teleport_args(json: &str) -> AddTeleporterParams {
    from_str(json).expect("test teleporter requests parse")
}

/// The skins the tests dress teleporters with.
fn pad_skins() -> TeleportSkins {
    TeleportSkins {
        floor_flat: "GATE1".to_owned(),
        wall: "WALL1".to_owned(),
    }
}

#[test]
fn add_stairs_params_read_the_wire_shape() {
    let full = stairs_args(
        r#"{"from": {"x": 64, "y": 128}, "to": {"x": 448, "y": 128},
                "width": 96, "step_height": 12, "step_depth": 48,
                "floor_from": 0, "floor_to": 64, "map": "MAP01"}"#,
    );
    assert_eq!(full.from, PointParam { x: 64, y: 128 });
    assert_eq!(full.to, PointParam { x: 448, y: 128 });
    assert_eq!(full.width, Some(96_i32));
    assert_eq!(full.step_height, Some(12_i32));
    assert_eq!(full.step_depth, Some(48_i32));
    assert_eq!(full.floor_from, Some(0_i32));
    assert_eq!(full.floor_to, Some(64_i32));
    assert_eq!(full.map.as_deref(), Some("MAP01"));
    let minimal = stairs_args(r#"{"from": {"x": 0, "y": 0}, "to": {"x": 128, "y": 0}}"#);
    assert!(
        minimal.width.is_none()
            && minimal.step_height.is_none()
            && minimal.step_depth.is_none()
            && minimal.floor_from.is_none()
            && minimal.floor_to.is_none()
            && minimal.map.is_none(),
        "the defaults stand aside"
    );
    assert!(
        from_str::<AddStairsParams>(r#"{"to": {"x": 1, "y": 1}}"#).is_err(),
        "from is required"
    );
    assert!(
        from_str::<AddStairsParams>(r#"{"from": {"x": 1, "y": 1}}"#).is_err(),
        "to is required"
    );
}

#[test]
fn stair_spec_defaults_fill_the_wire_gaps() {
    let plain = stairs_args(r#"{"from": {"x": 64, "y": 128}, "to": {"x": 448, "y": 128}}"#);
    let filled = stair_spec(&plain);
    assert_eq!(filled.width, 64_i32, "the documented width default");
    assert_eq!(filled.step_height, 16_i32, "and step height");
    assert_eq!(
        (
            filled.step_depth,
            filled.floor_from,
            filled.floor_to,
            filled.from,
            filled.to
        ),
        (
            None,
            None,
            None,
            Point { x: 64, y: 128 },
            Point { x: 448, y: 128 }
        ),
        "the open choices stay open for the pure engine"
    );
    let tuned = stairs_args(
        r#"{"from": {"x": 0, "y": 0}, "to": {"x": 96, "y": 0},
                "width": 32, "step_height": 8}"#,
    );
    let given = stair_spec(&tuned);
    assert_eq!((given.width, given.step_height), (32_i32, 8_i32), "kept");
}

#[test]
fn the_stair_engine_draws_on_the_door_fixture_map() {
    let mut map = two_rooms();
    let stairs = StairSpec {
        from: Point { x: 64, y: 128 },
        to: Point { x: 448, y: 128 },
        width: 64_i32,
        step_height: 16_i32,
        step_depth: None,
        floor_from: None,
        floor_to: None,
    };
    let built = build_stairs(&mut map, &stairs, "STEP1").expect("0 to 32 in two steps");
    assert_eq!(built.floor_from, 0_i32, "probed from the hall");
    assert_eq!(built.floor_to, 32_i32, "and from the raised room");
    assert_eq!(built.steps.len(), 2_usize, "two 16-unit steps");
    assert_eq!(map.sectors.len(), 4_usize, "two rooms and two new steps");
}

#[test]
fn lift_styles_round_trip_and_refuse_unknown_names() {
    for style in LiftStyle::all() {
        assert_eq!(
            lift_style_of(lift_style_name(style)).expect("canonical"),
            style
        );
        assert_eq!(
            lift_style_of(&lift_style_name(style).to_ascii_uppercase()).expect("case-insensitive"),
            style
        );
    }
    let message = refuse(&lift_style_of("press").unwrap_err());
    assert!(
        message.contains("use_wall, walk_trigger"),
        "the refusal lists both names: {message}"
    );
    assert_eq!(
        lift_style_names().len(),
        LiftStyle::all().len(),
        "one name each"
    );
}

#[test]
fn add_lift_params_read_the_wire_shape() {
    let drawn = lift_args(
        r#"{"platform": {"points": [{"x": 0, "y": 0}, {"x": 64, "y": 0}, {"x": 64, "y": 64}]},
                "low_floor": -32, "style": "WALK_TRIGGER", "repeatable": false, "map": "MAP01"}"#,
    );
    assert_eq!(drawn.low_floor, Some(-32_i32), "the ride floor arrives");
    assert_eq!(
        drawn.style.as_deref(),
        Some("WALK_TRIGGER"),
        "as given text"
    );
    assert!(!drawn.repeatable, "the flag arrives");
    assert_eq!(drawn.map.as_deref(), Some("MAP01"));
    let shape = lift_platform_of(drawn.platform.as_ref(), drawn.sector).expect("a drawn one");
    assert_eq!(
        shape,
        LiftPlatform::Draw(vec![(0_i64, 0_i64), (64_i64, 0_i64), (64_i64, 64_i64)]),
        "the polygon widens to the arrangement grid"
    );
    let existing = lift_args(r#"{"sector": 3}"#);
    assert_eq!(
        lift_platform_of(existing.platform.as_ref(), existing.sector).expect("a sector"),
        LiftPlatform::Existing(3_usize),
        "the selector passes through"
    );
    assert!(existing.repeatable, "a lift repeats unless told otherwise");
    assert!(existing.style.is_none(), "the default style stands aside");
    assert!(existing.platform.is_none(), "and the drawing selector");
}

#[test]
fn exactly_one_of_platform_and_sector_resolves() {
    let both = lift_args(r#"{"platform": {"points": []}, "sector": 1}"#);
    let message = refuse(&lift_platform_of(both.platform.as_ref(), both.sector).unwrap_err());
    assert!(message.contains("exactly one"), "{message}");
    assert!(
        message.contains("new platform to draw"),
        "the refusal explains both: {message}"
    );
    let neither = lift_args("{}");
    assert!(
        refuse(&lift_platform_of(neither.platform.as_ref(), neither.sector).unwrap_err())
            .contains("exactly one"),
        "neither is refused too"
    );
    assert!(
        from_str::<AddLiftParams>(r#"{"platform": {}}"#).is_err(),
        "the polygon needs its points"
    );
}

#[test]
fn add_teleporter_params_read_the_wire_shape() {
    let full = teleport_args(
        r#"{"pad": {"x": 64, "y": 64}, "destination": {"x": 384, "y": 64, "angle": 90},
                "pad_size": 128, "two_way": true, "map": "MAP01"}"#,
    );
    assert_eq!(full.pad, Some(PointParam { x: 64, y: 64 }));
    assert_eq!(
        full.destination,
        Some(TeleportDestinationParam {
            x: 384,
            y: 64,
            angle: 90_i32,
        })
    );
    assert_eq!(full.pad_size, Some(128_i32), "the side arrives");
    assert!(full.two_way, "the flag arrives");
    assert_eq!(full.map.as_deref(), Some("MAP01"));
    let minimal = teleport_args(r#"{"pad": {"x": 0, "y": 0}, "destination": {"x": 1, "y": 1}}"#);
    assert_eq!(
        minimal.destination,
        Some(TeleportDestinationParam {
            x: 1,
            y: 1,
            angle: 0_i32,
        }),
        "the angle defaults to zero"
    );
    assert!(minimal.pad_size.is_none(), "and the square side");
    assert!(!minimal.two_way, "and the return leg stands aside");
}

#[test]
fn teleport_spec_fills_defaults_and_demands_both_points() {
    let minimal = teleport_args(r#"{"pad": {"x": 8, "y": 8}, "destination": {"x": 12, "y": 12}}"#);
    let spec = teleport_spec(&minimal, &pad_skins()).expect("both points given");
    assert_eq!(
        spec.pad_size, DEFAULT_PAD_SIZE,
        "the documented default side"
    );
    assert_eq!(
        (
            spec.pad.x,
            spec.pad.y,
            spec.destination.x,
            spec.destination.y
        ),
        (8_i32, 8_i32, 12_i32, 12_i32),
        "the points ride through"
    );
    assert_eq!(
        (spec.floor_flat.as_str(), spec.wall.as_str()),
        ("GATE1", "WALL1"),
        "the skins ride through"
    );
    assert!(!spec.two_way, "one-way by default");
    let no_pad = teleport_args(r#"{"destination": {"x": 1, "y": 1}}"#);
    let message = refuse(&teleport_spec(&no_pad, &pad_skins()).unwrap_err());
    assert!(message.contains("pad is required"), "{message}");
    let no_destination = teleport_args(r#"{"pad": {"x": 1, "y": 1}}"#);
    let asked = refuse(&teleport_spec(&no_destination, &pad_skins()).unwrap_err());
    assert!(asked.contains("destination is required"), "{asked}");
    let neither = teleport_args("{}");
    assert!(
        refuse(&teleport_spec(&neither, &pad_skins()).unwrap_err()).contains("pad is required"),
        "the pad is asked for first"
    );
}
