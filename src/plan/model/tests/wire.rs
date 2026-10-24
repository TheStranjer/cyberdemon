//! Wire-format tests: the documented JSON layout round trips, omitted
//! fields materialise their defaults, the tagged feature rows keep their
//! names, a room's light effect names its snake-case word and
//! [`Room::polygon`](crate::plan::model::Room::polygon) answers for both
//! shapes.

use super::room_with;
use crate::plan::model::{ConnectionKind, ExitStyle, Feature, Plan, Shape, Side};
use crate::wad::map::features::{LiftStyle, LightEffect};

#[test]
fn a_full_plan_round_trips_through_json() {
    let text = r#"{
        "rooms": [
            {
                "id": "courtyard",
                "shape": {"polygon": {"points": [[0, 0], [128, 0], [128, 128], [0, 128]]}},
                "ceiling": 512,
                "light": 192,
                "ceiling_flat": "F_SKY1",
                "outdoor": true
            },
            {
                "id": "lab",
                "shape": {"rect": {"x": 144, "y": 0, "w": 128, "h": 128}},
                "floor": 8,
                "floor_flat": "FLAT10",
                "wall_texture": "STONE6"
            }
        ],
        "connections": [
            {"from": "courtyard", "to": "lab", "kind": "opening"},
            {"id": "d1", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "normal"}}, "at": [160, 64], "width": 48},
            {"id": "d2", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "fast"}}},
            {"id": "d3", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "stay_open"}}},
            {"id": "d4", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "red"}}}},
            {"id": "d5", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "blue"}}}},
            {"id": "d6", "from": "lab", "to": "courtyard", "kind": {"door": {"style": {"locked": "yellow"}}}},
            {"id": "d7", "from": "lab", "to": "courtyard", "kind": {"door": {"style": "remote"}}},
            {"id": "w1", "from": "courtyard", "to": "lab", "kind": {"window": {"sill": 48, "lintel": 80}}},
            {"id": "w2", "from": "courtyard", "to": "lab", "kind": {"window": {}}},
            {"id": "s1", "from": "lab", "to": "courtyard", "kind": {"stairs": {"step_height": 8, "step_depth": 16}}, "at": [0, 0]},
            {"id": "s2", "from": "lab", "to": "courtyard", "kind": {"stairs": {}}}
        ],
        "features": [
            {"kind": "exit", "room": "courtyard", "style": "walk_over", "secret": true, "wall": "north"},
            {"kind": "switch", "room": "lab", "opens": "d7", "wall": "west", "repeatable": true},
            {"kind": "lift", "room": "lab", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
            {"kind": "teleporter", "from_room": "courtyard", "to_room": "lab", "pad": [64, 64], "destination": [200, 64], "angle": 90, "two_way": true}
        ],
        "wall_thickness": 24
    }"#;
    let parsed: Plan = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(parsed.wall_thickness, 24_i64, "explicit wall thickness");
    let courtyard = parsed.rooms.first().expect("first room");
    assert_eq!(courtyard.floor, 0_i32, "default floor");
    assert_eq!(courtyard.ceiling, 512_i32, "explicit ceiling");
    assert!(courtyard.outdoor, "explicit outdoor");
    assert_eq!(courtyard.ceiling_flat.as_deref(), Some("F_SKY1"), "flat");
    let written = serde_json::to_string(&parsed).expect("serialising a plan");
    let again: Plan = serde_json::from_str(&written).expect("re-reading the plan");
    assert_eq!(parsed, again, "round trip must preserve the plan");
}

#[test]
fn feature_rows_keep_their_tagged_wire_names() {
    let text = r#"{
        "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
        "connections": [],
        "features": [
            {"kind": "exit", "room": "a", "style": "walk_over", "secret": true, "wall": "north", "at": [32, 96]},
            {"kind": "switch", "room": "a", "opens": "d1", "wall": "west", "repeatable": true, "at": [0, 32]},
            {"kind": "lift", "room": "a", "low_floor": -64, "style": "walk_trigger", "repeatable": false},
            {"kind": "teleporter", "from_room": "a", "to_room": "a", "pad": [64, 64], "destination": [200, 64], "angle": 90, "two_way": true}
        ]
    }"#;
    let parsed: Plan = serde_json::from_str(text).expect("the tagged feature rows");
    assert_eq!(
        parsed.features.first().expect("four features"),
        &Feature::Exit {
            room: "a".to_owned(),
            style: ExitStyle::WalkOver,
            secret: true,
            wall: Some(Side::North),
            at: Some((32_i64, 96_i64)),
        },
        "the exit row is tagged by kind"
    );
    assert_eq!(
        parsed.features.get(1).expect("four features"),
        &Feature::Switch {
            room: "a".to_owned(),
            opens: "d1".to_owned(),
            wall: Some(Side::West),
            repeatable: true,
            at: Some((0_i64, 32_i64)),
        },
        "the switch row"
    );
    assert_eq!(
        parsed.features.get(2).expect("four features"),
        &Feature::Lift {
            room: "a".to_owned(),
            low_floor: Some(-64_i32),
            style: LiftStyle::WalkTrigger,
            repeatable: false,
        },
        "the lift row"
    );
    assert_eq!(
        parsed.features.get(3).expect("four features"),
        &Feature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "a".to_owned(),
            pad: Some((64_i64, 64_i64)),
            destination: Some((200_i64, 64_i64)),
            angle: 90_i32,
            two_way: true,
        },
        "the teleporter row keeps its squares"
    );
    let written = serde_json::to_string(&parsed).expect("serialising features");
    let again: Plan = serde_json::from_str(&written).expect("re-reading the features");
    assert_eq!(parsed, again, "the tagged rows round trip");
}

