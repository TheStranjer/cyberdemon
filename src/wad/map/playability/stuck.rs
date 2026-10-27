//! Why a thing cannot stand where it was placed: the pure box tests
//! behind `check_playability`' stuck list.
//!
//! [`stuck_things`] looks only at things that spawn at the analysed
//! [`Skill`]. Every one of them must stand in some sector or it is
//! [`StuckReason::OutsideMap`]; the ones that *body* - monsters, player
//! starts, and thing types the definitions do not know, which are treated
//! as monsters with a 20-unit radius - additionally get the engine's
//! axis-aligned square of side twice their radius tested: a one-sided or
//! blocking line running strictly through it means
//! [`StuckReason::InWall`], a sector shallower than the 56 units every
//! standing thing needs (the definition tables carry no heights, so the
//! vanilla collision height stands in for all of them) means
//! [`StuckReason::LowCeiling`], and two bodies whose squares strictly
//! overlap yield one [`StuckReason::Overlaps`] row naming both, reported
//! once, lower index first. Squares that only touch - an edge exactly on
//! another's edge - are neighbours, not a jam.

use alloc::vec::Vec;

use serde::Serialize;

use super::PLAYER_HEIGHT;
use super::inventory::spawns;
use crate::defs::{Category, Definitions, ThingDef};
use crate::wad::map::consts::UNKNOWN_THING_RADIUS;
use crate::wad::map::flags::{FlagFamily, Skill, blocks_players};
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::model::{Map, Thing};

/// Why one thing cannot stand where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum StuckReason {
    /// The point stands in no sector of the map.
    OutsideMap,
    /// A wall or blocking line runs strictly through the thing's square.
    InWall {
        /// The linedef passing through it.
        line: usize,
    },
    /// The sector's ceiling sits lower over the floor than the thing is tall.
    LowCeiling {
        /// The space the sector actually leaves, in map units.
        height: i32,
    },
    /// Another body's square strictly overlaps this one's.
    Overlaps {
        /// The other thing's index into [`Map::things`], always the higher
        /// one: each pair reports once, lower index first.
        other: usize,
    },
}

/// One thing that cannot stand, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Stuck {
    /// The index into [`Map::things`].
    pub thing: usize,
    /// What holds it.
    #[serde(flatten)]
    pub reason: StuckReason,
}

/// Reports every skill-relevant thing of `map` that cannot stand where it
/// is, read against the game definitions `defs` at `skill`.
///
/// The rows come out in thing order: each stuck thing's own reason first,
/// then one overlap row per jammed pair found among what is left standing.
#[must_use]
#[inline]
pub fn stuck_things(map: &Map, defs: &Definitions, skill: Skill) -> Vec<Stuck> {
    let family = map.flag_family();
    let mut report: Vec<Stuck> = Vec::new();
    let mut standing: Vec<(usize, Square)> = Vec::new();
    for (index, row) in map.things.iter().enumerate() {
        if !spawns(row.flags, family, skill) {
            continue;
        }
        let Some(sector) = point_in_sector(map, i64::from(row.x), i64::from(row.y)) else {
            report.push(Stuck {
                thing: index,
                reason: StuckReason::OutsideMap,
            });
            continue;
        };
        let Some(radius) = body_radius(defs, row.kind) else {
            continue;
        };
        let square = Square::of(row, radius);
        if let Some(line) = wall_through(map, family, &square) {
            report.push(Stuck {
                thing: index,
                reason: StuckReason::InWall { line },
            });
            continue;
        }
        let Some(depth) = sector_depth(map, sector) else {
            continue;
        };
        if depth < PLAYER_HEIGHT {
            report.push(Stuck {
                thing: index,
                reason: StuckReason::LowCeiling { height: depth },
            });
            continue;
        }
        standing.push((index, square));
    }
    report.extend(overlaps(&standing));
    report
}

/// The half-side of the body thing type `kind` needs room for, `None` for
/// a defined thing that is neither a monster nor a player start: items and
/// decorations have no standing test.
fn body_radius(defs: &Definitions, kind: i32) -> Option<i64> {
    let found = u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number));
    match found {
        None => Some(UNKNOWN_THING_RADIUS),
        Some(row) if is_body(row) => Some(i64::from(row.radius)),
        Some(_) => None,
    }
}

/// Whether a thing definition stands on its own two feet (or legs): the
/// [`Category::Monsters`] and [`Category::PlayerStarts`] groups.
const fn is_body(row: &ThingDef) -> bool {
    matches!(row.group, Category::Monsters | Category::PlayerStarts)
}

