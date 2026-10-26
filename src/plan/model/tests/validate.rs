//! Validation tests: every structural refusal
//! [`Plan::validate`](crate::plan::model::Plan::validate) makes,
//! from the empty plan through the shape, connection and feature checks
//! to the `u32` label budgets, and the plans it accepts.

use super::{
    assert_invalid, exit_in, lift_in, opening, plan, plan_with_features, room_with, square_room,
    switch_in, teleporter,
};
use crate::error::PlanRef;
use crate::plan::model::{ConnectionKind, DoorStyle, KeyColor, Shape};

#[test]
fn an_empty_plan_is_refused() {
    let plan = plan(vec![], vec![]);
    assert_invalid(&plan, "rooms");
}

#[test]
fn duplicate_room_ids_are_refused() {
    let plan = plan(
        vec![square_room("a"), square_room("a"), square_room("b")],
        vec![],
    );
    assert_invalid(&plan, "room id 'a'");
}

#[test]
fn degenerate_rectangles_are_refused() {
    let flat = room_with(
        "cellar",
        Shape::Rect {
            x: 0,
            y: 0,
            w: 64,
            h: 0,
        },
    );
    let plan = plan(vec![square_room("a"), flat], vec![]);
    assert_invalid(&plan, "cellar");
}

#[test]
fn polygons_with_fewer_than_three_distinct_points_are_refused() {
    let bent = Shape::Polygon {
        points: vec![(0_i64, 0_i64), (64, 0), (0, 0)],
    };
    let plan = plan(vec![room_with("attic", bent)], vec![]);
    assert_invalid(&plan, "attic");
}

#[test]
fn connections_must_reference_existing_rooms() {
    let unknown_target = plan(vec![square_room("a")], vec![opening("a", "ghost")]);
    assert_invalid(&unknown_target, "'ghost'");
    let unknown_source = plan(vec![square_room("a")], vec![opening("ghost", "a")]);
    assert_invalid(&unknown_source, "'ghost'");
}

#[test]
fn self_connections_are_refused() {
    let plan = plan(vec![square_room("a")], vec![opening("a", "a")]);
    assert_invalid(&plan, "room 'a' to itself");
}

#[test]
fn non_positive_widths_are_refused() {
    let mut narrow = opening("a", "b");
    narrow.id = Some("c1".to_owned());
    narrow.width = 0_i64;
    let plan = plan(vec![square_room("a"), square_room("b")], vec![narrow]);
    assert_invalid(&plan, "c1");
}

#[test]
fn negative_wall_thickness_is_refused() {
    let mut plan = plan(vec![square_room("a")], vec![]);
    plan.wall_thickness = -1_i64;
    assert_invalid(&plan, "thickness");
}

#[test]
fn duplicate_connection_ids_are_refused_with_both_indices() {
    let mut first = opening("a", "b");
    first.id = Some("d1".to_owned());
    let mut second = opening("a", "b");
    second.id = Some("d1".to_owned());
    let plan = plan(
        vec![square_room("a"), square_room("b")],
        vec![first, second],
    );
    assert_invalid(&plan, "connections[0] and connections[1]");
}

#[test]
fn unnamed_connections_may_share_their_derived_label() {
    let plan = plan(
        vec![square_room("a"), square_room("b")],
        vec![opening("a", "b"), opening("a", "b")],
    );
    plan.validate()
        .expect("only carried ids key the run's lookups");
}

#[test]
fn distinct_connection_ids_validate() {
    let mut first = opening("a", "b");
    first.id = Some("d1".to_owned());
    let mut second = opening("a", "b");
    second.id = Some("d2".to_owned());
    let plan = plan(
        vec![square_room("a"), square_room("b")],
        vec![first, second],
    );
    plan.validate().expect("two ids, two connections");
}

#[test]
fn stairs_must_have_a_positive_step_depth() {
    let mut steep = opening("a", "b");
    steep.id = Some("s1".to_owned());
    steep.kind = ConnectionKind::Stairs {
        step_height: 16_i32,
        step_depth: 0_i32,
    };
    let plan = plan(vec![square_room("a"), square_room("b")], vec![steep]);
    assert_invalid(&plan, "s1");
}

#[test]
fn stairs_step_heights_outside_one_to_twenty_four_are_refused() {
    for step_height in [-8_i32, 0_i32, 25_i32, 512_i32] {
        let mut climb = opening("a", "b");
        climb.id = Some("s1".to_owned());
        climb.kind = ConnectionKind::Stairs {
            step_height,
            step_depth: 24_i32,
        };
        let plan = plan(vec![square_room("a"), square_room("b")], vec![climb]);
        assert_invalid(&plan, "step_height");
    }
}

