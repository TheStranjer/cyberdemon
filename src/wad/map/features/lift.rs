//! Pure lift core: the platform's activation edges and the down-wait-up
//! action each family runs on them.
//!
//! A lift is a sector standing at a high floor whose edges look down on
//! lower ground. [`build_lift`] takes a [`LiftSpec`] naming the platform
//! — an existing sector ([`LiftPlatform::Existing`]) or a polygon folded
//! over the map with [`apply_overlay`] ([`LiftPlatform::Draw`], whose
//! floor, ceiling, flats and light come from the sector under its
//! centre) — resolves the floor it lowers to (the explicit
//! [`LiftSpec::low_floor`], else the lowest neighbouring floor found
//! through [`neighbors`]), stamps the platform with a fresh
//! [`next_free_tag`], selects every boundary edge that faces a lower
//! sector and turns each one so its front looks down at the player who
//! triggers the ride, writes the [`lift_specials`] action onto it and
//! dresses the low-facing side's `lower` band with the caller's
//! [`LiftSpec::side`] texture. A drop under [`MIN_LIFT_DROP`] succeeds
//! with the note "a lift is not needed; stairs or a step will do": that
//! taste stays in the report, not in a refusal. Like the rest of
//! [`crate::wad`] this is pure model arithmetic and table lookup — no
//! sessions, no archives, no `MCP` — and every refusal on an existing
//! platform lands before a single row is written.

use serde::Serialize;

use crate::error::CyberError;
use crate::geom::Pt;
use crate::geom::segment::round_quotient;
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily};
use crate::wad::map::geometry::{neighbors, point_in_sector};
use crate::wad::map::model::{Linedef, Map, Sector};
use crate::wad::map::overlay::{Outside, OverlayRequest, Paint, WallTextures, apply_overlay};

use super::doors::{DoorLineSetup, USE, USE_REPEAT, next_free_tag};

/// The `blocking` linedef flag bit, cleared on the activation lines so
/// the player can reach (and walk across) the lowered platform.
const BLOCKING_BIT: u32 = 1_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
///
/// [`Sidedef`]: crate::wad::map::Sidedef
const NO_TEXTURE: &str = "-";
/// The `Hexen` `repeatspecial` bit, added on top of either the
/// `playeruse` or `playercross` activation for a repeating lift line.
const REPEAT_SPECIAL_BIT: u32 = 0x200;
/// The `Hexen` trigger bits [`dress_edge`] replaces wholesale: the
/// 3-bit activation field plus the `repeatspecial` bit, so a one-shot
/// clears both and a repeating line gets exactly what [`lift_specials`]
/// names.
const TRIGGER_MASK: u32 = ACTIVATION_MASK | REPEAT_SPECIAL_BIT;
/// The `Hexen` `playercross` activation: the activation field's zero
/// value, crossed rather than used.
const CROSS: u32 = 0_u32;
/// The `Hexen` lift line special: `Plat_DownWaitUpStayLip`.
const HEXEN_PLAT_SPECIAL: i32 = 206;
/// The `Hexen` lift's speed argument, in map units per game tic.
const PLAT_SPEED: i32 = 32;
/// The `Hexen` lift's wait-at-bottom argument, in game tics.
const PLAT_DELAY: i32 = 105;
/// The smallest drop worth a lift: below this the build succeeds but
/// notes that stairs or a step will do.
pub const MIN_LIFT_DROP: i32 = 64;
/// The fewest corners a drawn platform polygon may carry.
const MIN_PLATFORM_CORNERS: usize = 3;

/// How the player starts the lift ride from below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftStyle {
    /// The player presses the platform's side wall from the lower area.
    UseWall,
    /// Crossing a boundary edge of the platform starts the ride.
    WalkTrigger,
}

impl LiftStyle {
    /// Every style, in table order.
    #[must_use]
    #[inline]
    pub const fn all() -> [Self; 2] {
        [Self::UseWall, Self::WalkTrigger]
    }
}

