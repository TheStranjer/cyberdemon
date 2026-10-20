//! The sector movement graph: the topology a player crosses, the heights a
//! sector can present, and the player-shaped tests that decide whether a line
//! lets them through.
//!
//! A [`World`] is built once per walk from a decoded [`Map`] and the
//! [`Inventory`] of features read from it. It holds the two-sided lines as a
//! per-sector list of [`Crossing`]s, the heights as stored, and the two
//! heights the engine would move a sector to: the ceiling a closed door rises
//! to, and the floor a lift platform drops to. [`World::pass`] answers the one
//! question the walk asks, and it answers it in one direction at a time,
//! because a step up is not a step down.
//!
//! Nothing here reads a special number or a key: [`super::specials`] has
//! already said which sectors are doors and which are platforms.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::reach::Run;
use super::specials::Inventory;
use super::{CLOSED_DOOR_TOLERANCE, DOOR_HEADROOM, MAX_STEP_UP, PLAYER_HEIGHT};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::geometry::linedef_length;
use crate::wad::map::model::{Linedef, Map};

/// The `blocking` linedef bit, the same value in both flag families: nothing
/// gets through, player or monster.
const BLOCKING: u32 = 1_u32;
/// The `Hexen` `blockplayers` bit: the player cannot cross, monsters can.
const BLOCK_PLAYERS: u32 = 0x4000;
/// The `Hexen` `blockeverything` bit.
const BLOCK_EVERYTHING: u32 = 0x8000;

/// A sector's floor and ceiling heights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Height {
    /// The floor a player stands on.
    pub(super) floor: i32,
    /// The ceiling they must fit under.
    pub(super) ceiling: i32,
}

impl Height {
    /// This shape with its floor moved to `floor`.
    const fn at_floor(self, floor: i32) -> Self {
        Self { floor, ..self }
    }

    /// This shape with its ceiling moved to `ceiling`.
    const fn at_ceiling(self, ceiling: i32) -> Self {
        Self { ceiling, ..self }
    }
}

/// One direction of one two-sided line: a player in `from` stepping into `to`
/// across `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Crossing {
    /// The sector the player leaves.
    pub(super) from: usize,
    /// The sector they reach.
    pub(super) to: usize,
    /// The linedef between them.
    pub(super) line: usize,
}

/// What a crossing came up against, when it did not come out `Ok`. The variants
/// are ordered from the most encouraging to the least, so a crossing with more
/// than one shape to try reports the closest call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Pass {
    /// The player fits and can step through.
    Ok,
    /// The line blocks everything.
    Blocked,
    /// The step up is more than the player can walk.
    TooHigh,
    /// The gap across the line is shorter than the player.
    TooTight,
    /// The line is too short for the player to fit through.
    TooNarrow,
}

/// The sectors the opened features have moved: the state a walk ended in.
#[derive(Clone, Debug, Default)]
pub(super) struct Opened {
    /// Door sectors whose ceiling has risen to its open height.
    pub(super) open_doors: BTreeSet<usize>,
    /// Lift platforms that may ride at their low floor.
    pub(super) low_lifts: BTreeSet<usize>,
}

/// The sector a line side opens into: `None` when the side is absent, its
/// sidedef index dangles, or that sidedef's sector index dangles.
pub(super) fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sidedef = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
}

/// The two sectors `line` joins, `None` when either side fails to resolve or
/// both sides share one sector.
fn sides(map: &Map, line: &Linedef) -> Option<(usize, usize)> {
    let front = side_sector(map, line.front)?;
    let back = side_sector(map, line.back)?;
    (front != back).then_some((front, back))
}

/// Whether nothing can cross `line`: the shared `blocking` bit, plus the two
/// `Hexen` bits that keep players out. The `blockmonsters` bit is deliberately
/// not among them: a player walks past a wall that only holds monsters.
const fn impassable(flags: u32, family: FlagFamily) -> bool {
    if flags & BLOCKING == BLOCKING {
        return true;
    }
    matches!(family, FlagFamily::Hexen)
        && (flags & BLOCK_PLAYERS == BLOCK_PLAYERS || flags & BLOCK_EVERYTHING == BLOCK_EVERYTHING)
}

