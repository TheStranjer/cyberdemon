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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::KeyColor;
use super::graph;
use crate::wad::map::flags::{ACTIVATION_MASK, ACTIVATION_SHIFT, FlagFamily, Skill};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::{Linedef, Map, Thing};

/// The `Doom` family's `notsingle` thing bit: a multiplayer-only spawn.
const NOT_SINGLE: u32 = 16_u32;
/// The `Hexen` family's `single` thing bit: the single-player game mode.
const SINGLE_PLAYER: u32 = 0x0100;
/// The thing type of the player 1 start.
const PLAYER_START: i32 = 1_i32;
/// The `Doom` family's teleport destination thing.
const TELEPORT_DEST: i32 = 14_i32;
/// The thing types a `Hexen` teleporter accepts as its destination.
const HEXEN_DESTINATIONS: &[i32] = &[14_i32, 9_043_i32, 9_044_i32];
/// The `Hexen` teleport argument that narrows the destination search to the
/// sectors carrying this tag, when it is non-zero.
const SECTOR_TAG_ARG: usize = 1_usize;
/// The `Hexen` `playeruse` activation field value: the one activation this
/// model reads as "press use on the front side".
const PLAYER_USE_FIELD: u32 = 1_u32;

/// What a table row says its special does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowEffect {
    /// Raises the ceiling of the sector behind the line: a manual door.
    ManualDoor,
    /// Raises the ceiling of every sector carrying the line's tag.
    TaggedDoor,
    /// Drops every sector carrying the line's tag to its lowest neighbour.
    Lift,
    /// Moves the player to a destination sector.
    Teleport,
}

/// How a player sets a line off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    /// A use line: only the sector on its front side works it.
    Use,
    /// A walk or shoot line: either side works it.
    Walk,
}

/// One row of the `Doom` family's line-special table.
#[derive(Clone, Copy, Debug)]
struct DoomRow {
    /// The [`Linedef::special`] the row answers for.
    special: i32,
    /// What the special does.
    effect: RowEffect,
    /// How the player sets it off.
    trigger: Trigger,
    /// The key colour the line demands.
    lock: Option<KeyColor>,
}

/// A `Doom` row: an unlocked special of `effect` worked `trigger`.
const fn doom(special: i32, effect: RowEffect, trigger: Trigger) -> DoomRow {
    DoomRow {
        special,
        effect,
        trigger,
        lock: None,
    }
}

/// A `Doom` row for a line locked to `lock`.
const fn doom_locked(special: i32, effect: RowEffect, trigger: Trigger, lock: KeyColor) -> DoomRow {
    DoomRow {
        special,
        effect,
        trigger,
        lock: Some(lock),
    }
}

/// One row of the `Doom` family's exit table, reused for the `Hexen` portals
/// whose numbers say whether they are the secret end.
#[derive(Clone, Copy, Debug)]
struct ExitRow {
    /// The [`Linedef::special`] that ends the level.
    special: i32,
    /// Whether it ends the level through the secret map.
    secret: bool,
    /// How the player reaches it.
    trigger: Trigger,
}

/// One row of the `Hexen` family's action table.
#[derive(Clone, Copy, Debug)]
struct HexenRow {
    /// The [`Linedef::special`] the row answers for.
    special: i32,
    /// What the special does.
    effect: RowEffect,
    /// The `args` slot holding the sector tag, or the destination `TID`.
    tag_arg: usize,
    /// The `args` slot holding a lock code, for a lockable action.
    lock_arg: Option<usize>,
}

/// A `Hexen` row: an unlocked action addressing `args[tag_arg]`.
const fn hexen(special: i32, effect: RowEffect, tag_arg: usize) -> HexenRow {
    HexenRow {
        special,
        effect,
        tag_arg,
        lock_arg: None,
    }
}

