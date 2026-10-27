//! Named pipeline cases: the polygon and fraction helpers, the layer and
//! external-segment arrangements the drawing tools depend on, and the
//! refusals of degenerate or over-sized input.

use super::{CountingLabeler, assert_clean, find_piece, piece_claims, rect, room};
use crate::fixed::gcd;
use crate::geom::Pt;
use crate::geom::arrange::split::normalize_fraction;
use crate::geom::arrange::{
    arrange, cut, normalize_polygon, polygon_area2, ring_edges, self_crossing,
};
use crate::geom::label::Label;
use crate::geom::segment::{ExternalSegment, SourceId};
use crate::geom::{Layer, LayerLabeler};

#[test]
fn polygon_helpers_normalize_ring_and_area() {
    let closed = vec![(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)];
    assert_eq!(normalize_polygon(&closed).len(), 4, "closing vertex drops");
    let open = vec![(0, 0), (5, 5), (5, 5), (10, 0), (10, 0)];
    assert_eq!(
        normalize_polygon(&open).len(),
        3,
        "consecutive repeats drop"
    );
    assert_eq!(
        ring_edges(&[(1, 2)]).len(),
        1,
        "a lone point loops to itself"
    );
    assert_eq!(ring_edges(&[]).len(), 0, "empty ring");
    assert_eq!(polygon_area2(&rect(0, 0, 10, 10)), 200, "twice the area");
    assert_eq!(
        polygon_area2(&rect(0, 0, 10, 10).into_iter().rev().collect::<Vec<_>>()),
        -200,
        "winding flips sign"
    );
    assert_eq!(polygon_area2(&[(0, 0), (5, 0), (10, 0)]), 0, "flat");
    assert_eq!(self_crossing(&rect(0, 0, 10, 10)), None, "simple");
    assert_eq!(
        self_crossing(&[(0, 0), (10, 10), (10, 0), (0, 5)]),
        Some((0, 2)),
        "crossed quadrilateral: edges 0 and 2"
    );
}

#[test]
fn fraction_helpers_reduce_and_sign() {
    assert_eq!(
        normalize_fraction((4, -8)),
        (-1, 2),
        "sign moves to numerator"
    );
    assert_eq!(normalize_fraction((4, 8)), (1, 2), "plain reduction");
    assert_eq!(normalize_fraction((0, 7)), (0, 1), "zero stays zero");
    assert_eq!(normalize_fraction((5, 0)), (0, 1), "degenerate is zero");
    assert_eq!(gcd(12, 18), 6, "plain gcd");
    assert_eq!(gcd(0, 5), 5, "zero entry");
}

#[test]
fn two_squares_sharing_an_edge_yield_one_shared_piece() {
    let layers = vec![room(0, rect(0, 0, 10, 10)), room(1, rect(10, 0, 20, 10))];
    let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
    assert_eq!(pieces.len(), 7, "seven boundary pieces, shared edge once");
    let shared = find_piece(&pieces, (10, 0), (10, 10)).unwrap();
    assert_eq!(shared.origin.len(), 2, "both squares claim the edge");
    assert_eq!(shared.a, (10, 0), "sorted start");
    assert_eq!(shared.b, (10, 10), "sorted end");
    assert_eq!(shared.left, Label::Region(0), "left is square A");
    assert_eq!(shared.right, Label::Region(1), "right is square B");
    let keys: Vec<(Pt, Pt)> = pieces.iter().map(|piece| (piece.a, piece.b)).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), keys.len(), "no duplicate pieces");
}

#[test]
fn t_junction_splits_the_touched_edge_into_three() {
    let layers = vec![
        room(0, rect(0, 0, 10, 10)),
        Layer {
            label: Label::Region(1),
            polygon: vec![(10, 2), (16, 2), (16, 8), (10, 8)],
        },
    ];
    let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
    assert!(
        find_piece(&pieces, (10, 0), (10, 10)).is_none(),
        "A's edge was split"
    );
    let low = find_piece(&pieces, (10, 0), (10, 2)).unwrap();
    let middle = find_piece(&pieces, (10, 2), (10, 8)).unwrap();
    let high = find_piece(&pieces, (10, 8), (10, 10)).unwrap();
    assert_eq!(low.origin.len(), 1, "only square A reaches here");
    assert_eq!(high.origin.len(), 1, "only square A reaches here");
    assert_eq!(middle.origin.len(), 2, "A's edge and B's edge overlap");
    assert_eq!(middle.left, Label::Region(0), "left stays A");
    assert_eq!(middle.right, Label::Region(1), "right is B");
    assert_clean(&pieces);
}

