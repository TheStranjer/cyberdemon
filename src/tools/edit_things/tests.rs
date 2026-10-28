//! Unit tests for the thing editing tools: the family and range rules and
//! the append step in [`crate::tools::edit_things`], and the request
//! validation, flag resolution and coordinate math of `edit_thing` in
//! [`crate::tools::edit_things::edit`].

use serde_json::from_str;

use super::edit::{
    EditPlan, EditThingParams, edited_thing, has_change, new_angle, new_coord, validate_edit,
};
use super::{
    AddThingParams, COORD_MAX, COORD_MIN, DEGREES, TYPE_MAX, add_in_place, check_binary_ranges,
    check_doom_plain, default_bits, fits_field, new_thing,
};
use crate::error::CyberError;
use crate::tools::maps::params::EditBits;
use crate::wad::classify::MapFormat;
use crate::wad::map::{FlagFamily, Map, Thing};
use crate::wad::name::LumpName;

/// The error code of `error`, for compact asserts.
fn code(error: &CyberError) -> &str {
    error.code()
}

/// An edit request parsed from `json`.
fn edit_args(json: &str) -> EditThingParams {
    from_str(json).expect("test edit requests parse")
}

/// An add request parsed from `json`.
fn add_args(json: &str) -> AddThingParams {
    from_str(json).expect("test add requests parse")
}

/// A thing with no `Hexen`-only fields for the range and family checks.
fn plain(x: i32, y: i32, kind: i32) -> Thing {
    Thing {
        x,
        y,
        kind,
        ..Default::default()
    }
}

#[test]
fn the_default_flags_are_the_all_present_bits_per_family() {
    assert_eq!(default_bits(FlagFamily::Doom), 7_u32, "easy|medium|hard");
    assert_eq!(
        default_bits(FlagFamily::Hexen),
        2_023_u32,
        "0x7E7: skills, classes and modes"
    );
}

#[test]
fn angles_normalise_and_coordinates_shift() {
    assert_eq!(
        new_angle(0_i32, Some(450_i32), None).expect("450 is nameable"),
        90_i32,
        "the turn lands in 0..=359"
    );
    assert_eq!(
        new_angle(350_i32, Some(-90_i32), None).expect("-90 is nameable"),
        270_i32,
        "rem_euclid normalises negatives"
    );
    assert_eq!(
        new_angle(90_i32, None, Some(405_i32)).expect("a full turn plus a quarter"),
        135_i32,
        "deltas normalise too"
    );
    assert_eq!(
        new_coord("x", 32_000_i32, None, Some(1_000_i32)).expect("the sum fits"),
        33_000_i32,
        "a delta shifts the coordinate"
    );
    assert_eq!(DEGREES, 360_i32, "the full turn");
}

