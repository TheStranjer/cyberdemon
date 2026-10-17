//! The pure room-drawing engine: [`create_sector_from_loop`] grows a closed
//! loop of corners into one new sector with its boundary walls, the
//! algorithm behind the `create_sector_from_loop` tool.
//!
//! Everything here is a function of the [`Map`] model and a [`LoopRequest`]:
//! no sessions, no MCP. The loop is resolved against the map first (exact
//! coordinate reuse of existing vertices, creation of the rest), checked
//! (distinct corners, non-zero signed area, no self-crossing, known outer
//! sector) and only then mutated in, so a refused request leaves the map
//! untouched, the way [`crate::wad::map::remove::apply_removal`] does.
//!
//! The orientation rule is `Doom`'s: the front side of a linedef is on the
//! right of `v1` to `v2`, so the loop must run clockwise (negative shoelace
//! area with `Y` up) for the new sector's fronts to face its interior; a
//! counter-clockwise ring is reversed. Existing linedefs along an edge are
//! reused: same direction, its front side takes the new sector; opposite
//! direction, a back side is created for it and the line opens into
//! two-sided (two-sided bit `4` set, blocking bit `1` cleared, the front's
//! `middle` moved onto its free `upper`/`lower` faces, the new back all
//! `"-"`). A brand-new edge gets the [`OuterSector`] treatment: `Auto`
//! probes two map units left of the edge midpoint (the outside) with
//! [`geometry::point_in_sector`] on the map as it stood *before* this
//! call; an explicit index faces every new edge at that sector; `OneSided`
//! (the `"none"` mode) keeps every new edge a blocking single line. All
//! arithmetic is integer: coordinates widen to `i64`, areas and
//! orientations compute in `i128`.

use alloc::collections::BTreeMap;

use serde::Serialize;

use super::geometry;
use super::model::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::error::CyberError;

/// The `blocking` linedef flag bit: the flags of a new one-sided wall.
const BLOCKING_BIT: u32 = 1_u32;
/// The `twosided` linedef flag bit, the same value in both flag families.
const TWO_SIDED_BIT: u32 = 4_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
const NO_TEXTURE: &str = "-";
/// The fewest corners a loop may have.
const MIN_CORNERS: usize = 3_usize;
/// Divisor of the two-endpoint sum that gives an edge's midpoint.
const MIDPOINT_DIVISOR: i64 = 2_i64;
/// How far left of an edge midpoint the automatic probe step stands.
const PROBE_STEP: i64 = 2_i64;

/// One loop corner given as coordinates, in map units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopPoint {
    /// `X` coordinate.
    pub x: i32,
    /// `Y` coordinate.
    pub y: i32,
}

/// The shape of the loop to draw: fresh corners or existing vertex indices.
#[derive(Clone, Debug)]
pub enum LoopGeometry {
    /// Corners as points: a vertex already sitting at exactly the same
    /// coordinates is reused, the rest are appended to [`Map::vertices`].
    /// A trailing point repeating the first is dropped; the loop is
    /// implicitly closed.
    Points(Vec<LoopPoint>),
    /// Indices into [`Map::vertices`], walked as the loop's corners.
    Vertices(Vec<usize>),
}

/// What the outer side of the loop's brand-new edges faces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OuterSector {
    /// Probe the map just left of each new edge and make the line
    /// two-sided against the sector found there; edges along the void stay
    /// one-sided. The [`Default`].
    #[default]
    Auto,
    /// Every new edge stays a blocking one-sided wall (the `"none"` mode).
    OneSided,
    /// Every new edge becomes two-sided against this existing sector.
    Sector(usize),
}

/// A whole `create_sector_from_loop` request: the loop, the sector record
/// to append and the textures and outer rule for its walls.
///
/// The tool layer validates the strings and numeric fields for the map's
/// format; this module only ever reads them.
#[derive(Clone, Debug)]
pub struct LoopRequest {
    /// The loop's corners, as points or existing vertex indices.
    pub geometry: LoopGeometry,
    /// The sector record appended for the new region.
    pub sector: Sector,
    /// `middle` texture of a brand-new one-sided wall's front side.
    pub wall: String,
    /// `upper` texture of a brand-new two-sided transition (both faces).
    pub upper: String,
    /// `lower` texture of a brand-new two-sided transition (both faces).
    pub lower: String,
    /// What the new walls' outer sides face.
    pub outer: OuterSector,
}

/// What [`create_sector_from_loop`] did, as the tool reports it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LoopResult {
    /// Index of the new sector in [`Map::sectors`].
    pub sector: usize,
    /// New vertex indices, in loop order.
    pub vertices_created: Vec<usize>,
    /// New linedef indices, in loop order.
    pub linedefs_created: Vec<usize>,
    /// Existing linedef indices the loop reused, in loop order.
    pub linedefs_reused: Vec<usize>,
    /// New sidedef indices, in creation order.
    pub sidedefs_created: Vec<usize>,
    /// Loop linedefs left two-sided by this call: the existing lines the
    /// new sector took over from their back side, and the new lines given
    /// an outer back side.
    pub two_sided: Vec<usize>,
}

