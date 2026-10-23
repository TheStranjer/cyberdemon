//! Shared builders and assertions for the `plan::units` tests: the room,
//! connection, plan and teleporter constructors, the sample plan the
//! refusals fold over and the three `assert_*` helpers.
//!
//! [`mapping`] scales, mirrors, snaps and carries whole plans through
//! [`transform_plan`](super::transform_plan) and checks where a plan
//! cannot go; [`refusals`] refuses unusable [`Units`] and names the plan
//! path each overflowing coordinate sits on.

mod mapping;
mod refusals;

use super::{Units, transform_plan};
use crate::error::CyberError;
use crate::geom::Pt;
use crate::plan::model::{Connection, ConnectionKind, Feature, Plan, Room, Shape};

/// A rectangular room with default sector properties, based at `at`
/// and `size` wide.
pub(super) fn rect_room(id: &str, at: Pt, size: Pt) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Rect {
            x: at.0,
            y: at.1,
            w: size.0,
            h: size.1,
        },
        floor: 0_i32,
        ceiling: 128_i32,
        light: 160_i32,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A polygon room with default sector properties.
pub(super) fn polygon_room(id: &str, points: Vec<Pt>) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Polygon { points },
        floor: 0_i32,
        ceiling: 128_i32,
        light: 160_i32,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// An opening between two rooms, placed at `at` when given.
pub(super) fn opening(from: &str, to: &str, at: Option<Pt>) -> Connection {
    Connection {
        id: None,
        from: from.to_owned(),
        to: to.to_owned(),
        kind: ConnectionKind::Opening,
        at,
        width: 48_i64,
    }
}

/// A plan over `rooms` and `connections` with a 16-unit wall.
pub(super) fn plan_with(rooms: Vec<Room>, connections: Vec<Connection>) -> Plan {
    Plan {
        rooms,
        connections,
        features: Vec::new(),
        wall_thickness: 16_i64,
    }
}

/// The sample plan's rooms and opening carrying `features`.
pub(super) fn plan_features(features: Vec<Feature>) -> Plan {
    Plan {
        features,
        ..sample_plan()
    }
}

/// A teleporter from room `a` to room `b` with the given squares.
pub(super) fn teleporter(pad: Option<Pt>, destination: Option<Pt>) -> Feature {
    Feature::Teleporter {
        from_room: "a".to_owned(),
        to_room: "b".to_owned(),
        pad,
        destination,
        angle: 90_i32,
        two_way: true,
    }
}

/// The two-room, one-opening plan the refusals share.
pub(super) fn sample_plan() -> Plan {
    plan_with(
        vec![
            rect_room("a", (0, 0), (64, 64)),
            rect_room("b", (96, 0), (64, 64)),
        ],
        vec![opening("a", "b", Some((80, 32)))],
    )
}

/// Asserts `error` is an `invalid_argument` whose message names
/// `needle`.
#[track_caller]
pub(super) fn assert_argument(error: &CyberError, needle: &str) {
    assert_eq!(error.code(), "invalid_argument", "code for: {error}");
    assert!(
        error.to_string().contains(needle),
        "message '{error}' must name '{needle}'"
    );
}

/// Asserts `units` is refused by [`Units::check`] and by
/// [`transform_plan`], each time naming `needle`.
#[track_caller]
pub(super) fn assert_refused(units: &Units, needle: &str) {
    let checked = units.check().expect_err("the units must be refused");
    assert_argument(&checked, needle);
    let moved =
        transform_plan(&sample_plan(), units, 1_i64).expect_err("the transform must be refused");
    assert_argument(&moved, needle);
}

/// Asserts a `plan` transform under `units` and `grid` is refused
/// naming `needle`.
#[track_caller]
pub(super) fn assert_plan_refused(plan: &Plan, units: &Units, grid: i64, needle: &str) {
    let refused = transform_plan(plan, units, grid).expect_err("the coordinate must fail");
    assert_argument(&refused, needle);
}
