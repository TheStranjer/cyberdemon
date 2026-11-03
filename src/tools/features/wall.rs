//! The wall choice and void-strip machinery `add_door` and `add_window`
//! share: resolving a [`crate::tools::features::door::Target`] against
//! the map into a [`crate::tools::features::door::Chosen`] wall
//! ([`choose_wall`], [`choose_between`], [`choose_linedef`]), carving the
//! 16-unit void strip that pulls touching rooms apart ([`carve_strip`]
//! through [`void_request`], re-finding the room's face with
//! [`strip_wall`]), fitting the rectangle to paint ([`recess_rect`] over
//! [`crate::wad::map::features::door_wall`] and
//! [`crate::wad::map::features::door_rect`], with the [`Side`] a one-sided
//! wall is dug toward and the [`backside_depth`] ray that measures it) and
//! the check that a drawn recess really connects
//! ([`require_connected`]), beside the integer point maths ([`half_axis`],
//! [`outward_normal`]) all of it runs through.

use crate::error::CyberError;
use crate::fixed::{midpoint, narrow_i32};
use crate::geom::Pt;
use crate::geom::segment::Delta;
use crate::named::{Named, parse_named};
use crate::wad::map::features::{
    DoorWall, backside_depth, door_rect, door_wall, endpoints, longest_boundary, nearest_boundary,
    outward_depth, project_onto_line, void_strip,
};
use crate::wad::map::geometry::linedef_length;
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, Paint, WallTextures};
use crate::wad::map::{Linedef, Map};

use crate::tools::maps::params::ElementKind;
use crate::wad::map::query::{check_index, missing_index};

use super::door::{Chosen, DoorSpec, MAX_DEPTH, Placed, Target};
use super::{no_room, wall_lost};

/// The void strip `add_door` and `add_window` carve between touching
/// rooms, in map units.
pub(super) const STRIP_THICKNESS: i32 = 16_i32;

/// Which side of a one-sided wall the recess is dug toward, as the shared
/// `side` parameter of `add_door` and `add_window`.
///
/// A one-sided wall has a sector on its front side only, so the two sides
/// are the two readings of "the space this door opens into": [`Side::Front`]
/// digs into the room the wall already fronts (a bay across its space, the
/// wall sealed as a jamb), [`Side::Back`] digs through the wall into the
/// space behind it, which is what puts the door in the named wall itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Side {
    /// Toward the side the front sidedef faces: the room's own space. The
    /// default, and the behaviour of a call that leaves `side` out.
    #[default]
    Front,
    /// Through the wall, away from its room.
    Back,
}

impl Named for Side {
    const ALL: &'static [Self] = &[Self::Front, Self::Back];

    #[inline]
    fn name(self) -> &'static str {
        self.as_str()
    }
}

impl Side {
    /// The canonical wire name of this side.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }

    /// The side a caller has to pass to dig the other way.
    const fn other(self) -> Self {
        match self {
            Self::Front => Self::Back,
            Self::Back => Self::Front,
        }
    }
}

/// The `side` parameter as a [`Side`]: `None` when the call left it out,
/// the refusal listing both names when it spells neither.
pub(super) fn parse_side(given: Option<&str>) -> Result<Option<Side>, CyberError> {
    given.map_or(Ok(None), |text| parse_named("side", text).map(Some))
}

/// The `v1` to `v2` vector of linedef `index` as an exact `i128` delta.
fn wall_run(map: &Map, index: usize) -> Option<Delta> {
    let (from, to) = endpoints(map, index)?;
    Some((
        i128::from(to.x).saturating_sub(i128::from(from.x)),
        i128::from(to.y).saturating_sub(i128::from(from.y)),
    ))
}

/// The front-side outward perpendicular `(dy, -dx)` of linedef `index`:
/// the [`outward_depth`] ray direction, pointing at the line's front.
pub(super) fn outward_normal(map: &Map, index: usize) -> Option<Point> {
    let run = wall_run(map, index)?;
    Some(Point {
        x: narrow_i32(run.1),
        y: narrow_i32(run.0.saturating_neg()),
    })
}

/// The mean of two coordinates, halves rounded away from zero and
/// saturated back into `i32`.
pub(super) fn half_axis(first: i32, second: i32) -> i32 {
    narrow_i32(i128::from(midpoint(i64::from(first), i64::from(second))))
}

/// The squared distance from `from` to the midpoint of linedef `index`,
/// `None` when its vertices do not resolve.
fn mid_distance_sq(map: &Map, index: usize, from: Point) -> Option<i128> {
    let (a, b) = endpoints(map, index)?;
    let dx = i128::from(half_axis(a.x, b.x)).saturating_sub(i128::from(from.x));
    let dy = i128::from(half_axis(a.y, b.y)).saturating_sub(i128::from(from.y));
    Some(dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)))
}

