//! Shared builders and assertions for the `plan::model` tests: the
//! room, connection and plan constructors, the four feature-row builders
//! and the `assert_invalid` helper the topics fold their refusals
//! through.
//!
//! [`wire`] checks the serde shape and [`Room::polygon`]; [`validate`]
//! refuses every structural problem [`Plan::validate`] sees.

mod validate;
mod wire;

use super::{
    Connection, ConnectionKind, ExitStyle, Feature, Plan, Room, Shape, default_ceiling,
    default_floor, default_light, default_wall_thickness, default_width,
};
use crate::wad::map::features::LiftStyle;

/// A room with default sector properties and the given shape.
pub(super) fn room_with(id: &str, shape: Shape) -> Room {
    Room {
        id: id.to_owned(),
        shape,
        floor: default_floor(),
        ceiling: default_ceiling(),
        light: default_light(),
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A rectangular room at the origin, `64` x `64`.
pub(super) fn square_room(id: &str) -> Room {
    room_with(
        id,
        Shape::Rect {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
        },
    )
}

/// A default-width opening between two rooms.
pub(super) fn opening(from: &str, to: &str) -> Connection {
    Connection {
        id: None,
        from: from.to_owned(),
        to: to.to_owned(),
        kind: ConnectionKind::Opening,
        at: None,
        width: default_width(),
    }
}

/// A plan over the given rooms and connections.
pub(super) fn plan(rooms: Vec<Room>, connections: Vec<Connection>) -> Plan {
    Plan {
        rooms,
        connections,
        features: Vec::new(),
        wall_thickness: default_wall_thickness(),
    }
}

/// A plan over the given rooms, connections and features.
pub(super) fn plan_with_features(
    rooms: Vec<Room>,
    connections: Vec<Connection>,
    features: Vec<Feature>,
) -> Plan {
    Plan {
        rooms,
        connections,
        features,
        wall_thickness: default_wall_thickness(),
    }
}

/// A plain exit in the named room.
pub(super) fn exit_in(room: &str) -> Feature {
    Feature::Exit {
        room: room.to_owned(),
        style: ExitStyle::Switch,
        secret: false,
        wall: None,
        at: None,
    }
}

/// A switch in the named room opening the named connection.
pub(super) fn switch_in(room: &str, opens: &str) -> Feature {
    Feature::Switch {
        room: room.to_owned(),
        opens: opens.to_owned(),
        wall: None,
        repeatable: false,
        at: None,
    }
}

/// A plain lift in the named room.
pub(super) fn lift_in(room: &str) -> Feature {
    Feature::Lift {
        room: room.to_owned(),
        low_floor: None,
        style: LiftStyle::UseWall,
        repeatable: true,
    }
}

/// A plain teleporter from `from` to `to`.
pub(super) fn teleporter(from: &str, to: &str) -> Feature {
    Feature::Teleporter {
        from_room: from.to_owned(),
        to_room: to.to_owned(),
        pad: None,
        destination: None,
        angle: 0_i32,
        two_way: false,
    }
}

/// Asserts `plan` is rejected with `invalid_argument` and a message
/// mentioning `needle`.
#[track_caller]
pub(super) fn assert_invalid(plan: &Plan, needle: &str) {
    let error = plan.validate().expect_err("the plan must be rejected");
    assert_eq!(error.code(), "invalid_argument", "code for: {error}");
    assert!(
        error.to_string().contains(needle),
        "message '{error}' must mention '{needle}'"
    );
}
