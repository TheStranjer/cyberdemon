//! Shared builders for the overlay engine's tests: the error-code reader,
//! the request makers, the empty fixture map, the rectangle ring, the
//! linedef endpoint and side readers and the cross-and-overlap cleanliness
//! invariant every drawing is checked against.
//!
//! `rooms` paints rooms over the fixture map - the floating room, the walls
//! it splits and opens, `keep_interior` and the emptied sectors; `carve`
//! cuts solid rock out of it and reads the walls, offsets, outside rules
//! and refusals that come back.

mod carve;
mod rooms;

use alloc::collections::BTreeSet;

use crate::defs::definitions_for;
use crate::error::CyberError;
use crate::wad::game::Game;
use crate::wad::map::Sector;
use crate::wad::map::common::fixtures::{name, room};
use crate::wad::map::validate::{Context, Severity, run};
use crate::wad::map::{Map, MapFormat};

use super::{Outside, OverlayRequest, Paint, WallTextures};

/// Both endpoint coordinates of a linedef.
type Ends = ((i64, i64), (i64, i64));

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
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
pub(super) fn room_request(polygon: &[(i64, i64)]) -> OverlayRequest {
    paint_sector(polygon, room(0, 128, 160, 0))
}

/// A request carving `polygon` into solid rock.
pub(super) fn void_request(polygon: &[(i64, i64)]) -> OverlayRequest {
    OverlayRequest {
        polygon: polygon.to_vec(),
        paint: Paint::Void,
        keep_interior: false,
        textures: textures(),
        outside: Outside::Original,
    }
}

/// An empty `Doom` map to draw into.
pub(super) fn empty_map() -> Map {
    Map::new_empty(name("MAP01"), MapFormat::Doom)
}

/// A rectangle ring; either winding arranges the same.
pub(super) fn rect(low_x: i64, low_y: i64, high_x: i64, high_y: i64) -> Vec<(i64, i64)> {
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
pub(super) fn assert_clean(map: &Map) {
    for first in 0..map.linedefs.len() {
        check_following(map, first);
    }
    let errors = geometry_errors(map);
    assert!(errors.is_empty(), "validate errors: {errors:?}");
}

/// The linedef running between coordinates `a` and `b`, either way
/// round.
pub(super) fn line_between(map: &Map, a: (i64, i64), b: (i64, i64)) -> Option<usize> {
    (0..map.linedefs.len()).find(|index| {
        let (start, finish) = ends(map, *index);
        (start == a && finish == b) || (start == b && finish == a)
    })
}

/// The side of `line`'s named slot.
pub(super) fn side_of(map: &Map, line: usize, front: bool) -> crate::wad::map::Sidedef {
    let row = map.linedefs.get(line).expect("the linedef exists");
    let slot = if front { row.front } else { row.back };
    map.sidedefs
        .get(slot.expect("the side exists"))
        .expect("the sidedef exists")
        .clone()
}
