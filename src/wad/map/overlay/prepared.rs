//! One overlay's prepared inputs: the borrowed view of the original map
//! every read of it goes through, and the steps that turn the paint
//! request into rebuilt rows.
//!
//! [`Prepared::new`] takes that view and runs the request checks that
//! need the map as it stands, so [`apply_overlay`](super::apply_overlay)
//! can be refused before anything is written. [`Prepared::replaced_sectors`]
//! then probes which sectors `keep_interior` spares, [`Prepared::arrange_pieces`]
//! runs the arrangement under the labeler of [`super::labeler`], and
//! [`Prepared::rebuild`] writes the pieces up as the rows of
//! [`super::rebuilt`], reporting them as an
//! [`OverlayResult`](super::OverlayResult).

use alloc::collections::BTreeSet;
use alloc::slice;

use crate::error::CyberError;
use crate::geom::label::Label;
use crate::geom::segment::{ExternalSegment, delta, dot};
use crate::geom::{Layer, Piece, Pt, arrange};
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef};

use super::geometry::{
    ceiling_height, check_outside, collect_replaced, dangling_ends, distance_point_segment,
    external_segments, first_external, floor_height, midpoint, nearer, plan_vertices, region_of,
    sector_id, segment_of, unit_length, void_front,
};
use super::labeler::{OverlayLabeler, sector_shapes};
use super::rebuilt::{
    Built, LineCtx, Plan, Rebuilt, normalize_ring, orient_piece, reconcile_textures, shell_of,
    transition_face,
};
use super::{
    NO_TEXTURE, Outside, OutsideRule, OverlayRequest, OverlayResult, Paint, Shape, Source,
    WallClaim, WallEnds,
};

/// Everything one overlay works from: the borrowed original map, the
/// derived containment shapes and the paint's own label.
pub(super) struct Prepared<'req, 'src> {
    /// The map as it stood, borrowed for the whole prepare phase.
    source: Source<'src>,
    /// The request being applied.
    request: &'req OverlayRequest,
    /// The polygon without repeated neighbours or a closing repeat.
    polygon: Vec<Pt>,
    /// The label the paint puts inside the polygon.
    new_label: Label,
    /// The index the appended sector will take, `None` for void paint.
    new_sector: Option<usize>,
    /// The sector record a [`Paint::Sector`] appends.
    new_record: Option<Sector>,
    /// The original sectors as containment shapes.
    shapes: Vec<Shape>,
    /// One segment per original linedef (zero-length for the lines whose
    /// vertices do not resolve), index-aligned with them.
    external: Vec<ExternalSegment>,
    /// The placeable original wall segments, for the outside rules'
    /// "sits on an original wall" test.
    walls: Vec<(Pt, Pt)>,
    /// Original linedef indices whose vertex references do not resolve.
    unresolved: Vec<usize>,
}

impl<'req, 'src> Prepared<'req, 'src> {
    /// Borrow `map` as the original and run every request check that
    /// needs the map as it stands.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] for an unknown [`Outside::Face`] sector
    /// and [`CyberError::LimitExceeded`] when the sector count leaves the
    /// labeler's range.
    pub(super) fn new(map: &'src Map, request: &'req OverlayRequest) -> Result<Self, CyberError> {
        check_outside(request, map)?;
        let count = map.sectors.len();
        let paint = request.paint.clone();
        let new_record = match paint {
            Paint::Sector(record) => Some(record),
            Paint::Void => None,
        };
        let new_label = match new_record {
            Some(_) => Label::Region(sector_id(count)?),
            None => Label::Void,
        };
        let source = Source { map };
        let shapes = sector_shapes(&source);
        let (external, unresolved) = external_segments(&source);
        let walls = external
            .iter()
            .filter(|segment| segment.a != segment.b)
            .map(|segment| (segment.a, segment.b))
            .collect();
        Ok(Self {
            new_sector: new_record.is_some().then_some(count),
            shapes,
            external,
            walls,
            unresolved,
            source,
            request,
            polygon: normalize_ring(&request.polygon),
            new_label,
            new_record,
        })
    }

