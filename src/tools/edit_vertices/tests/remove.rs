//! The removal topic: the plain drop of an unreferenced point, the
//! `conflict` refusal of a used one, the cascade it opts into and the
//! pruning flags that reach it.

use super::{add_args, code, fixture, remove_args};
use crate::error::CyberError;
use crate::tools::edit_vertices::add_in_place;
use crate::tools::maps::params::ElementKind;
use crate::tools::maps::remove::{cascade_options, remove_in_place};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::remove::RemovalReport;

/// Run the shared vertex removal step the way `remove_vertex` does: read
/// the request's `cascade` flag and pruning switches and hand them on.
fn remove(map: &mut Map, json: &str) -> Result<RemovalReport, CyberError> {
    let args = remove_args(json);
    let indices = args.index.indices()?;
    remove_in_place(
        map,
        ElementKind::Vertices,
        &indices,
        args.cascade
            .then(|| cascade_options(args.prune_sidedefs, args.prune_vertices)),
    )
}

#[test]
fn an_unreferenced_vertex_removes_without_a_cascade() {
    let mut map = fixture(MapFormat::Doom);
    let added = add_in_place(&mut map, &add_args(r#"{"x": 32, "y": 32}"#)).expect("one point");
    let report = remove(&mut map, r#"{"index": 6}"#).expect("the loose point goes");
    assert_eq!(added.index, 6, "the fixture has six vertices");
    assert_eq!(report.vertices, vec![6], "the removed original index");
    assert!(
        report.linedefs.is_empty() && report.sidedefs.is_empty(),
        "nothing else went: {report:?}"
    );
    assert_eq!(map.vertices.len(), 6, "and the list shrank back");
}

#[test]
fn a_used_vertex_refuses_without_cascade_and_lists_its_lines() {
    let mut map = fixture(MapFormat::Doom);
    let before = map.clone();
    let error = remove(&mut map, r#"{"index": 4}"#).expect_err("three lines end at vertex 4");
    assert_eq!(code(&error), "conflict", "dependents code");
    assert!(
        error
            .to_string()
            .contains("vertex 4 is used by linedef(s) 1, 2, 4")
            && error.to_string().contains("cascade: true"),
        "the message lists the lines and the fix: {error}"
    );
    assert_eq!(map, before, "nothing was removed");
}

#[test]
fn a_cascaded_vertex_removal_takes_its_lines_and_orphaned_sides() {
    let mut map = fixture(MapFormat::Doom);
    let report = remove(&mut map, r#"{"index": 4, "cascade": true}"#)
        .expect("the cascade lifts the conflict");
    assert_eq!(report.vertices, vec![4], "the seeded point");
    assert_eq!(
        report.linedefs,
        vec![1, 2, 4],
        "the three lines that used it"
    );
    assert_eq!(
        report.sidedefs,
        vec![1, 2, 3, 5],
        "the sides only those lines had"
    );
    assert!(
        report.sectors.is_empty() && report.things.is_empty(),
        "nothing else: {report:?}"
    );
    assert_eq!(map.vertices.len(), 5, "one vertex dropped");
    assert_eq!(map.linedefs.len(), 4, "three lines dropped");
    let corner_gone = !map
        .vertices
        .iter()
        .any(|vertex| vertex.x == 256_i32 && vertex.y == 256_i32);
    assert!(corner_gone, "the removed point left the vertex list");
}

#[test]
fn the_pruning_flags_reach_the_cascade() {
    let defaults = cascade_options(
        remove_args(r#"{"index": 4, "cascade": true}"#).prune_sidedefs,
        remove_args(r#"{"index": 4, "cascade": true}"#).prune_vertices,
    );
    assert!(
        defaults.prune_sidedefs && defaults.prune_vertices,
        "pruning is on by default"
    );
    let off = cascade_options(Some(false), Some(false));
    assert!(
        !off.prune_sidedefs && !off.prune_vertices,
        "both switches reach the options"
    );
    let mut map = fixture(MapFormat::Doom);
    let report = remove(
        &mut map,
        r#"{"index": 4, "cascade": true, "prune_sidedefs": false, "prune_vertices": false}"#,
    )
    .expect("the unpruned cascade still applies");
    assert_eq!(report.linedefs, vec![1, 2, 4], "the seeded rule still runs");
    assert!(report.sidedefs.is_empty(), "the sides stay orphaned");
    assert_eq!(map.vertices.len(), 5, "only the selected vertex went");
    assert_eq!(map.sidedefs.len(), 8, "and no side followed it");
}

#[test]
fn removing_a_missing_index_reports_not_found() {
    let mut map = fixture(MapFormat::Doom);
    let error = remove(&mut map, r#"{"index": 40}"#).expect_err("the map holds six vertices");
    assert_eq!(code(&error), "not_found", "out of range code");
    assert_eq!(map, fixture(MapFormat::Doom), "nothing removed");
}
