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
//! they would have connected) into [`Playability`].
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
//! [`validate`]: crate::wad::map::validate

mod graph;
mod reach;
mod specials;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Serialize;

use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::model::Map;

use graph::World;
use reach::Run;
use reach::fixpoint;
use reach::touches;
use specials::Inventory;

/// The shortest two-sided line a player can walk through: twice the 16-unit
/// player radius. A line shorter than this is reported in
/// [`Playability::narrow_lines`] instead of being a way through.
pub const MIN_LINE_WIDTH: i32 = 32_i32;
/// The height a player needs to stand in, and the smallest gap across any
/// line they can cross.
pub const PLAYER_HEIGHT: i32 = 56_i32;
/// The highest ledge a player can walk up. Falling is free at any depth, so
/// movement is directional and a drop is a one-way connection.
pub const MAX_STEP_UP: i32 = 24_i32;
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
    /// Whether the walk both reached the key and reached a trigger.
    pub opened: bool,
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
            opened: walk.opened.contains(&lock.feature),
        })
        .collect()
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
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{ExitInfo, KeyColor, KeyInfo, LockInfo, Playability, analyze_playability};
    use crate::plan::FeatureDress;
    use crate::plan::compile::compile;
    use crate::plan::features::{FeatureRow, apply_features};
    use crate::plan::model::{
        Connection, ConnectionKind, DoorStyle, Feature as PlanFeature, KeyColor as PlanKeyColor,
        Plan, Room, Shape, Side,
    };
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{name, side, spawn, vertex};
    use crate::wad::map::features::LiftStyle;
    use crate::wad::map::flags::{FlagFamily, Skill};
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing};

    /// The `Hexen` `playeruse` activation: field value 1 at bit 10.
    const PLAYER_USE: u32 = 0x400_u32;
    /// The `Doom` `blocking` bit.
    const BLOCKING: u32 = 1_u32;
    /// The `blockmonsters` bit, which a player walks past.
    const BLOCK_MONSTERS: u32 = 2_u32;
    /// The shared `twosided` bit.
    const TWO_SIDED: u32 = 4_u32;
    /// The blue card key type.
    const BLUE_CARD: i32 = 5_i32;
    /// The red skull key type.
    const RED_SKULL: i32 = 38_i32;
    /// The teleport destination thing type.
    const TELEPORT_DEST: i32 = 14_i32;
    /// The `Hexen` thing bits that put a thing in the single-player game.
    const HEXEN_PRESENCE: u32 = 7_u32 | 0x100;

    /// A hand-built room: `floor` under 128 units of ceiling.
    fn cell(floor: i32) -> (i32, i32) {
        (floor, floor.saturating_add(128))
    }

    /// A hand-built slab: a sector with no headroom at all, which is what a
    /// shut door sector looks like to the walk.
    fn slab(floor: i32) -> (i32, i32) {
        (floor, floor)
    }

    /// The `x` of hand-built room column `column`.
    fn edge(column: usize) -> i32 {
        i32::try_from(column).map_or(i32::MAX, |index| index.saturating_mul(256))
    }

    /// The middle of hand-built room `room`, where a thing certainly stands.
    fn middle(room: usize) -> (i32, i32) {
        (edge(room).saturating_add(128), 128)
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

    /// A row of 256-unit rooms: `specs` is one `(floor, ceiling)` per room,
    /// room `i` spanning `x = 256i` to `x = 256(i + 1)` and owning sector
    /// `i`. Linedef `i` is the two-sided divider between rooms `i` and
    /// `i + 1`, its front facing the lower-numbered room; the walls that
    /// close each room follow the dividers. A player 1 start stands in the
    /// middle of room 0.
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
        for room in 0..count.saturating_sub(1) {
            let next = room.saturating_add(1);
            sidedefs.push(side("-", "-", "-", room));
            sidedefs.push(side("-", "-", "-", next));
            linedefs.push(Linedef {
                v1: top(next),
                v2: bottom(next),
                flags: TWO_SIDED,
                front: Some(room.saturating_mul(2)),
                back: Some(room.saturating_mul(2).saturating_add(1)),
                ..Default::default()
            });
        }
        for room in 0..count {
            let next = room.saturating_add(1);
            linedefs.push(one_sided(top(room), top(next), &mut sidedefs, room));
            linedefs.push(one_sided(bottom(next), bottom(room), &mut sidedefs, room));
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

    /// Stands a thing of `kind` at `at` with presence bits `flags` and
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

    /// Sets the `(special, tag, flags)` triple on linedef `line` of `map`.
    fn wire(map: &mut Map, line: usize, action: (i32, i32, u32)) {
        if let Some(def) = map.linedefs.get_mut(line) {
            def.special = action.0;
            def.tag = action.1;
            def.flags = action.2;
        }
    }

    /// Clears the line special off each of `lines`, taking a built feature's
    /// trigger away for the negative half of a test.
    fn clear_specials(map: &mut Map, lines: &[usize]) {
        let targets: BTreeSet<usize> = lines.iter().copied().collect();
        map.linedefs
            .iter_mut()
            .enumerate()
            .filter(|entry| targets.contains(&entry.0))
            .for_each(|entry| entry.1.special = 0_i32);
    }

    /// Retunes the hand-built map's player start to the `Hexen` presence
    /// bits, which ask for the single-player mode bit as well as the skill.
    fn hexen_start(map: &mut Map) {
        if let Some(thing) = map.things.first_mut() {
            thing.flags = HEXEN_PRESENCE;
        }
    }

    /// The sector set of `list`, for the assertions below.
    fn sectors(list: &[usize]) -> BTreeSet<usize> {
        list.iter().copied().collect()
    }

    /// The warnings of `found` that mention `text`.
    fn notes_about(found: &Playability, text: &str) -> Vec<String> {
        found
            .warnings
            .iter()
            .filter(|warning| warning.contains(text))
            .cloned()
            .collect()
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

    /// A connection of `kind` between the rooms of `pair`, `width` clear.
    fn link(id: &str, pair: (&str, &str), kind: ConnectionKind, width: i64) -> Connection {
        Connection {
            id: Some(id.to_owned()),
            from: pair.0.to_owned(),
            to: pair.1.to_owned(),
            kind,
            at: None,
            width,
        }
    }

    /// The plan every [`built`] test wraps: `rooms` joined by `links`.
    fn plan(rooms: Vec<Room>, links: Vec<Connection>, features: Vec<PlanFeature>) -> Plan {
        Plan {
            rooms,
            connections: links,
            features,
            wall_thickness: 16_i64,
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

    /// The sector of the plan's one door band, read from the feature rows.
    fn door_band(rows: &[FeatureRow]) -> usize {
        *rows
            .iter()
            .find(|row| row.kind == "door")
            .and_then(|row| row.sectors.first())
            .expect("the door's band sector")
    }

    #[test]
    fn an_opening_between_two_rooms_reaches_both() {
        let map = hall(&[cell(0_i32), cell(0_i32)]);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.start_sector, Some(0), "the start stands in the hall");
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "the gap between the rooms is a way through"
        );
        assert!(found.warnings.is_empty(), "nothing to complain about");
        assert!(found.doors_never_opened.is_empty(), "there are no doors");
        assert!(found.narrow_lines.is_empty(), "a 256-unit divider fits");
        assert!(found.exits.is_empty(), "and the map ends nowhere yet");
    }

    #[test]
    fn a_step_above_the_players_legs_is_a_way_down_only() {
        let map = hall(&[cell(32_i32), cell(0_i32), cell(32_i32)]);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "the player drops 32 units into the pit and cannot climb back out of it"
        );
    }

    #[test]
    fn a_ledge_at_the_step_limit_is_still_walkable() {
        let map = hall(&[cell(0_i32), cell(24_i32)]);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.reachable, sectors(&[0, 1]), "24 units is the limit");
        let taller = hall(&[cell(0_i32), cell(25_i32)]);
        let stuck = analyze_playability(&taller, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            stuck.reachable,
            sectors(&[0]),
            "and one unit past it is not"
        );
    }

    #[test]
    fn forty_eight_units_of_headroom_stop_the_player() {
        let map = hall(&[cell(0_i32), (0_i32, 48_i32)]);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0]),
            "a 48-unit room is shallower than the player"
        );
    }

    #[test]
    fn the_blocking_flag_ends_a_connection_whatever_the_heights() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCKING));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.reachable, sectors(&[0]), "a blocked line is no way");

        let mut hexen = map;
        hexen_start(&mut hexen);
        let blocked = analyze_playability(&hexen, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            blocked.reachable,
            sectors(&[0]),
            "the same bit blocks Hexen"
        );
    }

    #[test]
    fn the_blockplayers_bit_is_a_hexen_wall_and_a_doom_nop() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | 0x4000));
        let doom = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            doom.reachable,
            sectors(&[0, 1]),
            "bit 14 means nothing to the Doom tables"
        );
        let mut at_hexen = map.clone();
        hexen_start(&mut at_hexen);
        let hexen = analyze_playability(&at_hexen, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            hexen.reachable,
            sectors(&[0]),
            "and means blockplayers to the Hexen ones"
        );
    }

    #[test]
    fn the_monster_flag_alone_still_lets_the_player_through() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0_i32, 0_i32, TWO_SIDED | BLOCK_MONSTERS));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "flag 2 keeps monsters, not players"
        );
    }

    #[test]
    fn a_manual_door_opens_the_sector_behind_it() {
        let slab = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        let shut = analyze_playability(&slab, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            shut.reachable,
            sectors(&[0]),
            "a slab of a sector is nobody's way through"
        );
        assert!(
            shut.doors_never_opened.is_empty(),
            "with no special on its lines there is no door to report either"
        );

        let mut map = slab;
        wire(&mut map, 0, (1_i32, 0_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2]),
            "the use line in front of a player raises the ceiling over their head"
        );
        assert!(found.doors_never_opened.is_empty(), "the door opened");
        assert!(found.locks.is_empty(), "and it asked for no key");
    }

    #[test]
    fn a_locked_door_holds_until_its_key_is_reached() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
        stand(&mut map, BLUE_CARD, middle(2), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0]),
            "the blue card behind the blue door cannot be fetched"
        );
        assert_eq!(
            found.locks,
            vec![LockInfo {
                line: 0,
                color: KeyColor::Blue,
                opened: false,
            }],
            "the locked line is named and stays shut"
        );
        assert_eq!(found.doors_never_opened, vec![1_usize], "the band");
        assert_eq!(
            found.keys,
            vec![KeyInfo {
                thing: 1,
                color: KeyColor::Blue,
                sector: Some(2),
                reachable: false,
            }],
            "the key is where the player is not"
        );
    }

    #[test]
    fn a_key_in_the_reachable_room_opens_its_door() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
        stand(&mut map, BLUE_CARD, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2]),
            "the card in hand walks the player through"
        );
        assert_eq!(
            found.locks,
            vec![LockInfo {
                line: 0,
                color: KeyColor::Blue,
                opened: true,
            }]
        );
        assert!(found.doors_never_opened.is_empty(), "the band opened");
        assert_eq!(
            found.keys.first().map(|entry| entry.reachable),
            Some(true),
            "and the player walked onto it"
        );
    }

    #[test]
    fn a_card_and_a_skull_answer_for_the_same_colour() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (28_i32, 0_i32, TWO_SIDED));
        stand(&mut map, RED_SKULL, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert!(
            found.reachable.contains(&2),
            "the red skull opens the red door"
        );
        assert_eq!(found.keys.first().map(|key| key.color), Some(KeyColor::Red));
    }

    #[test]
    fn a_skill_that_hides_the_key_hides_the_way_through() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
        stand(&mut map, BLUE_CARD, middle(0), 4);
        let medium = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            medium.reachable,
            sectors(&[0]),
            "a hard-skill key does not spawn on medium"
        );
        assert!(medium.keys.is_empty(), "and so it is no key at all");
        let hard = analyze_playability(&map, FlagFamily::Doom, Skill::Hard);
        assert!(
            hard.reachable.contains(&2),
            "on hard the card is right there"
        );
        assert_eq!(hard.keys.first().map(|key| key.color), Some(KeyColor::Blue));
    }

    #[test]
    fn a_multiplayer_only_key_is_no_key() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (26_i32, 0_i32, TWO_SIDED));
        stand(&mut map, BLUE_CARD, middle(0), 7 | 16);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert!(found.keys.is_empty(), "a notsingle thing never spawns");
        assert_eq!(found.reachable, sectors(&[0]), "and so the door stays shut");
    }

    #[test]
    fn a_hexen_locked_door_wants_the_colour_of_its_lock_argument() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (13_i32, 0_i32, TWO_SIDED | PLAYER_USE));
        map.linedefs.get_mut(0).expect("the divider").args =
            [0_i32, 16_i32, 150_i32, 130_i32, 0_i32];
        hexen_start(&mut map);
        let shut = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(shut.reachable, sectors(&[0]), "no key, no way through");
        assert_eq!(
            shut.locks,
            vec![LockInfo {
                line: 0,
                color: KeyColor::Blue,
                opened: false,
            }],
            "arg3 130 is the blue any-key lock"
        );
        let mut with_key = map;
        stand(&mut with_key, BLUE_CARD, middle(0), HEXEN_PRESENCE);
        let open = analyze_playability(&with_key, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            open.reachable,
            sectors(&[0, 1, 2]),
            "arg0 0 leaves the door acting on the sector behind the line"
        );
        assert!(open.locks.iter().all(|lock| lock.opened), "and it opened");
    }

    #[test]
    fn an_unknown_lock_code_is_read_as_openable_and_says_so() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (13_i32, 0_i32, TWO_SIDED | PLAYER_USE));
        map.linedefs.get_mut(0).expect("the divider").args =
            [0_i32, 16_i32, 150_i32, 77_i32, 0_i32];
        hexen_start(&mut map);
        let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        let notes = notes_about(&found, "unknown lock 77");
        assert_eq!(
            notes.len(),
            1,
            "the odd code is named once: {:?}",
            found.warnings
        );
        assert!(
            notes
                .first()
                .is_some_and(|note| note.contains("assumed openable")),
            "and called what it is: {notes:?}"
        );
        assert!(
            found.reachable.contains(&2),
            "the door is walked through for want of a better guess"
        );
        assert!(found.locks.is_empty(), "and it is no longer a colour lock");
    }

    #[test]
    fn a_use_line_needs_its_front_side_and_a_walk_line_either() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        map.sectors.get_mut(1).expect("the band").tag = 4_i32;
        map.things = vec![spawn(1, 640, 128, 0)];
        wire(&mut map, 1, (103_i32, 4_i32, TWO_SIDED));
        let use_line = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            use_line.reachable,
            sectors(&[2]),
            "an SR door faces the band it opens, and the player is behind it"
        );

        wire(&mut map, 1, (2_i32, 4_i32, TWO_SIDED));
        let walk_line = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            walk_line.reachable,
            sectors(&[0, 1, 2]),
            "a W1 door works from the side the player is standing on"
        );
    }

    #[test]
    fn a_remote_door_needs_its_switch_on_the_reachable_side() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        map.sectors.get_mut(1).expect("the band").tag = 7_i32;
        let mut with_switch = map.clone();
        wire(&mut with_switch, 2, (103_i32, 7_i32, BLOCKING));
        let worked = analyze_playability(&with_switch, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            worked.reachable,
            sectors(&[0, 1, 2]),
            "the switch on a hall wall raises the tagged band"
        );
        assert!(worked.doors_never_opened.is_empty(), "and it opened");

        let mut behind = map;
        wire(&mut behind, 6, (103_i32, 7_i32, BLOCKING));
        let stuck = analyze_playability(&behind, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            stuck.reachable,
            sectors(&[0]),
            "the same switch on the far side of the door is never pressed"
        );
        assert_eq!(
            stuck.doors_never_opened,
            vec![1_usize],
            "so the band is a door that never opened"
        );
    }

    #[test]
    fn a_lift_carries_the_player_up_to_a_ledge_they_cannot_climb() {
        let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
        map.sectors.get_mut(1).expect("the platform").tag = 3_i32;
        wire(&mut map, 0, (21_i32, 3_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2]),
            "the platform rides them up to the ledge"
        );

        let mut plain = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
        plain.sectors.get_mut(1).expect("the platform").tag = 3_i32;
        let stuck = analyze_playability(&plain, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            stuck.reachable,
            sectors(&[0]),
            "without the lift the ledge is 96 units out of reach"
        );
    }

    #[test]
    fn a_walk_on_teleporter_reaches_the_room_it_points_at() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
        wire(&mut map, 0, (97_i32, 5_i32, TWO_SIDED | BLOCKING));
        stand(&mut map, TELEPORT_DEST, middle(1), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "the pad carries the player across a line they cannot walk"
        );
    }

    #[test]
    fn a_teleporter_without_its_destination_says_so() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
        wire(&mut map, 0, (97_i32, 5_i32, TWO_SIDED | BLOCKING));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.reachable, sectors(&[0]), "there is nowhere to land");
        let notes = notes_about(&found, "no teleport destination");
        assert_eq!(notes.len(), 1, "and the missing thing is named");
    }

    #[test]
    fn a_hexen_teleport_follows_its_destination_tid() {
        let mut map = hall(&[cell(0_i32), cell(96_i32)]);
        wire(&mut map, 0, (70_i32, 0_i32, TWO_SIDED | BLOCKING));
        map.linedefs.get_mut(0).expect("the divider").args = [7_i32, 0_i32, 0_i32, 0_i32, 0_i32];
        hexen_start(&mut map);
        stand(&mut map, TELEPORT_DEST, middle(1), HEXEN_PRESENCE);
        if let Some(thing) = map.things.last_mut() {
            thing.tid = 7_i32;
        }
        let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "the 96-unit step is taken by teleporting over it"
        );

        let mut other_tid = map.clone();
        if let Some(thing) = other_tid.things.last_mut() {
            thing.tid = 9_i32;
        }
        let wrong = analyze_playability(&other_tid, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            wrong.reachable,
            sectors(&[0]),
            "and a thing with another tid is not the destination"
        );
        assert_eq!(
            notes_about(&wrong, "no teleport destination").len(),
            1,
            "which the report says"
        );
    }

    #[test]
    fn an_exit_line_reads_as_reachable_with_its_kind() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (52_i32, 0_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.exits,
            vec![ExitInfo {
                line: 0,
                special: 52,
                secret: false,
                reachable: true,
            }],
            "a walk-over exit on a reached line"
        );
    }

    #[test]
    fn an_exit_behind_an_impassable_line_is_unreachable() {
        let mut map = hall(&[cell(0_i32), cell(128_i32), cell(0_i32)]);
        wire(&mut map, 0, (11_i32, 0_i32, TWO_SIDED));
        wire(&mut map, 1, (51_i32, 0_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.exits,
            vec![
                ExitInfo {
                    line: 0,
                    special: 11,
                    secret: false,
                    reachable: true,
                },
                ExitInfo {
                    line: 1,
                    special: 51,
                    secret: true,
                    reachable: false,
                },
            ],
            "the use side of the second exit is the room nobody reaches"
        );
    }

    #[test]
    fn the_hexen_portals_are_the_exits_their_numbers_say() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (243_i32, 0_i32, TWO_SIDED));
        hexen_start(&mut map);
        let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            found.exits.first(),
            Some(&ExitInfo {
                line: 0,
                special: 243,
                secret: false,
                reachable: true,
            }),
            "243 ends the level"
        );
        wire(&mut map, 0, (244_i32, 0_i32, TWO_SIDED));
        let secret = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            secret.exits.first().map(|exit| exit.secret),
            Some(true),
            "244 is the secret one"
        );
        wire(&mut map, 0, (74_i32, 0_i32, TWO_SIDED));
        let newmap = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            newmap.exits.first().map(|exit| exit.special),
            Some(74_i32),
            "and Teleport_NewMap ends it too"
        );
    }

    #[test]
    fn a_plan_built_locked_door_locks_both_of_its_room_facing_lines() {
        let blueprint = plan(
            vec![
                room("a", (0, 256), cell(0_i32)),
                room("b", (256, 256), cell(0_i32)),
            ],
            vec![link(
                "d1",
                ("a", "b"),
                ConnectionKind::Door {
                    style: DoorStyle::Locked(PlanKeyColor::Blue),
                },
                64,
            )],
            Vec::new(),
        );
        let (mut map, rows) = built(&blueprint, MapFormat::Doom);
        let door = rows.first().expect("the wired door row");
        assert_eq!(door.kind, "door", "the row is the door");
        let band = *door.sectors.first().expect("the band sector");
        stand(&mut map, 1, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0]),
            "the band is shut and its key is nowhere"
        );
        assert_eq!(found.doors_never_opened, vec![band], "so it never opened");
        assert_eq!(
            found.locks.len(),
            door.lines.len(),
            "every room-facing line of the door is a lock"
        );
        assert!(
            found.locks.iter().all(|lock| lock.color == KeyColor::Blue),
            "and every one of them wants the blue card"
        );
        assert!(
            found.locks.iter().all(|lock| !lock.opened),
            "all of them stayed shut"
        );

        let mut with_key = map.clone();
        stand(&mut with_key, BLUE_CARD, middle(0), 7);
        let walked = analyze_playability(&with_key, FlagFamily::Doom, Skill::Medium);
        assert!(
            walked.reachable.len() > found.reachable.len(),
            "drop the card in the hall and the whole map opens: {:?}",
            walked.reachable
        );
        assert!(walked.doors_never_opened.is_empty(), "the band rose");
    }

    #[test]
    fn a_plan_built_switch_raises_the_remote_door_it_names() {
        let rooms = vec![
            room("a", (0, 256), cell(0_i32)),
            room("b", (256, 256), cell(0_i32)),
        ];
        let remote = link(
            "d1",
            ("a", "b"),
            ConnectionKind::Door {
                style: DoorStyle::Remote,
            },
            64,
        );
        let bare = plan(rooms.clone(), vec![remote.clone()], Vec::new());
        let (bare_map, bare_rows) = built(&bare, MapFormat::Doom);
        let mut handleless = bare_map;
        stand(&mut handleless, 1, middle(0), 7);
        let shut = analyze_playability(&handleless, FlagFamily::Doom, Skill::Medium);
        let band = door_band(&bare_rows);
        assert!(
            shut.doors_never_opened.is_empty(),
            "a remote door writes no special on its own lines, so there is no trigger              to report as unworked"
        );
        assert!(
            !shut.reachable.contains(&band),
            "and the band it leaves shut is still out of reach"
        );

        let wired = plan(
            rooms,
            vec![remote],
            vec![PlanFeature::Switch {
                room: "a".to_owned(),
                opens: "d1".to_owned(),
                wall: Some(Side::West),
                repeatable: true,
            }],
        );
        let (wired_map, rows) = built(&wired, MapFormat::Doom);
        let mut played = wired_map;
        stand(&mut played, 1, middle(0), 7);
        let found = analyze_playability(&played, FlagFamily::Doom, Skill::Medium);
        assert!(
            found.reachable.contains(&band),
            "the switch mounted in the hall raises the band: {:?}",
            found.reachable
        );
        assert!(found.doors_never_opened.is_empty(), "and it opened");
        assert_eq!(door_band(&rows), band, "the same band in both builds");
    }

    #[test]
    fn a_plan_built_lift_reaches_the_high_room_it_serves() {
        let lifts = Plan {
            rooms: vec![
                room("low", (0, 256), cell(0_i32)),
                room("pad", (256, 256), cell(96_i32)),
            ],
            connections: vec![link("c1", ("low", "pad"), ConnectionKind::Opening, 64)],
            features: vec![PlanFeature::Lift {
                room: "pad".to_owned(),
                low_floor: None,
                style: LiftStyle::UseWall,
                repeatable: true,
            }],
            wall_thickness: 0_i64,
        };
        let (mut map, rows) = built(&lifts, MapFormat::Doom);
        let lift = rows
            .iter()
            .find(|row| row.kind == "lift")
            .expect("the lift row");
        let platform = *lift.sectors.first().expect("the platform sector");
        let edges = lift.lines.clone();
        stand(&mut map, 1, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert!(
            found.reachable.contains(&platform),
            "the platform rides the player up to the high room: {:?}",
            found.reachable
        );

        let mut stranded = map.clone();
        clear_specials(&mut stranded, &edges);
        let stuck = analyze_playability(&stranded, FlagFamily::Doom, Skill::Medium);
        assert!(
            !stuck.reachable.contains(&platform),
            "and with the specials stripped the same map leaves them below"
        );
        assert!(
            found.reachable.len() > stuck.reachable.len(),
            "the lift is what widens the map"
        );
    }

    #[test]
    fn a_plan_built_teleporter_reaches_the_far_room() {
        let teleported = plan(
            vec![
                room("a", (0, 256), cell(0_i32)),
                room("b", (512, 256), cell(0_i32)),
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
        );
        let (mut map, rows) = built(&teleported, MapFormat::Doom);
        let square = rows
            .iter()
            .find(|row| row.kind == "teleporter")
            .and_then(|row| row.sectors.get(1))
            .copied()
            .expect("the destination square");
        stand(&mut map, 1, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert!(
            found.reachable.contains(&square),
            "the pad in room a lands the player in room b: {:?}",
            found.reachable
        );
        assert!(
            found.reachable.len() > 2,
            "and they can walk on from the square: {:?}",
            found.reachable
        );
    }

    #[test]
    fn a_short_line_reports_itself_as_the_way_the_player_cannot_take() {
        let narrow = plan(
            vec![
                room("a", (0, 256), cell(0_i32)),
                room("b", (256, 256), cell(0_i32)),
            ],
            vec![link("c1", ("a", "b"), ConnectionKind::Opening, 24)],
            Vec::new(),
        );
        let (mut map, _) = built(&narrow, MapFormat::Doom);
        stand(&mut map, 1, middle(0), 7);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0]),
            "24 units is narrower than the player is wide"
        );
        assert!(
            !found.narrow_lines.is_empty(),
            "and the slit is reported as the way they just miss: {:?}",
            found.narrow_lines
        );

        let roomy = plan(
            vec![
                room("a", (0, 256), cell(0_i32)),
                room("b", (256, 256), cell(0_i32)),
            ],
            vec![link("c1", ("a", "b"), ConnectionKind::Opening, 64)],
            Vec::new(),
        );
        let (mut wide, _) = built(&roomy, MapFormat::Doom);
        stand(&mut wide, 1, middle(0), 7);
        let through = analyze_playability(&wide, FlagFamily::Doom, Skill::Medium);
        assert_eq!(through.reachable.len(), 3, "hall, band and back room");
        assert!(
            through.narrow_lines.is_empty(),
            "and a 64-wide opening is not a squeeze: {:?}",
            through.narrow_lines
        );
    }

    #[test]
    fn a_map_without_a_player_start_says_so_and_reaches_nothing() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.things.clear();
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.start_sector, None, "nobody spawns here");
        assert!(found.reachable.is_empty(), "and so nothing is reached");
        let notes = notes_about(&found, "no player 1 start");
        assert_eq!(notes.len(), 1, "the gap is reported: {notes:?}");
    }

    #[test]
    fn a_start_standing_in_the_void_is_no_start() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.things = vec![spawn(1, 4_000, 4_000, 0)];
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.start_sector, None, "the void holds no sector");
        let notes = notes_about(&found, "outside every sector");
        assert_eq!(notes.len(), 1, "and that is what is said: {notes:?}");
    }

    #[test]
    fn a_second_player_start_is_reported_and_the_first_is_used() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        map.things.push(spawn(1, 128, 64, 0));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(found.start_sector, Some(0), "thing 0 starts the walk");
        let notes = notes_about(&found, "2 player 1 starts");
        assert_eq!(notes.len(), 1, "the crowd is noted: {notes:?}");
    }

    #[test]
    fn specials_outside_the_tables_are_counted_not_guessed() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (12_160_i32, 0_i32, TWO_SIDED));
        wire(&mut map, 1, (12_160_i32, 0_i32, BLOCKING));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        let notes = notes_about(&found, "unhandled special 12160");
        assert_eq!(
            notes.len(),
            1,
            "one count for the whole special: {:?}",
            found.warnings
        );
        assert!(
            notes.first().is_some_and(|note| note.contains("2 line(s)")),
            "and both lines are counted: {notes:?}"
        );
    }

    #[test]
    fn a_boom_generalized_line_is_unhandled_like_any_other() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (0x2F_81_i32, 1_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert!(
            !found
                .warnings
                .iter()
                .any(|warning| warning.contains("generalized")),
            "the analysis does not decode Boom bit patterns: {:?}",
            found.warnings
        );
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "and the line is still a gap"
        );
    }

    #[test]
    fn a_doom_tagged_line_answered_by_every_untagged_sector() {
        let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32), slab(0_i32)]);
        wire(&mut map, 0, (2_i32, 0_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2, 3]),
            "a W1 door carrying no tag raises the ceiling of every untagged sector, which is \
             exactly the collision vanilla warns mappers about"
        );
        assert_eq!(
            notes_about(&found, "carries no tag").len(),
            1,
            "and the report says which line did it: {:?}",
            found.warnings
        );
    }

    #[test]
    fn a_door_acting_on_nothing_names_the_line_at_fault() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (2_i32, 9_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        let notes = notes_about(&found, "line 0 special 2 acts on no sector");
        assert_eq!(notes.len(), 1, "the inert door is named: {notes:?}");
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "and the gap it fails to open is still walkable"
        );
    }

    #[test]
    fn a_hexen_door_close_special_is_ignored_in_silence() {
        let mut map = hall(&[cell(0_i32), cell(0_i32)]);
        wire(&mut map, 0, (10_i32, 0_i32, TWO_SIDED | PLAYER_USE));
        hexen_start(&mut map);
        let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert!(
            notes_about(&found, "special 10").is_empty(),
            "a closing door is not an unhandled special: {:?}",
            found.warnings
        );
    }

    #[test]
    fn a_door_tag_that_lifts_a_sector_reports_the_platform() {
        let mut map = hall(&[cell(0_i32), slab(0_i32)]);
        map.sectors.get_mut(1).expect("the band").tag = 4_i32;
        wire(&mut map, 0, (111_i32, 4_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1]),
            "S1 open door 111 raises the tagged band for a player in front"
        );
        assert!(
            found.doors_never_opened.is_empty(),
            "the tagged door opened"
        );
    }

    #[test]
    fn an_empty_map_analyses_as_nothing_reached() {
        let map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Easy);
        assert_eq!(found.start_sector, None, "no map, no start");
        assert!(found.reachable.is_empty(), "nothing to stand in");
        assert!(found.exits.is_empty(), "and nothing to report about it");
        assert!(
            found
                .warnings
                .first()
                .is_some_and(|warning| warning.contains("no player 1 start")),
            "the missing start is still the first thing said: {:?}",
            found.warnings
        );
    }

    #[test]
    fn the_report_is_reproducible_and_defaults_empty() {
        let map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
        let once = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        let twice = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(once, twice, "the same map reads the same way twice");
        let empty = Playability::default();
        assert_eq!(empty.start_sector, None, "the default report has no player");
        assert!(
            empty.reachable.is_empty() && empty.warnings.is_empty(),
            "and nothing else either"
        );
    }
}