/// Whether a player standing in a sector of height `from` can step into one of
/// height `to`: the step up, and the gap between the two floors and the two
/// ceilings. The far sector's own depth needs no separate test - the gap is
/// never wider than it is.
fn standing(from: Height, to: Height) -> Pass {
    if to.floor.saturating_sub(from.floor) > MAX_STEP_UP {
        return Pass::TooHigh;
    }
    let gap = from
        .ceiling
        .min(to.ceiling)
        .saturating_sub(from.floor.max(to.floor));
    if gap < PLAYER_HEIGHT {
        return Pass::TooTight;
    }
    Pass::Ok
}

/// Whether a sector is a door at rest: its ceiling sits at or near its floor,
/// so nobody stands in it until something raises the ceiling.
const fn closed(height: Height) -> bool {
    height.ceiling.saturating_sub(height.floor) <= CLOSED_DOOR_TOLERANCE
}

/// The lowest ceiling over `neighbours`, the height a vanilla door rises to,
/// `None` when there is no neighbour to measure against.
fn lowest_ceiling(stored: &[Height], neighbours: &[usize]) -> Option<i32> {
    neighbours
        .iter()
        .filter_map(|index| stored.get(*index))
        .map(|height| height.ceiling)
        .min()
}

/// The lowest floor over `neighbours`, the height a vanilla lift drops to.
fn lowest_floor(stored: &[Height], neighbours: &[usize]) -> Option<i32> {
    neighbours
        .iter()
        .filter_map(|index| stored.get(*index))
        .map(|height| height.floor)
        .min()
}

/// Both directions of every two-sided line, grouped by the sector they leave
/// from and kept in linedef order. Lines whose two sides share a sector, and
/// lines whose references dangle, bound nothing and are left out.
fn passage_table(map: &Map) -> Vec<Vec<Crossing>> {
    let mut passages: Vec<Vec<Crossing>> = map.sectors.iter().map(|_| Vec::new()).collect();
    for (index, line) in map.linedefs.iter().enumerate() {
        let Some((front, back)) = sides(map, line) else {
            continue;
        };
        if let Some(out) = passages.get_mut(front) {
            out.push(Crossing {
                from: front,
                to: back,
                line: index,
            });
        }
        if let Some(around) = passages.get_mut(back) {
            around.push(Crossing {
                from: back,
                to: front,
                line: index,
            });
        }
    }
    passages
}

/// The sectors each sector's crossings lead to, deduplicated and in order, for
/// the door and lift height rules.
fn neighbour_table(passages: &[Vec<Crossing>]) -> Vec<Vec<usize>> {
    passages
        .iter()
        .map(|crossings| {
            let mut around: Vec<usize> = crossings.iter().map(|crossing| crossing.to).collect();
            around.sort_unstable();
            around.dedup();
            around
        })
        .collect()
}

/// The ceiling each closed door sector rises to: the lowest neighbouring
/// ceiling less the headroom a vanilla door leaves at the top.
fn door_ceilings(
    stored: &[Height],
    neighbours: &[Vec<usize>],
    inventory: &Inventory,
) -> Vec<Option<i32>> {
    stored
        .iter()
        .enumerate()
        .map(|(sector, height)| {
            if !inventory.door_sectors.contains(&sector) || !closed(*height) {
                return None;
            }
            let around = neighbours.get(sector)?.as_slice();
            lowest_ceiling(stored, around).map(|lowest| lowest.saturating_sub(DOOR_HEADROOM))
        })
        .collect()
}

