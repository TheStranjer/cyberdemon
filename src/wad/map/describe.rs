//! Pure per-sector descriptions over a decoded [`Map`]: the geometry,
//! identity and role half of the future `describe_map` core.
//!
//! [`Describe::sector`] turns one sector into a [`SectorFacts`] row: what
//! it measures (an integer shoelace [`SectorFacts::area`] over its
//! directed boundary, a bounding box and a label point the even-odd rule
//! places inside), how it looks (heights, light, flats, sky, the
//! definitions' name for its sector special, its tag, and a tally of the
//! textures its sides wear), what stands in it (things counted by
//! definition name), what it *is* ([`SectorFacts::roles`]) and what lies
//! next to it ([`SectorFacts::neighbours`]: every sector across its
//! two-sided boundary lines, with the `via` naming how a player gets
//! through). The per-map reads those last two fields need —
//! [`feature_targets`] and [`analyze_playability`] at [`Skill::Medium`] —
//! happen once per [`Describe`]; [`sector_facts`] is the one-sector
//! convenience wrapper over it.
//!
//! The roles, all lowercase, sorted and deduplicated: `door` (a door
//! action raises the sector's ceiling, or it is a closed band no more
//! than 8 units tall with two distinct neighbour sectors), `lift` (a lift
//! action drops it), `teleport_destination` (a teleport action lands a
//! player there), `secret`, `damaging` and `light_effect` (the family's
//! sector-special reads: the `Doom` family's vanilla numbers and `Boom`
//! bits through [`decode_sector_special`], the `Hexen` family's bit
//! 1024 and its own effect numbers), `exit_room` (a level-end line
//! touches the sector on either side, or a walk-over end line crosses
//! into it), `stair_step` (a footprint of at most 64 x 256 square units
//! with a neighbour one to 24 units up and another the same span down)
//! and `sky` (an `F_SKY1` ceiling).
//!
//! The neighbour `via` names, in precedence order: `door` and `lift`
//! when the neighbour sector wears that role; `window` when every shared
//! line blocks yet leaves at least 16 units of light between the higher
//! floor and the lower ceiling; `impassable` for any other boundary with
//! a blocking line; and otherwise `opening`, `step_up` or `step_down` by
//! the neighbour's floor over the sector's own.
//!
//! Like [`geometry`], this is a function of the map model and the passed
//! [`Definitions`] alone: no sessions, no `MCP`, no editing. References
//! that do not resolve simply do not take part in the answer, so a
//! sector with no usable boundary reports zero area, no box or centre,
//! no neighbours and only the roles its own row states.
//!
//! [`geometry`]: crate::wad::map::geometry

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString as _;
use alloc::vec::Vec;

use serde::Serialize;

use super::flags::{ACTIVATION_MASK, FlagFamily, Skill};
use super::geometry::{point_in_sector, sector_bbox, sector_boundary};
use super::model::{Linedef, Map};
use super::playability::{
    FeatureTargets, MAX_STEP_UP, Playability, analyze_playability, feature_targets,
};
use crate::defs::Definitions;
use crate::defs::boom::{SectorFamily, SectorFlag, decode_sector_special};

/// The ceiling flat that means open sky, the name the renderer switches on.
const SKY_FLAT: &str = "F_SKY1";
/// The model's name for "no texture on this side".
const NO_TEXTURE: &str = "-";
/// Half of the shoelace sum: the doubled area divides by it.
const AREA_HALF_OF: u128 = 2_u128;
/// The linedef flag bit that blocks a player: the `Doom` family's
/// `blocking` and the `Hexen` family's shared first bit alike.
const BLOCKING: u32 = 1_u32;
/// The `Hexen` family's `playeruse` activation: field value 1 at bit 10.
const PLAYER_USE: u32 = 0x400_u32;
/// The greatest headroom of a closed door band, map units.
const CLOSED_BAND_HEIGHT: i32 = 8_i32;
/// The smallest vertical light gap across a blocking boundary that reads
/// as a window, map units.
const LIGHT_GAP: i32 = 16_i32;
/// The largest footprint of a stair-step sector, square map units:
/// 64 x 256.
const STAIR_MAX_AREA: u64 = 0x4000_u64;
/// The smallest rising neighbour floor difference a step counts.
const STEP_UP: i32 = 1_i32;
/// The largest falling neighbour floor difference a step counts.
const STEP_DOWN_TOP: i32 = -1_i32;
/// The smallest rising difference negated: the deepest falling step a
/// stair step counts, mirroring [`MAX_STEP_UP`].
const MAX_STEP_DOWN: i32 = MAX_STEP_UP.saturating_neg();
/// The `Doom` family's vanilla secret sector type.
const DOOM_SECRET_BASE: u32 = 9_u32;
/// The `Doom` family's damaging vanilla sector types.
const DOOM_DAMAGE_BASES: [u32; 5] = [4_u32, 5_u32, 7_u32, 11_u32, 16_u32];
/// The `Doom` family's light-effect vanilla sector types.
const DOOM_LIGHT_BASES: [u32; 7] = [1_u32, 2_u32, 3_u32, 8_u32, 12_u32, 13_u32, 17_u32];
/// The `Hexen` family's secret sector flag bit.
const HEXEN_SECRET_BIT: i32 = 1024_i32;
/// The `Hexen` family's `ZDoom` damage sector specials.
const HEXEN_DAMAGE_SPECIALS: [i32; 7] = [69_i32, 70_i32, 71_i32, 82_i32, 83_i32, 84_i32, 85_i32];
/// The `Hexen` family's light-effect sector specials: the `Doom` numbers
/// shifted by 64.
const HEXEN_LIGHT_SPECIALS: [i32; 7] = [65_i32, 66_i32, 67_i32, 72_i32, 76_i32, 77_i32, 81_i32];
/// The `Doom` family's walk-over level ends, as the playability exit
/// table reads them.
const DOOM_WALK_EXITS: [i32; 4] = [52_i32, 124_i32, 197_i32, 198_i32];
/// The decoder's damage text for sectors that hurt nobody.
const NO_DAMAGE: &str = "none";

/// The role and `via` name a door sector answers with.
const NAME_DOOR: &str = "door";
/// The role and `via` name a lift platform answers with.
const NAME_LIFT: &str = "lift";
/// The role a teleport action's landing sector answers with.
const NAME_TELEPORT_DESTINATION: &str = "teleport_destination";
/// The role a hidden sector answers with.
const NAME_SECRET: &str = "secret";
/// The role a hurting sector answers with.
const NAME_DAMAGING: &str = "damaging";
/// The role a flickering sector answers with.
const NAME_LIGHT_EFFECT: &str = "light_effect";
/// The role a level-ending sector answers with.
const NAME_EXIT_ROOM: &str = "exit_room";
/// The role a stair step answers with.
const NAME_STAIR_STEP: &str = "stair_step";
/// The role an open-sky sector answers with.
const NAME_SKY: &str = "sky";
/// The `via` name a light-bearing blocking boundary answers with.
const NAME_WINDOW: &str = "window";
/// The `via` name a blocking boundary with no light answers with.
const NAME_IMPASSABLE: &str = "impassable";
/// The `via` name a level unblocked boundary answers with.
const NAME_OPENING: &str = "opening";
/// The `via` name a higher neighbour answers with.
const NAME_STEP_UP: &str = "step_up";
/// The `via` name a lower neighbour answers with.
const NAME_STEP_DOWN: &str = "step_down";

/// One world-space map point, coordinates widened to `i64`.
type Point = (i64, i64);

/// One directed boundary edge: a `(from, to)` pair of [`Point`]s.
type Edge = (Point, Point);

/// The number of a sector effect and the name the game's table gives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpecialInfo {
    /// The stored [`crate::wad::map::Sector::special`] value.
    pub value: i32,
    /// The display name from [`Definitions::sector_type`], or the plain
    /// number as text when the table has no row for it.
    pub name: String,
}

/// One kind of thing standing in a sector, and how many of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ThingCount {
    /// The display name from [`Definitions::thing`], or `Unknown <kind>`
    /// for a type the game does not define.
    pub name: String,
    /// How many things of this kind stand in the sector.
    pub count: u32,
}

/// One sector across a described sector's two-sided boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Neighbour {
    /// The index into [`Map::sectors`] the shared lines lie across.
    pub sector: usize,
    /// How a player gets through the shared boundary: `door` or `lift`
    /// when the neighbour sector wears that role, `window` or
    /// `impassable` for a wholly or partly blocking boundary with or
    /// without enough light between the higher floor and the lower
    /// ceiling, and otherwise `opening`, `step_up` or `step_down` by the
    /// neighbour's floor over the sector's own.
    pub via: String,
    /// The linedef indices shared with this neighbour, in linedef order.
    pub lines: Vec<usize>,
    /// The neighbour's floor height minus this sector's, map units.
    pub floor_delta: i32,
}