/// The [`DoorLineSetup`] a lift of `style` installs in flag family
/// `family`, triggered `repeatable` times or once, answering with the
/// platform's `tag`.
///
/// The table:
///
/// | Style | `Doom` once | `Doom` repeated | `Hexen` |
/// |---|---|---|---|
/// | [`UseWall`] | `21` (`S1` floor to lowest adjacent) | `62` (`SR`) | `206` `Plat_DownWaitUpStayLip`, args `[tag, 32, 105, 0]`, `playeruse` |
/// | [`WalkTrigger`] | `10` (`W1`) | `88` (`WR`) | the same special, `playercross` |
///
/// `Doom` setups carry `tag` in [`DoorLineSetup::tag`] (the platform
/// sector answers with the same one); their args and activation stay
/// zero. `Hexen` setups carry no line tag at all: the tag rides in
/// `args[0]` with the speed in `args[1]` and the bottom wait in
/// `args[2]`, and [`DoorLineSetup::flags_activation`] holds `playeruse`
/// for [`UseWall`] or the field's zero value (`playercross`) for
/// [`WalkTrigger`], plus `repeatspecial` when `repeatable`.
///
/// [`UseWall`]: LiftStyle::UseWall
/// [`WalkTrigger`]: LiftStyle::WalkTrigger
#[must_use]
#[inline]
pub const fn lift_specials(
    family: FlagFamily,
    style: LiftStyle,
    repeatable: bool,
    tag: i32,
) -> DoorLineSetup {
    match family {
        FlagFamily::Doom => doom_lift(style, repeatable, tag),
        FlagFamily::Hexen => hexen_lift(style, repeatable, tag),
    }
}

/// The `Doom`-family row of [`lift_specials`]: the vanilla down-wait-up
/// special (the `S1`/`W1` once form or its `SR`/`WR` repeat), the
/// platform's tag on the line, no arguments and no activation bits.
const fn doom_lift(style: LiftStyle, repeatable: bool, tag: i32) -> DoorLineSetup {
    let (once, again) = match style {
        LiftStyle::UseWall => (21_i32, 62_i32),
        LiftStyle::WalkTrigger => (10_i32, 88_i32),
    };
    DoorLineSetup {
        special: if repeatable { again } else { once },
        tag,
        args: [0_i32; 5],
        flags_activation: 0_u32,
    }
}

/// The `Hexen`-family row of [`lift_specials`]: `Plat_DownWaitUpStayLip`
/// targeting the tagged sector through `args[0]`, `use` or `cross`
/// activation and no line tag.
const fn hexen_lift(style: LiftStyle, repeatable: bool, tag: i32) -> DoorLineSetup {
    let activation = match (style, repeatable) {
        (LiftStyle::UseWall, true) => USE_REPEAT,
        (LiftStyle::UseWall, false) => USE,
        (LiftStyle::WalkTrigger, true) => CROSS | REPEAT_SPECIAL_BIT,
        (LiftStyle::WalkTrigger, false) => CROSS,
    };
    DoorLineSetup {
        special: HEXEN_PLAT_SPECIAL,
        tag: 0_i32,
        args: [tag, PLAT_SPEED, PLAT_DELAY, 0_i32, 0_i32],
        flags_activation: activation,
    }
}

/// The platform the parameters name: an existing sector or a polygon to
/// draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiftPlatform {
    /// The index in [`Map::sectors`] of the existing platform sector.
    Existing(usize),
    /// The corners of a new platform's polygon, in order, implicitly
    /// closed; it takes its heights from the sector under its centre.
    Draw(Vec<Pt>),
}

/// Everything one lift needs, as the tool layer resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftSpec {
    /// The platform: an existing sector or the polygon to draw.
    pub platform: LiftPlatform,
    /// The floor the platform lowers to; the lowest neighbouring floor
    /// by default.
    pub low_floor: Option<i32>,
    /// How the ride is triggered from below.
    pub style: LiftStyle,
    /// Whether the activation lines keep working after use.
    pub repeatable: bool,
    /// The lift's side texture, worn by every activation edge's
    /// low-facing `lower` band.
    pub side: String,
    /// The platform's floor flat (`"-"` keeps the sector's own).
    pub floor_flat: String,
}

/// What one [`build_lift`] call did, as the tool reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiftBuild {
    /// The platform sector's index in [`Map::sectors`] (new when the
    /// platform was drawn).
    pub platform_sector: usize,
    /// The fresh tag stamped onto the sector and answered by the lines.
    pub tag: i32,
    /// The indices of the activation lines, ascending.
    pub lines: Vec<usize>,
    /// The platform's high floor.
    pub floor_height: i32,
    /// The floor the platform lowers to.
    pub low_floor: i32,
    /// `floor_height - low_floor`, the ride's length.
    pub drop: i32,
    /// What the drawing reported, plus the small-drop note when
    /// `drop < MIN_LIFT_DROP`.
    pub notes: Vec<String>,
}

