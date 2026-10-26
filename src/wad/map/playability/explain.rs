//! Why each unreachable sector stayed unreachable: one plain reason per way
//! the reachability walk almost got in.
//!
//! [`blockers`] reads a finished [`Playability`] beside its map and answers
//! the question the walk itself never states: *what* kept the player out.
//! Every unreachable sector with a reachable neighbour across a two-sided
//! line gets one entry naming the **first** rule that fails at that line -
//! the blocking flag, a too-short opening, an unworked lock, a door no
//! reachable trigger ever opened, a step above the player's legs, a sector
//! shallower than the player, a crossing gap too low to walk under. An
//! unreachable sector nobody borders gets [`BlockerReason::Isolated`].
//!
//! The two standability rules are ordered far-sector first: a crossing gap
//! can never measure wider than the far sector's own depth, so asking about
//! the sector before asking about the line is the only order under which
//! both reasons can actually appear. The blocked-line rules keep the
//! walk's own order: what the flag says, then what the tape measure says,
//! then what the specials say, then what the heights say.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use serde::Serialize;

use super::graph::{impassable, sides};
use super::{KeyColor, MAX_STEP_UP, MIN_LINE_WIDTH, PLAYER_HEIGHT, Playability};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::geometry::linedef_length;
use crate::wad::map::model::Map;

/// What keeps a player out of one sector across one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum BlockerReason {
    /// The line carries the blocking flag (or a `Hexen` player-keeping bit):
    /// nothing gets through, whatever the heights say.
    Impassable,
    /// The line is shorter than a player is wide, so the opening fits
    /// nobody.
    Narrow {
        /// The line's length in map units, absent when a dangling vertex
        /// leaves it unmeasurable.
        #[serde(skip_serializing_if = "Option::is_none")]
        length: Option<u64>,
    },
    /// The line is a locked door no reached key ever opened.
    Locked {
        /// The colour of key the line wants.
        color: KeyColor,
    },
    /// The sector is a door no reachable trigger ever opened.
    DoorNeverOpened,
    /// The step up from the reachable sector is higher than the player
    /// can walk.
    StepTooHigh {
        /// How far the unreachable sector's floor stands above the other.
        delta: i64,
    },
    /// The unreachable sector is shallower than a player is tall: nobody
    /// can stand in it.
    TooLowSector {
        /// The sector's ceiling height above its own floor.
        height: i32,
    },
    /// The gap across the line is lower than a player needs, counting the
    /// ceiling of either side.
    NoHeadroom {
        /// The gap's height in map units.
        gap: i32,
    },
    /// No reachable sector borders this one at all: nothing connects it.
    Isolated,
}

/// One way the player almost got in, and why they did not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    /// The unreachable sector.
    pub sector: usize,
    /// The reachable sector the player came up against the line from,
    /// `None` for [`BlockerReason::Isolated`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<usize>,
    /// The two-sided line between them, `None` for
    /// [`BlockerReason::Isolated`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The first rule that fails at this line.
    #[serde(flatten)]
    pub reason: BlockerReason,
}

/// Explains every unreachable sector of `map` (whose lines store their
/// flags in the family `family`) against a finished [`Playability`] walk.
///
/// The rows come out sorted by sector, and by linedef within a sector.
/// Only sectors the map's own tables hold are considered, so a walk that
/// names sectors beyond the end of [`Map::sectors`] says nothing about
/// them.
#[must_use]
#[inline]
pub fn blockers(map: &Map, family: FlagFamily, found: &Playability) -> Vec<Blocker> {
    let why = Explainer { map, family, found };
    let mut report: Vec<Blocker> = Vec::new();
    let mut bordered: BTreeSet<usize> = BTreeSet::new();
    for (line, row) in map.linedefs.iter().enumerate() {
        let Some((front, back)) = sides(map, row) else {
            continue;
        };
        let Some((from, sector)) = crossing_pair(found, front, back) else {
            continue;
        };
        let Some(reason) = why.reason(sector, from, line) else {
            continue;
        };
        bordered.insert(sector);
        report.push(Blocker {
            sector,
            from: Some(from),
            line: Some(line),
            reason,
        });
    }
    report.extend(isolated(map, found, &bordered));
    report.sort_by_key(|blocker| blocker.sector);
    report
}