/// One sector described: geometry, look, contents, roles and neighbours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectorFacts {
    /// The index into [`Map::sectors`] this row describes.
    pub sector: usize,
    /// The even-odd floor area in square map units: the integer shoelace
    /// over the sector's directed boundary, `|sum| / 2` in exact `i128`.
    pub area: u64,
    /// The boundary's `(min_x, min_y, max_x, max_y)` box, `None` when the
    /// sector bounds no resolvable vertex.
    pub bbox: Option<[i32; 4]>,
    /// A world point the even-odd scan places inside the sector, `None`
    /// with no boundary.
    pub center: Option<[i64; 2]>,
    /// The floor height in map units.
    pub floor: i32,
    /// The ceiling height in map units.
    pub ceiling: i32,
    /// `ceiling` minus `floor`, saturating.
    pub height: i32,
    /// The light level as stored.
    pub light: i32,
    /// The floor flat name.
    pub floor_flat: String,
    /// The ceiling flat name.
    pub ceiling_flat: String,
    /// Whether the ceiling is the `F_SKY1` sky flat.
    pub sky: bool,
    /// The sector effect named through the definitions, `None` for
    /// special `0`.
    pub special: Option<SpecialInfo>,
    /// The sector tag matched by linedef and thing triggers.
    pub tag: i32,
    /// How often each texture name is worn by this sector's wall sides
    /// (`"-"` never counted), ordered by name.
    pub wall_textures: BTreeMap<String, u32>,
    /// The things standing here, counted by definition name, sorted by
    /// count descending then name ascending.
    pub things: Vec<ThingCount>,
    /// What this sector is, as the module-doc role rules name it:
    /// lowercase, sorted and deduplicated.
    pub roles: Vec<String>,
    /// The sectors across this sector's two-sided boundary lines, in
    /// sector order, each with the way through the shared boundary.
    pub neighbours: Vec<Neighbour>,
}

/// The per-map read behind [`SectorFacts`] rows.
///
/// Roles and neighbours ask what the map's door, lift, teleport and
/// level-end actions work on, so [`Describe::new`] folds the flag family,
/// [`feature_targets`] and [`analyze_playability`] once and every
/// [`Describe::sector`] call answers from them; nothing is walked again.
pub struct Describe<'map> {
    /// The map being described.
    map: &'map Map,
    /// The game definitions naming sector specials and thing types.
    defs: &'map Definitions,
    /// The flag family the map's lines and sectors store their bits in.
    family: FlagFamily,
    /// The sectors and lines the map's door, lift and teleport actions
    /// work on, read once.
    targets: FeatureTargets,
    /// The playability walk at [`Skill::Medium`], read once; its exit
    /// lines drive the `exit_room` role.
    play: Playability,
}

impl<'map> Describe<'map> {
    /// Fold `map` for describing: read its own flag family once, then the
    /// feature targets and the [`Skill::Medium`] playability walk over it.
    #[must_use]
    #[inline]
    pub fn new(map: &'map Map, defs: &'map Definitions) -> Self {
        let family = map.flag_family();
        Self {
            map,
            defs,
            family,
            targets: feature_targets(map, family),
            play: analyze_playability(map, family, Skill::Medium),
        }
    }

    /// Describe sector `sector`: the measured and cosmetic facts, the
    /// role names and the neighbour rows this module's rules define.
    ///
    /// `None` when the map holds no sector with that index. A sector with
    /// no usable boundary still describes: zero area, no box, no centre,
    /// no textures, no things and no neighbours.
    #[must_use]
    #[inline]
    pub fn sector(&self, sector: usize) -> Option<SectorFacts> {
        let row = self.map.sectors.get(sector)?;
        let edges = boundary_edges(self.map, sector);
        let area = shoelace_area(&edges);
        let neighbours = self.build_neighbours(sector, row.floor_height);
        Some(SectorFacts {
            sector,
            area,
            bbox: sector_bbox(self.map, sector).map(<[i32; 4]>::from),
            center: label_point(&edges),
            floor: row.floor_height,
            ceiling: row.ceiling_height,
            height: row.ceiling_height.saturating_sub(row.floor_height),
            light: row.light,
            floor_flat: row.floor_flat.clone(),
            ceiling_flat: row.ceiling_flat.clone(),
            sky: row.ceiling_flat == SKY_FLAT,
            special: special_info(row.special, self.defs),
            tag: row.tag,
            wall_textures: wall_textures(self.map, sector),
            things: thing_counts(self.map, self.defs, sector),
            roles: self.collect_roles(sector, area, &neighbours),
            neighbours,
        })
    }

    /// The neighbour rows of `sector`: its groups of shared two-sided
    /// lines, in sector order, each with the `via` its boundary and the
    /// sector across it answer with.
    fn build_neighbours(&self, sector: usize, own_floor: i32) -> Vec<Neighbour> {
        neighbour_groups(self.map, sector)
            .into_iter()
            .map(|(other, lines)| {
                let their_floor = self
                    .map
                    .sectors
                    .get(other)
                    .map_or(own_floor, |row| row.floor_height);
                let floor_delta = their_floor.saturating_sub(own_floor);
                let draft = Neighbour {
                    sector: other,
                    via: String::new(),
                    lines,
                    floor_delta,
                };
                let via = self.via_for(sector, &draft);
                Neighbour { via, ..draft }
            })
            .collect()
    }

    /// The way through the boundary `across` describes between `own` and
    /// its neighbour: the neighbour's `door` and `lift` roles first, then
    /// the blocking rules, then the heights.
    fn via_for(&self, own: usize, across: &Neighbour) -> String {
        if self.has_door_role(across.sector) {
            return NAME_DOOR.to_owned();
        }
        if self.targets.lifts.contains(&across.sector) {
            return NAME_LIFT.to_owned();
        }
        let blocking = across
            .lines
            .iter()
            .copied()
            .filter(|line| carries_flag(self.map, *line, BLOCKING))
            .count();
        if blocking == 0 {
            return height_via(across.floor_delta).to_owned();
        }
        if blocking == across.lines.len() && self.window_gap(own, across.sector) >= LIGHT_GAP {
            NAME_WINDOW.to_owned()
        } else {
            NAME_IMPASSABLE.to_owned()
        }
    }

    /// Whether `sector` wears the `door` role: a door action raises its
    /// ceiling, or it is a closed band with neighbours on both sides.
    fn has_door_role(&self, sector: usize) -> bool {
        self.targets.doors.contains(&sector) || self.closed_band(sector)
    }

    /// Whether `sector` is a closed door band: no more than
    /// [`CLOSED_BAND_HEIGHT`] of headroom and at least two distinct
    /// sectors across its two-sided lines.
    fn closed_band(&self, sector: usize) -> bool {
        let Some(row) = self.map.sectors.get(sector) else {
            return false;
        };
        row.ceiling_height.saturating_sub(row.floor_height) <= CLOSED_BAND_HEIGHT
            && neighbour_groups(self.map, sector).len() >= 2
    }

    /// The light across the blocking boundary between `own` and `other`:
    /// the lower ceiling over the higher floor, map units.
    fn window_gap(&self, own: usize, other: usize) -> i32 {
        let (Some(a), Some(b)) = (self.map.sectors.get(own), self.map.sectors.get(other)) else {
            return 0_i32;
        };
        a.ceiling_height
            .min(b.ceiling_height)
            .saturating_sub(a.floor_height.max(b.floor_height))
    }

    /// Whether a teleport action lands a player in `sector`.
    fn lands_teleports_in(&self, sector: usize) -> bool {
        self.targets
            .teleporters
            .iter()
            .any(|pad| pad.destination == Some(sector))
    }

    /// Whether an exit line of the playability read touches `sector` on
    /// either resolved side, or walks over into it at its midpoint.
    fn exits_here(&self, sector: usize) -> bool {
        self.play.exits.iter().any(|exit| {
            self.map
                .linedefs
                .get(exit.line)
                .is_some_and(|line| self.exit_side_or_sweep(line, sector))
        })
    }

    /// Whether `line` resolves to `sector` on either side, or is a
    /// walk-over end whose midpoint stands in `sector`.
    fn exit_side_or_sweep(&self, line: &Linedef, sector: usize) -> bool {
        side_sector(self.map, line.front) == Some(sector)
            || side_sector(self.map, line.back) == Some(sector)
            || (walk_over_exit(self.family, line)
                && line_midpoint(self.map, line)
                    .is_some_and(|(x, y)| point_in_sector(self.map, x, y) == Some(sector)))
    }