/// One resolved side of a line: its sidedef, sector and floor height.
#[derive(Clone, Copy, Debug)]
struct Side {
    /// Index in [`Map::sidedefs`].
    sidedef: usize,
    /// Index in [`Map::sectors`] the side opens into.
    sector: usize,
    /// That sector's floor height.
    floor: i32,
}

/// An activation edge: the line and the side that must face the lower
/// area once the ride is installed.
#[derive(Clone, Copy, Debug)]
struct Edge {
    /// Index in [`Map::linedefs`] of the line.
    line: usize,
    /// Index in [`Map::sidedefs`] of the side opening into the lower
    /// sector (the lift's face, wearing the side texture below).
    low_side: usize,
    /// Whether that side is currently the back, so the line turns
    /// around with it.
    flip: bool,
}

/// The platform `build_lift` matches lines against.
#[derive(Clone, Copy, Debug)]
struct EdgeProbe {
    /// The platform sector's index.
    platform: usize,
    /// The platform's floor height: only edges facing below it activate.
    floor: i32,
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// `InvalidArgument` naming the reference `kind`, its out-of-range
/// `index` and how many the map holds.
fn range_error(kind: &str, index: usize, len: usize) -> CyberError {
    invalid(&format!(
        "{kind} {index} is out of range: the map holds {len} of them; re-read the map \
         and pass current indices"
    ))
}

/// The refusal for a platform with nothing beside it to lower towards.
fn isolated(sector: usize) -> CyberError {
    invalid(&format!(
        "sector {sector} has no neighbouring sector to take a low floor from: pass an \
         explicit low_floor or draw the platform beside a lower area"
    ))
}

/// The refusal for a platform that is not the high floor of its drop.
fn no_way_down(sector: usize, floor: i32, low_floor: i32) -> CyberError {
    invalid(&format!(
        "sector {sector} stands at floor {floor}, not above its lift floor {low_floor}: \
         the platform is the high floor that comes down; pick the higher sector or pass \
         a lower low_floor"
    ))
}

/// The refusal for a platform with no wall the lower area can trigger.
fn no_edges(sector: usize) -> CyberError {
    invalid(&format!(
        "no edge of sector {sector} faces a lower sector: the activation lines are the \
         platform's walls a player reaches from below; draw the polygon so its centre \
         stands on the high side of the height change, or pick the sector above its \
         neighbours"
    ))
}

/// The refusal for a platform polygon whose centre is in the void.
fn centre_lost(centre: Pt) -> CyberError {
    invalid(&format!(
        "the platform's centre {}, {} lies in no sector: draw the polygon across the \
         map so its centre sits on the floor the platform starts at",
        centre.0, centre.1
    ))
}

/// The small-drop note: legal, but a lift wasted on it.
fn shrink_note(drop: i32) -> String {
    format!(
        "this lift would drop only {drop} map units: a lift is not needed; stairs or a \
         step will do (below {MIN_LIFT_DROP} the guide says prefer add_stairs)"
    )
}

/// The server-bug refusal for a platform paint that reported no sector.
fn paint_lost() -> CyberError {
    CyberError::Internal("the lift platform paint reported no sector (server bug)".to_owned())
}

/// The mean of one axis of the polygon's corners, rounded to the
/// nearest whole unit and saturated into the grid.
fn mean_axis(sum: i128, count: i128) -> i64 {
    let exact = round_quotient(sum, count).unwrap_or(sum);
    match i64::try_from(exact) {
        Ok(axis) => axis,
        Err(_error) if exact < 0_i128 => i64::MIN,
        Err(_error) => i64::MAX,
    }
}

/// The corner-average centre of a platform polygon.
fn polygon_centre(polygon: &[Pt]) -> Pt {
    let mut sum_x = 0_i128;
    let mut sum_y = 0_i128;
    for &(x, y) in polygon {
        sum_x = sum_x.saturating_add(i128::from(x));
        sum_y = sum_y.saturating_add(i128::from(y));
    }
    let count = i128::try_from(polygon.len()).unwrap_or(1);
    (mean_axis(sum_x, count), mean_axis(sum_y, count))
}

/// The sector record for a drawn platform: the base's heights, flats and
/// light, inert and untagged (the tag is stamped after the paint).
fn platform_record(map: &Map, base: usize) -> Sector {
    let mut record = map.sectors.get(base).cloned().unwrap_or_default();
    record.special = 0_i32;
    record.tag = 0_i32;
    record
}

/// The overlay request painting the platform, its new walls in `side`.
fn platform_request(map: &Map, polygon: &[Pt], base: usize, side: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: polygon.to_vec(),
        paint: Paint::Sector(platform_record(map, base)),
        keep_interior: false,
        textures: WallTextures {
            wall: side.to_owned(),
            upper: side.to_owned(),
            lower: side.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// Draw a new platform: the `polygon` folded over the map as a sector
/// copying the heights of the sector under its centre.
///
/// Returns the new sector's index and the paint's notes.
fn draw_platform(
    map: &mut Map,
    polygon: &[Pt],
    side: &str,
) -> Result<(usize, Vec<String>), CyberError> {
    if polygon.len() < MIN_PLATFORM_CORNERS {
        return Err(invalid(
            "platform.points needs at least three corners: pass the ordered outline of \
             the new platform (its last corner closes it)",
        ));
    }
    let centre = polygon_centre(polygon);
    let base = point_in_sector(map, centre.0, centre.1).ok_or_else(|| centre_lost(centre))?;
    let drawn = apply_overlay(map, &platform_request(map, polygon, base, side))?;
    let sector = drawn.sector.ok_or_else(paint_lost)?;
    Ok((sector, drawn.notes))
}

/// The floor the platform lowers to: the explicit `given` one, else the
/// lowest neighbouring sector's floor.
fn resolve_low_floor(map: &Map, sector: usize, given: Option<i32>) -> Result<i32, CyberError> {
    given.map_or_else(|| lowest_neighbour(map, sector), Ok)
}

/// The lowest floor among `sector`'s neighbouring sectors.
fn lowest_neighbour(map: &Map, sector: usize) -> Result<i32, CyberError> {
    neighbors(map, sector)
        .into_iter()
        .filter_map(|index| map.sectors.get(index).map(|row| row.floor_height))
        .min()
        .ok_or_else(|| isolated(sector))
}

/// The resolved side of a line, `None` when the sidedef or its sector
/// does not resolve.
fn resolve_side(map: &Map, side: Option<usize>) -> Option<Side> {
    let sidedef_index = side?;
    let sidedef = map.sidedefs.get(sidedef_index)?;
    let sector = sidedef.sector;
    let row = map.sectors.get(sector)?;
    Some(Side {
        sidedef: sidedef_index,
        sector,
        floor: row.floor_height,
    })
}

/// Whether `line` is a platform edge facing a lower sector, and which of
/// its sides must end up front.
fn edge_against_low(map: &Map, line: &Linedef, index: usize, probe: EdgeProbe) -> Option<Edge> {
    let front = resolve_side(map, line.front)?;
    let back = resolve_side(map, line.back)?;
    match (
        front.sector == probe.platform,
        back.sector == probe.platform,
    ) {
        (true, false) if back.floor < probe.floor => Some(Edge {
            line: index,
            low_side: back.sidedef,
            flip: true,
        }),
        (false, true) if front.floor < probe.floor => Some(Edge {
            line: index,
            low_side: front.sidedef,
            flip: false,
        }),
        _ => None,
    }
}

/// The platform's activation edges, in linedef order.
fn activation_edges(map: &Map, platform: usize, floor: i32) -> Vec<Edge> {
    let probe = EdgeProbe { platform, floor };
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| edge_against_low(map, line, index, probe))
        .collect()
}

/// Write the platform's own row: the fresh tag and the lift floor flat
/// (kept when the caller named no texture).
fn stamp_platform(map: &mut Map, sector: usize, tag: i32, floor_flat: &str) {
    if let Some(row) = map.sectors.get_mut(sector) {
        row.tag = tag;
        if floor_flat != NO_TEXTURE {
            floor_flat.clone_into(&mut row.floor_flat);
        }
    }
}

/// Dress one activation edge: turn it so its front faces the lower
/// area, write the family's lift action on it (the `Hexen` family
/// replaces the whole trigger — activation field and `repeatspecial` —
/// the `Doom` family keeps its flags except the blocking bit), and hang
/// the side texture below the platform on the low-facing side.
fn dress_edge(map: &mut Map, edge: &Edge, setup: &DoorLineSetup, side: &str) {
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
    if let Some(worn) = map.sidedefs.get_mut(edge.low_side) {
        side.clone_into(&mut worn.lower);
    }
}

/// Dress every activation edge and report the line indices, ascending.
fn dress_edges(map: &mut Map, edges: &[Edge], setup: &DoorLineSetup, side: &str) -> Vec<usize> {
    let mut lines: Vec<usize> = edges.iter().map(|edge| edge.line).collect();
    lines.sort_unstable();
    for edge in edges {
        dress_edge(map, edge, setup, side);
    }
    lines
}

/// Build a working lift platform as one map edit.
///
/// The platform is either the named existing sector or the polygon
/// painted by [`LiftPlatform::Draw`]; it lowers from its floor to
/// [`LiftSpec::low_floor`] (default: the lowest neighbouring floor), so
/// it must stand above that floor and at least one of its two-sided
/// boundary edges must face a sector below it — those edges carry the
/// [`lift_specials`] action, front-turned at the lower area. The sector
/// is stamped with a fresh [`next_free_tag`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an out-of-range platform sector,
/// a polygon with fewer than three corners or a
/// centre in no sector, a platform with no neighbour and no explicit
/// `low_floor`, a low floor not below the platform's, or a platform no
/// edge of which faces a lower sector; [`CyberError::Internal`] when a
/// platform paint reports no sector. A refusal on an existing platform
/// writes nothing.
///
/// # Panics
///
/// Never: every map access is checked.
#[inline]
pub fn build_lift(map: &mut Map, spec: &LiftSpec) -> Result<LiftBuild, CyberError> {
    let (platform_sector, mut notes) = match spec.platform.clone() {
        LiftPlatform::Existing(index) => {
            if map.sectors.get(index).is_none() {
                return Err(range_error("sector", index, map.sectors.len()));
            }
            (index, Vec::new())
        }
        LiftPlatform::Draw(polygon) => draw_platform(map, &polygon, &spec.side)?,
    };
    let floor = map
        .sectors
        .get(platform_sector)
        .map(|row| row.floor_height)
        .ok_or_else(|| {
            CyberError::Internal(format!(
                "the lift platform sector {platform_sector} vanished while the lift was \
                 built (server bug)"
            ))
        })?;
    let low_floor = resolve_low_floor(map, platform_sector, spec.low_floor)?;
    if low_floor >= floor {
        return Err(no_way_down(platform_sector, floor, low_floor));
    }
    let edges = activation_edges(map, platform_sector, floor);
    if edges.is_empty() {
        return Err(no_edges(platform_sector));
    }
    let tag = next_free_tag(map);
    stamp_platform(map, platform_sector, tag, &spec.floor_flat);
    let setup = lift_specials(map.flag_family(), spec.style, spec.repeatable, tag);
    let lines = dress_edges(map, &edges, &setup, &spec.side);
    let drop = floor.saturating_sub(low_floor);
    if drop < MIN_LIFT_DROP {
        notes.push(shrink_note(drop));
    }
    Ok(LiftBuild {
        platform_sector,
        tag,
        lines,
        floor_height: floor,
        low_floor,
        drop,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LiftPlatform, LiftSpec, LiftStyle, MIN_LIFT_DROP, activation_edges, build_lift,
        lift_specials, polygon_centre,
    };
    use crate::error::CyberError;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, two_room_map, vertex};
    use crate::wad::map::features::DoorLineSetup;
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::{Map, MapFormat};

    /// The tag the table tests ask `lift_specials` for.
    const SAMPLE_TAG: i32 = 42;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    fn refuse(error: &CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    /// A sidedef over `sector` wearing the fixture textures.
    fn sector_side(sector: usize) -> crate::wad::map::Sidedef {
        side("-", "-", "WALL1", sector)
    }

    /// A single-room square: no neighbour on any side.
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
                sector_side(0),
                sector_side(0),
                sector_side(0),
                sector_side(0),
            ],
            sectors: vec![room(0, 128, 160, 0)],
            ..Default::default()
        }
    }

