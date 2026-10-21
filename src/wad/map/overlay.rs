//! The shared overlay engine: one polygon painted onto an existing map.
//!
//! [`apply_overlay`] is the drawing step behind `create_sector_from_loop`
//! and `carve_void`: it folds a [`Layer`] polygon over the map's current
//! linedefs (fed to [`arrange`](fn@crate::geom::arrange) as [`ExternalSegment`]s), so
//! the result never crosses, overlaps or duplicates itself: touched walls
//! are split at every T-junction and crossing, painted-over walls drop,
//! and every surviving piece carries the sector label of each of its
//! sides. The pieces then become the map's new linedefs and sidedefs.
//!
//! The labelling rule: inside the polygon the paint wins (the new sector,
//! or [`Paint::Void`] for solid rock); outside it the sector found at the
//! sample point in the *original* map wins (the even-odd, smallest-box
//! rule of [`crate::wad::map::geometry::point_in_sector`] lifted to the
//! arrangement's sub-unit sample coordinates), so an existing wall the
//! polygon merely touches turns two-sided against the room beyond it and
//! a wall facing the void stays one-sided. `keep_interior` leaves
//! existing sectors that float strictly inside the polygon (pillars,
//! inner rooms) alone and repaints only the area of the sectors touching
//! the polygon boundary from inside. [`Outside::Solid`] and
//! [`Outside::Face`] bend only the brand-new boundary edges (samples off
//! every original wall): the first keeps new walls one-sided, the second
//! faces them at a named sector.
//!
//! Every linedef, sidedef and vertex of the map is rebuilt from the
//! arrangement's pieces, so indices can move: the [`OverlayResult`] maps
//! old linedef indices onto new (`linedef_map`), names the removed
//! linedefs and the sectors left without a boundary (kept, not deleted),
//! and the map's sectors keep their indices. A refused call leaves `map`
//! untouched, the way [`crate::wad::map::remove::apply_removal`] does:
//! every fallible step runs before the model is written back. All
//! arithmetic is integer: the arrangement's exact `i128` tests under
//! [`crate::geom`], checked `i64`/`i32` conversions here; no float ever
//! appears.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::slice;

use serde::Serialize;

use super::model::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::error::CyberError;
use crate::fixed::{hypot, isqrt};
use crate::geom::label::{
    Label, Labeler, ScaledPt, contains_point, crosses_ray, scale_point, spans,
};
use crate::geom::segment::{ExternalSegment, SourceId, cross, delta, dot, round_quotient};
use crate::geom::{Layer, Piece, Pt, arrange};

/// The `blocking` linedef flag bit: the flags of a new one-sided wall.
const BLOCKING_BIT: u32 = 1_u32;
/// The `twosided` linedef flag bit, the same value in both flag families.
const TWO_SIDED_BIT: u32 = 4_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
const NO_TEXTURE: &str = "-";
/// Half of `SAMPLE_SCALE`: turns an endpoint sum into the scaled midpoint.
const SAMPLE_HALF: i128 = 0x8_0000;
/// Side samples sit an eighth of a unit off the piece, along the unit
/// normal, in sample-scale units (the arrangement's own offset).
const SAMPLE_OFFSET: i128 = 0x2_0000;
/// Half a sample-scaled unit squared, `2^19 * 2^19`: the radius at which
/// a side sample counts as lying on a wall.
const NEAR_HALF_SQ: i128 = 0x40_0000_0000;
/// Divisor of the two-endpoint sum that gives a midpoint coordinate.
const MIDPOINT_DIVISOR: i128 = 2_i128;

/// The sector record [`Paint::Sector`] appends.
pub type SectorSpec = Sector;

/// What the painted polygon's interior becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paint {
    /// A new sector appended to [`Map::sectors`], facing every boundary
    /// wall's interior side.
    Sector(SectorSpec),
    /// Solid rock: the interior holds no sector, so surrounding sectors
    /// get one-sided walls and painted-over lines and sides vanish.
    Void,
}

/// What the brand-new boundary edges of the paint face outside the
/// polygon.
///
/// Samples sitting on an original wall always keep the original map's
/// answer, so existing geometry never loses a side to these modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outside {
    /// The sector found at the sample point in the original map, void
    /// when none holds it: an existing wall turns two-sided against the
    /// sector beyond it, a wall across nothing stays one-sided. The
    /// [`Default`].
    #[default]
    Original,
    /// Every new edge stays a blocking one-sided wall (the `"none"`
    /// outer mode).
    Solid,
    /// Every new edge becomes two-sided against this existing sector
    /// (the `outer_sector: index` mode).
    Face(
        /// The sector index to face.
        usize,
    ),
}

/// The textures the overlay puts on the sides it creates: `wall` as the
/// `middle` of a one-sided wall, `upper` and `lower` on the faces of a
/// two-sided transition, middle `"-"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallTextures {
    /// `middle` texture of a new one-sided wall's front side.
    pub wall: String,
    /// `upper` texture of a new two-sided transition (both faces).
    pub upper: String,
    /// `lower` texture of a new two-sided transition (both faces).
    pub lower: String,
}

/// A whole overlay: the polygon, what it paints, and how its walls are
/// dressed. The tool layer validates the strings and numeric fields for
/// the map's format; this module only ever reads them.
#[derive(Clone, Debug)]
pub struct OverlayRequest {
    /// The polygon's corners, in order, implicitly closed.
    pub polygon: Vec<Pt>,
    /// What the polygon's interior becomes.
    pub paint: Paint,
    /// Whether existing sectors strictly inside the polygon (pillars,
    /// inner rooms) keep their area (default `false`: everything inside
    /// is overwritten).
    pub keep_interior: bool,
    /// Textures for the sides this call creates.
    pub textures: WallTextures,
    /// What the new boundary edges face outside the polygon.
    pub outside: Outside,
}

/// What [`apply_overlay`] did, as the tools report it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OverlayResult {
    /// Index of the appended sector, `None` for a [`Paint::Void`].
    pub sector: Option<usize>,
    /// New indices of the created vertices, in ascending order.
    pub vertices_created: Vec<usize>,
    /// New indices of the linedefs no original line contributed to.
    pub linedefs_created: Vec<usize>,
    /// New indices of the sidedefs this call dressed from scratch
    /// (sides copied from the original map are reachable through their
    /// linedef and are not listed).
    pub sidedefs_created: Vec<usize>,
    /// Old linedef index to new index of its first surviving piece, or
    /// `None` when the line was painted over or split away.
    pub linedef_map: Vec<Option<usize>>,
    /// Old indices of the linedefs no surviving piece came from.
    pub removed_linedefs: Vec<usize>,
    /// Indices of the sectors left without a single sidedef. They are
    /// kept, not deleted; see also `notes`.
    pub empty_sectors: Vec<usize>,
    /// Human-readable remarks: every emptied sector and every linedef
    /// the map could not place (unresolvable vertex references).
    pub notes: Vec<String>,
}

