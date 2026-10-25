//! The shared overlay engine: one polygon painted onto an existing map.
//!
//! [`apply_overlay`] is the drawing step behind `create_sector_from_loop`
//! and `carve_void`: it folds a [`crate::geom::Layer`] polygon over the
//! map's current linedefs (fed to [`arrange`](fn@crate::geom::arrange) as
//! [`crate::geom::segment::ExternalSegment`]s), so
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
//!
//! This file keeps the shapes a call is phrased in, the private reads of
//! the original map and [`apply_overlay`] itself. The labeler the
//! arrangement runs under, with the containment shapes it answers from,
//! is the private `labeler` module; the snapshot of one overlay's inputs
//! and every step that reads the original map is `prepared`; the rows
//! under construction and the dressing they carry is `rebuilt`; and the
//! integer geometry all of those measure with is `geometry`.

mod geometry;
mod labeler;
mod prepared;
mod rebuilt;

#[cfg(test)]
mod tests;

use serde::Serialize;

use super::model::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::error::CyberError;
use crate::geom::Pt;

use prepared::Prepared;

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
