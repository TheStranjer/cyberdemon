//! Pure window core: the window sector record and the blocking boundary
//! dressing that turns a drawn strip into a window.
//!
//! A window is a thin raised sector between two rooms: [`window_heights`]
//! computes its floor and ceiling from the rooms' heights plus the
//! explicit `sill` and `lintel` trims (the opening must stay at least
//! [`MIN_OPENING`] map units, or the call refuses before anything is
//! drawn), and [`window_record`] bakes them into the [`Sector`] for
//! [`crate::wad::map::overlay::Paint::Sector`]. Once the strip is drawn,
//! [`boundary_lines`] finds its room-facing lines (two-sided, the window
//! on exactly one side, a resolvable sector on the other) and
//! [`apply_window`] turns them into the blocking window edge: the
//! `twosided` and `blocking` bits together, so nobody walks through, the
//! room-side faces dressed (`lower` the sill texture and `upper` the
//! wall texture, which are exactly the bands the two sectors' height
//! difference makes visible) and the window-side faces cleared. Like the
//! rest of [`crate::wad`] this is pure model arithmetic and table
//! lookup: no sessions, no archives, no MCP, and — mirroring
//! [`super::doors::apply_door`] — a refused [`apply_window`] leaves the
//! map untouched.

use crate::error::CyberError;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::query::{ElementKind, check_index, missing_index};

/// The smallest window opening the builder accepts, in map units: the
/// clear glass band between the sill and the lintel.
pub const MIN_OPENING: i32 = 16_i32;

/// The explicit vertical trim of one window, in map units above and
/// below the opening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowHeights {
    /// How far the window's floor sits above the higher room floor
    /// (the sill the glass starts on).
    pub sill: i32,
    /// How far the window's ceiling sits below the lower room ceiling
    /// (the lintel it stops under).
    pub lintel: i32,
}

/// The drawn window strip that [`apply_window`] closes off: the sector
/// the strip became and the room-facing lines around it, named by index
/// into the [`Map`] rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowSides {
    /// The sector the window strip became.
    pub window_sector: usize,
    /// The two-sided lines with the window sector on one side and a
    /// resolvable room on the other: the window's view lines, every one
    /// of which [`apply_window`] blocks off.
    pub room_lines: Vec<usize>,
}

/// The textures [`apply_window`] installs, resolved by the caller (from
/// the session's theme: the `window_sill` and `wall` roles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTextures {
    /// The sill band: the `lower` texture of every room-facing side.
    pub sill: String,
    /// The wall band above the opening: the `upper` texture of every
    /// room-facing side.
    pub wall: String,
}

/// A validated room line and its two sides, resolved before anything is
/// written.
#[derive(Clone, Copy, Debug)]
struct RoomPlan {
    /// Index in [`Map::linedefs`] of the line.
    line: usize,
    /// Index in [`Map::sidedefs`] of the side facing the room.
    room_side: usize,
    /// Index in [`Map::sidedefs`] of the side facing the window.
    window_side: usize,
}

/// `InvalidArgument` for a trim that pushes a window height out of
/// the representable range.
fn overflow(axis: &str, value: i32) -> CyberError {
    CyberError::invalid(&format!(
        "the {axis} of {value} map units puts the window outside the representable \
         height range: keep sill and lintel within a few thousand map units"
    ))
}

/// The floor and ceiling of sector `index`, refused with the shared
/// index guard when the map does not hold it.
fn room_state(map: &Map, index: usize) -> Result<(i32, i32), CyberError> {
    map.sectors
        .get(index)
        .map(|row| (row.floor_height, row.ceiling_height))
        .ok_or_else(|| missing_index(ElementKind::Sectors, index, map.sectors.len()))
}