/// The floor each lift platform drops to: its lowest neighbouring floor, and
/// only when that is genuinely below where the platform stands.
fn plat_floors(
    stored: &[Height],
    neighbours: &[Vec<usize>],
    inventory: &Inventory,
) -> Vec<Option<i32>> {
    stored
        .iter()
        .enumerate()
        .map(|(sector, height)| {
            if !inventory.lift_sectors.contains(&sector) {
                return None;
            }
            let around = neighbours.get(sector)?.as_slice();
            lowest_floor(stored, around).filter(|low| *low < height.floor)
        })
        .collect()
}

/// The topology and heights one reachability walk reads.
#[derive(Clone, Debug)]
pub(super) struct World<'map> {
    /// The map being analysed.
    map: &'map Map,
    /// The flag family its lines are stored in.
    family: FlagFamily,
    /// The shortest line this world lets a player cross. The wider walk that
    /// goes hunting for narrow lines passes `0`, which asks only that the line
    /// can be measured at all.
    min_width: i32,
    /// Per sector, the crossings out of it, in linedef order.
    passages: Vec<Vec<Crossing>>,
    /// Per sector, the heights as stored.
    stored: Vec<Height>,
    /// Per sector, the ceiling a closed door rises to once opened.
    open_ceiling: Vec<Option<i32>>,
    /// Per sector, the floor a lift platform drops to once called.
    low_floor: Vec<Option<i32>>,
}

impl<'map> World<'map> {
    /// Folds `map` into the graph a player may cross at `min_width`, marking
    /// the sectors `inventory` says are doors and platforms.
    pub(super) fn new(
        map: &'map Map,
        family: FlagFamily,
        inventory: &Inventory,
        min_width: i32,
    ) -> Self {
        let stored: Vec<Height> = map
            .sectors
            .iter()
            .map(|sector| Height {
                floor: sector.floor_height,
                ceiling: sector.ceiling_height,
            })
            .collect();
        let passages = passage_table(map);
        let neighbours = neighbour_table(&passages);
        let open_ceiling = door_ceilings(&stored, &neighbours, inventory);
        let low_floor = plat_floors(&stored, &neighbours, inventory);
        Self {
            map,
            family,
            min_width,
            passages,
            stored,
            open_ceiling,
            low_floor,
        }
    }

    /// The crossings out of `sector`, in linedef order.
    pub(super) fn passages(&self, sector: usize) -> &[Crossing] {
        self.passages.get(sector).map_or(&[], Vec::as_slice)
    }

    /// Whether a player in the near sector can cross `crossing` with the
    /// features in `opened` fired. The answer is directional, and a lift
    /// platform is tried at both of its heights.
    pub(super) fn pass(&self, crossing: Crossing, opened: &Opened) -> Pass {
        let Some(line) = self.map.linedefs.get(crossing.line) else {
            return Pass::Blocked;
        };
        if impassable(line.flags, self.family) {
            return Pass::Blocked;
        }
        let heights = self.cross_heights(crossing, opened);
        if heights != Pass::Ok {
            return heights;
        }
        if self.wide_enough(crossing.line) {
            Pass::Ok
        } else {
            Pass::TooNarrow
        }
    }

    /// The lines too short for a player whose two sides the walk reached by
    /// other means: the ways through the map one can see and not take.
    pub(super) fn narrow_lines(&self, run: &Run) -> Vec<usize> {
        self.map
            .linedefs
            .iter()
            .enumerate()
            .filter(|entry| self.squeezed(entry.0, entry.1, run))
            .map(|entry| entry.0)
            .collect()
    }

    /// Whether `line` is the one a player would take if they could: it joins
    /// two sectors the run reached, its heights would let them through, and it
    /// is shorter than they are wide.
    fn squeezed(&self, index: usize, line: &Linedef, run: &Run) -> bool {
        let Some((from, to)) = sides(self.map, line) else {
            return false;
        };
        let crossing = Crossing {
            from,
            to,
            line: index,
        };
        self.too_short(crossing)
            && run.reachable.contains(&from)
            && run.reachable.contains(&to)
            && self.cross_heights(crossing, &run.state) == Pass::Ok
    }

