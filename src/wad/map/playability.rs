//! Playability analysis: whether a player can actually get anywhere.
//!
//! [`validate`] answers "is this map well formed?". This module answers the
//! question a player cares about instead: can I get from the start to the
//! exit, does the blue key sit in front of the blue door, is there a way up
//! onto that ledge? It is a pure read of a decoded [`Map`] - no sessions, no
//! `MCP`, no editing - and it never refuses a map: a broken map simply comes
//! back with a short list of places nobody can reach.
//!
//! The model is a sector-level movement graph. The private `graph` module
//! folds the two-sided lines into a sector adjacency list and decides, from
//! the player's shape ([`MIN_LINE_WIDTH`], [`PLAYER_HEIGHT`],
//! [`MAX_STEP_UP`]) and the heights of the two sectors, whether a line lets
//! the player through; the private `specials` module reads the map's line
//! specials and things into the features a player can work - manual and
//! tagged doors, lifts, teleporters and level exits, the keys each lock wants
//! and the things that carry them; and the private `reach` module walks the
//! graph from the player start, opening every feature whose trigger sector
//! and key the walk has already reached, until the answer stops changing.
//! [`analyze_playability`] folds the two walks it needs (the strict one, and
//! a wider one that lets the player squeeze through short lines to see what
//! they would have connected) into [`Playability`]. [`blockers`] explains a
//! finished walk sector by sector - which rule kept the player out of each
//! unreachable one - and [`stuck_things`] reads the map's bodies to say
//! which things cannot stand where they are.
//!
//! Heights are taken exactly as stored, with one deliberate liberty: a closed
//! door sector is evaluated in its *open* state once the walk has reached a
//! trigger for it, because a map that needs a door to open is not unplayable
//! for the door being shut. A lift platform is evaluated both at its stored
//! floor and at the floor it drops to, which is how the engine treats a
//! call button: either position is available to a player who can reach the
//! platform.
//!
//! Things - starts, keys and teleport destinations alike - count only when
//! their presence bits name them at the analysed [`Skill`], and only in the
//! single-player game.
//!
//! [`feature_targets`] is the narrower read for a caller that only wants to
//! know *which* sectors the map's door, lift and teleport actions work on,
//! without walking anything: it folds the same inventory [`analyze_playability`]
//! reads and throws the walk away.
//!
//! [`validate`]: crate::wad::map::validate

mod explain;
mod graph;
mod reach;
mod specials;
mod stuck;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Serialize;

pub use explain::{Blocker, BlockerReason, blockers};
pub use stuck::{Stuck, StuckReason, stuck_things};

use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::model::Map;

use graph::World;
use reach::Run;
use reach::fixpoint;
use reach::touches;
use specials::Effect;
use specials::Inventory;
use specials::LockLine;

/// The highest ledge a player can walk up (the shared
/// [`MAX_STEP_HEIGHT`](crate::wad::map::consts::MAX_STEP_HEIGHT)):
/// falling is free at any depth, so movement is directional and a drop is
/// a one-way connection.
pub use crate::wad::map::consts::MAX_STEP_HEIGHT as MAX_STEP_UP;
/// The shortest two-sided line a player can walk through and the height
/// they needs to stand in, shared from
/// [`consts`](crate::wad::map::consts) and re-exported for the walk's
/// readers.
pub use crate::wad::map::consts::{MIN_LINE_WIDTH, PLAYER_HEIGHT};
/// A sector no more than this far above its floor is a closed door: while
/// its trigger is unreachable it is read as it stands, once the walk reaches
/// a trigger it is read open.
const CLOSED_DOOR_TOLERANCE: i32 = 8_i32;
/// How far below the lowest neighbouring ceiling a vanilla door leaves the
/// gap when it is fully open.
const DOOR_HEADROOM: i32 = 4_i32;

/// One of the three key colours a lock can demand. A colour is answered by
/// either its card or its skull key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyColor {
    /// The blue card or skull key.
    Blue,
    /// The red card or skull key.
    Red,
    /// The yellow card or skull key.
    Yellow,
}

impl KeyColor {
    /// The lowercase name of this colour.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Red => "red",
            Self::Yellow => "yellow",
        }
    }
}

/// One level exit the map carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExitInfo {
    /// The linedef index carrying the action.
    pub line: usize,
    /// The line's special number.
    pub special: i32,
    /// Whether this is the map's secret exit.
    pub secret: bool,
    /// Whether the walk reached a side of the line the action works from.
    pub reachable: bool,
}