    /// A use-wall, repeatable lift spec over `platform`.
    fn spec(platform: LiftPlatform) -> LiftSpec {
        LiftSpec {
            platform,
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable: true,
            side: "LIFTSIDE".to_owned(),
            floor_flat: "FLAT10".to_owned(),
        }
    }

    /// The sector a sidedef slot opens into.
    fn side_sector(map: &Map, slot: Option<usize>) -> Option<usize> {
        let index = slot?;
        map.sidedefs.get(index).map(|sidedef| sidedef.sector)
    }

    #[test]
    fn lift_specials_matches_the_doom_table() {
        let expected = [
            (LiftStyle::UseWall, false, 21_i32),
            (LiftStyle::UseWall, true, 62_i32),
            (LiftStyle::WalkTrigger, false, 10_i32),
            (LiftStyle::WalkTrigger, true, 88_i32),
        ];
        assert_eq!(
            expected.len(),
            LiftStyle::all().len() * 2_usize,
            "the table covers every style and repeat"
        );
        for (style, repeatable, special) in expected {
            assert_eq!(
                lift_specials(FlagFamily::Doom, style, repeatable, SAMPLE_TAG),
                DoorLineSetup {
                    special,
                    tag: SAMPLE_TAG,
                    args: [0_i32; 5],
                    flags_activation: 0_u32,
                },
                "the Doom row of {style:?} repeatable={repeatable}"
            );
        }
    }

