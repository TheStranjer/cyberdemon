//! The op topic: every comparison over the fixture maps — the numeric and
//! text ops, case-insensitive `glob`, the flag and role names, the derived
//! sector, linedef, thing and sidedef fields, and the `bbox` and `sectors`
//! bounds of each kind.

use serde_json::{Value, json};

use super::{boxed, diagonal_map, field_query, filter, flaggy_map, hits, query, roley_map};
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::query::{ElementKind, Op, Within};

/// One op-coverage row: field, op, value and the expected hits.
type Case = (&'static str, Op, Value, Vec<usize>);

#[test]
fn every_numeric_op_filters_the_fixture_lights() {
    let map = two_room_map();
    let cases: [Case; 8] = [
        ("light", Op::Eq, json!(160_i64), vec![0]),
        ("light", Op::Ne, json!(160_i64), vec![1]),
        ("light", Op::Lt, json!(170_i64), vec![0]),
        ("light", Op::Le, json!(160_i64), vec![0]),
        ("light", Op::Gt, json!(170_i64), vec![1]),
        ("light", Op::Ge, json!(192_i64), vec![1]),
        ("light", Op::In, json!([160_i64, 192_i64]), vec![0, 1]),
        ("light", Op::NotIn, json!([160_i64]), vec![1]),
    ];
    for (field, op, value, want) in cases {
        let q = field_query(ElementKind::Sectors, filter(field, op, value.clone()));
        assert_eq!(hits(&map, &q), want, "{field} {op:?} {value}");
    }
}

#[test]
fn every_text_op_filters_the_fixture_flats() {
    let map = two_room_map();
    let cases: [Case; 5] = [
        ("floor_flat", Op::Eq, json!("FLOOR1"), vec![0, 1]),
        ("floor_flat", Op::Ne, json!("FLOOR1"), vec![]),
        ("floor_flat", Op::In, json!(["FLOOR1"]), vec![0, 1]),
        ("floor_flat", Op::NotIn, json!(["FLOOR1"]), vec![]),
        ("floor_flat", Op::Glob, json!("floor*"), vec![0, 1]),
    ];
    for (field, op, value, want) in cases {
        let q = field_query(ElementKind::Sectors, filter(field, op, value.clone()));
        assert_eq!(hits(&map, &q), want, "{field} {op:?} {value}");
    }
}

#[test]
fn glob_matches_case_insensitively_on_patterns_and_names() {
    let map = two_room_map();
    let lower = field_query(ElementKind::Things, filter("name", Op::Glob, json!("imp*")));
    assert_eq!(hits(&map, &lower), vec![1], "the pattern is lowercase");
    let question = field_query(
        ElementKind::Sectors,
        filter("floor_flat", Op::Glob, json!("FLOOR?")),
    );
    assert_eq!(hits(&map, &question), vec![0, 1], "? takes one character");
    let miss = field_query(
        ElementKind::Sectors,
        filter("floor_flat", Op::Glob, json!("NUKAGE*")),
    );
    assert!(hits(&map, &miss).is_empty(), "non-matching patterns drop");
}

#[test]
fn has_tests_named_linedef_and_thing_flags() {
    let map = flaggy_map();
    let blocking = field_query(
        ElementKind::Linedefs,
        filter("flags", Op::Has, json!("blocking")),
    );
    assert_eq!(
        hits(&map, &blocking),
        vec![0, 1, 3, 4, 5, 6],
        "the step line does not block"
    );
    let twosided = field_query(
        ElementKind::Linedefs,
        filter("flags", Op::Has, json!("twosided")),
    );
    assert_eq!(
        hits(&map, &twosided),
        vec![2],
        "only the step line is two-sided"
    );
    let not_blocking = field_query(
        ElementKind::Linedefs,
        filter("flags", Op::HasNot, json!("blocking")),
    );
    assert_eq!(hits(&map, &not_blocking), vec![2], "has_not inverts");
    let ambush = field_query(
        ElementKind::Things,
        filter("flags", Op::Has, json!("ambush")),
    );
    assert_eq!(hits(&map, &ambush), vec![0], "the start carries the bit");
    let easy = field_query(ElementKind::Things, filter("flags", Op::Has, json!("easy")));
    assert_eq!(
        hits(&map, &easy),
        vec![0, 1, 2],
        "spawns carry the skill bits"
    );
}

#[test]
fn has_tests_named_sector_roles() {
    let map = roley_map();
    let secret = field_query(
        ElementKind::Sectors,
        filter("roles", Op::Has, json!("secret")),
    );
    assert_eq!(hits(&map, &secret), vec![1], "special 9 hides sector 1");
    let sky = field_query(ElementKind::Sectors, filter("roles", Op::Has, json!("sky")));
    assert_eq!(
        hits(&map, &sky),
        vec![1],
        "the F_SKY1 ceiling opens sector 1"
    );
    let none = field_query(
        ElementKind::Sectors,
        filter("roles", Op::Has, json!("door")),
    );
    assert!(hits(&map, &none).is_empty(), "no door action here");
    let not_sky = field_query(
        ElementKind::Sectors,
        filter("roles", Op::HasNot, json!("sky")),
    );
    assert_eq!(hits(&map, &not_sky), vec![0], "sector 0 stays closed");
}

#[test]
fn derived_sector_fields_measure_the_fixture_rooms() {
    let map = two_room_map();
    let height = field_query(
        ElementKind::Sectors,
        filter("height", Op::Eq, json!(96_i64)),
    );
    assert_eq!(
        hits(&map, &height),
        vec![1],
        "128 minus the raised 32-unit floor"
    );
    let area = field_query(
        ElementKind::Sectors,
        filter("area", Op::Ge, json!(0x0001_0000_i64)),
    );
    assert_eq!(hits(&map, &area), vec![0, 1], "both rooms span 256 x 256");
    let small = field_query(
        ElementKind::Sectors,
        filter("area", Op::Lt, json!(1000_i64)),
    );
    assert!(hits(&map, &small).is_empty(), "nothing is that small");
}

#[test]
fn derived_linedef_fields_resolve_sides_lengths_and_faces() {
    let map = diagonal_map();
    let two_sided = field_query(
        ElementKind::Linedefs,
        filter("two_sided", Op::Eq, json!(true)),
    );
    assert_eq!(
        hits(&map, &two_sided),
        vec![2],
        "only the step line has two sides"
    );
    let either = field_query(
        ElementKind::Linedefs,
        filter("two_sided", Op::In, json!([true, false])),
    );
    assert_eq!(
        hits(&map, &either),
        (0..8).collect::<Vec<usize>>(),
        "in covers both"
    );
    let front = field_query(
        ElementKind::Linedefs,
        filter("front_sector", Op::Eq, json!(0_i64)),
    );
    assert_eq!(
        hits(&map, &front),
        vec![0, 1, 2, 3, 7],
        "the hall fronts, plus the reused diagonal"
    );
    let back = field_query(
        ElementKind::Linedefs,
        filter("back_sector", Op::Eq, json!(1_i64)),
    );
    assert_eq!(
        hits(&map, &back),
        vec![2],
        "only the step line opens backwards"
    );
    let long = field_query(
        ElementKind::Linedefs,
        filter("length", Op::Gt, json!(300_i64)),
    );
    assert_eq!(hits(&map, &long), vec![7], "hypot(512, 256) beats 300");
    let front_lower = field_query(
        ElementKind::Linedefs,
        filter("front_lower", Op::Eq, json!("STEP1")),
    );
    assert_eq!(hits(&map, &front_lower), vec![2], "the step's lower face");
    let back_middle = field_query(
        ElementKind::Linedefs,
        filter("back_middle", Op::Eq, json!("-")),
    );
    assert_eq!(
        hits(&map, &back_middle),
        vec![2],
        "one-sided lines have no back face"
    );
}

#[test]
fn derived_thing_fields_read_names_categories_and_sectors() {
    let map = flaggy_map();
    let start = field_query(
        ElementKind::Things,
        filter("name", Op::Eq, json!("Player 1 Start")),
    );
    assert_eq!(hits(&map, &start), vec![0], "type 1 reads as the start");
    let monster = field_query(
        ElementKind::Things,
        filter("category", Op::Eq, json!("Monsters")),
    );
    assert_eq!(hits(&map, &monster), vec![1], "the imp is a monster");
    let unknown = field_query(
        ElementKind::Things,
        filter("name", Op::Glob, json!("unknown*")),
    );
    assert_eq!(
        hits(&map, &unknown),
        vec![2],
        "type 99999 reads Unknown (99999)"
    );
    let hall = field_query(ElementKind::Things, filter("sector", Op::Eq, json!(0_i64)));
    assert_eq!(hits(&map, &hall), vec![0], "the start stands in the hall");
    let voided = field_query(ElementKind::Things, filter("sector", Op::Ne, json!(0_i64)));
    assert_eq!(
        hits(&map, &voided),
        vec![1],
        "the imp is in a different sector and the void thing resolves to nothing, ne included"
    );
}

#[test]
fn sidedef_fields_include_their_owning_line() {
    let map = two_room_map();
    let wall = field_query(
        ElementKind::Sidedefs,
        filter("middle", Op::Eq, json!("WALL1")),
    );
    assert_eq!(
        hits(&map, &wall),
        vec![0, 1, 4, 5, 6, 7],
        "the dressed faces"
    );
    let step = field_query(
        ElementKind::Sidedefs,
        filter("lower", Op::Eq, json!("STEP1")),
    );
    assert_eq!(hits(&map, &step), vec![2], "the step's lower face");
    let owner = field_query(
        ElementKind::Sidedefs,
        filter("linedef", Op::Eq, json!(2_i64)),
    );
    assert_eq!(
        hits(&map, &owner),
        vec![2, 3],
        "sidedefs 2 and 3 dress line 2"
    );
    let over = field_query(
        ElementKind::Sidedefs,
        filter("sector", Op::Eq, json!(1_i64)),
    );
    assert_eq!(hits(&map, &over), vec![3, 5, 6, 7], "the back room's sides");
}

#[test]
fn bbox_bounds_the_point_kinds() {
    let map = two_room_map();
    let things = query(
        ElementKind::Things,
        vec![filter("index", Op::Ge, json!(0_i64))],
        Some(Within::Bbox {
            bbox: [256, 0, 512, 256],
        }),
    );
    assert_eq!(
        hits(&map, &things),
        vec![1],
        "the imp stands right of x 256"
    );
    let vertices = boxed(ElementKind::Vertices, [0, 0, 256, 256]);
    assert_eq!(
        hits(&map, &vertices),
        vec![0, 1, 4, 5],
        "the hall box corners"
    );
}

#[test]
fn bbox_bounds_the_shape_kinds_by_their_own_rules() {
    let map = two_room_map();
    let lines = boxed(ElementKind::Linedefs, [0, 0, 256, 256]);
    assert_eq!(
        hits(&map, &lines),
        vec![0, 1, 2, 3],
        "lines fully inside the box"
    );
    let sides = boxed(ElementKind::Sidedefs, [0, 0, 256, 256]);
    assert_eq!(
        hits(&map, &sides),
        vec![0, 1, 2, 3, 4],
        "sides of those lines"
    );
    let sectors = boxed(ElementKind::Sectors, [0, 0, 200, 255]);
    assert_eq!(
        hits(&map, &sectors),
        vec![0],
        "only the hall's centre is boxed in"
    );
}

#[test]
fn sector_sets_bound_every_kind_by_membership() {
    let map = two_room_map();
    let one = Within::Sectors { sectors: vec![1] };
    let zero = Within::Sectors { sectors: vec![0] };
    let things = query(ElementKind::Things, vec![], Some(one.clone()));
    assert_eq!(hits(&map, &things), vec![1], "the imp's room");
    let vertices = query(ElementKind::Vertices, vec![], Some(zero.clone()));
    assert_eq!(
        hits(&map, &vertices),
        vec![0, 1, 4, 5],
        "the hall's own corners and the step line's, which face it"
    );
    let lines = query(ElementKind::Linedefs, vec![], Some(one.clone()));
    assert_eq!(
        hits(&map, &lines),
        vec![2, 4, 5, 6],
        "every line with a side in room 1"
    );
    let sides = query(ElementKind::Sidedefs, vec![], Some(one));
    assert_eq!(hits(&map, &sides), vec![3, 5, 6, 7], "the room 1 sides");
    let sectors = query(ElementKind::Sectors, vec![], Some(zero));
    assert_eq!(hits(&map, &sectors), vec![0], "sectors bound by index");
    let none = query(
        ElementKind::Vertices,
        vec![],
        Some(Within::Sectors {
            sectors: vec![99_usize],
        }),
    );
    assert!(
        hits(&map, &none).is_empty(),
        "an out-of-range bound matches nothing"
    );
}
