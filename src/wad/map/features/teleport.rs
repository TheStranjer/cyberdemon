//! Pure teleporter core: the walk-on action of a pad border and the two
//! squares, tagged things and outward-facing lines of a working
//! teleporter.
//!
//! [`build_teleporter`] takes a [`TeleportSpec`] naming the pad centre,
//! the destination centre (with the arriving player's facing `angle`), a
//! `pad_size`, the `two_way` flag and the caller's [`TeleportSpec::floor_flat`]
//! (the theme's teleporter flat) and [`TeleportSpec::wall`] (the texture
//! bare new edges fall back to), and folds both squares over the map
//! with [`apply_overlay`] ([`Paint::Sector`], each record copying the
//! sector under its centre): the pad wears `floor_flat`, a
//! teleport-destination [`Thing`] (type [`TELEPORT_DESTINATION_THING`])
//! stands at the destination, and in the `Doom` family the destination
//! sector answers with a fresh [`next_free_tag`] (its vanilla search
//! finds the thing through the tagged sector; the `Hexen` family reaches
//! it through the thing's TID and leaves its sectors untagged). Every
//! two-sided border of the pad then carries [`teleport_specials`],
//! oriented with its FRONT facing out of the pad so a player walking
//! onto it from the surrounding rooms triggers the jump: the `Doom`
//! family answers with the destination's tag, the `Hexen` family puts
//! the thing's TID in `args[0]` with `playercross` plus `repeatspecial`
//! activation. The `two_way` leg runs the same pattern back — its own
//! fresh tag/TID, a return thing standing in the pad and facing the
//! opposite way (the `Doom` pad sector tagged for its vanilla search) —
//! and dresses the destination's borders with it.
//! Like the rest of [`crate::wad`] this is pure model arithmetic and
//! table lookup — no sessions, no archives, no `MCP`.

use serde::Serialize;

use crate::error::CyberError;
use crate::geom::Pt;
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::{Linedef, Map, Sector, Thing};
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{Outside, OverlayRequest, Paint, WallTextures, apply_overlay};

use super::doors::{DoorLineSetup, next_free_tag};

/// The `blocking` linedef flag bit, cleared on the dressed borders so
/// the player can reach and walk across them (crossing is the trigger).
const BLOCKING_BIT: u32 = 1_u32;
/// The `Hexen` `repeatspecial` bit: a teleporter pad keeps working.
const REPEAT_SPECIAL_BIT: u32 = 0x200;
/// The `Hexen` trigger bits [`dress_border`] replaces wholesale: the
/// 3-bit activation field plus the `repeatspecial` bit, so the border
/// ends up exactly [`teleport_specials`] names.
const TRIGGER_MASK: u32 = ACTIVATION_MASK | REPEAT_SPECIAL_BIT;
/// The `Hexen` `playercross` activation: the activation field's zero
/// value — a line walked over, not used.
const CROSS: u32 = 0_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
///
/// [`Sidedef`]: crate::wad::map::Sidedef
const NO_TEXTURE: &str = "-";
/// The `Doom` family's walk-over teleport line: `WR` teleport,
/// repeatable, answering with the destination sector's tag.
const DOOM_WR_TELEPORT: i32 = 97;
/// The `Hexen` family's `Teleport` line special, targeting the
/// destination thing's TID through `args[0]`.
const HEXEN_TELEPORT: i32 = 70;
/// Degrees in a full turn: thing angles are normalised into `0..=359`.
const FULL_CIRCLE: i32 = 360;
/// Half a turn: the way the `two_way` return thing faces relative to
/// the destination thing, so the pair looks at each other.
const HALF_CIRCLE: i32 = 180;
/// The thing number of a teleport destination (the arrival spot both
/// families' teleport lines look for).
pub const TELEPORT_DESTINATION_THING: i32 = 14;
/// The `add_teleporter` default side length of both squares, in map
/// units.
pub const DEFAULT_PAD_SIZE: i32 = 64_i32;
/// The presence bits of a placed thing: visible on every skill level
/// (the same bits the binary-format fixtures spawn with).
const VISIBLE_FLAGS: u32 = 7_u32;

