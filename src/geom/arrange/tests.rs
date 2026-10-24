//! Shared helpers for the arrangement tests: the rectangle layer builders,
//! the piece lookup and origin reader, the counting counter labeler every
//! no-layer case labels with, the brute-force cleanliness invariant, and
//! the seeded generator the property topic draws random layer stacks from.
//!
//! `cases` runs the named pipeline cases against these helpers,
//! `properties` the determinism and random-rectangle invariants.

mod cases;
mod properties;

use core::cell::RefCell;

use super::segments_cross;
use crate::geom::Pt;
use crate::geom::label::{Label, Labeler, ScaledPt};
use crate::geom::segment::{SourceId, orient};
use crate::geom::{Layer, Piece};

/// A counter labeler: every side sample gets its own region, so no
/// piece is ever interior and every call is observable.
#[derive(Default)]
pub(super) struct CountingLabeler {
    /// Side samples asked about so far.
    calls: RefCell<u32>,
}

impl CountingLabeler {
    /// How many times the pipeline asked for a side label.
    pub(super) fn calls(&self) -> u32 {
        *self.calls.borrow()
    }
}

impl Labeler for CountingLabeler {
    fn label(&self, _point: ScaledPt) -> Label {
        let mut guard = self.calls.borrow_mut();
        *guard = guard.saturating_add(1);
        Label::Region(*guard)
    }
}

/// An axis-aligned rectangle ring, counter-clockwise.
pub(super) fn rect(low_x: i64, low_y: i64, high_x: i64, high_y: i64) -> Vec<Pt> {
    vec![
        (low_x, low_y),
        (high_x, low_y),
        (high_x, high_y),
        (low_x, high_y),
    ]
}

/// A region layer over a rectangle.
pub(super) fn room(id: u32, ring: Vec<Pt>) -> Layer {
    Layer {
        label: Label::Region(id),
        polygon: ring,
    }
}

/// Find the kept piece between two endpoints, either direction.
pub(super) fn find_piece(pieces: &[Piece], a: Pt, b: Pt) -> Option<&Piece> {
    pieces
        .iter()
        .find(|piece| (piece.a == a && piece.b == b) || (piece.a == b && piece.b == a))
}

/// One origin as a (source, fraction) pair.
type Claim = (SourceId, (i128, i128));

/// The source claims of a piece.
pub(super) fn piece_claims(piece: &Piece) -> Vec<Claim> {
    piece
        .origin
        .iter()
        .map(|origin| (origin.source, origin.from_t))
        .collect()
}

/// Whether two collinear segments share more than one point.
fn collinear_overlap(one: (Pt, Pt), two: (Pt, Pt)) -> bool {
    let (start_a, end_a) = one;
    let (start_b, end_b) = two;
    if orient(start_a, end_a, start_b) != 0 || orient(start_a, end_a, end_b) != 0 {
        return false;
    }
    let (low_a, high_a, low_b, high_b) = if start_a.0 == end_a.0 {
        (start_a.1, end_a.1, start_b.1, end_b.1)
    } else {
        (start_a.0, end_a.0, start_b.0, end_b.0)
    };
    let (first_low, first_high) = ordered(low_a, high_a);
    let (second_low, second_high) = ordered(low_b, high_b);
    second_low.max(first_low) < first_high.min(second_high)
}

/// `(low, high)` sorted.
fn ordered(low: i64, high: i64) -> (i64, i64) {
    if low <= high {
        (low, high)
    } else {
        (high, low)
    }
}

/// Whether one pair of output pieces is clean: they neither cross
/// properly nor overlap as collinear segments.
fn pair_is_clean(first: &Piece, second: &Piece) -> bool {
    let one = (first.a, first.b);
    let two = (second.a, second.b);
    !segments_cross(&one, &two) && !collinear_overlap(one, two)
}

/// Check `first` against every piece after `index`.
fn check_following(pieces: &[Piece], index: usize, first: &Piece) {
    for second in pieces.iter().skip(index.saturating_add(1)) {
        assert!(
            pair_is_clean(first, second),
            "pieces touch badly: {:?} x {:?}",
            (first.a, first.b),
            (second.a, second.b)
        );
    }
}

/// Brute-force invariant: no output piece properly crosses another,
/// and no two collinear pieces share an interior point.
pub(super) fn assert_clean(pieces: &[Piece]) {
    for (index, first) in pieces.iter().enumerate() {
        check_following(pieces, index, first);
    }
}

/// A tiny xorshift so the property cases are deterministic.
pub(super) struct Rng(u64);

impl Rng {
    /// A seeded generator; xorshift needs a nonzero state.
    pub(super) fn new(seed: u64) -> Self {
        let mixed = seed
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(0x0000_0100_0000_0001);
        Self(mixed.max(1))
    }

    /// The next pseudo-random word.
    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value.wrapping_shl(13);
        value ^= value.wrapping_shr(7);
        value ^= value.wrapping_shl(17);
        self.0 = value;
        value
    }

    /// The next pseudo-random value in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64().rem_euclid(bound)
    }
}

/// Void one cases in eight, else region `id`.
fn pick_label(rng: &mut Rng, id: u32) -> Label {
    if rng.below(8) == 0 {
        Label::Void
    } else {
        Label::Region(id)
    }
}

/// Half the time, flip the ring's winding.
fn maybe_reverse(rng: &mut Rng, polygon: &mut [Pt]) {
    if rng.below(2) == 0 {
        polygon.reverse();
    }
}

/// Three to six random rectangle layers on a 4-unit grid, some void.
pub(super) fn random_layers(rng: &mut Rng) -> Vec<Layer> {
    let count = rng.below(4).saturating_add(3);
    let mut layers = Vec::new();
    for index in 0..count {
        let low_x = i64::try_from(rng.below(48).saturating_mul(4)).unwrap();
        let low_y = i64::try_from(rng.below(48).saturating_mul(4)).unwrap();
        let wide = i64::try_from(rng.below(7).saturating_add(2))
            .unwrap()
            .saturating_mul(4);
        let tall = i64::try_from(rng.below(7).saturating_add(2))
            .unwrap()
            .saturating_mul(4);
        let id = u32::try_from(index).unwrap().saturating_add(1);
        let mut polygon = rect(
            low_x,
            low_y,
            low_x.saturating_add(wide),
            low_y.saturating_add(tall),
        );
        maybe_reverse(rng, &mut polygon);
        layers.push(Layer {
            label: pick_label(rng, id),
            polygon,
        });
    }
    layers
}

/// Every kept piece is oriented and carries at least one origin.
pub(super) fn check_kept(pieces: &[Piece], case: u64) {
    for piece in pieces {
        assert!(piece.a < piece.b, "case {case}: piece is oriented");
        assert!(!piece.origin.is_empty(), "case {case}: piece has an origin");
    }
}
