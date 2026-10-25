//! The rule the arrangement labels its samples with, and the containment
//! geometry it answers from.
//!
//! [`OverlayLabeler`] turns one sample point into a [`Label`]: inside the
//! paint polygon the paint wins unless `keep_interior` spares an original
//! sector that does not touch the boundary; outside it the [`OutsideRule`]
//! decides, and every non-default rule keeps the original map's answer
//! wherever the sample sits on an original wall.
//!
//! The original map's side of that answer is [`Shape`]: one sector's
//! boundary segments and bounding-box area, built by [`shape_for`] and
//! [`sector_shapes`] and read back by [`label_at`] with the even-odd,
//! smallest-box rule of [`crate::wad::map::geometry::point_in_sector`].
//! [`near_segment`] is the half-unit "sits on an original wall" test,
//! [`side_samples`] the two points the arrangement asks a piece's sides
//! about and [`side_normal`] the offset they sit at.

use crate::geom::Pt;
use crate::geom::label::{
    Label, Labeler, ScaledPt, contains_point, crosses_ray, scale_point, spans,
};
use crate::geom::segment::{delta, round_quotient};

use super::geometry::{
    box_area, grid_length, s_cross, s_delta, s_dist2, s_dot, segment_of, widen_box,
};
use super::{NEAR_HALF_SQ, OutsideRule, SAMPLE_HALF, SAMPLE_OFFSET, Shape, Source};

/// The [`Labeler`] one overlay arranges under.
pub(super) struct OverlayLabeler<'lbl> {
    /// The normalized paint polygon.
    pub(super) polygon: &'lbl [Pt],
    /// The label the paint puts inside the polygon.
    pub(super) new_label: Label,
    /// The original sectors.
    pub(super) shapes: &'lbl [Shape],
    /// Whether sectors not in `replaced` survive inside the polygon.
    pub(super) keep_interior: bool,
    /// Sorted ids of the sectors the paint replaces.
    pub(super) replaced: &'lbl [u32],
    /// The resolved outside rule.
    pub(super) outside: OutsideRule,
    /// The original wall segments, for the "sits on an original wall"
    /// test the non-default outside rules use.
    pub(super) walls: &'lbl [(Pt, Pt)],
}

impl Labeler for OverlayLabeler<'_> {
    fn label(&self, point: ScaledPt) -> Label {
        if contains_point(self.polygon, point) {
            return self.inside(point);
        }
        self.beyond(point)
    }
}

impl OverlayLabeler<'_> {
    /// The label at a point inside the paint polygon.
    fn inside(&self, point: ScaledPt) -> Label {
        if !self.keep_interior {
            return self.new_label;
        }
        match self.original(point) {
            Label::Region(id) if self.replaced.binary_search(&id).is_err() => Label::Region(id),
            Label::Region(_) | Label::Void => self.new_label,
        }
    }

    /// The label at a point outside the paint polygon.
    fn beyond(&self, point: ScaledPt) -> Label {
        match self.outside {
            OutsideRule::Original => self.original(point),
            OutsideRule::Solid => self.on_existing(point, Label::Void),
            OutsideRule::Region(id) => self.on_existing(point, Label::Region(id)),
        }
    }

    /// The original label where `point` sits on an existing wall, else
    /// the `off_wall` label the outside rule paints.
    fn on_existing(&self, point: ScaledPt, off_wall: Label) -> Label {
        if self.on_wall(point) {
            self.original(point)
        } else {
            off_wall
        }
    }

    /// The sector holding `point` in the original map, void when none.
    fn original(&self, point: ScaledPt) -> Label {
        label_at(self.shapes, point)
    }

    /// Whether `point` sits on (or within half a unit of) an original
    /// linedef segment.
    fn on_wall(&self, point: ScaledPt) -> bool {
        self.walls
            .iter()
            .any(|wall| near_segment(wall.0, wall.1, point))
    }
}

