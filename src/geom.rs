//! Integer planar geometry: layer polygons split into side-labelled pieces.
//!
//! This is the pure geometry engine behind the drawing tools: given an
//! ordered stack of [`Layer`] polygons (painter's algorithm, topmost wins)
//! and any pre-existing [`ExternalSegment`]s, [`arrange`](fn@crate::geom::arrange)
//! cuts everything at every crossing, T-junction and overlap, drops
//! duplicate and interior pieces, and labels each remaining piece with the
//! [`Label`] on its left and right side. Every coordinate is an integer
//! [`Pt`], every test is an exact `i128` orientation or a documented
//! tolerance: no floats, so touching shapes, shared walls and
//! near-degenerate junctions never break the result. All drawing goes
//! through [`arrange`](fn@crate::geom::arrange).
//!
//! The pieces it returns are exactly the linedefs a `Doom` map needs:
//! non-overlapping, non-crossing segments with a sector label on each
//! side. The module carries no map or `MCP` types; the plan compiler and
//! drawing tools build on it.

pub mod arrange;
pub mod label;
pub mod segment;

pub use arrange::{Cut, arrange, cut};
pub use label::{Label, Labeler, LayerLabeler, SAMPLE_SCALE, ScaledPt};
pub use segment::{ExternalSegment, SegmentOrigin, SourceId};

/// An integer map point, `Doom` units.
pub type Pt = (i64, i64);

/// One painter's-algorithm layer: a labelled, simple polygon.
///
/// The ring may wind either way and may repeat its first vertex at the
/// end; repeated neighbours and a closing vertex are dropped before
/// validation. See
/// [`arrange`](fn@crate::geom::arrange) for the validity rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    /// The label this layer paints over everything below it.
    pub label: Label,
    /// The polygon's vertices, in order, closing implicitly to the first.
    pub polygon: Vec<Pt>,
}

/// One output segment: a piece of a cut line with its two side labels.
///
/// The piece spans `a` to `b` (inclusive), [`Piece::left`] is the label on
/// the left looking from `a` to `b`, [`Piece::right`] the other side, and
/// [`Piece::origin`] the source segment(s) it was cut from, with the
/// fraction along each where the piece starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    /// One end of the piece.
    pub a: Pt,
    /// The other end.
    pub b: Pt,
    /// Label to the left, looking from `a` to `b`.
    pub left: Label,
    /// Label to the right, looking from `a` to `b`.
    pub right: Label,
    /// The source segment(s) this piece came from, merged and sorted.
    pub origin: Vec<SegmentOrigin>,
}