/// A `Hexen` row whose action is locked to the code in `args[lock_arg]`.
const fn hexen_locked(
    special: i32,
    effect: RowEffect,
    tag_arg: usize,
    lock_arg: usize,
) -> HexenRow {
    HexenRow {
        special,
        effect,
        tag_arg,
        lock_arg: Some(lock_arg),
    }
}

/// The `Doom` family's door, lift and teleport specials.
///
/// The `DR`/`D1` rows at the top open the sector behind the line; the rest
/// answer a tag. `W1`/`WR` lines work from either side, `S1`/`SR` lines from
/// the front only, and `G1` line 46 is shootable, which this model reads as
/// "either side". The locked walkovers (99, 133 to 137) are tagged doors
/// whose key the row names.
const DOOM_ROWS: &[DoomRow] = &[
    // Press the wall: the sector behind it is the doorway.
    doom(1, RowEffect::ManualDoor, Trigger::Use),
    doom(31, RowEffect::ManualDoor, Trigger::Use),
    doom(117, RowEffect::ManualDoor, Trigger::Use),
    doom(118, RowEffect::ManualDoor, Trigger::Use),
    doom_locked(26, RowEffect::ManualDoor, Trigger::Use, KeyColor::Blue),
    doom_locked(32, RowEffect::ManualDoor, Trigger::Use, KeyColor::Blue),
    doom_locked(27, RowEffect::ManualDoor, Trigger::Use, KeyColor::Yellow),
    doom_locked(34, RowEffect::ManualDoor, Trigger::Use, KeyColor::Yellow),
    doom_locked(28, RowEffect::ManualDoor, Trigger::Use, KeyColor::Red),
    doom_locked(33, RowEffect::ManualDoor, Trigger::Use, KeyColor::Red),
    // Open or raise every sector carrying the line's tag.
    doom(2, RowEffect::TaggedDoor, Trigger::Walk),
    doom(4, RowEffect::TaggedDoor, Trigger::Walk),
    doom(86, RowEffect::TaggedDoor, Trigger::Walk),
    doom(90, RowEffect::TaggedDoor, Trigger::Walk),
    doom(105, RowEffect::TaggedDoor, Trigger::Walk),
    doom(106, RowEffect::TaggedDoor, Trigger::Walk),
    doom(108, RowEffect::TaggedDoor, Trigger::Walk),
    doom(109, RowEffect::TaggedDoor, Trigger::Walk),
    doom(29, RowEffect::TaggedDoor, Trigger::Use),
    doom(61, RowEffect::TaggedDoor, Trigger::Use),
    doom(63, RowEffect::TaggedDoor, Trigger::Use),
    doom(103, RowEffect::TaggedDoor, Trigger::Use),
    doom(111, RowEffect::TaggedDoor, Trigger::Use),
    doom(112, RowEffect::TaggedDoor, Trigger::Use),
    doom(114, RowEffect::TaggedDoor, Trigger::Use),
    doom(115, RowEffect::TaggedDoor, Trigger::Use),
    doom(46, RowEffect::TaggedDoor, Trigger::Walk),
    doom_locked(99, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Blue),
    doom_locked(133, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Blue),
    doom_locked(134, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Red),
    doom_locked(135, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Red),
    doom_locked(136, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Yellow),
    doom_locked(137, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Yellow),
    // Drop every tagged sector to its lowest neighbouring floor.
    doom(10, RowEffect::Lift, Trigger::Walk),
    doom(21, RowEffect::Lift, Trigger::Use),
    doom(62, RowEffect::Lift, Trigger::Use),
    doom(88, RowEffect::Lift, Trigger::Walk),
    doom(120, RowEffect::Lift, Trigger::Walk),
    doom(121, RowEffect::Lift, Trigger::Walk),
    doom(122, RowEffect::Lift, Trigger::Use),
    doom(123, RowEffect::Lift, Trigger::Use),
    // Move the player to the first destination thing in a tagged sector.
    doom(39, RowEffect::Teleport, Trigger::Use),
    doom(97, RowEffect::Teleport, Trigger::Walk),
];

