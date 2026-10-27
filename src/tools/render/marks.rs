//! Every `render_topdown` stroke: the linedef, sector-fill, vertex and
//! thing passes with their highlights, the index labels and free
//! annotations, and the isometric line and thing markers, all drawn onto
//! the [`Scene`] [`super::paint`] prepares.

use crate::fixed::{Angle, midpoint, rotate_point};
use crate::render::{Edge, Pt, Rgba, Stroke, TextStyle, draw_text, thing_visual};
use crate::wad::map::geometry;
use crate::wad::map::{Linedef, Thing, Vertex};

use super::frame::offset_pt;
use super::paint::{IsoPair, Scene, light_grey};
use super::params::Request;

use super::{HIGHLIGHT_RED, HIGHLIGHT_WIDTH, LINE_ONE_SIDED, LINE_TWO_SIDED};

/// Half edge of a vertex mark, in pixels (a `3x3` square).
const VERTEX_HALF: i64 = 1;
/// Smallest thing-marker radius, in pixels.
const MIN_THING_RADIUS: i64 = 2;
/// Extra length of a thing marker's facing tick, beyond its radius.
const TICK_PX: i64 = 4;
/// Radius of an isometric thing marker, in pixels.
const ISO_THING_RADIUS: i64 = 2;
/// The translucent red tint highlighted sectors get.
const HIGHLIGHT_TINT: Rgba = [255, 64, 64, 128];
/// The white outline highlighted things get.
const MARK_WHITE: Rgba = [255, 255, 255, 255];
/// The `3x3` vertex marks.
const VERTEX_MARK: Rgba = [255, 208, 0, 255];
/// The element-index label text.
pub(super) const LABEL_COLOUR: Rgba = [255, 255, 128, 255];
/// The outline under every overlay glyph.
const OUTLINE_COLOUR: Rgba = [0, 0, 0, 255];
/// How far a vertex label stands off its mark, in pixels.
const VERTEX_LABEL_OFFSET: i64 = 4;

/// Draws every linedef in its style colour and highlight width.
pub(super) fn paint_topdown_lines(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        paint_one_line(scene, line, index, request);
    }
}

/// Draws one linedef unless its style skips it.
fn paint_one_line(scene: &mut Scene<'_>, line: &Linedef, index: usize, request: &Request) {
    let Some((start, end)) = scene.line_pixels(line) else {
        return;
    };
    let Some((rgba, width)) = scene.line_mark(line, index, request) else {
        return;
    };
    scene
        .canvas
        .thick_line(start, end, Stroke::new(width, rgba));
}

/// Fills every sector with a grey of half its light, then tints the
/// highlighted ones red.
pub(super) fn paint_sector_fills(scene: &mut Scene<'_>, request: &Request) {
    for sector in 0..scene.map.sectors.len() {
        paint_one_sector(scene, sector, request);
    }
}

/// Fills (and maybe tints) one sector's polygon.
fn paint_one_sector(scene: &mut Scene<'_>, sector: usize, request: &Request) {
    let edges = scene.sector_edges(sector);
    if edges.is_empty() {
        return;
    }
    let light = scene.map.sectors.get(sector).map_or(0_i32, |row| row.light);
    scene.canvas.fill_polygon_evenodd(&edges, light_grey(light));
    if request.highlight.sectors.contains(&sector) {
        scene.canvas.blend_polygon_evenodd(&edges, HIGHLIGHT_TINT);
    }
}

/// Draws a `3x3` mark per vertex.
pub(super) fn paint_vertex_marks(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.vertices.iter().enumerate() {
        paint_one_vertex(scene, mark, index, request);
    }
}

/// Draws one vertex mark, red for the highlighted ones.
fn paint_one_vertex(scene: &mut Scene<'_>, mark: &Vertex, index: usize, request: &Request) {
    let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
    let rgba = if request.highlight.vertices.contains(&index) {
        HIGHLIGHT_RED
    } else {
        VERTEX_MARK
    };
    scene.canvas.fill_rect(
        offset_pt(center, VERTEX_HALF.saturating_neg()),
        offset_pt(center, VERTEX_HALF),
        rgba,
    );
}

