//! The special tables the playability walk reads, and the features, exits,
//! locks and keys folded out of a map by them.
//!
//! Every number here is written out on purpose. The `Doom` family's door,
//! lift, teleport and exit specials, the `Hexen` family's scripted actions,
//! argument slots and lock codes, and the thing types that carry a key are
//! the engine's own, not a definition table's: no `DeHackEd` rename and no
//! custom `DECORATE` title can move what this analysis believes the game
//! does. Anything the tables do not carry is counted, never guessed at.
//!
//! One line is one feature. Two lines sharing a tag - the pair of room-facing
//! lines of a plan-built door, say - are two features acting on the same
//! sectors, which is how the engine treats them too: reaching either one
//! opens the door.
//!
//! The rows themselves are the private `tables` module and the one pass that
//! folds a map through them, with the state it carries, is the private
//! `reader` module. This file is what both share: the constants, the result
//! types the walk reads and the three entry points it calls.

mod reader;
mod tables;

#[cfg(test)]
mod tests;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::KeyColor;
use crate::wad::map::flags::{
    ACTIVATION_MASK, ACTIVATION_SHIFT, FlagFamily, PLAYER_USE, Skill, THING_DOOM_NOT_SINGLE,
    THING_HEXEN_SINGLE,
};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::{Linedef, Map};
use tables::{KEY_THINGS, RowEffect, Trigger};

/// The thing type of the player 1 start.
const PLAYER_START: i32 = 1_i32;
/// The `Doom` family's teleport destination thing.
const TELEPORT_DEST: i32 = 14_i32;
/// The thing types a `Hexen` teleporter accepts as its destination.
const HEXEN_DESTINATIONS: &[i32] = &[14_i32, 9_043_i32, 9_044_i32];
/// The `Hexen` teleport argument that narrows the destination search to the
/// sectors carrying this tag, when it is non-zero.
const SECTOR_TAG_ARG: usize = 1_usize;

/// What an openable feature does to the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Its sectors' ceilings rise to the vanilla open-door height.
    Door,
    /// Its sectors may ride at their lowest neighbouring floor.
    Lift,
    /// It moves the player from its trigger sectors to its destination.
    Teleport,
}

/// One feature of the map a player can work.
#[derive(Clone, Debug)]
pub(super) struct Feature {
    /// The linedef carrying the action.
    pub(super) line: usize,
    /// What the action does.
    pub(super) effect: Effect,
    /// The sectors whose reachability sets the action off.
    pub(super) triggers: BTreeSet<usize>,
    /// The key colour the line demands before it works.
    pub(super) lock: Option<KeyColor>,
    /// The sectors the action changes; empty for a teleport.
    pub(super) targets: BTreeSet<usize>,
    /// The sector a teleport lands the player in: `None` for every other
    /// effect, and for a teleport line whose destination resolves to no
    /// sector.
    pub(super) destination: Option<usize>,
}

/// One level exit line.
#[derive(Clone, Debug)]
pub(super) struct ExitLine {
    /// The linedef carrying the action.
    pub(super) line: usize,
    /// The line's special number.
    pub(super) special: i32,
    /// Whether it ends the level through the secret map.
    pub(super) secret: bool,
    /// The sectors from which a player reaches it.
    pub(super) triggers: BTreeSet<usize>,
}

/// One locked line, pointing at the feature that wants the key.
#[derive(Clone, Debug)]
pub(super) struct LockLine {
    /// The linedef carrying the lock.
    pub(super) line: usize,
    /// The colour of key it wants.
    pub(super) color: KeyColor,
    /// The index into [`Inventory::features`] the line works.
    pub(super) feature: usize,
}

/// One key item that spawns at the analysed skill.
#[derive(Clone, Debug)]
pub(super) struct KeyThing {
    /// The index into [`Map::things`].
    pub(super) thing: usize,
    /// The colour of lock the key opens.
    pub(super) color: KeyColor,
    /// The sector it stands in, `None` when it stands in the void.
    pub(super) sector: Option<usize>,
}

/// Everything the walk needs to know about a map, read out of it once.
#[derive(Clone, Debug)]
pub(super) struct Inventory {
    /// Every openable feature, in linedef order.
    pub(super) features: Vec<Feature>,
    /// Every exit line, in linedef order.
    pub(super) exits: Vec<ExitLine>,
    /// Every locked line, in linedef order.
    pub(super) locks: Vec<LockLine>,
    /// Every key that spawns at the analysed skill, in thing order.
    pub(super) keys: Vec<KeyThing>,
    /// Every sector a door action acts on.
    pub(super) door_sectors: BTreeSet<usize>,
    /// Every sector a lift action acts on.
    pub(super) lift_sectors: BTreeSet<usize>,
    /// What the read could not use, in report order.
    pub(super) warnings: Vec<String>,
}

/// What reading one action line came up with.
enum Reading {
    /// A feature the player can work.
    Feature(Feature),
    /// A special the analysis knows it need not model.
    Ignored,
    /// A special none of the tables carry.
    Unknown,
}

/// A line's action as its table row describes it, before the map's sectors
/// are resolved.
struct Plan {
    /// What the special does.
    effect: RowEffect,
    /// How the player sets it off.
    trigger: Trigger,
    /// The key colour the line demands.
    lock: Option<KeyColor>,
    /// The tag or `TID` the action addresses.
    tag: i32,
}