/// Whether the line's front faces `room` and nothing sits behind it: a
/// one-sided wall of that room.
fn room_face(map: &Map, line: &Linedef, room: usize) -> bool {
    let (front, back) = map.line_sectors(line);
    front == Some(room) && back.is_none()
}

/// The door wall for `between` sectors: their shared boundary's nearest
/// line to `at` (else its longest), with both sector indices validated.
pub(super) fn choose_between(
    map: &Map,
    a: usize,
    b: usize,
    at: Option<Point>,
) -> Result<Chosen, CyberError> {
    check_index(ElementKind::Sectors, a, map.sectors.len())?;
    check_index(ElementKind::Sectors, b, map.sectors.len())?;
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        CyberError::invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary: draw the rooms touching (or \
             cut one room's one-sided wall and pass side: \"back\" to dig the recess \
             through it); get_map_summary lists each sector's neighbours"
        ))
    })?;
    let line = at
        .map_or_else(
            || longest_line(map, &boundary),
            |point| nearest_boundary(map, a, b, point),
        )
        .ok_or_else(|| {
            CyberError::invalid(&format!(
                "no line of the shared boundary of sectors {a} and {b} has resolvable \
                 vertices: repair the dangling vertex references first"
            ))
        })?;
    Ok(Chosen {
        room_a: a,
        room_b: b,
        wall: line,
        centre: project_onto_line(map, line, at),
        touching: true,
    })
}

/// The longest line of `boundary` by floored integer length, falling
/// back to the lowest index when none resolves.
fn longest_line(map: &Map, boundary: &[usize]) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for index in boundary {
        let Some(length) = linedef_length(map, *index) else {
            continue;
        };
        if best.is_none_or(|(known, _)| length > known) {
            best = Some((length, *index));
        }
    }
    best.map(|(_, index)| index)
        .or_else(|| boundary.iter().copied().next())
}

/// The door wall for an explicit `linedef`: two-sided lines behave like
/// [`choose_between`] on their own sectors (honouring the given wall),
/// one-sided ones carry their room on the front side and are dug toward the
/// [`Side`] the call names (its front by default, its back through the wall).
pub(super) fn choose_linedef(
    map: &Map,
    line: usize,
    at: Option<Point>,
) -> Result<Chosen, CyberError> {
    let def = map
        .linedefs
        .get(line)
        .ok_or_else(|| missing_index(ElementKind::Linedefs, line, map.linedefs.len()))?;
    match map.line_sectors(def) {
        (Some(a), Some(b)) if a != b => Ok(Chosen {
            room_a: a,
            room_b: b,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: true,
        }),
        (Some(a), Some(_)) => Err(CyberError::invalid(&format!(
            "both sides of linedef {line} face sector {a}: a door wall separates two \
             sectors or bounds the space behind it, not one sector twice"
        ))),
        (Some(a), None) => Ok(Chosen {
            room_a: a,
            room_b: a,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: false,
        }),
        (None, Some(b)) => Err(CyberError::invalid(&format!(
            "linedef {line} has only a back side (sector {b}): flip it or give it a \
             front sidedef before cutting a door into it (see add_sidedef)"
        ))),
        (None, None) => Err(CyberError::invalid(&format!(
            "linedef {line} has no usable sidedef: a door wall needs a front side \
             facing its room (see add_sidedef)"
        ))),
    }
}

/// The wall the parameters pick, validated against `map`. A `side` is
/// accepted only for a one-sided wall: a two-sided one already names the
/// two sectors its recess spans.
pub(super) fn choose_wall(
    map: &Map,
    target: Target,
    centre: Option<Point>,
    side: Option<Side>,
) -> Result<Chosen, CyberError> {
    let chosen = match target {
        Target::Between { a, b } => choose_between(map, a, b, centre)?,
        Target::Linedef(line) => choose_linedef(map, line, centre)?,
    };
    if chosen.touching {
        require_one_sided(side, chosen.wall)?;
    }
    Ok(chosen)
}

