//! Mapping tests: the round trip between [`Units::to_map`] and
//! [`Units::to_image`], the identity default, scaling, the `y` mirror,
//! grid snapping, which plan fields travel and which stay map-unit sizes.

use super::{
    assert_plan_refused, opening, plan_features, plan_with, polygon_room, rect_room, sample_plan,
    teleporter,
};
use crate::geom::Pt;
use crate::plan::model::{Connection, ConnectionKind, ExitStyle, Feature, Shape, Side};
use crate::plan::units::{Units, snap, transform_plan};
use crate::wad::map::features::LiftStyle;

/// A west-wall `exit` in room `a` standing at `at`.
fn exit_at(at: Option<Pt>) -> Feature {
    Feature::Exit {
        room: "a".to_owned(),
        style: ExitStyle::Switch,
        secret: false,
        wall: Some(Side::West),
        at,
    }
}

/// A west-wall `switch` in room `a` opening `d1`, standing at `at`.
fn switch_at(at: Option<Pt>) -> Feature {
    Feature::Switch {
        room: "a".to_owned(),
        opens: "d1".to_owned(),
        wall: Some(Side::West),
        repeatable: false,
        at,
    }
}

/// The plan point a wall-mounted row carries: the `at` of an `exit`
/// or a `switch`, nothing for the rows that mount no panel.
const fn wall_point(feature: &Feature) -> Option<Pt> {
    match *feature {
        Feature::Exit { at, .. } | Feature::Switch { at, .. } => at,
        Feature::Lift { .. } | Feature::Teleporter { .. } => None,
    }
}

#[test]
fn to_map_and_to_image_invert_each_other() {
    let units = Units {
        scale_num: 4_i64,
        origin_x: 10_i64,
        origin_y: -20_i64,
        flip_y: true,
        ..Units::default()
    };
    for (px, py) in [(0_i64, 0_i64), (12_i64, 30_i64), (-8_i64, 7_i64)] {
        let (x, y) = units
            .to_map(px, py)
            .expect("small scaled coordinates stay in range");
        assert_eq!(
            units.to_image(x, y),
            Some((px, py)),
            "an exact pixel round-trips at scale 4 with the mirror"
        );
    }
    assert_eq!(
        units.to_map(0_i64, 4_i64),
        Some((10_i64, -36_i64)),
        "the unsnapped direction matches point(1, x, y)"
    );
    assert_eq!(
        units.to_image(12_i64, -20_i64),
        Some((1_i64, 0_i64)),
        "a map point between pixels rounds to the nearest, halves away from zero"
    );
    let shrink = Units {
        scale_num: 1_i64,
        scale_den: 8_i64,
        ..Units::default()
    };
    assert_eq!(
        shrink.to_image(7_i64, -7_i64),
        Some((56_i64, -56_i64)),
        "a fractional scale divides out by eight"
    );
    assert_eq!(
        Units::default().to_map(3_i64, -3_i64),
        Some((3_i64, -3_i64)),
        "the identity carries pixels through"
    );
}

#[test]
fn the_default_units_are_the_identity() {
    let units = Units::default();
    units.check().expect("the identity is usable");
    assert_eq!(
        units.point(1_i64, -7_i64, 23_i64),
        Some((-7_i64, 23_i64)),
        "a point passes through"
    );
    let plan = sample_plan();
    let moved = transform_plan(&plan, &units, 1_i64).expect("the identity runs");
    assert_eq!(plan, moved, "an identity transform changes nothing");
}

#[test]
fn a_scale_of_four_turns_a_hundred_pixels_into_four_hundred_units() {
    let units = Units {
        scale_num: 4_i64,
        ..Units::default()
    };
    let plan = plan_with(vec![rect_room("hall", (10, 20), (100, 50))], vec![]);
    let moved = transform_plan(&plan, &units, 1_i64).expect("scale 4 runs");
    assert_eq!(
        moved.rooms.first().expect("one room").shape,
        Shape::Rect {
            x: 40,
            y: 80,
            w: 400,
            h: 200
        },
        "the 100x50 rect becomes 400x200"
    );
}

