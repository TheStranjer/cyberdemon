//! The checks topic: filters `AND`ed together, every refusal the field
//! table, the op and value checks and the flag and role names answer with,
//! the family-specific tables, the eager check before any element is read,
//! the `JSON` wire shape and the plain context reads.

use serde_json::json;

use super::{defs, doom_ctx, field_query, filter, hexen_ctx, hits, query, refuse_all};
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::{room, side, solid_line, two_room_map, vertex};
use crate::wad::map::query::{ElementKind, Op, Query, Within, run_query};
use crate::wad::map::{Linedef, MapFormat};

#[test]
fn filters_combine_with_and() {
    let map = two_room_map();
    let both = query(
        ElementKind::Sectors,
        vec![
            filter("light", Op::Gt, json!(100_i64)),
            filter("tag", Op::Eq, json!(1_i64)),
        ],
        None,
    );
    assert_eq!(hits(&map, &both), vec![1], "bright and tagged");
    let empty = query(
        ElementKind::Sectors,
        vec![
            filter("light", Op::Gt, json!(100_i64)),
            filter("tag", Op::Eq, json!(0_i64)),
        ],
        Some(Within::Sectors { sectors: vec![1] }),
    );
    assert!(
        hits(&map, &empty).is_empty(),
        "the third clause contradicts"
    );
    let bare = query(ElementKind::Vertices, vec![], None);
    assert_eq!(
        hits(&map, &bare),
        (0..6).collect::<Vec<usize>>(),
        "no filters keeps everything"
    );
}

#[test]
fn unknown_fields_are_refused_with_the_valid_list() {
    let defs = defs();
    let map = two_room_map();
    let q = field_query(ElementKind::Sectors, filter("aro", Op::Eq, json!(1_i64)));
    let error = run_query(&map, &doom_ctx(&defs), &q).expect_err("no such field");
    assert_eq!(error.code(), "invalid_argument", "bad field code");
    let message = error.to_string();
    assert!(
        message.contains("unknown field 'aro' for sectors")
            && message.contains("floor_flat")
            && message.contains("roles")
            && message.contains("height"),
        "the valid fields are listed: {message}"
    );
}