#[test]
fn coordinate_overflow_is_refused_and_absolute_wins() {
    let error = new_coord("y", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
    assert_eq!(code(&error), "invalid_argument", "overflow code");
    assert!(
        error.to_string().contains('y'),
        "the field is named: {error}"
    );
    assert_eq!(
        new_coord("x", 10_i32, Some(-5_i32), None).expect("absolute wins"),
        -5_i32,
        "an absolute value passes through untouched"
    );
}

#[test]
fn doom_family_things_stay_plain() {
    let with_tid = Thing {
        tid: 5_i32,
        ..plain(0_i32, 0_i32, 1_i32)
    };
    let error =
        check_doom_plain(FlagFamily::Doom, &with_tid).expect_err("a tid on a Doom map refuses");
    assert_eq!(code(&error), "invalid_argument", "family rule code");
    assert!(
        error
            .to_string()
            .contains("Doom-format things have no tid/z/special/args; use a Hexen or UDMF map"),
        "the documented message: {error}"
    );
    let extras = [
        Thing {
            z: 8_i32,
            ..plain(0_i32, 0_i32, 1_i32)
        },
        Thing {
            special: 1_i32,
            ..plain(0_i32, 0_i32, 1_i32)
        },
        Thing {
            args: [0_i32, 0_i32, 0_i32, 0_i32, 1_i32],
            ..plain(0_i32, 0_i32, 1_i32)
        },
    ];
    for extra in extras {
        assert!(
            check_doom_plain(FlagFamily::Doom, &extra).is_err(),
            "z/special/args refuse too"
        );
    }
    assert!(
        check_doom_plain(FlagFamily::Doom, &plain(0_i32, 0_i32, 1_i32)).is_ok(),
        "all zeros are plain"
    );
    let wild = Thing {
        tid: 9_i32,
        z: 64_i32,
        special: 2_i32,
        args: [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
        ..plain(0_i32, 0_i32, 1_i32)
    };
    assert!(
        check_doom_plain(FlagFamily::Hexen, &wild).is_ok(),
        "the Hexen family stores them all"
    );
}

#[test]
fn binary_ranges_are_checked_and_udmf_is_unbounded() {
    let error = fits_field("type", 70_000_i32, 0_i32, TYPE_MAX).expect_err("above u16 refuses");
    assert_eq!(code(&error), "invalid_argument", "range code");
    assert!(
        error.to_string().contains("0..=65535"),
        "the message shows the field range: {error}"
    );
    assert!(
        check_binary_ranges(MapFormat::Doom, &plain(COORD_MAX, COORD_MIN, TYPE_MAX)).is_ok(),
        "the i16 edges fit"
    );
    let over = plain(
        COORD_MAX.checked_add(1).expect("edge plus one"),
        0_i32,
        1_i32,
    );
    assert!(
        check_binary_ranges(MapFormat::Doom, &over).is_err(),
        "x above i16 refuses on binary Doom"
    );
    let big_special = Thing {
        special: TYPE_MAX,
        ..plain(0_i32, 0_i32, 1_i32)
    };
    assert!(
        check_binary_ranges(MapFormat::Hexen, &big_special).is_err(),
        "a Hexen special above u8 refuses"
    );
    let big_arg = Thing {
        args: [0_i32, 0_i32, 300_i32, 0_i32, 0_i32],
        ..plain(0_i32, 0_i32, 1_i32)
    };
    assert!(
        check_binary_ranges(MapFormat::Hexen, &big_arg).is_err(),
        "a Hexen arg above u8 refuses"
    );
    assert!(
        check_binary_ranges(MapFormat::Udmf, &plain(i32::MAX, i32::MIN, i32::MAX)).is_ok(),
        "UDMF stores full integers"
    );
    let doom_special = Thing {
        special: 300_i32,
        ..plain(0_i32, 0_i32, 1_i32)
    };
    assert!(
        check_binary_ranges(MapFormat::Doom, &doom_special).is_ok(),
        "the u8 script checks are Hexen-only (the family rule zeroes these earlier)"
    );
}

#[test]
fn new_things_take_defaults_and_normalised_angles() {
    let args = add_args(r#"{"type": 3001, "x": 8, "y": -8, "angle": 450}"#);
    let doom = new_thing(&args, FlagFamily::Doom).expect("a plain Doom thing");
    assert_eq!(
        (doom.kind, doom.x, doom.y, doom.angle),
        (3_001_i32, 8_i32, -8_i32, 90_i32),
        "the required fields and the normalised angle"
    );
    assert_eq!(doom.flags, 7_u32, "the Doom default");
    assert_eq!(doom.args, [0_i32; 5], "and zeros elsewhere");
    let named = add_args(
        r#"{"type": 1, "x": 0, "y": 0, "flags": ["ambush", "class1"], "z": 8, "tid": 3,
                "special": 1, "args": [1, 2, 3, 4, 5]}"#,
    );
    let hexen = new_thing(&named, FlagFamily::Hexen).expect("named flags");
    assert_eq!(hexen.flags, 8_u32 | 32_u32, "ambush|class1 from the table");
    assert_eq!(hexen.tid, 3_i32, "the parameters pass through");
    let plain_args = add_args(r#"{"type": 1, "x": 0, "y": 0}"#);
    let spawned = new_thing(&plain_args, FlagFamily::Hexen).expect("defaults resolve");
    assert_eq!(spawned.flags, 2_023_u32, "the Hexen default is 0x7E7");
}

#[test]
fn edits_replace_or_adjust_the_flag_bits() {
    let adjusting = EditBits {
        absolute: None,
        set: 8_u32,
        clear: 1_u32,
    };
    assert_eq!(adjusting.apply(7_u32), 14_u32, "easy clears, ambush sets");
    let replacing = EditBits {
        absolute: Some(3_u32),
        set: 8_u32,
        clear: 1_u32,
    };
    assert_eq!(replacing.apply(7_u32), 3_u32, "the absolute bitset wins");
}

#[test]
fn edited_things_combine_unchanged_fields_with_the_request() {
    let current = Thing {
        tid: 2_i32,
        x: 100_i32,
        y: -100_i32,
        z: 8_i32,
        angle: 90_i32,
        kind: 3_001_i32,
        flags: 7_u32,
        special: 0_i32,
        args: [0_i32; 5],
        extra: alloc::collections::BTreeMap::new(),
    };
    let args = edit_args(r#"{"index": 0, "dx": -30, "angle_delta": 45}"#);
    let plan = EditPlan::new(&args, vec![0_usize], FlagFamily::Hexen).expect("a flagless plan");
    let edited = edited_thing(&current, &plan).expect("the edit computes");
    assert_eq!((edited.x, edited.y), (70_i32, -100_i32), "dx moves x only");
    assert_eq!(edited.angle, 135_i32, "the turn stays in 0..=359");
    assert_eq!(edited.kind, 3_001_i32, "unasked fields stand");
    assert_eq!(edited.tid, 2_i32, "and the tid survives");
}

#[test]
fn edit_validation_names_conflicts_and_nothings() {
    let sorted = validate_edit(&edit_args(r#"{"index": [3, 1], "dx": 8, "dy": -8}"#))
        .expect("a list with deltas is fine");
    assert_eq!(sorted, vec![1_usize, 3], "sorted and deduplicated");
    let refusals = [
        r#"{"index": 0, "x": 1, "dx": 2}"#,
        r#"{"index": 0, "y": 1, "dy": 2}"#,
        r#"{"index": 0, "angle": 1, "angle_delta": 2}"#,
        r#"{"index": 0, "flags": 3, "set_flags": ["easy"]}"#,
        r#"{"index": 0, "flags": 3, "clear_flags": ["easy"]}"#,
        r#"{"index": [0, 1], "x": 4}"#,
        r#"{"index": [0, 1], "y": 4}"#,
        r#"{"index": 0}"#,
        r#"{"index": []}"#,
    ];
    for json in refusals {
        let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
    }
    assert!(
        has_change(&edit_args(r#"{"index": 0, "tid": 0}"#)),
        "tid counts"
    );
    assert!(
        has_change(&edit_args(r#"{"index": 0, "args": [0, 0, 0, 0, 0]}"#)),
        "args count"
    );
}

#[test]
fn an_added_thing_never_touches_the_geometry() {
    let mut map = Map::new_empty(
        LumpName::parse("MAP01").expect("a valid name"),
        MapFormat::Doom,
    );
    map.things.push(plain(0_i32, 0_i32, 1_i32));
    let args = add_args(r#"{"type": 3004, "x": 16, "y": 16}"#);
    let row = add_in_place(&mut map, &args).expect("the thing appends");
    assert_eq!(row.index, 1_usize, "after the existing one");
    assert_eq!(map.things.len(), 2, "and the list grew by one");
    assert!(map.vertices.is_empty(), "geometry stayed untouched");
}

#[test]
fn a_doom_namespace_udmf_map_takes_the_plain_rule_without_binary_ranges() {
    let mut map = Map::new_empty(
        LumpName::parse("MAP01").expect("a valid name"),
        MapFormat::Udmf,
    );
    map.udmf_namespace = Some("doom".to_owned());
    let wide = add_args(r#"{"type": 70000, "x": 40000, "y": 0}"#);
    let row = add_in_place(&mut map, &wide).expect("UDMF stores wide values");
    assert_eq!(row.detail.kind, 70_000_i32, "no u16 cap in TEXTMAP");
    assert_eq!(
        row.detail.flags, 7_u32,
        "the doom namespace is the Doom family"
    );
    let tidied = add_args(r#"{"type": 1, "x": 0, "y": 0, "tid": 3}"#);
    let error = add_in_place(&mut map, &tidied).expect_err("the tid refuses there too");
    assert_eq!(code(&error), "invalid_argument", "namespace family code");
    assert_eq!(map.things.len(), 1, "and nothing appended");
}