/// One consecutive pair of a list, for the closing ring walk.
#[derive(Clone, Copy, Debug)]
struct EdgePair<T> {
    /// The pair's first item.
    from: T,
    /// The pair's second item.
    to: T,
}

/// The loop after corner resolution: vertex indices (provisional for the
/// to-be-created ones), their coordinates and the pending new vertices.
#[derive(Debug, Default)]
struct Ring {
    /// One vertex index per corner, in loop order.
    ids: Vec<usize>,
    /// The corners' coordinates, widened to `i64`, index-aligned with
    /// [`Ring::ids`].
    pts: Vec<(i64, i64)>,
    /// Corners with no existing vertex: appended to [`Map::vertices`]
    /// before any linedef is created, at indices past the map's current
    /// vertex count.
    pending: Vec<LoopPoint>,
}

/// The per-edge application context: the request, the map as it stood
/// before this call (for the automatic outer probe) and what has been
/// built so far.
struct Drawing<'req> {
    /// The request being applied.
    request: &'req LoopRequest,
    /// The map's geometry as it stood before this call.
    before: Map,
    /// Index of the new sector.
    sector: usize,
    /// The report being built.
    report: LoopResult,
}

/// Grows a closed corner loop into one new sector of `map`: appends the
/// sector and reuses or creates one linedef per edge, front sides facing
/// the loop's interior, the walls dressed per `request`.
///
/// Reports what was created and reused as a [`LoopResult`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the loop has fewer than three
/// corners, repeats a corner, has zero area, or crosses itself;
/// [`CyberError::NotFound`] for a vertex index or an explicit
/// [`OuterSector::Sector`] the map does not hold.
///
/// On error `map` is left untouched.
#[inline]
pub fn create_sector_from_loop(
    map: &mut Map,
    request: &LoopRequest,
) -> Result<LoopResult, CyberError> {
    refuse_unknown_outer(map, request)?;
    let ring = orient(resolve(map, request.geometry.clone())?)?;
    refuse_self_crossing(&ring.pts)?;
    let before = snapshot(map);
    let base = map.vertices.len();
    for point in &ring.pending {
        map.vertices.push(Vertex {
            x: point.x,
            y: point.y,
            extra: BTreeMap::new(),
        });
    }
    let sector = map.sectors.len();
    map.sectors.push(request.sector.clone());
    let mut drawing = Drawing {
        request,
        before,
        sector,
        report: LoopResult {
            vertices_created: (base..map.vertices.len()).collect(),
            ..LoopResult::default()
        },
    };
    for (from, to) in edge_indices(&ring.ids) {
        drawing.edge(map, from, to);
    }
    drawing.report.sector = sector;
    Ok(drawing.report)
}

/// The resolved corners of `geometry` against `map`: existing vertices
/// found by exact coordinates plus provisional indices for the pending
/// new ones.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for fewer than three corners or one
/// used twice; [`CyberError::NotFound`] for a vertex index the map does
/// not hold.
fn resolve(map: &Map, geometry: LoopGeometry) -> Result<Ring, CyberError> {
    match geometry {
        LoopGeometry::Points(points) => resolve_points(map, &points),
        LoopGeometry::Vertices(indices) => resolve_vertices(map, &indices),
    }
}

/// The corner list with a trailing repeat of the first point dropped.
fn trim_closure(mut points: Vec<LoopPoint>) -> Vec<LoopPoint> {
    if points.len() >= 2_usize && points.first() == points.last() {
        points.pop();
    }
    points
}

/// Resolves point corners: exact-coordinate vertex reuse, pending creation
/// otherwise, refusing repeats.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for fewer than three corners or one
/// used twice.
fn resolve_points(map: &Map, points: &[LoopPoint]) -> Result<Ring, CyberError> {
    let corners = trim_closure(points.to_vec());
    if corners.len() < MIN_CORNERS {
        return Err(CyberError::InvalidArgument(
            "the loop needs at least 3 distinct corners: pass points around the room's \
             perimeter, implicitly closed (do not repeat the first point)"
                .to_owned(),
        ));
    }
    let base = map.vertices.len();
    let mut ring = Ring::default();
    for point in &corners {
        let known = find_vertex(map, *point).or_else(|| {
            ring.pending
                .iter()
                .position(|pending| pending == point)
                .map(|offset| base.saturating_add(offset))
        });
        let index = if let Some(existing) = known {
            existing
        } else {
            let fresh = base.saturating_add(ring.pending.len());
            ring.pending.push(*point);
            fresh
        };
        ring.ids.push(index);
        ring.pts.push((i64::from(point.x), i64::from(point.y)));
    }
    refuse_repeats(&ring.ids)?;
    Ok(ring)
}