    /// A labeler over this overlay's rule.
    fn labeler<'lbl>(
        &'lbl self,
        keep: bool,
        replaced: &'lbl [u32],
        outside: OutsideRule,
    ) -> OverlayLabeler<'lbl> {
        OverlayLabeler {
            polygon: &self.polygon,
            new_label: self.new_label,
            shapes: &self.shapes,
            keep_interior: keep,
            replaced,
            outside,
            walls: &self.walls,
        }
    }

    /// The resolved outside rule; [`Outside::Face`]'s index was checked
    /// in [`Self::new`].
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] if the checked index leaves the
    /// `u32` range (defensive).
    fn outside_rule(&self) -> Result<OutsideRule, CyberError> {
        match self.request.outside {
            Outside::Original => Ok(OutsideRule::Original),
            Outside::Solid => Ok(OutsideRule::Solid),
            Outside::Face(index) => Ok(OutsideRule::Region(sector_id(index)?)),
        }
    }

    /// The sectors touching the polygon boundary from inside, found by
    /// arranging the paint once with nothing kept and sampling the
    /// inside of every boundary piece in the original map.
    ///
    /// # Errors
    ///
    /// [`apply_overlay`](super::apply_overlay)'s arrangement errors.
    pub(super) fn replaced_sectors(&self) -> Result<Vec<u32>, CyberError> {
        let layer = Layer {
            label: self.new_label,
            polygon: self.request.polygon.clone(),
        };
        let probe = self.labeler(false, &[], OutsideRule::Original);
        let pieces = arrange(slice::from_ref(&layer), &self.external, &probe)?;
        Ok(collect_replaced(&pieces, &self.shapes, &self.polygon))
    }

    /// The final arrangement of the paint over the original lines.
    ///
    /// # Errors
    ///
    /// [`apply_overlay`](super::apply_overlay)'s arrangement errors.
    pub(super) fn arrange_pieces(&self, replaced: &[u32]) -> Result<Vec<Piece>, CyberError> {
        let layer = Layer {
            label: self.new_label,
            polygon: self.request.polygon.clone(),
        };
        let outside = self.outside_rule()?;
        let labeler = self.labeler(self.request.keep_interior, replaced, outside);
        arrange(slice::from_ref(&layer), &self.external, &labeler)
    }

    /// Rebuild the map's vertices, linedefs and sidedefs from `pieces`.
    ///
    /// # Errors
    ///
    /// [`apply_overlay`](super::apply_overlay)'s conversion and offset errors.
    pub(super) fn rebuild(&self, pieces: &[Piece]) -> Result<Rebuilt, CyberError> {
        let plan = plan_vertices(pieces, &self.source)?;
        let mut built = Built::default();
        for piece in pieces {
            self.push_piece(piece, &plan, &mut built)?;
        }
        let result = self.finish(&plan, &built);
        Ok(Rebuilt {
            vertices: plan.vertices,
            linedefs: built.linedefs,
            sidedefs: built.sidedefs,
            new_record: self.new_record.clone(),
            result,
        })
    }

    /// Append the new line one `piece` becomes to `built`.
    fn push_piece(&self, piece: &Piece, plan: &Plan, built: &mut Built) -> Result<(), CyberError> {
        let (start, finish, front_label, back_label) = orient_piece(piece);
        let old = first_external(piece);
        let source = old.and_then(|index| self.source.map.linedefs.get(index));
        let (start_len, end_len) =
            source.map_or(Ok((0_i32, 0_i32)), |line| self.shifts_for(line, piece))?;
        let front = region_of(front_label).ok_or_else(void_front)?;
        let ctx = LineCtx {
            source,
            start_len,
            end_len,
            front,
            back: region_of(back_label),
            start,
            finish,
        };
        let mut line = shell_of(&ctx, plan)?;
        let (front_side, front_fresh) = self.build_side(&ctx, ctx.front, true);
        line.front = Some(built.push_side(front_side, front_fresh));
        if let Some(back_sector) = ctx.back {
            let (back_side, back_fresh) = self.build_side(&ctx, back_sector, false);
            line.back = Some(built.push_side(back_side, back_fresh));
        }
        built.register(line, old);
        Ok(())
    }

    /// One side of a new line: the original side whose sector still
    /// matches (copied, offset-shifted and reconciled), else a freshly
    /// dressed side, and whether it was dressed from scratch.
    fn build_side(&self, ctx: &LineCtx<'_>, sector: usize, front_slot: bool) -> (Sidedef, bool) {
        let mut side = match ctx.copied_side(&self.source, sector, front_slot) {
            Some((mut original, shift)) => {
                original.x_offset = original.x_offset.wrapping_add(shift);
                (original, false)
            }
            None => (self.fresh_side(ctx, sector, front_slot), true),
        };
        if !side.1 {
            reconcile_textures(&mut side.0, ctx);
        }
        side
    }

    /// The distances from the original `line`'s ends to the `piece`'s
    /// near ends: the front side's shift and the back side's shift.
    fn shifts_for(&self, line: &Linedef, piece: &Piece) -> Result<(i32, i32), CyberError> {
        let (from, to) = segment_of(&self.source, line).ok_or_else(dangling_ends)?;
        let direction = delta(from, to);
        let first = dot(direction, delta(from, piece.a));
        let second = dot(direction, delta(from, piece.b));
        let (near, far) = if first <= second {
            (piece.a, piece.b)
        } else {
            (piece.b, piece.a)
        };
        Ok((unit_length(from, near)?, unit_length(to, far)?))
    }

    /// A side this call dresses from scratch: `wall` on the middle of a
    /// one-sided wall, transition faces where the two sectors' heights
    /// differ on a two-sided one.
    fn fresh_side(&self, ctx: &LineCtx<'_>, sector: usize, front_slot: bool) -> Sidedef {
        let neighbor = if front_slot {
            ctx.back
        } else {
            Some(ctx.front)
        };
        let mut side = Sidedef {
            upper: NO_TEXTURE.to_owned(),
            lower: NO_TEXTURE.to_owned(),
            middle: NO_TEXTURE.to_owned(),
            sector,
            ..Default::default()
        };
        if let Some(across) = neighbor {
            side.upper = transition_face(
                self.faces_differ(sector, across, ceiling_height),
                &self.request.textures.upper,
            );
            side.lower = transition_face(
                self.faces_differ(sector, across, floor_height),
                &self.request.textures.lower,
            );
        }
        if neighbor.is_none() {
            side.middle = self.wall_texture(ctx, sector);
        }
        side
    }

    /// The `middle` texture of a new one-sided wall: the request's wall
    /// texture, except under a [`Paint::Void`], which inherits the
    /// sector's own wall texture from the nearest original wall when it
    /// has one.
    fn wall_texture(&self, ctx: &LineCtx<'_>, sector: usize) -> String {
        if self.new_record.is_some() {
            return self.request.textures.wall.clone();
        }
        self.nearest_wall(sector, midpoint(ctx.start, ctx.finish))
            .unwrap_or_else(|| self.request.textures.wall.clone())
    }

    /// The wall candidates with their distance from `at`.
    fn wall_claims(&self, sector: usize, at: Pt) -> Vec<WallClaim> {
        self.wall_candidates(sector)
            .into_iter()
            .map(|(ends, name)| (ends, distance_point_segment(at, ends.0, ends.1), name))
            .collect()
    }

    /// The `middle` texture of `sector`'s sidedefs on the original linedef
    /// nearest to `at` (exact `i128` point-to-segment distance compared
    /// as fractions, ties first-wins in linedef order).
    fn nearest_wall(&self, sector: usize, at: Pt) -> Option<String> {
        self.wall_claims(sector, at)
            .into_iter()
            .reduce(nearer)
            .map(|(_, _, name)| name)
    }

    /// The original wall segments with a `middle` texture owned by
    /// `sector`, in linedef order (front side first).
    fn wall_candidates(&self, sector: usize) -> Vec<(WallEnds, String)> {
        self.source
            .map
            .linedefs
            .iter()
            .filter_map(|line| Some((segment_of(&self.source, line)?, line)))
            .filter_map(|(ends, line)| {
                let name = [line.front, line.back]
                    .into_iter()
                    .flatten()
                    .filter_map(|slot| self.source.map.sidedefs.get(slot))
                    .find(|side| side.sector == sector && side.middle != NO_TEXTURE)?;
                Some((ends, name.middle.clone()))
            })
            .collect()
    }

    /// Whether the two sectors' records answer `face` differently; an
    /// unresolvable neighbour counts as differing.
    fn faces_differ(&self, a: usize, b: usize, face: fn(&Sector) -> i32) -> bool {
        match (self.sector_record(a), self.sector_record(b)) {
            (Some(left), Some(right)) => face(left) != face(right),
            _ => true,
        }
    }

    /// The sector `id` names: an original record, or the paint's new one.
    fn sector_record(&self, id: usize) -> Option<&Sector> {
        if let Some(record) = self.source.map.sectors.get(id) {
            return Some(record);
        }
        match self.new_sector {
            Some(created) if created == id => self.new_record.as_ref(),
            Some(_) | None => None,
        }
    }

    /// The report of one finished rebuild.
    fn finish(&self, plan: &Plan, built: &Built) -> OverlayResult {
        let linedef_map = (0..self.source.map.linedefs.len())
            .map(|old| built.first_of.get(&old).copied())
            .collect::<Vec<Option<usize>>>();
        let empty_sectors = self.empty_sectors(built);
        let notes = self.notes(&empty_sectors);
        OverlayResult {
            sector: self.new_sector,
            vertices_created: plan.created.clone(),
            linedefs_created: built.created_lines.clone(),
            sidedefs_created: built.created_sides.clone(),
            linedef_map: linedef_map.clone(),
            removed_linedefs: linedef_map
                .iter()
                .enumerate()
                .filter_map(|(old, slot)| slot.is_none().then_some(old))
                .collect(),
            notes,
            empty_sectors,
        }
    }

    /// The sectors no new sidedef faces that the original map did face.
    fn empty_sectors(&self, built: &Built) -> Vec<usize> {
        let used: BTreeSet<usize> = built.sidedefs.iter().map(|row| row.sector).collect();
        let before: BTreeSet<usize> = self
            .source
            .map
            .sidedefs
            .iter()
            .map(|row| row.sector)
            .filter(|sector| *sector < self.source.map.sectors.len())
            .collect();
        let total = self
            .source
            .map
            .sectors
            .len()
            .saturating_add(usize::from(self.new_sector.is_some()));
        (0..total)
            .filter(|index| !used.contains(index) && before.contains(index))
            .collect()
    }

    /// The remarks: one per emptied sector, one per dropped unplaceable
    /// line.
    fn notes(&self, empty_sectors: &[usize]) -> Vec<String> {
        let mut notes: Vec<String> = empty_sectors
            .iter()
            .map(|index| {
                format!(
                    "sector {index} is left with no boundary (no sidedef faces it); \
                     it stays in the map - remove_sector it if you meant it gone"
                )
            })
            .collect();
        notes.extend(self.unresolved.iter().map(|index| {
            format!(
                "linedef {index} references vertices the map cannot resolve and was \
                 dropped by the rebuild"
            )
        }));
        notes
    }
}