#[test]
fn stairs_step_heights_at_the_bounds_validate() {
    for step_height in [1_i32, 16_i32, 24_i32] {
        let mut climb = opening("a", "b");
        climb.id = Some("s1".to_owned());
        climb.kind = ConnectionKind::Stairs {
            step_height,
            step_depth: 24_i32,
        };
        let plan = plan(vec![square_room("a"), square_room("b")], vec![climb]);
        plan.validate().expect("in-bounds step heights climb");
    }
}

#[test]
fn negative_window_trims_are_refused() {
    for (sill, lintel) in [(-1_i32, 24_i32), (32_i32, -1_i32), (-16_i32, -16_i32)] {
        let mut view = opening("a", "b");
        view.id = Some("w1".to_owned());
        view.kind = ConnectionKind::Window { sill, lintel };
        let plan = plan(vec![square_room("a"), square_room("b")], vec![view]);
        assert_invalid(&plan, "0 or greater");
    }
}

#[test]
fn window_trims_that_fill_the_rooms_opening_are_refused() {
    for (sill, lintel) in [(48_i32, 80_i32), (128_i32, 0_i32), (200_i32, 200_i32)] {
        let mut view = opening("a", "b");
        view.id = Some("w1".to_owned());
        view.kind = ConnectionKind::Window { sill, lintel };
        let plan = plan(vec![square_room("a"), square_room("b")], vec![view]);
        assert_invalid(&plan, "leave no glass");
    }
}

#[test]
fn window_trims_that_leave_an_opening_validate() {
    let mut view = opening("a", "b");
    view.id = Some("w1".to_owned());
    view.kind = ConnectionKind::Window {
        sill: 48_i32,
        lintel: 79_i32,
    };
    let plan = plan(vec![square_room("a"), square_room("b")], vec![view]);
    plan.validate()
        .expect("127 of the 128-unit opening trimmed still leaves a sliver to the model");
}

#[test]
fn every_feature_kind_refuses_an_unknown_room_by_name() {
    let rooms = vec![square_room("a"), square_room("b")];
    for (feature, kind) in [
        (exit_in("ghost"), "exit"),
        (switch_in("ghost", "d1"), "switch"),
        (lift_in("ghost"), "lift"),
        (teleporter("ghost", "a"), "teleporter"),
        (teleporter("a", "ghost"), "teleporter"),
    ] {
        let plan = plan_with_features(rooms.clone(), vec![], vec![feature]);
        assert_invalid(&plan, &format!("{kind} feature names unknown room 'ghost'"));
    }
}

#[test]
fn a_switch_must_open_a_connection_by_id() {
    let mut named = opening("a", "b");
    named.id = Some("d1".to_owned());
    let rooms = vec![square_room("a"), square_room("b")];
    let good = plan_with_features(
        rooms.clone(),
        vec![named.clone()],
        vec![switch_in("a", "d1")],
    );
    good.validate().expect("the switch names the connection");
    let unknown = plan_with_features(rooms, vec![named], vec![switch_in("a", "nope")]);
    assert_invalid(&unknown, "opens unknown connection 'nope'");
}

#[test]
fn a_switch_opening_an_unnamed_connection_is_refused_by_its_label() {
    let plan = plan_with_features(
        vec![square_room("a"), square_room("b")],
        vec![opening("a", "b")],
        vec![switch_in("a", "a->b")],
    );
    assert_invalid(&plan, "opens connection 'a->b', which carries no id");
}

#[test]
fn a_feature_bearing_plan_validates() {
    let rooms = vec![square_room("a"), square_room("b")];
    let mut named = opening("a", "b");
    named.id = Some("d1".to_owned());
    let plan = plan_with_features(
        rooms,
        vec![named],
        vec![
            exit_in("b"),
            switch_in("a", "d1"),
            lift_in("b"),
            teleporter("a", "b"),
        ],
    );
    plan.validate()
        .expect("features are reference-checked only");
}

#[test]
fn plans_past_the_room_budget_are_refused() {
    let rooms = (0_i64..4_097)
        .map(|index| square_room(&format!("room-{index}")))
        .collect();
    let plan = plan(rooms, vec![]);
    let error = plan.validate().expect_err("4097 rooms must be refused");
    assert_eq!(error.code(), "limit_exceeded", "room budget code");
    assert!(
        error.to_string().contains("4097"),
        "message names the count: {error}"
    );
}