/// Everything one teleporter needs, as the tool layer resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeleportSpec {
    /// The centre of the pad the player walks onto.
    pub pad: Point,
    /// The centre of the destination the player arrives in.
    pub destination: Point,
    /// The arriving player's facing at the destination, in degrees
    /// (normalised into `0..=359`).
    pub angle: i32,
    /// The side length of both squares, in map units (positive).
    pub pad_size: i32,
    /// Whether the destination leads back to the pad as well.
    pub two_way: bool,
    /// The pad's floor flat (`"-"` keeps the sector under the pad).
    pub floor_flat: String,
    /// The texture bare new edges (walls across nothing) fall back to.
    pub wall: String,
}

/// What one [`build_teleporter`] call did, as the tool reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TeleportBuild {
    /// Index in [`Map::sectors`] of the drawn pad sector.
    pub pad_sector: usize,
    /// Index in [`Map::sectors`] of the drawn destination sector.
    pub destination_sector: usize,
    /// Indices in [`Map::things`] of the placed teleport destinations,
    /// destination first and the return thing at the pad (if any) after.
    pub things: Vec<usize>,
    /// Indices of the dressed pad border lines, ascending.
    pub pad_lines: Vec<usize>,
    /// Indices of the dressed destination border lines (`two_way` only,
    /// ascending; empty on a one-way teleporter).
    pub destination_lines: Vec<usize>,
    /// The destination's fresh tag (`Doom`) or the destination thing's
    /// fresh TID (`Hexen`) the pad lines answer with.
    pub tag: i32,
    /// The return leg's own tag/TID (`two_way` only, `None` otherwise).
    pub return_tag: Option<i32>,
    /// What the two drawings reported.
    pub notes: Vec<String>,
    /// The pre-build linedef index to the post-build index of its first
    /// surviving piece (`None` for a line the two square rebuilds
    /// painted over or split away): the arrangement sorts every piece,
    /// so the squares renumber the whole map and a caller holding line
    /// indices from before the build follows them through this map.
    /// Skipped by `JSON`: it is a handoff for index-tracking callers
    /// (the plan feature engine), not part of a tool's report.
    #[serde(skip)]
    pub line_remap: Vec<Option<usize>>,
}

/// The [`DoorLineSetup`] a teleporter border carries in flag family
/// `family`, pointing at the destination named by `tag` (the `Doom`
/// family's sector tag, the `Hexen` family's thing TID).
///
/// The table:
///
/// | Family | Special | Line tag | Args | Activation |
/// |---|---|---|---|---|
/// | `Doom` | `97` (`WR` teleport) | `tag` | all zero | none (flags kept) |
/// | `Hexen` | `70` `Teleport` | `0` | `[tag, 0, 0, 0, 0]` | `playercross` + `repeatspecial` |
///
/// A teleporter is always walk-on and always repeatable, so the `Doom`
/// family gets the `WR` (not `W1`) form and the `Hexen` family's setup
/// carries the field's zero value (`playercross`) with the repeat bit on
/// top.
#[must_use]
#[inline]
pub const fn teleport_specials(family: FlagFamily, tag: i32) -> DoorLineSetup {
    match family {
        FlagFamily::Doom => DoorLineSetup {
            special: DOOM_WR_TELEPORT,
            tag,
            args: [0_i32; 5],
            flags_activation: 0_u32,
        },
        FlagFamily::Hexen => DoorLineSetup {
            special: HEXEN_TELEPORT,
            tag: 0_i32,
            args: [tag, 0_i32, 0_i32, 0_i32, 0_i32],
            flags_activation: CROSS | REPEAT_SPECIAL_BIT,
        },
    }
}