/// The window's `(floor, ceiling)` between rooms `a` and `b` under
/// `trim`: `max(floorA, floorB) + sill` and `min(ceilA, ceilB) - lintel`.
///
/// The opening (`ceiling - floor`) must keep at least [`MIN_OPENING`]
/// map units, counted even when it is negative. Both sector indices are
/// resolved against `map` first, so no arithmetic is trusted from a row
/// the map does not hold.
///
/// # Errors
///
/// [`CyberError::NotFound`] for an out-of-range sector, and
/// [`CyberError::InvalidArgument`] for a trim that overflows the height
/// range or an opening under [`MIN_OPENING`]; the opening refusal states
/// the minimum.
#[inline]
pub fn window_heights(
    map: &Map,
    a: usize,
    b: usize,
    trim: WindowHeights,
) -> Result<(i32, i32), CyberError> {
    let (floor_a, ceiling_a) = room_state(map, a)?;
    let (floor_b, ceiling_b) = room_state(map, b)?;
    let floor = floor_a
        .max(floor_b)
        .checked_add(trim.sill)
        .ok_or_else(|| overflow("sill", trim.sill))?;
    let ceiling = ceiling_a
        .min(ceiling_b)
        .checked_sub(trim.lintel)
        .ok_or_else(|| overflow("lintel", trim.lintel))?;
    let opening = ceiling.saturating_sub(floor);
    if opening < MIN_OPENING {
        return Err(CyberError::invalid(&format!(
            "the window opening between these rooms would be {opening} map units: a \
             window needs at least {MIN_OPENING} between its floor ({floor}) and its \
             ceiling ({ceiling}); lower the sill, lower the lintel, or pick taller \
             rooms"
        )));
    }
    Ok((floor, ceiling))
}

/// The window sector record between rooms `a` and `b` under `trim`:
/// room A's flats and light, the [`window_heights`] pair, no effect and
/// no tag.
///
/// # Errors
///
/// [`window_heights`]' refusals.
#[inline]
pub fn window_record(
    map: &Map,
    a: usize,
    b: usize,
    trim: WindowHeights,
) -> Result<Sector, CyberError> {
    let (floor, ceiling) = window_heights(map, a, b, trim)?;
    let mut record = map.sectors.get(a).cloned().unwrap_or_default();
    record.floor_height = floor;
    record.ceiling_height = ceiling;
    record.special = 0_i32;
    record.tag = 0_i32;
    Ok(record)
}

/// What a linedef is to the window sector `window`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Border {
    /// Two-sided with the window on exactly one side and a resolvable
    /// sector on the other: a view line [`apply_window`] blocks.
    RoomLine,
    /// Anything else: not a room-facing window boundary.
    Elsewhere,
}

/// Classify `line` against the window sector `window`.
fn window_border(map: &Map, line: &Linedef, window: usize) -> Border {
    let (front, back) = map.line_sectors(line);
    match (front == Some(window), back == Some(window)) {
        (true, false) | (false, true) if front.is_some() && back.is_some() => Border::RoomLine,
        _ => Border::Elsewhere,
    }
}

/// The [`WindowSides::room_lines`] of the drawn window sector `window`:
/// every two-sided line with the window on exactly one side and a
/// resolvable sector on the other, in ascending linedef order.
///
/// One-sided window edges (the strip's plugs) and lines the window
/// bounds on neither side never appear; dangling side or sector
/// references resolve to nothing and are skipped.
#[must_use]
#[inline]
pub fn boundary_lines(map: &Map, window: usize) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| window_border(map, line, window) == Border::RoomLine)
        .map(|(index, _line)| index)
        .collect()
}

/// Resolve one room line: two-sided, both side references live, and
/// exactly one side belongs to the window sector.
fn plan_room_line(map: &Map, line: usize, window: usize) -> Result<RoomPlan, CyberError> {
    let linedef = map.linedefs.get(line).ok_or_else(|| {
        missing_index(ElementKind::Linedefs, line, map.linedefs.len())
            .map_message(|message| format!("room {message}"))
    })?;
    let (Some(front), Some(back)) = (linedef.front, linedef.back) else {
        return Err(CyberError::invalid(&format!(
            "room linedef {line} is not two-sided: a window view line needs the window \
             sector on one side and its room on the other"
        )));
    };
    let at_window = (
        map.required_side_sector(line, front)? == window,
        map.required_side_sector(line, back)? == window,
    );
    match at_window {
        (false, true) => Ok(RoomPlan {
            line,
            room_side: front,
            window_side: back,
        }),
        (true, false) => Ok(RoomPlan {
            line,
            room_side: back,
            window_side: front,
        }),
        (true, true) | (false, false) => Err(CyberError::invalid(&format!(
            "room linedef {line} does not separate a room from window sector {window}: \
             exactly one of its sides must belong to the window"
        ))),
    }
}

