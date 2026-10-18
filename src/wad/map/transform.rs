//! Pure selection transform: the integer scale/rotate/translate math and
//! the whole-map step behind `transform_selection`.
//!
//! Everything here is a function of the [`Map`] model and plain integers:
//! no sessions, no MCP. Positions move through
//! `p' = origin + rotate(scale(p - origin)) + translate`, applied in that
//! order: the scale is the exact fraction `num / den` folded onto the
//! origin-relative offset with [`mul_div_round`], the rotation is
//! [`rotate_point`] (counter-clockwise, `BAM` angles), and every stage
//! rounds to nearest, so the result is a whole map unit. Thing facings take
//! the rotation as whole degrees normalised into `0..=359`. [`expand_selection`]
//! turns a request over any mix of vertices, linedefs, sectors and things
//! into the vertex and thing sets the transform moves, using
//! [`crate::wad::map::geometry`].
//!
//! The safety rule mirrors [`super::ops`]: a refused call leaves the map
//! untouched. Every resulting coordinate is checked against what the map's
//! format can store (`i16` for the binary formats) and every touched
//! linedef is checked against collapsing to zero length before a single
//! row is written. Vertices that land on an unselected vertex are allowed
//! and merely reported as coincident; nothing is merged.

use alloc::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::{Angle, mul_div_round, rotate_point};
use crate::wad::classify::MapFormat;

use super::geometry::{point_in_sector, sector_boundary};
use super::model::{Linedef, Map};

/// Lowest coordinate an `i16` binary vertex column holds.
const COORD_MIN: i32 = -0x8000_i32;
/// Highest coordinate an `i16` binary vertex column holds.
const COORD_MAX: i32 = 0x7FFF_i32;
/// Whole degrees in one turn, for thing-facing normalisation.
const DEGREES: i64 = 360;
/// Millidegrees in one degree.
const MILLIDEGREES: i64 = 1_000;
/// Numerator of the halving that turns a two-endpoint sum into a midpoint.
const HALF_NUM: i64 = 1;
/// Denominator of that halving.
const HALF_DEN: i64 = 2;

/// A point in map units, widened to `i64` so no transform intermediate
/// can overflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Point {
    /// `X` coordinate in map units.
    pub x: i64,
    /// `Y` coordinate in map units.
    pub y: i64,
}

impl Point {
    /// The point at a model element's `i32` coordinates.
    #[must_use]
    #[inline]
    pub fn from_model(x: i32, y: i32) -> Self {
        Self {
            x: i64::from(x),
            y: i64::from(y),
        }
    }
}

/// An exact positive rational scale factor `num / den`
/// (e.g. `3 / 2` grows to 150%, `1 / 2` shrinks to 50%).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// Numerator; above zero.
    pub num: i64,
    /// Denominator; above zero.
    pub den: i64,
}

impl Scale {
    /// Builds `num / den`, refusing either part at or below zero.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a part is not positive.
    #[inline]
    pub fn new(num: i32, den: i32) -> Result<Self, CyberError> {
        if num <= 0_i32 || den <= 0_i32 {
            return Err(CyberError::InvalidArgument(format!(
                "scale {num}/{den} is not a positive factor: pass a numerator and a \
                 denominator both above 0 (3/2 grows to 150%, 1/2 shrinks to 50%)"
            )));
        }
        Ok(Self {
            num: i64::from(num),
            den: i64::from(den),
        })
    }
}

/// A counter-clockwise rotation: exact in `BAM` for positions, whole
/// degrees for thing facings, which the formats store as integers.
#[derive(Clone, Copy, Debug)]
pub struct Rotation {
    /// The `BAM` angle positions are turned by.
    pub angle: Angle,
    /// Whole degrees added to the selected things' `angle`.
    pub degrees: i64,
}

impl Rotation {
    /// A rotation of whole `degrees` (any integer, counter-clockwise).
    #[must_use]
    #[inline]
    pub fn from_degrees(degrees: i64) -> Self {
        Self {
            angle: Angle::from_degrees(degrees),
            degrees,
        }
    }

    /// A rotation of `millidegrees` thousandths of a degree; thing facings
    /// take the nearest whole degree.
    #[must_use]
    #[inline]
    pub fn from_millidegrees(millidegrees: i64) -> Self {
        Self {
            angle: Angle::from_millidegrees(millidegrees),
            degrees: mul_div_round(millidegrees, HALF_NUM, MILLIDEGREES).unwrap_or(millidegrees),
        }
    }
}

/// The operations of one call, applied in the order scale, rotate,
/// translate.
#[derive(Clone, Copy, Debug, Default)]
pub struct Operations {
    /// Exact-fraction scaling about the origin, applied first.
    pub scale: Option<Scale>,
    /// Counter-clockwise rotation about the origin, applied second.
    pub rotation: Option<Rotation>,
    /// Whole-unit shift, applied last.
    pub translate: Option<Point>,
}