    #[test]
    fn lift_specials_matches_the_hexen_table() {
        let args = [SAMPLE_TAG, 32_i32, 105_i32, 0_i32, 0_i32];
        let expected = [
            (LiftStyle::UseWall, false, 0x400_u32),
            (LiftStyle::UseWall, true, 0x600_u32),
            (LiftStyle::WalkTrigger, false, 0_u32),
            (LiftStyle::WalkTrigger, true, 0x200_u32),
        ];
        for (style, repeatable, activation) in expected {
            assert_eq!(
                lift_specials(FlagFamily::Hexen, style, repeatable, SAMPLE_TAG),
                DoorLineSetup {
                    special: 206_i32,
                    tag: 0_i32,
                    args,
                    flags_activation: activation,
                },
                "the Hexen row of {style:?} repeatable={repeatable}"
            );
        }
    }

    #[test]
    fn the_fixture_step_becomes_a_repeating_use_lift() {
        let mut map = two_room_map();
        let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
        assert_eq!(
            build.platform_sector, 1_usize,
            "the raised room is the platform"
        );
        assert_eq!(build.tag, 2_i32, "the fresh tag");
        assert_eq!(build.lines, vec![2_usize], "the step line only");
        assert_eq!(
            (build.floor_height, build.low_floor, build.drop),
            (32_i32, 0_i32, 32_i32),
            "a 32-unit ride"
        );
        assert!(
            build
                .notes
                .iter()
                .any(|note| note.contains("a lift is not needed; stairs or a step will do")),
            "the small-drop note: {:?}",
            build.notes
        );
        let platform = map.sectors.get(1).expect("the platform");
        assert_eq!(platform.tag, 2_i32, "the fresh tag is stamped");
        assert_eq!(platform.floor_flat, "FLAT10", "the theme's lift floor");
        let line = map.linedefs.get(2).expect("the step line");
        assert_eq!(line.special, 62_i32, "the SR lift");
        assert_eq!(line.tag, 2_i32, "the line answers with the platform tag");
        assert_eq!(line.args, [0_i32; 5], "Doom lines carry no args");
        assert_eq!(line.flags, 4_u32, "two-sided and reachable");
        assert_eq!(
            side_sector(&map, line.front),
            Some(0_usize),
            "the front already faced the lower hall"
        );
        assert_eq!(
            map.sidedefs.get(2).expect("the low side").lower,
            "LIFTSIDE",
            "the platform's face below the ledge"
        );
    }

