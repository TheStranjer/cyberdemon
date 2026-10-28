//! The reader that folds a map's lines and things into an [`Inventory`].
//!
//! One pass over the linedefs, then one over the things: each line is looked
//! up in the family's table in [`crate::wad::map::specials::tables`], the
//! row's tag or `TID` is resolved against the map's sectors and its
//! destination things, and what comes out is filed as a feature, an exit, a
//! lock or a counted warning. The state the pass carries — the map, the
//! tables it consults and the accumulators it fills — is [`Reader`], and
//! [`Reader::finish`] is the last word: it turns the accumulated counts into
//! the trailing notes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::vec::Vec;

use super::{
    Effect, ExitLine, Feature, HEXEN_DESTINATIONS, Inventory, KeyColor, KeyThing, LockLine, Plan,
    Reader, Reading, SECTOR_TAG_ARG, TELEPORT_DEST, argument, key_color, openable, sectors_by_tag,
    spawns,
};
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::model::{Linedef, Map, Thing};
use crate::wad::map::specials::doom_row;
use crate::wad::map::specials::exit_for;
use crate::wad::map::specials::hexen_trigger;
use crate::wad::map::specials::lock_color;
use crate::wad::map::specials::tables::{HEXEN_ROWS, RowEffect, Trigger};

impl<'map> Reader<'map> {
    /// A reader over `map` for flag family `family` at `skill`: one
    /// [`SectorIndex`] folded in here serves every home-sector read of
    /// the whole pass.
    pub(super) fn new(map: &'map Map, family: FlagFamily, skill: Skill) -> Self {
        Self {
            map,
            geometry: SectorIndex::new(map),
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
    pub(super) fn spawns(&self, flags: u32) -> bool {
        spawns(flags, self.family, self.skill)
    }

    /// The sectors from which a player can work `line`: its front side for a
    /// use or shoot line, either side for a walk line, and none at all for a
    /// monster's own activation, which no player can set off.
    fn trigger_sectors(&self, line: &Linedef, trigger: Trigger) -> BTreeSet<usize> {
        match trigger {
            Trigger::Walk => self
                .map
                .side_sector(line.front)
                .into_iter()
                .chain(self.map.side_sector(line.back))
                .collect(),
            Trigger::Use | Trigger::Shoot => self.map.side_sector(line.front).into_iter().collect(),
            Trigger::Monster => BTreeSet::new(),
        }
    }

    /// Reads one linedef into the features, exits and unhandled counts.
    pub(super) fn run_line(&mut self, index: usize, line: &Linedef) {
        if line.special == 0_i32 {
            return;
        }
        if let Some(exit) = exit_for(self.family, line) {
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

    /// The feature `line` carries, or why it carries none.
    fn read_feature(&mut self, index: usize, line: &Linedef) -> Reading {
        match self.family {
            FlagFamily::Doom => self.doom_feature(index, line),
            FlagFamily::Hexen => self.hexen_feature(index, line),
        }
    }

    /// Reads a `Doom` family line against the `Doom` row table, or the row
    /// a `Boom` generalized door, locked door or lift decodes to.
    fn doom_feature(&mut self, index: usize, line: &Linedef) -> Reading {
        let Some(row) = doom_row(line.special) else {
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
    ///
    /// A door shutter ([`RowEffect::CloseDoor`]) is a door action a player
    /// never works to get through, so it is filed as known and ignored.
    /// A teleport line that lands the player nowhere is still kept as a
    /// feature, with [`Feature::destination`] `None`: it moves no sector and
    /// jumps no player, so the walk is unchanged by it, and
    /// [`feature_targets`](crate::wad::map::playability::feature_targets)
    /// can still name the pad whose destination thing is missing.
    fn build(&mut self, index: usize, line: &Linedef, plan: &Plan) -> Reading {
        let Some(effect) = openable(plan.effect) else {
            return Reading::Ignored;
        };
        let destination = self.destination_for(line, plan, effect);
        if effect == Effect::Teleport && destination.is_none() {
            self.warnings.push(format!(
                "line {index} special {} has no teleport destination",
                line.special
            ));
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
    /// asks for no narrowing at all. The home-sector read comes from the
    /// read's one [`SectorIndex`].
    fn in_tagged_sector(&self, spawned: &Thing, wanted: i32) -> Option<usize> {
        let sector = self
            .geometry
            .point_in_sector(i64::from(spawned.x), i64::from(spawned.y))?;
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
            return self.map.side_sector(line.back).into_iter().collect();
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
    pub(super) fn run_thing(&mut self, index: usize, spawn: &Thing) {
        let Some(color) = key_color(spawn.kind) else {
            return;
        };
        if !self.spawns(spawn.flags) {
            return;
        }
        let sector = self
            .geometry
            .point_in_sector(i64::from(spawn.x), i64::from(spawn.y));
        self.keys.push(KeyThing {
            thing: index,
            color,
            sector,
        });
    }

    /// The gathered inventory, its trailing notes counted per special and per
    /// unnamed lock code.
    pub(super) fn finish(self) -> Inventory {
        let mut warnings = self.warnings;
        for code in &self.unknown_locks {
            warnings.push(format!(
                "unknown lock {code} assumed openable: no key is needed to work it"
            ));
        }
        for (special, count) in &self.unhandled {
            warnings.push(format!(
                "unhandled special {special} on {count} line(s) is not modelled: the walk \
                 ignores the action and reads those lines by their flags alone"
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