/// The [`OverlayRequest`] carving `polygon` out as solid rock, dressing
/// bare sectors' new walls with `wall`.
fn void_request(polygon: &[Point], wall: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: to_pts(polygon),
        paint: Paint::Void,
        keep_interior: false,
        textures: WallTextures {
            wall: wall.to_owned(),
            upper: wall.to_owned(),
            lower: wall.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// A [`Point`] list widened to the arrangement's `i64` grid.
pub(super) fn to_pts(points: &[Point]) -> Vec<Pt> {
    points
        .iter()
        .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
        .collect()
}

/// Carve the void strip along the whole shared boundary of the chosen
/// wall's sectors, and report the notes the carve left.
///
/// Touching rooms would share their wall with the feature drawn across
/// it: the strip (thickness [`STRIP_THICKNESS`]) turns them apart so the
/// recess or window has a real space to span. `wall` dresses the bare
/// edges and `spanner` names the thing that spans the strip in the note.
pub(super) fn carve_strip(
    map: &mut Map,
    chosen: &Chosen,
    wall: &str,
    spanner: &str,
) -> Result<Vec<String>, CyberError> {
    let (a, b) = (chosen.room_a, chosen.room_b);
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        CyberError::invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary to carve; re-read the map \
             and pass current indices"
        ))
    })?;
    let strip = void_strip(map, &boundary, STRIP_THICKNESS)?;
    let drawn = overlay::apply_overlay(map, &void_request(&strip.polygon, wall))?;
    let mut notes = vec![format!(
        "carved a {STRIP_THICKNESS}-unit void strip along the shared boundary of sectors \
         {a} and {b}: {spanner} spans it"
    )];
    notes.extend(drawn.notes);
    Ok(notes)
}

/// The room's wall along a freshly carved strip: the one-sided wall of
/// `room` whose midpoint sits nearest `near`; `None` when the carve left
/// the room without a face to it.
pub(super) fn strip_wall(map: &Map, room: usize, near: Point) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for (index, line) in map.linedefs.iter().enumerate() {
        if !room_face(map, line, room) {
            continue;
        }
        let Some(square) = mid_distance_sq(map, index, near) else {
            continue;
        };
        if best.is_none_or(|(known, _)| square < known) {
            best = Some((square, index));
        }
    }
    best.map(|(_, index)| index)
}

/// The refusal when the strip carve left room `room` unfaceable.
pub(super) fn strip_face_lost(room: usize) -> CyberError {
    CyberError::invalid(&format!(
        "sector {room} has no wall facing the carved strip: the void carve removed its \
         boundary; check the two sectors' shared boundary and re-read the map"
    ))
}

/// The depth of a one-sided wall's recess on the side `dig` names: the
/// explicit `depth` when the call gave one, else the distance the ray on
/// that side measures to the sector it stops at.
pub(super) fn recess_depth(
    map: &Map,
    wall: &DoorWall,
    wanted: Option<i32>,
    dig: Side,
) -> Result<i32, CyberError> {
    match wanted {
        Some(given) if given > 0_i32 => Ok(given),
        Some(_) => Err(CyberError::invalid(
            "the door depth must be a positive number of map units (the recess the \
             door slides into)",
        )),
        None => reach(map, wall, dig)
            .map_or_else(|| Err(unreachable(map, wall, dig)), |(hit, _)| Ok(hit)),
    }
}

/// How far the recess can go on `dig`'s side of `wall` before it breaks
/// into a sector: the outward ray over the front, the back-side ray
/// through the wall.
fn reach(map: &Map, wall: &DoorWall, dig: Side) -> Option<(i32, usize)> {
    match dig {
        Side::Front => outward_depth(map, wall.linedef, wall.point, MAX_DEPTH),
        Side::Back => backside_depth(map, wall.linedef, wall.point, MAX_DEPTH),
    }
}

/// The refusal for a wall whose `dig` side reaches no sector: [`no_room`],
/// with the hint naming `side` when the sector does lie on the wall's
/// other side.
fn unreachable(map: &Map, wall: &DoorWall, dig: Side) -> CyberError {
    let other = dig.other();
    let Some((steps, _)) = reach(map, wall, other) else {
        return no_room();
    };
    let side = other.as_str();
    no_room().map_message(|message| {
        format!(
            "{message}; a sector lies {steps} units on this wall's {side} side: pass \
             side: \"{side}\""
        )
    })
}

/// The refusal for `side` on a wall that has a sector on both of its
/// sides: there the two rooms are named by the wall itself and the recess
/// spans the carved strip between them, so the parameter would decide
/// nothing.
fn require_one_sided(side: Option<Side>, wall: usize) -> Result<(), CyberError> {
    let Some(given) = side else {
        return Ok(());
    };
    Err(CyberError::invalid(&format!(
        "side: \"{}\" picks a side for a one-sided wall: linedef {wall} has a sector on \
         both of its sides; leave side out, or name the two sectors to join with between",
        given.as_str(),
    )))
}

/// The note one `add_door`/`add_window` run leaves about the side a
/// one-sided wall was dug toward: which way the recess went and what that
/// did to the named wall, which is the part a caller cannot see in the
/// indices it passed.
pub(super) fn dig_note(chosen: &Chosen, dig: Side, name: &str) -> String {
    match dig {
        Side::Front => format!(
            "dug the {name} toward linedef {}'s front side, into sector {}; that wall \
             itself is closed as a jamb",
            chosen.wall, chosen.room_a
        ),
        Side::Back => format!(
            "dug the {name} through linedef {}'s back side; the named wall itself now \
             faces the recess",
            chosen.wall
        ),
    }
}