#[test]
fn plans_past_the_connection_budget_are_refused() {
    let connections = (0_i64..4_097).map(|_| opening("a", "b")).collect();
    let plan = plan(vec![square_room("a"), square_room("b")], connections);
    let error = plan
        .validate()
        .expect_err("4097 connections must be refused");
    assert_eq!(error.code(), "limit_exceeded", "connection budget code");
    assert!(
        error.to_string().contains("4097"),
        "message names the count: {error}"
    );
}

#[test]
fn plans_past_the_feature_budget_are_refused() {
    let features = (0_i64..4_097).map(|_| exit_in("a")).collect();
    let plan = plan_with_features(vec![square_room("a")], vec![], features);
    let error = plan.validate().expect_err("4097 features must be refused");
    assert_eq!(error.code(), "limit_exceeded", "feature budget code");
    assert!(
        error.to_string().contains("4097"),
        "message names the count: {error}"
    );
}

#[test]
fn a_happy_plan_validates() {
    let courtyard = room_with(
        "courtyard",
        Shape::Polygon {
            points: vec![(0_i64, 0_i64), (256, 0), (256, 256), (0, 256)],
        },
    );
    let lab = square_room("lab");
    let mut door = opening("courtyard", "lab");
    door.id = Some("d1".to_owned());
    door.kind = ConnectionKind::Door {
        style: DoorStyle::Locked(KeyColor::Blue),
    };
    let mut stairs = opening("lab", "courtyard");
    stairs.kind = ConnectionKind::Stairs {
        step_height: 16_i32,
        step_depth: 24_i32,
    };
    let mut plan = plan(vec![courtyard, lab], vec![door, stairs]);
    plan.wall_thickness = 0_i64;
    plan.features = vec![
        exit_in("courtyard"),
        switch_in("lab", "d1"),
        lift_in("lab"),
        teleporter("courtyard", "lab"),
    ];
    plan.validate().expect("the plan is well-formed");
}

/// The build tool reads the offending wire row off a refusal's
/// [`PlanRef`]; every structural pass carries one for the element it was
/// checking, so no message text has to be parsed.
#[test]
fn every_structural_refusal_carries_its_plan_element() {
    let flat = room_with(
        "cellar",
        Shape::Rect {
            x: 0,
            y: 0,
            w: 64,
            h: 0,
        },
    );
    let room = plan(vec![square_room("a"), flat], vec![])
        .validate()
        .expect_err("a 0-high room is refused");
    assert_eq!(
        room.plan_ref(),
        Some(PlanRef::room("cellar")),
        "the room row the caller must fix: {room}"
    );

    let connection = plan(
        vec![square_room("a"), square_room("b")],
        vec![opening("a", "ghost")],
    )
    .validate()
    .expect_err("an unknown endpoint is refused");
    assert_eq!(
        connection.plan_ref(),
        Some(PlanRef::connection("a->ghost")),
        "the unnamed connection's derived label: {connection}"
    );

    let ghost = plan_with_features(vec![square_room("a")], vec![], vec![exit_in("ghost")])
        .validate()
        .expect_err("a feature of no room is refused");
    assert_eq!(
        ghost.plan_ref(),
        Some(PlanRef::feature("ghost")),
        "the feature row names the room it cannot find: {ghost}"
    );

    let orphan = plan_with_features(vec![square_room("a")], vec![], vec![switch_in("a", "nope")])
        .validate()
        .expect_err("a switch of no connection is refused");
    assert_eq!(
        orphan.plan_ref(),
        Some(PlanRef::feature("nope")),
        "the feature row names the connection it cannot find: {orphan}"
    );
}

/// A plan-wide refusal names no element, and so carries no reference for
/// the build tool to turn into a path.
#[test]
fn a_plan_wide_refusal_carries_no_element() {
    let empty = plan(vec![], vec![])
        .validate()
        .expect_err("an empty plan is refused");
    assert_eq!(empty.plan_ref(), None, "nothing to point at: {empty}");

    let mut negative = plan(vec![square_room("a")], vec![]);
    negative.wall_thickness = -4_i64;
    let error = negative
        .validate()
        .expect_err("a negative thickness is refused");
    assert_eq!(
        error.plan_ref(),
        None,
        "the request's own field, not a row: {error}"
    );
}