/// One sector's boundary segments and box area, `None` when it bounds
/// nothing the map can place or its index leaves the labeler's range.
fn shape_for(source: &Source, sector: usize) -> Option<Shape> {
    let mut edges: Vec<(Pt, Pt)> = Vec::new();
    let mut bbox: Option<(i64, i64, i64, i64)> = None;
    for line in &source.map.linedefs {
        let (front, back) = source.map.line_sectors(line);
        if front == back || (front != Some(sector) && back != Some(sector)) {
            continue;
        }
        let (a, b) = segment_of(source, line)?;
        edges.push((a, b));
        bbox = Some(widen_box(bbox, a, b));
    }
    if edges.is_empty() {
        return None;
    }
    Some(Shape {
        sector: u32::try_from(sector).ok()?,
        area: box_area(bbox.unwrap_or((0_i64, 0_i64, 0_i64, 0_i64))),
        edges,
    })
}

/// The containment shapes of every original sector, ascending.
pub(super) fn sector_shapes(source: &Source) -> Vec<Shape> {
    (0..source.map.sectors.len())
        .filter_map(|sector| shape_for(source, sector))
        .collect()
}

/// The sector holding `point`, mirroring
/// [`crate::wad::map::geometry::point_in_sector`] for sample-scaled
/// points: the containing shape with the smallest bounding box, lowest
/// index on ties, void when none holds it.
pub(super) fn label_at(shapes: &[Shape], point: ScaledPt) -> Label {
    let mut best: Option<(i128, u32)> = None;
    for shape in shapes {
        if !shape_contains(shape, point) {
            continue;
        }
        if best.is_none_or(|(area, _)| shape.area < area) {
            best = Some((shape.area, shape.sector));
        }
    }
    best.map_or(Label::Void, |(_, sector)| Label::Region(sector))
}

/// Even-odd containment of a scaled point in a sector's boundary
/// segments, either winding.
fn shape_contains(shape: &Shape, point: ScaledPt) -> bool {
    let mut inside = false;
    for &(from, to) in &shape.edges {
        let (low, high) = (scale_point(from), scale_point(to));
        if spans(low, high, point.1) && crosses_ray(low, high, point) {
            inside = !inside;
        }
    }
    inside
}

/// Whether `point` lies within half a map unit of the integer segment
/// `from` to `to`, in scaled coordinates.
fn near_segment(from: Pt, to: Pt, point: ScaledPt) -> bool {
    let (low, high) = (scale_point(from), scale_point(to));
    let direction = s_delta(low, high);
    let offset = s_delta(low, point);
    let squared = s_dot(direction, direction);
    if squared <= 0 {
        return s_dist2(low, point) <= NEAR_HALF_SQ;
    }
    let along = s_dot(direction, offset);
    if along <= 0 {
        return s_dist2(low, point) <= NEAR_HALF_SQ;
    }
    if along >= squared {
        return s_dist2(high, point) <= NEAR_HALF_SQ;
    }
    let cross = s_cross(direction, offset);
    cross.saturating_mul(cross) <= squared.saturating_mul(NEAR_HALF_SQ)
}

/// The sample points an eighth of a unit left and right of the piece
/// midpoint (the arrangement's own side samples).
pub(super) fn side_samples(start: Pt, finish: Pt) -> [ScaledPt; 2] {
    let centre = (
        i128::from(start.0)
            .saturating_add(i128::from(finish.0))
            .saturating_mul(SAMPLE_HALF),
        i128::from(start.1)
            .saturating_add(i128::from(finish.1))
            .saturating_mul(SAMPLE_HALF),
    );
    let normal = side_normal(start, finish);
    [
        (
            centre.0.saturating_add(normal.0),
            centre.1.saturating_add(normal.1),
        ),
        (
            centre.0.saturating_sub(normal.0),
            centre.1.saturating_sub(normal.1),
        ),
    ]
}

/// The unit normal of `start` to `finish` scaled to an eighth of a unit,
/// left positive.
pub(super) fn side_normal(start: Pt, finish: Pt) -> ScaledPt {
    let direction = delta(start, finish);
    let length = grid_length(start, finish);
    (
        round_quotient(
            direction.1.saturating_neg().saturating_mul(SAMPLE_OFFSET),
            length,
        )
        .unwrap_or(0_i128),
        round_quotient(direction.0.saturating_mul(SAMPLE_OFFSET), length).unwrap_or(0_i128),
    )
}