/// The map, flag family and finished walk one explanation reads.
struct Explainer<'map> {
    /// The map being explained.
    map: &'map Map,
    /// The flag family its lines store their flags in.
    family: FlagFamily,
    /// The walk being explained.
    found: &'map Playability,
}

/// The `(from, to)` direction one line is a border in: its far end
/// unreachable while its near end is reached, `None` when both ends agree.
fn crossing_pair(found: &Playability, front: usize, back: usize) -> Option<(usize, usize)> {
    let front_reached = found.reachable.contains(&front);
    let back_reached = found.reachable.contains(&back);
    if front_reached == back_reached {
        return None;
    }
    Some(if front_reached {
        (front, back)
    } else {
        (back, front)
    })
}

/// The unreachable sectors no reachable sector borders, one `Isolated` row
/// each, in sector order.
fn isolated(map: &Map, found: &Playability, bordered: &BTreeSet<usize>) -> Vec<Blocker> {
    (0..map.sectors.len())
        .filter(|sector| !found.reachable.contains(sector) && !bordered.contains(sector))
        .map(|sector| Blocker {
            sector,
            from: None,
            line: None,
            reason: BlockerReason::Isolated,
        })
        .collect()
}

impl Explainer<'_> {
    /// The first rule that keeps a player in the reachable `from` sector
    /// out of `sector` across `line`, `None` when every rule passes -
    /// which no real walk leaves standing, since a passing line would
    /// have been crossed.
    fn reason(&self, sector: usize, from: usize, line: usize) -> Option<BlockerReason> {
        let map = self.map;
        let row = map.linedefs.get(line)?;
        if impassable(row.flags, self.family) {
            return Some(BlockerReason::Impassable);
        }
        let length = linedef_length(map, line);
        if too_short(length) {
            return Some(BlockerReason::Narrow { length });
        }
        if let Some(locked) = self
            .found
            .locks
            .iter()
            .find(|lock| lock.line == line && !lock.opened)
        {
            return Some(BlockerReason::Locked {
                color: locked.color,
            });
        }
        if self.found.doors_never_opened.contains(&sector) {
            return Some(BlockerReason::DoorNeverOpened);
        }
        let near = map.sectors.get(from)?;
        let far = map.sectors.get(sector)?;
        let delta = i64::from(far.floor_height).saturating_sub(i64::from(near.floor_height));
        if delta > i64::from(MAX_STEP_UP) {
            return Some(BlockerReason::StepTooHigh { delta });
        }
        let depth = far.ceiling_height.saturating_sub(far.floor_height);
        if depth < PLAYER_HEIGHT {
            return Some(BlockerReason::TooLowSector { height: depth });
        }
        let gap = near
            .ceiling_height
            .min(far.ceiling_height)
            .saturating_sub(near.floor_height.max(far.floor_height));
        if gap < PLAYER_HEIGHT {
            return Some(BlockerReason::NoHeadroom { gap });
        }
        None
    }
}