/// One original wall segment's two endpoints.
type WallEnds = (Pt, Pt);

/// A squared distance as an exact fraction `(numerator, denominator)`.
type Distance = (i128, i128);

/// A nearest-wall candidate: its segment, distance and texture name.
type WallClaim = (WallEnds, Distance, String);

/// The map as it stood before the paint: the geometry the labels, splits
/// and copies consult while new rows are built.
#[derive(Clone, Debug)]
struct Source {
    /// The original vertices.
    vertices: Vec<Vertex>,
    /// The original linedefs.
    linedefs: Vec<Linedef>,
    /// The original sidedefs.
    sidedefs: Vec<Sidedef>,
    /// The original sectors.
    sectors: Vec<Sector>,
}

/// One original sector as a containment shape: its boundary segments and
/// bounding-box area, the way [`crate::wad::map::geometry::point_in_sector`]
/// picks the smallest holder.
struct Shape {
    /// The sector index.
    sector: u32,
    /// The boundary segments, in linedef order.
    edges: Vec<(Pt, Pt)>,
    /// The bounding box's area in square map units.
    area: i128,
}

/// The resolved [`Outside`] rule, with the sector index already checked
/// and widened.
#[derive(Clone, Copy, Debug)]
enum OutsideRule {
    /// The original map's sector at the sample point.
    Original,
    /// Void off every original wall.
    Solid,
    /// This sector off every original wall.
    Region(
        /// The sector to face.
        u32,
    ),
}

/// The [`Labeler`] one overlay arranges under.
struct OverlayLabeler<'lbl> {
    /// The normalized paint polygon.
    polygon: &'lbl [Pt],
    /// The label the paint puts inside the polygon.
    new_label: Label,
    /// The original sectors.
    shapes: &'lbl [Shape],
    /// Whether sectors not in `replaced` survive inside the polygon.
    keep_interior: bool,
    /// Sorted ids of the sectors the paint replaces.
    replaced: &'lbl [u32],
    /// The resolved outside rule.
    outside: OutsideRule,
    /// The original wall segments, for the "sits on an original wall"
    /// test the non-default outside rules use.
    walls: &'lbl [(Pt, Pt)],
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

/// Paint `request`'s polygon onto `map`, rebuilding the map's linedefs,
/// sidedefs and vertices from the resulting arrangement.
///
/// Sectors keep their indices; a [`Paint::Sector`] request appends its
/// record and reports it in [`OverlayResult::sector`]. Lines the polygon
/// touches mid-span are split, lines it crosses are cut, and lines or
/// line pieces whose two sides end up with the same label are dropped.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the polygon is degenerate or
/// self-crossing (the arrangement refuses it, naming the layer) or when a
/// split piece is too long to shift a texture offset by;
/// [`CyberError::NotFound`] for an [`Outside::Face`] sector the map does
/// not hold; [`CyberError::LimitExceeded`] when the arrangement leaves the
/// coordinate range or the sector list outgrows the labeler. On error
/// `map` is left untouched.
#[inline]
pub fn apply_overlay(map: &mut Map, request: &OverlayRequest) -> Result<OverlayResult, CyberError> {
    let prepared = Prepared::new(map, request)?;
    let replaced = if request.keep_interior {
        prepared.replaced_sectors()?
    } else {
        Vec::new()
    };
    let pieces = prepared.arrange_pieces(&replaced)?;
    let rebuilt = prepared.rebuild(&pieces)?;
    let result = rebuilt.result.clone();
    rebuilt.apply_to(map);
    Ok(result)
}

/// Everything one overlay works from: the original map, the derived
/// containment shapes and the paint's own label.
struct Prepared<'req> {
    /// The map as it stood.
    source: Source,
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

