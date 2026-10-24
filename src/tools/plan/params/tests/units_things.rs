//! The wire `units`/`grid` mapping and the thing rows: the identity an
//! omitted `units` object keeps, what a full one snaps to, the refusals
//! unusable scales meet, and the shape `things` and `player_start` rows
//! validate into.

use serde_json::from_str;

use super::{plan_request, refuse, request_with};
use crate::plan::model::Shape;
use crate::plan::things::{Skill, ThingSpot};
use crate::plan::{Units, transform_plan};
use crate::tools::plan::params::{BuildPlanParams, KindSource};
#[test]
fn omitted_units_stay_the_identity_and_grid_defaults_to_eight() {
    let args = request_with("");
    assert_eq!(args.grid, 8_i64, "the documented snap grid");
    assert_eq!(
        args.to_units(),
        Units {
            grid: 8_i64,
            ..Units::default()
        },
        "identity scale and origins, the request grid carried in"
    );
    let explicit_null = request_with(r#", "units": null"#);
    assert_eq!(
        explicit_null.to_units(),
        Units {
            grid: 8_i64,
            ..Units::default()
        },
        "an explicit null units object is the identity too"
    );
}

#[test]
fn a_partial_units_object_fills_the_documented_defaults() {
    let args = request_with(r#", "units": {"scale_num": 4, "flip_y": true}"#);
    assert_eq!(
        args.to_units(),
        Units {
            scale_num: 4_i64,
            scale_den: 1_i64,
            origin_x: 0_i64,
            origin_y: 0_i64,
            grid: 8_i64,
            flip_y: true,
        },
        "the sketch recipe: unset fields keep their defaults"
    );
}

#[test]
fn a_full_units_object_and_grid_lands_on_the_transform() {
    let args = request_with(
        r#", "units": {"scale_num": 2, "scale_den": 4, "origin_x": 10,
                 "origin_y": 20, "flip_y": true}, "grid": 16"#,
    );
    assert_eq!(args.grid, 16_i64, "the wire grid");
    let units = args.to_units();
    assert_eq!(
        units,
        Units {
            scale_num: 2_i64,
            scale_den: 4_i64,
            origin_x: 10_i64,
            origin_y: 20_i64,
            grid: 16_i64,
            flip_y: true,
        }
    );
    let moved = transform_plan(
        &args.to_model().expect("the plan converts"),
        &units,
        args.grid,
    )
    .expect("the wired transform runs");
    assert_eq!(
        moved.rooms.first().expect("the room").shape,
        Shape::Rect {
            x: 16,
            y: -16,
            w: 32,
            h: 32
        },
        "half scale, mirrored y, 16 grid: 10 snaps to 16, 42 to 48, 20 to 16, -12 to -16"
    );
}

#[test]
fn the_grid_snaps_untransformed_coordinates_too() {
    let text = r#"{"map": "MAP01",
            "rooms": [{"id": "a", "rect": {"x": 3, "y": 0, "w": 10, "h": 8}}],
            "connections": []}"#;
    let args: BuildPlanParams = from_str(text).expect("the request parses");
    let moved = transform_plan(
        &args.to_model().expect("the plan converts"),
        &args.to_units(),
        args.grid,
    )
    .expect("the default 8 grid runs");
    assert_eq!(
        moved.rooms.first().expect("the room").shape,
        Shape::Rect {
            x: 0,
            y: 0,
            w: 16,
            h: 8
        },
        "identity scale still snaps: 3 goes to 0, 13 to 16"
    );
}

#[test]
fn a_grid_aligned_plan_passes_the_default_snap_untouched() {
    let args = request_with("");
    let moved = transform_plan(
        &args.to_model().expect("the plan converts"),
        &args.to_units(),
        args.grid,
    )
    .expect("the default 8 grid runs");
    assert_eq!(
        moved.rooms.first().expect("the room").shape,
        Shape::Rect {
            x: 0,
            y: 0,
            w: 64,
            h: 64
        },
        "the existing grid-aligned plans compile exactly as before"
    );
}