/// Resolves existing-vertex corners, bounds-checking every index.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for fewer than three corners or one
/// used twice; [`CyberError::NotFound`] for an index past [`Map::vertices`].
fn resolve_vertices(map: &Map, indices: &[usize]) -> Result<Ring, CyberError> {
    if indices.len() < MIN_CORNERS {
        return Err(CyberError::InvalidArgument(
            "the loop needs at least 3 distinct corner indices".to_owned(),
        ));
    }
    for index in indices {
        if *index >= map.vertices.len() {
            return Err(CyberError::NotFound(format!(
                "vertex {index} does not exist (map has {} vertices): pass existing \
                 indices, or draw with points",
                map.vertices.len()
            )));
        }
    }
    let pts = indices
        .iter()
        .filter_map(|index| map.vertices.get(*index))
        .map(|vertex| (i64::from(vertex.x), i64::from(vertex.y)))
        .collect();
    let ring = Ring {
        ids: indices.to_vec(),
        pts,
        pending: Vec::new(),
    };
    refuse_repeats(&ring.ids)?;
    Ok(ring)
}

/// The index of a vertex sitting at exactly `point`'s coordinates.
fn find_vertex(map: &Map, point: LoopPoint) -> Option<usize> {
    map.vertices
        .iter()
        .position(|vertex| vertex.x == point.x && vertex.y == point.y)
}

/// Refuses a corner list that uses one vertex index twice.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the repeated vertex.
fn refuse_repeats(ids: &[usize]) -> Result<(), CyberError> {
    for (position, index) in ids.iter().enumerate() {
        if ids.iter().take(position).any(|earlier| earlier == index) {
            return Err(CyberError::InvalidArgument(format!(
                "the loop uses vertex {index} twice: every corner must be distinct, a \
                 repeated point or closing repeat leaves no area between the edges"
            )));
        }
    }
    Ok(())
}

/// Reverses a counter-clockwise ring so it runs clockwise (`Doom`'s
/// front-on-the-right rule points the new fronts inwards); refuses a ring
/// with no area.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the signed area is zero.
fn orient(mut ring: Ring) -> Result<Ring, CyberError> {
    let area = signed_area_2(&ring.pts);
    if area == 0_i128 {
        return Err(CyberError::InvalidArgument(
            "the loop encloses no area: its corners are collinear or cancel out; draw a \
             real polygon instead"
                .to_owned(),
        ));
    }
    if area > 0_i128 {
        ring.ids.reverse();
        ring.pts.reverse();
    }
    Ok(ring)
}

/// The exact `i128` product of two `i64` coordinate factors (map
/// coordinates never come close to the `i128` range).
fn wide(a: i64, b: i64) -> i128 {
    i128::from(a)
        .checked_mul(i128::from(b))
        .unwrap_or(i128::MAX)
}

/// The `i64` difference of two widened coordinates.
fn delta(from: i64, to: i64) -> i64 {
    to.checked_sub(from).unwrap_or(i64::MAX)
}

/// The shoelace term `ax * by - bx * ay` of one edge, exact in `i128`.
fn area_term(from: (i64, i64), to: (i64, i64)) -> i128 {
    wide(from.0, to.1)
        .checked_sub(wide(to.0, from.1))
        .unwrap_or(i128::MAX)
}

/// Twice the signed area of the ring (positive counter-clockwise, `Y` up),
/// summed in `i128`.
fn signed_area_2(pts: &[(i64, i64)]) -> i128 {
    ring_edges(pts).into_iter().fold(0_i128, |sum, edge| {
        sum.saturating_add(area_term(edge.from, edge.to))
    })
}

/// The consecutive pairs of `ring`, closing back to the first item.
fn ring_edges<T: Copy>(ring: &[T]) -> Vec<EdgePair<T>> {
    let tail = ring.iter().skip(1).chain(ring.iter().take(1));
    ring.iter()
        .zip(tail)
        .map(|(from, to)| EdgePair {
            from: *from,
            to: *to,
        })
        .collect()
}

/// Whether the two edge indices share a corner of the ring: consecutive,
/// or the first and the last.
const fn adjacent(i: usize, j: usize, count: usize) -> bool {
    j == i.saturating_add(1) || (i == 0_usize && j == count.saturating_sub(1))
}

