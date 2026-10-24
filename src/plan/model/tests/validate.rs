//! Validation tests: every structural refusal
//! [`Plan::validate`](crate::plan::model::Plan::validate) makes,
//! from the empty plan through the shape, connection and feature checks
//! to the `u32` label budgets, and the plans it accepts.

use super::{
    assert_invalid, exit_in, lift_in, opening, plan, plan_with_features, room_with, square_room,
    switch_in, teleporter,
};
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