    #[test]
    fn the_other_doom_rows_land_on_their_specials() {
        for (style, repeatable, special) in [
            (LiftStyle::UseWall, false, 21_i32),
            (LiftStyle::WalkTrigger, false, 10_i32),
            (LiftStyle::WalkTrigger, true, 88_i32),
        ] {
            let mut map = two_room_map();
            let wanted = LiftSpec {
                style,
                repeatable,
                ..spec(LiftPlatform::Existing(1_usize))
            };
            let build = build_lift(&mut map, &wanted).expect("a Doom lift");
            assert_eq!(
                map.linedefs.get(2).expect("the step line").special,
                special,
                "{style:?} repeatable={repeatable}"
            );
            assert_eq!(build.tag, 2_i32, "the same fresh tag every time");
        }
    }

    #[test]
    fn a_reversed_edge_turns_to_face_the_lower_sector() {
        let mut map = two_room_map();
        if let Some(line) = map.linedefs.get_mut(2) {
            line.v1 = 1_usize;
            line.v2 = 4_usize;
            line.front = Some(3_usize);
            line.back = Some(2_usize);
        }
        let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
        assert_eq!(build.lines, vec![2_usize]);
        let line = map.linedefs.get(2).expect("the step line");
        assert_eq!(line.v1, 4_usize, "the line turned around with its sides");
        assert_eq!(line.v2, 1_usize);
        assert_eq!(
            side_sector(&map, line.front),
            Some(0_usize),
            "the front now faces the lower hall"
        );
        assert_eq!(side_sector(&map, line.back), Some(1_usize));
        assert_eq!(line.special, 62_i32);
        assert_eq!(map.sidedefs.get(2).expect("the low side").lower, "LIFTSIDE");
    }

