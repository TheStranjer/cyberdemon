//! Side labels for the planar arrangement and the painter's-algorithm default.
//!
//! A [`Label`] is what one side of a piece carries: a named region or
//! [`Label::Void`] (solid rock, everything no layer covers). The
//! [`Labeler`] trait answers "what covers this sample point?" so callers
//! can swap in their own rule (e.g. keep existing sectors);
//! [`LayerLabeler`] is the painter's default: the topmost layer whose
//! polygon contains the point, even-odd tested in `i128`.

use crate::geom::{Layer, Pt};

/// What one side of a piece carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Label {
    /// Solid rock / outside: everything no layer covers.
    Void,
    /// A named region.
    Region(u32),
}

impl Label {
    /// A short human-readable name for error messages.
    #[must_use]
    #[inline]
    pub fn describe(self) -> String {
        match self {
            Self::Void => "void".to_owned(),
            Self::Region(id) => format!("region {id}"),
        }
    }
}

/// Scale factor for side-sample coordinates: `2^20` units per map unit,
/// so a sample can sit a fraction of a unit off the piece.
pub const SAMPLE_SCALE: i128 = 0x10_0000;

/// A point in [`SAMPLE_SCALE`]-scaled integer coordinates.
pub type ScaledPt = (i128, i128);

/// Decides which [`Label`] covers a side sample of a piece.
pub trait Labeler {
    /// The label covering `point`, given in [`SAMPLE_SCALE`]-scaled
    /// integer coordinates.
    fn label(&self, point: ScaledPt) -> Label;
}

/// The default [`Labeler`]: the topmost layer whose polygon contains the
/// sample point, else [`Label::Void`].
///
/// A layer's own [`Label`] is reported even when it is
/// [`Label::Void`], so void layers punch holes. Containment uses the
/// even-odd rule on the raw integer vertices, tested entirely in `i128`.
#[derive(Clone, Copy, Debug)]
pub struct LayerLabeler<'layers> {
    /// The layers in painter's order, bottom first.
    layers: &'layers [Layer],
}

impl<'layers> LayerLabeler<'layers> {
    /// A labeler over `layers`, which are in painter's order (bottom
    /// first) as they are for
    /// [`arrange`](fn@crate::geom::arrange).
    #[must_use]
    #[inline]
    pub const fn new(layers: &'layers [Layer]) -> Self {
        Self { layers }
    }
}

impl Labeler for LayerLabeler<'_> {
    #[inline]
    fn label(&self, point: ScaledPt) -> Label {
        self.layers
            .iter()
            .rev()
            .find(|layer| contains_point(&layer.polygon, point))
            .map_or(Label::Void, |layer| layer.label)
    }
}

/// One vertex scaled to sample coordinates.
#[inline]
pub(crate) fn scale_point(point: Pt) -> ScaledPt {
    (
        i128::from(point.0).saturating_mul(SAMPLE_SCALE),
        i128::from(point.1).saturating_mul(SAMPLE_SCALE),
    )
}

/// Whether the edge `low`→`high` straddles the horizontal ray at `y`.
#[must_use]
#[inline]
pub(crate) const fn spans(low: ScaledPt, high: ScaledPt, y: i128) -> bool {
    (low.1 > y) != (high.1 > y)
}

/// Whether the ray from `point` towards `+x` crosses the edge `from`→`to`
/// left of `point` (the edge must straddle the ray's height).
#[must_use]
#[inline]
pub(crate) const fn crosses_ray(from: ScaledPt, to: ScaledPt, point: ScaledPt) -> bool {
    let (from_x, from_y) = from;
    let (to_x, to_y) = to;
    let (point_x, point_y) = point;
    let rise = to_y.saturating_sub(from_y);
    let run = to_x.saturating_sub(from_x);
    let lhs = point_x.saturating_sub(from_x).saturating_mul(rise);
    let rhs = run.saturating_mul(point_y.saturating_sub(from_y));
    if rise > 0 { lhs < rhs } else { lhs > rhs }
}

/// Even-odd containment of a scaled point in an integer polygon, either
/// winding; fewer than three vertices is never inside.
pub(crate) fn contains_point(polygon: &[Pt], point: ScaledPt) -> bool {
    let Some(first) = polygon.first().copied() else {
        return false;
    };
    let mut previous = scale_point(first);
    let mut inside = false;
    for vertex in polygon.iter().skip(1).chain(core::iter::once(&first)) {
        let current = scale_point(*vertex);
        if spans(previous, current, point.1) && crosses_ray(previous, current, point) {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::{Label, Labeler as _, LayerLabeler, SAMPLE_SCALE, contains_point, scale_point};
    use crate::geom::{Layer, Pt};

    /// A square ring from `low` to `high`.
    fn square(low: i64, high: i64) -> Vec<Pt> {
        vec![(low, low), (high, low), (high, high), (low, high)]
    }

    /// A scaled point at whole integer coordinates.
    fn at(x: i64, y: i64) -> (i128, i128) {
        (
            i128::from(x).saturating_mul(SAMPLE_SCALE),
            i128::from(y).saturating_mul(SAMPLE_SCALE),
        )
    }

    #[test]
    fn ordering_puts_void_below_every_region() {
        assert!(Label::Void < Label::Region(0), "void sorts first");
        assert!(Label::Region(0) < Label::Region(1), "ids order");
    }

    #[test]
    fn describe_names_void_and_region() {
        assert_eq!(Label::Void.describe(), "void", "void wording");
        assert_eq!(Label::Region(7).describe(), "region 7", "region wording");
    }

    #[test]
    fn scale_point_multiplies_by_the_sample_scale() {
        assert_eq!(
            scale_point((2, -3)),
            (2 * SAMPLE_SCALE, -3 * SAMPLE_SCALE),
            "exact"
        );
    }

    #[test]
    fn even_odd_contains_interior_not_boundary_or_exterior() {
        let unit = square(0, 10);
        assert!(contains_point(&unit, at(5, 5)), "centre inside");
        assert!(!contains_point(&unit, at(15, 5)), "outside");
        assert!(!contains_point(&unit, at(-1, -1)), "diagonal outside");
        assert!(!contains_point(&[], at(5, 5)), "empty polygon");
        assert!(
            !contains_point(&[(0, 0), (10, 0)], at(6, 2)),
            "a two-vertex spike encloses nothing"
        );
        assert!(
            contains_point(&[(0, 0), (10, 0), (10, 10)], at(6, 2)),
            "three vertices close a real triangle"
        );
    }

    #[test]
    fn even_odd_ignores_winding() {
        let clockwise: Vec<Pt> = square(0, 10).into_iter().rev().collect();
        assert!(
            contains_point(&clockwise, at(5, 5)),
            "reversed ring still inside"
        );
    }

    #[test]
    fn layer_labeler_picks_the_topmost_containing_layer() {
        let layers = vec![
            Layer {
                label: Label::Region(0),
                polygon: square(0, 20),
            },
            Layer {
                label: Label::Region(1),
                polygon: square(10, 30),
            },
            Layer {
                label: Label::Void,
                polygon: square(25, 26),
            },
        ];
        let labeler = LayerLabeler::new(&layers);
        assert_eq!(labeler.label(at(5, 5)), Label::Region(0), "bottom only");
        assert_eq!(labeler.label(at(15, 15)), Label::Region(1), "top wins");
        assert_eq!(labeler.label(at(25, 25)), Label::Void, "a void layer voids");
        assert_eq!(labeler.label(at(40, 40)), Label::Void, "nothing covers");
    }
}
