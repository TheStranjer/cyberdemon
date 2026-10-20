//! The reachability fixpoint: what a player can stand in, and what working
//! what they can stand in lets them stand in next.
//!
//! [`fixpoint`] is the whole engine. It walks the sector graph from the player
//! start, collects the keys lying in the sectors it reached, opens every
//! feature whose trigger the walk has reached and whose key the walk now
//! holds, and walks again - until a round changes nothing. The loop is bounded
//! by the number of features, because every round either opens a feature or
//! ends the walk: no map can hang it.
//!
//! A teleporter is not a line a player walks through but an edge they get for
//! free once they can work it, so [`walk`] consults the opened features for
//! jumps out of every sector it visits. A lift platform is not a sector with
//! one height but a sector with two, which [`super::graph`] already knows: the
//! walk only asks about both.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::KeyColor;
use super::graph::{Opened, Pass, World};
use super::specials::{Effect, Feature, Inventory};

/// One fixpoint's answer.
#[derive(Clone, Debug, Default)]
pub(super) struct Run {
    /// The sectors a player can stand in.
    pub(super) reachable: BTreeSet<usize>,
    /// The indices into [`Inventory::features`] the player can work.
    pub(super) opened: BTreeSet<usize>,
    /// The sector state those features leave behind.
    pub(super) state: Opened,
}

/// Whether `reachable` holds any of `sectors`.
pub(super) fn touches(sectors: &BTreeSet<usize>, reachable: &BTreeSet<usize>) -> bool {
    sectors.iter().any(|sector| reachable.contains(sector))
}

/// Whether a player can work `feature`: they reach one of its trigger sides
/// and, for a locked line, they hold the key it asks for.
fn works(feature: &Feature, reachable: &BTreeSet<usize>, keys: &BTreeSet<KeyColor>) -> bool {
    if feature.lock.is_some_and(|color| !keys.contains(&color)) {
        return false;
    }
    touches(&feature.triggers, reachable)
}

/// The colours of the keys lying in the sectors of `reachable`. A key floating
/// outside every sector is out of everyone's reach.
fn held(inventory: &Inventory, reachable: &BTreeSet<usize>) -> BTreeSet<KeyColor> {
    inventory
        .keys
        .iter()
        .filter(|key| key.sector.is_some_and(|sector| reachable.contains(&sector)))
        .map(|key| key.color)
        .collect()
}

/// The sectors the features in `opened` have moved: opened doors rise, called
/// platforms drop, and a teleport moves no sector at all.
fn moved(inventory: &Inventory, opened: &BTreeSet<usize>) -> Opened {
    let mut state = Opened::default();
    for index in opened {
        let Some(feature) = inventory.features.get(*index) else {
            continue;
        };
        match feature.effect {
            Effect::Door => state.open_doors.extend(feature.targets.iter().copied()),
            Effect::Lift => state.low_lifts.extend(feature.targets.iter().copied()),
            Effect::Teleport => {}
        }
    }
    state
}

/// The sectors an opened teleporter worked from `sector` lands the player in.
/// A teleporter whose line resolved to no destination sector lands nowhere, so
/// it contributes nothing here.
fn jumps(inventory: &Inventory, opened: &BTreeSet<usize>, sector: usize) -> Vec<usize> {
    inventory
        .features
        .iter()
        .enumerate()
        .filter(|entry| lands(entry.0, entry.1, opened, sector))
        .filter_map(|entry| entry.1.destination)
        .collect()
}

/// Whether the feature at `index` teleports a player standing in `sector`:
/// it is a teleporter, it has been worked, and its line faces this sector.
fn lands(index: usize, feature: &Feature, opened: &BTreeSet<usize>, sector: usize) -> bool {
    feature.effect == Effect::Teleport
        && opened.contains(&index)
        && feature.triggers.contains(&sector)
}

/// One breadth-first pass over the sector graph with a fixed set of opened
/// features.
struct Walker<'ctx> {
    /// The sector graph and heights the player walks.
    world: &'ctx World<'ctx>,
    /// The map's openable features.
    inventory: &'ctx Inventory,
    /// The features this pass treats as worked.
    opened: &'ctx BTreeSet<usize>,
    /// The sectors those features moved.
    state: Opened,
}