    #[test]
    fn a_hexen_platform_runs_the_scripted_lip_plat() {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        let build = build_lift(&mut map, &spec(LiftPlatform::Existing(1_usize))).expect("a lift");
        assert_eq!(build.tag, 2_i32, "the minted tag reaches args[0]");
        let line = map.linedefs.get(2).expect("the step line");
        assert_eq!(line.special, 206_i32, "Plat_DownWaitUpStayLip");
        assert_eq!(
            line.args,
            [2_i32, 32_i32, 105_i32, 0_i32, 0_i32],
            "tag, speed, wait"
        );
        assert_eq!(line.tag, 0_i32, "a Hexen line carries no tag");
        assert_eq!(
            line.flags,
            4_u32 | 0x600_u32,
            "playeruse plus repeatspecial, two-sided"
        );
    }

    #[test]
    fn a_hexen_walk_trigger_clears_the_activation_field() {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        if let Some(line) = map.linedefs.get_mut(2) {
            line.flags = 4_u32 | 0x1000_u32;
        }
        let wanted = LiftSpec {
            style: LiftStyle::WalkTrigger,
            repeatable: false,
            ..spec(LiftPlatform::Existing(1_usize))
        };
        build_lift(&mut map, &wanted).expect("a cross lift");
        let line = map.linedefs.get(2).expect("the step line");
        assert_eq!(
            line.flags, 4_u32,
            "the playerpush field clears to playercross (its zero value), twosided stays"
        );
        assert_eq!(line.special, 206_i32);
        let repeating = LiftSpec {
            repeatable: true,
            ..wanted
        };
        build_lift(&mut map, &repeating).expect("a repeating cross lift");
        assert_eq!(
            map.linedefs.get(2).expect("the step line").flags,
            4_u32 | 0x200_u32,
            "repeatable adds repeatspecial on top"
        );
    }

    #[test]
    fn the_explicit_low_floor_moves_the_drop_and_the_warning() {
        for (low_floor, warns) in [(-32_i32, false), (-31_i32, true)] {
            let mut map = two_room_map();
            let wanted = LiftSpec {
                low_floor: Some(low_floor),
                ..spec(LiftPlatform::Existing(1_usize))
            };
            let build = build_lift(&mut map, &wanted).expect("an explicit ride");
            assert_eq!(
                (build.low_floor, build.drop),
                (low_floor, 32_i32 - low_floor)
            );
            let small = build
                .notes
                .iter()
                .any(|note| note.contains("a lift is not needed"));
            assert_eq!(small, warns, "the warning at drop {}", 32_i32 - low_floor);
        }
        assert_eq!(MIN_LIFT_DROP, 64_i32, "the guide's threshold");
    }

    #[test]
    fn a_platform_that_is_not_the_high_floor_refuses() {
        let mut map = two_room_map();
        let before = map.clone();
        let message =
            refuse(&build_lift(&mut map, &spec(LiftPlatform::Existing(0_usize))).unwrap_err());
        assert!(message.contains("sector 0 stands at floor 0"), "{message}");
        assert!(message.contains("not above its lift floor 32"), "{message}");
        assert_eq!(map, before, "the refusal wrote nothing");
    }

    #[test]
    fn an_isolated_platform_names_its_missing_neighbour() {
        let mut lonely = one_room();
        let before = lonely.clone();
        let missing =
            refuse(&build_lift(&mut lonely, &spec(LiftPlatform::Existing(0_usize))).unwrap_err());
        assert!(missing.contains("no neighbouring sector"), "{missing}");
        let wanted = LiftSpec {
            low_floor: Some(-64_i32),
            ..spec(LiftPlatform::Existing(0_usize))
        };
        let bare = refuse(&build_lift(&mut lonely, &wanted).unwrap_err());
        assert!(bare.contains("no edge of sector 0 faces a lower"), "{bare}");
        assert_eq!(lonely, before, "both refusals wrote nothing");
    }

