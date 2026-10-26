//! The column spans: what a behind-camera edge drops, what an
//! off-screen edge drops, the widened front span, the near-plane clip to
//! the left screen half, its mirror to the right half, and the
//! degenerate crossing through the camera point that keeps everything.

use alloc::vec;

use super::{bucket_edges, edge_span};
use crate::render::light::WallDir;
use crate::render::view::ViewCamera;
use crate::render::view::projection::Projection;
use crate::render::view::tests::horizontal;
use crate::render::view::walk::Edge;

/// The east-facing camera the spans are measured from: origin, level,
/// fov 90 over a 32-wide raster, so `u = -32y / x` and the column is
/// `(u + 31) / 2` (north leans left, the right vector being `(0, -1)`).
fn camera() -> ViewCamera {
    ViewCamera {
        x: 0,
        y: 0,
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    }
}

/// The 32x64 projection of [`camera`].
fn projection() -> Projection {
    Projection::new(&camera(), 32, 64)
}

/// A prepared edge from `(x0, y0)` to `(x1, y1)`; the span maths reads
/// only its endpoints, so the sidedef stubs never matter.
fn segment(x0: i64, y0: i64, x1: i64, y1: i64) -> Edge {
    Edge {
        line: 0,
        x0,
        y0,
        ex: x1.saturating_sub(x0),
        ey: y1.saturating_sub(y0),
        len16: 0,
        dir: WallDir::Other,
        front: 0,
        back: None,
        upper_unpegged: false,
        lower_unpegged: false,
    }
}

#[test]
fn an_edge_behind_the_camera_plane_spans_no_column() {
    let proj = projection();
    assert_eq!(edge_span(&horizontal(-200, 5, 100), &proj, (0, 0)), None);
    assert_eq!(edge_span(&horizontal(-10, 5, 10), &proj, (0, 0)), None);
    assert_eq!(
        edge_span(&horizontal(0, 5, 0), &proj, (0, 0)),
        None,
        "the zero-depth endpoint counts as behind"
    );
}

#[test]
fn an_edge_outside_the_field_of_view_spans_no_column() {
    let proj = projection();
    // Both endpoints sit at x 100..150 with y -1000: far north of the
    // 45-degree edge, whose column estimate is over 120.
    assert_eq!(
        edge_span(&horizontal(100, -1000, 50), &proj, (0, 0)),
        None,
        "nothing east-leaning reaches that far north"
    );
    assert_eq!(
        edge_span(&horizontal(100, 950, 50), &proj, (0, 0)),
        None,
        "nor west of the left screen edge"
    );
}

#[test]
fn a_front_edge_spans_its_projected_columns_widened_by_one() {
    let proj = projection();
    // (64, 0) to (96, 0): dead ahead, both endpoints land on column 15.
    assert_eq!(
        edge_span(&horizontal(64, 0, 32), &proj, (0, 0)),
        Some((14, 16))
    );
    // The wide wall at x 32 from y -100 to y 100 covers the frame.
    assert_eq!(
        edge_span(&segment(32, -100, 32, 100), &proj, (0, 0)),
        Some((0, 31))
    );
}

#[test]
fn a_clipped_edge_spans_the_half_towards_its_crossing() {
    let proj = projection();
    // (64, 32) into (-64, 32): the visible piece runs north (the left
    // screen half) from column 7 towards the crossing at the plane.
    assert_eq!(
        edge_span(&segment(64, 32, -64, 32), &proj, (0, 0)),
        Some((0, 8))
    );
    // The mirror south of the camera axis keeps the right half.
    assert_eq!(
        edge_span(&segment(64, -32, -64, -32), &proj, (0, 0)),
        Some((22, 31))
    );
}

#[test]
fn the_crossing_through_the_camera_point_keeps_every_column() {
    let proj = projection();
    // The edge's line runs straight through the camera's origin.
    assert_eq!(
        edge_span(&horizontal(-32, 0, 64), &proj, (0, 0)),
        Some((0, 31))
    );
}

#[test]
fn bucket_edges_files_edges_by_column_and_whole_width_spans_apart() {
    let proj = projection();
    let edges = vec![
        horizontal(-200, 5, 100),   // behind: dropped
        horizontal(64, 0, 32),      // columns 14..=16
        segment(64, 32, -64, 32),   // columns 0..=8
        segment(32, -100, 32, 100), // the whole width
    ];
    let bucketed = bucket_edges(&edges, &proj, (0, 0));
    assert_eq!(
        bucketed.always,
        vec![3_usize],
        "the whole-width edge is always tested"
    );
    let behind_kept = bucketed.columns.iter().flatten().any(|index| *index == 0);
    assert!(!behind_kept, "the behind-camera edge joins no column");
    assert!(
        bucketed
            .columns
            .get(15)
            .is_some_and(|bucket| bucket.contains(&1)),
        "the ahead edge sits in the centre column"
    );
    assert!(
        bucketed
            .columns
            .first()
            .is_some_and(|bucket| bucket.contains(&2)),
        "the clipped edge sits in the left column it spans"
    );
    assert!(
        bucketed
            .columns
            .last()
            .is_some_and(|bucket| !bucket.contains(&2)),
        "and in no column of the half it was clipped away from"
    );
}