/// One border line of a teleporter square, planned so its front ends up
/// facing out (the player walking from the outer sector into the square
/// crosses it front-first).
#[derive(Clone, Copy, Debug)]
struct EdgePlan {
    /// Index in [`Map::linedefs`] of the line.
    line: usize,
    /// Whether the front side belongs to the square, so the line turns
    /// around with its sides.
    flip: bool,
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// Refuses a `pad_size` that is no square side.
fn check_size(spec: &TeleportSpec) -> Result<(), CyberError> {
    if spec.pad_size > 0_i32 {
        return Ok(());
    }
    Err(invalid(&format!(
        "pad_size {} is no side length: pass a positive number of map units \
         (the default {DEFAULT_PAD_SIZE})",
        spec.pad_size
    )))
}

/// The sector holding `point`, refused with role-naming instructions
/// when the map holds no sector there.
fn centre_sector(map: &Map, point: Point, role: &str) -> Result<usize, CyberError> {
    point_in_sector(map, i64::from(point.x), i64::from(point.y)).ok_or_else(|| {
        invalid(&format!(
            "the {role} point {}, {} lies in no sector: pass a point inside the \
             room the {role} stands in (get_map_summary lists the rooms)",
            point.x, point.y
        ))
    })
}

/// The refusal for a square whose border no player can walk across.
fn no_borders(sector: usize) -> CyberError {
    invalid(&format!(
        "sector {sector} has no two-sided border a player can cross: a teleporter \
         square must sit inside (or across) walkable rooms, not float in solid rock; \
         move its point onto a room (get_map_summary lists the rooms)"
    ))
}

/// The server-bug refusal for a square paint that reported no sector.
fn paint_lost() -> CyberError {
    CyberError::Internal("the teleporter square paint reported no sector (server bug)".to_owned())
}

/// What one [`draw_square`] reports: the new square's sector index,
/// the paint's notes, and the paint's old-to-new linedef map.
type SquareDraw = (usize, Vec<String>, Vec<Option<usize>>);

/// The angle normalised into `0..=359` degrees.
const fn normalize_angle(angle: i32) -> i32 {
    angle.rem_euclid(FULL_CIRCLE)
}

/// The way a `two_way` return thing faces: the destination angle turned
/// half a circle around, so the pair looks at each other.
const fn reversed_angle(angle: i32) -> i32 {
    normalize_angle(angle)
        .saturating_add(HALF_CIRCLE)
        .rem_euclid(FULL_CIRCLE)
}

/// The axis-aligned `size`-wide square centred on `centre`, corners
/// counterclockwise from the lower-left (an odd size gives its extra
/// unit to the upper right).
fn pad_square(centre: Point, size: i32) -> Vec<Pt> {
    let half = i64::from(size.saturating_div(2));
    let wide = i64::from(size);
    let x = i64::from(centre.x).saturating_sub(half);
    let y = i64::from(centre.y).saturating_sub(half);
    vec![
        (x, y),
        (x.saturating_add(wide), y),
        (x.saturating_add(wide), y.saturating_add(wide)),
        (x, y.saturating_add(wide)),
    ]
}

/// The sector record for one square: the base's heights, flats and
/// light, `floor_flat` when the caller named one (and it is not `"-"`),
/// inert and tagged `tag`.
fn square_record(map: &Map, base: usize, floor_flat: Option<&str>, tag: i32) -> Sector {
    let mut record = map.sectors.get(base).cloned().unwrap_or_default();
    record.special = 0_i32;
    record.tag = tag;
    if let Some(flat) = floor_flat.filter(|name| *name != NO_TEXTURE) {
        flat.clone_into(&mut record.floor_flat);
    }
    record
}

/// The destination sector's tag under `family`: the `Doom` family finds
/// the thing through the tagged sector and answers with `tag`; the
/// `Hexen` family reaches it through the thing's TID, so its sectors
/// stay untagged.
const fn destination_tag(family: FlagFamily, tag: i32) -> i32 {
    match family {
        FlagFamily::Doom => tag,
        FlagFamily::Hexen => 0_i32,
    }
}

/// Fold one teleporter square over the map as a new sector, reporting
/// its index, the paint's notes and the paint's old-to-new linedef map.
fn draw_square(
    map: &mut Map,
    spec: &TeleportSpec,
    centre: Point,
    record: &Sector,
) -> Result<SquareDraw, CyberError> {
    let request = OverlayRequest {
        polygon: pad_square(centre, spec.pad_size),
        paint: Paint::Sector(record.clone()),
        keep_interior: false,
        textures: WallTextures {
            wall: spec.wall.clone(),
            upper: spec.wall.clone(),
            lower: spec.wall.clone(),
        },
        outside: Outside::Original,
    };
    let drawn = apply_overlay(map, &request)?;
    let sector = drawn.sector.ok_or_else(paint_lost)?;
    Ok((sector, drawn.notes, drawn.linedef_map))
}

/// Chain two rebuilds' old-to-new linedef maps: what index `i` became
/// after the first rebuild, then after the second; `None` when either
/// step lost the line.
fn chain_remaps(first: &[Option<usize>], second: &[Option<usize>]) -> Vec<Option<usize>> {
    first
        .iter()
        .copied()
        .map(|slot| slot.and_then(|mid| second.get(mid).copied().flatten()))
        .collect()
}

/// The sector the sidedef slot `side` opens into: `None` when the side
/// is absent, its sidedef index dangles, or that sidedef's sector
/// dangles.
fn owned_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sector = side.and_then(|index| map.sidedefs.get(index))?.sector;
    map.sectors.get(sector).map(|_| sector)
}