impl Operations {
    /// Whether no operation is set (a call must carry at least one).
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.scale.is_none() && self.rotation.is_none() && self.translate.is_none()
    }

    /// `origin + rotate(scale(p - origin)) + translate`, each stage rounded
    /// to nearest. `None` when any intermediate or the result leaves `i64`.
    fn apply_point(&self, point: Point, origin: Point) -> Option<Point> {
        let mut dx = point.x.checked_sub(origin.x)?;
        let mut dy = point.y.checked_sub(origin.y)?;
        if let Some(scale) = self.scale {
            dx = mul_div_round(dx, scale.num, scale.den)?;
            dy = mul_div_round(dy, scale.num, scale.den)?;
        }
        if let Some(rotation) = self.rotation {
            (dx, dy) = rotate_point(dx, dy, rotation.angle);
        }
        if let Some(delta) = self.translate {
            dx = dx.checked_add(delta.x)?;
            dy = dy.checked_add(delta.y)?;
        }
        Some(Point {
            x: origin.x.checked_add(dx)?,
            y: origin.y.checked_add(dy)?,
        })
    }
}

/// The caller's selection request, before it is expanded against a map.
#[derive(Clone, Debug, Default)]
pub struct SelectionRequest {
    /// Vertex indices to move.
    pub vertices: Vec<usize>,
    /// Linedef indices: both their endpoints join the selection.
    pub linedefs: Vec<usize>,
    /// Sector indices: their whole boundary joins the selection, and their
    /// things may too.
    pub sectors: Vec<usize>,
    /// Thing indices to move.
    pub things: Vec<usize>,
    /// Whether every vertex and thing of the map is selected.
    pub all: bool,
    /// Whether things standing in a selected sector join the selection.
    pub include_things_in_sectors: bool,
}

/// An expanded selection: exactly which vertices and things move.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Indices into [`Map::vertices`].
    pub vertices: BTreeSet<usize>,
    /// Indices into [`Map::things`].
    pub things: BTreeSet<usize>,
}

/// What one successful [`apply_transform`] did.
#[derive(Clone, Debug, Serialize)]
pub struct TransformReport {
    /// The point positions were scaled and rotated about.
    pub origin: Point,
    /// The expanded selection's vertex indices, ascending.
    pub vertices_selected: Vec<usize>,
    /// The expanded selection's thing indices, ascending.
    pub things_selected: Vec<usize>,
    /// How many vertices were written at new coordinates.
    pub vertices_moved: usize,
    /// How many things were written at new coordinates.
    pub things_moved: usize,
    /// `[moved, unmoved]` index pairs that now share one position.
    pub coincident_vertices: Vec<[usize; 2]>,
}

/// One vertex's planned destination, already checked to fit the format.
#[derive(Clone, Copy, Debug)]
struct Move {
    /// Index into [`Map::vertices`].
    index: usize,
    /// The stored `X` to write.
    x: i32,
    /// The stored `Y` to write.
    y: i32,
}

impl Move {
    /// The planned position as a [`Point`].
    fn point(self) -> Point {
        Point::from_model(self.x, self.y)
    }
}

/// A thing's planned destination and facing.
#[derive(Clone, Copy, Debug)]
struct ThingMove {
    /// The position to write.
    position: Move,
    /// The facing after a rotation; `None` keeps the current angle.
    angle: Option<i32>,
}

/// Expand `request` against `map` into the vertex and thing indices to move.
///
/// The rules: `all` selects every vertex and thing; otherwise the named
/// vertices and things are the seed, every named linedef adds its two
/// endpoints, every named sector adds the endpoints of all its boundary
/// linedefs ([`sector_boundary`]) and, when `include_things_in_sectors`,
/// the things whose [`point_in_sector`] is one of them (judged on the
/// untouched map). References that dangle simply do not contribute.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a requested index the map does not hold,
/// naming the kind and index; [`CyberError::InvalidArgument`] when the
/// expansion holds no vertex and no thing.
#[inline]
pub fn expand_selection(map: &Map, request: &SelectionRequest) -> Result<Selection, CyberError> {
    check_requested(map, request)?;
    if request.all {
        return refuse_empty(Selection {
            vertices: (0..map.vertices.len()).collect(),
            things: (0..map.things.len()).collect(),
        });
    }
    let mut vertices: BTreeSet<usize> = request.vertices.iter().copied().collect();
    for index in &request.linedefs {
        add_endpoints(map, map.linedefs.get(*index), &mut vertices);
    }
    let mut things: BTreeSet<usize> = request.things.iter().copied().collect();
    if !request.sectors.is_empty() {
        add_sector_boundaries(map, &request.sectors, &mut vertices);
        if request.include_things_in_sectors {
            add_sector_things(map, &request.sectors, &mut things);
        }
    }
    refuse_empty(Selection { vertices, things })
}