#[test]
fn family_tables_see_their_own_fields() {
    let defs = defs();
    let map = two_room_map();
    let tag_on_hexen = field_query(ElementKind::Linedefs, filter("tag", Op::Eq, json!(0_i64)));
    let error = run_query(&map, &hexen_ctx(&defs), &tag_on_hexen)
        .expect_err("the Hexen family carries arg0 instead");
    assert!(
        error.to_string().contains("arg0"),
        "the extras are listed: {error}"
    );
    let arg_on_doom = field_query(ElementKind::Linedefs, filter("arg0", Op::Eq, json!(0_i64)));
    assert_eq!(
        run_query(&map, &doom_ctx(&defs), &arg_on_doom)
            .expect_err("the Doom family carries tag instead")
            .code(),
        "invalid_argument",
        "arg0 is not a Doom field"
    );
    let mut hexen = Map::new_empty(map.name, MapFormat::Hexen);
    hexen.linedefs.push(Linedef {
        args: [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        flags: 1_u32,
        ..Default::default()
    });
    let arg0 = field_query(ElementKind::Linedefs, filter("arg0", Op::Eq, json!(7_i64)));
    assert_eq!(
        run_query(&hexen, &hexen_ctx(&defs), &arg0).expect("arg0 reads"),
        vec![0],
        "the script argument filters"
    );
    let cross = field_query(
        ElementKind::Linedefs,
        filter("flags", Op::Has, json!("playercross")),
    );
    assert_eq!(
        run_query(&hexen, &hexen_ctx(&defs), &cross).expect("activation names"),
        vec![0],
        "field value 0 reads as playercross"
    );
    let tid_on_doom = field_query(ElementKind::Things, filter("tid", Op::Eq, json!(0_i64)));
    assert_eq!(
        run_query(&map, &doom_ctx(&defs), &tid_on_doom)
            .expect_err("Doom things carry no tid")
            .code(),
        "invalid_argument",
        "tid is a Hexen field"
    );
}

#[test]
fn wrong_ops_and_values_are_refused_with_instructions() {
    let cases = vec![
        (
            field_query(ElementKind::Sectors, filter("light", Op::Eq, json!("x"))),
            vec!["needs a number"],
        ),
        (
            field_query(
                ElementKind::Sectors,
                filter("floor_flat", Op::Gt, json!("A")),
            ),
            vec!["does not apply", "string", "eq"],
        ),
        (
            field_query(ElementKind::Linedefs, filter("flags", Op::Eq, json!(1_i64))),
            vec!["does not apply", "has"],
        ),
        (
            field_query(ElementKind::Sectors, filter("roles", Op::Eq, json!("door"))),
            vec!["does not apply"],
        ),
        (
            field_query(
                ElementKind::Sectors,
                filter("light", Op::In, json!([160_i64, "x"])),
            ),
            vec!["needs an array of a number", "does not fit"],
        ),
        (
            field_query(
                ElementKind::Sectors,
                filter("light", Op::In, json!(160_i64)),
            ),
            vec!["needs an array"],
        ),
        (
            field_query(
                ElementKind::Sectors,
                filter("floor_flat", Op::Glob, json!(5_i64)),
            ),
            vec!["needs a string pattern"],
        ),
    ];
    refuse_all(cases);
}

#[test]
fn unknown_flag_and_role_names_list_the_valid_ones() {
    let cases = vec![
        (
            field_query(
                ElementKind::Linedefs,
                filter("flags", Op::Has, json!("impassable")),
            ),
            vec!["unknown linedef flag 'impassable'", "blocking", "twosided"],
        ),
        (
            field_query(
                ElementKind::Things,
                filter("flags", Op::Has, json!("twosided")),
            ),
            vec!["unknown thing flag 'twosided'", "ambush", "easy"],
        ),
        (
            field_query(
                ElementKind::Sectors,
                filter("roles", Op::Has, json!("hall")),
            ),
            vec!["unknown sector role 'hall'", "stair_step", "exit_room"],
        ),
    ];
    refuse_all(cases);
}

#[test]
fn bad_queries_fail_before_any_element_is_read() {
    let defs = defs();
    let empty = Map::default();
    let q = field_query(ElementKind::Sectors, filter("aro", Op::Eq, json!(1_i64)));
    assert_eq!(
        run_query(&empty, &doom_ctx(&defs), &q)
            .expect_err("unknown fields fail even with no sectors")
            .code(),
        "invalid_argument",
        "checked eagerly"
    );
}

#[test]
fn the_wire_shape_parses_and_documents_where() {
    let value = json!({
        "kind": "sectors",
        "where": [{ "field": "light", "op": "gt", "value": 170_i64 }],
        "within": { "sectors": [1_usize] },
    });
    let parsed: Query = serde_json::from_value(value).expect("the wire shape parses");
    assert_eq!(parsed.kind, ElementKind::Sectors, "the kind");
    assert_eq!(parsed.r#where.len(), 1, "one filter");
    assert_eq!(
        parsed.r#where.first().expect("the filter").op,
        Op::Gt,
        "snake-case op"
    );
    assert!(
        matches!(parsed.within, Some(Within::Sectors { .. })),
        "tagged within"
    );
    let schema = serde_json::to_value(schemars::schema_for!(Query)).expect("a schema");
    assert!(
        schema["properties"]["where"].is_object(),
        "the schema property is named where, not r#where: {schema}"
    );
}

#[test]
fn context_names_things_through_the_definitions() {
    let defs = defs();
    let ctx = doom_ctx(&defs);
    assert_eq!(ctx.thing_name(3001), "Imp", "the table name");
    assert_eq!(ctx.thing_category(3001), "Monsters", "the table category");
    assert_eq!(ctx.thing_name(99999), "Unknown (99999)", "undefined type");
    assert_eq!(ctx.thing_category(-1), "Unknown", "negative type");
}

#[test]
fn unresolvable_derived_values_and_empty_lists_match_nothing() {
    let map = two_room_map();
    let one_sided = query(
        ElementKind::Linedefs,
        vec![filter("back_sector", Op::Ne, json!(9_i64))],
        None,
    );
    assert_eq!(
        hits(&map, &one_sided),
        vec![2],
        "ne does not rescue a missing side"
    );
    let empty_in = field_query(ElementKind::Sectors, filter("light", Op::In, json!([])));
    assert!(
        hits(&map, &empty_in).is_empty(),
        "an empty list matches nothing"
    );
    let empty_not_in = field_query(ElementKind::Sectors, filter("light", Op::NotIn, json!([])));
    assert_eq!(
        hits(&map, &empty_not_in),
        vec![0, 1],
        "an empty list excludes nothing"
    );
}

#[test]
fn vertices_and_things_filter_by_plain_numbers() {
    let map = two_room_map();
    let east = field_query(ElementKind::Vertices, filter("x", Op::Ge, json!(256_i64)));
    assert_eq!(hits(&map, &east), vec![1, 2, 3, 4], "right-hand vertices");
    let facing = field_query(ElementKind::Things, filter("angle", Op::Eq, json!(180_i64)));
    assert_eq!(hits(&map, &facing), vec![1], "the imp faces south");
    let kind = field_query(ElementKind::Things, filter("type", Op::Eq, json!(1_i64)));
    assert_eq!(hits(&map, &kind), vec![0], "the player start");
}

#[test]
fn extra_sector_rows_and_lines_stay_queryable() {
    let mut map = two_room_map();
    map.sectors.push(room(0, 128, 128, 2));
    map.sidedefs.push(side("-", "-", "STARTAN", 2));
    let mut lone = solid_line(0, 1, 8);
    lone.special = 1_i32;
    map.linedefs.push(lone);
    map.vertices.push(vertex(0, 128));
    map.vertices.push(vertex(256, 128));
    let dark = field_query(
        ElementKind::Sectors,
        filter("light", Op::Lt, json!(160_i64)),
    );
    assert_eq!(hits(&map, &dark), vec![2], "the added room is darkest");
    let tagged = field_query(ElementKind::Sectors, filter("tag", Op::Eq, json!(2_i64)));
    assert_eq!(hits(&map, &tagged), vec![2], "and tagged");
    let special = field_query(
        ElementKind::Linedefs,
        filter("special", Op::Eq, json!(1_i64)),
    );
    assert_eq!(
        hits(&map, &special),
        vec![7],
        "the new line wears the action"
    );
}