/// One key item that spawns at the analysed skill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    /// The index into [`Map::things`].
    pub thing: usize,
    /// The colour of lock this key opens.
    pub color: KeyColor,
    /// The sector the key stands in; `None` when it stands in the void.
    pub sector: Option<usize>,
    /// Whether the walk reached that sector.
    pub reachable: bool,
}

/// One locked line: a door whose action demands a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LockInfo {
    /// The linedef index carrying the lock.
    pub line: usize,
    /// The colour of key the line wants.
    pub color: KeyColor,
    /// Whether this line's door stands open at the end of the walk: the
    /// walk both reached the key and worked a trigger, or another line had
    /// already raised every sector this one acts on - the way the pair of
    /// room-facing lines of one built door opens whichever side is pressed.
    pub opened: bool,
}

/// One teleport action the map carries: the line working it and the sector it
/// lands a player in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Teleporter {
    /// The linedef index carrying the teleport action.
    pub line: usize,
    /// The sector the read landed the player in: `None` when the line's tag or
    /// `TID` answers no destination thing at all.
    pub destination: Option<usize>,
}

/// The sectors and lines the map's door, lift and teleport actions work on.
///
/// A read of the map's own specials, not of any walk: it says which sectors are
/// door bands and which are lift platforms, and which lines carry the actions,
/// for a caller labelling map parts. [`analyze_playability`] starts from the
/// same inventory and goes on to ask what a player can reach.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FeatureTargets {
    /// Every sector a door action raises its ceiling over: the sectors answering
    /// a tagged door's tag, and the sector behind a manual door's line.
    pub doors: BTreeSet<usize>,
    /// Every linedef carrying a door action the tables read.
    pub door_lines: BTreeSet<usize>,
    /// Every sector a lift action drops to its lowest neighbouring floor.
    pub lifts: BTreeSet<usize>,
    /// Every linedef carrying a lift action the tables read.
    pub lift_lines: BTreeSet<usize>,
    /// One row per teleport action line the tables read, in linedef order.
    pub teleporters: Vec<Teleporter>,
}

/// What a player can and cannot do in a map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Playability {
    /// The sector the player 1 start stands in: `None` when the map holds no
    /// start at the analysed skill, or its start stands outside every
    /// sector. Everything else then reads as unreachable.
    pub start_sector: Option<usize>,
    /// Every sector a player starting in [`Self::start_sector`] can stand
    /// in, with the features they can work already worked.
    pub reachable: BTreeSet<usize>,
    /// Every exit line, in linedef order.
    pub exits: Vec<ExitInfo>,
    /// Every key item that spawns, in thing order.
    pub keys: Vec<KeyInfo>,
    /// Every locked line, in linedef order.
    pub locks: Vec<LockInfo>,
    /// The sectors behind a door action no reachable trigger ever opens, in
    /// sector order.
    pub doors_never_opened: Vec<usize>,
    /// The two-sided lines too short to walk through that would otherwise
    /// connect two areas the player can reach, in linedef order.
    pub narrow_lines: Vec<usize>,
    /// Everything the analysis could not read: line specials it does not
    /// model, lock codes it cannot name, doors acting on nothing and the
    /// missing player start, in report order.
    pub warnings: Vec<String>,
}

/// Asks what a player at `skill` can do in `map`, whose lines and things
/// store their flags in the family `family`.
///
/// `family` is normally [`Map::flag_family`]; the caller may pass the other
/// one to read a map's numbers as if they belonged to it.
///
/// [`Map::flag_family`]: crate::wad::map::Map::flag_family
#[must_use]
#[inline]
pub fn analyze_playability(map: &Map, family: FlagFamily, skill: Skill) -> Playability {
    let mut warnings = Vec::new();
    let start = specials::start_sector(map, family, skill, &mut warnings);
    let inventory = specials::read_inventory(map, family, skill);
    let strict = World::new(map, family, &inventory, MIN_LINE_WIDTH);
    let squeezed = World::new(map, family, &inventory, 0_i32);
    let walk = fixpoint(&strict, &inventory, start);
    let loose = fixpoint(&squeezed, &inventory, start);
    warnings.extend_from_slice(&inventory.warnings);
    Playability {
        start_sector: start,
        exits: exit_report(&inventory, &walk),
        keys: key_report(&inventory, &walk),
        locks: lock_report(&inventory, &walk),
        doors_never_opened: never_opened(&inventory, &walk),
        narrow_lines: squeezed.narrow_lines(&loose),
        warnings,
        reachable: walk.reachable,
    }
}