/// Refuse unless every index `request` names exists in its block.
fn check_requested(map: &Map, request: &SelectionRequest) -> Result<(), CyberError> {
    check_kind("vertex", "vertices", &request.vertices, map.vertices.len())?;
    check_kind("linedef", "linedefs", &request.linedefs, map.linedefs.len())?;
    check_kind("sector", "sectors", &request.sectors, map.sectors.len())?;
    check_kind("thing", "things", &request.things, map.things.len())
}

/// Refuse the first index of `kind` at or past `count`, phrased the way the
/// shared validator phrases it (`"vertex 9 does not exist (map has 6
/// vertices)"`). `plural` is spelled per kind because `"vertex"` takes no
/// plain `s`.
fn check_kind(kind: &str, plural: &str, indices: &[usize], count: usize) -> Result<(), CyberError> {
    for index in indices {
        if *index >= count {
            return Err(CyberError::NotFound(format!(
                "{kind} {index} does not exist (map has {count} {plural})"
            )));
        }
    }
    Ok(())
}

/// Add a resolvable linedef's two endpoints, skipping references that
/// dangle past the vertex block.
fn add_endpoints(map: &Map, picked: Option<&Linedef>, out: &mut BTreeSet<usize>) {
    let Some(line) = picked else {
        return;
    };
    for vertex in [line.v1, line.v2] {
        if vertex < map.vertices.len() {
            out.insert(vertex);
        }
    }
}

/// Add the endpoints of every boundary linedef of every sector in
/// `sectors`.
fn add_sector_boundaries(map: &Map, sectors: &[usize], out: &mut BTreeSet<usize>) {
    for sector in sectors {
        for (linedef, _) in sector_boundary(map, *sector) {
            add_endpoints(map, map.linedefs.get(linedef), out);
        }
    }
}

/// Add every thing standing in one of `sectors`, judged on the map as it
/// now is (before any transform).
fn add_sector_things(map: &Map, sectors: &[usize], out: &mut BTreeSet<usize>) {
    for (index, object) in map.things.iter().enumerate() {
        let containing = point_in_sector(map, i64::from(object.x), i64::from(object.y));
        if sectors.iter().any(|sector| Some(*sector) == containing) {
            out.insert(index);
        }
    }
}

/// Pass a non-empty selection through; refuse one that holds nothing.
fn refuse_empty(selection: Selection) -> Result<Selection, CyberError> {
    if selection.vertices.is_empty() && selection.things.is_empty() {
        return Err(CyberError::InvalidArgument(
            "nothing selected: pass at least one of vertices, linedefs, sectors, things \
             or all=true (the expanded selection must hold a vertex or a thing)"
                .to_owned(),
        ));
    }
    Ok(selection)
}

/// Scale, rotate and translate the selected vertices and things of `map`
/// about `origin`, in place, and report what moved.
///
/// `origin` defaults to the centre (rounded to nearest) of the selection's
/// bounding box. The refusal rule: a coordinate the map's format cannot
/// store fails `invalid_argument` and a linedef that becomes zero-length
/// fails `conflict` naming it, both before any row is written, so a failed
/// call leaves `map` untouched. Vertices landing on unselected vertices are
/// allowed and listed in the report's `coincident_vertices`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty selection, a coordinate
/// that overflows or leaves the format's range; [`CyberError::Conflict`]
/// for a linedef collapsing to zero length.
#[inline]
pub fn apply_transform(
    map: &mut Map,
    selection: &Selection,
    ops: &Operations,
    pivot: Option<Point>,
) -> Result<TransformReport, CyberError> {
    if selection.vertices.is_empty() && selection.things.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the selection holds no vertex and no thing: expand a non-empty request first"
                .to_owned(),
        ));
    }
    let origin = pivot.unwrap_or_else(|| default_origin(map, selection));
    let vertices = move_vertices(map, selection, ops, origin)?;
    let moved = moved_points(&vertices);
    let things = move_things(map, selection, ops, origin)?;
    check_collapse(map, &moved)?;
    let coincident_vertices = coincident_pairs(map, &moved);
    write_moves(map, &vertices, &things);
    Ok(TransformReport {
        origin,
        vertices_selected: vertices.iter().map(|vertex| vertex.index).collect(),
        things_selected: things.iter().map(|item| item.position.index).collect(),
        vertices_moved: vertices.len(),
        things_moved: things.len(),
        coincident_vertices,
    })
}

/// The planned vertex destinations keyed by index, for the checks that
/// compare positions after the move.
fn moved_points(vertices: &[Move]) -> BTreeMap<usize, Point> {
    vertices
        .iter()
        .map(|vertex| (vertex.index, vertex.point()))
        .collect()
}

/// The centre of the selected vertices' and things' bounding box, each
/// axis rounded to nearest. A degenerate span is its own midpoint, and a
/// selection whose references all dangle pivots at the zero point.
fn default_origin(map: &Map, selection: &Selection) -> Point {
    let mut span: Option<(i64, i64, i64, i64)> = None;
    for index in &selection.vertices {
        if let Some(vertex) = map.vertices.get(*index) {
            span = Some(widen(span, Point::from_model(vertex.x, vertex.y)));
        }
    }
    for index in &selection.things {
        if let Some(object) = map.things.get(*index) {
            span = Some(widen(span, Point::from_model(object.x, object.y)));
        }
    }
    span.map_or(Point { x: 0, y: 0 }, centre)
}