#[test]
fn the_top_square_relabels_below_and_drops_interior_edges() {
    let layers = vec![room(0, rect(0, 0, 10, 10)), room(1, rect(5, 5, 15, 15))];
    let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
    assert!(
        find_piece(&pieces, (10, 5), (10, 10)).is_none(),
        "A's edge inside B is dropped"
    );
    assert!(
        find_piece(&pieces, (5, 10), (10, 10)).is_none(),
        "A's top inside B is dropped"
    );
    let edge = find_piece(&pieces, (10, 0), (10, 5)).unwrap();
    assert_eq!(edge.left, Label::Region(0), "outside B, inside A");
    assert_eq!(edge.right, Label::Void, "east of A is nothing");
    let b_floor = find_piece(&pieces, (5, 5), (10, 5)).unwrap();
    assert_eq!(b_floor.left, Label::Region(1), "inside both: top wins");
    assert_eq!(b_floor.right, Label::Region(0), "below is only A");
    assert_clean(&pieces);
}

#[test]
fn a_void_strip_on_top_separates_two_rooms() {
    let layers = vec![
        room(1, rect(0, 0, 28, 10)),
        room(2, rect(32, 0, 60, 10)),
        Layer {
            label: Label::Void,
            polygon: rect(28, 0, 32, 10),
        },
    ];
    let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
    let left_wall = find_piece(&pieces, (28, 0), (28, 10)).unwrap();
    assert_eq!(left_wall.left, Label::Region(1), "west is room 1");
    assert_eq!(left_wall.right, Label::Void, "east is the void strip");
    assert_eq!(left_wall.origin.len(), 2, "room and strip share the wall");
    let right_wall = find_piece(&pieces, (32, 0), (32, 10)).unwrap();
    assert_eq!(right_wall.left, Label::Void, "west is the void strip");
    assert_eq!(right_wall.right, Label::Region(2), "east is room 2");
    assert!(
        find_piece(&pieces, (28, 0), (32, 0)).is_none(),
        "the strip floor borders void both sides and is dropped"
    );
    assert!(
        find_piece(&pieces, (28, 10), (32, 10)).is_none(),
        "and the ceiling"
    );
    assert_clean(&pieces);
}

#[test]
fn crossing_diagonals_split_at_the_rounded_point() {
    let external = vec![
        ExternalSegment {
            a: (0, 0),
            b: (10, 10),
        },
        ExternalSegment {
            a: (0, 9),
            b: (9, 0),
        },
    ];
    let labeler = CountingLabeler::default();
    let pieces = arrange(&[], &external, &labeler).unwrap();
    assert!(labeler.calls() > 0, "the labeler was used");
    assert_eq!(pieces.len(), 4, "both diagonals split at (5, 5)");
    assert!(find_piece(&pieces, (0, 0), (5, 5)).is_some(), "first leg 1");
    assert!(
        find_piece(&pieces, (5, 5), (10, 10)).is_some(),
        "first leg 2"
    );
    assert!(
        find_piece(&pieces, (0, 9), (5, 5)).is_some(),
        "second leg 1"
    );
    assert!(
        find_piece(&pieces, (5, 5), (9, 0)).is_some(),
        "second leg 2"
    );
    assert_clean(&pieces);
}

#[test]
fn collinear_overlap_merges_into_one_piece_with_both_origins() {
    let external = vec![
        ExternalSegment {
            a: (0, 0),
            b: (10, 0),
        },
        ExternalSegment {
            a: (5, 0),
            b: (15, 0),
        },
    ];
    let pieces = arrange(&[], &external, &CountingLabeler::default()).unwrap();
    assert_eq!(pieces.len(), 3, "0-5, 5-10, 10-15");
    let middle = find_piece(&pieces, (5, 0), (10, 0)).unwrap();
    assert_eq!(middle.origin.len(), 2, "both external segments claim it");
    assert_eq!(
        middle.origin[0].source,
        SourceId::External(0),
        "first source"
    );
    assert_eq!(middle.origin[0].from_t, (1, 2), "half along the first");
    assert_eq!(
        middle.origin[1].source,
        SourceId::External(1),
        "second source"
    );
    assert_eq!(
        middle.origin[1].from_t,
        (0, 1),
        "at the start of the second"
    );
}

