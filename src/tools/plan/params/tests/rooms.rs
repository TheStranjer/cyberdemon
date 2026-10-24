//! The wire rooms and connections: how the documented shape lands in
//! the model, which defaults an omitted field fills, the ring a `points`
//! room carries, and the paths and vocabularies an unknown `kind`, door
//! `style` or footprint refusal names.

use serde_json::from_str;

use super::{connections_request, refuse, rooms_request};
use crate::plan::model::{ConnectionKind, DoorStyle, KeyColor, Plan, Shape};
use crate::tools::plan::params::{BuildPlanParams, DEFAULT_WIDTH};

/// Wire parameters parsed from the two-room-and-door example.
const FULL: &str = r#"{
        "map": "MAP05",
        "rooms": [
            {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
            {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128},
             "floor": 16, "ceiling": 160, "light": 200,
             "floor_flat": "FLAT10", "ceiling_flat": "F_SKY1",
             "wall_texture": "WALL1", "outdoor": true}
        ],
        "connections": [
            {"id": "d1", "from": "a", "to": "b", "kind": "door",
             "at": [128, 32], "width": 48, "style": "locked_blue"},
            {"from": "b", "to": "a", "kind": "window", "at": [128, 96],
             "sill": 48, "lintel": 20},
            {"from": "a", "to": "b", "kind": "stairs",
             "step_height": 8, "step_depth": 16},
            {"from": "a", "to": "b", "kind": "OPENING", "width": 32}
        ],
        "wall_thickness": 24,
        "format": "udmf",
        "namespace": "ZDoom",
        "replace": true,
        "theme": "hell"
    }"#;
#[test]
fn the_documented_wire_shape_lands_in_the_model() {
    let args: BuildPlanParams = from_str(FULL).expect("the full example parses");
    assert_eq!(args.map, "MAP05");
    assert_eq!(args.format.as_deref(), Some("udmf"));
    assert_eq!(args.namespace.as_deref(), Some("ZDoom"));
    assert!(args.replace, "the flag arrives");
    assert_eq!(args.wall_thickness, 24_i64);
    assert_eq!(args.theme.as_deref(), Some("hell"));
    let plan = args.to_model().expect("the plan converts");
    let b = plan.rooms.get(1).expect("room b");
    assert_eq!(b.floor, 16_i32, "explicit sector properties");
    assert_eq!(b.ceiling, 160_i32);
    assert_eq!(b.light, 200_i32);
    assert_eq!(b.floor_flat.as_deref(), Some("FLAT10"));
    assert_eq!(b.wall_texture.as_deref(), Some("WALL1"));
    assert!(b.outdoor, "the sky flag rides along");
    let door = plan.connections.first().expect("the door");
    assert_eq!(
        door.kind,
        ConnectionKind::Door {
            style: DoorStyle::Locked(KeyColor::Blue),
        },
        "the locked-blue style"
    );
    assert_eq!(door.at, Some((128_i64, 32_i64)), "the placement point");
    assert_eq!(door.width, 48_i64, "the explicit width");
    assert_eq!(
        plan.connections.get(1).expect("the window").kind,
        ConnectionKind::Window {
            sill: 48_i32,
            lintel: 20_i32
        },
        "the window band"
    );
    assert_eq!(
        plan.connections.get(2).expect("the stairs").kind,
        ConnectionKind::Stairs {
            step_height: 8_i32,
            step_depth: 16_i32
        },
        "the stair run"
    );
    assert_eq!(
        plan.connections.get(3).expect("the opening").kind,
        ConnectionKind::Opening,
        "kinds are case-insensitive"
    );
    assert_eq!(plan.connections.get(3).expect("the opening").width, 32_i64);
}

