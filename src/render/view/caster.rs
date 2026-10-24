//! The column caster: what one screen column walks, in which order.
//!
//! [`Caster::new`] prepares the castable [`Edge`]s and usable [`Side`]s
//! once per render; [`Caster::draw_column`] intersects every edge with the
//! column's direction, sorts the [`Hit`]s by exact ray parameter and
//! walks them near to far. Each visit fills the rows of the
//! [`super::walk::Window`] the faced sector's [`super::walk::Plane`]s own,
//! shrinks the window to the wall's own rows and then paints the solid
//! [`Slot::Middle`], or paints the [`Slot::Upper`] and [`Slot::Lower`]
//! bands and collects the middle seen through the transparent crossing.
//! Piercing, mapping and pegging run through the sibling `texels` module;
//! [`Walk::paint`] is the one place a pixel and its depth are written.

use alloc::vec::Vec;

use super::projection::Projection;
use super::texels::{
    edge, flat_texel, masked_tex_top, part, pierce, plane_point, side, texture_u16, wrap16,
};
use super::walk::{Edge, FacePlan, FlatRun, Hit, Part, Side, Slot, Surface, Visit, Walk};
use super::{BLACK, MaskedMiddle, ONE_16, ViewCamera};
use crate::gfx::ops::parse_hex_color;
use crate::render::canvas::Rgba;
use crate::render::light::{WallDir, colormap_index, shade};
use crate::render::scene::Scene;
use crate::render::settings::Lighting;

/// The column caster: the scene, the projection and the per-call
/// preparations the column walk reads.
pub(super) struct Caster<'scene> {
    /// The prepared scene.
    scene: &'scene Scene,
    /// The camera position in map units.
    cam: (i64, i64),
    /// Projection constants.
    pub(super) proj: Projection,
    /// Castable linedefs.
    edges: Vec<Edge>,
    /// Prepared sidedefs, indexed by sidedef.
    sides: Vec<Option<Side<'scene>>>,
    /// The background colour.
    pub(super) background: Rgba,
    /// The lighting model.
    lighting: Lighting,
}