/// The knobs a recess is fitted from, shared by `add_door` and
/// `add_window`: the opening width, the depth the call asked for (a door's
/// own knob; the window leaves it out) and the [`Side`] of a one-sided
/// wall it digs toward.
#[derive(Clone, Copy, Debug)]
pub(super) struct Recess {
    /// The opening width in map units.
    pub(super) width: i32,
    /// The requested recess depth, if any.
    pub(super) depth: Option<i32>,
    /// The requested dig side, if any.
    pub(super) side: Option<Side>,
}

/// The rectangle a recess is drawn in, shared by the door and the window:
/// the cut fitted to the wall (a feature too wide for it refuses with the
/// wall's available length, before any drawing) and grown across the
/// carved strip between touching rooms, or to the [`Recess`]'s depth on
/// the side of a one-sided wall it digs toward.
pub(super) fn recess_rect(
    map: &Map,
    chosen: &Chosen,
    recess: Recess,
) -> Result<Vec<Point>, CyberError> {
    let wall = door_wall(map, chosen.wall, Some(chosen.centre), recess.width)?;
    let normal = outward_normal(map, chosen.wall).ok_or_else(|| wall_lost(chosen.wall))?;
    if chosen.touching {
        return Ok(door_rect(&wall, normal, STRIP_THICKNESS, true));
    }
    let dig = recess.side.unwrap_or_default();
    let depth = recess_depth(map, &wall, recess.depth, dig)?;
    Ok(door_rect(&wall, normal, depth, dig == Side::Back))
}

/// The side a drawn feature is on: the sector `openings` faces with the
/// feature's own `sector` on the other side, `None` when the line does not
/// resolve to two sectors.
fn far_sector(map: &Map, line: usize, sector: usize) -> Option<usize> {
    let (front, back) = map
        .linedefs
        .get(line)
        .map_or((None, None), |def| map.line_sectors(def));
    front.into_iter().chain(back).find(|&other| other != sector)
}

/// A drawn recess, ready for [`require_connected`] to judge it: the wall it
/// was cut into and the rooms that wall named, the sector the paint made,
/// the room-facing lines around it, the side a one-sided wall was dug
/// toward, and the name the refusal calls the feature by.
pub(super) struct Landing<'land> {
    /// The chosen wall, its rooms and the side the strip faces.
    pub(super) chosen: &'land Chosen,
    /// Index of the sector the paint drew.
    pub(super) sector: usize,
    /// The two-sided lines facing that sector.
    pub(super) openings: &'land [usize],
    /// The side of a one-sided wall the recess was dug toward.
    pub(super) dig: Side,
    /// What the refusal calls the feature: "door recess" or "window strip".
    pub(super) name: &'static str,
}

/// Refuse a drawn recess that connects nothing: a door or a window is only
/// worth having when one of its room-facing lines faces a sector other than
/// `room_a`, the room the wall already fronts. The refusal names the `side`
/// or `depth` that would reach one.
pub(super) fn require_connected(map: &Map, landing: &Landing<'_>) -> Result<(), CyberError> {
    let connects = landing
        .openings
        .iter()
        .filter_map(|&line| far_sector(map, line, landing.sector))
        .any(|other| other != landing.chosen.room_a);
    if connects {
        return Ok(());
    }
    Err(disconnected(landing))
}

/// The refusal for a recess whose every side is the room it was cut from:
/// the fix is the other `side` when the wall was dug toward its front, the
/// depth (or a room drawn on the far side) when it was dug through.
fn disconnected(landing: &Landing<'_>) -> CyberError {
    let wall = landing.chosen.wall;
    let room_a = landing.chosen.room_a;
    let name = landing.name;
    let hint = match landing.dig {
        Side::Front => format!(
            "pass side: \"back\" to dig the {name} through linedef {wall}, or a larger \
             depth so it reaches the sector beyond"
        ),
        Side::Back => format!(
            "nothing lies behind linedef {wall} for the {name} to reach: give a larger \
             depth, or draw the room on that side with create_sector_from_loop"
        ),
    };
    CyberError::invalid(&format!(
        "the {name} on linedef {wall} opens back into sector {room_a} on every side: it \
         would connect nothing; {hint}"
    ))
}

/// The door rectangle to paint: [`recess_rect`] on the spec's knobs, stood
/// on the chosen wall as a [`Placed`] recess.
pub(super) fn place_rect(
    map: &Map,
    chosen: &Chosen,
    spec: &DoorSpec,
) -> Result<Placed, CyberError> {
    Ok(Placed {
        wall: chosen.wall,
        room_a: chosen.room_a,
        rect: recess_rect(map, chosen, spec.recess())?,
        tag: None,
    })
}