/// The sign of the cross product of the `a` to `b` and `a` to `c`
/// directions, exact in `i128`.
fn orient3(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i128 {
    let p_x = delta(a.0, b.0);
    let p_y = delta(a.1, b.1);
    let q_x = delta(a.0, c.0);
    let q_y = delta(a.1, c.1);
    wide(p_x, q_y).saturating_sub(wide(q_x, p_y))
}

/// Whether the two orientations are strictly on opposite sides (the
/// product of their signs is `-1`).
fn opposite(first: i128, second: i128) -> bool {
    first.signum().checked_mul(second.signum()) == Some(-1_i128)
}

/// Whether the two segments properly cross: each endpoint pair straddles
/// the other's line, no touching involved.
fn properly_cross(first: &EdgePair<(i64, i64)>, second: &EdgePair<(i64, i64)>) -> bool {
    opposite(
        orient3(first.from, first.to, second.from),
        orient3(first.from, first.to, second.to),
    ) && opposite(
        orient3(second.from, second.to, first.from),
        orient3(second.from, second.to, first.to),
    )
}

/// The first edge index whose edge properly crosses edge `i`, skipping the
/// ring-adjacent neighbours.
fn crossing_of(edges: &[EdgePair<(i64, i64)>], i: usize) -> Option<usize> {
    let first = edges.get(i)?;
    edges
        .iter()
        .enumerate()
        .skip(i.saturating_add(1))
        .find_map(|(j, second)| {
            (!adjacent(i, j, edges.len()) && properly_cross(first, second)).then_some(j)
        })
}

/// Refuses a ring two of whose non-adjacent edges properly cross.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the crossing edge pair.
fn refuse_self_crossing(pts: &[(i64, i64)]) -> Result<(), CyberError> {
    let edges = ring_edges(pts);
    for i in 0_usize..edges.len() {
        if let Some(j) = crossing_of(&edges, i) {
            return Err(CyberError::InvalidArgument(format!(
                "the loop crosses itself: edge {i} meets edge {j}; reorder the corners or \
                 split the shape into separate loops"
            )));
        }
    }
    Ok(())
}

/// Refuses an explicit outer sector the map does not hold.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the sector index.
fn refuse_unknown_outer(map: &Map, request: &LoopRequest) -> Result<(), CyberError> {
    if let OuterSector::Sector(index) = request.outer {
        if index >= map.sectors.len() {
            return Err(CyberError::NotFound(format!(
                "sector {index} does not exist (map has {} sectors): pass an existing \
                 sector index, \"auto\", or \"none\"",
                map.sectors.len()
            )));
        }
    }
    Ok(())
}

/// The geometry-only twin of `map`: what the outer probe consults, exactly
/// as the map stood before this call.
fn snapshot(map: &Map) -> Map {
    Map {
        name: map.name,
        format: map.format,
        udmf_namespace: map.udmf_namespace.clone(),
        things: Vec::new(),
        vertices: map.vertices.clone(),
        linedefs: map.linedefs.clone(),
        sidedefs: map.sidedefs.clone(),
        sectors: map.sectors.clone(),
        other: Vec::new(),
    }
}

/// The loop's consecutive vertex-index pairs, closing back to the first.
fn edge_indices(ids: &[usize]) -> Vec<(usize, usize)> {
    ring_edges(ids)
        .into_iter()
        .map(|pair| (pair.from, pair.to))
        .collect()
}

/// One [`Sidedef`] with the given face textures over `sector`.
fn face(upper: &str, lower: &str, middle: &str, sector: usize) -> Sidedef {
    Sidedef {
        upper: upper.to_owned(),
        lower: lower.to_owned(),
        middle: middle.to_owned(),
        sector,
        ..Default::default()
    }
}

/// Moves a side's middle texture onto its `upper` and `lower` where those
/// are free, then clears the middle: what keeps a wall's texture visible
/// when a one-sided line opens into a two-sided one.
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

/// The index of the linedef running exactly from vertex `a` to vertex `b`.
fn find_line(map: &Map, a: usize, b: usize) -> Option<usize> {
    map.linedefs
        .iter()
        .position(|line| line.v1 == a && line.v2 == b)
}

/// The probe point of edge `from` to `to`: the midpoint stepped two units
/// along the left normal's component signs, good enough for axis-aligned
/// and diagonal edges alike.
fn outside_point(from: &Vertex, to: &Vertex) -> Option<(i64, i64)> {
    let ax = i64::from(from.x);
    let ay = i64::from(from.y);
    let dx = delta(ax, i64::from(to.x));
    let dy = delta(ay, i64::from(to.y));
    let mid_x = ax
        .checked_add(i64::from(to.x))?
        .checked_div(MIDPOINT_DIVISOR)?;
    let mid_y = ay
        .checked_add(i64::from(to.y))?
        .checked_div(MIDPOINT_DIVISOR)?;
    let step_x = dy.signum().checked_neg()?.checked_mul(PROBE_STEP)?;
    let step_y = dx.signum().checked_mul(PROBE_STEP)?;
    Some((mid_x.checked_add(step_x)?, mid_y.checked_add(step_y)?))
}

impl Drawing<'_> {
    /// Applies the loop edge from vertex `a` to vertex `b`: reuse the
    /// existing line in either direction, else create one.
    fn edge(&mut self, map: &mut Map, a: usize, b: usize) {
        if let Some(index) = find_line(map, a, b) {
            self.take_front(map, index);
            self.report.linedefs_reused.push(index);
            return;
        }
        if let Some(index) = find_line(map, b, a) {
            self.take_back(map, index);
            self.report.linedefs_reused.push(index);
            return;
        }
        self.create_line(map, a, b);
    }

    /// Points an existing same-direction line's front side at the new
    /// sector, creating the side when the line has none.
    fn take_front(&mut self, map: &mut Map, index: usize) {
        let front = map.linedefs.get(index).and_then(|line| line.front);
        if let Some(side) = front.and_then(|id| map.sidedefs.get_mut(id)) {
            side.sector = self.sector;
            return;
        }
        let created = self.push_side(
            map,
            face(NO_TEXTURE, NO_TEXTURE, &self.request.wall, self.sector),
        );
        if let Some(line) = map.linedefs.get_mut(index) {
            line.front = Some(created);
        }
    }

    /// Appends a [`Sidedef`] and records its new index in the report.
    fn push_side(&mut self, map: &mut Map, side: Sidedef) -> usize {
        let index = map.sidedefs.len();
        map.sidedefs.push(side);
        self.report.sidedefs_created.push(index);
        index
    }

    /// Takes an existing opposite-direction line's back side for the new
    /// sector, creating it and opening the line when the side is missing.
    fn take_back(&mut self, map: &mut Map, index: usize) {
        if !self.retarget_back(map, index) {
            self.open_back(map, index);
        }
        self.report.two_sided.push(index);
    }

    /// Points the line's existing back sidedef at the new sector, whether
    /// it did (no line, no back reference or dangling side: it did not).
    fn retarget_back(&self, map: &mut Map, index: usize) -> bool {
        let Some(side) = map.linedefs.get(index).and_then(|line| line.back) else {
            return false;
        };
        map.sidedefs.get_mut(side).is_some_and(|sidedef| {
            sidedef.sector = self.sector;
            true
        })
    }

    /// Creates the back side of a line the loop runs against and makes the
    /// line two-sided, moving its front's middle texture onto free faces.
    fn open_back(&mut self, map: &mut Map, index: usize) {
        let front = map.linedefs.get(index).and_then(|line| line.front);
        let side = self.push_side(map, face(NO_TEXTURE, NO_TEXTURE, NO_TEXTURE, self.sector));
        if let Some(line) = map.linedefs.get_mut(index) {
            line.back = Some(side);
            line.flags = (line.flags | TWO_SIDED_BIT) & !BLOCKING_BIT;
        }
        if let Some(sidedef) = front.and_then(|id| map.sidedefs.get_mut(id)) {
            move_middle_to_faces(sidedef);
        }
    }

    /// Creates the loop's missing edge: a blocking one-sided wall, or a
    /// two-sided transition when an outer sector stands across it.
    fn create_line(&mut self, map: &mut Map, a: usize, b: usize) {
        let outer = self.outer_behind(map, a, b);
        let front = match outer {
            None => face(NO_TEXTURE, NO_TEXTURE, &self.request.wall, self.sector),
            Some(_) => face(
                &self.request.upper,
                &self.request.lower,
                NO_TEXTURE,
                self.sector,
            ),
        };
        let front_side = self.push_side(map, front);
        let mut line = Linedef {
            v1: a,
            v2: b,
            flags: BLOCKING_BIT,
            front: Some(front_side),
            ..Default::default()
        };
        let index = map.linedefs.len();
        if let Some(across) = outer {
            let back = face(&self.request.upper, &self.request.lower, NO_TEXTURE, across);
            line.back = Some(self.push_side(map, back));
            line.flags = TWO_SIDED_BIT;
            self.report.two_sided.push(index);
        }
        map.linedefs.push(line);
        self.report.linedefs_created.push(index);
    }

    /// The sector across a new edge `a` to `b`, if the request says there
    /// is one.
    fn outer_behind(&self, map: &Map, a: usize, b: usize) -> Option<usize> {
        match self.request.outer {
            OuterSector::OneSided => None,
            OuterSector::Sector(index) => Some(index),
            OuterSector::Auto => self.probe_outer(map, a, b),
        }
    }

    /// The automatic rule: the sector containing a point just left of the
    /// edge midpoint, on the pre-call snapshot.
    fn probe_outer(&self, map: &Map, a: usize, b: usize) -> Option<usize> {
        let (from, to) = (map.vertices.get(a)?, map.vertices.get(b)?);
        let (px, py) = outside_point(from, to)?;
        geometry::point_in_sector(&self.before, px, py)
    }
}