    #[test]
    fn an_out_of_range_platform_refuses_with_the_current_count() {
        let mut map = two_room_map();
        let message =
            refuse(&build_lift(&mut map, &spec(LiftPlatform::Existing(99_usize))).unwrap_err());
        assert!(message.contains("sector 99 is out of range"), "{message}");
        assert!(message.contains("holds 2 of them"), "{message}");
        assert_eq!(map, two_room_map(), "the refusal wrote nothing");
    }

    #[test]
    fn the_centre_of_a_platform_polygon_is_the_corner_average() {
        assert_eq!(
            polygon_centre(&[(0, 0), (64, 0), (64, 64), (0, 64)]),
            (32_i64, 32_i64),
            "the rectangle's middle"
        );
    }

    #[test]
    fn a_drawn_platform_straddles_the_step() {
        let mut map = two_room_map();
        let wanted = spec(LiftPlatform::Draw(vec![
            (224_i64, 64_i64),
            (320_i64, 64_i64),
            (320_i64, 192_i64),
            (224_i64, 192_i64),
        ]));
        let build = build_lift(&mut map, &wanted).expect("a drawn ledge");
        assert_eq!(build.platform_sector, 2_usize, "the third sector");
        assert_eq!(
            (build.floor_height, build.low_floor, build.drop),
            (32_i32, 0_i32, 32_i32)
        );
        assert!(!build.lines.is_empty(), "the hall-facing edges activate");
        assert!(
            build
                .notes
                .iter()
                .any(|note| note.contains("a lift is not needed")),
            "the 32-unit drop warns: {:?}",
            build.notes
        );
        let platform = map.sectors.get(2).expect("the platform");
        assert_eq!(
            (platform.floor_height, platform.ceiling_height),
            (32_i32, 128_i32)
        );
        assert_eq!(platform.tag, build.tag, "stamped with the fresh tag");
        assert_eq!(platform.floor_flat, "FLAT10", "the theme's lift floor");
        for index in &build.lines {
            let line = map.linedefs.get(*index).expect("an activation line");
            assert_eq!(
                side_sector(&map, line.front),
                Some(0_usize),
                "edge {index} faces the lower hall"
            );
            assert_eq!(
                side_sector(&map, line.back),
                Some(2_usize),
                "and the platform behind"
            );
            assert_eq!(line.special, 62_i32, "the SR lift on edge {index}");
            assert_eq!(line.tag, build.tag, "the shared tag on edge {index}");
            let front = map
                .sidedefs
                .get(line.front.expect("a front side"))
                .expect("side");
            assert_eq!(front.lower, "LIFTSIDE", "the lift's face on edge {index}");
        }
    }

    #[test]
    fn a_small_or_misplaced_platform_polygon_refuses() {
        let mut thin = two_room_map();
        let before = thin.clone();
        let tiny = refuse(
            &build_lift(
                &mut thin,
                &spec(LiftPlatform::Draw(vec![
                    (224_i64, 64_i64),
                    (320_i64, 64_i64),
                ])),
            )
            .unwrap_err(),
        );
        assert!(tiny.contains("at least three corners"), "{tiny}");

        let mut void = two_room_map();
        let lost = refuse(
            &build_lift(
                &mut void,
                &spec(LiftPlatform::Draw(vec![
                    (1000_i64, 1000_i64),
                    (1064_i64, 1000_i64),
                    (1064_i64, 1064_i64),
                    (1000_i64, 1064_i64),
                ])),
            )
            .unwrap_err(),
        );
        assert!(
            lost.contains("centre 1032, 1032 lies in no sector"),
            "{lost}"
        );

        let mut flat = two_room_map();
        let flush = refuse(
            &build_lift(
                &mut flat,
                &spec(LiftPlatform::Draw(vec![
                    (64_i64, 64_i64),
                    (192_i64, 64_i64),
                    (192_i64, 192_i64),
                    (64_i64, 192_i64),
                ])),
            )
            .unwrap_err(),
        );
        assert!(flush.contains("not above its lift floor"), "{flush}");
        assert_eq!(thin, before, "the short refusal wrote nothing");
    }

    #[test]
    fn the_edge_probe_skips_walls_that_are_not_lower_edges() {
        let map = two_room_map();
        assert_eq!(
            activation_edges(&map, 0_usize, 0_i32).len(),
            0_usize,
            "nothing is under the hall floor"
        );
        let edges = activation_edges(&map, 1_usize, 32_i32);
        assert_eq!(edges.len(), 1_usize, "the step line only");
        assert_eq!(edges.first().expect("the edge").line, 2_usize);
    }
}