impl<'req> Prepared<'req> {
    /// Take the snapshot and run every request check that needs the map
    /// as it stands.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] for an unknown [`Outside::Face`] sector
    /// and [`CyberError::LimitExceeded`] when the sector count leaves the
    /// labeler's range.
    fn new(map: &Map, request: &'req OverlayRequest) -> Result<Self, CyberError> {
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
        let source = Source {
            vertices: map.vertices.clone(),
            linedefs: map.linedefs.clone(),
            sidedefs: map.sidedefs.clone(),
            sectors: map.sectors.clone(),
        };
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
    /// [`apply_overlay`]'s arrangement errors.
    fn replaced_sectors(&self) -> Result<Vec<u32>, CyberError> {
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
    /// [`apply_overlay`]'s arrangement errors.
    fn arrange_pieces(&self, replaced: &[u32]) -> Result<Vec<Piece>, CyberError> {
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
    /// [`apply_overlay`]'s conversion and offset errors.
    fn rebuild(&self, pieces: &[Piece]) -> Result<Rebuilt, CyberError> {
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
        let source = old.and_then(|index| self.source.linedefs.get(index));
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
            .linedefs
            .iter()
            .filter_map(|line| Some((segment_of(&self.source, line)?, line)))
            .filter_map(|(ends, line)| {
                let name = [line.front, line.back]
                    .into_iter()
                    .flatten()
                    .filter_map(|slot| self.source.sidedefs.get(slot))
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
        if let Some(record) = self.source.sectors.get(id) {
            return Some(record);
        }
        match self.new_sector {
            Some(created) if created == id => self.new_record.as_ref(),
            Some(_) | None => None,
        }
    }

    /// The report of one finished rebuild.
    fn finish(&self, plan: &Plan, built: &Built) -> OverlayResult {
        let linedef_map = (0..self.source.linedefs.len())
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
            .sidedefs
            .iter()
            .map(|row| row.sector)
            .filter(|sector| *sector < self.source.sectors.len())
            .collect();
        let total = self
            .source
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

/// The rebuilt rows plus the report, waiting to be written into the map.
struct Rebuilt {
    /// The new vertices.
    vertices: Vec<Vertex>,
    /// The new linedefs.
    linedefs: Vec<Linedef>,
    /// The new sidedefs.
    sidedefs: Vec<Sidedef>,
    /// The sector record a [`Paint::Sector`] appends.
    new_record: Option<Sector>,
    /// The report to return.
    result: OverlayResult,
}

impl Rebuilt {
    /// Write the rebuild into `map`. Infallible, so an error anywhere in
    /// [`apply_overlay`] leaves the model untouched.
    fn apply_to(self, map: &mut Map) {
        map.vertices = self.vertices;
        map.linedefs = self.linedefs;
        map.sidedefs = self.sidedefs;
        if let Some(record) = self.new_record {
            map.sectors.push(record);
        }
    }
}

/// The planned vertex rows: the rebuilt list, the grid lookup and the
/// indices of the appended corners.
struct Plan {
    /// The new vertex list: reused originals in index order, then the
    /// new points in ascending coordinate order.
    vertices: Vec<Vertex>,
    /// Grid point to index in `vertices`.
    index: BTreeMap<Pt, usize>,
    /// Indices of the appended vertices, ascending.
    created: Vec<usize>,
}

/// The linedefs and sidedefs under construction and their reports.
#[derive(Default)]
struct Built {
    /// The new linedefs, in piece order.
    linedefs: Vec<Linedef>,
    /// The new sidedefs, front before back per linedef.
    sidedefs: Vec<Sidedef>,
    /// New indices of linedefs no original line contributed to.
    created_lines: Vec<usize>,
    /// New indices of freshly dressed sidedefs.
    created_sides: Vec<usize>,
    /// Old linedef index to the new index of its first piece.
    first_of: BTreeMap<usize, usize>,
}

impl Built {
    /// Append one side and report its index, listing freshly dressed
    /// sides in `created_sides`.
    fn push_side(&mut self, side: Sidedef, fresh: bool) -> usize {
        let index = self.sidedefs.len();
        if fresh {
            self.created_sides.push(index);
        }
        self.sidedefs.push(side);
        index
    }

    /// Record the finished `line` and its origin.
    fn register(&mut self, line: Linedef, old: Option<usize>) {
        let index = self.linedefs.len();
        match old {
            Some(previous) => {
                self.first_of.entry(previous).or_insert(index);
            }
            None => self.created_lines.push(index),
        }
        self.linedefs.push(line);
    }
}

/// One piece's finished linedef context: where it came from and which
/// sectors its two sides face.
struct LineCtx<'ctx> {
    /// The original linedef the piece continues, when it runs along one.
    source: Option<&'ctx Linedef>,
    /// The original start's side shift: the distance from the original
    /// start to the piece's near end.
    start_len: i32,
    /// The original end's side shift: the distance from the piece's far
    /// end to the original end.
    end_len: i32,
    /// The sector the front side faces.
    front: usize,
    /// The sector the back side faces, `None` when the back is void.
    back: Option<usize>,
    /// The line's start point.
    start: Pt,
    /// The line's end point.
    finish: Pt,
}

impl LineCtx<'_> {
    /// The original side that still owns `sector` for this slot, with
    /// the offset shift that keeps its texture in place. The source's
    /// front side is measured from the line's start, its back side from
    /// the line's end, whichever way the new line runs.
    fn copied_side(
        &self,
        source: &Source,
        sector: usize,
        front_slot: bool,
    ) -> Option<(Sidedef, i32)> {
        let line = self.source?;
        let owns = |slot: Option<usize>, shift: i32| {
            slot.and_then(|index| source.sidedefs.get(index))
                .filter(|side| side.sector == sector)
                .cloned()
                .map(|side| (side, shift))
        };
        if front_slot {
            owns(line.front, self.start_len).or_else(|| owns(line.back, self.end_len))
        } else {
            owns(line.back, self.end_len).or_else(|| owns(line.front, self.start_len))
        }
    }
}

/// `texture` when the two sectors' face heights differ, `"-"` when they
/// match and the face would be invisible.
fn transition_face(differ: bool, texture: &str) -> String {
    if differ {
        return texture.to_owned();
    }
    NO_TEXTURE.to_owned()
}

/// The linedef shell for `ctx`: endpoints, normalized flags and the
/// original line's special, tag, args and extras.
fn shell_of(ctx: &LineCtx<'_>, plan: &Plan) -> Result<Linedef, CyberError> {
    let base = ctx.source.map_or(0_u32, |line| line.flags);
    let flags = if ctx.back.is_some() {
        (base | TWO_SIDED_BIT) & !BLOCKING_BIT
    } else {
        (base | BLOCKING_BIT) & !TWO_SIDED_BIT
    };
    Ok(Linedef {
        v1: plan_point(plan, ctx.start)?,
        v2: plan_point(plan, ctx.finish)?,
        flags,
        special: ctx.source.map_or(0_i32, |line| line.special),
        tag: ctx.source.map_or(0_i32, |line| line.tag),
        args: ctx.source.map_or([0_i32; 5], |line| line.args),
        front: None,
        back: None,
        extra: ctx
            .source
            .map_or_else(BTreeMap::new, |line| line.extra.clone()),
    })
}

/// The `Doom` orientation rule for one piece: the front side must be on
/// the right, so a piece whose right side is void runs backwards.
#[must_use]
const fn orient_piece(piece: &Piece) -> (Pt, Pt, Label, Label) {
    if matches!(piece.right, Label::Void) {
        (piece.b, piece.a, piece.left, piece.right)
    } else {
        (piece.a, piece.b, piece.right, piece.left)
    }
}

/// Whether a copied side needs its textures moved because the line's
/// sidedness changed: `middle` onto free faces when a one-sided line
/// opens, an invisible face onto `middle` when a two-sided line closes.
fn reconcile_textures(side: &mut Sidedef, ctx: &LineCtx<'_>) {
    let source_one_sided = ctx.source.is_none_or(|line| line.back.is_none());
    let now_two_sided = ctx.back.is_some();
    if source_one_sided && now_two_sided {
        move_middle_to_faces(side);
    }
    if !source_one_sided && !now_two_sided {
        move_faces_to_middle(side);
    }
}

/// Moves a side's middle texture onto its `upper` and `lower` where
/// those are free, then clears the middle.
fn move_middle_to_faces(side: &mut Sidedef) {
    if side.middle == NO_TEXTURE {
        return;
    }
    if side.upper == NO_TEXTURE {
        side.upper = side.middle.clone();
    }
    if side.lower == NO_TEXTURE {
        side.lower = side.middle.clone();
    }
    NO_TEXTURE.clone_into(&mut side.middle);
}

/// Moves an otherwise invisible `upper` (else `lower`) texture onto the
/// middle of a side whose line has just closed into one side.
fn move_faces_to_middle(side: &mut Sidedef) {
    if side.middle != NO_TEXTURE {
        return;
    }
    if let Some(texture) = [side.upper.as_str(), side.lower.as_str()]
        .into_iter()
        .find(|name| *name != NO_TEXTURE)
    {
        texture.clone_into(&mut side.middle);
    }
    NO_TEXTURE.clone_into(&mut side.upper);
    NO_TEXTURE.clone_into(&mut side.lower);
}

/// Drop repeated neighbours and a closing repeat so every ring edge has
/// positive length (the arrangement's own normalization, needed here for
/// the containment probes).
fn normalize_ring(points: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(points.len());
    for point in points {
        if out.last() != Some(point) {
            out.push(*point);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// One sector's boundary segments and box area, `None` when it bounds
/// nothing the map can place or its index leaves the labeler's range.
fn shape_for(source: &Source, sector: usize) -> Option<Shape> {
    let mut edges: Vec<(Pt, Pt)> = Vec::new();
    let mut bbox: Option<(i64, i64, i64, i64)> = None;
    for line in &source.linedefs {
        let front = side_sector(source, line.front);
        let back = side_sector(source, line.back);
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
fn sector_shapes(source: &Source) -> Vec<Shape> {
    (0..source.sectors.len())
        .filter_map(|sector| shape_for(source, sector))
        .collect()
}

/// The sector holding `point`, mirroring
/// [`crate::wad::map::geometry::point_in_sector`] for sample-scaled
/// points: the containing shape with the smallest bounding box, lowest
/// index on ties, void when none holds it.
fn label_at(shapes: &[Shape], point: ScaledPt) -> Label {
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
fn side_samples(start: Pt, finish: Pt) -> [ScaledPt; 2] {
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
fn side_normal(start: Pt, finish: Pt) -> ScaledPt {
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

/// The integer length of `start` to `finish`, at least one.
fn grid_length(start: Pt, finish: Pt) -> i128 {
    let direction = delta(start, finish);
    let squared = dot(direction, direction);
    let magnitude = u64::try_from(squared).unwrap_or(u64::MAX);
    i128::from(isqrt(magnitude)).max(1)
}

/// The original sectors touching the paint polygon's boundary from
/// inside: every boundary piece's inside sample.
fn collect_replaced(pieces: &[Piece], shapes: &[Shape], polygon: &[Pt]) -> Vec<u32> {
    let mut replaced: Vec<u32> = pieces
        .iter()
        .filter(|piece| has_layer_origin(piece))
        .flat_map(|piece| side_samples(piece.a, piece.b))
        .filter(|sample| contains_point(polygon, *sample))
        .filter_map(|sample| match label_at(shapes, sample) {
            Label::Region(id) => Some(id),
            Label::Void => None,
        })
        .collect();
    replaced.sort_unstable();
    replaced.dedup();
    replaced
}

/// Whether the piece was cut from the paint polygon's own boundary.
fn has_layer_origin(piece: &Piece) -> bool {
    piece
        .origin
        .iter()
        .any(|origin| matches!(origin.source, SourceId::Layer { .. }))
}

/// The original linedef index the piece continues (the first of its
/// external claims, the smallest when it overlaps several).
fn first_external(piece: &Piece) -> Option<usize> {
    piece.origin.iter().find_map(|origin| match origin.source {
        SourceId::External(index) => Some(index),
        SourceId::Layer { .. } => None,
    })
}

/// The sector a label names, `None` for void.
#[must_use]
fn region_of(label: Label) -> Option<usize> {
    match label {
        Label::Void => None,
        Label::Region(id) => usize::try_from(id).ok(),
    }
}

/// The vertex plan for `pieces`: reused originals in index order, then
/// the new grid points in ascending coordinate order.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a merged point leaves the `i32`
/// coordinate range.
fn plan_vertices(pieces: &[Piece], source: &Source) -> Result<Plan, CyberError> {
    let wanted: BTreeSet<Pt> = pieces.iter().flat_map(|piece| [piece.a, piece.b]).collect();
    let mut index: BTreeMap<Pt, usize> = BTreeMap::new();
    let mut vertices: Vec<Vertex> = Vec::new();
    for vertex in &source.vertices {
        let point = (i64::from(vertex.x), i64::from(vertex.y));
        if wanted.contains(&point) && !index.contains_key(&point) {
            index.insert(point, vertices.len());
            vertices.push(vertex.clone());
        }
    }
    let mut created = Vec::new();
    for point in wanted {
        if index.contains_key(&point) {
            continue;
        }
        index.insert(point, vertices.len());
        vertices.push(grid_vertex(point)?);
        created.push(vertices.len().saturating_sub(1));
    }
    Ok(Plan {
        vertices,
        index,
        created,
    })
}

/// The [`Vertex`] for a grid point that needs one.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a coordinate leaves the `i32`
/// range.
fn grid_vertex(point: Pt) -> Result<Vertex, CyberError> {
    let grid = (i32::try_from(point.0), i32::try_from(point.1));
    match grid {
        (Ok(x), Ok(y)) => Ok(Vertex {
            x,
            y,
            ..Default::default()
        }),
        _ => Err(CyberError::LimitExceeded(format!(
            "the arrangement point {}, {} leaves the map coordinate range",
            point.0, point.1
        ))),
    }
}

/// The index of the planned vertex at `point`.
fn plan_point(plan: &Plan, point: Pt) -> Result<usize, CyberError> {
    plan.index
        .get(&point)
        .copied()
        .ok_or_else(|| CyberError::Internal(format!("overlay lost the grid point {point:?}")))
}

/// The [`Linedef`] segment of `line`'s two vertices, `None` when either
/// reference dangles.
fn segment_of(source: &Source, line: &Linedef) -> Option<(Pt, Pt)> {
    let start = source.vertices.get(line.v1)?;
    let end = source.vertices.get(line.v2)?;
    Some((
        (i64::from(start.x), i64::from(start.y)),
        (i64::from(end.x), i64::from(end.y)),
    ))
}

/// The sector behind an optional side reference.
fn side_sector(source: &Source, side: Option<usize>) -> Option<usize> {
    side.and_then(|index| source.sidedefs.get(index))
        .map(|row| row.sector)
}

/// One external segment per linedef, in linedef order; unresolvable
/// lines become zero-length segments the arrangement drops, keeping the
/// indices aligned.
fn external_segments(source: &Source) -> (Vec<ExternalSegment>, Vec<usize>) {
    let mut external = Vec::with_capacity(source.linedefs.len());
    let mut unresolved = Vec::new();
    for (index, line) in source.linedefs.iter().enumerate() {
        if let Some((a, b)) = segment_of(source, line) {
            external.push(ExternalSegment { a, b });
        } else {
            external.push(ExternalSegment {
                a: (0, 0),
                b: (0, 0),
            });
            unresolved.push(index);
        }
    }
    (external, unresolved)
}

/// Refuse an [`Outside::Face`] sector the map does not hold.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the index and the sector count.
fn check_outside(request: &OverlayRequest, map: &Map) -> Result<(), CyberError> {
    match request.outside {
        Outside::Face(index) if index >= map.sectors.len() => {
            let count = map.sectors.len();
            let range = count.checked_sub(1).map_or_else(
                || "the map has no sectors".to_owned(),
                |last| format!("the map has {count} sectors, valid indices are 0..={last}"),
            );
            Err(CyberError::NotFound(format!(
                "sector {index} does not exist ({range}): pass an existing \
                 sector index, \"auto\", or \"none\""
            ))
            .with_hint("describe_map or find_elements returns indices"))
        }
        Outside::Original | Outside::Solid | Outside::Face(_) => Ok(()),
    }
}

/// The `u32` label id of sector `index`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the index leaves the labeler's
/// range.
fn sector_id(index: usize) -> Result<u32, CyberError> {
    u32::try_from(index).map_err(|_error| {
        CyberError::LimitExceeded(format!(
            "sector {index} is past the {} the painter can label",
            u32::MAX
        ))
    })
}

/// The `(x, y)` midpoint of two grid points, truncated toward zero.
fn midpoint(from: Pt, to: Pt) -> Pt {
    (half_sum(from.0, to.0), half_sum(from.1, to.1))
}

/// `(from + to) / 2` truncated toward zero, overflow-free.
fn half_sum(from: i64, to: i64) -> i64 {
    let sum = i128::from(from).saturating_add(i128::from(to));
    let half = sum.checked_div(MIDPOINT_DIVISOR).unwrap_or(sum);
    i64::try_from(half).unwrap_or(from)
}

/// The floored `hypot` length of a grid segment, refused when no offset
/// can be shifted by it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the piece is longer than
/// `i32::MAX` map units.
fn unit_length(from: Pt, to: Pt) -> Result<i32, CyberError> {
    let length = hypot(to.0.saturating_sub(from.0), to.1.saturating_sub(from.1));
    i32::try_from(length).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "the split piece is {length} units long, past what an x_offset can shift by"
        ))
    })
}

/// The box covering `bbox` and a segment's endpoints.
fn widen_box(bbox: Option<(i64, i64, i64, i64)>, a: Pt, b: Pt) -> (i64, i64, i64, i64) {
    let (min_x, min_y, max_x, max_y) = bbox.unwrap_or((a.0, a.1, a.0, a.1));
    (
        min_x.min(a.0).min(b.0),
        min_y.min(a.1).min(b.1),
        max_x.max(a.0).max(b.0),
        max_y.max(a.1).max(b.1),
    )
}

/// The area of a bounding box, saturating.
fn box_area((min_x, min_y, max_x, max_y): (i64, i64, i64, i64)) -> i128 {
    let width = i128::from(max_x)
        .saturating_sub(i128::from(min_x))
        .saturating_add(1);
    let height = i128::from(max_y)
        .saturating_sub(i128::from(min_y))
        .saturating_add(1);
    width.saturating_mul(height)
}

/// The squared distance from a grid point to a grid segment, as an
/// exact fraction `(numerator, denominator)`.
fn distance_point_segment(point: Pt, from: Pt, to: Pt) -> (i128, i128) {
    let direction = delta(from, to);
    let offset = delta(from, point);
    let squared = dot(direction, direction);
    let along = dot(direction, offset);
    if squared <= 0 || along <= 0 {
        return (dot(offset, offset), 1_i128);
    }
    if along >= squared {
        let far = delta(to, point);
        return (dot(far, far), 1_i128);
    }
    let cross = cross(direction, offset);
    (cross.saturating_mul(cross), squared)
}

/// Whether fraction `left` is strictly below fraction `right` (saturated
/// products; equal or indistinguishable counts as not closer).
const fn closer(left: Distance, right: Distance) -> bool {
    left.0.saturating_mul(right.1) < right.0.saturating_mul(left.1)
}

/// The nearer of two wall claims, keeping the first on ties.
fn nearer(best: WallClaim, claim: WallClaim) -> WallClaim {
    if closer(claim.1, best.1) { claim } else { best }
}

/// The sector's ceiling height, a `faces_differ` probe.
const fn ceiling_height(sector: &Sector) -> i32 {
    sector.ceiling_height
}

/// The sector's floor height, a `faces_differ` probe.
const fn floor_height(sector: &Sector) -> i32 {
    sector.floor_height
}

/// `to - from` in scaled coordinates.
const fn s_delta(from: ScaledPt, to: ScaledPt) -> ScaledPt {
    (to.0.saturating_sub(from.0), to.1.saturating_sub(from.1))
}

/// The `i128` dot product of two scaled vectors.
const fn s_dot(left: ScaledPt, right: ScaledPt) -> i128 {
    left.0
        .saturating_mul(right.0)
        .saturating_add(left.1.saturating_mul(right.1))
}

/// The `i128` cross product of two scaled vectors.
const fn s_cross(left: ScaledPt, right: ScaledPt) -> i128 {
    left.0
        .saturating_mul(right.1)
        .saturating_sub(left.1.saturating_mul(right.0))
}

/// The squared scaled distance between two sample points.
const fn s_dist2(from: ScaledPt, to: ScaledPt) -> i128 {
    let step = s_delta(from, to);
    s_dot(step, step)
}

/// The `Internal` for a source line whose vertices vanished after the
/// external segments were built (defensive: they were checked there).
fn dangling_ends() -> CyberError {
    CyberError::Internal("overlay lost a source linedef's vertices".to_owned())
}

/// The `Internal` for a piece whose front side came out void (the
/// orientation rule makes that impossible).
fn void_front() -> CyberError {
    CyberError::Internal("overlay found a void front side".to_owned())
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{
        Outside, OverlayRequest, Paint, WallTextures, apply_overlay, midpoint, normalize_ring,
    };
    use crate::defs::definitions_for;
    use crate::error::CyberError;
    use crate::wad::game::Game;
    use crate::wad::map::Sector;
    use crate::wad::map::common::fixtures::{name, room, two_room_map};
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::validate::{Context, Severity, run};
    use crate::wad::map::{Map, MapFormat};

    /// Both endpoint coordinates of a linedef.
    type Ends = ((i64, i64), (i64, i64));

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A wall-texture set for tests.
    fn textures() -> WallTextures {
        WallTextures {
            wall: "W1".to_owned(),
            upper: "UP".to_owned(),
            lower: "LO".to_owned(),
        }
    }

    /// A request painting `sector` over `polygon`.
    fn paint_sector(polygon: &[(i64, i64)], sector: Sector) -> OverlayRequest {
        OverlayRequest {
            polygon: polygon.to_vec(),
            paint: Paint::Sector(sector),
            keep_interior: false,
            textures: textures(),
            outside: Outside::Original,
        }
    }

    /// A request painting a plain room over `polygon`.
    fn room_request(polygon: &[(i64, i64)]) -> OverlayRequest {
        paint_sector(polygon, room(0, 128, 160, 0))
    }

    /// A request carving `polygon` into solid rock.
    fn void_request(polygon: &[(i64, i64)]) -> OverlayRequest {
        OverlayRequest {
            polygon: polygon.to_vec(),
            paint: Paint::Void,
            keep_interior: false,
            textures: textures(),
            outside: Outside::Original,
        }
    }

    /// An empty `Doom` map to draw into.
    fn empty_map() -> Map {
        Map::new_empty(name("MAP01"), MapFormat::Doom)
    }

    /// A rectangle ring; either winding arranges the same.
    fn rect(low_x: i64, low_y: i64, high_x: i64, high_y: i64) -> Vec<(i64, i64)> {
        vec![
            (low_x, low_y),
            (high_x, low_y),
            (high_x, high_y),
            (low_x, high_y),
        ]
    }

    /// Both endpoint coordinates of linedef `index`.
    fn ends(map: &Map, index: usize) -> Ends {
        let line = map.linedefs.get(index).expect("the linedef exists");
        let at = |vertex: usize| {
            let row = map.vertices.get(vertex).expect("the vertex exists");
            (i64::from(row.x), i64::from(row.y))
        };
        (at(line.v1), at(line.v2))
    }

    /// The exact `i128` orientation of three points.
    fn orient(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i128 {
        let ab = (
            i128::from(b.0).saturating_sub(i128::from(a.0)),
            i128::from(b.1).saturating_sub(i128::from(a.1)),
        );
        let ac = (
            i128::from(c.0).saturating_sub(i128::from(a.0)),
            i128::from(c.1).saturating_sub(i128::from(a.1)),
        );
        ab.0.saturating_mul(ac.1)
            .saturating_sub(ab.1.saturating_mul(ac.0))
    }

    /// Whether orientations `left` and `right` are strictly opposite.
    fn opposite(left: i128, right: i128) -> bool {
        left.signum().saturating_mul(right.signum()) < 0
    }

    /// Whether the two endpoint pairs properly cross.
    fn properly_cross(a: Ends, b: Ends) -> bool {
        opposite(orient(a.0, a.1, b.0), orient(a.0, a.1, b.1))
            && opposite(orient(b.0, b.1, a.0), orient(b.0, b.1, a.1))
    }

    /// The dominant-axis span `(low, high)` of a segment.
    fn span(endpoints: Ends) -> (i64, i64) {
        let (start, finish) = endpoints;
        if start.0 == finish.0 {
            (start.1.min(finish.1), start.1.max(finish.1))
        } else {
            (start.0.min(finish.0), start.0.max(finish.0))
        }
    }

    /// Whether the two collinear segments share more than one point.
    fn overlaps(a: Ends, b: Ends) -> bool {
        if orient(a.0, a.1, b.0) != 0 || orient(a.0, a.1, b.1) != 0 {
            return false;
        }
        let (low_a, high_a) = span(a);
        let (low_b, high_b) = span(b);
        low_b.max(low_a) < high_a.min(high_b)
    }

    /// The error-severity issues `validate_map`'s own geometry checks
    /// report over `map`.
    fn geometry_errors(map: &Map) -> Vec<String> {
        let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
        let resources = crate::wad::map::validate::ResourceIndex::default();
        let context = Context {
            definitions: &definitions,
            hexen_specials: &definitions,
            resources: &resources,
        };
        let enabled: BTreeSet<&'static str> = [
            "vertex_out_of_range",
            "sidedef_out_of_range",
            "sector_out_of_range",
            "missing_front_side",
            "zero_length_line",
            "duplicate_line",
            "two_sided_flag_mismatch",
        ]
        .into_iter()
        .collect();
        run(map, &context, Some(&enabled))
            .issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .map(|issue| format!("{}: {}", issue.code, issue.message))
            .collect()
    }

    /// Check `first` against every later line.
    fn check_following(map: &Map, first: usize) {
        let one = ends(map, first);
        for second in first.saturating_add(1)..map.linedefs.len() {
            let two = ends(map, second);
            assert!(
                !properly_cross(one, two) && !overlaps(one, two),
                "lines {first} and {second} touch badly: {one:?} x {two:?}"
            );
        }
    }

    /// Invariant: the rebuilt map neither crosses nor overlaps itself
    /// and passes `validate_map`'s structural checks.
    fn assert_clean(map: &Map) {
        for first in 0..map.linedefs.len() {
            check_following(map, first);
        }
        let errors = geometry_errors(map);
        assert!(errors.is_empty(), "validate errors: {errors:?}");
    }

    /// The linedef running between coordinates `a` and `b`, either way
    /// round.
    fn line_between(map: &Map, a: (i64, i64), b: (i64, i64)) -> Option<usize> {
        (0..map.linedefs.len()).find(|index| {
            let (start, finish) = ends(map, *index);
            (start == a && finish == b) || (start == b && finish == a)
        })
    }

    /// The side of `line`'s named slot.
    fn side_of(map: &Map, line: usize, front: bool) -> crate::wad::map::Sidedef {
        let row = map.linedefs.get(line).expect("the linedef exists");
        let slot = if front { row.front } else { row.back };
        map.sidedefs
            .get(slot.expect("the side exists"))
            .expect("the sidedef exists")
            .clone()
    }

    #[test]
    fn ring_normalization_and_midpoints_follow_the_engine() {
        let closed = vec![(0, 0), (10, 0), (10, 10), (0, 0)];
        assert_eq!(normalize_ring(&closed).len(), 3, "the closing repeat drops");
        let doubled = vec![(0, 0), (0, 0), (10, 0), (10, 10)];
        assert_eq!(normalize_ring(&doubled).len(), 3, "adjacent repeats drop");
        assert_eq!(midpoint((0, 0), (10, 4)), (5, 2), "exact halves");
        assert_eq!(
            midpoint((-3, 0), (0, 3)),
            (-1, 1),
            "odd sums truncate to zero"
        );
    }

    #[test]
    fn a_floating_room_gets_four_one_sided_walls() {
        let mut map = empty_map();
        let result = apply_overlay(&mut map, &room_request(&rect(0, 0, 256, 256))).expect("draws");
        assert_eq!(result.sector, Some(0), "the appended sector");
        assert_eq!(result.linedefs_created.len(), 4, "four new walls");
        assert!(result.removed_linedefs.is_empty(), "nothing to remove");
        assert_eq!(
            result.vertices_created,
            vec![0_usize, 1, 2, 3],
            "four corners"
        );
        assert_eq!(result.sidedefs_created.len(), 4, "one front side per wall");
        for line in &map.linedefs {
            assert_eq!(line.flags, 1, "every new wall blocks");
            assert!(line.back.is_none(), "every wall is one-sided");
        }
        for side in &map.sidedefs {
            assert_eq!(side.sector, 0, "fronts face the new sector");
            assert_eq!(side.middle, "W1", "the front wears the wall texture");
            assert_eq!(side.upper, "-", "and nothing above or below");
        }
        assert_clean(&map);
    }

    #[test]
    fn a_room_attached_mid_wall_splits_the_wall_and_opens_the_middle() {
        let mut map = two_room_map();
        map.sidedefs.get_mut(1).expect("hall north side").x_offset = 10_i32;
        let result = apply_overlay(&mut map, &room_request(&rect(64, 256, 192, 384)))
            .expect("the nub room draws");
        assert_eq!(result.sector, Some(2), "a third sector");
        assert_clean(&map);
        let low = line_between(&map, (0, 256), (64, 256)).expect("wall piece before the nub");
        let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
        let high = line_between(&map, (192, 256), (256, 256)).expect("wall piece after");
        assert_eq!(map.linedefs.get(low).expect("line").flags, 1, "one-sided");
        assert_eq!(map.linedefs.get(high).expect("line").flags, 1, "one-sided");
        assert_eq!(
            map.linedefs.get(shared).expect("line").flags,
            4,
            "two-sided"
        );
        let hall_side = side_of(&map, shared, true);
        assert_eq!(hall_side.sector, 0, "still faces the hall");
        assert_eq!(hall_side.middle, "-", "the middle moved onto the faces");
        assert_eq!(hall_side.upper, "WALL1");
        assert_eq!(
            hall_side.x_offset, 74_i32,
            "10 plus the 64 units up to the nub"
        );
        let back = side_of(&map, shared, false);
        assert_eq!(back.sector, 2, "the new room");
        assert_eq!(back.middle, "-", "same heights, bare faces");
        assert_eq!(back.upper, "-");
        assert_eq!(back.lower, "-");
        assert!(result.removed_linedefs.is_empty(), "no line vanished");
        assert_eq!(
            result.linedef_map.get(1),
            Some(&Some(low)),
            "the old wall maps to its first piece"
        );
    }

    #[test]
    fn a_room_over_two_sectors_drops_the_painted_over_parts() {
        let mut map = two_room_map();
        let result = apply_overlay(&mut map, &room_request(&rect(128, 64, 384, 320)))
            .expect("the overlap draws");
        assert_eq!(result.sector, Some(2), "a third sector");
        assert_clean(&map);
        assert!(
            line_between(&map, (256, 64), (256, 256)).is_none(),
            "the step line's painted-over middle is gone"
        );
        assert!(
            line_between(&map, (256, 0), (256, 64)).is_some(),
            "and the step line's south part stays"
        );
        assert!(
            line_between(&map, (0, 256), (128, 256)).is_some(),
            "the hall north wall keeps its west part"
        );
        assert!(
            !result.notes.iter().any(|note| note.contains("sector 0")),
            "the hall still has a boundary: {:?}",
            result.notes
        );
        let sides = map.sidedefs.iter().filter(|side| side.sector == 2).count();
        assert!(sides >= 4, "the new room is bounded all around");
    }

    #[test]
    fn keep_interior_leaves_an_inner_room_alone_and_repaints_the_rest() {
        let mut map = two_room_map();
        apply_overlay(&mut map, &room_request(&rect(64, 64, 192, 192))).expect("the platform");
        let before = map.sectors.len();
        let mut request = room_request(&rect(32, 32, 224, 224));
        request.keep_interior = true;
        let result = apply_overlay(&mut map, &request).expect("the bigger room draws");
        assert_clean(&map);
        assert_eq!(result.sector, Some(before), "the new sector sits last");
        assert_eq!(
            result.empty_sectors,
            Vec::<usize>::new(),
            "the platform keeps its area: {:?}",
            result.notes
        );
        let platform = before.saturating_sub(1);
        let walls = map
            .sidedefs
            .iter()
            .filter(|side| side.sector == platform)
            .count();
        assert_eq!(walls, 4, "the platform's four sides stand");
        assert_eq!(
            point_in_sector(&map, 128, 128),
            Some(2),
            "the pillar centre still belongs to the platform"
        );
    }

    #[test]
    fn overwriting_the_interior_empties_the_buried_sector_and_notes_it() {
        let mut map = two_room_map();
        apply_overlay(&mut map, &room_request(&rect(64, 64, 192, 192))).expect("the platform");
        let result = apply_overlay(&mut map, &room_request(&rect(32, 32, 224, 224)))
            .expect("the bigger room buries it");
        assert_clean(&map);
        assert_eq!(result.empty_sectors, vec![2_usize], "the platform sector");
        assert!(
            result.notes.iter().any(|note| note.contains("sector 2")),
            "the note names it: {:?}",
            result.notes
        );
        assert_eq!(map.sectors.len(), 4, "the empty sector stays in the map");
        assert_eq!(point_in_sector(&map, 128, 128), Some(3), "now the big room");
    }

    #[test]
    fn carving_a_pillar_gives_the_room_one_sided_walls_in_its_own_texture() {
        let mut map = two_room_map();
        let result = apply_overlay(&mut map, &void_request(&rect(96, 96, 160, 160)))
            .expect("the pillar carves");
        assert_eq!(result.sector, None, "void paint appends no sector");
        assert_clean(&map);
        assert_eq!(result.linedefs_created.len(), 4, "four new walls");
        for index in &result.linedefs_created {
            let line = map.linedefs.get(*index).expect("a new wall");
            assert_eq!(line.flags, 1, "blocking, one-sided");
            assert!(line.back.is_none(), "no back side");
            let side = side_of(&map, *index, true);
            assert_eq!(side.sector, 0, "faces the hall");
            assert_eq!(side.middle, "WALL1", "the hall's own wall texture");
        }
        assert!(
            point_in_sector(&map, 128, 128).is_none(),
            "the pillar centre is solid now"
        );
        assert!(
            result.removed_linedefs.is_empty(),
            "nothing was painted away"
        );
    }

    #[test]
    fn carving_with_only_dash_walls_falls_back_to_the_request_texture() {
        let mut map = two_room_map();
        for row in &mut map.sidedefs {
            row.middle = "-".to_owned();
        }
        let result = apply_overlay(&mut map, &void_request(&rect(96, 96, 160, 160)))
            .expect("the pillar carves");
        let index = *result.linedefs_created.first().expect("four walls");
        let side = side_of(&map, index, true);
        assert_eq!(side.middle, "W1", "the request's wall texture stands in");
    }

    #[test]
    fn carving_across_two_rooms_separates_them_with_one_sided_walls() {
        let mut map = two_room_map();
        let result = apply_overlay(&mut map, &void_request(&rect(192, 64, 320, 192)))
            .expect("the trench carves");
        assert_clean(&map);
        assert!(
            line_between(&map, (256, 64), (256, 192)).is_none(),
            "the shared step line's carved middle is gone"
        );
        assert!(
            result.linedefs_created.iter().all(|index| {
                map.linedefs
                    .get(*index)
                    .is_some_and(|line| line.flags == 1 && line.back.is_none())
            }),
            "every new wall is one-sided"
        );
        let west = line_between(&map, (192, 64), (192, 192)).expect("the hall-facing wall");
        let side = side_of(&map, west, true);
        assert_eq!(side.sector, 0, "the hall walls its new edge");
        assert_eq!(side.middle, "WALL1", "with its own texture");
        assert!(result.empty_sectors.is_empty(), "both rooms survive");
    }

    #[test]
    fn split_walls_keep_their_texture_offsets() {
        let mut map = two_room_map();
        map.sidedefs.get_mut(1).expect("hall north side").x_offset = 32_i32;
        apply_overlay(&mut map, &room_request(&rect(64, 256, 192, 384))).expect("the nub");
        let high = line_between(&map, (192, 256), (256, 256)).expect("east wall piece");
        let side = side_of(&map, high, true);
        assert_eq!(side.x_offset, 224_i32, "32 plus the 192 units before it");
        let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
        assert_eq!(side_of(&map, shared, true).x_offset, 96_i32, "32 plus 64");
        let low = line_between(&map, (0, 256), (64, 256)).expect("west wall piece");
        assert_eq!(
            side_of(&map, low, true).x_offset,
            32_i32,
            "the anchor stands"
        );
    }

    #[test]
    fn reused_vertices_keep_their_indices_and_records() {
        let mut map = two_room_map();
        let result = apply_overlay(&mut map, &room_request(&rect(0, 256, 256, 512)))
            .expect("the room above the hall draws");
        assert_eq!(result.vertices_created, vec![6_usize, 7], "two new corners");
        assert_eq!(map.vertices.len(), 8, "six reused, two added");
        let shared = line_between(&map, (0, 256), (256, 256)).expect("the shared wall");
        let line = map.linedefs.get(shared).expect("line");
        assert_eq!(
            (line.v1, line.v2),
            (5_usize, 4_usize),
            "the reused vertices"
        );
        assert_clean(&map);
    }

    #[test]
    fn a_solid_outside_keeps_new_edges_one_sided_but_the_shared_wall_open() {
        let mut map = two_room_map();
        let mut request = room_request(&rect(64, 256, 192, 384));
        request.outside = Outside::Solid;
        apply_overlay(&mut map, &request).expect("the nub draws");
        assert_clean(&map);
        let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
        assert_eq!(
            map.linedefs.get(shared).expect("line").flags,
            4,
            "the existing wall still opens"
        );
        let west = line_between(&map, (64, 256), (64, 384)).expect("a new edge");
        let line = map.linedefs.get(west).expect("new line");
        assert_eq!(line.flags, 1, "the new edge stays a single line");
        assert!(line.back.is_none(), "no side into the void");
    }

    #[test]
    fn a_faced_outside_turns_new_edges_two_sided_against_the_named_sector() {
        let mut map = two_room_map();
        let mut request = room_request(&rect(1024, 0, 1280, 256));
        request.outside = Outside::Face(0);
        apply_overlay(&mut map, &request).expect("the far room draws against the hall");
        assert_clean(&map);
        let west = line_between(&map, (1024, 0), (1024, 256)).expect("a new edge");
        let line = map.linedefs.get(west).expect("new line");
        assert_eq!(line.flags, 4, "two-sided against the named sector");
        assert_eq!(side_of(&map, west, false).sector, 0, "the hall is faced");
        assert_eq!(side_of(&map, west, true).sector, 2, "and the new room");
    }

    #[test]
    fn invalid_polygons_and_unknown_outsiders_are_refused_untouched() {
        let mut map = two_room_map();
        let before = map.clone();
        let bow = vec![(0_i64, 0_i64), (256, 256), (256, 0), (0, 256)];
        let error = apply_overlay(&mut map, &room_request(&bow)).expect_err("crosses itself");
        assert_eq!(code(&error), "invalid_argument", "self-crossing code");
        let flat = rect(0, 0, 128, 0);
        assert_eq!(
            code(&apply_overlay(&mut map, &room_request(&flat)).expect_err("zero area refuses")),
            "invalid_argument",
            "flat code"
        );
        let mut request = room_request(&rect(0, 0, 64, 64));
        request.outside = Outside::Face(99);
        let missing = apply_overlay(&mut map, &request).expect_err("sector 99 is not there");
        assert_eq!(code(&missing), "not_found", "unknown face code");
        assert!(
            missing.to_string().contains("map has 2 sectors"),
            "the message counts the sectors: {missing}"
        );
        assert_eq!(map, before, "the refusals painted nothing");
    }

    #[test]
    fn a_void_paint_over_nothing_changes_nothing_but_the_rows_rebuild() {
        let mut map = empty_map();
        let result = apply_overlay(&mut map, &void_request(&rect(0, 0, 128, 128))).expect("carves");
        assert_eq!(result.sector, None, "no sector");
        assert!(map.linedefs.is_empty(), "void over void leaves no walls");
        assert!(map.vertices.is_empty(), "no piece, no vertex");
    }
}