/// Whether `line` is a square border a player can cross from outside:
/// two-sided with `sector` on exactly one side and a resolvable sector
/// on the other, as the [`EdgePlan`] that turns its front outwards.
fn border_edge(map: &Map, line: usize, def: &Linedef, sector: usize) -> Option<EdgePlan> {
    let front = owned_sector(map, def.front)?;
    let back = owned_sector(map, def.back)?;
    match (front == sector, back == sector) {
        (true, false) => Some(EdgePlan { line, flip: true }),
        (false, true) => Some(EdgePlan { line, flip: false }),
        (true, true) | (false, false) => None,
    }
}

/// The crossable borders of sector `sector`, in linedef order.
fn border_edges(map: &Map, sector: usize) -> Vec<EdgePlan> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(line, def)| border_edge(map, line, def, sector))
        .collect()
}

/// Dress one border: turn it so its front faces out of the square (the
/// player crosses it walking in), write the family's teleport action on
/// it (the `Hexen` family replaces the whole trigger — activation field
/// and `repeatspecial` — the `Doom` family keeps its flags except the
/// blocking bit), and leave the line reachable.
fn dress_border(map: &mut Map, edge: EdgePlan, setup: &DoorLineSetup) {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    if let Some(line) = map.linedefs.get_mut(edge.line) {
        if edge.flip {
            core::mem::swap(&mut line.v1, &mut line.v2);
            core::mem::swap(&mut line.front, &mut line.back);
        }
        line.special = setup.special;
        line.tag = setup.tag;
        line.args = setup.args;
        line.flags &= !BLOCKING_BIT;
        if hexen {
            line.flags = (line.flags & !TRIGGER_MASK) | setup.flags_activation;
        }
    }
}

/// Dress every crossable border of sector `sector` with `setup`,
/// reporting the line indices (ascending); a square nothing touches is
/// refused with instructions.
fn dress_borders(
    map: &mut Map,
    sector: usize,
    setup: &DoorLineSetup,
) -> Result<Vec<usize>, CyberError> {
    let edges = border_edges(map, sector);
    if edges.is_empty() {
        return Err(no_borders(sector));
    }
    let lines = edges.iter().map(|edge| edge.line).collect();
    for edge in edges {
        dress_border(map, edge, setup);
    }
    Ok(lines)
}

/// Append a teleport destination standing at `at`, facing `angle` and
/// answering with `tid` (the `Hexen` family's thing id; the `Doom`
/// family is found through the sector tag instead and carries none).
fn place_destination(map: &mut Map, at: Point, angle: i32, tid: i32) -> usize {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    map.things.push(Thing {
        tid: if hexen { tid } else { 0_i32 },
        x: at.x,
        y: at.y,
        angle: normalize_angle(angle),
        kind: TELEPORT_DESTINATION_THING,
        flags: VISIBLE_FLAGS,
        ..Default::default()
    });
    map.things.len().saturating_sub(1)
}

/// Store `tag` in sector `index` (the `two_way` `Doom` return leg: the
/// pad sector answers with the return tag so vanilla finds the thing
/// standing in the pad; the `Hexen` leg passes `0`, where it reads as a
/// no-op).
fn stamp_tag(map: &mut Map, sector: usize, tag: i32) {
    if let Some(row) = map.sectors.get_mut(sector) {
        row.tag = tag;
    }
}

/// Dress the `two_way` return leg: a fresh tag/TID, a destination thing
/// standing at the pad and facing the way back (the given angle turned
/// half a circle, so the pair looks at each other), the pad sector
/// tagged to answer with it in the `Doom` family (whose vanilla search
/// finds the thing through the tagged sector), and the destination
/// sector's borders carrying the same action pointing at it.
fn build_return_leg(
    map: &mut Map,
    spec: &TeleportSpec,
    build: &mut TeleportBuild,
) -> Result<(), CyberError> {
    let back = next_free_tag(map);
    let pad_thing = place_destination(map, spec.pad, reversed_angle(spec.angle), back);
    build.things.push(pad_thing);
    let setup = teleport_specials(map.flag_family(), back);
    build.destination_lines = dress_borders(map, build.destination_sector, &setup)?;
    stamp_tag(
        map,
        build.pad_sector,
        destination_tag(map.flag_family(), back),
    );
    build.return_tag = Some(back);
    Ok(())
}

