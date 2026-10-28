//! The `assemble_map` graph builders: the include and inline-limit
//! parsing and the whole graph over the two-room fixture, with the
//! vertex buckets' self-loop and dangling-index rules checked directly.

use alloc::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Kind, MAX_INLINE_LIMIT, assemble, parse_include, parse_inline_limit, vertex_linedefs};
use crate::state::AppState;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A vertex at `x`/`y`.
fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        extra: BTreeMap::new(),
    }
}

/// A one-sided line from `v1` to `v2` over front sidedef `front`.
fn line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1,
        front: Some(front),
        ..Default::default()
    }
}

/// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
fn line_both(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4,
        front: Some(front),
        back: Some(back),
        ..Default::default()
    }
}

/// A sidedef over `sector`.
fn side(sector: usize) -> Sidedef {
    Sidedef {
        sector,
        ..Default::default()
    }
}

/// A spawn of `kind` at `x`/`y`.
fn spawn(kind: i32, x: i32, y: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        angle: 0,
        flags: 7,
        ..Default::default()
    }
}

/// The unit-test two-room map: a hall (`0..256`) and a back room
/// (`256..512`) split by the two-sided line `2`.
fn two_rooms() -> Map {
    Map {
        name: name("MAP01"),
        things: vec![spawn(1, 64, 128), spawn(3001, 384, 128)],
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            line(0, 5, 0),
            line(5, 4, 1),
            line_both(4, 1, 2, 3),
            line(1, 0, 4),
            line(4, 3, 5),
            line(3, 2, 6),
            line(2, 1, 7),
        ],
        sidedefs: vec![
            side(0),
            side(0),
            side(0),
            side(1),
            side(0),
            side(1),
            side(1),
            side(1),
        ],
        sectors: vec![Sector::default(), Sector::default()],
        ..Default::default()
    }
}

/// The full graph of `map` as a JSON value.
fn graph_json(map: &Map) -> Value {
    let include = parse_include(None).expect("all kinds");
    let plan = assemble(name("MAP01"), map, &include);
    let outcome = plan
        .finish(&AppState::default(), MAX_INLINE_LIMIT)
        .expect("inlining needs no asset server");
    serde_json::to_value(&outcome).expect("outcomes serialize")
}

#[test]
fn parse_include_defaults_deduplicates_and_refuses_unknowns() {
    assert_eq!(
        parse_include(None).expect("absent means all"),
        vec![
            Kind::Vertices,
            Kind::Linedefs,
            Kind::Sidedefs,
            Kind::Sectors,
            Kind::Things
        ],
        "all five, in report order",
    );
    let given = vec![
        "Things".to_owned(),
        "linedefs".to_owned(),
        "things".to_owned(),
    ];
    assert_eq!(
        parse_include(Some(given)).expect("known names"),
        vec![Kind::Things, Kind::Linedefs],
        "case-insensitive, deduplicated, request order",
    );
    let empty = parse_include(Some(Vec::new())).expect("nothing is valid");
    assert!(empty.is_empty(), "an empty include carries no arrays");
    let error = parse_include(Some(vec!["nope".to_owned()])).expect_err("not a kind");
    assert_eq!(error.code(), "invalid_argument", "bad kind code");
    let message = error.to_string();
    assert!(
        message.contains("\"nope\"")
            && message.contains("vertices, linedefs, sidedefs, sectors, things"),
        "the refusal names the word and the valid list: {message}"
    );
}

#[test]
fn parse_inline_limit_applies_the_default_and_the_range() {
    assert_eq!(
        parse_inline_limit(None).expect("absent"),
        2000,
        "the default"
    );
    assert_eq!(
        parse_inline_limit(Some(MAX_INLINE_LIMIT)).expect("the boundary"),
        MAX_INLINE_LIMIT,
        "the maximum is inclusive"
    );
    let zero = parse_inline_limit(Some(0)).expect_err("zero refuses");
    assert_eq!(zero.code(), "invalid_argument", "zero code");
    let over = parse_inline_limit(Some(20_001)).expect_err("above the maximum");
    assert_eq!(over.code(), "invalid_argument", "over-max code");
    assert!(
        zero.to_string().contains("1..=20000"),
        "the message names the range: {zero}"
    );
}