#[test]
fn flip_y_mirrors_about_the_origin() {
    let units = Units {
        origin_y: 500_i64,
        flip_y: true,
        ..Units::default()
    };
    assert_eq!(
        units.point(1_i64, 6_i64, 100_i64),
        Some((6_i64, 400_i64)),
        "y is measured down from the origin"
    );
    let plan = plan_with(vec![rect_room("attic", (0, 100), (64, 32))], vec![]);
    let moved = transform_plan(&plan, &units, 1_i64).expect("a mirror runs");
    assert_eq!(
        moved.rooms.first().expect("one room").shape,
        Shape::Rect {
            x: 0,
            y: 368,
            w: 64,
            h: 32
        },
        "the mirrored rect keeps a positive extent below its old top"
    );
}

#[test]
fn snapping_pulls_every_coordinate_to_the_nearest_grid_line() {
    let units = Units {
        grid: 16_i64,
        ..Units::default()
    };
    let plan = plan_with(vec![rect_room("a", (10, 6), (100, 50))], vec![]);
    let moved = transform_plan(&plan, &units, 16_i64).expect("a 16 grid runs");
    assert_eq!(
        moved.rooms.first().expect("one room").shape,
        Shape::Rect {
            x: 16,
            y: 0,
            w: 96,
            h: 64
        },
        "10 goes to 16, 6 to 0, 110 to 112 and the 56 tie to 64"
    );
    let Shape::Rect {
        x: left,
        y: bottom,
        w: width,
        h: height,
    } = moved.rooms.first().expect("one room").shape
    else {
        panic!("a rect room stays a rect");
    };
    for value in [left, bottom, width, height] {
        assert_eq!(snap(value, 16_i64), Some(value), "{value} is grid-aligned");
    }
}

#[test]
fn snapping_rounds_halves_away_from_zero_on_both_sides() {
    let units = Units::default();
    assert_eq!(
        units.point(16_i64, 10_i64, 6_i64),
        Some((16_i64, 0_i64)),
        "nearest line wins"
    );
    assert_eq!(
        units.point(16_i64, -10_i64, -24_i64),
        Some((-16_i64, -32_i64)),
        "negatives snap outward at the tie"
    );
    assert_eq!(
        units.point(8_i64, 12_i64, -4_i64),
        Some((16_i64, -8_i64)),
        "the ties 12 and -4 move away from zero"
    );
}

#[test]
fn a_fractional_scale_rounds_once_per_coordinate() {
    let units = Units {
        scale_num: 2_i64,
        scale_den: 4_i64,
        origin_x: 10_i64,
        origin_y: 20_i64,
        flip_y: true,
        ..Units::default()
    };
    assert_eq!(
        units.point(8_i64, 30_i64, 30_i64),
        Some((24_i64, 8_i64)),
        "30 scaled by a half is 15, then 25 and 5, snapped to 24 and 8"
    );
}

#[test]
fn polygon_points_and_connection_positions_are_transformed() {
    let units = Units {
        scale_num: 2_i64,
        ..Units::default()
    };
    let plan = plan_with(
        vec![
            polygon_room("court", vec![(0, 0), (64, 0), (32, 48)]),
            rect_room("lab", (96, 0), (64, 64)),
        ],
        vec![
            opening("court", "lab", Some((96, 32))),
            opening("lab", "court", None),
        ],
    );
    let moved = transform_plan(&plan, &units, 1_i64).expect("scale 2 runs");
    assert_eq!(
        moved.rooms.first().expect("one room").shape,
        Shape::Polygon {
            points: vec![(0, 0), (128, 0), (64, 96)]
        },
        "every polygon point is transformed"
    );
    assert_eq!(
        moved.connections.first().expect("one connection").at,
        Some((192, 64)),
        "the placement follows the rooms"
    );
    assert_eq!(
        moved.connections.get(1).expect("second connection").at,
        None,
        "an unplaced connection stays unplaced"
    );
    moved.validate().expect("the scaled plan still validates");
}