impl Walker<'_> {
    /// The sectors one step out of `sector`: the crossings the player fits
    /// through and the teleporters they can work from there.
    fn onward(&self, sector: usize) -> Vec<usize> {
        let mut arrivals: Vec<usize> = self
            .world
            .passages(sector)
            .iter()
            .filter(|crossing| self.world.pass(**crossing, &self.state) == Pass::Ok)
            .map(|crossing| crossing.to)
            .collect();
        arrivals.extend(jumps(self.inventory, self.opened, sector));
        arrivals
    }
}

/// The sectors reachable from `start` with the features in `opened` fired:
/// every line a player can cross and every teleporter they can work, followed
/// outwards until they run out of map.
fn walk(
    world: &World<'_>,
    inventory: &Inventory,
    opened: &BTreeSet<usize>,
    start: usize,
) -> BTreeSet<usize> {
    let walker = Walker {
        world,
        inventory,
        opened,
        state: moved(inventory, opened),
    };
    let mut seen: BTreeSet<usize> = core::iter::once(start).collect();
    let mut queue = Vec::from([start]);
    while let Some(sector) = queue.pop() {
        claim(&mut seen, &mut queue, walker.onward(sector));
    }
    seen
}

/// Records every not-yet-seen sector of `arrivals` and queues it for a visit.
fn claim(seen: &mut BTreeSet<usize>, queue: &mut Vec<usize>, arrivals: Vec<usize>) {
    for to in arrivals {
        if seen.insert(to) {
            queue.push(to);
        }
    }
}

/// Opens every feature the walk can now work, reporting whether the set grew.
fn fire(inventory: &Inventory, reachable: &BTreeSet<usize>, opened: &mut BTreeSet<usize>) -> bool {
    let keys = held(inventory, reachable);
    let mut grew = false;
    for (index, feature) in inventory.features.iter().enumerate() {
        if !opened.contains(&index) && works(feature, reachable, &keys) {
            opened.insert(index);
            grew = true;
        }
    }
    grew
}

