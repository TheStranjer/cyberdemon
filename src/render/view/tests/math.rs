//! Exact maths: what a column ray pierces and misses, how the `i128`
//! fractions order, what a camera projects around its horizon, how a pitch
//! shears that horizon, the focal length across the field-of-view range,
//! the documented pitch limits and the ray parameters a plane owns.

use alloc::string::ToString as _;
use core::cmp::Ordering;

use super::{east, frac, horizontal, raw};
use crate::fixed::Angle;
use crate::render::view::projection::{Projection, focal_length};
use crate::render::view::texels::{pierce, plane_point};
use crate::render::view::{DIRECTION_SCALE, MAX_PITCH, MIN_PITCH, ONE_16, ViewCamera, check_pitch};

#[test]
fn pierce_reports_exact_fractions_and_the_facing_side() {
    let edge = horizontal(0, 0, 10);
    // From (0,5) heading north-east: the segment is behind the ray.
    let away = pierce((0, 5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge);
    assert!(away.is_none(), "a crossing at t < 0 is no hit");
    // From (0,5) heading south-east: (5,0) at s = 1/2, back side.
    let (t, s, front) =
        pierce((0, 5), (DIRECTION_SCALE, -DIRECTION_SCALE), &edge).expect("a crossing");
    assert_eq!(t.times(ONE_16), raw(5), "t = 5");
    assert_eq!(
        s.times(ONE_16),
        raw(1).checked_div(2_i128).expect("test divisor"),
        "s = 1/2"
    );
    assert!(!front, "the ray crosses the back side from above");
    // From (0,-5) heading north-east: the same line from the front.
    let (_, _, faced_front) =
        pierce((0, -5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge).expect("a crossing");
    assert!(faced_front, "the front side bounds the right of v1 -> v2");
}

#[test]
fn pierce_misses_parallel_and_outside_segments() {
    let edge = horizontal(0, 0, 10);
    let parallel = pierce((0, 5), east(), &edge);
    assert!(parallel.is_none(), "an east ray never meets an east edge");
    // Crossing the segment's line far beyond v2: s > 1 rejects it.
    let short = horizontal(0, 10, 2);
    let beyond = pierce((0, 0), (DIRECTION_SCALE, DIRECTION_SCALE), &short);
    assert!(beyond.is_none(), "the crossing at t = 10 lands at s = 5");
    let behind = pierce((20, 5), (DIRECTION_SCALE, DIRECTION_SCALE), &edge);
    assert!(behind.is_none(), "the segment lies behind this ray");
}

#[test]
fn fractions_order_by_cross_multiplication() {
    let half = frac(1, 2);
    let fifth = frac(1, 5);
    let big = frac(12, 10);
    assert_eq!(fifth.cmp_value(half), Ordering::Less);
    assert_eq!(half.cmp_value(frac(2, 4)), Ordering::Equal);
    assert_eq!(big.cmp_value(half), Ordering::Greater);
    let mut values = vec![big, fifth, half];
    values.sort_unstable_by(|a, b| a.cmp_value(*b));
    assert_eq!(values, vec![fifth, half, big], "near to far");
}

#[test]
fn heights_project_around_the_horizon() {
    let camera = ViewCamera {
        x: 64,
        y: 128,
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    };
    let proj = Projection::new(&camera, 160, 100);
    assert_eq!(proj.horizon, raw(50), "level pitch centres the view");
    assert_eq!(proj.focal2, raw(160), "cot(45) leaves focal2 = W");
    let t = frac(12, 10);
    assert_eq!(proj.row(41, t), 50, "the eye height lands on the horizon");
    assert_eq!(proj.row(32, t), 53, "9 below the eye projects 3.75 down");
    assert_eq!(proj.row(128, t), 13, "87 above the eye projects 36.25 up");
    assert_eq!(proj.depth16(t), raw(192), "t * focal2 is the depth");
    let h = proj.height_at(53, t);
    assert!(
        h.saturating_sub(raw(32)).abs() < raw(2),
        "the step row measures back to about 32: {h}"
    );
}

#[test]
fn pitch_shears_the_horizon_downwards() {
    let flat = ViewCamera {
        x: 0,
        y: 0,
        z: 0,
        angle: 0,
        pitch: 0,
        fov: 90,
    };
    let up = ViewCamera { pitch: 30, ..flat };
    let proj = Projection::new(&up, 160, 100);
    let lift = raw(50).saturating_add(
        raw(160)
            .saturating_mul(37_837_i128)
            .checked_shr(17)
            .unwrap_or(0),
    );
    assert!(
        proj.horizon.saturating_sub(lift).abs() < raw(2),
        "tan(30) pushes the horizon about 46 rows down: {}",
        proj.horizon
    );
    assert!(
        proj.horizon > raw(50),
        "a positive pitch moves the horizon down the screen"
    );
}

#[test]
fn the_focal_length_spans_the_field_of_view_range() {
    let wide = focal_length(160, Angle::from_millidegrees(85_000));
    assert!(
        wide.saturating_sub(raw(14)).abs() < raw(2),
        "cot(85) shrinks focal2: {wide}"
    );
    let narrow = focal_length(160, Angle::from_millidegrees(5_000));
    assert!(
        narrow.saturating_sub(raw(1_829)).abs() < raw(20),
        "cot(5) stretches focal2: {narrow}"
    );
}

#[test]
fn the_pitch_range_is_the_documented_one() {
    for pitch in [MIN_PITCH, 0_i32, MAX_PITCH] {
        assert!(check_pitch(pitch).is_ok(), "{pitch} is legal");
    }
    for pitch in [
        MIN_PITCH.saturating_sub(1),
        MAX_PITCH.saturating_add(1),
        90_i32,
    ] {
        let error = check_pitch(pitch).expect_err("outside the range");
        assert_eq!(error.code(), "invalid_argument", "pitch {pitch} code");
        assert!(
            error.to_string().contains("-60..=60"),
            "the message shows the range: {error}"
        );
    }
}

#[test]
fn plane_ray_parameters_land_on_the_row_geometry() {
    // An odd width keeps column 80 exactly on the camera axis, and a
    // level pitch in an 84-row frame puts the horizon at row 42.
    let camera = ViewCamera {
        x: 100,
        y: 200,
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    };
    let proj = Projection::new(&camera, 161, 84);
    assert_eq!(proj.horizon, raw(42), "level pitch centres the view");
    let d = proj.direction(80);
    assert_eq!(
        d,
        (raw(161).saturating_mul(ONE_16), 0_i128),
        "the centre column runs due east"
    );
    let floor = proj
        .plane_t(0, 83)
        .expect("the bottom row sees the floor below the eye");
    assert_eq!(
        floor.times(ONE_16),
        raw(1).checked_div(2_i128).expect("test divisor"),
        "t = 1/2 at the bottom row"
    );
    assert_eq!(
        proj.depth16(floor),
        raw(161).checked_div(2_i128).expect("test divisor"),
        "80.5 units of perpendicular depth"
    );
    let point = plane_point((100, 200), d, floor);
    assert_eq!(
        point.0,
        raw(361).checked_div(2_i128).expect("test divisor"),
        "on the floor, 80.5 map units east of the camera's x"
    );
    assert_eq!(point.1, raw(200), "no north-south offset: straight ahead");
    assert_eq!(
        point.0.saturating_sub(raw(100)),
        raw(161).checked_div(2_i128).expect("test divisor"),
        "the point sits 80.5 units forward of the column under the camera"
    );
}

#[test]
fn planes_the_camera_cannot_see_have_no_ray_parameter() {
    let camera = ViewCamera {
        x: 0,
        y: 0,
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    };
    let proj = Projection::new(&camera, 161, 84);
    assert!(
        proj.plane_t(0, 0).is_none(),
        "a floor below the eye owns no row above the horizon"
    );
    assert!(
        proj.plane_t(0, 42).is_none(),
        "the horizon row itself is skipped"
    );
    assert!(
        proj.plane_t(128, 83).is_none(),
        "a ceiling above the eye owns no row below the horizon"
    );
    assert!(
        proj.plane_t(41, 83).is_none(),
        "a plane at eye height projects nowhere"
    );
    assert!(
        proj.plane_t(128, 0).is_some(),
        "the ceiling owns the top row"
    );
}
