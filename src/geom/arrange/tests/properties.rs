//! Invariants over the whole pipeline: the same input arranges to the
//! same output, a seeded run of random axis-aligned rectangles never
//! leaves a crossing or overlapping piece behind, and degenerate input
//! contributes nothing.

use super::{CountingLabeler, Rng, assert_clean, check_kept, random_layers, rect, room};
use crate::geom::arrange::arrange;
use crate::geom::label::Label;
use crate::geom::segment::ExternalSegment;
use crate::geom::{Layer, LayerLabeler};

#[test]
fn the_same_input_produces_the_same_output() {
    let layers = vec![
        room(0, rect(0, 0, 10, 10)),
        room(1, rect(5, 5, 15, 15)),
        Layer {
            label: Label::Void,
            polygon: rect(12, 0, 13, 20),
        },
    ];
    let external = vec![ExternalSegment {
        a: (-4, 7),
        b: (18, 2),
    }];
    let first = arrange(&layers, &external, &LayerLabeler::new(&layers)).unwrap();
    let second = arrange(&layers, &external, &LayerLabeler::new(&layers)).unwrap();
    assert_eq!(first, second, "deterministic run to run");
    assert!(!first.is_empty(), "the fixture arranges something");
    assert_clean(&first);
}

#[test]
fn random_axis_aligned_rectangles_never_cross_or_overlap() {
    for case in 0..200_u64 {
        let mut rng = Rng::new(case);
        let layers = random_layers(&mut rng);
        let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers))
            .unwrap_or_else(|error| panic!("case {case} failed: {error}"));
        check_kept(&pieces, case);
        assert_clean(&pieces);
    }
}

#[test]
fn zero_length_inputs_do_not_produce_pieces() {
    let external = vec![ExternalSegment {
        a: (3, 3),
        b: (3, 3),
    }];
    let pieces = arrange(&[], &external, &CountingLabeler::default()).unwrap();
    assert!(pieces.is_empty(), "a point is not a segment");
}
