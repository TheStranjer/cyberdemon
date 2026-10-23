//! Refusal tests: the unusable [`Units`] settings [`Units::check`] and
//! [`transform_plan`](super::transform_plan) reject, and the plan path each
//! overflowing or collapsed coordinate is named by.

use super::{
    assert_argument, assert_plan_refused, assert_refused, opening, plan_features, plan_with,
    polygon_room, rect_room, sample_plan, teleporter,
};
use crate::plan::units::{Units, transform_plan};

#[test]
fn an_overflowing_teleporter_square_names_its_path() {
    let units = Units {
        origin_x: i64::MAX - 1_000_i64,
        ..Units::default()
    };
    let plan = plan_features(vec![teleporter(Some((5_000, 32)), None)]);
    assert_plan_refused(&plan, &units, 1_i64, "features[0].pad");
}

#[test]
fn a_zero_denominator_is_refused_naming_scale_den() {
    let units = Units {
        scale_den: 0_i64,
        ..Units::default()
    };
    assert_refused(&units, "scale_den");
}

#[test]
fn a_zero_numerator_is_refused_naming_scale_num() {
    let units = Units {
        scale_num: 0_i64,
        ..Units::default()
    };
    assert_refused(&units, "scale_num");
}

#[test]
fn a_zero_or_negative_grid_is_refused_naming_grid() {
    for grid in [0_i64, -8_i64] {
        assert_refused(
            &Units {
                grid,
                ..Units::default()
            },
            "grid",
        );
        let refused = transform_plan(&sample_plan(), &Units::default(), grid)
            .expect_err("snapping needs a grid");
        assert_argument(&refused, "grid");
    }
    assert_eq!(
        Units::default().point(0_i64, 4_i64, 4_i64),
        None,
        "a grid below 1 snaps nothing"
    );
}

#[test]
fn an_overflowing_rectangle_corner_names_its_field() {
    let units = Units {
        scale_num: i64::MAX,
        ..Units::default()
    };
    let plan = plan_with(vec![rect_room("a", (100, 0), (64, 64))], vec![]);
    assert_plan_refused(&plan, &units, 1_i64, "rooms[0].rect.x");
}

#[test]
fn an_overflowing_polygon_point_names_its_index() {
    let units = Units {
        scale_num: i64::MAX,
        ..Units::default()
    };
    let plan = plan_with(
        vec![polygon_room("a", vec![(0, 0), (64, 0), (0, 64)])],
        vec![],
    );
    assert_plan_refused(&plan, &units, 1_i64, "rooms[0].polygon[1]");
}

#[test]
fn an_overflowing_connection_placement_names_its_path() {
    let units = Units {
        origin_x: i64::MAX - 1_000_i64,
        ..Units::default()
    };
    let plan = plan_with(
        vec![
            rect_room("a", (0, 0), (64, 64)),
            rect_room("b", (96, 0), (64, 64)),
        ],
        vec![opening("a", "b", Some((5_000, 32)))],
    );
    assert_plan_refused(&plan, &units, 1_i64, "connections[0].at");
}

#[test]
fn a_rectangle_the_grid_collapses_is_refused_naming_its_extent() {
    let units = Units {
        grid: 4_096_i64,
        ..Units::default()
    };
    let plan = plan_with(vec![rect_room("a", (0, 0), (64, 64))], vec![]);
    assert_plan_refused(&plan, &units, 4_096_i64, "rooms[0].rect.w");
}