    /// The sector's role names: lowercase, sorted and deduplicated, as
    /// the module-doc rules state them for `area`, its `neighbours` and
    /// its own row.
    fn collect_roles(&self, sector: usize, area: u64, neighbours: &[Neighbour]) -> Vec<String> {
        let Some(row) = self.map.sectors.get(sector) else {
            return Vec::new();
        };
        let mut found: BTreeSet<String> = BTreeSet::new();
        if self.has_door_role(sector) {
            found.insert(NAME_DOOR.to_owned());
        }
        if self.targets.lifts.contains(&sector) {
            found.insert(NAME_LIFT.to_owned());
        }
        if self.lands_teleports_in(sector) {
            found.insert(NAME_TELEPORT_DESTINATION.to_owned());
        }
        if self.exits_here(sector) {
            found.insert(NAME_EXIT_ROOM.to_owned());
        }
        if row.ceiling_flat == SKY_FLAT {
            found.insert(NAME_SKY.to_owned());
        }
        if is_stair_step(area, neighbours) {
            found.insert(NAME_STAIR_STEP.to_owned());
        }
        found.extend(
            special_roles(row.special, self.family)
                .into_iter()
                .map(str::to_owned),
        );
        found.into_iter().collect()
    }
}

/// Describe sector `sector` of `map`, naming its sector special and its
/// things' types through the game definitions `defs`.
///
/// A thin wrapper over [`Describe`]: the per-map feature and playability
/// reads are folded for this one answer, so a caller describing a whole
/// map holds one [`Describe`] instead. `None` when the map holds no
/// sector with that index.
#[must_use]
#[inline]
pub fn sector_facts(map: &Map, defs: &Definitions, sector: usize) -> Option<SectorFacts> {
    Describe::new(map, defs).sector(sector)
}

/// `sector`'s boundary as directed world-space edges: front sides walk
/// `v1` to `v2`, back sides `v2` to `v1`, linedef order; lines whose
/// vertices do not resolve are skipped.
fn boundary_edges(map: &Map, sector: usize) -> Vec<Edge> {
    sector_boundary(map, sector)
        .into_iter()
        .filter_map(|(index, is_front)| {
            let line = map.linedefs.get(index)?;
            let start = map.vertices.get(line.v1)?;
            let end = map.vertices.get(line.v2)?;
            let from = (i64::from(start.x), i64::from(start.y));
            let to = (i64::from(end.x), i64::from(end.y));
            Some(if is_front { (from, to) } else { (to, from) })
        })
        .collect()
}

/// The sector the sidedef `side` opens into: `None` when the side is
/// absent, its sidedef index dangles, or that sidedef's sector index
/// dangles.
fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sidedef = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
}

/// The neighbour sectors across `sector`'s two-sided boundary, each with
/// the linedef indices it shares, in sector order and linedef order. A
/// line whose own side resolves to `sector` and whose far side resolves
/// to another sector joins that neighbour's group; lines to the void, to
/// `sector` itself or with a dangling far side group nothing.
fn neighbour_groups(map: &Map, sector: usize) -> Vec<(usize, Vec<usize>)> {
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, is_front) in sector_boundary(map, sector) {
        let across = map
            .linedefs
            .get(index)
            .and_then(|line| side_sector(map, if is_front { line.back } else { line.front }))
            .filter(|other| *other != sector);
        if let Some(other) = across {
            groups.entry(other).or_default().push(index);
        }
    }
    groups.into_iter().collect()
}

/// Whether linedef `line` carries the flag bit `flag`; a dangling index
/// carries none.
fn carries_flag(map: &Map, line: usize, flag: u32) -> bool {
    map.linedefs
        .get(line)
        .is_some_and(|def| def.flags & flag == flag)
}

/// The way through an unblocked boundary whose neighbour stands `delta`
/// map units above: level is an `opening`, higher a `step_up`, lower a
/// `step_down`.
const fn height_via(delta: i32) -> &'static str {
    if delta > 0_i32 {
        NAME_STEP_UP
    } else if delta < 0_i32 {
        NAME_STEP_DOWN
    } else {
        NAME_OPENING
    }
}

/// Whether an `area` sector wedged between `neighbours` reads as a stair
/// step: a footprint of at most [`STAIR_MAX_AREA`] square units with one
/// neighbour [`STEP_UP`] to [`MAX_STEP_UP`] units above and another
/// [`MAX_STEP_DOWN`] to [`STEP_DOWN_TOP`] units below.
fn is_stair_step(area: u64, neighbours: &[Neighbour]) -> bool {
    area <= STAIR_MAX_AREA
        && neighbours
            .iter()
            .any(|row| (STEP_UP..=MAX_STEP_UP).contains(&row.floor_delta))
        && neighbours
            .iter()
            .any(|row| (MAX_STEP_DOWN..=STEP_DOWN_TOP).contains(&row.floor_delta))
}

/// Whether the exit special on `line` ends the level by walking over it:
/// a `Doom`-family touch special, or a `Hexen`-family portal on any
/// activation but `playeruse`.
fn walk_over_exit(family: FlagFamily, line: &Linedef) -> bool {
    match family {
        FlagFamily::Doom => DOOM_WALK_EXITS.contains(&line.special),
        FlagFamily::Hexen => line.flags & ACTIVATION_MASK != PLAYER_USE,
    }
}

/// The world-space middle of `line`'s span, `None` when either vertex
/// fails to resolve.
fn line_midpoint(map: &Map, line: &Linedef) -> Option<Point> {
    let start = map.vertices.get(line.v1)?;
    let end = map.vertices.get(line.v2)?;
    Some((
        midpoint(i64::from(start.x), i64::from(end.x)),
        midpoint(i64::from(start.y), i64::from(end.y)),
    ))
}

/// The roles a sector special of the flag family `family` states.
fn special_roles(special: i32, family: FlagFamily) -> Vec<&'static str> {
    match family {
        FlagFamily::Doom => doom_special_roles(special),
        FlagFamily::Hexen => hexen_special_roles(special),
    }
}

/// The `Doom` family's read of one sector special, every bit field taken
/// through [`decode_sector_special`] and every bit outside them left
/// alone: the vanilla `9` or the `Boom` bit 128 hides, the vanilla
/// damaging numbers or any `Boom` damage level hurts, the vanilla light
/// numbers flicker; a value the decoder takes none of reads as nothing.
fn doom_special_roles(special: i32) -> Vec<&'static str> {
    let Some(decoded) = u32::try_from(special)
        .ok()
        .and_then(|value| decode_sector_special(value, SectorFamily::Doom))
    else {
        return Vec::new();
    };
    let mut roles = Vec::new();
    if decoded.base == DOOM_SECRET_BASE || decoded.flags.contains(&SectorFlag::Secret) {
        roles.push(NAME_SECRET);
    }
    if decoded.damage != NO_DAMAGE || DOOM_DAMAGE_BASES.contains(&decoded.base) {
        roles.push(NAME_DAMAGING);
    }
    if DOOM_LIGHT_BASES.contains(&decoded.base) {
        roles.push(NAME_LIGHT_EFFECT);
    }
    roles
}

/// The `Hexen` family's read of one sector special: bit 1024 hides, the
/// `ZDoom` damage specials hurt and the `Doom` light numbers shifted by
/// 64 flicker.
fn hexen_special_roles(special: i32) -> Vec<&'static str> {
    let mut roles = Vec::new();
    if special & HEXEN_SECRET_BIT == HEXEN_SECRET_BIT {
        roles.push(NAME_SECRET);
    }
    if HEXEN_DAMAGE_SPECIALS.contains(&special) {
        roles.push(NAME_DAMAGING);
    }
    if HEXEN_LIGHT_SPECIALS.contains(&special) {
        roles.push(NAME_LIGHT_EFFECT);
    }
    roles
}

/// The `u64` floor area of the directed `edges` loop set: the shoelace
/// sum of every edge's vertex cross product in exact `i128`, halved on its
/// absolute value. One consistent direction makes a hole's loop subtract
/// itself, so a sector with an island reports the even-odd sum.
fn shoelace_area(edges: &[Edge]) -> u64 {
    let mut sum = 0_i128;
    for edge in edges {
        let left = i128::from(edge.0.0).saturating_mul(i128::from(edge.1.1));
        let right = i128::from(edge.1.0).saturating_mul(i128::from(edge.0.1));
        sum = sum.saturating_add(left.saturating_sub(right));
    }
    let half = sum.unsigned_abs().checked_div(AREA_HALF_OF);
    u64::try_from(half.unwrap_or(u128::MAX)).unwrap_or(u64::MAX)
}