#[test]
fn connection_sizes_and_wall_thickness_stay_in_map_units() {
    let units = Units {
        scale_num: 4_i64,
        grid: 8_i64,
        ..Units::default()
    };
    let window = Connection {
        kind: ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32,
        },
        ..opening("a", "b", Some((80, 32)))
    };
    let stairs = Connection {
        kind: ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32,
        },
        ..opening("b", "a", None)
    };
    let plan = plan_with(
        vec![
            rect_room("a", (0, 0), (64, 64)),
            rect_room("b", (96, 0), (64, 64)),
        ],
        vec![window, stairs],
    );
    let moved = transform_plan(&plan, &units, 8_i64).expect("the transform runs");
    assert_eq!(moved.wall_thickness, 16_i64, "the wall is a map-unit size");
    let glass = moved.connections.first().expect("the window");
    assert_eq!(glass.width, 48_i64, "an opening width is a map-unit size");
    assert_eq!(
        glass.kind,
        ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32
        },
        "sill and lintel are heights"
    );
    assert_eq!(
        moved.connections.get(1).expect("the stairs").kind,
        ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32
        },
        "step height and depth are sizes"
    );
}

#[test]
fn teleporter_squares_scale_with_the_rooms() {
    let units = Units {
        scale_num: 4_i64,
        ..Units::default()
    };
    let plan = plan_features(vec![teleporter(Some((10, 12)), None)]);
    let moved = transform_plan(&plan, &units, 8_i64).expect("scale 4 runs");
    assert_eq!(
        moved.features.first().expect("one feature"),
        &teleporter(Some((40_i64, 48_i64)), None),
        "the pad travels like a placement, an absent square stays absent"
    );
}

#[test]
fn teleporter_squares_mirror_and_snap_like_placements() {
    let units = Units {
        scale_num: 2_i64,
        origin_y: 100_i64,
        flip_y: true,
        ..Units::default()
    };
    let plan = plan_features(vec![teleporter(Some((5, 10)), Some((-14, 30)))]);
    let moved = transform_plan(&plan, &units, 8_i64).expect("the mirror runs");
    assert_eq!(
        moved.features.first().expect("one feature"),
        &teleporter(Some((8_i64, 80_i64)), Some((-32_i64, 40_i64))),
        "10 goes to 80 below the origin, -28 snaps away from zero to -32"
    );
}

#[test]
fn a_wall_point_travels_with_the_rooms() {
    let units = Units {
        scale_num: 4_i64,
        ..Units::default()
    };
    let rows = [
        (exit_at(Some((10, 12))), Some((40_i64, 48_i64))),
        (switch_at(Some((6, 8))), Some((24_i64, 32_i64))),
        (exit_at(None), None),
    ];
    for (row, want) in rows {
        let moved = transform_plan(&plan_features(vec![row]), &units, 8_i64).expect("scale 4 runs");
        let got = wall_point(moved.features.first().expect("one feature"));
        assert_eq!(got, want, "the point travels like a placement");
    }
}

#[test]
fn a_wall_point_the_transform_cannot_carry_names_its_path() {
    let units = Units {
        origin_x: i64::MAX - 1_000_i64,
        ..Units::default()
    };
    for row in [exit_at(Some((5_000, 32))), switch_at(Some((5_000, 32)))] {
        assert_plan_refused(&plan_features(vec![row]), &units, 1_i64, "features[0].at");
    }
}

#[test]
fn a_lifts_low_floor_is_a_height_and_stays_put() {
    let units = Units {
        scale_num: 4_i64,
        ..Units::default()
    };
    let lift = Feature::Lift {
        room: "a".to_owned(),
        low_floor: Some(-64_i32),
        style: LiftStyle::WalkTrigger,
        repeatable: false,
    };
    let plan = plan_features(vec![lift.clone()]);
    let moved = transform_plan(&plan, &units, 8_i64).expect("scale 4 runs");
    assert_eq!(moved.features, vec![lift], "no coordinate, no transform");
}
