//! Shared builders and assertions for the `edit_geometry` tests: the
//! parsed-request readers, the two-room fixture and the deletion step the
//! `delete_geometry` unit tests fold over.
//!
//! [`transform`] parses the `transform_selection` request types and checks
//! the operations, rotation and selection defaults against the engine's
//! own shapes; the tests here run the always-on cascade of a deletion over
//! [`two_room`], refuse an out-of-range index and read the untouched
//! fragment of a dry run.

mod transform;

use serde_json::{Value, from_str};

use super::delete::{
    DeleteGeometryParams, cascade_options, check_selection, delete_in_place, has_selection,
    removal_size, seed_removal, selected_lists,
};
use super::transform::{RotateInput, TransformSelectionParams};
use super::untouched_fragment;
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::remove::{CascadeOptions, RemovalReport};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::name::LumpName;
use alloc::collections::BTreeSet;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A delete request parsed from `json`.
fn args(json: &str) -> DeleteGeometryParams {
    from_str(json).expect("test delete requests parse")
}

/// A valid parsed test name.
fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A vertex at `x`/`y`.
fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Default::default()
    }
}

/// A fully visible spawn of `kind` at `x`/`y`.
fn spawn(kind: i32, x: i32, y: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        flags: 7,
        ..Default::default()
    }
}

/// A blocking one-sided line from `v1` to `v2` over sidedef `front`.
fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1,
        front: Some(front),
        ..Default::default()
    }
}

/// A two-sided step line from `v1` to `v2` over sidedefs `front`/`back`.
fn step_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4,
        front: Some(front),
        back: Some(back),
        ..Default::default()
    }
}

/// A sidedef opening into `sector`.
fn side(sector: usize) -> Sidedef {
    Sidedef {
        sector,
        ..Default::default()
    }
}