/// Check every [`WindowSides`] reference against `map` before anything
/// is written.
fn plan_window(map: &Map, sides: &WindowSides) -> Result<Vec<RoomPlan>, CyberError> {
    check_index(ElementKind::Sectors, sides.window_sector, map.sectors.len())
        .map_err(|error| error.map_message(|message| format!("window {message}")))?;
    sides
        .room_lines
        .iter()
        .copied()
        .map(|line| plan_room_line(map, line, sides.window_sector))
        .collect()
}

/// Write the blocking window edge onto one planned line (the indices
/// were checked by [`plan_window`], so a row that has since vanished is
/// skipped rather than a new refusal).
fn dress_room_line(map: &mut Map, room: RoomPlan, textures: &WindowTextures) {
    if let Some(linedef) = map.linedefs.get_mut(room.line) {
        linedef.flags = linedef.flags | TWO_SIDED | BLOCKING;
    }
    if let Some(side) = map.sidedefs.get_mut(room.room_side) {
        textures.wall.clone_into(&mut side.upper);
        textures.sill.clone_into(&mut side.lower);
    }
    if let Some(side) = map.sidedefs.get_mut(room.window_side) {
        NO_TEXTURE.clone_into(&mut side.upper);
        NO_TEXTURE.clone_into(&mut side.lower);
    }
}