/// The `Doom` family's level ends.
const DOOM_EXITS: &[ExitRow] = &[
    ExitRow {
        special: 11,
        secret: false,
        trigger: Trigger::Use,
    },
    ExitRow {
        special: 51,
        secret: true,
        trigger: Trigger::Use,
    },
    ExitRow {
        special: 52,
        secret: false,
        trigger: Trigger::Walk,
    },
    ExitRow {
        special: 124,
        secret: true,
        trigger: Trigger::Walk,
    },
    ExitRow {
        special: 197,
        secret: false,
        trigger: Trigger::Walk,
    },
    ExitRow {
        special: 198,
        secret: true,
        trigger: Trigger::Walk,
    },
];

/// The `Hexen` family's door, lift and teleport actions. `Door_Close` is not
/// among them: a line that shuts a door is never the way through.
const HEXEN_ROWS: &[HexenRow] = &[
    hexen(11, RowEffect::TaggedDoor, 0),
    hexen(12, RowEffect::TaggedDoor, 0),
    hexen_locked(13, RowEffect::TaggedDoor, 0, 3),
    hexen_locked(14, RowEffect::TaggedDoor, 0, 3),
    hexen_locked(202, RowEffect::TaggedDoor, 0, 4),
    hexen(60, RowEffect::Lift, 0),
    hexen(62, RowEffect::Lift, 0),
    hexen(63, RowEffect::Lift, 0),
    hexen(64, RowEffect::Lift, 0),
    hexen(65, RowEffect::Lift, 0),
    hexen(172, RowEffect::Lift, 0),
    hexen(206, RowEffect::Lift, 0),
    hexen(207, RowEffect::Lift, 0),
    hexen(70, RowEffect::Teleport, 0),
    hexen(71, RowEffect::Teleport, 0),
];

/// The `Hexen` specials the analysis knows and deliberately does not model.
const HEXEN_IGNORED: &[i32] = &[10_i32];

/// The `Hexen` family's level ends: `Portal`, its instantaneous secret twin,
/// and `Teleport_NewMap`. Each is a `(special, secret)` pair; the side the
/// player reaches it from is the line's own activation field.
const HEXEN_EXITS: &[(i32, bool)] = &[(243_i32, false), (244_i32, true), (74_i32, false)];