#[test]
fn omitted_feature_fields_materialise_their_defaults() {
    let text = r#"{
        "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
        "connections": [],
        "features": [
            {"kind": "exit", "room": "a", "style": "switch"},
            {"kind": "switch", "room": "a", "opens": "d1"},
            {"kind": "lift", "room": "a"},
            {"kind": "teleporter", "from_room": "a", "to_room": "a"}
        ]
    }"#;
    let parsed: Plan = serde_json::from_str(text).expect("a plan with minimal features");
    assert_eq!(
        parsed.features.first().expect("four features"),
        &Feature::Exit {
            room: "a".to_owned(),
            style: ExitStyle::Switch,
            secret: false,
            wall: None,
            at: None,
        },
        "default exit secret, wall and point"
    );
    assert_eq!(
        parsed.features.get(1).expect("four features"),
        &Feature::Switch {
            room: "a".to_owned(),
            opens: "d1".to_owned(),
            wall: None,
            repeatable: false,
            at: None,
        },
        "default switch repeatable, wall and point"
    );
    assert_eq!(
        parsed.features.get(2).expect("four features"),
        &Feature::Lift {
            room: "a".to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
        },
        "default lift style and repeatable"
    );
    assert_eq!(
        parsed.features.get(3).expect("four features"),
        &Feature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "a".to_owned(),
            pad: None,
            destination: None,
            angle: 0_i32,
            two_way: false,
        },
        "default teleporter squares, angle and return leg"
    );
}

#[test]
fn omitted_fields_materialise_their_defaults() {
    let text = r#"{
        "rooms": [{"id": "a", "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}}}],
        "connections": [
            {"from": "a", "to": "a", "kind": {"window": {}}},
            {"from": "a", "to": "a", "kind": {"stairs": {}}}
        ]
    }"#;
    let parsed: Plan = serde_json::from_str(text).expect("a minimal plan");
    assert_eq!(parsed.wall_thickness, 16_i64, "default wall thickness");
    let room = parsed.rooms.first().expect("one room");
    assert_eq!(room.floor, 0_i32, "default floor");
    assert_eq!(room.ceiling, 128_i32, "default ceiling");
    assert_eq!(room.light, 160_i32, "default light");
    assert!(!room.outdoor, "default outdoor");
    assert!(!room.secret, "default secret");
    assert_eq!(room.light_effect, None, "default light effect");
    assert_eq!(room.floor_flat, None, "default floor flat");
    assert_eq!(room.ceiling_flat, None, "default ceiling flat");
    assert_eq!(room.wall_texture, None, "default wall texture");
    assert!(parsed.features.is_empty(), "default feature list");
    let window = parsed.connections.first().expect("first connection");
    assert_eq!(window.width, 64_i64, "default connection width");
    assert_eq!(window.id, None, "default connection id");
    assert_eq!(window.at, None, "default placement");
    assert_eq!(
        window.kind,
        ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32
        },
        "default sill and lintel"
    );
    let stairs = parsed.connections.get(1).expect("second connection");
    assert_eq!(
        stairs.kind,
        ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32
        },
        "default step height and depth"
    );
}

#[test]
fn a_rooms_light_effect_names_its_snake_case_word() {
    let text = r#"{
        "rooms": [{
            "id": "a",
            "shape": {"rect": {"x": 0, "y": 0, "w": 64, "h": 64}},
            "secret": true,
            "light_effect": "strobe_slow"
        }],
        "connections": []
    }"#;
    let parsed: Plan = serde_json::from_str(text).expect("a lit room");
    let room = parsed.rooms.first().expect("one room");
    assert!(room.secret, "explicit secret");
    assert_eq!(room.light_effect, Some(LightEffect::StrobeSlow));
    let written = serde_json::to_string(room).expect("serialising a room");
    assert!(written.contains("\"strobe_slow\""), "{written}");
}

#[test]
fn rect_polygon_is_counter_clockwise_from_the_base_corner() {
    let room = room_with(
        "r",
        Shape::Rect {
            x: 10,
            y: 20,
            w: 64,
            h: 32,
        },
    );
    assert_eq!(
        room.polygon(),
        vec![(10, 20), (74, 20), (74, 52), (10, 52)],
        "counter-clockwise corners from (x, y)"
    );
}

#[test]
fn polygon_room_returns_the_points_as_given() {
    let points = vec![(0_i64, 0_i64), (128, 0), (96, 128), (0, 96)];
    let room = room_with(
        "r",
        Shape::Polygon {
            points: points.clone(),
        },
    );
    assert_eq!(room.polygon(), points, "the ring passes through");
}