    /// Whether the heights on either side of `crossing` let the player
    /// through, reporting the most encouraging failure when none do. A lift
    /// platform offers two shapes on each side, and the player takes whichever
    /// of the four combinations fits.
    fn cross_heights(&self, crossing: Crossing, opened: &Opened) -> Pass {
        let froms = self.shapes(crossing.from, opened);
        let tos = self.shapes(crossing.to, opened);
        froms
            .into_iter()
            .flat_map(|from| tos.into_iter().map(move |to| standing(from, to)))
            .min()
            .unwrap_or(Pass::Blocked)
    }

    /// The one or two shapes `sector` can present: its stored height with an
    /// opened door's raised ceiling, plus a called lift platform's low floor.
    fn shapes(&self, sector: usize, opened: &Opened) -> [Height; 2] {
        let Some(stored) = self.stored.get(sector).copied() else {
            return [Height::default(); 2];
        };
        let raised = self
            .open_ceiling
            .get(sector)
            .copied()
            .flatten()
            .filter(|_| opened.open_doors.contains(&sector));
        let high = raised.map_or(stored, |ceiling| stored.at_ceiling(ceiling));
        let low = opened
            .low_lifts
            .contains(&sector)
            .then(|| self.low_floor.get(sector).copied().flatten())
            .flatten();
        [high, low.map_or(high, |floor| high.at_floor(floor))]
    }

    /// Whether `crossing` is shorter than a player needs to fit through.
    fn too_short(&self, crossing: Crossing) -> bool {
        let wanted = u64::try_from(super::MIN_LINE_WIDTH).unwrap_or(u64::MAX);
        self.length(crossing.line)
            .is_some_and(|length| length < wanted)
    }

    /// Whether `line` is at least as long as this world asks for: a line that
    /// cannot be measured is no way through.
    fn wide_enough(&self, line: usize) -> bool {
        if self.min_width <= 0_i32 {
            return self.length(line).is_some();
        }
        let wanted = u64::try_from(self.min_width).unwrap_or(u64::MAX);
        self.length(line).is_some_and(|length| length >= wanted)
    }

    /// The length of `line` in map units, `None` when a vertex reference
    /// dangles and it cannot be measured.
    fn length(&self, line: usize) -> Option<u64> {
        linedef_length(self.map, line)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK_EVERYTHING, BLOCK_PLAYERS, BLOCKING, Crossing, Height, Opened, Pass, World, closed,
        impassable, lowest_ceiling, lowest_floor, neighbour_table, passage_table, side_sector,
        sides, standing,
    };
    use crate::wad::map::common::fixtures::{name, room, side, spawn, two_room_map, vertex};
    use crate::wad::map::flags::{FlagFamily, Skill};
    use crate::wad::map::playability::MIN_LINE_WIDTH;
    use crate::wad::map::playability::reach::Run;
    use crate::wad::map::playability::specials::read_inventory;
    use crate::wad::map::{Linedef, Map, MapFormat, Sector};

    /// A one-sector map whose two linedefs are `front`-and-`back` sided: the
    /// first two-sided, the second missing its back side.
    fn single_sided_pair() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![vertex(0, 0), vertex(128, 0)];
        map.linedefs = vec![
            Linedef {
                v1: 0,
                v2: 1,
                flags: 4,
                front: Some(0),
                back: Some(1),
                ..Default::default()
            },
            Linedef {
                v1: 0,
                v2: 1,
                flags: 4,
                front: Some(0),
                ..Default::default()
            },
        ];
        map.sidedefs = vec![side("-", "-", "-", 0), side("-", "-", "-", 0)];
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    #[test]
    fn side_sector_needs_both_ends_of_the_reference_chain() {
        let map = two_room_map();
        assert_eq!(side_sector(&map, Some(0)), Some(0), "the hall side");
        assert_eq!(side_sector(&map, Some(3)), Some(1), "the back room side");
        assert_eq!(side_sector(&map, None), None, "no side, no sector");
        assert_eq!(side_sector(&map, Some(99)), None, "a dangling sidedef");
        let mut broken = map;
        broken.sidedefs.get_mut(0).expect("sidedef 0").sector = 99;
        assert_eq!(side_sector(&broken, Some(0)), None, "a dangling sector");
    }