/// The index of the player 1 start the reachability walk begins from: the
/// first type-1 thing that spawns for a single player at `skill` in the
/// flag family `family`, `None` when the map holds no such thing.
#[must_use]
#[inline]
pub fn start_thing(map: &Map, family: FlagFamily, skill: Skill) -> Option<usize> {
    specials::start_thing(map, family, skill)
}

/// Whether a thing carrying `flags` spawns for a single player at `skill`
/// in the flag family `family`: its skill bit set, and the family's own
/// answer to "is this thing in the game at all".
#[must_use]
#[inline]
pub fn thing_spawns(flags: u32, family: FlagFamily, skill: Skill) -> bool {
    specials::spawns(flags, family, skill)
}

/// The sectors and lines `map`'s door, lift and teleport actions work on,
/// reading its lines and things in the flag family `family`.
///
/// Nothing is walked here: the answer is what the map's specials say, at
/// [`Skill::Medium`] - the skill every playability tool defaults to. Door and
/// lift targets do not depend on the skill at all; a teleport's destination
/// sector does, because only a destination thing that spawns at the analysed
/// skill lands a player. A teleport line with no such thing is still a row,
/// with [`Teleporter::destination`] `None`.
///
/// A line whose special the tables read as a door or a lift but that resolves
/// to no sector at all is in neither set: like the walk, this read reports only
/// actions with something to act on, and [`analyze_playability`] is the one
/// that warns about the line.
#[must_use]
#[inline]
pub fn feature_targets(map: &Map, family: FlagFamily) -> FeatureTargets {
    let inventory = specials::read_inventory(map, family, Skill::Medium);
    let mut found = FeatureTargets {
        doors: inventory.door_sectors.clone(),
        lifts: inventory.lift_sectors.clone(),
        ..FeatureTargets::default()
    };
    for feature in &inventory.features {
        match feature.effect {
            Effect::Door => {
                found.door_lines.insert(feature.line);
                found.doors.extend(feature.targets.iter().copied());
            }
            Effect::Lift => {
                found.lift_lines.insert(feature.line);
                found.lifts.extend(feature.targets.iter().copied());
            }
            Effect::Teleport => found.teleporters.push(Teleporter {
                line: feature.line,
                destination: feature.destination,
            }),
        }
    }
    found
}

/// The exit rows: every exit line the map carries, with whether the walk
/// reached a side the action works from.
fn exit_report(inventory: &Inventory, walk: &Run) -> Vec<ExitInfo> {
    inventory
        .exits
        .iter()
        .map(|exit| ExitInfo {
            line: exit.line,
            special: exit.special,
            secret: exit.secret,
            reachable: touches(&exit.triggers, &walk.reachable),
        })
        .collect()
}

/// The key rows: every key that spawns at the analysed skill, with the
/// sector it stands in and whether the walk reached it.
fn key_report(inventory: &Inventory, walk: &Run) -> Vec<KeyInfo> {
    inventory
        .keys
        .iter()
        .map(|key| KeyInfo {
            thing: key.thing,
            color: key.color,
            sector: key.sector,
            reachable: key
                .sector
                .is_some_and(|sector| walk.reachable.contains(&sector)),
        })
        .collect()
}

/// The lock rows: every locked line, with whether its door was opened.
fn lock_report(inventory: &Inventory, walk: &Run) -> Vec<LockInfo> {
    inventory
        .locks
        .iter()
        .map(|lock| LockInfo {
            line: lock.line,
            color: lock.color,
            opened: lock_opened(inventory, walk, lock),
        })
        .collect()
}

/// Whether the walk ends with `lock`'s door up: the line's own feature was
/// worked, or every sector that feature raises already stands raised, which
/// is how the room-facing pair of one built door reads from the side nobody
/// pressed. A lock whose feature raised nothing stays shut.
fn lock_opened(inventory: &Inventory, walk: &Run, lock: &LockLine) -> bool {
    if walk.opened.contains(&lock.feature) {
        return true;
    }
    inventory
        .features
        .get(lock.feature)
        .filter(|feature| feature.effect == Effect::Door)
        .is_some_and(|feature| {
            !feature.targets.is_empty()
                && feature
                    .targets
                    .iter()
                    .all(|sector| walk.state.open_doors.contains(sector))
        })
}

/// The door sectors no opened feature ever raised.
fn never_opened(inventory: &Inventory, walk: &Run) -> Vec<usize> {
    inventory
        .door_sectors
        .iter()
        .filter(|sector| !walk.state.open_doors.contains(sector))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests;