/// The reference tables and accumulators one inventory read works through.
struct Reader<'map> {
    /// The map being read.
    map: &'map Map,
    /// The flag family its lines and things are stored in.
    family: FlagFamily,
    /// The skill whose presence bits decide what spawns.
    skill: Skill,
    /// The sectors grouped by tag, for the tagged actions.
    tagged: BTreeMap<i32, BTreeSet<usize>>,
    /// The features found so far.
    features: Vec<Feature>,
    /// The exit lines found so far.
    exits: Vec<ExitLine>,
    /// The locked lines found so far.
    locks: Vec<LockLine>,
    /// The keys that spawn.
    keys: Vec<KeyThing>,
    /// The sectors a door action acts on.
    door_sectors: BTreeSet<usize>,
    /// The sectors a lift action acts on.
    lift_sectors: BTreeSet<usize>,
    /// The specials the tables do not carry, counted per number.
    unhandled: BTreeMap<i32, usize>,
    /// The lock codes the table cannot name, gathered once each.
    unknown_locks: BTreeSet<i32>,
    /// The notes the read has made so far.
    warnings: Vec<String>,
}

/// Whether a thing carrying `flags` spawns for a single player at `skill` in
/// flag family `family`: its skill bit set, and the family's own answer to
/// "is this thing in the game at all" - the `Doom` family leaves its
/// multiplayer-only things out, the `Hexen` one asks for the single-player
/// mode bit.
pub(super) fn spawns(flags: u32, family: FlagFamily, skill: Skill) -> bool {
    let bit = skill.bit(family);
    if bit == 0_u32 || flags & bit != bit {
        return false;
    }
    match family {
        FlagFamily::Doom => flags & THING_DOOM_NOT_SINGLE != THING_DOOM_NOT_SINGLE,
        FlagFamily::Hexen => flags & THING_HEXEN_SINGLE == THING_HEXEN_SINGLE,
    }
}

/// The `args` slot `slot` of `line`, or `0` past the end.
fn argument(line: &Linedef, slot: usize) -> i32 {
    line.args.get(slot).copied().unwrap_or(0_i32)
}

/// How a `Hexen` family line is worked: only `playeruse` needs the player on
/// a particular side, every other activation reaches from either side.
fn hexen_trigger(flags: u32) -> Trigger {
    let field = (flags & ACTIVATION_MASK)
        .checked_shr(ACTIVATION_SHIFT)
        .unwrap_or(0_u32);
    if field == PLAYER_USE {
        return Trigger::Use;
    }
    Trigger::Walk
}

/// The colour a `Hexen` lock code answers for, `None` for a code beyond the
/// six vanilla keys and the three any-colour numbers.
const fn lock_color(code: i32) -> Option<KeyColor> {
    match code {
        1_i32 | 4_i32 | 129_i32 => Some(KeyColor::Red),
        2_i32 | 5_i32 | 130_i32 => Some(KeyColor::Blue),
        3_i32 | 6_i32 | 131_i32 => Some(KeyColor::Yellow),
        _ => None,
    }
}

/// The colour of key the thing type `kind` carries.
fn key_color(kind: i32) -> Option<KeyColor> {
    KEY_THINGS
        .iter()
        .find(|entry| entry.0 == kind)
        .map(|entry| entry.1)
}

/// The map's sectors grouped by tag, for resolving a tagged action.
fn sectors_by_tag(map: &Map) -> BTreeMap<i32, BTreeSet<usize>> {
    let mut tagged: BTreeMap<i32, BTreeSet<usize>> = BTreeMap::new();
    for (index, sector) in map.sectors.iter().enumerate() {
        tagged.entry(sector.tag).or_default().insert(index);
    }
    tagged
}

/// The index of the player 1 start the walk begins from: the first type-1
/// thing that spawns at `skill`, `None` when the map holds no such thing.
pub(super) fn start_thing(map: &Map, family: FlagFamily, skill: Skill) -> Option<usize> {
    map.things
        .iter()
        .enumerate()
        .find(|entry| entry.1.kind == PLAYER_START && spawns(entry.1.flags, family, skill))
        .map(|entry| entry.0)
}

/// The sector the player 1 start stands in.
///
/// The first type-1 thing that spawns at `skill` decides. A map holding more
/// than one start, or a start standing outside every sector, cannot walk
/// anywhere either, and says so in `warnings` rather than failing.
pub(super) fn start_sector(
    map: &Map,
    family: FlagFamily,
    skill: Skill,
    warnings: &mut Vec<String>,
) -> Option<usize> {
    let first = start_thing(map, family, skill);
    let Some(spawn_index) = first else {
        warnings.push(format!(
            "no player 1 start spawns at {} skill: nothing can be reached",
            skill.as_str()
        ));
        return None;
    };
    let extra = map
        .things
        .iter()
        .enumerate()
        .filter(|entry| entry.1.kind == PLAYER_START && spawns(entry.1.flags, family, skill))
        .count();
    if extra > 1_usize {
        warnings.push(format!(
            "{extra} player 1 starts spawn at {} skill: the walk begins at thing {spawn_index}",
            skill.as_str()
        ));
    }
    let spawn = map.things.get(spawn_index)?;
    let sector = point_in_sector(map, i64::from(spawn.x), i64::from(spawn.y));
    if sector.is_none() {
        warnings.push(format!(
            "player 1 start thing {spawn_index} stands outside every sector: nothing can be reached"
        ));
    }
    sector
}

/// Reads `map` into the [`Inventory`] of features, exits, locks and keys a
/// player at `skill` can work, in flag family `family`.
pub(super) fn read_inventory(map: &Map, family: FlagFamily, skill: Skill) -> Inventory {
    let mut reader = Reader::new(map, family, skill);
    for (index, line) in map.linedefs.iter().enumerate() {
        reader.run_line(index, line);
    }
    for (index, spawn) in map.things.iter().enumerate() {
        reader.run_thing(index, spawn);
    }
    reader.finish()
}