/// Whether a line of `length` map units is too short for a player to fit
/// through; an unmeasurable line is.
fn too_short(length: Option<u64>) -> bool {
    let wanted = u64::try_from(MIN_LINE_WIDTH).unwrap_or(u64::MAX);
    length.is_none_or(|units| units < wanted)
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use serde_json::{json, to_value};

    use super::{Blocker, BlockerReason, blockers};
    use crate::plan::compile::compile;
    use crate::plan::model::{Connection, ConnectionKind, Plan, Room, Shape};
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::Map;
    use crate::wad::map::common::fixtures::{cell, hall, middle, name, slab, spawn};
    use crate::wad::map::flags::{BLOCKING, FlagFamily, Skill, TWO_SIDED};
    use crate::wad::map::playability::{Playability, analyze_playability};

    /// The blue card thing type.
    const BLUE_CARD: i32 = 5_i32;
    /// The blue door use special.
    const BLUE_DOOR: i32 = 26_i32;

    /// Sets the `(special, tag, flags)` triple on linedef `line` of `map`.
    fn wire(map: &mut Map, line: usize, action: (i32, i32, u32)) {
        if let Some(def) = map.linedefs.get_mut(line) {
            def.special = action.0;
            def.tag = action.1;
            def.flags = action.2;
        }
    }

    /// Stands a thing of `kind` at `at` with presence bits `flags`.
    fn stand(map: &mut Map, kind: i32, at: (i32, i32), flags: u32) {
        map.things.push(spawn(kind, at.0, at.1, 0));
        if let Some(last) = map.things.last_mut() {
            last.flags = flags;
        }
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

    /// A rectangular plan room, 256 units tall, spanning `span` on `x` and
    /// standing at `heights`' `(floor, ceiling)`.
    fn room(id: &str, span: (i64, i64), heights: (i32, i32)) -> Room {
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

    /// The blockers of a plan of `rooms` joined by `links`, walked from a
    /// start stood in room `a`.
    fn built_blockers(rooms: Vec<Room>, links: Vec<Connection>) -> Vec<Blocker> {
        let blueprint = Plan {
            rooms,
            connections: links,
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        let mut compiled = compile(&blueprint, name("MAP01"), MapFormat::Doom, &FlatRoles)
            .expect("the plan compiles");
        stand(&mut compiled.map, 1, middle(0), 7);
        let map = compiled.map;
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        blockers(&map, FlagFamily::Doom, &found)
    }

    /// The blocker rows of the walked hand-built hall.
    fn walked(map: &Map) -> Vec<Blocker> {
        let found = analyze_playability(map, FlagFamily::Doom, Skill::Medium);
        blockers(map, FlagFamily::Doom, &found)
    }

    #[test]
    fn a_reachable_pair_produces_no_blockers() {
        let map = hall(&[cell(0_i32), cell(0_i32)]);
        assert!(walked(&map).is_empty(), "everybody reached, nothing to say");
    }

    #[test]
    fn the_blocking_flag_is_the_first_thing_named() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCKING));
        assert_eq!(
            walked(&map),
            vec![Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::Impassable,
            }],
            "the flag outranks the equally failing nothing else"
        );
    }

    #[test]
    fn a_plan_built_slit_reports_its_length() {
        let report = built_blockers(
            vec![
                room("a", (0, 256), cell(0_i32)),
                room("b", (256, 256), cell(0_i32)),
            ],
            vec![Connection {
                id: Some("c1".to_owned()),
                from: "a".to_owned(),
                to: "b".to_owned(),
                kind: ConnectionKind::Opening,
                at: None,
                width: 24,
            }],
        );
        let opening = report
            .iter()
            .find(|blocker| {
                blocker.reason
                    == BlockerReason::Narrow {
                        length: Some(24_u64),
                    }
            })
            .expect("a narrow blocker naming the 24-unit line");
        assert_eq!(opening.sector, 2, "the band behind the slit is sector 2");
        assert_eq!(opening.from, Some(0), "the player stands in room a");
        assert_eq!(
            opening.line,
            Some(4),
            "the room-to-band line through the slit"
        );
        assert!(
            report
                .iter()
                .any(|blocker| blocker.sector == 1 && blocker.reason == BlockerReason::Isolated),
            "room b borders nothing reachable: {report:?}"
        );
    }

    #[test]
    fn an_unworked_lock_names_its_colour_and_isolates_the_rest() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (BLUE_DOOR, 0_i32, TWO_SIDED));
        stand(&mut map, BLUE_CARD, middle(2), 7);
        let report = walked(&map);
        assert_eq!(
            report.first(),
            Some(&Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::Locked {
                    color: crate::wad::map::playability::KeyColor::Blue,
                },
            }),
            "the blue door is what holds them: {report:?}"
        );
        assert!(
            report
                .iter()
                .any(|blocker| blocker.sector == 2 && blocker.reason == BlockerReason::Isolated),
            "the far room has no reachable neighbour at all: {report:?}"
        );
    }

    #[test]
    fn a_door_no_reachable_switch_works_says_so() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        map.sectors.get_mut(1).expect("the band").tag = 7_i32;
        wire(&mut map, 6, (103_i32, 7_i32, BLOCKING));
        let report = walked(&map);
        assert!(
            report.iter().any(|blocker| blocker.sector == 1
                && blocker.from == Some(0)
                && blocker.reason == BlockerReason::DoorNeverOpened),
            "the switch on the far wall is never pressed: {report:?}"
        );
    }

    #[test]
    fn a_line_that_cannot_be_measured_is_a_narrow_one() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.linedefs.get_mut(0).expect("the divider").v2 = 9_999_usize;
        let found = Playability {
            start_sector: Some(0),
            reachable: core::iter::once(0_usize).collect(),
            ..Default::default()
        };
        let report = blockers(&map, FlagFamily::Doom, &found);
        assert_eq!(
            report.first(),
            Some(&Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::Narrow { length: None },
            }),
            "an unmeasurable line fits nobody: {report:?}"
        );
    }

    #[test]
    fn a_forty_unit_ledge_reports_its_step() {
        let map = hall(&[cell(0_i32), cell(40_i32)]);
        assert_eq!(
            walked(&map),
            vec![Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::StepTooHigh { delta: 40_i64 },
            }],
            "the delta travels with the reason"
        );
    }

    #[test]
    fn a_shallow_sector_is_named_before_the_crossing_gap() {
        let map = hall(&[cell(0_i32), (0_i32, 50_i32)]);
        assert_eq!(
            walked(&map),
            vec![Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::TooLowSector { height: 50_i32 },
            }],
            "the sector itself is shallower than the player"
        );
    }

    #[test]
    fn a_low_ceiling_over_the_near_floor_is_the_crossing_gap() {
        let map = hall(&[(100_i32, 160_i32), (110_i32, 200_i32)]);
        let report = walked(&map);
        assert_eq!(
            report.first(),
            Some(&Blocker {
                sector: 1,
                from: Some(0),
                line: Some(0),
                reason: BlockerReason::NoHeadroom { gap: 50_i32 },
            }),
            "both rooms stand in, but the lip ducking over is 50 units: {report:?}"
        );
    }

    #[test]
    fn isolated_sectors_sort_by_index_after_the_bordered() {
        let mut map = hall(&[cell(0_i32), cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCKING));
        let report = walked(&map);
        assert_eq!(
            report
                .iter()
                .map(|blocker| blocker.sector)
                .collect::<Vec<usize>>(),
            vec![1_usize, 2],
            "sorted by sector"
        );
        assert_eq!(
            report.last().map(|blocker| &blocker.reason),
            Some(&BlockerReason::Isolated),
            "the far room borders only the blocked one"
        );
    }

    #[test]
    fn the_reason_rides_flat_in_the_json() {
        let blocker = Blocker {
            sector: 5,
            from: Some(2),
            line: Some(17),
            reason: BlockerReason::StepTooHigh { delta: 40_i64 },
        };
        assert_eq!(
            to_value(&blocker).expect("a serialisable blocker"),
            json!({
                "sector": 5_u64,
                "from": 2_u64,
                "line": 17_u64,
                "reason": "step_too_high",
                "delta": 40_i64,
            }),
            "the ticket's row shape"
        );
        let lonely = Blocker {
            sector: 9,
            from: None,
            line: None,
            reason: BlockerReason::Isolated,
        };
        assert_eq!(
            to_value(&lonely).expect("a serialisable blocker"),
            json!({ "sector": 9_u64, "reason": "isolated" }),
            "an isolated row carries no from or line"
        );
    }
}