/// The headroom `sector` leaves between its floor and its ceiling, `None`
/// for a dangling sector reference.
fn sector_depth(map: &Map, sector: usize) -> Option<i32> {
    let row = map.sectors.get(sector)?;
    Some(row.ceiling_height.saturating_sub(row.floor_height))
}

/// The first linedef running strictly through `square`, checking every
/// one-sided line and every line whose bits stop a player in the map's
/// own flag family, in linedef order.
fn wall_through(map: &Map, family: FlagFamily, square: &Square) -> Option<usize> {
    map.linedefs.iter().enumerate().find_map(|(index, line)| {
        let solid = line.back.is_none() || blocks_players(line.flags, family);
        if !solid {
            return None;
        }
        let start = map.vertices.get(line.v1)?;
        let end = map.vertices.get(line.v2)?;
        let from = (i64::from(start.x), i64::from(start.y));
        let to = (i64::from(end.x), i64::from(end.y));
        pierces(square, from, to).then_some(index)
    })
}

/// The axis-aligned body square of `at` with half-side `radius`: the
/// engine's bounding box, which in `Doom` is a square of side 2 r.
#[derive(Clone, Copy, Debug)]
struct Square {
    /// West edge.
    min_x: i64,
    /// East edge.
    max_x: i64,
    /// South edge.
    min_y: i64,
    /// North edge.
    max_y: i64,
}

impl Square {
    /// The body square of thing `at` with half-side `radius`.
    fn of(at: &Thing, radius: i64) -> Self {
        let (x, y) = (i64::from(at.x), i64::from(at.y));
        Self {
            min_x: x.saturating_sub(radius),
            max_x: x.saturating_add(radius),
            min_y: y.saturating_sub(radius),
            max_y: y.saturating_add(radius),
        }
    }

    /// Whether `self` and `other` share ground strictly: touching edges
    /// are not an overlap.
    const fn overlaps(&self, other: &Self) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }
}

/// One parameter fraction of a segment, kept as an exact `i128` ratio with
/// a positive denominator.
#[derive(Clone, Copy, Debug)]
struct Frac {
    /// Numerator.
    num: i128,
    /// Denominator, always positive.
    den: i128,
}

impl Frac {
    /// The fraction `num / den`, its sign carried by the numerator.
    const fn new(num: i128, den: i128) -> Self {
        if den < 0_i128 {
            return Self {
                num: num.saturating_neg(),
                den: den.saturating_neg(),
            };
        }
        Self { num, den }
    }

    /// `0`.
    const fn zero() -> Self {
        Self {
            num: 0_i128,
            den: 1_i128,
        }
    }

    /// `1`.
    const fn one() -> Self {
        Self {
            num: 1_i128,
            den: 1_i128,
        }
    }

    /// The ordering of two fractions by cross-multiplication: both
    /// denominators are positive and every coordinate sits far inside
    /// `i64`, so the `i128` products cannot overflow.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let left = self.num.saturating_mul(other.den);
        let right = other.num.saturating_mul(self.den);
        left.cmp(&right)
    }
}

/// The open parameter interval `[lo, hi]` of a segment, clipped to the
/// box's inside by [`Span::clip_axis`].
#[derive(Clone, Copy, Debug)]
struct Span {
    /// The first surviving parameter.
    lo: Frac,
    /// The last surviving parameter.
    hi: Frac,
}

impl Span {
    /// The whole segment: parameters `0..=1`.
    const fn unit() -> Self {
        Self {
            lo: Frac::zero(),
            hi: Frac::one(),
        }
    }

    /// Drops every parameter whose `start + t * (stop - start)` coordinate
    /// is outside the open range `low..high`, reporting whether anything is
    /// left. A constant coordinate either lies strictly inside or the
    /// interval dies.
    fn clip_axis(&mut self, range: (i64, i64), axis: (i64, i64)) -> bool {
        let (low, high) = range;
        let (start, stop) = axis;
        let delta = stop.saturating_sub(start);
        if delta == 0_i64 {
            return low < start && start < high;
        }
        let shift = i128::from(delta);
        let enter = Frac::new(i128::from(low).saturating_sub(i128::from(start)), shift);
        let leave = Frac::new(i128::from(high).saturating_sub(i128::from(start)), shift);
        let (from, to) = if delta > 0_i64 {
            (enter, leave)
        } else {
            (leave, enter)
        };
        if from.cmp(&self.lo) == core::cmp::Ordering::Greater {
            self.lo = from;
        }
        if to.cmp(&self.hi) == core::cmp::Ordering::Less {
            self.hi = to;
        }
        self.lo.cmp(&self.hi) == core::cmp::Ordering::Less
    }
}