#[test]
fn external_segments_split_and_keep_origin_fractions() {
    let layers = vec![room(0, rect(0, 0, 10, 10))];
    let external = vec![ExternalSegment {
        a: (-5, 5),
        b: (15, 5),
    }];
    let labeler = CountingLabeler::default();
    let pieces = arrange(&layers, &external, &labeler).unwrap();
    assert!(labeler.calls() > 0, "the labeler callback was used");
    let before = find_piece(&pieces, (-5, 5), (0, 5)).unwrap();
    let inside = find_piece(&pieces, (0, 5), (10, 5)).unwrap();
    let after = find_piece(&pieces, (10, 5), (15, 5)).unwrap();
    let claim = |t: (i128, i128)| (SourceId::External(0), t);
    assert!(
        piece_claims(before).contains(&claim((0, 1))),
        "starts at zero: {:?}",
        piece_claims(before)
    );
    assert!(
        piece_claims(inside).contains(&claim((1, 4))),
        "one quarter along: {:?}",
        piece_claims(inside)
    );
    assert!(
        piece_claims(after).contains(&claim((3, 4))),
        "three quarters along: {:?}",
        piece_claims(after)
    );
    assert_eq!(
        inside.origin.len(),
        1,
        "a perpendicular crossing shares no piece"
    );
    assert_eq!(
        inside.origin[0].source,
        SourceId::External(0),
        "single external claim"
    );
    assert!(
        find_piece(&pieces, (0, 0), (0, 10)).is_none(),
        "the crossed wall is split at (0, 5)"
    );
    let wall_low = find_piece(&pieces, (0, 0), (0, 5)).unwrap();
    assert!(
        matches!(
            wall_low.origin[0].source,
            SourceId::Layer { layer: 0, edge: 3 }
        ),
        "wall halves keep the layer edge as their source"
    );
}

#[test]
fn degenerate_layers_are_invalid_arguments() {
    let bowtie = Layer {
        label: Label::Region(9),
        polygon: vec![(0, 0), (10, 10), (10, 0), (0, 5)],
    };
    let crossing_error = arrange(&[bowtie], &[], &LayerLabeler::new(&[])).unwrap_err();
    assert_eq!(crossing_error.code(), "invalid_argument", "self-crossing");
    let message = crossing_error.to_string();
    assert!(
        message.contains("layer 0")
            && message.contains("region 9")
            && message.contains("self-intersecting"),
        "names the layer and the fault: {message}"
    );

    let flat = Layer {
        label: Label::Void,
        polygon: vec![(0, 0), (5, 0), (10, 0)],
    };
    let flat_error = arrange(&[flat], &[], &LayerLabeler::new(&[])).unwrap_err();
    assert_eq!(flat_error.code(), "invalid_argument", "zero area");
    assert!(flat_error.to_string().contains("void"), "names the label");

    let few = Layer {
        label: Label::Region(1),
        polygon: vec![(0, 0), (5, 5), (5, 5)],
    };
    let few_error = arrange(&[few], &[], &LayerLabeler::new(&[])).unwrap_err();
    assert_eq!(few_error.code(), "invalid_argument", "too few vertices");
}

#[test]
fn more_than_the_limit_of_segments_is_rejected() {
    let external: Vec<ExternalSegment> = (0..200_001_u32)
        .map(|index| {
            let x = i64::from(index);
            ExternalSegment {
                a: (x, 0),
                b: (x.saturating_add(1), 0),
            }
        })
        .collect();
    let error = arrange(&[], &external, &LayerLabeler::new(&[])).unwrap_err();
    assert_eq!(error.code(), "limit_exceeded", "segment cap");
}

#[test]
fn one_cut_labelled_repeats_what_every_full_arrange_produces() {
    let rooms = vec![room(1, rect(0, 0, 32, 20)), room(2, rect(28, 10, 60, 30))];
    let external = vec![ExternalSegment {
        a: (10, -5),
        b: (10, 40),
    }];
    let cut = cut(&rooms, &external).expect("the stack cuts");
    let relabelled = cut.label(&CountingLabeler::default());
    let fresh = arrange(&rooms, &external, &CountingLabeler::default()).expect("arranges");
    assert_eq!(relabelled, fresh, "cut once, label, is the whole arrange");
    assert_clean(&cut.label(&LayerLabeler::new(&rooms)));
}

#[test]
fn a_cut_holds_its_pieces_unlabelled_for_several_rules() {
    let rooms = vec![room(1, rect(0, 0, 10, 10))];
    let cut = cut(&rooms, &[]).expect("the room cuts");
    let painter = cut.label(&LayerLabeler::new(&rooms));
    let counted = CountingLabeler::default();
    let every_side = cut.label(&counted);
    assert_eq!(painter.len(), 4, "the ring's four walls");
    assert_eq!(
        every_side.len(),
        4,
        "a pointwise labeler gives every side its own region: all four kept"
    );
    let again = cut.label(&LayerLabeler::new(&rooms));
    assert_eq!(again, painter, "one cut, relabelled, unchanged");
}