/// Marks every thing with its category circle and facing tick.
pub(super) fn paint_thing_marks(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        paint_one_thing(scene, mark, index, request);
    }
}

/// Draws one thing: a scaled circle of its category colour plus a facing
/// tick; highlighted things get a white outline.
fn paint_one_thing(scene: &mut Scene<'_>, mark: &Thing, index: usize, request: &Request) {
    let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
    let (rgba, radius) = thing_visual(scene.defs, mark.kind);
    let pixels = scene.view.scale(i64::from(radius)).max(MIN_THING_RADIUS);
    if request.highlight.things.contains(&index) {
        scene
            .canvas
            .fill_circle(center, pixels.saturating_add(1), MARK_WHITE);
    }
    scene.canvas.fill_circle(center, pixels, rgba);
    let facing = rotate_point(
        pixels.saturating_add(TICK_PX),
        0_i64,
        Angle::from_degrees(i64::from(mark.angle)),
    );
    let tip = (
        center.0.saturating_add(facing.0),
        center.1.saturating_sub(facing.1),
    );
    scene.canvas.line(center, tip, rgba);
}

/// Draws every requested element's number, centred on its anchor, after
/// the map itself so the text always reads on top.
pub(super) fn paint_labels(scene: &mut Scene<'_>, request: &Request) {
    if request.labels.linedefs.is_on() {
        paint_linedef_labels(scene, request);
    }
    if request.labels.sectors.is_on() {
        paint_sector_labels(scene, request);
    }
    if request.labels.vertices.is_on() {
        paint_vertex_labels(scene, request);
    }
    if request.labels.things.is_on() {
        paint_thing_labels(scene, request);
    }
}

/// Draws one centred overlay number in the label colour.
fn label_number(scene: &mut Scene<'_>, at: Pt, index: usize, request: &Request) {
    let style = TextStyle {
        colour: LABEL_COLOUR,
        outline: OUTLINE_COLOUR,
        scale: request.label_scale,
    };
    draw_text(&mut scene.canvas, at, &index.to_string(), style);
}

/// Draws every linedef's index at its midpoint; a dangling vertex skips.
fn paint_linedef_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        if let Some((from, to)) = scene.line_pixels(line) {
            label_number(
                scene,
                (midpoint(from.0, to.0), midpoint(from.1, to.1)),
                index,
                request,
            );
        }
    }
}

/// Draws every sector's index at a point guaranteed inside it.
fn paint_sector_labels(scene: &mut Scene<'_>, request: &Request) {
    for sector in 0..scene.map.sectors.len() {
        let edges = scene.sector_edges(sector);
        if let Some(at) = sector_label_point(&edges) {
            label_number(scene, at, sector, request);
        }
    }
}

/// Draws every vertex's index beside its mark.
fn paint_vertex_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.vertices.iter().enumerate() {
        let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
        label_number(
            scene,
            offset_pt(center, VERTEX_LABEL_OFFSET),
            index,
            request,
        );
    }
}

/// Draws every thing's index beside its marker, a scaled radius away.
fn paint_thing_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
        let (_, radius) = thing_visual(scene.defs, mark.kind);
        let away = scene
            .view
            .scale(i64::from(radius))
            .max(MIN_THING_RADIUS)
            .saturating_add(VERTEX_HALF);
        label_number(scene, offset_pt(center, away), index, request);
    }
}

/// A point guaranteed inside the sector polygon drawn as pixel `edges`:
/// [`crate::geom`]'s one label-point scan — the middle row of the
/// bounding box is even-odd crossed and the widest inside interval's
/// midpoint answers, falling back to the first vertex — so the top-down
/// labels and [`crate::wad::map::describe`]'s sector centres are
/// identical by construction. An empty edge list has no anchor.
pub(super) fn sector_label_point(edges: &[Edge]) -> Option<Pt> {
    crate::geom::ring::label_point(edges)
}