/// Walks `world` from `start` to the point where working what the player has
/// reached reaches nothing new, and reports both.
///
/// Without a `start` there is nobody to walk, and the answer is an empty run:
/// every sector unreachable, every feature shut.
pub(super) fn fixpoint(world: &World<'_>, inventory: &Inventory, start: Option<usize>) -> Run {
    let Some(first) = start else {
        return Run::default();
    };
    let mut opened = BTreeSet::new();
    let mut reachable = walk(world, inventory, &opened, first);
    for _round in 0..inventory.features.len().saturating_add(1) {
        let mut changed = fire(inventory, &reachable, &mut opened);
        let walked = walk(world, inventory, &opened, first);
        changed |= walked != reachable;
        reachable = walked;
        if !changed {
            break;
        }
    }
    Run {
        state: moved(inventory, &opened),
        reachable,
        opened,
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{fire, fixpoint, held, jumps, moved, touches, works};
    use crate::wad::map::common::fixtures::{name, spawn, vertex};
    use crate::wad::map::flags::{FlagFamily, Skill};
    use crate::wad::map::playability::graph::World;
    use crate::wad::map::playability::specials::{
        Effect, Feature, Inventory, KeyThing, read_inventory,
    };
    use crate::wad::map::playability::{KeyColor, MIN_LINE_WIDTH};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef};

    /// The sector set of `list`, for the assertions below.
    fn set(list: &[usize]) -> BTreeSet<usize> {
        list.iter().copied().collect()
    }

    /// The single colour set of `color`.
    fn one(color: KeyColor) -> BTreeSet<KeyColor> {
        core::iter::once(color).collect()
    }

    /// A door feature on sectors `targets`, worked from `triggers`, locked to
    /// `lock` when it names a colour.
    fn door(line: usize, triggers: &[usize], targets: &[usize], lock: Option<KeyColor>) -> Feature {
        Feature {
            line,
            effect: Effect::Door,
            triggers: set(triggers),
            lock,
            targets: set(targets),
            destination: None,
        }
    }

    /// A one-sided wall from `v1` to `v2` with sidedef `front`.
    fn wall(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided line from `v1` to `v2` with sidedefs `front` and `back`.
    fn join(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A three-box chain: a hall at 128 units of headroom, a shut door band,
    /// and a platform room 96 units up, each its own sector, joined by two
    /// full-height two-sided lines (indices 2 and 5).
    fn chain() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.things = vec![spawn(1, 64, 64, 0)];
        map.vertices = vec![
            vertex(0, 0),
            vertex(128, 0),
            vertex(192, 0),
            vertex(320, 0),
            vertex(320, 128),
            vertex(192, 128),
            vertex(128, 128),
            vertex(0, 128),
        ];
        let owners = [0_usize, 0, 0, 1, 0, 1, 1, 2, 1, 2, 2, 2];
        map.sidedefs = owners
            .iter()
            .map(|sector| Sidedef {
                sector: *sector,
                ..Default::default()
            })
            .collect();
        map.linedefs = vec![
            wall(0, 7, 0),
            wall(7, 6, 1),
            join(6, 1, 2, 3),
            wall(1, 0, 4),
            wall(6, 5, 5),
            join(5, 2, 6, 7),
            wall(2, 1, 8),
            wall(5, 4, 9),
            wall(4, 3, 10),
            wall(3, 2, 11),
        ];
        map.sectors = vec![
            Sector {
                floor_height: 0,
                ceiling_height: 128,
                ..Default::default()
            },
            Sector {
                floor_height: 0,
                ceiling_height: 0,
                ..Default::default()
            },
            Sector {
                floor_height: 96,
                ceiling_height: 224,
                ..Default::default()
            },
        ];
        map
    }

    /// The chain's features with nothing read from the map: a door on the band
    /// worked from the hall, and a lift on the platform worked from the band.
    fn chain_inventory() -> Inventory {
        Inventory {
            features: vec![door(2, &[0], &[1], None), door(5, &[1], &[2], None)],
            exits: Vec::new(),
            locks: Vec::new(),
            keys: Vec::new(),
            door_sectors: set(&[1, 2]),
            lift_sectors: set(&[2]),
            warnings: Vec::new(),
        }
    }

    /// The chain's features with the second one read as a lift, which is what
    /// the world's height rules need.
    fn chain_lift() -> Feature {
        Feature {
            effect: Effect::Lift,
            ..door(5, &[1], &[2], None)
        }
    }

    #[test]
    fn touches_asks_for_any_one_of_the_sectors() {
        assert!(touches(&set(&[3, 4]), &set(&[4, 5])), "one of them");
        assert!(!touches(&set(&[3, 4]), &set(&[5])), "none of them");
        assert!(
            !touches(&set(&[]), &set(&[0])),
            "an empty side never matches anything"
        );
    }

    #[test]
    fn a_locked_feature_waits_for_its_own_colour() {
        let blue_door = door(0, &[0], &[1], Some(KeyColor::Blue));
        assert!(
            !works(&blue_door, &set(&[0]), &BTreeSet::new()),
            "no key, no opening"
        );
        assert!(
            !works(&blue_door, &set(&[0]), &one(KeyColor::Red)),
            "the wrong key is no key"
        );
        assert!(
            works(&blue_door, &set(&[0]), &one(KeyColor::Blue)),
            "the blue card works the blue line"
        );
        assert!(
            !works(&blue_door, &set(&[1]), &one(KeyColor::Blue)),
            "and the key is no use from behind the door"
        );
    }

    #[test]
    fn held_keys_come_from_reachable_sectors_only() {
        let inventory = Inventory {
            features: Vec::new(),
            exits: Vec::new(),
            locks: Vec::new(),
            keys: vec![
                KeyThing {
                    thing: 0,
                    color: KeyColor::Blue,
                    sector: Some(0),
                },
                KeyThing {
                    thing: 1,
                    color: KeyColor::Red,
                    sector: Some(1),
                },
                KeyThing {
                    thing: 2,
                    color: KeyColor::Yellow,
                    sector: None,
                },
            ],
            door_sectors: BTreeSet::new(),
            lift_sectors: BTreeSet::new(),
            warnings: Vec::new(),
        };
        assert_eq!(held(&inventory, &set(&[0])), one(KeyColor::Blue));
        assert_eq!(
            held(&inventory, &set(&[0, 1])),
            [KeyColor::Blue, KeyColor::Red].into_iter().collect(),
            "two rooms, two colours"
        );
        assert!(
            held(&inventory, &set(&[])).is_empty(),
            "the void key and the unreachable key both stay out"
        );
    }

    #[test]
    fn opened_features_move_their_own_kind_of_sector() {
        let mut inventory = chain_inventory();
        inventory.features[1] = chain_lift();
        let state = moved(&inventory, &set(&[0, 1]));
        assert_eq!(state.open_doors, set(&[1]), "the door band rose");
        assert_eq!(state.low_lifts, set(&[2]), "and the platform dropped");
        let none = moved(&inventory, &BTreeSet::new());
        assert!(
            none.open_doors.is_empty() && none.low_lifts.is_empty(),
            "nothing opened moves nothing"
        );
        let dangling = moved(&inventory, &set(&[9]));
        assert!(
            dangling.open_doors.is_empty() && dangling.low_lifts.is_empty(),
            "and a feature the inventory does not hold is skipped"
        );
    }

    #[test]
    fn an_opened_teleporter_jumps_from_either_side_of_its_line() {
        let mut inventory = chain_inventory();
        inventory.features.push(Feature {
            line: 2,
            effect: Effect::Teleport,
            triggers: set(&[0, 1]),
            lock: None,
            targets: BTreeSet::new(),
            destination: Some(2),
        });
        let opened = set(&[2]);
        assert_eq!(
            jumps(&inventory, &opened, 0),
            vec![2_usize],
            "from the hall"
        );
        assert_eq!(
            jumps(&inventory, &opened, 1),
            vec![2_usize],
            "and from the band"
        );
        assert!(
            jumps(&inventory, &opened, 2).is_empty(),
            "not from the destination"
        );
        assert!(
            jumps(&inventory, &BTreeSet::new(), 0).is_empty(),
            "and not before the pad is walked on"
        );
    }

    #[test]
    fn the_fixpoint_needs_a_player_and_stops_without_one() {
        let map = chain();
        let inventory = read_inventory(&map, FlagFamily::Doom, Skill::Medium);
        let world = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        assert!(inventory.features.is_empty(), "the chain ships no specials");
        let run = fixpoint(&world, &inventory, Some(0));
        assert_eq!(
            run.reachable,
            set(&[0]),
            "the band is shut and nothing opens it"
        );
        assert!(run.opened.is_empty(), "and there is nothing to work");
        assert_eq!(
            fixpoint(&world, &inventory, None).reachable,
            set(&[]),
            "with no player there is no walk at all"
        );
    }

    #[test]
    fn two_rounds_of_working_and_reaching_open_the_chain() {
        let map = chain();
        let mut inventory = chain_inventory();
        inventory.features[1] = chain_lift();
        let world = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        let run = fixpoint(&world, &inventory, Some(0));
        assert_eq!(
            run.reachable,
            set(&[0, 1, 2]),
            "the door reaches the band, the band reaches the lift, the lift reaches the platform"
        );
        assert_eq!(run.opened, set(&[0, 1]), "both features fired");
        assert_eq!(run.state.open_doors, set(&[1]), "and the band stayed up");
    }

    #[test]
    fn a_key_behind_its_own_door_leaves_the_door_shut() {
        let map = chain();
        let mut inventory = chain_inventory();
        inventory.features[0] = door(2, &[0], &[1], Some(KeyColor::Blue));
        inventory.keys.push(KeyThing {
            thing: 7,
            color: KeyColor::Blue,
            sector: Some(2),
        });
        let world = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        let run = fixpoint(&world, &inventory, Some(0));
        assert_eq!(
            run.reachable,
            set(&[0]),
            "the blue card is behind the blue door"
        );
        let mut opened = run.opened.clone();
        assert!(
            !fire(&inventory, &run.reachable, &mut opened),
            "and a whole round fires nothing"
        );
        assert_eq!(opened, run.opened, "the set did not grow");
    }

    #[test]
    fn a_key_in_the_start_room_opens_the_door_in_one_round() {
        let map = chain();
        let mut inventory = chain_inventory();
        inventory.features[0] = door(2, &[0], &[1], Some(KeyColor::Blue));
        inventory.keys.push(KeyThing {
            thing: 0,
            color: KeyColor::Blue,
            sector: Some(0),
        });
        let world = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        let run = fixpoint(&world, &inventory, Some(0));
        assert_eq!(
            run.reachable,
            set(&[0, 1]),
            "card in hand, band open - and the platform room, which no lift was read \
             for this run, stays 96 units out of reach"
        );
    }
}
