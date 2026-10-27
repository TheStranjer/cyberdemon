//! The wire feature rows: the four variants with their explicit fields
//! and their documented defaults, the vocabulary each refusal lists, and
//! the room's own `secret` and `light_effect` dressing fields.

use super::{features_request, refuse, request_with, rooms_request};
use crate::plan::model::{ExitStyle, Feature, Side};
use crate::wad::map::features::{LiftStyle, LightEffect};

#[test]
fn every_feature_variant_lands_in_the_model_with_its_explicit_fields() {
    let args = features_request(
        r#"[
                {"kind": "exit", "room": "a", "style": "WALK_OVER", "secret": true, "wall": "North",
                 "at": [8, 56]},
                {"kind": "switch", "room": "a", "opens": "d1", "wall": "west", "repeatable": true,
                 "at": [0, 8]},
                {"kind": "lift", "room": "a", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
                {"kind": "teleporter", "from_room": "a", "to_room": "a", "pad": [40, 40],
                 "destination": [120, 8], "angle": 90, "two_way": true}
            ]"#,
    );
    let plan = args.to_model().expect("all four variants convert");
    assert_eq!(
        plan.features.first().expect("four features"),
        &Feature::Exit {
            room: "a".to_owned(),
            style: ExitStyle::WalkOver,
            secret: true,
            wall: Some(Side::North),
            at: Some((8_i64, 56_i64)),
        },
        "kind, style and wall names are case-insensitive"
    );
    assert_eq!(
        plan.features.get(1).expect("four features"),
        &Feature::Switch {
            room: "a".to_owned(),
            opens: "d1".to_owned(),
            wall: Some(Side::West),
            repeatable: true,
            at: Some((0_i64, 8_i64)),
        },
        "the switch row"
    );
    assert_eq!(
        plan.features.get(2).expect("four features"),
        &Feature::Lift {
            room: "a".to_owned(),
            low_floor: Some(-64_i32),
            style: LiftStyle::WalkTrigger,
            repeatable: false,
        },
        "the lift row"
    );
    assert_eq!(
        plan.features.get(3).expect("four features"),
        &Feature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "a".to_owned(),
            pad: Some((40_i64, 40_i64)),
            destination: Some((120_i64, 8_i64)),
            angle: 90_i32,
            two_way: true,
        },
        "the teleporter row"
    );
}

#[test]
fn feature_rows_fill_their_documented_defaults() {
    let args = features_request(
        r#"[
                {"kind": "exit", "room": "a"},
                {"kind": "switch", "room": "a", "opens": "d1"},
                {"kind": "lift", "room": "a"},
                {"kind": "teleporter", "from_room": "a", "to_room": "a"}
            ]"#,
    );
    let plan = args.to_model().expect("the defaults fill in");
    assert_eq!(
        plan.features.first().expect("four features"),
        &Feature::Exit {
            room: "a".to_owned(),
            style: ExitStyle::Switch,
            secret: false,
            wall: None,
            at: None,
        },
        "the exit defaults"
    );
    assert_eq!(
        plan.features.get(1).expect("four features"),
        &Feature::Switch {
            room: "a".to_owned(),
            opens: "d1".to_owned(),
            wall: None,
            repeatable: false,
            at: None,
        },
        "a switch defaults to one use"
    );
    assert_eq!(
        plan.features.get(2).expect("four features"),
        &Feature::Lift {
            room: "a".to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
        },
        "a lift defaults to use_wall and repeating"
    );
    assert_eq!(
        plan.features.get(3).expect("four features"),
        &Feature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "a".to_owned(),
            pad: None,
            destination: None,
            angle: 0_i32,
            two_way: false,
        },
        "the teleporter defaults"
    );
}

#[test]
fn a_missing_features_list_stays_empty() {
    let args = request_with("");
    let plan = args.to_model().expect("the plan converts");
    assert!(plan.features.is_empty(), "features default to nothing");
}

#[test]
fn feature_refusals_name_their_row_and_offer_the_vocabulary() {
    for (row, needle) in [
        (
            r#"{"kind": "trap", "room": "a"}"#,
            "exit, switch, lift, teleporter",
        ),
        (r#"{"kind": "exit"}"#, "needs a room"),
        (r#"{"kind": "lift"}"#, "needs a room"),
        (r#"{"kind": "switch", "room": "a"}"#, "needs opens"),
        (
            r#"{"kind": "teleporter", "to_room": "a"}"#,
            "needs a from_room",
        ),
        (
            r#"{"kind": "teleporter", "from_room": "a"}"#,
            "needs a to_room",
        ),
        (
            r#"{"kind": "exit", "room": "a", "style": "lever"}"#,
            "\"switch\", \"walk_over\"",
        ),
        (
            r#"{"kind": "lift", "room": "a", "style": "jump"}"#,
            "\"use_wall\", \"walk_trigger\"",
        ),
        (
            r#"{"kind": "exit", "room": "a", "wall": "up"}"#,
            "\"north\", \"south\", \"east\", \"west\"",
        ),
    ] {
        let args = features_request(&format!("[{row}]"));
        let message = refuse(&args.to_model().expect_err("the row is refused"));
        assert!(message.contains("features[0]"), "{message} names the row");
        assert!(message.contains(needle), "{message} names {needle}");
    }
}

#[test]
fn a_rooms_secret_and_light_effect_land_in_the_model() {
    let args = rooms_request(
        r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64},
                "secret": true, "light_effect": "GLOW"}]"#,
    );
    let plan = args.to_model().expect("the room converts");
    let room = plan.rooms.first().expect("one room");
    assert!(room.secret, "the secret flag arrives");
    assert_eq!(
        room.light_effect,
        Some(LightEffect::Glow),
        "case-insensitive"
    );
    let unset = rooms_request(r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}]"#);
    let defaults = unset.to_model().expect("the plain room converts");
    let bare = defaults.rooms.first().expect("one room");
    assert!(!bare.secret, "the default room is no secret");
    assert_eq!(bare.light_effect, None, "no effect asks for nothing");
    let bad = rooms_request(
        r#"[{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64},
                "light_effect": "pulse"}]"#,
    );
    let message = refuse(&bad.to_model().expect_err("no such effect"));
    assert!(message.contains("rooms[0]"), "{message} names the room");
    assert!(
        message.contains("\"flicker\", \"strobe_fast\", \"strobe_slow\", \"glow\", \"random_off\""),
        "the refusal lists the effects: {message}"
    );
}
