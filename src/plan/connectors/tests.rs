//! Shared builders and assertions for the `plan::connectors` tests: the
//! room, connection and plan fixtures, the walls-plus-connectors compile
//! helpers and the band and piece checks the topics share.
//!
//! [`bands`] places openings, doors and windows on the runs two rooms
//! share and checks the band, the gap in the wall and every refusal.
//! [`stairs`] stacks a stair run's steps into the lower room, follows the
//! full-width openings that drop their wall and measures the rounding and
//! clamping helpers directly.

mod bands;
mod stairs;

use super::{ConnectorGeometry, plan_connectors};
use crate::error::CyberError;
use crate::geom::{Piece, Pt};
use crate::plan::model::{Connection, ConnectionKind, Plan, Room, Shape};
use crate::plan::walls::{Walls, build_walls};

/// A rectangular room with default sector properties.
pub(super) fn rect_room(id: &str, left_bottom: Pt, width_height: Pt) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Rect {
            x: left_bottom.0,
            y: left_bottom.1,
            w: width_height.0,
            h: width_height.1,
        },
        floor: 0,
        ceiling: 128,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A polygon room with default sector properties.
fn polygon_room(id: &str, points: Vec<Pt>) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Polygon { points },
        floor: 0,
        ceiling: 128,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A default-width opening between two rooms by id.
pub(super) fn opening(from: &str, to: &str) -> Connection {
    Connection {
        id: None,
        from: from.to_owned(),
        to: to.to_owned(),
        kind: ConnectionKind::Opening,
        at: None,
        width: 64,
    }
}

/// A default stair run between two rooms by id.
pub(super) fn stairs(from: &str, to: &str) -> Connection {
    let mut climb = opening(from, to);
    climb.kind = ConnectionKind::Stairs {
        step_height: 16_i32,
        step_depth: 24_i32,
    };
    climb
}

/// Two 128 x 128 rooms side by side, sharing the line x = 128.
pub(super) fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
    Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (128, 0), (128, 128)),
        ],
        connections,
        features: Vec::new(),
        wall_thickness: thickness,
    }
}

/// Rooms sharing two disjoint runs on x = 128: a tall room `a` and
/// a `C`-shaped room `b` whose notch separates the runs.
pub(super) fn notch_plan(connections: Vec<Connection>) -> Plan {
    Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 256)),
            polygon_room(
                "b",
                vec![
                    (128, 0),
                    (256, 0),
                    (256, 128),
                    (128, 128),
                    (128, 64),
                    (192, 64),
                    (192, 32),
                    (128, 32),
                ],
            ),
        ],
        connections,
        features: Vec::new(),
        wall_thickness: 16,
    }
}

/// Raise room `b` (the plan's second room) to `floor`.
pub(super) fn raise_b(plan: &mut Plan, floor: i32) {
    if let Some(room) = plan.rooms.get_mut(1) {
        room.floor = floor;
    }
}

/// Build walls and connectors for a plan expected to succeed.
#[track_caller]
pub(super) fn compile(plan: &Plan) -> (Walls, Vec<ConnectorGeometry>) {
    let walls = build_walls(plan).expect("the fixture rooms arrange");
    let connectors = plan_connectors(plan, &walls).expect("the connectors place");
    (walls, connectors)
}

/// The first connector of a plan that compiles.
#[track_caller]
pub(super) fn first(plan: &Plan) -> ConnectorGeometry {
    compile(plan)
        .1
        .into_iter()
        .next()
        .expect("the plan places one connector")
}

/// Build walls and expect the connector pass to fail.
#[track_caller]
pub(super) fn error_of(plan: &Plan) -> CyberError {
    let walls = build_walls(plan).expect("the fixture rooms arrange");
    plan_connectors(plan, &walls).expect_err("the connector pass must fail")
}

/// The piece between two endpoints, either direction.
pub(super) fn find_piece(pieces: &[Piece], one: Pt, two: Pt) -> Option<&Piece> {
    pieces
        .iter()
        .find(|piece| (piece.a == one && piece.b == two) || (piece.a == two && piece.b == one))
}

/// Assert the band rectangle centred on `centre` with half extent
/// `half` (across, along) matching `Frame::band`'s corner order.
#[track_caller]
pub(super) fn assert_band(polygon: &[Pt], centre: Pt, half: Pt, note: &str) {
    let (west, east) = (
        centre.0.saturating_sub(half.0),
        centre.0.saturating_add(half.0),
    );
    let (low, high) = (
        centre.1.saturating_sub(half.1),
        centre.1.saturating_add(half.1),
    );
    assert_eq!(
        polygon.to_vec(),
        vec![(west, high), (west, low), (east, low), (east, high)],
        "{note}"
    );
}