impl<'scene> Caster<'scene> {
    /// Prepares one view render of `scene` from `camera`.
    pub(super) fn new(scene: &'scene Scene, camera: &ViewCamera, width: u32, height: u32) -> Self {
        let sky = scene.settings.sky.is_on().then_some(Surface::Sky);
        let edges = scene
            .map
            .linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| edge(index, &scene.map, line))
            .collect();
        let sides = (0..scene.map.sidedefs.len())
            .map(|index| side(index, scene, sky))
            .collect();
        Self {
            scene,
            cam: (i64::from(camera.x), i64::from(camera.y)),
            proj: Projection::new(camera, width, height),
            edges,
            sides,
            background: parse_hex_color(&scene.settings.background).unwrap_or(BLACK),
            lighting: scene.settings.lighting,
        }
    }

    /// Casts and walks one screen column; whatever the walk leaves
    /// uncovered keeps the background and the `i64::MAX` depth.
    pub(super) fn draw_column(&self, walk: &mut Walk<'_>) {
        for hit in self.hits(walk.d) {
            self.visit(walk, &hit);
        }
    }

    /// Every hit of direction `d`, sorted by ray parameter then linedef.
    fn hits(&self, d: (i128, i128)) -> Vec<Hit> {
        let mut found: Vec<Hit> = self
            .edges
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| self.hit(index, edge, d))
            .collect();
        found.sort_unstable_by(|a, b| a.t.cmp_value(b.t).then(a.edge.cmp(&b.edge)));
        found
    }

    /// The hit of `edge` with direction `d`, `None` when the ray passes
    /// through a back side that cannot be faced.
    fn hit(&self, index: usize, edge: &Edge, d: (i128, i128)) -> Option<Hit> {
        let (t, s, front) = pierce(self.cam, d, edge)?;
        let side = if front { edge.front } else { edge.back? };
        let opposite = if front { edge.back } else { Some(edge.front) };
        Some(Hit {
            edge: index,
            t,
            s,
            front,
            side,
            opposite,
        })
    }

    /// Gathers the hit's prepared facts, `None` when a side is missing.
    fn visit_of(&self, hit: &Hit) -> Option<Visit<'_>> {
        let edge = self.edges.get(hit.edge)?;
        let side = self.sides.get(hit.side).and_then(Option::as_ref)?;
        let far_side = hit
            .opposite
            .and_then(|index| self.sides.get(index).and_then(Option::as_ref));
        Some(Visit {
            edge,
            side_index: hit.side,
            side,
            far_side,
            t: hit.t,
            s: hit.s,
            front: hit.front,
            y_ceil: self.proj.row(side.plane.ceil, hit.t),
            y_floor: self.proj.row(side.plane.floor, hit.t),
            y_far_ceil: far_side.map(|far| self.proj.row(far.plane.ceil, hit.t)),
            y_far_floor: far_side.map(|far| self.proj.row(far.plane.floor, hit.t)),
        })
    }

    /// Processes one hit: fills the planes up to it, then draws (or
    /// seals) the wall it faces; a sealed column does nothing.
    fn visit(&self, walk: &mut Walk<'_>, hit: &Hit) {
        if walk.window.closed() {
            return;
        }
        let Some(visit) = self.visit_of(hit) else {
            walk.window.seal();
            return;
        };
        self.fill_planes(walk, &visit);
        walk.window.top = walk.window.top.max(visit.y_ceil);
        walk.window.bottom = walk.window.bottom.min(visit.y_floor);
        if walk.window.closed() {
            return;
        }
        if visit.far_side.is_none() {
            self.draw_face(walk, &visit, Slot::Middle);
            walk.window.seal();
            return;
        }
        self.draw_face(walk, &visit, Slot::Upper);
        self.draw_face(walk, &visit, Slot::Lower);
        if walk.window.closed() {
            return;
        }
        self.collect_masked(walk, &visit);
    }

    /// Fills the window rows the faced sector's planes own up to this
    /// hit: ceiling above, floor below.
    fn fill_planes(&self, walk: &mut Walk<'_>, visit: &Visit<'_>) {
        let plane = &visit.side.plane;
        if let Some(surface) = plane.ceil_surface {
            let run = FlatRun {
                top: walk.window.top,
                bottom: visit.y_ceil.min(walk.window.bottom),
                surface,
                height: plane.ceil,
                light: plane.light,
            };
            self.fill_flat(walk, &run);
        }
        if let Some(surface) = plane.floor_surface {
            let run = FlatRun {
                top: visit.y_floor.max(walk.window.top),
                bottom: walk.window.bottom,
                surface,
                height: plane.floor,
                light: plane.light,
            };
            self.fill_flat(walk, &run);
        }
    }

    /// Fills the run's rows from the plane's picture, each row sampled
    /// at its own world point and shaded by its own plane depth.
    fn fill_flat(&self, walk: &mut Walk<'_>, run: &FlatRun<'_>) {
        for row in run.top..=run.bottom {
            self.fill_flat_row(walk, run, row);
        }
    }

    /// Draws one row of a plane run; the horizon row (zero step, and so
    /// no ray parameter) skips, leaving the background.
    fn fill_flat_row(&self, walk: &mut Walk<'_>, run: &FlatRun<'_>, row: i64) {
        match run.surface {
            Surface::Sky => self.paint_sky_row(walk, row),
            Surface::Flat(_) => self.paint_flat_row(walk, run, row),
        }
    }

    /// Draws one row of a flat-covered plane: the world point the row's
    /// ray parameter lands on, the texel it wraps to on the world grid,
    /// shaded by that row's own plane depth. Rows the camera cannot see
    /// (no positive `t`, no texel, no pixel) skip.
    fn paint_flat_row(&self, walk: &mut Walk<'_>, run: &FlatRun<'_>, row: i64) {
        let Surface::Flat(image) = run.surface else {
            return;
        };
        let Some(t) = self.proj.plane_t(run.height, row) else {
            return;
        };
        let (x16, y16) = plane_point(self.cam, walk.d, t);
        let Some((texcol, texrow)) = flat_texel(x16, y16, image.width, image.height) else {
            return;
        };
        let Some((index, _)) = image.get(texcol, texrow) else {
            return;
        };
        let raw = self.proj.depth16(t).div_euclid(ONE_16);
        let depth = i64::try_from(raw).unwrap_or(i64::MAX);
        let map = colormap_index(self.lighting, run.light, depth, WallDir::Other);
        walk.paint(row, shade(self.scene, index, map), depth);
    }

    /// Draws one row of the sky: the ray angle's sky column, `Doom`'s
    /// vertical placement, full bright and infinitely deep.
    fn paint_sky_row(&self, walk: &mut Walk<'_>, row: i64) {
        let sky = &self.scene.sky;
        let Some((index, _)) = sky.get(walk.skycol, self.proj.sky_row(row, sky.height)) else {
            return;
        };
        walk.paint(row, shade(self.scene, index, 0), i64::MAX);
    }

    /// Paints `slot`'s texture across the window rows it owns (an empty
    /// `-` name leaves them to the background and later hits).
    fn draw_face(&self, walk: &mut Walk<'_>, visit: &Visit<'_>, slot: Slot) {
        let Some(span) = Self::face_span(walk, visit, slot) else {
            return;
        };
        if span.0 > span.1 {
            return;
        }
        let name = match slot {
            Slot::Upper => &visit.side.upper,
            Slot::Lower => &visit.side.lower,
            Slot::Middle => &visit.side.middle,
        };
        let Some(key) = name.as_deref() else {
            return;
        };
        let Some(image) = self.scene.textures.get(key) else {
            return;
        };
        let plan = FacePlan {
            visit,
            slot,
            image,
            span,
            depth16: self.proj.depth16(visit.t),
        };
        let part = part(&plan);
        self.paint_part(walk, &part);
    }

    /// The rows `slot`'s face covers, shrinking the window past it;
    /// `None` when the face does not exist geometrically.
    fn face_span(walk: &mut Walk<'_>, visit: &Visit<'_>, slot: Slot) -> Option<(i64, i64)> {
        match slot {
            Slot::Middle => Some((walk.window.top, walk.window.bottom)),
            Slot::Upper => Self::upper_span(walk, visit),
            Slot::Lower => Self::lower_span(walk, visit),
        }
    }

    /// The upper band: rows from the window top to the far ceiling,
    /// skipped when both sectors look at the sky.
    fn upper_span(walk: &mut Walk<'_>, visit: &Visit<'_>) -> Option<(i64, i64)> {
        let far = visit.far_side?;
        if far.plane.ceil >= visit.side.plane.ceil {
            return None;
        }
        if far.plane.ceil_sky && visit.side.plane.ceil_sky {
            return None;
        }
        let y = visit.y_far_ceil?;
        let span = (walk.window.top, y.min(walk.window.bottom));
        walk.window.top = walk.window.top.max(y);
        Some(span)
    }

    /// The lower band: rows from the far floor down to the window
    /// bottom, kept when the near ceiling is sky above a plain floor.
    fn lower_span(walk: &mut Walk<'_>, visit: &Visit<'_>) -> Option<(i64, i64)> {
        let far = visit.far_side?;
        if far.plane.floor <= visit.side.plane.floor {
            return None;
        }
        let y = visit.y_far_floor?;
        let span = (y.max(walk.window.top), walk.window.bottom);
        walk.window.bottom = walk.window.bottom.min(y);
        Some(span)
    }

    /// Paints the part's rows: one texture column, heights from the
    /// projection, one lighting table for the whole face.
    fn paint_part(&self, walk: &mut Walk<'_>, part: &Part<'_>) {
        for row in part.top..=part.bottom {
            self.paint_row(walk, part, row);
        }
    }

    /// Draws one wall row: the texture row the screen row projects to,
    /// through the whole-face lighting table, at the face's depth.
    fn paint_row(&self, walk: &mut Walk<'_>, part: &Part<'_>, row: i64) {
        let depth = i64::try_from(part.depth16.div_euclid(ONE_16)).unwrap_or(i64::MAX);
        let map = colormap_index(self.lighting, part.light, depth, part.dir);
        let v16 = part
            .tex_top16
            .saturating_sub(self.proj.height_at(row, part.t))
            .saturating_add(part.y_off16);
        let rows = i128::from(part.image.height);
        let Some((index, _)) = part.image.get(part.texcol, wrap16(v16, rows)) else {
            return;
        };
        walk.paint(row, shade(self.scene, index, map), depth);
    }

    /// Remembers the middle texture seen through this transparent
    /// crossing, mapped, placed, lit and stamped with the exact ray
    /// parameter, for the render module's transparent pass to draw.
    ///
    /// A crossing whose middle names a texture the scene lacks carries
    /// nothing drawable, so it collects nothing.
    fn collect_masked(&self, walk: &mut Walk<'_>, visit: &Visit<'_>) {
        let Some(key) = visit.side.middle.as_ref() else {
            return;
        };
        let Some(image) = self.scene.textures.get(key) else {
            return;
        };
        let near = &visit.side.plane;
        let far = visit.far_side.map_or(near, |side| &side.plane);
        let top = masked_tex_top(
            near,
            far,
            i64::from(image.height),
            visit.edge.lower_unpegged,
        );
        let u16 = texture_u16(visit);
        walk.masked.push(MaskedMiddle {
            column: u32::try_from(walk.column).unwrap_or(u32::MAX),
            line: visit.edge.line,
            side: visit.side_index,
            top: walk.window.top,
            bottom: walk.window.bottom,
            depth16: self.proj.depth16(visit.t),
            texture: key.to_owned(),
            texcol: wrap16(u16, i128::from(image.width)),
            tex_top16: i128::from(top).saturating_mul(ONE_16),
            y_off16: i128::from(visit.side.y_offset).saturating_mul(ONE_16),
            light: near.light,
            dir: visit.edge.dir,
            t: visit.t,
        });
    }
}