/// Draws every annotation's text centred on its world position.
pub(super) fn paint_annotations(scene: &mut Scene<'_>, request: &Request) {
    for note in &request.annotations {
        let at = scene.view.point(note.x, note.y);
        let style = TextStyle {
            colour: note.colour,
            outline: OUTLINE_COLOUR,
            scale: request.label_scale,
        };
        draw_text(&mut scene.canvas, at, &note.text, style);
    }
}

/// Draws the isometric wireframe of every linedef.
pub(super) fn paint_iso_lines(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        paint_one_iso_line(scene, line, index, request);
    }
}

/// Draws one linedef's isometric edges: the front floor, the back floor
/// at a height change and, for one-sided lines, the ceiling and the two
/// vertical posts.
fn paint_one_iso_line(scene: &mut Scene<'_>, line: &Linedef, index: usize, request: &Request) {
    let Some(pair) = scene.iso_endpoints(line) else {
        return;
    };
    let stroke = iso_stroke(line, index, request);
    let floor = scene.side_floor(line.front).unwrap_or(0_i64);
    iso_span(scene, pair, floor, stroke);
    if let Some(back) = scene.side_floor(line.back).filter(|z| *z != floor) {
        iso_span(scene, pair, back, stroke);
    }
    if line.back.is_some() {
        return;
    }
    let ceiling = scene.side_ceiling(line.front).unwrap_or(0_i64);
    iso_span(scene, pair, ceiling, stroke);
    for corner in [pair.start, pair.end] {
        iso_post(scene, corner, (floor, ceiling), stroke);
    }
}

/// The stroke one isometric linedef gets: the highlight red and width
/// when selected, else the editor two-sided/one-sided greys.
fn iso_stroke(line: &Linedef, index: usize, request: &Request) -> Stroke {
    if request.highlight.linedefs.contains(&index) {
        return Stroke::new(HIGHLIGHT_WIDTH, HIGHLIGHT_RED);
    }
    if line.back.is_some() {
        return Stroke::thin(LINE_TWO_SIDED);
    }
    Stroke::thin(LINE_ONE_SIDED)
}

/// The isometric edge between the corners of `pair` at height `z`.
fn iso_span(scene: &mut Scene<'_>, pair: IsoPair, z: i64, stroke: Stroke) {
    let from = scene.view.iso(pair.start.0, pair.start.1, z);
    let to = scene.view.iso(pair.end.0, pair.end.1, z);
    scene.canvas.thick_line(from, to, stroke);
}

/// The vertical isometric post at one world corner, from the lower to
/// the higher of `heights`.
fn iso_post(scene: &mut Scene<'_>, corner: (i64, i64), heights: (i64, i64), stroke: Stroke) {
    let low = scene.view.iso(corner.0, corner.1, heights.0);
    let high = scene.view.iso(corner.0, corner.1, heights.1);
    scene.canvas.thick_line(low, high, stroke);
}

/// Marks each thing with a small circle at its sector's floor height.
pub(super) fn paint_iso_things(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        paint_one_iso_thing(scene, mark, index, request);
    }
}

/// Marks one thing at the floor height of the sector that holds it.
fn paint_one_iso_thing(scene: &mut Scene<'_>, mark: &Thing, index: usize, request: &Request) {
    let (x, y) = (i64::from(mark.x), i64::from(mark.y));
    let floor = geometry::point_in_sector(scene.map, x, y)
        .and_then(|sector| scene.map.sectors.get(sector))
        .map_or(0_i64, |sector| i64::from(sector.floor_height));
    let center = scene.view.iso(x, y, floor);
    let (rgba, _) = thing_visual(scene.defs, mark.kind);
    if request.highlight.things.contains(&index) {
        scene
            .canvas
            .fill_circle(center, ISO_THING_RADIUS.saturating_add(1), MARK_WHITE);
    }
    scene.canvas.fill_circle(center, ISO_THING_RADIUS, rgba);
}