/// The world-space port of the renderer's sector label point: the middle
/// row of the boundary's box is scanned with the even-odd rule, the
/// longest inside interval wins and its midpoint is the centre; a polygon
/// the row misses falls back to its first boundary vertex. No edges, no
/// centre.
fn label_point(edges: &[Edge]) -> Option<[i64; 2]> {
    let (min_y, max_y) = edge_rows(edges)?;
    let row = midpoint(min_y, max_y);
    let mut crossings: Vec<i64> = edges
        .iter()
        .filter(|edge| crosses_row(edge, row))
        .map(|edge| intersect_x(edge.0, edge.1, row))
        .collect();
    crossings.sort_unstable();
    longest_interval(&crossings)
        .map(|(lo, hi)| [midpoint(lo, hi), row])
        .or_else(|| edges.first().map(|edge| <[i64; 2]>::from(edge.0)))
}

/// Whether `edge` straddles row `row` under the half-open even-odd rule.
const fn crosses_row(edge: &Edge, row: i64) -> bool {
    (edge.0.1 > row) != (edge.1.1 > row)
}

/// The `(lowest, highest)` endpoint rows over `edges`, `None` when empty.
fn edge_rows(edges: &[Edge]) -> Option<(i64, i64)> {
    let rows = edges.iter().flat_map(|edge| [edge.0.1, edge.1.1]);
    let min = rows.clone().min()?;
    Some((min, rows.max()?))
}

/// The midpoint of two world coordinates, rounded down.
const fn midpoint(low: i64, high: i64) -> i64 {
    low.saturating_add(high).div_euclid(2)
}

/// The widest `(lo, hi)` interval between consecutive pairs of the sorted
/// `crossings`, `None` when fewer than two of them exist.
fn longest_interval(crossings: &[i64]) -> Option<(i64, i64)> {
    let mut best: Option<(i64, i64)> = None;
    for pair in crossings.chunks_exact(2) {
        let Ok(&[lo, hi]) = <&[i64; 2]>::try_from(pair) else {
            continue;
        };
        if wider_than((lo, hi), best) {
            best = Some((lo, hi));
        }
    }
    best
}

/// Whether the interval `one` is wider than `best` (or `best` is unset).
const fn wider_than(one: (i64, i64), best: Option<(i64, i64)>) -> bool {
    match best {
        None => true,
        Some((b_lo, b_hi)) => one.1.saturating_sub(one.0) > b_hi.saturating_sub(b_lo),
    }
}

/// The column where the `start` to `end` line crosses row `y`,
/// floor-rounded in exact `i128`; a horizontal edge answers `start`'s
/// column (the even-odd filter never asks about one).
fn intersect_x(start: Point, end: Point, y: i64) -> i64 {
    let rise = i128::from(y).saturating_sub(i128::from(start.1));
    let span = i128::from(end.1).saturating_sub(i128::from(start.1));
    scale_down(start.0, end.0.saturating_sub(start.0), rise, span)
}

/// `base + delta * num / den` in exact `i128`, floor-rounded and
/// saturating back to `i64`; a zero `den` answers `base`.
fn scale_down(base: i64, delta: i64, num: i128, den: i128) -> i64 {
    if den == 0_i128 {
        return base;
    }
    let (n, d) = if den < 0_i128 {
        (num.saturating_neg(), den.saturating_neg())
    } else {
        (num, den)
    };
    let drawn = i128::from(base)
        .saturating_mul(d)
        .saturating_add(i128::from(delta).saturating_mul(n))
        .div_euclid(d);
    narrow(drawn)
}

/// The `i64` nearest an `i128` value, saturating at the bounds.
fn narrow(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0_i128 { i64::MIN } else { i64::MAX })
}

/// The [`SpecialInfo`] of sector special `value`: `None` for the plain `0`
/// that means "no effect", the table's name for a number
/// [`Definitions::sector_type`] knows, and the plain number as text for
/// everything else.
fn special_info(value: i32, defs: &Definitions) -> Option<SpecialInfo> {
    if value == 0_i32 {
        return None;
    }
    let name = u32::try_from(value)
        .ok()
        .and_then(|number| defs.sector_type(number))
        .map_or_else(|| value.to_string(), |row| row.name.clone());
    Some(SpecialInfo { value, name })
}

/// The texture tally of the sides opening into `sector`: for every
/// boundary line, each of its two sides whose own sector resolves to
/// `sector` contributes its upper, lower and middle names; `"-"` never
/// counts and a dangling side reference contributes nothing.
fn wall_textures(map: &Map, sector: usize) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (index, _) in sector_boundary(map, sector) {
        let Some(line) = map.linedefs.get(index) else {
            continue;
        };
        for side in [line.front, line.back].into_iter().flatten() {
            count_side(&mut counts, map, side, sector);
        }
    }
    counts
}

/// Record the three texture names of `map`'s sidedef `side` in `counts`,
/// when that side opens into `sector`.
fn count_side(counts: &mut BTreeMap<String, u32>, map: &Map, side: usize, sector: usize) {
    let Some(row) = map.sidedefs.get(side) else {
        return;
    };
    if row.sector != sector {
        return;
    }
    for texture in [row.upper.as_str(), row.lower.as_str(), row.middle.as_str()] {
        if texture != NO_TEXTURE {
            bump(counts, texture);
        }
    }
}

/// The display name of thing type `kind`: the table's name for a type
/// [`Definitions::thing`] knows, `Unknown <kind>` for anything else
/// (including a negative type number).
fn thing_name(defs: &Definitions, kind: i32) -> String {
    u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
        .map_or_else(|| format!("Unknown {kind}"), |row| row.name.clone())
}

/// Every thing that stands in `sector` - skill-independent, whatever its
/// presence bits say - counted by definition name and sorted by count
/// descending, then by name ascending.
fn thing_counts(map: &Map, defs: &Definitions, sector: usize) -> Vec<ThingCount> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.things {
        let at = point_in_sector(map, i64::from(row.x), i64::from(row.y));
        if at != Some(sector) {
            continue;
        }
        bump(&mut counts, &thing_name(defs, row.kind));
    }
    let mut rows: Vec<ThingCount> = counts
        .into_iter()
        .map(|(name, count)| ThingCount { name, count })
        .collect();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// Record `name` once more in `counts`.
