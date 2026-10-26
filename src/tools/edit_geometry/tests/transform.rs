//! The `transform_selection` request's own checks: the operations, the
//! rotation shape, the step label and the selection defaults.

use super::{code, rotate_of, transform_args};
use crate::fixed::Angle;
use crate::tools::edit_geometry::transform::{
    counted, operations_of, rotation_of, selection_request,
};
use crate::wad::map::transform::{Point, Scale};

#[test]
fn a_transform_needs_at_least_one_operation() {
    let none = operations_of(&transform_args(r#"{"selection": {"all": true}}"#))
        .expect_err("no operation does nothing");
    assert_eq!(code(&none), "invalid_argument", "no-operation code");
    assert!(
        none.to_string().contains("scale, rotate and translate"),
        "the message names the three: {none}"
    );
    let ops = operations_of(&transform_args(
        r#"{"selection": {"all": true}, "translate": {"dx": 0, "dy": 0}}"#,
    ))
    .expect("even the zero shift is an operation");
    assert_eq!(
        ops.translate,
        Some(Point { x: 0, y: 0 }),
        "the shift reaches the operations"
    );
}

#[test]
fn operations_validate_scale_fractions_and_rotate_shapes() {
    let bad_scale =
        transform_args(r#"{"selection": {"all": true}, "scale": {"num": 0, "den": 2}}"#);
    let zero = operations_of(&bad_scale).expect_err("0/2 is no factor");
    assert_eq!(code(&zero), "invalid_argument", "zero numerator code");
    let both = operations_of(&transform_args(
        r#"{"selection": {"all": true},
            "rotate": {"degrees": 90, "millidegrees": 90000}}"#,
    ))
    .expect_err("two rotation numbers are ambiguous");
    assert_eq!(code(&both), "invalid_argument", "two-of-one code");
    assert!(
        both.to_string().contains("exactly one"),
        "the message says what to pass: {both}"
    );
    let combined = operations_of(&transform_args(
        r#"{"selection": {"all": true},
            "scale": {"num": 3, "den": 2},
            "rotate": {"millidegrees": 45500},
            "translate": {"dx": 8, "dy": -8}}"#,
    ))
    .expect("all three operations parse");
    assert_eq!(
        combined.scale,
        Some(Scale { num: 3, den: 2 }),
        "the fraction widens intact"
    );
    assert_eq!(
        combined.rotation.map(|rotation| rotation.degrees),
        Some(46),
        "thing facings take the rounded degree"
    );
    assert_eq!(
        combined.translate,
        Some(Point { x: 8, y: -8 }),
        "the shift comes through"
    );
}

#[test]
fn rotation_reads_its_one_number() {
    let degrees = rotation_of(&rotate_of(r#"{"degrees": -90}"#)).expect("whole degrees");
    assert_eq!(degrees.degrees, -90, "the facing delta passes through");
    assert_eq!(degrees.angle, Angle::from_degrees(-90), "and the angle");
    let fine = rotation_of(&rotate_of(r#"{"millidegrees": 45000}"#)).expect("millidegrees");
    assert_eq!(
        fine.angle,
        Angle::from_millidegrees(45_000),
        "the exact angle"
    );
    assert!(
        rotation_of(&rotate_of("{}")).is_err(),
        "neither number refuses"
    );
}

#[test]
fn the_step_label_counts_each_kind_by_its_own_form() {
    assert_eq!(
        counted(0, "vertex", "vertices"),
        "0 vertices",
        "zero is plural"
    );
    assert_eq!(counted(1, "thing", "things"), "1 thing", "one is singular");
    assert_eq!(
        counted(4, "vertex", "vertices"),
        "4 vertices",
        "irregular plural"
    );
}

#[test]
fn the_selection_defaults_to_including_sector_things() {
    let bare = selection_request(transform_args(r#"{"selection": {}}"#).selection);
    assert!(!bare.all, "all is off by default");
    assert!(
        bare.include_things_in_sectors,
        "the include switch is on by default"
    );
    assert!(
        bare.vertices.is_empty()
            && bare.linedefs.is_empty()
            && bare.sectors.is_empty()
            && bare.things.is_empty(),
        "absent lists stay empty"
    );
    let switched = transform_args(
        r#"{"selection": {"sectors": [1], "things": [0], "all": false,
            "include_things_in_sectors": false}}"#,
    );
    let request = selection_request(switched.selection);
    assert_eq!(request.sectors, vec![1_usize], "the lists pass through");
    assert_eq!(request.things, vec![0_usize]);
    assert!(
        !request.include_things_in_sectors,
        "the switch reaches the request"
    );
}