/// The key items both families spawn: `ZDoom` keeps the `Doom` numbers. A
/// colour's card and skull answer for each other.
const KEY_THINGS: &[(i32, KeyColor)] = &[
    (5_i32, KeyColor::Blue),
    (40_i32, KeyColor::Blue),
    (13_i32, KeyColor::Red),
    (38_i32, KeyColor::Red),
    (6_i32, KeyColor::Yellow),
    (39_i32, KeyColor::Yellow),
];

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
    /// The sector a teleport lands the player in; `None` for every other
    /// effect.
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
fn spawns(flags: u32, family: FlagFamily, skill: Skill) -> bool {
    let bit = skill.bit(family);
    if bit == 0_u32 || flags & bit != bit {
        return false;
    }
    match family {
        FlagFamily::Doom => flags & NOT_SINGLE != NOT_SINGLE,
        FlagFamily::Hexen => flags & SINGLE_PLAYER == SINGLE_PLAYER,
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
    if field == PLAYER_USE_FIELD {
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

impl<'map> Reader<'map> {
    /// A reader over `map` for flag family `family` at `skill`.
    fn new(map: &'map Map, family: FlagFamily, skill: Skill) -> Self {
        Self {
            map,
            family,
            skill,
            tagged: sectors_by_tag(map),
            features: Vec::new(),
            exits: Vec::new(),
            locks: Vec::new(),
            keys: Vec::new(),
            door_sectors: BTreeSet::new(),
            lift_sectors: BTreeSet::new(),
            unhandled: BTreeMap::new(),
            unknown_locks: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    /// Whether a thing carrying `flags` spawns in this read.
    fn spawns(&self, flags: u32) -> bool {
        spawns(flags, self.family, self.skill)
    }

    /// The sectors from which a player can work `line`: its front side for a
    /// use line, either side for a walk or shoot line.
    fn trigger_sectors(&self, line: &Linedef, trigger: Trigger) -> BTreeSet<usize> {
        let mut sectors: BTreeSet<usize> = graph::side_sector(self.map, line.front)
            .into_iter()
            .collect();
        if trigger == Trigger::Walk {
            sectors.extend(graph::side_sector(self.map, line.back));
        }
        sectors
    }

    /// Reads one linedef into the features, exits and unhandled counts.
    fn run_line(&mut self, index: usize, line: &Linedef) {
        if line.special == 0_i32 {
            return;
        }
        if let Some(exit) = self.exit_rule(line) {
            let triggers = self.trigger_sectors(line, exit.trigger);
            self.exits.push(ExitLine {
                line: index,
                special: line.special,
                secret: exit.secret,
                triggers,
            });
            return;
        }
        match self.read_feature(index, line) {
            Reading::Feature(feature) => self.record(feature),
            Reading::Ignored => {}
            Reading::Unknown => {
                self.unhandled
                    .entry(line.special)
                    .and_modify(|count| *count = count.saturating_add(1))
                    .or_insert(1_usize);
            }
        }
    }

    /// The exit row `line` carries, if its special ends the level. The
    /// `Hexen` portals take their side from the line's own activation.
    fn exit_rule(&self, line: &Linedef) -> Option<ExitRow> {
        match self.family {
            FlagFamily::Doom => DOOM_EXITS
                .iter()
                .copied()
                .find(|row| row.special == line.special),
            FlagFamily::Hexen => HEXEN_EXITS
                .iter()
                .find(|entry| entry.0 == line.special)
                .map(|entry| ExitRow {
                    special: entry.0,
                    secret: entry.1,
                    trigger: hexen_trigger(line.flags),
                }),
        }
    }

    /// The feature `line` carries, or why it carries none.
    fn read_feature(&mut self, index: usize, line: &Linedef) -> Reading {
        match self.family {
            FlagFamily::Doom => self.doom_feature(index, line),
            FlagFamily::Hexen => self.hexen_feature(index, line),
        }
    }

    /// Reads a `Doom` family line against [`DOOM_ROWS`].
    fn doom_feature(&mut self, index: usize, line: &Linedef) -> Reading {
        let Some(row) = DOOM_ROWS.iter().find(|entry| entry.special == line.special) else {
            return Reading::Unknown;
        };
        let plan = Plan {
            effect: row.effect,
            trigger: row.trigger,
            lock: row.lock,
            tag: line.tag,
        };
        self.build(index, line, &plan)
    }

    /// Reads a `Hexen` family line against [`HEXEN_ROWS`]: the tag and the
    /// lock both ride in the row's argument slots, and the activation field
    /// decides which side of the line works it.
    fn hexen_feature(&mut self, index: usize, line: &Linedef) -> Reading {
        if HEXEN_IGNORED.contains(&line.special) {
            return Reading::Ignored;
        }
        let Some(row) = HEXEN_ROWS
            .iter()
            .find(|entry| entry.special == line.special)
        else {
            return Reading::Unknown;
        };
        let code = row.lock_arg.map_or(0_i32, |slot| argument(line, slot));
        let plan = Plan {
            effect: row.effect,
            trigger: hexen_trigger(line.flags),
            lock: self.hexen_lock(code),
            tag: argument(line, row.tag_arg),
        };
        self.build(index, line, &plan)
    }

    /// The colour a `Hexen` lock code demands, gathering the codes this table
    /// cannot name so the finished read can say so.
    fn hexen_lock(&mut self, code: i32) -> Option<KeyColor> {
        if code == 0_i32 {
            return None;
        }
        if let Some(known) = lock_color(code) {
            return Some(known);
        }
        self.unknown_locks.insert(code);
        None
    }

    /// Turns one planned action into the feature it describes for `line`, or
    /// into the warning the line deserves when it addresses nothing.
    fn build(&mut self, index: usize, line: &Linedef, plan: &Plan) -> Reading {
        let effect = match plan.effect {
            RowEffect::ManualDoor | RowEffect::TaggedDoor => Effect::Door,
            RowEffect::Lift => Effect::Lift,
            RowEffect::Teleport => Effect::Teleport,
        };
        let destination = self.destination_for(line, plan, effect);
        if effect == Effect::Teleport && destination.is_none() {
            self.warnings.push(format!(
                "line {index} special {} has no teleport destination",
                line.special
            ));
            return Reading::Ignored;
        }
        let targets = self.targets(index, line, plan);
        if effect != Effect::Teleport && targets.is_empty() {
            self.warnings.push(format!(
                "line {index} special {} acts on no sector",
                line.special
            ));
            return Reading::Ignored;
        }
        Reading::Feature(Feature {
            line: index,
            effect,
            triggers: self.trigger_sectors(line, plan.trigger),
            lock: plan.lock,
            targets,
            destination,
        })
    }

    /// The sector a teleport line lands the player in, `None` for every other
    /// effect, which needs no destination at all.
    fn destination_for(&self, line: &Linedef, plan: &Plan, effect: Effect) -> Option<usize> {
        if effect != Effect::Teleport {
            return None;
        }
        match self.family {
            FlagFamily::Doom => self.tagged_destination(plan.tag),
            FlagFamily::Hexen => self.tid_destination(line, plan.tag),
        }
    }

    /// The `Doom` lookup: the first destination thing standing in a sector
    /// that answers the line's tag.
    fn tagged_destination(&self, tag: i32) -> Option<usize> {
        self.map
            .things
            .iter()
            .filter(|spawned| spawned.kind == TELEPORT_DEST && self.spawns(spawned.flags))
            .find_map(|spawned| self.in_tagged_sector(spawned, tag))
    }

    /// The `Hexen` lookup: the first thing of a destination type carrying the
    /// line's `TID`, narrowed to the sectors answering `args[1]` when that
    /// argument is non-zero.
    fn tid_destination(&self, line: &Linedef, tid: i32) -> Option<usize> {
        let restrict = argument(line, SECTOR_TAG_ARG);
        self.map
            .things
            .iter()
            .filter(|spawned| {
                HEXEN_DESTINATIONS.contains(&spawned.kind)
                    && spawned.tid == tid
                    && self.spawns(spawned.flags)
            })
            .find_map(|spawned| self.in_tagged_sector(spawned, restrict))
    }

    /// The sector `thing` stands in, when it answers the tag `wanted`; `0`
    /// asks for no narrowing at all.
    fn in_tagged_sector(&self, spawned: &Thing, wanted: i32) -> Option<usize> {
        let sector = point_in_sector(self.map, i64::from(spawned.x), i64::from(spawned.y))?;
        if wanted == 0_i32 {
            return Some(sector);
        }
        let found = self.map.sectors.get(sector)?;
        (found.tag == wanted).then_some(sector)
    }

    /// The sectors a tagged action addresses: the ones carrying its tag, or
    /// the sector behind the line when the row is a manual door or the
    /// `Hexen` family's `args[0]` is empty. A zero tag in the `Doom` family is
    /// a tag like any other, so it answers every untagged sector at once - the
    /// collision vanilla mappers are warned about - and the read says so.
    fn targets(&mut self, index: usize, line: &Linedef, plan: &Plan) -> BTreeSet<usize> {
        if plan.effect == RowEffect::Teleport {
            return BTreeSet::new();
        }
        let behind_the_line = plan.effect == RowEffect::ManualDoor
            || (plan.tag == 0_i32 && self.family == FlagFamily::Hexen);
        if behind_the_line {
            return graph::side_sector(self.map, line.back)
                .into_iter()
                .collect();
        }
        if plan.tag == 0_i32 {
            self.warnings.push(format!(
                "line {index} special {} carries no tag: every untagged sector answers it",
                line.special
            ));
        }
        self.tagged.get(&plan.tag).cloned().unwrap_or_default()
    }

    /// Files one feature with its lock and the sectors it acts on.
    fn record(&mut self, feature: Feature) {
        let index = self.features.len();
        if let Some(color) = feature.lock {
            self.locks.push(LockLine {
                line: feature.line,
                color,
                feature: index,
            });
        }
        match feature.effect {
            Effect::Door => self.door_sectors.extend(feature.targets.iter().copied()),
            Effect::Lift => self.lift_sectors.extend(feature.targets.iter().copied()),
            Effect::Teleport => {}
        }
        self.features.push(feature);
    }

    /// Files one spawn: the keys that appear at this skill.
    fn run_thing(&mut self, index: usize, spawn: &Thing) {
        let Some(color) = key_color(spawn.kind) else {
            return;
        };
        if !self.spawns(spawn.flags) {
            return;
        }
        let sector = point_in_sector(self.map, i64::from(spawn.x), i64::from(spawn.y));
        self.keys.push(KeyThing {
            thing: index,
            color,
            sector,
        });
    }

    /// The gathered inventory, its trailing notes counted per special and per
    /// unnamed lock code.
    fn finish(self) -> Inventory {
        let mut warnings = self.warnings;
        for code in &self.unknown_locks {
            warnings.push(format!(
                "unknown lock {code} assumed openable: no key is needed to work it"
            ));
        }
        for (special, count) in &self.unhandled {
            warnings.push(format!(
                "unhandled special {special} on {count} line(s) is not modelled: the walk \
                 treats those lines as plain walls"
            ));
        }
        Inventory {
            features: self.features,
            exits: self.exits,
            locks: self.locks,
            keys: self.keys,
            door_sectors: self.door_sectors,
            lift_sectors: self.lift_sectors,
            warnings,
        }
    }
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
    let starts: Vec<usize> = map
        .things
        .iter()
        .enumerate()
        .filter(|entry| entry.1.kind == PLAYER_START && spawns(entry.1.flags, family, skill))
        .map(|entry| entry.0)
        .collect();
    let Some(first) = starts.first().copied() else {
        warnings.push(format!(
            "no player 1 start spawns at {} skill: nothing can be reached",
            skill.as_str()
        ));
        return None;
    };
    if starts.len() > 1_usize {
        warnings.push(format!(
            "{} player 1 starts spawn at {} skill: the walk begins at thing {first}",
            starts.len(),
            skill.as_str()
        ));
    }
    let spawn = map.things.get(first)?;
    let sector = point_in_sector(map, i64::from(spawn.x), i64::from(spawn.y));
    if sector.is_none() {
        warnings.push(format!(
            "player 1 start thing {first} stands outside every sector: nothing can be reached"
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

#[cfg(test)]
mod tests {
    use super::{
        DOOM_EXITS, DOOM_ROWS, HEXEN_ROWS, Reader, RowEffect, Trigger, argument, hexen_trigger,
        key_color, lock_color, spawns,
    };
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::name;
    use crate::wad::map::flags::{FlagFamily, Skill};
    use crate::wad::map::model::{Linedef, Map, Sector};
    use crate::wad::map::playability::KeyColor;

    /// The `Doom` row for `special`, for the table spot checks.
    fn row(special: i32) -> Option<&'static super::DoomRow> {
        DOOM_ROWS.iter().find(|entry| entry.special == special)
    }

    #[test]
    fn the_doom_table_spells_out_its_door_letters() {
        let manual = row(1).expect("DR open door");
        assert_eq!(
            manual.effect,
            RowEffect::ManualDoor,
            "the wall opens itself"
        );
        assert_eq!(manual.trigger, Trigger::Use, "and wants a press");
        let walk = row(2).expect("W1 open door");
        assert_eq!(walk.effect, RowEffect::TaggedDoor, "a tagged door");
        assert_eq!(walk.trigger, Trigger::Walk, "worked from either side");
        assert_eq!(
            row(46).expect("G1 open door").trigger,
            Trigger::Walk,
            "a shot reaches either side too"
        );
        assert_eq!(row(62).expect("SR lift").trigger, Trigger::Use);
        assert_eq!(row(88).expect("WR lift").trigger, Trigger::Walk);
        assert_eq!(
            row(10).expect("W1 lift").effect,
            RowEffect::Lift,
            "the same lift family"
        );
        assert_eq!(row(97).expect("WR teleport").effect, RowEffect::Teleport);
        assert_eq!(row(1).and_then(|entry| entry.lock), None, "no key");
    }

    #[test]
    fn the_doom_locks_answer_to_their_colours() {
        for special in [26_i32, 32_i32, 99_i32, 133_i32] {
            assert_eq!(
                row(special).and_then(|entry| entry.lock),
                Some(KeyColor::Blue),
                "special {special} is blue"
            );
        }
        for special in [28_i32, 33_i32, 134_i32, 135_i32] {
            assert_eq!(
                row(special).and_then(|entry| entry.lock),
                Some(KeyColor::Red),
                "special {special} is red"
            );
        }
        for special in [27_i32, 34_i32, 136_i32, 137_i32] {
            assert_eq!(
                row(special).and_then(|entry| entry.lock),
                Some(KeyColor::Yellow),
                "special {special} is yellow"
            );
        }
    }

    #[test]
    fn the_exit_tables_name_the_secret_ends() {
        for special in [51_i32, 124_i32, 198_i32] {
            assert!(
                DOOM_EXITS
                    .iter()
                    .any(|exit| exit.special == special && exit.secret),
                "{special} is the secret exit"
            );
        }
        for special in [11_i32, 52_i32, 197_i32] {
            assert!(
                DOOM_EXITS
                    .iter()
                    .any(|exit| exit.special == special && !exit.secret),
                "{special} ends the level"
            );
        }
    }

    #[test]
    fn the_hexen_table_puts_its_arguments_in_the_right_slots() {
        let locked = HEXEN_ROWS
            .iter()
            .find(|entry| entry.special == 13_i32)
            .expect("Door_LockedRaise");
        assert_eq!(locked.tag_arg, 0_usize, "arg0 is the tag");
        assert_eq!(locked.lock_arg, Some(3_usize), "arg3 is the lock");
        assert_eq!(
            HEXEN_ROWS
                .iter()
                .find(|entry| entry.special == 202_i32)
                .expect("Generic_Door")
                .lock_arg,
            Some(4_usize),
            "arg4 is its lock"
        );
        assert!(
            !HEXEN_ROWS.iter().any(|entry| entry.special == 10_i32),
            "Door_Close is not an action row at all"
        );
        assert!(
            super::HEXEN_IGNORED.contains(&10_i32),
            "and it is named as known-but-unmodelled, so it is never counted unhandled"
        );
    }

    #[test]
    fn arguments_read_by_slot_and_default_to_no_tag() {
        let line = Linedef {
            special: 202,
            args: [9_i32, 1_i32, 16_i32, 150_i32, 130_i32],
            ..Default::default()
        };
        assert_eq!(argument(&line, 0), 9_i32, "the tag slot reads");
        assert_eq!(argument(&line, 4), 130_i32, "and the lock slot");
        assert_eq!(argument(&line, 9), 0_i32, "past the end is no tag");
        assert_eq!(
            argument(&Linedef::default(), 0),
            0_i32,
            "and an empty line has none"
        );
    }

    #[test]
    fn only_playeruse_pins_a_hexen_line_to_its_front_side() {
        assert_eq!(hexen_trigger(0x400_u32), Trigger::Use, "field 1 is use");
        assert_eq!(hexen_trigger(0_u32), Trigger::Walk, "field 0 is cross");
        assert_eq!(
            hexen_trigger(0x1_400),
            Trigger::Walk,
            "field 5 is missilecross"
        );
        assert_eq!(
            hexen_trigger(0x600_u32),
            Trigger::Use,
            "use plus repeatspecial and monsteractivate"
        );
    }

    #[test]
    fn the_lock_and_key_tables_answer_for_each_other() {
        for code in [1_i32, 4_i32, 129_i32] {
            assert_eq!(lock_color(code), Some(KeyColor::Red), "code {code}");
        }
        for code in [2_i32, 5_i32, 130_i32] {
            assert_eq!(lock_color(code), Some(KeyColor::Blue), "code {code}");
        }
        for code in [3_i32, 6_i32, 131_i32] {
            assert_eq!(lock_color(code), Some(KeyColor::Yellow), "code {code}");
        }
        assert_eq!(lock_color(7_i32), None, "the extended list is not modelled");
        assert_eq!(lock_color(0_i32), None, "and zero is no lock at all");
        for kind in [5_i32, 40_i32] {
            assert_eq!(key_color(kind), Some(KeyColor::Blue), "type {kind}");
        }
        assert_eq!(key_color(13_i32), Some(KeyColor::Red));
        assert_eq!(key_color(38_i32), Some(KeyColor::Red));
        assert_eq!(key_color(6_i32), Some(KeyColor::Yellow));
        assert_eq!(key_color(39_i32), Some(KeyColor::Yellow));
        assert_eq!(key_color(2_005_i32), None, "a medikit is no key");
    }

    #[test]
    fn presence_reads_the_skill_bit_and_the_game_mode() {
        assert!(
            spawns(4_u32, FlagFamily::Doom, Skill::Hard),
            "the named skill"
        );
        assert!(
            !spawns(2_u32, FlagFamily::Doom, Skill::Hard),
            "a medium-only thing does not spawn on hard"
        );
        assert!(
            !spawns(8_u32, FlagFamily::Doom, Skill::Medium),
            "an ambush-only thing carries no skill bit and so never spawns"
        );
        assert!(
            !spawns(4_u32 | 16_u32, FlagFamily::Doom, Skill::Hard),
            "a notsingle thing is out of the single-player game"
        );
        assert!(
            spawns(4_u32 | 0x100_u32, FlagFamily::Hexen, Skill::Hard),
            "a Hexen thing needs its mode bit"
        );
        assert!(
            !spawns(4_u32, FlagFamily::Hexen, Skill::Hard),
            "and no mode bit means no spawn"
        );
    }

    #[test]
    fn reading_an_empty_map_invents_nothing() {
        let map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        let reader = Reader::new(&map, FlagFamily::Doom, Skill::Medium);
        assert!(!reader.spawns(8_u32), "no skill bit is never present");
        assert!(reader.tagged.is_empty(), "an empty map tags nothing");
    }

    #[test]
    fn a_tagged_action_finds_the_sectors_answering_its_tag() {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.sectors = vec![
            Sector {
                tag: 7_i32,
                ..Default::default()
            },
            Sector {
                tag: 7_i32,
                ..Default::default()
            },
            Sector::default(),
        ];
        let reader = Reader::new(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            reader.tagged.get(&7_i32),
            Some(&[0_usize, 1].into_iter().collect()),
            "both tagged sectors answer"
        );
        assert!(
            reader.tagged.contains_key(&0_i32),
            "and the untagged ones group under 0"
        );
    }
}