#[cfg(test)]
mod tests {
    use super::create_sector_from_loop;
    use super::{LoopGeometry, LoopPoint, LoopRequest, OuterSector};
    use crate::error::CyberError;
    use crate::wad::map::common::fixtures::{name, room, solid_line, two_room_map, vertex};
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::{Map, MapFormat};

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A loop corner at `x`/`y`.
    const fn point(x: i32, y: i32) -> LoopPoint {
        LoopPoint { x, y }
    }

    /// A point loop over `corners`.
    fn points(corners: &[LoopPoint]) -> LoopGeometry {
        LoopGeometry::Points(corners.to_vec())
    }

    /// The request drawing a plain sector over `geometry`, probing the
    /// outer side automatically.
    fn request(geometry: LoopGeometry) -> LoopRequest {
        LoopRequest {
            geometry,
            sector: room(0, 128, 160, 0),
            wall: "W1".to_owned(),
            upper: "UP".to_owned(),
            lower: "LO".to_owned(),
            outer: OuterSector::Auto,
        }
    }

    /// A request like `request` with the outer rule replaced.
    fn with_outer(mut req: LoopRequest, outer: OuterSector) -> LoopRequest {
        req.outer = outer;
        req
    }

    /// An empty `Doom` map to draw into.
    fn empty_map() -> Map {
        Map::new_empty(name("MAP01"), MapFormat::Doom)
    }