#[test]
fn unusable_units_are_refused_naming_the_parameter() {
    for (extra, needle) in [
        (r#", "units": {"scale_den": 0}"#, "scale_den"),
        (r#", "units": {"scale_num": 0}"#, "scale_num"),
        (r#", "grid": 0"#, "grid"),
        (r#", "grid": -8"#, "grid"),
    ] {
        let args = request_with(extra);
        let error = args
            .to_units()
            .check()
            .expect_err("the refusal must surface at the tool");
        assert_eq!(error.code(), "invalid_argument", "{extra}");
        assert!(error.to_string().contains(needle), "{error} names {needle}");
    }
}

#[test]
fn thing_rows_and_the_player_start_keep_their_wire_shape() {
    let args = plan_request(
        r#", "things": [
                {"type": 2035, "x": 40, "y": 40},
                {"name": "imp", "room": "a", "count": 4, "angle": 90,
                 "skills": ["easy", "HARD"], "multiplayer_only": true}
            ],
            "player_start": {"room": "a", "angle": 270}"#,
    );
    let rows = args.to_things().expect("both rows and the start convert");
    assert_eq!(rows.len(), 3, "things first, the player start last");
    let pinned = rows.first().expect("three rows");
    assert_eq!(pinned.kind, KindSource::Type(2035_u32));
    assert_eq!(pinned.spot, ThingSpot::Point((40_i64, 40_i64)));
    assert_eq!(pinned.count, 1_u32, "the documented count default");
    assert_eq!(pinned.angle, 0_i32, "the documented angle default");
    assert_eq!(pinned.skills, None, "unset skills mean every skill");
    assert!(!pinned.multiplayer_only);
    assert_eq!(pinned.path, "things[0]");
    let filled = rows.get(1).expect("three rows");
    assert_eq!(filled.kind, KindSource::Name("imp".to_owned()));
    assert_eq!(filled.spot, ThingSpot::Room { id: "a".to_owned() });
    assert_eq!(filled.count, 4_u32);
    assert_eq!(filled.angle, 90_i32);
    assert_eq!(
        filled.skills,
        Some(vec![Skill::Easy, Skill::Hard]),
        "skills are case-insensitive"
    );
    assert!(filled.multiplayer_only);
    let start = rows.get(2).expect("three rows");
    assert_eq!(start.kind, KindSource::PlayerOne);
    assert_eq!(
        start.spot,
        ThingSpot::RoomCentre { id: "a".to_owned() },
        "the room form centres"
    );
    assert_eq!(start.angle, 270_i32);
    assert_eq!(start.path, "player_start");
}

#[test]
fn an_omitted_things_list_and_player_start_ask_for_a_start_at_the_tool() {
    let args = plan_request("");
    let rows = args.to_things().expect("nothing to convert");
    assert!(rows.is_empty(), "no rows to place means no rows to convert");
}

#[test]
fn a_type_or_name_that_is_both_or_neither_names_its_row() {
    let both = plan_request(r#", "things": [{"type": 2035, "name": "barrel", "x": 8, "y": 8}]"#);
    let message = refuse(&both.to_things().expect_err("type and name are exclusive"));
    assert!(message.contains("things[0]"), "{message}");
    assert!(message.contains("exactly one"), "{message}");
    let neither = plan_request(r#", "things": [{"room": "a"}]"#);
    let bare = refuse(&neither.to_things().expect_err("no type or name"));
    assert!(bare.contains("things[0]"), "{bare}");
}

#[test]
fn half_a_coordinate_or_a_room_and_a_point_names_its_row() {
    for body in [
        r#"{"type": 1, "x": 8, "room": "a"}"#,
        r#"{"type": 1, "y": 8}"#,
        r#"{"type": 1}"#,
        r#"{"type": 1, "x": 8, "y": 8, "room": "a"}"#,
    ] {
        let args = plan_request(&format!(r#", "things": [{body}]"#));
        let message = refuse(&args.to_things().expect_err("the row is malformed"));
        assert!(message.contains("things[0]"), "{message} names things[0]");
    }
}

#[test]
fn angles_outside_the_compass_and_unknown_skills_are_refused() {
    for (extra, needle) in [
        (r#""x": 8, "y": 8, "angle": 360"#, "0..=359"),
        (r#""x": 8, "y": 8, "angle": -1"#, "0..=359"),
        (
            r#""room": "a", "skills": ["nightmare"]"#,
            "things[0].skills",
        ),
    ] {
        let args = plan_request(&format!(
            r#", "things": [{{"type": 2035, {extra}}}],
                    "player_start": {{"room": "a"}}"#
        ));
        let error = args.to_things().expect_err("the row is refused");
        assert_eq!(error.code(), "invalid_argument", "{extra}");
        assert!(error.to_string().contains(needle), "{error} names {needle}");
    }
}

#[test]
fn the_player_start_shape_mirrors_the_thing_rows() {
    let point = plan_request(
        r#", "player_start": {"x": 16, "y": 24, "angle": 45},
                "things": [{"type": 1, "room": "a"}]"#,
    );
    let rows = point.to_things().expect("a pinned start converts");
    assert_eq!(
        rows.first().expect("two rows").spot,
        ThingSpot::Room { id: "a".to_owned() },
        "the thing rows keep their order"
    );
    assert_eq!(
        rows.get(1).expect("two rows").spot,
        ThingSpot::Point((16_i64, 24_i64))
    );
    assert_eq!(rows.get(1).expect("two rows").angle, 45_i32);
    for (start, needle) in [
        (r#"{"x": 8}"#, "player_start"),
        (r#"{"x": 8, "y": 8, "room": "a"}"#, "player_start"),
        ("{}", "player_start"),
        (r#"{"room": "a", "angle": 900}"#, "0..=359"),
    ] {
        let args = plan_request(&format!(r#", "player_start": {start}"#));
        let message = refuse(&args.to_things().expect_err("the start is malformed"));
        assert!(message.contains(needle), "{message} names {needle}");
    }
}