/// The span covering `span` and one more point; a fresh span when empty.
fn widen(span: Option<(i64, i64, i64, i64)>, at: Point) -> (i64, i64, i64, i64) {
    let (min_x, min_y, max_x, max_y) = span.unwrap_or((at.x, at.y, at.x, at.y));
    (
        min_x.min(at.x),
        min_y.min(at.y),
        max_x.max(at.x),
        max_y.max(at.y),
    )
}

/// The midpoint of a `(min_x, min_y, max_x, max_y)` span.
fn centre((min_x, min_y, max_x, max_y): (i64, i64, i64, i64)) -> Point {
    Point {
        x: mid(min_x, max_x),
        y: mid(min_y, max_y),
    }
}

/// Midpoint of `low` and `high`, rounded to nearest (halves away from
/// zero).
fn mid(low: i64, high: i64) -> i64 {
    let sum = low.checked_add(high).unwrap_or(low);
    mul_div_round(sum, HALF_NUM, HALF_DEN).unwrap_or(low)
}

/// The planned moves of every selected vertex.
fn move_vertices(
    map: &Map,
    selection: &Selection,
    ops: &Operations,
    origin: Point,
) -> Result<Vec<Move>, CyberError> {
    let mut moves = Vec::with_capacity(selection.vertices.len());
    for index in &selection.vertices {
        let vertex = map
            .vertices
            .get(*index)
            .ok_or_else(|| missing("vertex", *index))?;
        let from = Point::from_model(vertex.x, vertex.y);
        let to = ops
            .apply_point(from, origin)
            .ok_or_else(|| overflow("vertex", *index))?;
        let (x, y) = fit(&format!("vertex {index}"), to, map.format)?;
        moves.push(Move {
            index: *index,
            x,
            y,
        });
    }
    Ok(moves)
}

/// The planned moves of every selected thing, with its new facing.
fn move_things(
    map: &Map,
    selection: &Selection,
    ops: &Operations,
    origin: Point,
) -> Result<Vec<ThingMove>, CyberError> {
    let mut moves = Vec::with_capacity(selection.things.len());
    for index in &selection.things {
        let object = map
            .things
            .get(*index)
            .ok_or_else(|| missing("thing", *index))?;
        let from = Point::from_model(object.x, object.y);
        let to = ops
            .apply_point(from, origin)
            .ok_or_else(|| overflow("thing", *index))?;
        let (x, y) = fit(&format!("thing {index}"), to, map.format)?;
        let angle = ops
            .rotation
            .map(|rotation| rotated_angle(object.angle, rotation.degrees));
        moves.push(ThingMove {
            position: Move {
                index: *index,
                x,
                y,
            },
            angle,
        });
    }
    Ok(moves)
}

/// The thing facing after a rotation: the current angle plus the
/// rotation's whole degrees, normalised into `0..=359`.
fn rotated_angle(current: i32, degrees: i64) -> i32 {
    let turned = i64::from(current)
        .checked_add(degrees)
        .unwrap_or_else(|| i64::from(current))
        .rem_euclid(DEGREES);
    i32::try_from(turned).unwrap_or(current)
}

/// Refuse a transformed point the map's format cannot store: the model's
/// `i32` always, and the binary formats' `i16` columns on top. The `Ok`
/// carries the pair as the model stores it.
fn fit(label: &str, at: Point, format: MapFormat) -> Result<(i32, i32), CyberError> {
    let Point { x, y } = at;
    let (Ok(stored_x), Ok(stored_y)) = (i32::try_from(x), i32::try_from(y)) else {
        return Err(CyberError::InvalidArgument(format!(
            "{label} would land at ({x}, {y}), past the range this model can hold: \
             lower `scale` or `translate`, or move `origin`"
        )));
    };
    let fits =
        (COORD_MIN..=COORD_MAX).contains(&stored_x) && (COORD_MIN..=COORD_MAX).contains(&stored_y);
    if format != MapFormat::Udmf && !fits {
        let name = format_name(format);
        return Err(CyberError::InvalidArgument(format!(
            "{label} would land at ({stored_x}, {stored_y}), outside the i16 range a {name} \
             map stores: lower `scale` or `translate`, move `origin`, or run \
             convert_map_format to UDMF first"
        )));
    }
    Ok((stored_x, stored_y))
}

/// The format's name as map tools spell it.
const fn format_name(format: MapFormat) -> &'static str {
    match format {
        MapFormat::Doom => "Doom",
        MapFormat::Hexen => "Hexen",
        MapFormat::Udmf => "UDMF",
    }
}

