//! Pure per-sector descriptions and the map-wide overview over a decoded
//! [`Map`]: the geometry, identity and role core behind the `describe_map`
//! tool.
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
//! through). The per-map reads a row needs — the [`SectorIndex`] geometry
//! and each thing's home sector, [`feature_targets`], and
//! [`analyze_playability`] at [`Skill::Medium`] — happen once per
//! [`Describe`]; [`sector_facts`] is the one-sector convenience wrapper
//! over it.
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
//! and `sky` (a ceiling wearing the session's sky flat: `F_SKY1`, `F_SKY`
//! in `Hexen`).
//!
//! The neighbour `via` names, in precedence order: `door` and `lift`
//! when the neighbour sector wears that role; `window` when every shared
//! line blocks yet leaves at least 16 units of light between the higher
//! floor and the lower ceiling; `impassable` for any other boundary with
//! a blocking line; and otherwise `opening`, `step_up` or `step_down` by
//! the neighbour's floor over the sector's own.
//!
//! The map-wide [`Describe::summary`] folds the same per-map reads into a
//! [`MapOverview`]: the stored format and vertex bounds, element counts,
//! the player starts, the [`Skill::Medium`] walk's exit, key and lock rows
//! reused as-is, the teleport actions with their (possibly unresolved)
//! destinations, the sectors wearing the `secret` role, the monsters and
//! items tallied by definition name, the ten busiest wall textures and
//! flats, the five largest sectors and a `playable` `{ok, summary}` read
//! computed the way `check_playability` computes its own — the sentence
//! builders are duplicated here because this layer may not import the
//! tool layer.
//!
//! Like [`geometry`], this is a function of the map model and the passed
//! [`Definitions`] alone: no sessions, no `MCP`, no editing. References
//! that do not resolve simply do not take part in the answer, so a
//! sector with no usable boundary reports zero area, no box or centre,
//! no neighbours and only the roles its own row states.
//!
//! This root holds the answer shapes, the [`Describe`] type and the
//! [`sector_facts`] entry point. The reading itself is split over four
//! private modules: `reader` is the one `impl Describe`, the neighbour and
//! role rules that need the folded walks at hand; `roles` the boundary and
//! neighbour-group reads and the per-family sector-special tables; `shape`
//! the exact integer geometry of the area and the label point; and `tally`
//! every count, tally and playable sentence.
//!
//! [`geometry`]: crate::wad::map::geometry
//! [`feature_targets`]: crate::wad::map::playability::feature_targets
//! [`analyze_playability`]: crate::wad::map::playability::analyze_playability
//! [`decode_sector_special`]: crate::defs::boom::decode_sector_special
//! [`Skill::Medium`]: crate::wad::map::flags::Skill::Medium

mod reader;
mod roles;
mod shape;
mod tally;

#[cfg(test)]
mod tests;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Serialize;

use super::flags::FlagFamily;
use super::geometry::SectorIndex;
use super::model::Map;
use super::playability::{ExitInfo, FeatureTargets, KeyInfo, LockInfo, Playability, Teleporter};
use super::stats::ElementCounts;
use crate::defs::Definitions;
use crate::wad::classify::MapFormat;

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

/// Every role name a sector can wear, as [`SectorFacts::roles`] spells
/// them. The query engine validates `roles has` names against this list.
pub const ROLE_NAMES: [&str; 9] = [
    NAME_DOOR,
    NAME_LIFT,
    NAME_TELEPORT_DESTINATION,
    NAME_SECRET,
    NAME_DAMAGING,
    NAME_LIGHT_EFFECT,
    NAME_EXIT_ROOM,
    NAME_STAIR_STEP,
    NAME_SKY,
];

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
    /// Whether the ceiling is the session's sky flat (`F_SKY1`, `F_SKY`
    /// in `Hexen`).
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

/// One player-start thing: where a type 1 to 4 stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PlayerStart {
    /// The index into [`Map::things`].
    pub thing: usize,
    /// The stored thing type: 1, 2, 3 or 4.
    pub kind: i32,
}

/// Whether a player can start and finish the map, in the
/// `check_playability` tool's own words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlayableSummary {
    /// Whether a player start stands in a sector, a non-secret exit is
    /// reachable from a side the walk reached, and the start thing itself
    /// is not among the stuck things.
    pub ok: bool,
    /// The exit note (or the no-start note) and the reachable-sector
    /// count, one pair of plain sentences.
    pub summary: String,
}