    #[test]
    fn sides_wants_two_different_sectors() {
        let map = two_room_map();
        let divider = map.linedefs.get(2).expect("the step line");
        assert_eq!(
            sides(&map, divider),
            Some((0_usize, 1_usize)),
            "front then back"
        );
        let wall = map.linedefs.first().expect("the west wall");
        assert_eq!(sides(&map, wall), None, "a one-sided line joins nothing");
    }

    #[test]
    fn blocked_lines_are_blocked_in_their_own_family() {
        assert!(impassable(BLOCKING, FlagFamily::Doom), "the shared bit");
        assert!(impassable(BLOCKING, FlagFamily::Hexen), "which Hexen keeps");
        assert!(
            !impassable(2_u32, FlagFamily::Doom),
            "blockmonsters does not stop the player"
        );
        assert!(!impassable(0_u32, FlagFamily::Hexen), "an open line");
        assert!(
            impassable(BLOCK_PLAYERS, FlagFamily::Hexen),
            "blockplayers keeps the player out"
        );
        assert!(
            impassable(BLOCK_EVERYTHING, FlagFamily::Hexen),
            "and blockeverything more so"
        );
        assert!(
            !impassable(BLOCK_PLAYERS, FlagFamily::Doom),
            "which the Doom family does not define"
        );
    }

    #[test]
    fn standing_needs_a_step_a_gap_and_room_to_stand_in() {
        let flat = Height {
            floor: 0,
            ceiling: 128,
        };
        assert_eq!(standing(flat, flat), Pass::Ok, "level ground");
        let ledge = Height {
            floor: 24,
            ceiling: 152,
        };
        assert_eq!(standing(flat, ledge), Pass::Ok, "the tallest walkable step");
        let beyond = Height {
            floor: 25,
            ceiling: 153,
        };
        assert_eq!(standing(flat, beyond), Pass::TooHigh, "one unit past it");
        let shallow = Height {
            floor: 0,
            ceiling: 48,
        };
        assert_eq!(
            standing(flat, shallow),
            Pass::TooTight,
            "a 48-unit sector is shallower than the player, and the gap across the line can \
             never be wider than that"
        );
        let tall_enough = Height {
            floor: 0,
            ceiling: 56,
        };
        assert_eq!(
            standing(flat, tall_enough),
            Pass::Ok,
            "56 units is exactly enough to stand in"
        );
    }

    #[test]
    fn a_drop_is_never_too_high_but_still_needs_its_gap() {
        let high = Height {
            floor: 512,
            ceiling: 640,
        };
        let deep = Height {
            floor: 0,
            ceiling: 640,
        };
        assert_eq!(standing(high, deep), Pass::Ok, "any depth falls free");
        assert_eq!(standing(deep, high), Pass::TooHigh, "and never climbs");
        let shallow = Height {
            floor: 0,
            ceiling: 512,
        };
        assert_eq!(
            standing(high, shallow),
            Pass::TooTight,
            "but the pit has to be tall enough to catch the player at the lip"
        );
    }

    #[test]
    fn a_shut_sector_is_a_door_at_rest() {
        let shut = Height {
            floor: 0,
            ceiling: 8,
        };
        assert!(closed(shut), "eight units of gap is a slab");
        assert!(closed(Height {
            floor: 16,
            ceiling: 16,
        }));
        assert!(
            !closed(Height {
                floor: 0,
                ceiling: 9,
            }),
            "nine units of gap is a room, not a shut door"
        );
    }