    #[test]
    fn a_counter_clockwise_square_is_reversed_to_face_inwards() {
        let mut map = empty_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 0),
                point(256, 0),
                point(256, 256),
                point(0, 256),
            ])),
        )
        .expect("the CCW square is drawn, reversed");
        assert_eq!(result.sector, 0, "the first sector");
        assert_eq!(
            result.vertices_created,
            vec![0_usize, 1, 2, 3],
            "all four corners are new"
        );
        assert_eq!(
            result.linedefs_created,
            vec![0_usize, 1, 2, 3],
            "all four walls are new"
        );
        assert!(result.linedefs_reused.is_empty(), "nothing was reused");
        assert!(
            result.two_sided.is_empty(),
            "the void outside stays one-sided"
        );
        let first = map.linedefs.first().expect("four linedefs");
        assert_eq!(
            (first.v1, first.v2),
            (3_usize, 2_usize),
            "the reversed ring starts (0,256) to (256,256)"
        );
        assert_eq!(
            point_in_sector(&map, 128, 128),
            Some(0),
            "the centre lies inside the new sector"
        );
        assert_eq!(
            point_in_sector(&map, -8, 128),
            None,
            "outside points in nothing"
        );
    }

    #[test]
    fn a_clockwise_square_in_an_empty_map_gets_four_one_sided_walls() {
        let mut map = empty_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 0),
                point(0, 256),
                point(256, 256),
                point(256, 0),
            ])),
        )
        .expect("a CW square draws");
        assert_eq!(result.sidedefs_created, vec![0_usize, 1, 2, 3]);
        for line in &map.linedefs {
            assert_eq!(line.flags, 1, "every new wall blocks");
            assert!(line.back.is_none(), "every wall is one-sided");
        }
        for side in &map.sidedefs {
            assert_eq!(side.sector, 0, "every front faces the new sector");
            assert_eq!(side.middle, "W1", "the front wears the wall texture");
            assert_eq!(side.upper, "-", "and nothing above or below");
            assert_eq!(side.lower, "-");
        }
    }

    #[test]
    fn an_adjacent_room_opens_the_shared_wall_and_moves_its_texture() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 256),
                point(0, 512),
                point(256, 512),
                point(256, 256),
            ])),
        )
        .expect("the room above room 0 draws");
        assert_eq!(result.sector, 2, "a third sector");
        assert_eq!(
            result.vertices_created,
            vec![6_usize, 7],
            "the two upper corners are new"
        );
        assert_eq!(
            result.linedefs_reused,
            vec![1_usize],
            "room 0's north wall is reused backwards"
        );
        assert_eq!(
            result.linedefs_created,
            vec![7_usize, 8, 9],
            "the other three walls are new"
        );
        assert_eq!(
            result.two_sided,
            vec![1_usize],
            "only the shared line opens"
        );
        let shared = map.linedefs.get(1).expect("linedef 1");
        assert_eq!(shared.flags, 4, "two-sided, no longer blocking");
        assert_eq!(shared.back, Some(11), "a new back side for the room");
        let front = map.sidedefs.get(1).expect("the hall's side");
        assert_eq!(front.middle, "-", "the middle moved off the now-open line");
        assert_eq!(front.upper, "WALL1", "onto both free faces");
        assert_eq!(front.lower, "WALL1");
        assert_eq!(front.sector, 0, "and it still faces the hall");
        let back = map.sidedefs.get(11).expect("the new back side");
        assert_eq!(back.sector, 2, "facing the new room");
        assert_eq!(
            (
                back.upper.as_str(),
                back.lower.as_str(),
                back.middle.as_str()
            ),
            ("-", "-", "-"),
            "bare on the new side"
        );
        assert_eq!(result.sidedefs_created, vec![8_usize, 9, 10, 11]);
    }

    #[test]
    fn a_platform_inside_a_room_turns_every_edge_two_sided_against_it() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(64, 64),
                point(64, 192),
                point(192, 192),
                point(192, 64),
            ])),
        )
        .expect("the platform draws");
        assert_eq!(result.sector, 2);
        assert!(result.linedefs_reused.is_empty(), "no wall was shared");
        assert_eq!(
            result.two_sided,
            vec![7_usize, 8, 9, 10],
            "every edge probes room 0 outside"
        );
        for index in 7_usize..11 {
            let line = map.linedefs.get(index).expect("a new wall");
            assert_eq!(line.flags, 4, "two-sided against the hall");
            let front = map.sidedefs.get(line.front.expect("front")).expect("side");
            let back = map.sidedefs.get(line.back.expect("back")).expect("side");
            assert_eq!(front.sector, 2, "the front faces the platform");
            assert_eq!(back.sector, 0, "the back faces the hall");
            assert_eq!(front.middle, "-", "the ledge shows its faces");
            assert_eq!(front.upper, "UP");
            assert_eq!(front.lower, "LO");
            assert_eq!(back.upper, "UP", "the same textures from both sides");
            assert_eq!(back.lower, "LO");
            assert_eq!(back.middle, "-");
        }
        assert_eq!(result.sidedefs_created.len(), 8, "two sides per edge");
    }

    #[test]
    fn the_one_sided_outer_rule_keeps_every_new_wall_a_single_line() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &with_outer(
                request(points(&[
                    point(64, 64),
                    point(64, 192),
                    point(192, 192),
                    point(192, 64),
                ])),
                OuterSector::OneSided,
            ),
        )
        .expect("the sealed platform draws");
        assert!(result.two_sided.is_empty(), "nothing opens");
        for line in map.linedefs.iter().skip(7) {
            assert_eq!(line.flags, 1, "blocking one-sided");
            assert!(line.back.is_none(), "no back side");
            let front = map.sidedefs.get(line.front.expect("front")).expect("side");
            assert_eq!(front.middle, "W1", "a plain wall");
        }
        assert_eq!(result.sidedefs_created, vec![8_usize, 9, 10, 11]);
    }

    #[test]
    fn an_explicit_outer_sector_faces_every_new_edge_at_it() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &with_outer(
                request(points(&[
                    point(64, 64),
                    point(64, 192),
                    point(192, 192),
                    point(192, 64),
                ])),
                OuterSector::Sector(1),
            ),
        )
        .expect("the platform with an explicit outer draws");
        assert_eq!(
            result.two_sided,
            vec![7_usize, 8, 9, 10],
            "every new line opens"
        );
        for index in 7_usize..11 {
            let back = map.linedefs.get(index).expect("line").back.expect("back");
            assert_eq!(map.sidedefs.get(back).expect("side").sector, 1, "room 1");
        }
    }

    #[test]
    fn existing_vertices_can_be_the_corners() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(LoopGeometry::Vertices(vec![1_usize, 2, 3, 4])),
        )
        .expect("room 1's own ring redraws over its lines");
        assert_eq!(result.sector, 2, "a new sector records them");
        assert!(result.vertices_created.is_empty(), "no new vertex");
        assert!(result.linedefs_created.is_empty(), "no new linedef");
        assert!(result.sidedefs_created.is_empty(), "no new sidedef");
        assert_eq!(
            result.linedefs_reused,
            vec![4_usize, 5, 6, 2],
            "reversed CW order walks room 1's four walls"
        );
        assert_eq!(
            result.two_sided,
            vec![2_usize],
            "the step line's back is taken over"
        );
        assert_eq!(map.sidedefs.get(3).expect("step back").sector, 2);
        assert_eq!(map.sidedefs.get(5).expect("east side").sector, 2);
        assert_eq!(
            point_in_sector(&map, 384, 128),
            Some(2),
            "room 1's ground now belongs to the new sector"
        );
    }

    #[test]
    fn a_sideless_existing_line_gains_its_sides_from_the_loop() {
        let mut map = empty_map();
        map.vertices = vec![vertex(0, 0), vertex(256, 0), vertex(128, 256)];
        map.linedefs = vec![solid_line(2, 1, 0), solid_line(0, 1, 0)];
        for line in &mut map.linedefs {
            line.front = None;
        }
        let result = create_sector_from_loop(
            &mut map,
            &request(LoopGeometry::Vertices(vec![0_usize, 2, 1])),
        )
        .expect("the triangle reuses both sideless lines");
        assert_eq!(result.linedefs_reused, vec![0_usize, 1]);
        assert_eq!(result.linedefs_created, vec![2_usize], "the third edge");
        let front = map
            .linedefs
            .first()
            .expect("line")
            .front
            .expect("new front");
        assert_eq!(map.sidedefs.get(front).expect("side").sector, 0);
        assert_eq!(map.sidedefs.get(front).expect("side").middle, "W1");
        let opened = map.linedefs.get(1).expect("line");
        assert_eq!(opened.flags, 4, "opened two-sided for its back");
        assert!(opened.front.is_none(), "the missing front stays missing");
        let back = opened.back.expect("new back");
        assert_eq!(map.sidedefs.get(back).expect("side").sector, 0);
    }

    #[test]
    fn a_trailing_repeat_of_the_first_corner_is_dropped() {
        let mut map = empty_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 0),
                point(0, 256),
                point(256, 256),
                point(0, 0),
            ])),
        )
        .expect("the closing repeat is trimmed to a triangle");
        assert_eq!(
            result.vertices_created,
            vec![0_usize, 1, 2],
            "three corners, not four"
        );
        assert_eq!(map.linedefs.len(), 3, "three walls");
    }

    #[test]
    fn a_self_crossing_bow_tie_is_refused() {
        let mut map = empty_map();
        let error = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 0),
                point(256, 0),
                point(64, 192),
                point(256, 256),
            ])),
        )
        .expect_err("crossing edges refuse");
        assert_eq!(code(&error), "invalid_argument", "crossing code");
        assert!(
            error.to_string().contains("crosses itself"),
            "the message names the problem: {error}"
        );
        assert!(
            map.linedefs.is_empty() && map.sectors.is_empty(),
            "map untouched"
        );
    }

    #[test]
    fn degenerate_loops_are_refused() {
        let mut map = empty_map();
        let collinear = create_sector_from_loop(
            &mut map,
            &request(points(&[point(0, 0), point(128, 0), point(256, 0)])),
        )
        .expect_err("a flat loop has no area");
        assert_eq!(code(&collinear), "invalid_argument", "area code");
        assert!(
            collinear.to_string().contains("no area"),
            "the message names the problem: {collinear}"
        );
        let repeated = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(0, 0),
                point(0, 0),
                point(256, 0),
                point(0, 256),
            ])),
        )
        .expect_err("a doubled corner refuses");
        assert_eq!(code(&repeated), "invalid_argument", "repeat code");
        assert!(
            repeated.to_string().contains("twice"),
            "the message names the problem: {repeated}"
        );
        let tiny =
            create_sector_from_loop(&mut map, &request(points(&[point(0, 0), point(128, 128)])))
                .expect_err("two corners draw nothing");
        assert_eq!(code(&tiny), "invalid_argument", "size code");
        let mut rooms = two_room_map();
        let dup_index = create_sector_from_loop(
            &mut rooms,
            &request(LoopGeometry::Vertices(vec![0_usize, 1, 0])),
        )
        .expect_err("a repeated vertex index refuses");
        assert_eq!(code(&dup_index), "invalid_argument", "repeat-index code");
        assert!(map.sectors.is_empty(), "none of the refusals drew");
        assert_eq!(rooms.sectors.len(), 2, "nor into the two-room map");
    }

    #[test]
    fn unknown_reference_indices_are_refused() {
        let mut map = two_room_map();
        let past = create_sector_from_loop(
            &mut map,
            &request(LoopGeometry::Vertices(vec![0_usize, 1, 99])),
        )
        .expect_err("vertex 99 is not there");
        assert_eq!(code(&past), "not_found", "vertex code");
        assert!(
            past.to_string().contains("map has 6 vertices"),
            "the message counts the vertices: {past}"
        );
        let outer = create_sector_from_loop(
            &mut map,
            &with_outer(
                request(points(&[
                    point(64, 64),
                    point(64, 192),
                    point(192, 192),
                    point(192, 64),
                ])),
                OuterSector::Sector(9),
            ),
        )
        .expect_err("sector 9 is not there");
        assert_eq!(code(&outer), "not_found", "outer code");
        assert!(
            outer.to_string().contains("map has 2 sectors"),
            "the message counts the sectors: {outer}"
        );
        assert_eq!(map.sectors.len(), 2, "the refusals drew nothing");
    }

    #[test]
    fn a_loop_sharing_only_vertices_welds_to_the_existing_ones() {
        let mut map = two_room_map();
        let result = create_sector_from_loop(
            &mut map,
            &request(points(&[
                point(256, 256),
                point(256, 512),
                point(512, 512),
                point(512, 256),
            ])),
        )
        .expect("the room above room 1 draws");
        assert_eq!(
            result.vertices_created,
            vec![6_usize, 7],
            "the two corners on room 1's top wall reused vertices 4 and 3"
        );
        assert_eq!(
            result.linedefs_reused,
            vec![4_usize],
            "room 1's north wall runs the other way under the new bottom edge"
        );
        assert_eq!(result.linedefs_created, vec![7_usize, 8, 9]);
        assert_eq!(
            result.two_sided,
            vec![4_usize],
            "the shared wall opens, the void edges stay one-sided"
        );
        assert_eq!(result.sidedefs_created, vec![8_usize, 9, 10, 11]);
        let shared = map.linedefs.get(4).expect("the north wall");
        assert_eq!(shared.flags, 4, "two-sided, blocking cleared");
        let back = shared.back.expect("a back side onto the new room");
        assert_eq!(map.sidedefs.get(back).expect("side").sector, 2);
        let front = map.sidedefs.get(5).expect("room 1's side");
        assert_eq!(
            (front.upper.as_str(), front.lower.as_str()),
            ("WALL1", "WALL1")
        );
        assert_eq!(front.middle, "-");
        assert_eq!(front.sector, 1, "and it still faces room 1");
    }
}