/// The map-wide overview: the pagination-independent picture an agent
/// orients on before reading any single row.
///
/// [`Describe::summary`] folds it from the reads one [`Describe`] already
/// holds plus one stuck-test pass: nothing here paginates, sorts a whole
/// element block out, or reports a sector's own facts — the per-sector
/// [`SectorFacts`] rows answer those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MapOverview {
    /// The layout the map block is stored in, as kept.
    pub format: MapFormat,
    /// The vertex box `[min_x, min_y, max_x, max_y]`, `None` when the map
    /// holds no vertices.
    pub bounds: Option<[i32; 4]>,
    /// How many rows each of the five element blocks holds.
    pub counts: ElementCounts,
    /// The player-start things, in thing order.
    pub player_starts: Vec<PlayerStart>,
    /// The level exits as the [`Skill::Medium`] walk saw them, in linedef
    /// order.
    ///
    /// [`Skill::Medium`]: crate::wad::map::flags::Skill::Medium
    pub exits: Vec<ExitInfo>,
    /// The keys that spawn at the skill, in thing order.
    pub keys: Vec<KeyInfo>,
    /// The locked lines, in linedef order; each row says whether the walk
    /// opened its door.
    pub locked_doors: Vec<LockInfo>,
    /// The teleport actions, in linedef order, each landing on a sector
    /// or on nothing (`destination` `None`) when its tag or `TID` answers
    /// no destination thing.
    pub teleporters: Vec<Teleporter>,
    /// The sectors wearing the `secret` role, ascending.
    pub secret_sectors: Vec<usize>,
    /// The things whose definitions stand under a Monsters category,
    /// counted by display name; unknown and absent definitions count
    /// under neither tally.
    pub monsters: BTreeMap<String, u32>,
    /// The things whose definitions stand under a Weapons, Ammunition,
    /// Powerups, Keys or Health And Armour category, counted by display
    /// name.
    pub items: BTreeMap<String, u32>,
    /// The ten busiest texture names worn by the map's sidedefs (upper,
    /// middle and lower faces, `"-"` never counted): count descending,
    /// ties by name ascending.
    pub top_wall_textures: Vec<(String, u32)>,
    /// The ten busiest flat names worn by the map's sectors (floors and
    /// ceilings, `"-"` never counted), read the same way.
    pub top_flats: Vec<(String, u32)>,
    /// The indices of the five largest sectors by [`SectorFacts::area`],
    /// descending, ties by index.
    pub largest_sectors: Vec<usize>,
    /// Whether a player can start and finish the map, and the sentences
    /// that say so.
    pub playable: PlayableSummary,
}

/// The per-map read behind [`SectorFacts`] rows and the [`MapOverview`].
///
/// Roles and neighbours ask what the map's door, lift, teleport and
/// level-end actions work on, so [`Describe::new`] folds the flag family,
/// [`feature_targets`] and [`analyze_playability`] once and every
/// [`Describe::sector`] call answers from them; nothing is walked again.
/// The boundary, box and point reads come from one [`SectorIndex`] folded
/// in [`Describe::new`] too, alongside each thing's home sector resolved
/// through it once, so no per-row call rescans the map.
/// [`Describe::summary`] folds the same reads, plus one stuck-test pass,
/// into the map-wide [`MapOverview`].
///
/// [`analyze_playability`]: crate::wad::map::playability::analyze_playability
/// [`feature_targets`]: crate::wad::map::playability::feature_targets
/// [`Skill::Medium`]: crate::wad::map::flags::Skill::Medium
pub struct Describe<'map> {
    /// The map being described.
    map: &'map Map,
    /// The game definitions naming sector specials and thing types.
    defs: &'map Definitions,
    /// The flag family the map's lines and sectors store their bits in.
    family: FlagFamily,
    /// The sky flat of the caller's game ([`crate::wad::game::sky_flat`]):
    /// the ceiling name that reads as the `sky` field and role.
    sky_flat: &'static str,
    /// The map's per-sector geometry, indexed once: the boundary, box and
    /// point reads every row answers from.
    index: SectorIndex,
    /// Each thing's home sector, resolved once through
    /// [`SectorIndex::point_in_sector`]: each thing's stored `x`/`y`
    /// point's holder, `None` for a thing the map holds nowhere.
    homes: Vec<Option<usize>>,
    /// The sectors and lines the map's door, lift and teleport actions
    /// work on, read once.
    targets: FeatureTargets,
    /// The playability walk at [`Skill::Medium`], read once; its exit
    /// lines drive the `exit_room` role.
    ///
    /// [`Skill::Medium`]: crate::wad::map::flags::Skill::Medium
    play: Playability,
}

/// Describe sector `sector` of `map`, naming its sector special and its
/// things' types through the game definitions `defs` and reading its
/// ceiling against the sky flat `sky_flat`.
///
/// A thin wrapper over [`Describe`]: the per-map geometry, feature and
/// playability reads are folded for this one answer, so a caller
/// describing a whole map holds one [`Describe`] instead. `None` when the
/// map holds no sector with that index.
#[must_use]
#[inline]
pub fn sector_facts(
    map: &Map,
    defs: &Definitions,
    sky_flat: &'static str,
    sector: usize,
) -> Option<SectorFacts> {
    Describe::new(map, defs, sky_flat).sector(sector)
}