/// The two-room map the test fixtures build: a `0,0` to `512,256` hall
/// and raised back room, six vertices, seven lines (`2` the two-sided
/// step), eight sides over two sectors, a start and an imp.
fn two_room() -> Map {
    Map {
        name: name("MAP01"),
        format: MapFormat::Doom,
        things: vec![spawn(1, 64, 128), spawn(3_001, 384, 128)],
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            solid_line(0, 5, 0),
            solid_line(5, 4, 1),
            step_line(4, 1, 2, 3),
            solid_line(1, 0, 4),
            solid_line(4, 3, 5),
            solid_line(3, 2, 6),
            solid_line(2, 1, 7),
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

/// The step of `json`'s selection, checked and cascaded onto `map`.
fn delete(map: &mut Map, json: &str) -> Result<RemovalReport, CyberError> {
    let request = args(json);
    let opts = cascade_options(&request);
    let removal = seed_removal(&request);
    delete_in_place(map, opts, &removal)
}

/// A transform request parsed from `json`.
pub(super) fn transform_args(json: &str) -> TransformSelectionParams {
    from_str(json).expect("test transform requests parse")
}

/// The `rotate` object of a parsed request.
pub(super) fn rotate_of(json: &str) -> RotateInput {
    transform_args(&format!(r#"{{"selection": {{}}, "rotate": {json}}}"#))
        .rotate
        .expect("the rotate object parses")
}

#[test]
fn a_selection_needs_at_least_one_index() {
    assert!(!has_selection(&args("{}")), "no lists at all");
    assert!(
        !has_selection(&args(r#"{"vertices": [], "sectors": []}"#)),
        "empty lists select nothing"
    );
    assert!(
        has_selection(&args(r#"{"linedefs": [], "sectors": [1]}"#)),
        "one non-empty list is enough"
    );
    let mixed = args(r#"{"vertices": [4, 1, 4], "things": [0]}"#);
    assert_eq!(
        selected_lists(&mixed)[0],
        [4_usize, 1, 4],
        "the lists pass through as given"
    );
    let removal = seed_removal(&mixed);
    assert_eq!(
        removal.vertices,
        BTreeSet::from([1_usize, 4]),
        "duplicates collapse"
    );
    assert_eq!(removal.things, BTreeSet::from([0_usize]));
    assert!(
        removal.linedefs.is_empty() && removal.sidedefs.is_empty() && removal.sectors.is_empty(),
        "the absent lists stay empty"
    );
    assert_eq!(removal_size(&removal), 3, "the seed count labels the step");
}

#[test]
fn the_pruning_flags_default_to_the_cascading_pair() {
    let defaults = cascade_options(&args(r#"{"sectors": [1]}"#));
    assert!(
        defaults.prune_sidedefs && defaults.prune_vertices,
        "pruning is on by default"
    );
    assert_eq!(
        cascade_options(&args(
            r#"{"sectors": [1], "prune_sidedefs": false, "prune_vertices": false}"#,
        )),
        CascadeOptions {
            prune_sidedefs: false,
            prune_vertices: false,
        },
        "both switches reach the options"
    );
}

#[test]
fn check_selection_names_the_missing_index_and_its_kind() {
    let map = two_room();
    assert!(
        check_selection(
            &map,
            &seed_removal(&args(r#"{"sectors": [0, 1], "vertices": [5]}"#)),
        )
        .is_ok(),
        "every index of the fixture selection exists"
    );
    let cases = [
        (
            r#"{"vertices": [99]}"#,
            "vertex 99 does not exist (the map has 6 vertices, valid indices are 0..=5)",
        ),
        (
            r#"{"linedefs": [7]}"#,
            "linedef 7 does not exist (the map has 7 linedefs, valid indices are 0..=6)",
        ),
        (
            r#"{"sidedefs": [8]}"#,
            "sidedef 8 does not exist (the map has 8 sidedefs, valid indices are 0..=7)",
        ),
        (
            r#"{"sectors": [2]}"#,
            "sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)",
        ),
        (
            r#"{"things": [2]}"#,
            "thing 2 does not exist (the map has 2 things, valid indices are 0..=1)",
        ),
    ];
    for (json, phrasing) in cases {
        let error = check_selection(&map, &seed_removal(&args(json)))
            .expect_err("the named index is past the end of its block");
        assert_eq!(code(&error), "not_found", "out of range code for {json}");
        assert_eq!(error.to_string(), phrasing, "the documented phrasing");
    }
    let first_kind = check_selection(
        &map,
        &seed_removal(&args(r#"{"vertices": [9], "sectors": [9]}"#)),
    )
    .expect_err("both lists are out of range");
    assert!(
        first_kind.to_string().contains("vertex 9"),
        "the vertex list is checked first: {first_kind}"
    );
}

#[test]
fn a_cascaded_sector_deletion_takes_the_room_apart() {
    let mut map = two_room();
    let report = delete(&mut map, r#"{"sectors": [1]}"#).expect("sector 1 goes");
    assert_eq!(report.sectors, vec![1], "the seeded sector");
    assert_eq!(report.sidedefs, vec![3, 5, 6, 7], "every side of it");
    assert_eq!(
        report.linedefs,
        vec![4, 5, 6],
        "the lines that only those sides held"
    );
    assert_eq!(report.vertices, vec![2, 3], "the orphaned corners follow");
    assert!(report.things.is_empty(), "the cascade never adds things");
    assert_eq!(report.one_sided_linedefs, vec![2], "the shared line closed");
    assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
    assert_eq!(map.sectors.len(), 1, "only the hall survives");
    assert_eq!(map.sidedefs.len(), 4, "its four sides survive");
    assert_eq!(map.linedefs.len(), 4, "four lines survive");
    assert_eq!(map.vertices.len(), 4, "the back room's corners went");
}

#[test]
fn a_mixed_selection_deletes_things_and_geometry_together() {
    let mut map = two_room();
    let report =
        delete(&mut map, r#"{"things": [1], "vertices": [5]}"#).expect("the imp and one corner go");
    assert_eq!(report.things, vec![1], "the seeded thing");
    assert_eq!(report.vertices, vec![5], "the seeded corner");
    assert_eq!(
        report.linedefs,
        vec![0, 1],
        "both lines ending at the corner go"
    );
    assert_eq!(
        report.sidedefs,
        vec![0, 1],
        "the sides only those lines had are pruned"
    );
    assert!(report.sectors.is_empty(), "no sector went");
    assert_eq!(map.things.len(), 1, "the start survives");
    assert_eq!(map.vertices.len(), 5, "one vertex dropped");
    assert_eq!(map.linedefs.len(), 5, "five lines survive");
}

#[test]
fn pruning_off_leaves_the_orphans_behind() {
    let mut unpruned = two_room();
    let report = delete(
        &mut unpruned,
        r#"{"linedefs": [5, 6], "prune_sidedefs": false, "prune_vertices": false}"#,
    )
    .expect("the two named lines go");
    assert_eq!(report.linedefs, vec![5, 6], "exactly the seed");
    assert!(
        report.sidedefs.is_empty() && report.vertices.is_empty(),
        "nothing followed: {report:?}"
    );
    assert_eq!(unpruned.vertices.len(), 6, "the orphaned vertex stays");
    assert_eq!(unpruned.sidedefs.len(), 8, "the orphaned sides stay");
    let mut vertexless = two_room();
    let sector_report = delete(
        &mut vertexless,
        r#"{"sectors": [1], "prune_vertices": false}"#,
    )
    .expect("the sector rule still runs");
    assert_eq!(
        sector_report.sidedefs,
        vec![3, 5, 6, 7],
        "the sector rule is not a pruning"
    );
    assert_eq!(
        sector_report.linedefs,
        vec![4, 5, 6],
        "sideless lines still go"
    );
    assert!(
        sector_report.vertices.is_empty(),
        "the orphaned vertices stay: {sector_report:?}"
    );
    assert_eq!(
        vertexless.vertices.len(),
        6,
        "both back-room corners survive"
    );
}

#[test]
fn a_missing_index_deletes_nothing() {
    let before = two_room();
    let mut map = two_room();
    let error =
        delete(&mut map, r#"{"things": [0], "sectors": [9]}"#).expect_err("sector 9 is not there");
    assert_eq!(code(&error), "not_found", "out of range code");
    assert_eq!(map, before, "the whole map stands");
}

#[test]
fn an_untouched_fragment_reports_the_map_by_name() {
    let fragment = untouched_fragment(name("MAP01"));
    assert_eq!(
        fragment.get("map").and_then(Value::as_str),
        Some("MAP01"),
        "named"
    );
    assert_eq!(
        fragment.get("nodes_invalidated").and_then(Value::as_bool),
        Some(false),
        "nothing was invalidated"
    );
    assert_eq!(
        fragment.get("copied_to_working").and_then(Value::as_bool),
        Some(false),
        "nothing was copied"
    );
}