fn bump(counts: &mut BTreeMap<String, u32>, name: &str) {
    let slot = counts.entry(name.to_owned()).or_insert(0_u32);
    *slot = slot.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use serde_json::{json, to_value};

    use super::{
        BLOCKING, Describe, Neighbour, SectorFacts, SpecialInfo, ThingCount, sector_facts,
    };
    use crate::defs::{Definitions, definitions_for};
    use crate::plan::FeatureDress;
    use crate::plan::compile::compile;
    use crate::plan::features::{FeatureRow, apply_features};
    use crate::plan::model::{
        Connection, ConnectionKind, DoorStyle, ExitStyle, Feature as PlanFeature,
        KeyColor as PlanKeyColor, Plan, Room, Shape,
    };
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, spawn, vertex};
    use crate::wad::map::features::LiftStyle;
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing};

    /// The shared `twosided` bit.
    const TWO_SIDED: u32 = 4_u32;
    /// The `Hexen` `playeruse` activation: field value 1 at bit 10.
    const PLAYER_USE: u32 = 0x400_u32;

    /// The embedded `DOOM` definitions (the sector table's home).
    fn doom_defs() -> Definitions {
        definitions_for(Game::Doom).expect("the embedded Doom table is valid")
    }

    /// The embedded `DOOM II` definitions (the thing table's superset).
    fn doom2_defs() -> Definitions {
        definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
    }

    /// The 256x256 square room: `WALL1` on all four one-sided walls,
    /// floor `0` under a 128-unit ceiling, light 160, tag 0, sector 0.
    fn square_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(256, 256),
            vertex(0, 256),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 0, 3),
        ];
        map.sidedefs = (0_usize..4_usize)
            .map(|_| side("-", "-", "WALL1", 0_usize))
            .collect();
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// The concave room: a 512x256 foot with a 256x256 tower on its west
    /// end, six `WALL1` sides around sector 0.
    fn elbow_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(256, 512),
            vertex(0, 512),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 4, 3),
            solid_line(4, 5, 4),
            solid_line(5, 0, 5),
        ];
        map.sidedefs = (0_usize..6_usize)
            .map(|_| side("-", "-", "WALL1", 0_usize))
            .collect();
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// The square room with its east side split in two and its north side
    /// in three: six `WALL1` sides and one `SWITCH1` middle around
    /// sector 0.
    fn switch_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(256, 128),
            vertex(256, 256),
            vertex(192, 256),
            vertex(64, 256),
            vertex(0, 256),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 4, 3),
            solid_line(4, 5, 4),
            solid_line(5, 6, 5),
            solid_line(6, 0, 6),
        ];
        map.sidedefs = vec![
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "SWITCH1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
        ];
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// Retune sector 0 of `map` in place.
    fn retune(map: &mut Map, row: Sector) {
        map.sectors = vec![row];
    }

    /// Stands `row` among `map`'s things.
    fn stand(map: &mut Map, row: Thing) {
        map.things.push(row);
    }

    /// A hand-built room: `floor` under 128 units of ceiling.
    fn cell(floor: i32) -> (i32, i32) {
        (floor, floor.saturating_add(128))
    }

    /// A hand-built slab: a sector with no headroom at all, which is what
    /// a shut door sector looks like.
    fn slab(floor: i32) -> (i32, i32) {
        (floor, floor)
    }

    /// The `x` of hand-built room column `column`.
    fn edge(column: usize) -> i32 {
        i32::try_from(column).map_or(i32::MAX, |index| index.saturating_mul(256))
    }

    /// A blocking one-sided wall from `v1` to `v2` wearing a fresh sidedef
    /// that faces `sector`.
    fn one_sided(v1: usize, v2: usize, sidedefs: &mut Vec<Sidedef>, sector: usize) -> Linedef {
        sidedefs.push(side("-", "-", "WALL1", sector));
        Linedef {
            v1,
            v2,
            flags: BLOCKING,
            front: Some(sidedefs.len().saturating_sub(1)),
            ..Default::default()
        }
    }

    /// The two-sided, unblocked divider between hall rooms `room` and
    /// `room + 1` of a `count`-room hall, its front facing `room`; it
    /// appends the two blank sidedefs it wears.
    fn hall_divider(room: usize, count: usize, sidedefs: &mut Vec<Sidedef>) -> Linedef {
        let next = room.saturating_add(1);
        let front = sidedefs.len();
        sidedefs.push(side("-", "-", "-", room));
        sidedefs.push(side("-", "-", "-", next));
        Linedef {
            v1: count.saturating_add(1).saturating_add(next),
            v2: next,
            flags: TWO_SIDED,
            front: Some(front),
            back: Some(front.saturating_add(1)),
            ..Default::default()
        }
    }

    /// A row of 256-unit rooms: `specs` is one `(floor, ceiling)` per
    /// room, room `i` spanning `x = 256i` to `x = 256(i + 1)` and owning
    /// sector `i`. Linedef `i` is the two-sided, unblocked divider between
    /// rooms `i` and `i + 1`, its front facing the lower-numbered room;
    /// the walls that close each room follow the dividers. A player 1
    /// start stands in the middle of room 0.
    fn hall(specs: &[(i32, i32)]) -> Map {
        let count = specs.len();
        let bottom = |column: usize| column;
        let top = |column: usize| count.saturating_add(1).saturating_add(column);
        let mut vertices = Vec::new();
        for column in 0..=count {
            vertices.push(vertex(edge(column), 0));
        }
        for column in 0..=count {
            vertices.push(vertex(edge(column), 256));
        }
        let mut linedefs = Vec::new();
        let mut sidedefs = Vec::new();
        for room_index in 0..count.saturating_sub(1) {
            linedefs.push(hall_divider(room_index, count, &mut sidedefs));
        }
        for room_index in 0..count {
            let next = room_index.saturating_add(1);
            linedefs.push(one_sided(
                top(room_index),
                top(next),
                &mut sidedefs,
                room_index,
            ));
            linedefs.push(one_sided(
                bottom(next),
                bottom(room_index),
                &mut sidedefs,
                room_index,
            ));
        }
        linedefs.push(one_sided(bottom(0), top(0), &mut sidedefs, 0));
        let last = count.saturating_sub(1);
        linedefs.push(one_sided(top(count), bottom(count), &mut sidedefs, last));
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things: vec![spawn(1, 128, 128, 0)],
            vertices,
            linedefs,
            sidedefs,
            sectors: specs
                .iter()
                .map(|spec| Sector {
                    floor_height: spec.0,
                    ceiling_height: spec.1,
                    floor_flat: "FLOOR1".to_owned(),
                    ceiling_flat: "FLOOR1".to_owned(),
                    light: 160,
                    ..Default::default()
                })
                .collect(),
            other: Vec::new(),
        }
    }

    /// Two equal rooms, `A` at `x 0..256` and `B` at `x 256..512`, whose
    /// shared wall is split at `y = 128` into linedefs `6` and `7`, both
    /// two-sided and facing `A` in front, wearing `lower_flags` and
    /// `upper_flags`.
    fn split_divider_map(lower_flags: u32, upper_flags: u32) -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
            vertex(256, 128),
        ];
        let mut sidedefs = Vec::new();
        let mut linedefs = vec![
            one_sided(0, 1, &mut sidedefs, 0),
            one_sided(1, 2, &mut sidedefs, 1),
            one_sided(2, 3, &mut sidedefs, 1),
            one_sided(3, 4, &mut sidedefs, 1),
            one_sided(4, 5, &mut sidedefs, 0),
            one_sided(5, 0, &mut sidedefs, 0),
        ];
        for (v1, v2, flags) in [(1_usize, 6_usize, lower_flags), (6, 4, upper_flags)] {
            let front = sidedefs.len();
            sidedefs.push(side("-", "-", "-", 0));
            sidedefs.push(side("-", "-", "-", 1));
            linedefs.push(Linedef {
                v1,
                v2,
                flags,
                front: Some(front),
                back: Some(front.saturating_add(1)),
                ..Default::default()
            });
        }
        map.linedefs = linedefs;
        map.sidedefs = sidedefs;
        map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 0)];
        map
    }

    /// The nested squares - an outer `0..1000` sector and an inner
    /// `400..600` sector of one-sided walls - plus a floating line inside
    /// the inner square whose front opens into the outer sector and which
    /// carries exit `special` with `flags`: the midpoint rule's test bench.
    fn nested_exit_map(format: MapFormat, special: i32, flags: u32) -> Map {
        let mut map = Map::new_empty(name("MAP01"), format);
        map.vertices = vec![
            vertex(0, 0),
            vertex(1000, 0),
            vertex(1000, 1000),
            vertex(0, 1000),
            vertex(400, 400),
            vertex(600, 400),
            vertex(600, 600),
            vertex(400, 600),
            vertex(450, 500),
            vertex(550, 500),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 0, 3),
            solid_line(4, 5, 4),
            solid_line(5, 6, 5),
            solid_line(6, 7, 6),
            solid_line(7, 4, 7),
            Linedef {
                v1: 8,
                v2: 9,
                flags,
                special,
                front: Some(8),
                ..Default::default()
            },
        ];
        map.sidedefs = (0_usize..9_usize)
            .map(|index| {
                let inner = (4_usize..8_usize).contains(&index);
                side("-", "-", "WALL1", usize::from(inner))
            })
            .collect();
        map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 0)];
        map
    }

    /// A theme answering every dressing role with a fixed name.
    struct FlatRoles;

    impl TextureRoles for FlatRoles {
        fn texture(&self, role: TextureRole) -> Option<&str> {
            Some(match role {
                TextureRole::Floor => "FLATROL",
                TextureRole::Ceiling => "CEIROL",
                TextureRole::Wall => "WALROL",
                TextureRole::Door => "DOORROL",
                TextureRole::DoorFloor => "DFLAROL",
                TextureRole::DoorTrack => "DTRROL",
                TextureRole::WindowSill => "WSIROL",
                TextureRole::Step => "STEPRO",
            })
        }
    }

    /// A rectangular plan room, 256 units tall, spanning `span` on `x`
    /// and standing at `heights`' `(floor, ceiling)`.
    fn plan_room(id: &str, span: (i64, i64), heights: (i32, i32)) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: span.0,
                y: 0,
                w: span.1,
                h: 256,
            },
            floor: heights.0,
            ceiling: heights.1,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A connection of `kind` between the rooms of `pair`, `width` clear.
    fn plan_link(id: &str, pair: (&str, &str), kind: ConnectionKind, width: i64) -> Connection {
        Connection {
            id: Some(id.to_owned()),
            from: pair.0.to_owned(),
            to: pair.1.to_owned(),
            kind,
            at: None,
            width,
        }
    }

    /// The plan every [`built`] test wraps: `rooms` joined by `links`,
    /// `features` wired and `thickness` of wall grown between the rooms.
    fn plan_of(
        rooms: Vec<Room>,
        links: Vec<Connection>,
        features: Vec<PlanFeature>,
        thickness: i64,
    ) -> Plan {
        Plan {
            rooms,
            connections: links,
            features,
            wall_thickness: thickness,
        }
    }

    /// A compiled plan in `format` with its features wired.
    fn built(plan: &Plan, format: MapFormat) -> (Map, Vec<FeatureRow>) {
        let mut compiled =
            compile(plan, name("MAP01"), format, &FlatRoles).expect("the test plans compile");
        let dress = FeatureDress {
            door: "DOORROL".to_owned(),
            track: "DTRROL".to_owned(),
            track_red: "REDTRAK".to_owned(),
            track_blue: "BLUETRAK".to_owned(),
            track_yellow: "YELTRAK".to_owned(),
            switch: "SWITROL".to_owned(),
            exit_switch: "EXITROL".to_owned(),
            step: "STEPRO".to_owned(),
            wall: "WALROL".to_owned(),
        };
        let rows = apply_features(plan, &mut compiled, &dress).expect("the test features install");
        (compiled.map, rows)
    }

    /// The feature row `apply_features` reported for kind `kind`.
    fn row(rows: &[FeatureRow], kind: &str) -> FeatureRow {
        rows.iter()
            .find(|entry| entry.kind == kind)
            .cloned()
            .unwrap_or_else(|| panic!("a {kind} feature row"))
    }

    /// The sector standing at the middle of a 256-unit-wide plan room
    /// column `column`.
    fn room_sector(map: &Map, column: usize) -> usize {
        let x = i32::try_from(column.saturating_mul(256)).unwrap_or(i32::MAX);
        point_in_sector(map, i64::from(x).saturating_add(128), 128_i64).expect("the room resolves")
    }

    /// Whether `facts` names the `role`.
    fn has_role(facts: &SectorFacts, role: &str) -> bool {
        facts.roles.iter().any(|found| found.as_str() == role)
    }

    /// The neighbour of `facts` across from sector `sector`.
    fn neighbour_across(facts: &SectorFacts, sector: usize) -> &Neighbour {
        facts
            .neighbours
            .iter()
            .find(|entry| entry.sector == sector)
            .unwrap_or_else(|| panic!("sector {sector} is a neighbour: {:?}", facts.neighbours))
    }

    /// The facts of `map`'s sector `index` through one shared [`Describe`].
    fn described(map: &Map, index: usize) -> SectorFacts {
        Describe::new(map, &doom_defs())
            .sector(index)
            .unwrap_or_else(|| panic!("sector {index} exists"))
    }

    #[test]
    fn a_square_room_reports_area_box_centre_and_look() {
        let map = square_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(facts.sector, 0, "the index echoes");
        assert_eq!(facts.area, 0x0001_0000_u64, "256x256 of floor");
        assert_eq!(
            facts.bbox,
            Some([0_i32, 0_i32, 256_i32, 256_i32]),
            "the box spans the walls"
        );
        assert_eq!(facts.center, Some([128_i64, 128_i64]), "the square's heart");
        let at = facts.center.expect("the asserted centre");
        assert_eq!(
            point_in_sector(&map, at[0], at[1]),
            Some(0),
            "and the point really stands in the sector"
        );
        assert_eq!(facts.floor, 0_i32, "floor height");
        assert_eq!(facts.ceiling, 128_i32, "ceiling height");
        assert_eq!(facts.height, 128_i32, "ceiling minus floor");
        assert_eq!(facts.light, 160_i32, "light level");
        assert_eq!(facts.floor_flat, "FLOOR1", "floor flat");
        assert_eq!(facts.ceiling_flat, "FLOOR1", "ceiling flat");
        assert!(!facts.sky, "a solid ceiling is no sky");
        assert_eq!(facts.special, None, "special 0 describes as nothing");
        assert_eq!(facts.tag, 0_i32, "tag passthrough");
        assert_eq!(
            to_value(&facts.wall_textures).expect("counts serialise"),
            json!({ "WALL1": 4_u64 }),
            "four `WALL1` sides"
        );
        assert!(facts.things.is_empty(), "nobody stood in");
        assert!(facts.roles.is_empty(), "a plain room is nothing special");
        assert!(facts.neighbours.is_empty(), "and stands alone");
    }

    #[test]
    fn a_concave_room_reports_its_true_area_and_a_centre_in_the_foot() {
        let map = elbow_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(facts.area, 196_608_u64, "512x256 plus 256x256");
        assert_eq!(
            facts.bbox,
            Some([0_i32, 0_i32, 512_i32, 512_i32]),
            "the box bounds the whole room, concavity included"
        );
        assert_eq!(
            facts.center,
            Some([128_i64, 256_i64]),
            "the middle row's widest inside interval is the tower's base"
        );
        assert_eq!(
            point_in_sector(&map, 128, 256),
            Some(0),
            "and the label point stands inside the sector"
        );
    }

    #[test]
    fn a_sky_ceiling_is_flagged_and_wears_the_sky_role() {
        let mut map = square_map();
        let mut outdoor = room(0, 128, 160, 0);
        outdoor.ceiling_flat = "F_SKY1".to_owned();
        retune(&mut map, outdoor);
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert!(facts.sky, "an `F_SKY1` ceiling is the sky");
        assert_eq!(facts.ceiling_flat, "F_SKY1", "and the name rides along");
        assert_eq!(facts.roles, vec!["sky".to_owned()], "and the role too");

        let mut indoors = square_map();
        let mut floored = room(0, 128, 160, 0);
        floored.floor_flat = "F_SKY1".to_owned();
        retune(&mut indoors, floored);
        let under = sector_facts(&indoors, &doom2_defs(), 0).expect("sector 0 exists");
        assert!(!under.sky, "an `F_SKY1` floor is nobody's sky");
        assert!(under.roles.is_empty(), "and it earns no role either");
    }

    #[test]
    fn sector_specials_are_named_through_the_definitions() {
        let mut map = square_map();
        let mut secret = room(0, 128, 160, 0);
        secret.special = 9_i32;
        retune(&mut map, secret);
        let facts = sector_facts(&map, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            facts.special,
            Some(SpecialInfo {
                value: 9,
                name: "Secret".to_owned(),
            }),
            "the Doom table names type 9"
        );

        let mut unassigned = square_map();
        let mut odd = room(0, 128, 160, 0);
        odd.special = 42_i32;
        retune(&mut unassigned, odd);
        let unnamed = sector_facts(&unassigned, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            unnamed.special,
            Some(SpecialInfo {
                value: 42,
                name: "42".to_owned(),
            }),
            "a number the table has no row for falls back to its own text"
        );

        let mut negative = square_map();
        let mut below = room(0, 128, 160, 0);
        below.special = -3_i32;
        retune(&mut negative, below);
        let off_table = sector_facts(&negative, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            off_table.special,
            Some(SpecialInfo {
                value: -3,
                name: "-3".to_owned(),
            }),
            "and a negative value reads as its own text too"
        );
    }

    #[test]
    fn wall_textures_count_both_faces_of_every_side_opening_here() {
        let map = switch_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            to_value(&facts.wall_textures).expect("counts serialise"),
            json!({ "SWITCH1": 1_u64, "WALL1": 6_u64 }),
            "six walls and one switch panel, split sides folded per name"
        );
        assert_eq!(
            facts.area, 0x0001_0000_u64,
            "the split walls do not move the box"
        );
    }

    #[test]
    fn things_are_counted_by_name_sorted_by_size_and_all_skills_count() {
        let mut map = square_map();
        stand(&mut map, spawn(3001, 64, 64, 0));
        stand(&mut map, spawn(3001, 128, 64, 0));
        stand(
            &mut map,
            Thing {
                flags: 4,
                ..spawn(3001, 64, 192, 0)
            },
        );
        stand(&mut map, spawn(2001, 192, 192, 0));
        stand(&mut map, spawn(9999, 128, 192, 0));
        stand(&mut map, spawn(3001, 4_000, 4_000, 7));
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            facts.things,
            vec![
                ThingCount {
                    name: "Imp".to_owned(),
                    count: 3,
                },
                ThingCount {
                    name: "Shotgun".to_owned(),
                    count: 1,
                },
                ThingCount {
                    name: "Unknown 9999".to_owned(),
                    count: 1,
                },
            ],
            "count descending, ties by name ascending; the hard-skill imp \
             counts and the one in the void does not"
        );
    }

    #[test]
    fn a_missing_sector_describes_as_nothing_and_an_empty_one_as_zeroes() {
        let map = square_map();
        assert!(
            sector_facts(&map, &doom2_defs(), 9).is_none(),
            "no sector 9 in a one-sector map"
        );
        let mut lonelier = square_map();
        lonelier.sectors.push(room(0, 0, 0, 0));
        let bare = sector_facts(&lonelier, &doom2_defs(), 1).expect("sector 1 exists");
        assert_eq!(bare.area, 0, "a sector with no boundary has no area");
        assert_eq!(bare.bbox, None, "and no box");
        assert_eq!(bare.center, None, "and no centre");
        assert!(bare.wall_textures.is_empty(), "and no walls");
        assert!(bare.things.is_empty(), "and nothing stands in it");
        assert!(bare.neighbours.is_empty(), "and nobody next to it");
        assert!(bare.roles.is_empty(), "and it states no role at all");
    }

    #[test]
    fn the_row_serialises_to_the_tickets_shape() {
        let value = to_value(sector_facts(&square_map(), &doom2_defs(), 0).expect("sector 0"))
            .expect("facts serialise");
        assert_eq!(
            value,
            json!({
                "sector": 0_u64,
                "area": 0x0001_0000_u64,
                "bbox": [0_i64, 0_i64, 256_i64, 256_i64],
                "center": [128_i64, 128_i64],
                "floor": 0_i64,
                "ceiling": 128_i64,
                "height": 128_i64,
                "light": 160_i64,
                "floor_flat": "FLOOR1",
                "ceiling_flat": "FLOOR1",
                "sky": false,
                "special": null,
                "tag": 0_i64,
                "wall_textures": { "WALL1": 4_u64 },
                "things": [],
                "roles": [],
                "neighbours": [],
            }),
            "every field the two ticket halves named, in their names"
        );
    }

    #[test]
    fn equal_floors_meet_across_an_opening() {
        let map = hall(&[cell(0_i32), cell(0_i32)]);
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let from_a = describe.sector(0).expect("sector 0 exists");
        assert_eq!(
            from_a.neighbours,
            vec![Neighbour {
                sector: 1,
                via: "opening".to_owned(),
                lines: vec![0_usize],
                floor_delta: 0_i32,
            }],
            "the level gap between the rooms reads as an opening"
        );
        assert!(from_a.roles.is_empty(), "a full-height room is nothing");
        let from_b = describe.sector(1).expect("sector 1 exists");
        assert_eq!(
            to_value(&from_b.neighbours).expect("neighbours serialise"),
            json!([{
                "sector": 0_u64,
                "via": "opening",
                "lines": [0_u64],
                "floor_delta": 0_i64,
            }]),
            "and the row reads back in the ticket's names"
        );
    }

    #[test]
    fn a_step_up_below_a_ledge_reads_as_a_step_down_above_it() {
        let map = hall(&[cell(0_i32), cell(16_i32)]);
        let low = described(&map, 0);
        assert_eq!(
            neighbour_across(&low, 1).via,
            "step_up",
            "the neighbour stands above"
        );
        assert_eq!(neighbour_across(&low, 1).floor_delta, 16_i32, "16 units");
        let high = described(&map, 1);
        assert_eq!(
            neighbour_across(&high, 0).via,
            "step_down",
            "and back down from the ledge"
        );
        assert_eq!(neighbour_across(&high, 0).floor_delta, -16_i32, "-16 units");
    }

    #[test]
    fn a_blocking_divider_without_light_is_impassable_from_either_side() {
        let mut map = hall(&[cell(0_i32), (128_i32, 256_i32)]);
        map.linedefs.get_mut(0).expect("the divider").flags = TWO_SIDED | BLOCKING;
        let low = described(&map, 0);
        let from_below = neighbour_across(&low, 1);
        assert_eq!(from_below.via, "impassable", "a sealed divider is no way");
        assert_eq!(
            from_below.floor_delta, 128_i32,
            "and the tall step never gets its say"
        );
        let high = described(&map, 1);
        assert_eq!(
            neighbour_across(&high, 0).via,
            "impassable",
            "the same seal from the other side"
        );
    }

    #[test]
    fn a_blocking_raised_sill_with_light_is_a_window() {
        let mut map = hall(&[cell(0_i32), (16_i32, 256_i32)]);
        map.linedefs.get_mut(0).expect("the divider").flags = TWO_SIDED | BLOCKING;
        let low = described(&map, 0);
        let through = neighbour_across(&low, 1);
        assert_eq!(through.via, "window", "112 units over the sill");
        assert_eq!(
            through.floor_delta, 16_i32,
            "and the delta still rides along"
        );
        let high = described(&map, 1);
        assert_eq!(
            neighbour_across(&high, 0).via,
            "window",
            "the glass reads the same from the raised side"
        );
    }

    #[test]
    fn one_blocking_line_among_several_seals_the_whole_boundary() {
        let sealed = split_divider_map(TWO_SIDED, TWO_SIDED | BLOCKING);
        let facts = described(&sealed, 0);
        let across = neighbour_across(&facts, 1);
        assert_eq!(across.via, "impassable", "a partly sealed boundary seals");
        assert_eq!(
            across.lines,
            vec![6_usize, 7],
            "both shared lines ride along"
        );
        let open = split_divider_map(TWO_SIDED, TWO_SIDED);
        let wide = described(&open, 0);
        assert_eq!(
            neighbour_across(&wide, 1).via,
            "opening",
            "and unblocked it is a wide gap"
        );
        assert_eq!(
            neighbour_across(&wide, 1).lines,
            vec![6_usize, 7],
            "grouped into one neighbour in linedef order"
        );
    }

    #[test]
    fn a_closed_band_between_two_rooms_reads_as_a_door() {
        let map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let band = describe.sector(1).expect("the band exists");
        assert_eq!(band.roles, vec!["door".to_owned()], "no special needed");
        assert!(
            band.neighbours.iter().all(|entry| entry.via == "opening"),
            "the rooms beside it are plain floors: {:?}",
            band.neighbours
        );
        for room_index in [0_usize, 2] {
            let room_facts = describe.sector(room_index).expect("a room exists");
            let across = neighbour_across(&room_facts, 1);
            assert_eq!(
                across.via, "door",
                "the band beside room {room_index} is a door"
            );
            assert_eq!(across.floor_delta, 0_i32, "level with it");
        }
    }

    #[test]
    fn a_plan_built_door_band_wears_the_door_role_and_offers_its_doors() {
        let blueprint = plan_of(
            vec![
                plan_room("a", (0, 256), cell(0_i32)),
                plan_room("b", (256, 256), cell(0_i32)),
            ],
            vec![plan_link(
                "d1",
                ("a", "b"),
                ConnectionKind::Door {
                    style: DoorStyle::Locked(PlanKeyColor::Blue),
                },
                64,
            )],
            Vec::new(),
            16_i64,
        );
        let (map, rows) = built(&blueprint, MapFormat::Doom);
        let band = *row(&rows, "door").sectors.first().expect("the band");
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let band_facts = describe.sector(band).expect("the band exists");
        assert!(has_role(&band_facts, "door"), "the band is a door");
        for column in [0_usize, 1] {
            let room_index = room_sector(&map, column);
            let room_facts = describe.sector(room_index).expect("a room exists");
            assert!(
                !has_role(&room_facts, "door"),
                "room {column} stands full-height, not a door"
            );
            assert_eq!(
                neighbour_across(&room_facts, band).via,
                "door",
                "room {column} reaches the band through a door"
            );
        }
    }

    #[test]
    fn a_plan_built_lift_is_a_lift_and_rides_up_from_its_low_neighbour() {
        let lifts = plan_of(
            vec![
                plan_room("low", (0, 256), cell(0_i32)),
                plan_room("pad", (256, 256), cell(96_i32)),
            ],
            vec![plan_link("c1", ("low", "pad"), ConnectionKind::Opening, 64)],
            vec![PlanFeature::Lift {
                room: "pad".to_owned(),
                low_floor: None,
                style: LiftStyle::UseWall,
                repeatable: true,
            }],
            0_i64,
        );
        let (map, rows) = built(&lifts, MapFormat::Doom);
        let platform = *row(&rows, "lift").sectors.first().expect("the platform");
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let pad = describe.sector(platform).expect("the platform exists");
        assert!(has_role(&pad, "lift"), "the platform is a lift");
        let low_index = room_sector(&map, 0);
        let low = describe.sector(low_index).expect("the low room exists");
        let across = neighbour_across(&low, platform);
        assert_eq!(across.via, "lift", "the way up is the lift");
        assert_eq!(across.floor_delta, 96_i32, "over the ledge's height");
    }

    #[test]
    fn a_plan_built_secret_room_wears_the_secret_role() {
        let mut hidden = plan_room("b", (256, 256), cell(0_i32));
        hidden.secret = true;
        let blueprint = plan_of(
            vec![plan_room("a", (0, 256), cell(0_i32)), hidden],
            vec![plan_link("c1", ("a", "b"), ConnectionKind::Opening, 64)],
            Vec::new(),
            16_i64,
        );
        let (map, rows) = built(&blueprint, MapFormat::Doom);
        let secret = *row(&rows, "secret")
            .sectors
            .first()
            .expect("the secret room");
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let facts = describe.sector(secret).expect("the secret room exists");
        assert!(has_role(&facts, "secret"), "the flagged room is a secret");
        let open_index = room_sector(&map, 0);
        let open = describe.sector(open_index).expect("the hall exists");
        assert!(!has_role(&open, "secret"), "the hall beside it is not");
    }

    #[test]
    fn a_walk_over_exit_room_is_an_exit_room() {
        let blueprint = plan_of(
            vec![
                plan_room("a", (0, 256), cell(0_i32)),
                plan_room("b", (256, 256), cell(0_i32)),
            ],
            vec![plan_link("c1", ("a", "b"), ConnectionKind::Opening, 64)],
            vec![PlanFeature::Exit {
                room: "b".to_owned(),
                style: ExitStyle::WalkOver,
                secret: false,
                wall: None,
            }],
            16_i64,
        );
        let (map, _) = built(&blueprint, MapFormat::Doom);
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let exit_room = describe
            .sector(room_sector(&map, 1))
            .expect("the exit room exists");
        assert!(
            has_role(&exit_room, "exit_room"),
            "walking its boundary ends the level"
        );
        let hall = describe
            .sector(room_sector(&map, 0))
            .expect("the hall exists");
        assert!(
            !has_role(&hall, "exit_room"),
            "and the far room does not share the line"
        );
    }

    #[test]
    fn a_walk_over_exit_line_sweeps_its_midpoint_sector_in() {
        let defs = doom_defs();
        let crossed = nested_exit_map(MapFormat::Doom, 52_i32, BLOCKING);
        let describe = Describe::new(&crossed, &defs);
        let outer = describe.sector(0).expect("the outer sector exists");
        assert!(
            has_role(&outer, "exit_room"),
            "the line's own side reads by the boundary rule"
        );
        let inner = describe.sector(1).expect("the inner sector exists");
        assert!(
            has_role(&inner, "exit_room"),
            "and the walk-over midpoint stands in the inner square"
        );

        let pressed = nested_exit_map(MapFormat::Doom, 11_i32, BLOCKING);
        let by_use = Describe::new(&pressed, &defs);
        assert!(
            !has_role(&by_use.sector(1).expect("inner"), "exit_room"),
            "a use line sweeps nothing into the inner square"
        );
        assert!(
            has_role(&by_use.sector(0).expect("outer"), "exit_room"),
            "while the side rule still holds for it"
        );
    }

    #[test]
    fn hexen_exits_follow_their_activation_field() {
        let defs = doom_defs();
        let portal = nested_exit_map(MapFormat::Hexen, 243_i32, 0_u32);
        let describe = Describe::new(&portal, &defs);
        assert!(
            has_role(&describe.sector(1).expect("inner"), "exit_room"),
            "a playercross portal walks over into the inner square"
        );
        let used = nested_exit_map(MapFormat::Hexen, 243_i32, PLAYER_USE);
        let pressed = Describe::new(&used, &defs);
        assert!(
            !has_role(&pressed.sector(1).expect("inner"), "exit_room"),
            "and a playeruse one sweeps nothing"
        );
        assert!(
            has_role(&pressed.sector(0).expect("outer"), "exit_room"),
            "while its own side still answers"
        );
    }

    #[test]
    fn stepped_floors_flag_the_middle_step_only() {
        let climb = plan_of(
            vec![
                plan_room("a", (0, 256), cell(0_i32)),
                plan_room("b", (256, 256), cell(32_i32)),
            ],
            vec![plan_link(
                "s1",
                ("a", "b"),
                ConnectionKind::Stairs {
                    step_height: 16_i32,
                    step_depth: 24_i32,
                },
                64,
            )],
            Vec::new(),
            0_i64,
        );
        let (map, _) = built(&climb, MapFormat::Doom);
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let stepped: Vec<usize> = (0..map.sectors.len())
            .filter_map(|index| describe.sector(index))
            .filter(|facts| has_role(facts, "stair_step"))
            .map(|facts| facts.sector)
            .collect();
        assert_eq!(stepped.len(), 1, "one step between the rooms: {stepped:?}");
        let middle = describe
            .sector(*stepped.first().expect("the step"))
            .expect("it exists");
        assert_eq!(middle.floor, 16_i32, "the 16-unit tread");
        let vias: Vec<&str> = middle.neighbours.iter().map(|n| n.via.as_str()).collect();
        assert!(vias.contains(&"step_up"), "one side rises: {vias:?}");
        assert!(vias.contains(&"step_down"), "the other falls: {vias:?}");
    }

    #[test]
    fn a_plan_built_teleporter_marks_its_destination_square() {
        let teleported = plan_of(
            vec![
                plan_room("a", (0, 256), cell(0_i32)),
                plan_room("b", (512, 256), cell(0_i32)),
            ],
            Vec::new(),
            vec![PlanFeature::Teleporter {
                from_room: "a".to_owned(),
                to_room: "b".to_owned(),
                pad: Some((64, 64)),
                destination: Some((576, 64)),
                angle: 0_i32,
                two_way: false,
            }],
            16_i64,
        );
        let (map, rows) = built(&teleported, MapFormat::Doom);
        let squares = row(&rows, "teleporter").sectors;
        let pad = *squares.first().expect("the pad square");
        let square = *squares.get(1).expect("the destination square");
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        let landing = describe.sector(square).expect("the square exists");
        assert!(
            has_role(&landing, "teleport_destination"),
            "a teleport action lands here"
        );
        let start = describe.sector(pad).expect("the pad exists");
        assert!(
            !has_role(&start, "teleport_destination"),
            "the pad itself is nobody's landing"
        );
    }

    #[test]
    fn doom_sector_specials_state_their_vanilla_and_boom_roles() {
        let mut hurt = square_map();
        let mut damaging = room(0, 128, 160, 0);
        damaging.special = 4_i32;
        retune(&mut hurt, damaging);
        let facts = sector_facts(&hurt, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(facts.roles, vec!["damaging".to_owned()], "20% and strobe");

        let mut lit = square_map();
        let mut flickering = room(0, 128, 160, 0);
        flickering.special = 1_i32;
        retune(&mut lit, flickering);
        let lit_facts = sector_facts(&lit, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            lit_facts.roles,
            vec!["light_effect".to_owned()],
            "light on and off randomly"
        );

        let mut boom = square_map();
        let mut flagged = room(0, 128, 160, 0);
        flagged.special = 133_i32;
        retune(&mut boom, flagged);
        let boom_facts = sector_facts(&boom, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            boom_facts.roles,
            vec!["damaging".to_owned(), "secret".to_owned()],
            "the Boom read: base 5 hurts and bit 128 hides"
        );

        let mut plain_secret = square_map();
        let mut hidden = room(0, 128, 160, 0);
        hidden.special = 128_i32;
        retune(&mut plain_secret, hidden);
        let hidden_facts = sector_facts(&plain_secret, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            hidden_facts.roles,
            vec!["secret".to_owned()],
            "bit 128 alone hides a plain sector"
        );

        let mut negative = square_map();
        let mut below = room(0, 128, 160, 0);
        below.special = -3_i32;
        retune(&mut negative, below);
        let off_table = sector_facts(&negative, &doom_defs(), 0).expect("sector 0 exists");
        assert!(
            off_table.roles.is_empty(),
            "a negative special states nothing"
        );
    }

    #[test]
    fn hexen_sector_specials_state_their_own_numbers() {
        let mut map = square_map();
        map.format = MapFormat::Hexen;
        let mut hidden = room(0, 128, 160, 0);
        hidden.special = 1024_i32;
        retune(&mut map, hidden);
        let described_map = described(&map, 0);
        assert_eq!(
            described_map.roles,
            vec!["secret".to_owned()],
            "bit 1024 hides a Hexen-family sector"
        );

        let mut nine = square_map();
        nine.format = MapFormat::Hexen;
        let mut doomish = room(0, 128, 160, 0);
        doomish.special = 9_i32;
        retune(&mut nine, doomish);
        assert!(
            described(&nine, 0).roles.is_empty(),
            "and the Doom family's 9 states nothing here"
        );

        for (special, role) in [
            (69_i32, "damaging"),
            (85_i32, "damaging"),
            (65_i32, "light_effect"),
            (81_i32, "light_effect"),
        ] {
            let mut one = square_map();
            one.format = MapFormat::Hexen;
            let mut row_special = room(0, 128, 160, 0);
            row_special.special = special;
            retune(&mut one, row_special);
            assert_eq!(
                described(&one, 0).roles,
                vec![role.to_owned()],
                "special {special} states {role}"
            );
        }
    }

    #[test]
    fn one_describe_answers_every_sector_like_the_wrapper() {
        let map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        let defs = doom_defs();
        let describe = Describe::new(&map, &defs);
        assert_eq!(
            describe.sector(9),
            None,
            "an index past the sectors still answers nothing"
        );
        for index in [0_usize, 1, 2] {
            assert_eq!(
                describe.sector(index),
                sector_facts(&map, &doom_defs(), index),
                "the wrapper and the shared read agree on sector {index}"
            );
        }
    }
}
