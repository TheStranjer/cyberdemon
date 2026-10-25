//! The shared element-index guard's own checks: the spelled singular and
//! plural per kind, the range wording for a full and an empty block, the
//! index-listing hint, the in-range pass, and the list guard's first
//! offender.

use crate::error::CyberError;
use crate::wad::map::query::{ElementKind, check_index, check_indices, missing_index};
use crate::wad::map::{Map, Vertex};

/// The hint every index refusal carries.
const HINT: &str = "describe_map or find_elements returns indices";

#[test]
fn every_kind_names_its_singular_and_its_spelled_plural() {
    let names = [
        (ElementKind::Sectors, "sector", "sectors"),
        (ElementKind::Linedefs, "linedef", "linedefs"),
        (ElementKind::Sidedefs, "sidedef", "sidedefs"),
        (ElementKind::Things, "thing", "things"),
        (ElementKind::Vertices, "vertex", "vertices"),
    ];
    for (kind, singular, plural) in names {
        assert_eq!(kind.singular(), singular, "singular of {kind:?}");
        assert_eq!(kind.plural(), plural, "plural of {kind:?}");
        assert_eq!(kind.plural(), kind.as_str(), "the plural is the wire name");
    }
}

#[test]
fn check_index_accepts_the_range_and_refuses_past_it() {
    assert!(
        check_index(ElementKind::Linedefs, 6, 7).is_ok(),
        "the last index is in"
    );
    let error = check_index(ElementKind::Linedefs, 12, 7).expect_err("12 is out");
    assert_eq!(error.code(), "not_found", "out of range code");
    assert_eq!(
        error.message(),
        "linedef 12 does not exist (the map has 7 linedefs, valid indices are 0..=6)",
        "the documented phrasing names the valid range"
    );
    assert_eq!(error.hint(), HINT, "the hint names the index-listing tools");
}

#[test]
fn the_vertex_plural_is_spelled_never_appended() {
    let error = check_index(ElementKind::Vertices, 40, 6).expect_err("40 is out");
    assert_eq!(
        error.message(),
        "vertex 40 does not exist (the map has 6 vertices, valid indices are 0..=5)",
        "vertices, never vertexs"
    );
}

#[test]
fn an_empty_block_refuses_with_the_no_rows_wording() {
    for kind in ElementKind::ALL {
        let error = check_index(kind, 0, 0).expect_err("nothing to index into");
        assert_eq!(
            error.message(),
            format!(
                "{} 0 does not exist (the map has no {})",
                kind.singular(),
                kind.plural()
            ),
            "the empty-map phrasing of {kind:?}"
        );
    }
}

#[test]
fn missing_index_answers_the_error_check_index_refuses_with() {
    for kind in ElementKind::ALL {
        let built = missing_index(kind, 9, 4);
        let refused = check_index(kind, 9, 4).expect_err("9 is past 4");
        assert_eq!(built.code(), refused.code(), "same code for {kind:?}");
        assert_eq!(
            built.message(),
            refused.message(),
            "same message for {kind:?}"
        );
        assert_eq!(built.hint(), refused.hint(), "same hint for {kind:?}");
        assert!(
            matches!(built, CyberError::WithHint { .. }),
            "the hint rides as a wrapper: {built:?}"
        );
    }
}

/// A map holding two vertices and nothing else.
fn two_vertices() -> Map {
    Map {
        vertices: vec![
            Vertex {
                x: 0,
                y: 0,
                ..Default::default()
            },
            Vertex {
                x: 1,
                y: 0,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[test]
fn check_indices_passes_an_empty_list_and_names_the_first_offender() {
    let map = two_vertices();
    let none: [usize; 0] = [];
    assert!(
        check_indices(ElementKind::Vertices, &none, &map).is_ok(),
        "no indices checks nothing"
    );
    assert!(
        check_indices(ElementKind::Vertices, &[0_usize, 1], &map).is_ok(),
        "the whole block exists"
    );
    let error = check_indices(ElementKind::Vertices, &[1_usize, 40, 5], &map)
        .expect_err("the list runs past the block");
    assert_eq!(
        error.message(),
        "vertex 40 does not exist (the map has 2 vertices, valid indices are 0..=1)",
        "the first past-the-end index answers"
    );
    assert_eq!(error.hint(), HINT, "the list guard shares the hint");
}

#[test]
fn check_indices_reads_the_kind_count_straight_off_the_map() {
    let empty = Map::default();
    for kind in ElementKind::ALL {
        let error =
            check_indices(kind, &[0_usize], &empty).expect_err("the empty map holds nothing");
        assert_eq!(
            error.message(),
            format!(
                "{} 0 does not exist (the map has no {})",
                kind.singular(),
                kind.plural()
            ),
            "the {kind:?} block is empty"
        );
    }
}