/// Whether the segment `from` to `to` passes strictly through the interior
/// of `square`: some open stretch of the segment lies inside all four
/// edges. Touching an edge, or running along one, is not passing through.
fn pierces(square: &Square, from: (i64, i64), to: (i64, i64)) -> bool {
    let mut span = Span::unit();
    span.clip_axis((square.min_x, square.max_x), (from.0, to.0))
        && span.clip_axis((square.min_y, square.max_y), (from.1, to.1))
}

/// One `Overlaps` row per strictly jammed pair among `standing` (thing
/// index and body square, in thing order), the pair reported once at its
/// lower index.
fn overlaps(standing: &[(usize, Square)]) -> Vec<Stuck> {
    standing
        .iter()
        .enumerate()
        .flat_map(|(position, &(index, square))| {
            standing
                .iter()
                .skip(position.saturating_add(1))
                .filter(move |other| square.overlaps(&other.1))
                .map(move |other| Stuck {
                    thing: index,
                    reason: StuckReason::Overlaps { other: other.0 },
                })
                .collect::<Vec<Stuck>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use serde_json::{json, to_value};

    use super::{Square, Stuck, StuckReason, pierces, stuck_things};
    use crate::defs::{Definitions, definitions_for};
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, spawn, vertex};
    use crate::wad::map::flags::{
        DOOM_BLOCK_LAND_MONSTERS, DOOM_BLOCK_PLAYERS, HEXEN_BLOCK_EVERYTHING, HEXEN_BLOCK_PLAYERS,
        Skill, THING_EASY, THING_HEXEN_SINGLE, THING_MEDIUM, THING_SKILL_BITS, TWO_SIDED,
    };
    use crate::wad::map::{Map, Thing};

    /// The built-in `DOOM II` definitions the tests read bodies from.
    fn defs() -> Definitions {
        definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
    }

    /// A walled 256-unit room standing at `heights`, with a player 1 start
    /// in the middle. Line 3 is the west wall, `x = 0`.
    fn room_map(heights: (i32, i32)) -> Map {
        room_map_in(heights, MapFormat::Doom)
    }

    /// [`room_map`] storing its block in the map format `format`.
    fn room_map_in(heights: (i32, i32), format: MapFormat) -> Map {
        let mut map = Map::new_empty(name("MAP01"), format);
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
        map.sectors = vec![room(heights.0, heights.1, 160, 0)];
        map.things = vec![spawn(1, 128, 128, 0)];
        map
    }

    /// Stands a thing of `kind` at `x`/`y` with presence bits `flags` and
    /// reports its index.
    fn stand(map: &mut Map, kind: i32, at: (i32, i32), flags: u32) -> usize {
        map.things.push(Thing {
            kind,
            x: at.0,
            y: at.1,
            angle: 0,
            flags,
            ..Default::default()
        });
        map.things.len().saturating_sub(1)
    }

    #[test]
    fn a_thing_in_the_void_is_outside_the_map() {
        let mut map = room_map((0_i32, 128_i32));
        let imp = stand(&mut map, 3001, (4_000_i32, 4_000_i32), 7);
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium),
            vec![Stuck {
                thing: imp,
                reason: StuckReason::OutsideMap,
            }],
            "the start stands fine and the lost imp does not"
        );
    }

    #[test]
    fn a_wall_strictly_through_the_square_is_in_wall() {
        let mut map = room_map((0_i32, 128_i32));
        let imp = stand(&mut map, 3001, (4_i32, 128_i32), 7);
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium),
            vec![Stuck {
                thing: imp,
                reason: StuckReason::InWall { line: 3 },
            }],
            "the west wall runs through the imp's square"
        );
    }

    /// Turn `map`'s west wall (line 3) into a two-sided line wearing
    /// `extra`: no longer one-sided, so only its bits can stop a body.
    fn open_west_wall(map: &mut Map, extra: u32) {
        let wall = map.linedefs.get_mut(3).expect("the west wall");
        wall.flags = TWO_SIDED | extra;
        wall.back = Some(0);
    }

    #[test]
    fn a_hexen_blockplayers_line_holds_a_thing_in_wall() {
        let mut map = room_map_in((0_i32, 128_i32), MapFormat::Hexen);
        map.things.clear();
        open_west_wall(&mut map, HEXEN_BLOCK_PLAYERS);
        let imp = stand(
            &mut map,
            3001,
            (4_i32, 128_i32),
            THING_SKILL_BITS | THING_HEXEN_SINGLE,
        );
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium),
            vec![Stuck {
                thing: imp,
                reason: StuckReason::InWall { line: 3 },
            }],
            "the walk and the placement already read blockplayers as a wall; so does this"
        );
    }

    #[test]
    fn a_mbf21_blockplayers_line_holds_a_doom_thing_in_wall() {
        let mut map = room_map((0_i32, 128_i32));
        map.things.clear();
        open_west_wall(&mut map, DOOM_BLOCK_PLAYERS);
        let imp = stand(&mut map, 3001, (4_i32, 128_i32), THING_SKILL_BITS);
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium),
            vec![Stuck {
                thing: imp,
                reason: StuckReason::InWall { line: 3 },
            }],
            "the Doom family's own blockplayers bit stops a body"
        );
        open_west_wall(&mut map, DOOM_BLOCK_LAND_MONSTERS);
        let report = stuck_things(&map, &defs(), Skill::Medium);
        assert!(
            report.is_empty(),
            "blocklandmonsters is not a wall to it: {report:?}"
        );
    }

    #[test]
    fn a_hexen_blockeverything_line_holds_a_thing_in_wall() {
        let mut map = room_map_in((0_i32, 128_i32), MapFormat::Hexen);
        map.things.clear();
        open_west_wall(&mut map, HEXEN_BLOCK_EVERYTHING);
        stand(
            &mut map,
            3001,
            (4_i32, 128_i32),
            THING_SKILL_BITS | THING_HEXEN_SINGLE,
        );
        let report = stuck_things(&map, &defs(), Skill::Medium);
        assert!(
            report
                .iter()
                .any(|stuck| stuck.reason == StuckReason::InWall { line: 3 }),
            "blockeverything stops every body: {report:?}"
        );
    }

    #[test]
    fn the_same_hexen_bits_are_nothing_to_a_doom_wall_test() {
        let mut map = room_map((0_i32, 128_i32));
        map.things.clear();
        open_west_wall(&mut map, HEXEN_BLOCK_PLAYERS | HEXEN_BLOCK_EVERYTHING);
        stand(&mut map, 3001, (4_i32, 128_i32), THING_SKILL_BITS);
        assert!(
            stuck_things(&map, &defs(), Skill::Medium).is_empty(),
            "the Doom tables do not name those bits, so the open line passes"
        );
    }

    #[test]
    fn a_square_touching_a_wall_is_not_stuck() {
        let mut map = room_map((0_i32, 128_i32));
        stand(&mut map, 3001, (20_i32, 128_i32), 7);
        assert!(
            stuck_things(&map, &defs(), Skill::Medium).is_empty(),
            "the imp's west edge lies exactly on the wall line: touching is not crossing"
        );
    }

    #[test]
    fn a_shallower_sector_than_the_body_needs_is_low_ceiling() {
        let mut map = room_map((0_i32, 40_i32));
        let imp = stand(&mut map, 3001, (128_i32, 200_i32), 7);
        let report = stuck_things(&map, &defs(), Skill::Medium);
        assert_eq!(
            report.get(1),
            Some(&Stuck {
                thing: imp,
                reason: StuckReason::LowCeiling { height: 40_i32 },
            }),
            "40 units of room hold nobody: {report:?}"
        );
        assert_eq!(
            report.first().map(|stuck| stuck.thing),
            Some(0_usize),
            "and the player start in the same room is reported too"
        );
    }

    #[test]
    fn two_jammed_monsters_report_their_pair_once() {
        let mut map = room_map((0_i32, 128_i32));
        let first = stand(&mut map, 3001, (60_i32, 60_i32), 7);
        let second = stand(&mut map, 3001, (90_i32, 60_i32), 7);
        let report = stuck_things(&map, &defs(), Skill::Medium);
        assert_eq!(
            report,
            vec![Stuck {
                thing: first,
                reason: StuckReason::Overlaps { other: second },
            }],
            "30 units apart, 40 of imp: one row, lower index first"
        );
    }

    #[test]
    fn squares_that_only_touch_are_neighbours_not_jams() {
        let mut map = room_map((0_i32, 128_i32));
        stand(&mut map, 3001, (60_i32, 60_i32), 7);
        stand(&mut map, 3001, (100_i32, 60_i32), 7);
        assert!(
            stuck_things(&map, &defs(), Skill::Medium).is_empty(),
            "exactly 40 apart: the edges meet and nothing overlaps"
        );
    }

    #[test]
    fn an_item_beside_a_wall_is_nobody_s_complaint() {
        let mut map = room_map((0_i32, 128_i32));
        stand(&mut map, 2001, (4_i32, 128_i32), 7);
        assert!(
            stuck_things(&map, &defs(), Skill::Medium).is_empty(),
            "a shotgun shoved into the wall has no body to test"
        );
    }

    #[test]
    fn unknown_types_get_the_monster_default_radius() {
        let mut close = room_map((0_i32, 128_i32));
        let jammed = stand(&mut close, 12_345, (19_i32, 128_i32), 7);
        assert_eq!(
            stuck_things(&close, &defs(), Skill::Medium),
            vec![Stuck {
                thing: jammed,
                reason: StuckReason::InWall { line: 3 },
            }],
            "19 from the wall with the 20-unit default square is inside it"
        );
        let mut clear = room_map((0_i32, 128_i32));
        stand(&mut clear, 12_345, (20_i32, 128_i32), 7);
        assert!(
            stuck_things(&clear, &defs(), Skill::Medium).is_empty(),
            "and the default is no wider than 20 either"
        );
    }

    #[test]
    fn unknown_types_stand_under_56_units_of_ceiling() {
        let mut map = room_map((0_i32, 55_i32));
        let odd = stand(&mut map, 12_345, (128_i32, 200_i32), 7);
        let report = stuck_things(&map, &defs(), Skill::Medium);
        assert!(
            report.iter().any(|stuck| stuck.thing == odd
                && stuck.reason == StuckReason::LowCeiling { height: 55_i32 }),
            "the unknown body still needs the vanilla 56: {report:?}"
        );
    }

    #[test]
    fn a_player_start_body_is_tested_like_a_monster() {
        let mut map = room_map((0_i32, 128_i32));
        map.things.clear();
        let start = stand(&mut map, 1, (15_i32, 128_i32), 7);
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium),
            vec![Stuck {
                thing: start,
                reason: StuckReason::InWall { line: 3 },
            }],
            "15 from the west wall with the start's 16-unit radius is inside it"
        );
    }

    #[test]
    fn a_thing_that_does_not_spawn_at_the_skill_is_left_alone() {
        let mut map = room_map((0_i32, 128_i32));
        let imp = stand(&mut map, 3001, (4_i32, 128_i32), THING_EASY);
        assert!(
            stuck_things(&map, &defs(), Skill::Medium).is_empty(),
            "an easy-only imp is not on the map at medium skill"
        );
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Easy),
            vec![Stuck {
                thing: imp,
                reason: StuckReason::InWall { line: 3 },
            }],
            "but on easy he is stuck in the wall like anyone else"
        );
    }

    #[test]
    fn a_medium_skill_bit_spawns_on_medium_only() {
        let mut map = room_map((0_i32, 128_i32));
        map.things.clear();
        stand(&mut map, 3001, (4_i32, 128_i32), THING_MEDIUM);
        assert!(
            stuck_things(&map, &defs(), Skill::Easy).is_empty(),
            "the medium bit keeps him off easy"
        );
        assert_eq!(
            stuck_things(&map, &defs(), Skill::Medium).len(),
            1,
            "on medium he spawns and the wall holds him"
        );
    }

    #[test]
    fn the_reason_rides_flat_in_the_json() {
        let stuck = Stuck {
            thing: 4,
            reason: StuckReason::InWall { line: 12 },
        };
        assert_eq!(
            to_value(stuck).expect("a serialisable stuck row"),
            json!({ "thing": 4_u64, "reason": "in_wall", "line": 12_u64 }),
            "the ticket's row shape"
        );
    }

    #[test]
    fn pierces_wants_an_open_stretch_inside_the_square() {
        let at = Square::of(
            &Thing {
                kind: 1,
                x: 20,
                y: 20,
                ..Default::default()
            },
            20,
        );
        assert!(
            pierces(&at, (0, 0), (200, 20)),
            "a line straight through the middle"
        );
        assert!(
            !pierces(&at, (0, 0), (0, 200)),
            "a line running along the west edge touches nothing inside"
        );
        assert!(
            !pierces(&at, (0, 0), (0, 0)),
            "a degenerate corner graze is no crossing"
        );
        assert!(
            pierces(&at, (40, 20), (10, 20)),
            "an endpoint exactly on the east edge, running inward, does pierce"
        );
        assert!(
            !pierces(&at, (40, 20), (40, 100)),
            "running along the east edge keeps every point outside"
        );
    }
}