/// Refuse any linedef whose moved endpoints collapse onto each other,
/// naming it. A line already zero-length stays zero-length without a
/// refusal; lines with a dangling, unmoved endpoint cannot be checked and
/// stand.
fn check_collapse(map: &Map, moved: &BTreeMap<usize, Point>) -> Result<(), CyberError> {
    for (index, line) in map.linedefs.iter().enumerate() {
        if !moved.contains_key(&line.v1) && !moved.contains_key(&line.v2) {
            continue;
        }
        let (Some(from1), Some(from2)) = (map.vertices.get(line.v1), map.vertices.get(line.v2))
        else {
            continue;
        };
        let before = Point::from_model(from1.x, from1.y) != Point::from_model(from2.x, from2.y);
        let to1 = moved
            .get(&line.v1)
            .copied()
            .unwrap_or_else(|| Point::from_model(from1.x, from1.y));
        let to2 = moved
            .get(&line.v2)
            .copied()
            .unwrap_or_else(|| Point::from_model(from2.x, from2.y));
        if before && to1 == to2 {
            let Point { x, y } = to1;
            return Err(CyberError::Conflict(format!(
                "linedef {index} would become zero-length at ({x}, {y}): lower `scale` \
                 or `translate`, or leave one of its vertices out of the selection"
            )));
        }
    }
    Ok(())
}

/// Every `[moved, unmoved]` vertex pair that shares a position after the
/// moves; coincidences are reported, never merged.
fn coincident_pairs(map: &Map, moved: &BTreeMap<usize, Point>) -> Vec<[usize; 2]> {
    let mut unmoved: BTreeMap<Point, Vec<usize>> = BTreeMap::new();
    for (index, vertex) in map.vertices.iter().enumerate() {
        if !moved.contains_key(&index) {
            unmoved
                .entry(Point::from_model(vertex.x, vertex.y))
                .or_default()
                .push(index);
        }
    }
    let mut pairs: Vec<[usize; 2]> = moved
        .iter()
        .filter_map(|(index, at)| unmoved.get(at).map(|others| (*index, others)))
        .flat_map(|(index, others)| others.iter().map(move |other| [index, *other]))
        .collect();
    pairs.sort_unstable();
    pairs
}

/// Commit the planned moves to the map's rows (every check has passed).
fn write_moves(map: &mut Map, vertices: &[Move], things: &[ThingMove]) {
    for vertex in vertices {
        if let Some(row) = map.vertices.get_mut(vertex.index) {
            row.x = vertex.x;
            row.y = vertex.y;
        }
    }
    for item in things {
        if let Some(row) = map.things.get_mut(item.position.index) {
            row.x = item.position.x;
            row.y = item.position.y;
            row.angle = item.angle.unwrap_or(row.angle);
        }
    }
}

/// The refusal for a selection index past its block (the expansion has
/// already checked; a hand-built selection is the caller's own mismatch).
fn missing(kind: &str, index: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "the selection names {kind} {index}, which this map does not hold: \
         re-expand the selection against the current map"
    ))
}