    #[test]
    fn the_lowest_neighbour_answers_the_door_and_lift_rules() {
        let stored = [
            Height {
                floor: 0,
                ceiling: 128,
            },
            Height {
                floor: 64,
                ceiling: 96,
            },
            Height {
                floor: 8,
                ceiling: 256,
            },
        ];
        let around = [0_usize, 1, 2];
        assert_eq!(
            lowest_ceiling(&stored, &around),
            Some(96_i32),
            "the shortest ceiling"
        );
        assert_eq!(
            lowest_floor(&stored, &around),
            Some(0_i32),
            "the lowest floor"
        );
        assert_eq!(lowest_ceiling(&stored, &[]), None, "no neighbour, no rule");
        assert_eq!(
            lowest_floor(&stored, &[9]),
            None,
            "and a dangling index is no neighbour"
        );
    }

    #[test]
    fn the_passage_table_lists_both_sides_of_every_real_line() {
        let map = two_room_map();
        let passages = passage_table(&map);
        assert_eq!(
            passages.first().map(Vec::len),
            Some(1),
            "the hall crosses into the back room once"
        );
        assert_eq!(
            passages.get(1).and_then(|list| list.first()),
            Some(&Crossing {
                from: 1,
                to: 0,
                line: 2
            }),
            "and back over the same line"
        );
        assert_eq!(
            neighbour_table(&passages).first().map(Vec::as_slice),
            Some(&[1_usize][..]),
            "the neighbour table names the same sector"
        );
    }

    #[test]
    fn lines_into_the_same_sector_or_with_one_side_passage_nothing() {
        let map = single_sided_pair();
        let passages = passage_table(&map);
        assert!(
            passages.first().is_some_and(Vec::is_empty),
            "a line from a sector to itself, or with one side missing, is no way through"
        );
        assert_eq!(
            sides(&map, map.linedefs.first().expect("the two-sided line")),
            None,
            "and both sides share the only sector"
        );
    }

    #[test]
    fn the_opened_state_starts_empty() {
        let opened = Opened::default();
        assert!(
            opened.open_doors.is_empty() && opened.low_lifts.is_empty(),
            "the default state moves nothing"
        );
    }

    #[test]
    fn a_world_walks_only_its_own_crossings() {
        let map = two_room_map();
        let inventory = read_inventory(&map, FlagFamily::Doom, Skill::Medium);
        let strict = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        let up = Crossing {
            from: 0,
            to: 1,
            line: 2,
        };
        assert_eq!(
            strict.pass(up, &Opened::default()),
            Pass::TooHigh,
            "the fixture's 32-unit step is one too many"
        );
        assert_eq!(
            strict.pass(
                Crossing {
                    from: 1,
                    to: 0,
                    line: 2
                },
                &Opened::default()
            ),
            Pass::Ok,
            "and free on the way down"
        );
        assert_eq!(
            strict.pass(
                Crossing {
                    from: 0,
                    to: 1,
                    line: 99
                },
                &Opened::default()
            ),
            Pass::Blocked,
            "a line the map does not hold"
        );
        let loose = World::new(&map, FlagFamily::Doom, &inventory, 0_i32);
        assert_eq!(
            loose.pass(up, &Opened::default()),
            Pass::TooHigh,
            "relaxing the width does not relax the step"
        );
        assert!(
            loose
                .narrow_lines(&Run {
                    reachable: [0_usize, 1].into_iter().collect(),
                    ..Run::default()
                })
                .is_empty(),
            "the 256-unit divider is not a squeeze"
        );
    }

    #[test]
    fn a_world_of_nothing_crosses_nothing() {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.sectors = vec![Sector::default()];
        map.things = vec![spawn(1, 0, 0, 0)];
        let inventory = read_inventory(&map, FlagFamily::Doom, Skill::Easy);
        let world = World::new(&map, FlagFamily::Doom, &inventory, MIN_LINE_WIDTH);
        assert!(world.passages(0).is_empty(), "one sector crosses nothing");
        assert!(
            world.passages(9).is_empty(),
            "and a sector that is not there neither"
        );
        assert!(
            world.narrow_lines(&Run::default()).is_empty(),
            "with nothing reached there is nothing to report"
        );
    }
}