#[test]
fn omissions_materialise_the_documented_defaults() {
    let args = rooms_request(r#"[{"id": "a", "rect": {"x": 1, "y": 2, "w": 3, "h": 4}}]"#);
    let plan: Plan = args.to_model().expect("defaults fill in");
    assert_eq!(plan.wall_thickness, 16_i64, "the default wall");
    let room = plan.rooms.first().expect("the one room");
    assert_eq!(room.floor, 0_i32);
    assert_eq!(room.ceiling, 128_i32);
    assert_eq!(room.light, 160_i32);
    assert!(!room.outdoor);
    assert_eq!(room.floor_flat, None, "the theme answers the roles");
    assert_eq!(
        room.shape,
        Shape::Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4
        }
    );
    let window_args = connections_request(r#"[{"from": "a", "to": "a", "kind": "window"}]"#);
    let model = window_args.to_model().expect("kind defaults fill in");
    let window = model.connections.first().expect("the window");
    assert_eq!(window.width, DEFAULT_WIDTH, "the default opening width");
    assert_eq!(window.at, None, "centred on the shared wall");
    assert_eq!(window.id, None, "the label is derived by the model");
    assert_eq!(
        window.kind,
        ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32
        },
        "the default glass band"
    );
}

#[test]
fn points_rooms_carry_the_ring_as_given() {
    let args = rooms_request(r#"[{"id": "a", "points": [[0, 0], [64, 0], [32, 64]]}]"#);
    let plan = args.to_model().expect("a polygon room converts");
    assert_eq!(
        plan.rooms.first().expect("the room").shape,
        Shape::Polygon {
            points: vec![(0_i64, 0_i64), (64, 0), (32, 64)]
        }
    );
}

#[test]
fn a_room_with_neither_or_both_footprints_names_its_path() {
    let neither = rooms_request(r#"[{"id": "a"}, {"id": "b", "rect": {"x":0,"y":0,"w":1,"h":1}}]"#);
    let bare = refuse(&neither.to_model().expect_err("no footprint"));
    assert!(bare.contains("rooms[0]"), "{bare}");

    let second = rooms_request(
        r#"[{"id": "a", "rect": {"x":0,"y":0,"w":8,"h":8}},
                {"id": "b", "rect": {"x":0,"y":0,"w":8,"h":8},
                 "points": [[0,0],[8,0],[8,8]]}]"#,
    );
    let message = refuse(&second.to_model().expect_err("both footprints"));
    assert!(message.contains("rooms[1]"), "{message}");
    assert!(message.contains("exactly one"), "{message}");
}

#[test]
fn unknown_kind_and_style_names_mention_their_paths() {
    let args = connections_request(r#"[{"from": "a", "to": "a", "kind": "portal"}]"#);
    let kind = refuse(&args.to_model().expect_err("no such kind"));
    assert!(kind.contains("connections[0]"), "{kind}");
    assert!(
        kind.contains("opening, door, window, stairs"),
        "the refusal lists the kinds: {kind}"
    );

    let styled =
        connections_request(r#"[{"from": "a", "to": "a", "kind": "door", "style": "slide"}]"#);
    let message = refuse(&styled.to_model().expect_err("no such style"));
    assert!(message.contains("connections[0]"), "{message}");
    assert!(
        message.contains("normal, fast, stay_open, locked_red, locked_blue, locked_yellow, remote"),
        "the refusal lists the styles: {message}"
    );
}

#[test]
fn the_door_style_defaults_to_normal_and_reads_every_name() {
    let plain = connections_request(r#"[{"from": "a", "to": "a", "kind": "door"}]"#);
    let model = plain.to_model().expect("the plain door converts");
    assert_eq!(
        model.connections.first().expect("door").kind,
        ConnectionKind::Door {
            style: DoorStyle::Normal
        }
    );
    for (text, style) in [
        ("normal", DoorStyle::Normal),
        ("FAST", DoorStyle::Fast),
        ("stay_open", DoorStyle::StayOpen),
        ("locked_red", DoorStyle::Locked(KeyColor::Red)),
        ("locked_blue", DoorStyle::Locked(KeyColor::Blue)),
        ("locked_yellow", DoorStyle::Locked(KeyColor::Yellow)),
        ("remote", DoorStyle::Remote),
    ] {
        let probe = connections_request(&format!(
            r#"[{{"from":"a","to":"a","kind":"door","style":"{text}"}}]"#
        ));
        let converted = probe.to_model().expect("styles are case-insensitive");
        assert_eq!(
            converted.connections.first().expect("door").kind,
            ConnectionKind::Door { style },
            "{text}"
        );
    }
}