/// Build a working teleporter as one map edit.
///
/// Draw the pad and the destination squares through the overlay engine,
/// place the type-14 destination thing, dress the pad's outward-facing
/// borders with [`teleport_specials`] answering with the destination's
/// fresh tag (or TID), and, with [`TeleportSpec::two_way`], run the
/// return leg that makes the destination lead back.
///
/// Both drawings rebuild the map from a sorted arrangement, so every
/// linedef index can move: [`TeleportBuild::line_remap`] carries the
/// composed pre-build-to-final index map for callers that hold line
/// indices across the call.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-positive `pad_size`, a pad
/// or destination point in no sector, or a square whose borders face
/// only solid rock; [`CyberError::Internal`] when a square paint reports
/// no sector. The shape and point refusals write nothing; the border
/// refusal can only be found after the squares are drawn, and the
/// caller's edit step rolls it back.
///
/// # Panics
///
/// Never: every map access is checked.
#[inline]
pub fn build_teleporter(map: &mut Map, spec: &TeleportSpec) -> Result<TeleportBuild, CyberError> {
    check_size(spec)?;
    let pad_base = centre_sector(map, spec.pad, "pad")?;
    let destination_base = centre_sector(map, spec.destination, "destination")?;
    let family = map.flag_family();
    let tag = next_free_tag(map);
    let (pad_sector, mut notes, pad_remap) = draw_square(
        map,
        spec,
        spec.pad,
        &square_record(map, pad_base, Some(&spec.floor_flat), 0_i32),
    )?;
    let (destination_sector, drawn, dest_remap) = draw_square(
        map,
        spec,
        spec.destination,
        &square_record(map, destination_base, None, destination_tag(family, tag)),
    )?;
    notes.extend(drawn);
    let destination_thing = place_destination(map, spec.destination, spec.angle, tag);
    let pad_lines = dress_borders(map, pad_sector, &teleport_specials(family, tag))?;
    let mut build = TeleportBuild {
        pad_sector,
        destination_sector,
        things: vec![destination_thing],
        pad_lines,
        destination_lines: Vec::new(),
        tag,
        return_tag: None,
        notes,
        line_remap: chain_remaps(&pad_remap, &dest_remap),
    };
    if spec.two_way {
        build_return_leg(map, spec, &mut build)?;
    }
    Ok(build)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_PAD_SIZE, TELEPORT_DESTINATION_THING, TeleportSpec, border_edges, build_teleporter,
        dress_borders, pad_square, teleport_specials,
    };
    use crate::error::CyberError;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, two_room_map, vertex};
    use crate::wad::map::features::DoorLineSetup;
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::ops::Point;
    use crate::wad::map::{Linedef, Map, MapFormat};

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    fn refuse(error: &CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    /// A point at `x`/`y`.
    const fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// A one-way teleporter spec: a pad in the hall, a destination in
    /// the raised room, the theme's flat and wall.
    fn spec() -> TeleportSpec {
        TeleportSpec {
            pad: point(64, 64),
            destination: point(384, 64),
            angle: 0_i32,
            pad_size: DEFAULT_PAD_SIZE,
            two_way: false,
            floor_flat: "GATE1".to_owned(),
            wall: "WALL1".to_owned(),
        }
    }

    /// A single-room 128 square: every wall one-sided, nothing behind
    /// any of them.
    fn one_room() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(0, 128),
                vertex(128, 128),
                vertex(128, 0),
            ],
            linedefs: vec![
                solid_line(0, 1, 0),
                solid_line(1, 2, 0),
                solid_line(2, 3, 0),
                solid_line(3, 0, 0),
            ],
            sidedefs: vec![
                side("-", "-", "WALL1", 0),
                side("-", "-", "WALL1", 0),
                side("-", "-", "WALL1", 0),
                side("-", "-", "WALL1", 0),
            ],
            sectors: vec![room(0, 128, 160, 0)],
            ..Default::default()
        }
    }

    /// The sector a sidedef slot opens into.
    fn side_sector(map: &Map, slot: Option<usize>) -> Option<usize> {
        let index = slot?;
        map.sidedefs.get(index).map(|sidedef| sidedef.sector)
    }

    /// The linedef at `index`, named by its border role.
    fn border_line(map: &Map, index: usize) -> &Linedef {
        map.linedefs.get(index).expect("a border line")
    }

    /// Requires every listed border line to carry the action pair
    /// `action` (`special`, line tag), its front facing out of the
    /// square sector `square.0` into the room `square.1` behind nobody
    /// but that room, and the square behind.
    fn expect_outward_borders(
        map: &Map,
        indices: &[usize],
        square: (usize, usize),
        action: (i32, i32),
    ) {
        assert_eq!(indices.len(), 4, "a border line on every side");
        for index in indices {
            let line = border_line(map, *index);
            assert_eq!(
                line.special, action.0,
                "the teleport action on line {index}"
            );
            assert_eq!(line.tag, action.1, "the line tag of line {index}");
            assert_eq!(
                side_sector(map, line.front),
                Some(square.1),
                "line {index} fronts the room outside the square"
            );
            assert_eq!(
                side_sector(map, line.back),
                Some(square.0),
                "and the square lies behind it"
            );
        }
    }

    #[test]
    fn teleport_specials_matches_the_doom_table() {
        assert_eq!(
            teleport_specials(FlagFamily::Doom, 42_i32),
            DoorLineSetup {
                special: 97_i32,
                tag: 42_i32,
                args: [0_i32; 5],
                flags_activation: 0_u32,
            },
            "WR teleport answering with the destination tag"
        );
    }

    #[test]
    fn teleport_specials_matches_the_hexen_table() {
        assert_eq!(
            teleport_specials(FlagFamily::Hexen, 42_i32),
            DoorLineSetup {
                special: 70_i32,
                tag: 0_i32,
                args: [42_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                flags_activation: 0x200_u32,
            },
            "Teleport, TID in args[0], playercross (the zero field) plus repeatspecial"
        );
    }

    #[test]
    fn the_square_straddles_its_centre() {
        assert_eq!(
            pad_square(point(64, 64), 64_i32),
            vec![(32_i64, 32_i64), (96, 32), (96, 96), (32, 96)],
            "counterclockwise from the lower left"
        );
        assert_eq!(
            pad_square(point(64, 64), 65_i32),
            vec![(32_i64, 32_i64), (97, 32), (97, 97), (32, 97)],
            "an odd size gives its extra unit to the upper right"
        );
    }

    #[test]
    fn a_doom_pad_dresses_its_borders_to_the_tagged_destination() {
        let mut map = two_room_map();
        let build = build_teleporter(&mut map, &spec()).expect("a teleporter");
        assert_eq!(build.pad_sector, 2_usize, "the third sector");
        assert_eq!(build.destination_sector, 3_usize, "the fourth");
        assert_eq!(build.tag, 2_i32, "a fresh tag above room 1's 1");
        assert_eq!(build.return_tag, None, "one-way");
        assert_eq!(build.things, vec![2_usize], "the destination thing");
        let pad = map.sectors.get(2).expect("the pad sector");
        assert_eq!(pad.floor_flat, "GATE1", "the theme's teleport floor");
        assert_eq!(
            (pad.floor_height, pad.ceiling_height),
            (0_i32, 128_i32),
            "the hall's heights"
        );
        assert_eq!(pad.tag, 0_i32, "one-way pads carry no tag");
        let destination = map.sectors.get(3).expect("the destination sector");
        assert_eq!(
            destination.tag, 2_i32,
            "the Doom search finds the thing through the tagged sector"
        );
        assert_eq!(destination.floor_height, 32_i32, "room 1's floor");
        assert_eq!(
            destination.floor_flat, "FLOOR1",
            "the destination keeps the base flat"
        );
        let thing = map.things.get(2).expect("the destination thing");
        assert_eq!(thing.kind, TELEPORT_DESTINATION_THING, "type 14");
        assert_eq!((thing.x, thing.y, thing.angle), (384_i32, 64_i32, 0_i32));
        assert_eq!(thing.tid, 0_i32, "Doom things carry no tid");
        expect_outward_borders(&map, &build.pad_lines, (2_usize, 0_usize), (97_i32, 2_i32));
        for index in &build.pad_lines {
            let line = border_line(&map, *index);
            assert_eq!(line.args, [0_i32; 5], "Doom lines carry no args");
            assert_eq!(line.flags, 4_u32, "the Doom line keeps its two-sided flags");
        }
    }

    #[test]
    fn a_hexen_pad_threads_the_tid_through_crossing_lines() {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        let wanted = TeleportSpec {
            angle: 45_i32,
            ..spec()
        };
        let build = build_teleporter(&mut map, &wanted).expect("a Hexen teleporter");
        assert_eq!(build.tag, 2_i32, "one above every tag the map carries");
        let thing = map.things.get(2).expect("the destination thing");
        assert_eq!(thing.tid, build.tag, "the fresh TID rides the thing");
        assert_eq!(thing.angle, 45_i32, "the arriving player's facing");
        assert_eq!(
            map.sectors.get(3).expect("the destination").tag,
            0_i32,
            "a Hexen teleporter reaches the thing, not a tagged sector"
        );
        expect_outward_borders(&map, &build.pad_lines, (2_usize, 0_usize), (70_i32, 0_i32));
        for index in &build.pad_lines {
            let line = border_line(&map, *index);
            assert_eq!(
                line.args,
                [2_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                "the TID in args[0] of line {index}"
            );
            assert_eq!(
                line.flags,
                4_u32 | 0x200_u32,
                "playercross (the zero field) plus repeatspecial, two-sided stays"
            );
        }
    }

    #[test]
    fn a_two_way_doom_teleporter_returns_with_its_own_tag() {
        let mut map = two_room_map();
        let wanted = TeleportSpec {
            two_way: true,
            angle: 90_i32,
            ..spec()
        };
        let build = build_teleporter(&mut map, &wanted).expect("a two-way teleporter");
        assert_eq!(build.tag, 2_i32, "the outward leg's tag");
        assert_eq!(build.return_tag, Some(3_i32), "the return leg's own tag");
        assert_eq!(build.things, vec![2_usize, 3_usize], "destination then pad");
        assert_eq!(
            map.sectors.get(2).expect("the pad").tag,
            3_i32,
            "the pad sector answers with the return tag"
        );
        let back = map.things.get(3).expect("the return thing");
        assert_eq!(back.kind, TELEPORT_DESTINATION_THING, "type 14");
        assert_eq!((back.x, back.y, back.tid), (64_i32, 64_i32, 0_i32));
        assert_eq!(back.angle, 270_i32, "facing the way back");
        expect_outward_borders(&map, &build.pad_lines, (2_usize, 0_usize), (97_i32, 2_i32));
        expect_outward_borders(
            &map,
            &build.destination_lines,
            (3_usize, 1_usize),
            (97_i32, 3_i32),
        );
    }

    #[test]
    fn a_two_way_hexen_teleporter_gives_the_return_thing_its_own_tid() {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        let wanted = TeleportSpec {
            two_way: true,
            ..spec()
        };
        let build = build_teleporter(&mut map, &wanted).expect("a two-way Hexen teleporter");
        assert_eq!(build.tag, 2_i32, "the outward TID");
        assert_eq!(build.return_tag, Some(3_i32), "the return TID");
        let back = map.things.get(3).expect("the return thing");
        assert_eq!(back.tid, 3_i32, "its own fresh TID");
        assert_eq!(back.angle, 180_i32, "half a circle from the outward angle");
        assert_eq!(
            map.sectors.get(2).expect("the pad").tag,
            0_i32,
            "the Hexen pad sector stays untagged"
        );
        expect_outward_borders(
            &map,
            &build.destination_lines,
            (3_usize, 1_usize),
            (70_i32, 0_i32),
        );
        for index in &build.destination_lines {
            let line = border_line(&map, *index);
            assert_eq!(
                line.args,
                [3_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                "the return TID in args[0] of line {index}"
            );
            assert_eq!(line.flags, 4_u32 | 0x200_u32, "cross and repeat");
        }
    }

    /// One linedef's `(v1, v2)` endpoint coordinates.
    type Corners = ((i32, i32), (i32, i32));

    /// The `(x, y)` endpoints of `map`'s linedef rows.
    fn all_ends(map: &Map) -> Vec<Corners> {
        map.linedefs
            .iter()
            .map(|line| {
                let first = map.vertices.get(line.v1).expect("test vertex");
                let second = map.vertices.get(line.v2).expect("test vertex");
                ((first.x, first.y), (second.x, second.y))
            })
            .collect()
    }

    /// The `(x, y)` endpoints of `map`'s linedef `index`.
    fn ends_of(map: &Map, index: usize) -> Corners {
        *all_ends(map)
            .get(index)
            .expect("a linedef row exists for a reported index")
    }

    #[test]
    fn the_line_remap_follows_every_prebuild_line_through_the_rebuilds() {
        let mut map = two_room_map();
        let before = all_ends(&map);
        let build = build_teleporter(&mut map, &spec()).expect("a teleporter");
        assert_eq!(
            build.line_remap.len(),
            before.len(),
            "one slot per pre-build line"
        );
        for (old, ends) in before.iter().enumerate() {
            let new = build
                .line_remap
                .get(old)
                .copied()
                .flatten()
                .expect("these squares cross no wall");
            let after = ends_of(&map, new);
            assert!(
                after == *ends || after == (ends.1, ends.0),
                "line {old} kept its geometry at new index {new}: {ends:?} vs {after:?}"
            );
        }
    }

    #[test]
    fn angles_wrap_into_the_vanilla_range() {
        let mut map = two_room_map();
        let wanted = TeleportSpec {
            two_way: true,
            angle: 450_i32,
            ..spec()
        };
        let build = build_teleporter(&mut map, &wanted).expect("a teleporter with odd angles");
        assert_eq!(build.things.len(), 2_usize, "both legs placed");
        assert_eq!(
            map.things.get(2).expect("outward").angle,
            90_i32,
            "450 arrives facing 90"
        );
        assert_eq!(
            map.things.get(3).expect("return").angle,
            270_i32,
            "and the return thing faces back"
        );
    }

    #[test]
    fn a_bad_size_or_a_point_in_the_void_writes_nothing() {
        for (broken, needle) in [
            (
                TeleportSpec {
                    pad_size: 0_i32,
                    ..spec()
                },
                "positive",
            ),
            (
                TeleportSpec {
                    pad_size: -8_i32,
                    ..spec()
                },
                "positive",
            ),
            (
                TeleportSpec {
                    pad: point(1000, 1000),
                    ..spec()
                },
                "the pad point 1000, 1000 lies in no sector",
            ),
            (
                TeleportSpec {
                    destination: point(1000, 1000),
                    ..spec()
                },
                "the destination point 1000, 1000 lies in no sector",
            ),
        ] {
            let mut map = two_room_map();
            let before = map.clone();
            let message = refuse(&build_teleporter(&mut map, &broken).unwrap_err());
            assert!(message.contains(needle), "{needle} must be in: {message}");
            assert_eq!(map, before, "the refusal wrote nothing");
        }
    }

    #[test]
    fn the_border_probe_keeps_only_crossable_two_sided_borders() {
        let map = two_room_map();
        let hall = border_edges(&map, 0_usize);
        assert_eq!(
            hall.len(),
            1_usize,
            "three of the hall's walls are one-sided and never count"
        );
        let step = hall.first().expect("the step line");
        assert_eq!(step.line, 2_usize, "the two-sided boundary counts");
        assert!(
            step.flip,
            "the hall is on the front, so the border must turn"
        );
        let raised = border_edges(&map, 1_usize);
        assert_eq!(raised.len(), 1_usize, "the room sees the same line");
        assert!(
            !raised.first().expect("the step edge").flip,
            "there the room already sits behind it"
        );
        assert!(
            border_edges(&map, 9_usize).is_empty(),
            "a sector nobody borders has no edges"
        );
    }

    #[test]
    fn a_border_facing_the_square_turns_around_when_dressed() {
        let mut map = two_room_map();
        let setup = teleport_specials(FlagFamily::Doom, 2_i32);
        let lines = dress_borders(&mut map, 0_usize, &setup).expect("the hall's step line");
        assert_eq!(lines, vec![2_usize], "the hall's only two-sided border");
        let line = border_line(&map, 2);
        assert_eq!(
            (line.v1, line.v2),
            (1_usize, 4_usize),
            "the line turned around with its sides"
        );
        assert_eq!(
            side_sector(&map, line.front),
            Some(1_usize),
            "the room side now faces out front"
        );
        assert_eq!(
            side_sector(&map, line.back),
            Some(0_usize),
            "and the square sits behind"
        );
        assert_eq!(line.special, 97_i32, "the WR teleport");
        assert_eq!(line.tag, 2_i32, "answering with the destination tag");
    }

    #[test]
    fn a_square_walled_in_by_solid_rock_has_nobody_to_teleport() {
        let mut lonely = one_room();
        let setup = teleport_specials(FlagFamily::Doom, 1_i32);
        let message = refuse(&dress_borders(&mut lonely, 0_usize, &setup).unwrap_err());
        assert!(
            message.contains("sector 0 has no two-sided border"),
            "{message}"
        );
    }
}