#[test]
fn the_graph_resolves_linedefs_sidedefs_and_vertices() {
    let report = graph_json(&two_rooms());
    assert_eq!(report["map"], json!("MAP01"), "the map echoes");
    assert_eq!(report["problems"], json!([]), "the fixture map is sound");
    let step = &report["linedefs"][2];
    assert_eq!(
        step["front"]["sector"],
        json!(0_i64),
        "the step line's front room"
    );
    assert_eq!(step["back"]["sector"], json!(1_i64), "and its back room");
    assert_eq!(step["flag_names"], json!(["twosided"]), "flags named");
    assert_eq!(step["length"], json!(256_u64), "the shared wall's length");
    assert_eq!(
        step["v1"]["index"],
        json!(4_i64),
        "ends resolved to coordinates"
    );
    assert_eq!(step["v1"]["y"], json!(256_i64), "vertex 4 is at y 256");
    assert_eq!(
        report["sidedefs"][3]["side"],
        json!("back"),
        "sidedef 3 is a back side"
    );
    assert_eq!(
        report["sidedefs"][3]["linedef"],
        json!(2_i64),
        "of the step line"
    );
    assert_eq!(
        report["vertices"][1]["linedefs"],
        json!([2_i64, 3_i64, 6_i64]),
        "shared corner"
    );
}

#[test]
fn the_graph_places_sectors_and_things() {
    let report = graph_json(&two_rooms());
    assert_eq!(
        report["sectors"][0]["neighbors"],
        json!([1_i64]),
        "hall neighbours"
    );
    assert_eq!(
        report["sectors"][0]["things"],
        json!([0_i64]),
        "the player start stands in the hall"
    );
    assert_eq!(
        report["sectors"][1]["things"],
        json!([1_i64]),
        "the imp stands in the back room"
    );
    assert_eq!(
        report["sectors"][0]["bbox"],
        json!([0_i64, 0_i64, 256_i64, 256_i64]),
        "the hall box"
    );
    assert_eq!(
        report["things"][1]["sector"],
        json!(1_i64),
        "and back from the thing side"
    );
    assert_eq!(
        report["things"][0]["flag_names"],
        json!(["easy", "medium", "hard"]),
        "thing flags named",
    );
    assert_eq!(
        report["sectors"][0]["sidedefs"],
        json!([0_i64, 1_i64, 2_i64, 4_i64]),
        "the hall's sides"
    );
}

#[test]
fn include_narrows_the_arrays_and_the_counts() {
    let include = parse_include(Some(vec!["things".to_owned()])).expect("one kind");
    let plan = assemble(name("MAP01"), &two_rooms(), &include);
    let outcome = plan
        .finish(&AppState::default(), MAX_INLINE_LIMIT)
        .expect("two things inline");
    let report = serde_json::to_value(&outcome).expect("outcomes serialize");
    assert!(
        report.get("things").is_some(),
        "the requested array is there"
    );
    for key in ["vertices", "linedefs", "sidedefs", "sectors"] {
        assert!(report.get(key).is_none(), "{key} was not requested");
    }
    assert_eq!(
        report["things"].as_array().expect("array").len(),
        2,
        "both things"
    );
    assert_eq!(report["problems"], json!([]), "problems always ride along");
}

#[test]
fn dangling_references_become_problems_and_nulls() {
    let mut broken = two_rooms();
    broken.sidedefs.get_mut(4).expect("sidedef 4").sector = 99;
    let report = graph_json(&broken);
    let problems = report["problems"].as_array().expect("array");
    assert!(!problems.is_empty(), "the graph reports the break");
    assert!(
        problems.iter().any(|problem| problem
            .as_str()
            .is_some_and(|text| text.contains("sector 99"))),
        "the dangling sector is named: {problems:?}"
    );
    assert_eq!(
        report["sidedefs"][4]["sector"],
        Value::Null,
        "the row nulls it"
    );
    assert_eq!(
        report["linedefs"][3]["front"]["sector"],
        Value::Null,
        "and through the line"
    );
    assert_eq!(
        report["sectors"][0]["sidedefs"],
        json!([0_i64, 1_i64, 2_i64]),
        "the hall lost side 4"
    );
}

#[test]
fn vertex_buckets_count_a_self_loop_once_and_skip_dangling_indices() {
    let mut map = two_rooms();
    map.linedefs.push(line(0, 0, 0));
    let loop_at = map.linedefs.len().saturating_sub(1);
    map.linedefs.push(line(999, 5, 0));
    let dangling = map.linedefs.len().saturating_sub(1);
    let buckets = vertex_linedefs(&map);
    assert_eq!(buckets.len(), map.vertices.len(), "one bucket per vertex");
    assert_eq!(
        buckets[0],
        vec![0_usize, 3, loop_at],
        "the self-loop is counted once, in linedef order"
    );
    assert_eq!(
        buckets[5],
        vec![0_usize, 1, dangling],
        "the line whose other end dangles still claims its live vertex"
    );
}