/// The refusal when a transform intermediate or result leaves `i64`.
fn overflow(kind: &str, index: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{kind} {index} would leave the computable range: lower `scale` or \
         `translate`, or move `origin` closer"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        Operations, Point, Rotation, Scale, Selection, SelectionRequest, TransformReport,
        apply_transform, check_collapse, coincident_pairs, default_origin, expand_selection,
        rotated_angle,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::Map;
    use crate::wad::map::common::fixtures::{spawn, two_room_map};
    use alloc::collections::{BTreeMap, BTreeSet};

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A translate-only operation set.
    fn shifted(dx: i64, dy: i64) -> Operations {
        Operations {
            translate: Some(Point { x: dx, y: dy }),
            ..Default::default()
        }
    }

    /// A rotation-only operation set of whole `degrees`.
    fn turned(degrees: i64) -> Operations {
        Operations {
            rotation: Some(Rotation::from_degrees(degrees)),
            ..Default::default()
        }
    }

    /// A scale-only operation set of `num / den`.
    fn grown(num: i32, den: i32) -> Operations {
        Operations {
            scale: Some(Scale::new(num, den).expect("test fractions are positive")),
            ..Default::default()
        }
    }

    /// The point `(x, y)`.
    const fn point(x: i64, y: i64) -> Point {
        Point { x, y }
    }

    /// The zero origin.
    const ZERO: Point = point(0, 0);

    /// A selection of exactly these vertices and things.
    fn selection(vertices: &[usize], things: &[usize]) -> Selection {
        Selection {
            vertices: vertices.iter().copied().collect(),
            things: things.iter().copied().collect(),
        }
    }

    /// The coordinates of `map`'s vertex `index`.
    fn vertex_at(map: &Map, index: usize) -> (i32, i32) {
        let vertex = map.vertices.get(index).expect("test vertices exist");
        (vertex.x, vertex.y)
    }

    /// Transform `map`'s selection with `ops` about the default origin.
    fn transform(
        map: &mut Map,
        picked: &Selection,
        ops: &Operations,
    ) -> Result<TransformReport, CyberError> {
        apply_transform(map, picked, ops, None)
    }

    #[test]
    fn translate_shifts_points_and_ignores_the_origin() {
        let ops = shifted(30, -40);
        assert_eq!(
            ops.apply_point(point(10, 20), ZERO),
            Some(point(40, -20)),
            "plain shift"
        );
        assert_eq!(
            ops.apply_point(point(10, 20), point(7, 9)),
            Some(point(40, -20)),
            "a translation never needs a pivot: the origin cancels exactly"
        );
        assert_eq!(
            shifted(0, 0).apply_point(point(5, 5), point(-3, -3)),
            Some(point(5, 5)),
            "the zero shift keeps the point"
        );
    }

    #[test]
    fn rotate_turns_points_counter_clockwise() {
        assert_eq!(
            turned(90).apply_point(point(100, 0), ZERO),
            Some(point(0, 100)),
            "a quarter turn about the origin"
        );
        assert_eq!(
            turned(45).apply_point(point(100, 0), ZERO),
            Some(point(71, 71)),
            "45 degrees rounds to nearest"
        );
        assert_eq!(
            turned(360).apply_point(point(123, 45), ZERO),
            Some(point(123, 45)),
            "a full turn is the identity"
        );
        assert_eq!(
            turned(90).apply_point(point(110, 100), point(10, 100)),
            Some(point(10, 200)),
            "rotation is about the origin, not the zero point"
        );
    }

    #[test]
    fn scale_multiplies_the_origin_relative_offset() {
        let origin = point(10, 10);
        assert_eq!(
            grown(2, 1).apply_point(point(20, 30), origin),
            Some(point(30, 50)),
            "2/1 doubles the offset"
        );
        assert_eq!(
            grown(1, 2).apply_point(point(20, 30), origin),
            Some(point(15, 20)),
            "1/2 halves it"
        );
        assert_eq!(
            grown(1, 2).apply_point(point(21, 30), origin),
            Some(point(16, 20)),
            "an odd half offset rounds away from zero"
        );
        assert_eq!(
            grown(3, 2).apply_point(point(10, 10), origin),
            Some(point(10, 10)),
            "the origin itself never moves"
        );
    }

    #[test]
    fn operations_apply_scale_then_rotate_then_translate() {
        let ops = Operations {
            scale: Some(Scale::new(2, 1).expect("2/1 is positive")),
            rotation: Some(Rotation::from_degrees(90)),
            translate: Some(point(10, 0)),
        };
        assert_eq!(
            ops.apply_point(point(2, 0), ZERO),
            Some(point(10, 4)),
            "(2,0) scales to (4,0), turns to (0,4), shifts to (10,4)"
        );
        assert!(!ops.is_empty(), "a set of three operations is not empty");
        assert!(Operations::default().is_empty(), "the empty set refuses");
    }

    #[test]
    fn a_thing_angle_wraps_past_a_full_turn() {
        assert_eq!(rotated_angle(350, 20), 10_i32, "350 + 20 wraps to 10");
        assert_eq!(rotated_angle(180, -180), 0_i32, "a half turn back");
        assert_eq!(rotated_angle(0, -1), 359_i32, "one degree south of east");
        let mut map = Map {
            things: vec![spawn(1, 100, 100, 350)],
            ..Default::default()
        };
        let report =
            transform(&mut map, &selection(&[], &[0]), &turned(20)).expect("the thing turns");
        assert_eq!(map.things[0].angle, 10_i32, "the stored facing wraps");
        assert_eq!(
            (map.things[0].x, map.things[0].y),
            (100_i32, 100_i32),
            "a thing at the default origin keeps its position"
        );
        assert_eq!(report.things_moved, 1, "one thing moved");
    }

    #[test]
    fn the_default_origin_is_the_selection_bbox_centre() {
        let map = two_room_map();
        assert_eq!(
            default_origin(&map, &selection(&[1, 2, 3, 4], &[1])),
            point(384, 128),
            "x 256..512 and y 0..256 centre on (384, 128)"
        );
        let report = transform(&mut map.clone(), &selection(&[2, 3], &[]), &grown(1, 1))
            .expect("a 1/1 scale just reports");
        assert_eq!(report.origin, point(512, 128), "the right wall's centre");
        assert_eq!(
            default_origin(&map, &selection(&[0], &[])),
            point(0, 0),
            "a single point spans nothing and centres on itself"
        );
    }

    #[test]
    fn sector_selection_grabs_its_boundary_and_its_things() {
        let map = two_room_map();
        let back_room = expand_selection(
            &map,
            &SelectionRequest {
                sectors: vec![1],
                include_things_in_sectors: true,
                ..Default::default()
            },
        )
        .expect("sector 1 is there");
        assert_eq!(
            back_room.vertices,
            BTreeSet::from([1_usize, 2, 3, 4]),
            "every corner of the back room, the step line's too"
        );
        assert_eq!(
            back_room.things,
            BTreeSet::from([1_usize]),
            "the imp stands in it"
        );
        let walls_only = expand_selection(
            &map,
            &SelectionRequest {
                sectors: vec![1],
                ..Default::default()
            },
        )
        .expect("the same sector without things");
        assert!(walls_only.things.is_empty(), "the switch reaches off");
        let line = expand_selection(
            &map,
            &SelectionRequest {
                linedefs: vec![2],
                ..Default::default()
            },
        )
        .expect("the step line");
        assert_eq!(
            line.vertices,
            BTreeSet::from([1_usize, 4]),
            "a linedef drags its two endpoints"
        );
        assert!(line.things.is_empty(), "linedefs never drag things");
        let everything = expand_selection(
            &map,
            &SelectionRequest {
                all: true,
                ..Default::default()
            },
        )
        .expect("all");
        assert_eq!(everything.vertices.len(), 6, "every vertex");
        assert_eq!(everything.things.len(), 2, "every thing");
    }

    #[test]
    fn bad_indices_and_empty_expansions_are_refused() {
        let map = two_room_map();
        let cases = [
            (
                SelectionRequest {
                    vertices: vec![9],
                    ..Default::default()
                },
                "vertex 9 does not exist (map has 6 vertices)",
            ),
            (
                SelectionRequest {
                    linedefs: vec![7],
                    ..Default::default()
                },
                "linedef 7 does not exist (map has 7 linedefs)",
            ),
            (
                SelectionRequest {
                    sectors: vec![2],
                    ..Default::default()
                },
                "sector 2 does not exist (map has 2 sectors)",
            ),
            (
                SelectionRequest {
                    things: vec![2],
                    ..Default::default()
                },
                "thing 2 does not exist (map has 2 things)",
            ),
        ];
        for (picked, phrasing) in cases {
            let error =
                expand_selection(&map, &picked).expect_err("the named index is past the block");
            assert_eq!(code(&error), "not_found", "out of range code");
            assert_eq!(error.to_string(), phrasing, "the documented phrasing");
        }
        let empty =
            expand_selection(&map, &SelectionRequest::default()).expect_err("nothing is selected");
        assert_eq!(code(&empty), "invalid_argument", "empty expansion code");
        let nothing_here = Map::default();
        let bare = expand_selection(
            &nothing_here,
            &SelectionRequest {
                all: true,
                ..Default::default()
            },
        )
        .expect_err("an empty map selects nothing even with all");
        assert_eq!(code(&bare), "invalid_argument", "still empty");
    }

    #[test]
    fn the_engine_moves_a_whole_selection_and_reports_it() {
        let mut map = two_room_map();
        let report = transform(&mut map, &selection(&[1, 2, 3, 4], &[1]), &shifted(0, 64))
            .expect("the back room slides up");
        assert_eq!(report.origin, point(384, 128), "the selection centre");
        assert_eq!(report.vertices_moved, 4, "four corners");
        assert_eq!(report.things_moved, 1, "the imp");
        assert_eq!(report.vertices_selected, vec![1, 2, 3, 4], "ascending");
        assert_eq!(report.things_selected, vec![1], "the moved thing");
        assert!(report.coincident_vertices.is_empty(), "nothing meets");
        assert_eq!(
            vertex_at(&map, 1),
            (256_i32, 64_i32),
            "the shared corner moved"
        );
        assert_eq!(
            vertex_at(&map, 3),
            (512_i32, 320_i32),
            "the top corner moved"
        );
        assert_eq!(vertex_at(&map, 0), (0_i32, 0_i32), "the hall corner stayed");
        assert_eq!(
            (map.things[1].x, map.things[1].y),
            (384_i32, 192_i32),
            "the imp rode along"
        );
        assert_eq!(map.things[0].x, 64_i32, "the player stands still");
    }

    #[test]
    fn a_half_turn_maps_the_fixture_onto_itself() {
        let mut map = two_room_map();
        let all = selection(&[0, 1, 2, 3, 4, 5], &[0, 1]);
        let report = apply_transform(&mut map, &all, &turned(180), Some(point(256, 128)))
            .expect("the whole map turns");
        assert_eq!(report.vertices_moved, 6, "everything moved");
        assert_eq!(
            vertex_at(&map, 0),
            (512_i32, 256_i32),
            "the hall corner is the far one"
        );
        assert_eq!(
            (map.things[0].x, map.things[0].angle),
            (448_i32, 180_i32),
            "the player moved and turned to face the new way"
        );
        let back = apply_transform(&mut map, &all, &turned(-180), Some(point(256, 128)))
            .expect("and back again");
        assert!(back.coincident_vertices.is_empty(), "nothing stacked");
        assert_eq!(map, two_room_map(), "a half turn twice is the identity");
    }

    #[test]
    fn coordinates_the_format_cannot_store_refuse_intact() {
        let before = two_room_map();
        let mut doom = two_room_map();
        let error = transform(&mut doom, &selection(&[0, 2], &[]), &grown(200, 1))
            .expect_err("x -50944 leaves the i16 range");
        assert_eq!(code(&error), "invalid_argument", "out of range code");
        let message = error.to_string();
        assert!(
            message.contains("i16") && message.contains("convert_map_format"),
            "the error names the limit and the fix: {message}"
        );
        assert_eq!(doom, before, "nothing moved");
        let mut udmf = two_room_map();
        udmf.format = MapFormat::Udmf;
        let report = transform(&mut udmf, &selection(&[0, 2], &[]), &grown(200, 1))
            .expect("UDMF stores 32-bit coordinates");
        assert_eq!(
            report.vertices_moved, 2,
            "the bottom wall grew out both ways"
        );
        assert_eq!(
            vertex_at(&udmf, 0),
            (-50_944_i32, 0_i32),
            "far left, on the floor line"
        );
        let mut runaway = two_room_map();
        let huge = transform(
            &mut runaway,
            &selection(&[0, 2], &[]),
            &grown(0x4000_0000, 1),
        )
        .expect_err("past even the model's i32");
        assert_eq!(code(&huge), "invalid_argument", "model overflow code");
        assert_eq!(runaway, before, "still nothing moved");
    }

    #[test]
    fn a_collapsing_scale_is_a_conflict_naming_the_line() {
        let before = two_room_map();
        let mut map = two_room_map();
        let error = transform(
            &mut map,
            &selection(&[0, 1, 2, 3, 4, 5], &[0, 1]),
            &grown(1, 1_000),
        )
        .expect_err("1/1000 stacks the map onto its centre");
        assert_eq!(code(&error), "conflict", "a zero-length line conflicts");
        let message = error.to_string();
        assert!(
            message.contains("linedef") && message.contains("zero-length"),
            "the error names the line: {message}"
        );
        assert_eq!(map, before, "the map stands");
    }

    #[test]
    fn vertices_stacking_on_unmoved_vertices_are_reported() {
        let mut map = two_room_map();
        let report = transform(&mut map, &selection(&[5], &[]), &shifted(256, -256))
            .expect("vertex 5 may land on vertex 1");
        assert_eq!(
            report.coincident_vertices,
            vec![[5_usize, 1_usize]],
            "the pair is [moved, unmoved]"
        );
        assert_eq!(map.vertices.len(), 6, "nothing merged");
        assert_eq!(
            vertex_at(&map, 5),
            (256_i32, 0_i32),
            "both sit at the corner"
        );
    }

    #[test]
    fn a_hand_built_selection_past_the_blocks_is_refused() {
        let mut map = two_room_map();
        let error =
            transform(&mut map, &selection(&[99], &[]), &shifted(1, 0)).expect_err("no vertex 99");
        assert_eq!(
            code(&error),
            "invalid_argument",
            "hand-built selections refuse"
        );
        let empty = transform(&mut map, &selection(&[], &[]), &shifted(1, 0))
            .expect_err("the engine never moves nothing");
        assert_eq!(code(&empty), "invalid_argument", "empty selections refuse");
    }

    #[test]
    fn scale_refuses_non_positive_fractions() {
        let zero = Scale::new(0, 2).expect_err("zero grows nothing");
        assert_eq!(code(&zero), "invalid_argument", "zero numerator code");
        assert!(Scale::new(-1, 2).is_err(), "a negative factor refuses");
        assert!(Scale::new(2, -1).is_err(), "a negative denominator refuses");
        assert_eq!(
            Scale::new(3, 2).expect("3/2 is a factor"),
            Scale { num: 3, den: 2 },
            "the parts widen to i64"
        );
    }

    #[test]
    fn millidegree_rotations_round_thing_facings() {
        let exact = Rotation::from_millidegrees(45_000);
        assert_eq!(exact.degrees, 45, "45.000 degrees");
        assert_eq!(exact.angle, Rotation::from_degrees(45).angle, "same angle");
        let rounded = Rotation::from_millidegrees(45_500);
        assert_eq!(rounded.degrees, 46, "45.5 rounds away from zero");
        assert_eq!(
            rotated_angle(10, rounded.degrees),
            56_i32,
            "facings take the rounded degree"
        );
    }

    #[test]
    fn dangling_endpoints_neither_collapse_nor_hide_a_pair() {
        let mut map = two_room_map();
        map.linedefs
            .last_mut()
            .expect("the fixture has linedefs")
            .v2 = 999;
        let moved: BTreeMap<usize, Point> = BTreeMap::from([(2_usize, point(512, 64))]);
        check_collapse(&map, &moved).expect("a dangling endpoint cannot collide");
        let pairs = coincident_pairs(
            &map,
            &BTreeMap::from([(0_usize, point(512, 0)), (1_usize, point(256, 0))]),
        );
        assert_eq!(
            pairs,
            vec![[0_usize, 2_usize]],
            "vertex 5's corner is the only unmoved meeting point"
        );
    }
}