/// Close off a drawn window strip as one undo-free map edit.
///
/// Every line of [`WindowSides::room_lines`] becomes (or stays)
/// two-sided with the `blocking` bit set, keeping its other flag bits:
/// the two-sided bit lets both rooms see through, the blocking bit stops
/// actors from walking through wherever the engine reads it, and the
/// raised floor is what stops them in vanilla. Its room-facing side
/// wears [`WindowTextures::sill`] as the `lower` (the band from the
/// room's floor up to the window's) and [`WindowTextures::wall`] as the
/// `upper` (the band from the window's ceiling up to the room's); the
/// window-facing side's faces are cleared, because from inside the strip
/// the line is fully open.
///
/// Build the strip with [`crate::wad::map::overlay`] first and dress the
/// indices [`boundary_lines`] reports; a refused call leaves `map`
/// untouched.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `sides.window_sector` is not a sector
/// of the map or a line index is not a linedef, and
/// [`CyberError::InvalidArgument`] when a line is not two-sided, when a
/// line does not separate a room from the window, or when a side
/// reference dangles.
#[inline]
pub fn apply_window(
    map: &mut Map,
    sides: &WindowSides,
    textures: &WindowTextures,
) -> Result<(), CyberError> {
    let plan = plan_window(map, sides)?;
    for room in plan {
        dress_room_line(map, room, textures);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Border, MIN_OPENING, RoomPlan, WindowHeights, WindowSides, WindowTextures, apply_window,
        boundary_lines, dress_room_line, plan_room_line, plan_window, window_border,
        window_heights, window_record,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

    /// The default test trim: sill 32, lintel 24 (the tool defaults).
    fn trim() -> WindowHeights {
        WindowHeights {
            sill: 32_i32,
            lintel: 24_i32,
        }
    }

    /// A trim of the given `sill` and `lintel`.
    const fn sill_lintel(sill: i32, lintel: i32) -> WindowHeights {
        WindowHeights { sill, lintel }
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A sidedef over `sector` with the model's empty-face names.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: "-".to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A two-sided line from vertex 0 to vertex 1 over sides `front` and
    /// `back`.
    fn two_sided(front: usize, back: usize) -> Linedef {
        Linedef {
            v1: 0_usize,
            v2: 1_usize,
            flags: 4_u32,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A blocking one-sided line from vertex 0 to vertex 1 with `front`.
    fn one_sided(front: usize) -> Linedef {
        Linedef {
            v1: 0_usize,
            v2: 1_usize,
            flags: 1_u32,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A sector at the given heights and floor flat, effect 9 and tag 4
    /// (so the record's zeroing shows).
    fn sector(floor_height: i32, ceiling_height: i32, flat: &str) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            floor_flat: flat.to_owned(),
            ceiling_flat: "CEIL5".to_owned(),
            light: 192_i32,
            tag: 4_i32,
            special: 9_i32,
            ..Default::default()
        }
    }

    /// The windowed test map: hall sector 0 (floor 0, ceiling 128, flat
    /// `FLOOR4_8`), room sector 1 (floor 32, ceiling 128, flat
    /// `FLOOR7_2`) and the drawn window strip as sector 2 (floor 64,
    /// ceiling 104), with
    /// - linedef 0: two-sided, front (sidedef 0) hall, back (1) window;
    /// - linedef 1: two-sided, front (2) window, back (3) room (mirror);
    /// - linedef 2: one-sided, front (4) window: the strip's plug;
    /// - linedef 3: two-sided hall (5) / room (6), no window;
    /// - linedef 4: two-sided window (7) on both sides.
    ///
    /// Line 0 starts `dontpegtop`-flagged (bit 8) so the dressing's
    /// bit-keeping shows.
    fn windowed() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![vertex(0, 0), vertex(0, 128)],
            linedefs: vec![
                Linedef {
                    flags: 12_u32,
                    ..two_sided(0_usize, 1_usize)
                },
                two_sided(2_usize, 3_usize),
                one_sided(4_usize),
                two_sided(5_usize, 6_usize),
                two_sided(7_usize, 7_usize),
            ],
            sidedefs: vec![
                side(0_usize), // 0: line 0 front, hall
                side(2_usize), // 1: line 0 back, window
                side(2_usize), // 2: line 1 front, window
                side(1_usize), // 3: line 1 back, room
                side(2_usize), // 4: line 2 front, window
                side(0_usize), // 5: line 3 front, hall
                side(1_usize), // 6: line 3 back, room
                side(2_usize), // 7: line 4 both sides, window
            ],
            sectors: vec![
                sector(0_i32, 128_i32, "FLOOR4_8"),
                sector(32_i32, 128_i32, "FLOOR7_2"),
                sector(64_i32, 104_i32, "FLAT10"),
            ],
            ..Default::default()
        }
    }

    /// The textures the tests dress windows with.
    fn textures() -> WindowTextures {
        WindowTextures {
            sill: "STEP1".to_owned(),
            wall: "WALL1".to_owned(),
        }
    }

    /// The drawn strip [`apply_window`] dresses.
    fn sides() -> WindowSides {
        WindowSides {
            window_sector: 2_usize,
            room_lines: vec![0_usize, 1_usize],
        }
    }

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    fn refuse(error: &CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    /// The message of a refusal that must be a `NotFound`.
    fn missing(error: &CyberError) -> String {
        assert_eq!(code(error), "not_found", "index-refusal code");
        error.to_string()
    }

    /// `line`'s row of `map`.
    fn line_of(map: &Map, line: usize) -> &Linedef {
        map.linedefs.get(line).expect("the linedef")
    }

    #[test]
    fn heights_sit_on_the_higher_sill_under_the_lower_lintel() {
        let map = windowed();
        assert_eq!(
            window_heights(&map, 0_usize, 1_usize, trim()).expect("the default trim fits"),
            (64_i32, 104_i32),
            "max(0, 32) + 32 under min(128, 128) - 24"
        );
        assert_eq!(
            window_heights(&map, 1_usize, 0_usize, trim()).expect("either order"),
            (64_i32, 104_i32),
            "the rooms' order does not matter"
        );
        assert_eq!(
            window_heights(&map, 0_usize, 0_usize, trim()).expect("one room twice"),
            (32_i32, 104_i32),
            "a one-sided wall's window uses its room on both sides"
        );
    }

    #[test]
    fn the_record_copies_room_a_and_moves_only_the_heights() {
        let map = windowed();
        let record = window_record(&map, 1_usize, 0_usize, trim()).expect("room 1 as A");
        assert_eq!(record.floor_height, 64_i32, "the computed floor");
        assert_eq!(record.ceiling_height, 104_i32, "the computed ceiling");
        assert_eq!(record.floor_flat, "FLOOR7_2", "room A's floor flat");
        assert_eq!(record.ceiling_flat, "CEIL5", "room A's ceiling flat");
        assert_eq!(record.light, 192_i32, "room A's light");
        assert_eq!(
            (record.special, record.tag),
            (0_i32, 0_i32),
            "inert and untagged whatever room A carried"
        );
        let from_hall = window_record(&map, 0_usize, 1_usize, trim()).expect("hall as A");
        assert_eq!(
            from_hall.floor_flat, "FLOOR4_8",
            "the chosen side lends its flats"
        );
        assert_eq!(
            from_hall.floor_height, 64_i32,
            "the heights do not depend on the side"
        );
    }

    #[test]
    fn the_min_opening_passes_and_one_unit_less_refuses() {
        let map = windowed();
        assert_eq!(
            window_heights(&map, 0_usize, 1_usize, sill_lintel(56_i32, 24_i32))
                .expect("exactly the minimum"),
            (88_i32, 104_i32),
            "an opening of {MIN_OPENING} is allowed"
        );
        let message = refuse(
            &window_heights(&map, 0_usize, 1_usize, sill_lintel(57_i32, 24_i32)).unwrap_err(),
        );
        assert!(message.contains("at least 16"), "{message}");
        assert!(
            message.contains("15 map units"),
            "the opening is stated: {message}"
        );
    }

    #[test]
    fn a_negative_opening_is_refused_with_its_number() {
        let map = windowed();
        let message = refuse(
            &window_heights(&map, 0_usize, 1_usize, sill_lintel(90_i32, 24_i32)).unwrap_err(),
        );
        assert!(message.contains("-18 map units"), "{message}");
        assert!(message.contains("at least 16"), "{message}");
        assert!(message.contains("lower the sill"), "{message}");
    }

    #[test]
    fn heights_refuse_bad_sectors_and_overflowing_trims() {
        let map = windowed();
        let gone = missing(&window_heights(&map, 9_usize, 0_usize, trim()).unwrap_err());
        assert!(
            gone.contains(
                "sector 9 does not exist (the map has 3 sectors, valid indices are \
                            0..=2)"
            ),
            "{gone}"
        );
        let sill = refuse(
            &window_heights(&map, 0_usize, 1_usize, sill_lintel(i32::MAX, 0_i32)).unwrap_err(),
        );
        assert!(sill.contains("sill"), "{sill}");
        let lintel = refuse(
            &window_heights(&map, 0_usize, 1_usize, sill_lintel(0_i32, i32::MIN)).unwrap_err(),
        );
        assert!(lintel.contains("lintel"), "{lintel}");
        assert_eq!(
            code(&window_record(&map, 9_usize, 0_usize, trim()).unwrap_err()),
            "not_found",
            "the record checks through window_heights"
        );
    }

    #[test]
    fn boundary_lines_finds_the_two_sided_window_edges_only() {
        let map = windowed();
        assert_eq!(
            boundary_lines(&map, 2_usize),
            vec![0_usize, 1_usize],
            "the one-sided plug, the window-free line and the both-window line stay out"
        );
        assert_eq!(
            boundary_lines(&map, 9_usize),
            Vec::<usize>::new(),
            "a sector the map does not hold owns no window edges"
        );
        let empty = Map::default();
        assert!(
            boundary_lines(&empty, 0_usize).is_empty(),
            "an empty map has no edges"
        );
    }

    #[test]
    fn border_classification_covers_every_shape() {
        let map = windowed();
        for index in [0_usize, 1_usize] {
            assert_eq!(
                window_border(&map, line_of(&map, index), 2_usize),
                Border::RoomLine,
                "line {index} faces the window from a room"
            );
        }
        for index in [2_usize, 3_usize, 4_usize] {
            assert_eq!(
                window_border(&map, line_of(&map, index), 2_usize),
                Border::Elsewhere,
                "line {index} is not a view line"
            );
        }
    }

    #[test]
    fn apply_window_blocks_both_faces_and_keeps_other_bits() {
        let mut map = windowed();
        apply_window(&mut map, &sides(), &textures()).expect("a drawn strip dresses");
        let flagged = line_of(&map, 0);
        assert_eq!(
            flagged.flags, 13_u32,
            "blocking joins the two-sided line's bits, dontpegtop survives"
        );
        let mirrored = line_of(&map, 1);
        assert_eq!(mirrored.flags, 5_u32, "twosided and blocking");
        let hall_side = map.sidedefs.first().expect("hall side");
        assert_eq!(hall_side.lower, "STEP1", "the sill band under the opening");
        assert_eq!(hall_side.upper, "WALL1", "the wall band over it");
        let window_side = map.sidedefs.get(1).expect("window side");
        assert_eq!(
            window_side.upper, "-",
            "from inside the window the line is open"
        );
        assert_eq!(window_side.lower, "-", "and stays open below");
        let room_side = map.sidedefs.get(3).expect("room side of line 1");
        assert_eq!(
            room_side.upper, "WALL1",
            "the dressing follows the room side, whichever slot it occupies"
        );
        assert_eq!(room_side.lower, "STEP1", "and both faces move together");
        assert_eq!(
            line_of(&map, 2).flags,
            1_u32,
            "the one-sided plug is left alone"
        );
        assert_eq!(map.sectors, windowed().sectors, "no sector row moves");
    }

    #[test]
    fn dress_room_line_skips_rows_the_map_lost() {
        let mut map = windowed();
        let gone = RoomPlan {
            line: 9_usize,
            room_side: 9_usize,
            window_side: 9_usize,
        };
        dress_room_line(&mut map, gone, &textures());
        assert_eq!(
            map,
            windowed(),
            "a vanished row writes nothing (plan_window refuses before this point)"
        );
    }

    #[test]
    fn apply_window_refuses_shapes_that_are_not_view_lines() {
        let mut base = windowed();
        let mut plug = sides();
        plug.room_lines = vec![2_usize];
        let plug_message = refuse(&apply_window(&mut base, &plug, &textures()).unwrap_err());
        assert!(
            plug_message.contains("room linedef 2 is not two-sided"),
            "{plug_message}"
        );

        let mut no_window = sides();
        no_window.room_lines = vec![3_usize];
        let plain_message = refuse(&apply_window(&mut base, &no_window, &textures()).unwrap_err());
        assert!(
            plain_message.contains("does not separate"),
            "{plain_message}"
        );

        let mut both_window = sides();
        both_window.room_lines = vec![4_usize];
        let doubled_message =
            refuse(&apply_window(&mut base, &both_window, &textures()).unwrap_err());
        assert!(
            doubled_message.contains("does not separate"),
            "{doubled_message}"
        );

        let mut sectorless = sides();
        sectorless.window_sector = 9_usize;
        let sector_message =
            missing(&apply_window(&mut base, &sectorless, &textures()).unwrap_err());
        assert!(
            sector_message.contains("window sector 9 does not exist"),
            "{sector_message}"
        );

        let mut linless = sides();
        linless.room_lines = vec![77_usize];
        let line_message = missing(&apply_window(&mut base, &linless, &textures()).unwrap_err());
        assert!(
            line_message.contains("room linedef 77 does not exist"),
            "{line_message}"
        );
        assert_eq!(base, windowed(), "every refusal left the one map alone");
    }

    #[test]
    fn refused_calls_leave_the_map_untouched() {
        let mut map = windowed();
        let before = map.clone();
        let mut broken = sides();
        broken.room_lines = vec![0_usize, 2_usize];
        let error = apply_window(&mut map, &broken, &textures()).expect_err("the plug refuses");
        assert!(refuse(&error).contains("not two-sided"), "{error}");
        assert_eq!(
            map, before,
            "every index is checked before any row is written"
        );
    }

    #[test]
    fn plans_refuse_dangling_sides_and_the_wrong_window() {
        let mut map = windowed();
        map.linedefs.get_mut(0).expect("line 0").back = Some(99_usize);
        let message = refuse(&plan_room_line(&map, 0_usize, 2_usize).unwrap_err());
        assert!(message.contains("sidedef 99"), "{message}");
        let listed = refuse(&plan_window(&map, &sides()).unwrap_err());
        assert!(listed.contains("sidedef 99"), "{listed}");
        let other = refuse(&plan_room_line(&map, 0_usize, 0_usize).unwrap_err());
        assert!(
            other.contains("sidedef 99"),
            "the dangling side refuses whatever the window is"
        );
    }
}
