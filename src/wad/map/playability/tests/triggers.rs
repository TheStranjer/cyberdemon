//! How a player sets a line off, read through the walk: the `Use` press and
//! the `Walk` crossing from the [`movement`](super::movement) topic, beside
//! the `Shoot` gun specials (the `Doom` family's `G1` lines, the `Hexen`
//! family's `impact`, `playerpush` and `missilecross` fields) that work from
//! the front side only, the `Monster` activation (`monstercross`) that no
//! player ever sets off, and the `Boom` generalized door, locked door and
//! lift patterns that [`doom_row`](crate::wad::map::specials::doom_row)
//! decodes into the walk's own rows.

use super::super::{BlockerReason, ExitInfo, KeyColor, analyze_playability, blockers};
use crate::wad::map::flags::{BLOCKING, FlagFamily, PLAYER_USE_BITS, Skill, TWO_SIDED};
use crate::wad::map::model::Map;

use super::{
    BLUE_CARD, HEXEN_PRESENCE, TELEPORT_DEST, cell, hall, hexen_start, middle, notes_about,
    sectors, slab, stand, wire,
};

/// The `Hexen` activation fields at their position in the flag bits;
/// `playeruse` comes from [`flags`](crate::wad::map::flags) itself, and
/// the all-clear `playercross` field sets no bit at all.
const WALK_BITS: u32 = 0_u32;
const MONSTER_CROSS_BITS: u32 = 0x800_u32;
const IMPACT_BITS: u32 = 0xC00_u32;
const PLAYER_PUSH_BITS: u32 = 0x1000_u32;
const MISSILE_CROSS_BITS: u32 = 0x1400_u32;

/// The `Boom` generalized patterns the tests below wire, spelled the way
/// [`crate::defs::boom`] lays the bits out.
const GENERALIZED_DOOR_WR: i32 = 0x3C01_i32;
const GENERALIZED_DOOR_LOCKED_BLUE_SR: i32 = 0x38A3_i32;
const GENERALIZED_DOOR_LOCKED_ANY_WR: i32 = 0x3801_i32;
const GENERALIZED_LIFT_WR: i32 = 0x3401_i32;

/// Swaps `line`'s sides, so its front faces what used to be behind it.
fn flip(map: &mut Map, line: usize) {
    if let Some(def) = map.linedefs.get_mut(line) {
        let (front, back) = (def.front, def.back);
        def.front = back;
        def.back = front;
    }
}

/// A hall with a shut door band in the middle: rooms 0 and 2, band 1, the
/// divider between the hall and the band carrying `action`.
fn wired_door(action: (i32, i32, u32)) -> Map {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the band").tag = 4_i32;
    wire(&mut map, 0, action);
    map
}

/// A `Hexen` hall with a special 12 door band whose `args[0]` answers the
/// tagged band (sector 4, like [`wired_door`] lays it out) and whose
/// activation field is `bits`.
fn hexen_door(bits: u32) -> Map {
    let mut map = wired_door((12_i32, 0_i32, TWO_SIDED | bits));
    map.linedefs.get_mut(0).expect("the divider").args = [4_i32, 16_i32, 150_i32, 0_i32, 0_i32];
    hexen_start(&mut map);
    map
}

#[test]
fn a_doom_gun_door_opens_for_a_player_on_its_front_side() {
    let map = wired_door((46_i32, 4_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "G1 open door 46: a shot from the hall raises the band"
    );
}

#[test]
fn a_player_behind_a_gun_door_cannot_shoot_it_from_behind() {
    let mut map = wired_door((46_i32, 4_i32, TWO_SIDED));
    flip(&mut map, 0);
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "the modelled shot reaches the front face only, and that face now looks into \
         the band nobody stands in"
    );
    assert_eq!(found.doors_never_opened, vec![1], "the band stayed shut");
}

#[test]
fn a_gun_exit_counts_for_a_player_in_front_of_it_only() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (197_i32, 0_i32, TWO_SIDED | BLOCKING));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.exits.first(),
        Some(&ExitInfo {
            line: 0,
            special: 197,
            secret: false,
            reachable: true,
        }),
        "the hall side is the front: shooting across it ends the level"
    );
    let mut turned = map.clone();
    flip(&mut turned, 0);
    let behind = analyze_playability(&turned, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        behind.exits.first().map(|exit| exit.reachable),
        Some(false),
        "with the front facing the far room, a player who cannot reach it cannot shoot \
         the exit either"
    );
}

#[test]
fn a_monstercross_door_never_opens_for_a_player() {
    let map = hexen_door(MONSTER_CROSS_BITS);
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "only a monster's crossing works the line, and the walk is single-player"
    );
    assert_eq!(found.doors_never_opened, vec![1], "the band is named");
    assert_eq!(
        blockers(&map, FlagFamily::Hexen, &found)
            .first()
            .map(|blocker| blocker.reason),
        Some(BlockerReason::DoorNeverOpened),
        "and the blocker says no reachable player ever worked it"
    );
    assert!(
        notes_about(&found, "unhandled").is_empty(),
        "a special 12 on a monstercross line is modelled, not unhandled: {:?}",
        found.warnings
    );
}

#[test]
fn a_monstercross_exit_line_ends_nothing_for_the_walk() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(
        &mut map,
        0,
        (243_i32, 0_i32, TWO_SIDED | MONSTER_CROSS_BITS),
    );
    hexen_start(&mut map);
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1]),
        "the portal line is an open gap a player walks through"
    );
    assert_eq!(
        found.exits.first(),
        Some(&ExitInfo {
            line: 0,
            special: 243,
            secret: false,
            reachable: false,
        }),
        "and yet no crossing of it ends the level for the player"
    );
}

#[test]
fn a_monstercross_plat_leaves_the_ledge_out_of_reach() {
    let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the plat").tag = 1_i32;
    map.linedefs.get_mut(0).expect("the divider").args = [1_i32, 0_i32, 0_i32, 0_i32, 0_i32];
    hexen_start(&mut map);
    wire(&mut map, 0, (62_i32, 0_i32, TWO_SIDED | WALK_BITS));
    let crossed = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        crossed.reachable,
        sectors(&[0, 1, 2]),
        "the playercross plat drops the ledge into reach"
    );
    wire(&mut map, 0, (62_i32, 0_i32, TWO_SIDED | MONSTER_CROSS_BITS));
    let shut = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        shut.reachable,
        sectors(&[0]),
        "and with the monstercross field the plat is never called"
    );
    assert!(
        notes_about(&shut, "unhandled").is_empty(),
        "the plat is modelled, only never worked: {:?}",
        shut.warnings
    );
}

#[test]
fn the_shoot_and_push_fields_work_the_front_side_like_a_gun() {
    for bits in [IMPACT_BITS, PLAYER_PUSH_BITS, MISSILE_CROSS_BITS] {
        let map = hexen_door(bits);
        let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2]),
            "field {bits:#06x}: the player in the hall reaches the far room"
        );
        let mut behind = hexen_door(bits);
        flip(&mut behind, 0);
        let stuck = analyze_playability(&behind, FlagFamily::Hexen, Skill::Medium);
        assert_eq!(
            stuck.reachable,
            sectors(&[0]),
            "field {bits:#06x}: and a line whose front faces away keeps its answer \
             behind the band"
        );
        assert_eq!(
            stuck.doors_never_opened,
            vec![1],
            "field {bits:#06x}: the tagged band is named as never raised - a crossing \
             player works none of these fields"
        );
    }
}

#[test]
fn a_playercross_door_reaches_both_sides_and_playeruse_only_the_front() {
    let crossed = analyze_playability(&hexen_door(WALK_BITS), FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        crossed.reachable,
        sectors(&[0, 1, 2]),
        "the all-clear field walks through from either side"
    );
    let pressed = analyze_playability(
        &hexen_door(PLAYER_USE_BITS),
        FlagFamily::Hexen,
        Skill::Medium,
    );
    assert_eq!(
        pressed.reachable,
        sectors(&[0, 1, 2]),
        "playeruse works from the hall its front faces"
    );
    let mut behind = hexen_door(PLAYER_USE_BITS);
    flip(&mut behind, 0);
    let stuck = analyze_playability(&behind, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        stuck.reachable,
        sectors(&[0]),
        "playeruse pins the line to its front side: turned to face the band, nobody can \
         press it"
    );
    assert_eq!(stuck.doors_never_opened, vec![1], "and the band is named");
}

#[test]
fn a_boom_generalized_door_walks_like_a_tagged_one() {
    let map = wired_door((GENERALIZED_DOOR_WR, 4_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "a WR generalized door raises the tagged band for a player who crosses its line"
    );
    assert!(
        found.warnings.is_empty(),
        "and is fully modelled: {:?}",
        found.warnings
    );
}

#[test]
fn a_boom_generalized_locked_door_wants_its_card() {
    let map = wired_door((GENERALIZED_DOOR_LOCKED_BLUE_SR, 4_i32, TWO_SIDED));
    let shut = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        shut.reachable,
        sectors(&[0]),
        "the SR blue-card door holds without the card"
    );
    assert_eq!(
        blockers(&map, FlagFamily::Doom, &shut)
            .first()
            .map(|blocker| blocker.reason),
        Some(BlockerReason::Locked {
            color: KeyColor::Blue,
        }),
        "and names the colour it wants"
    );
    let mut carded = map;
    stand(&mut carded, BLUE_CARD, middle(0), 7);
    let opened = analyze_playability(&carded, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        opened.reachable,
        sectors(&[0, 1, 2]),
        "a blue card in the hall works the generalized lock"
    );
}

#[test]
fn an_any_key_generalized_door_is_counted_not_guessed() {
    let map = wired_door((GENERALIZED_DOOR_LOCKED_ANY_WR, 4_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    let notes = notes_about(&found, "unhandled special 14337");
    assert_eq!(
        notes.len(),
        1,
        "the any-key pattern names no colour: {:?}",
        found.warnings
    );
    assert!(
        notes
            .first()
            .is_some_and(|note| note.contains("by their flags alone")),
        "and the note says what the walk does instead of pretending: {notes:?}"
    );
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "the band it would have raised stays shut and reads unreachable"
    );
}

#[test]
fn a_boom_generalized_lift_drops_its_platform_like_a_plat() {
    let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the plat").tag = 3_i32;
    wire(&mut map, 0, (GENERALIZED_LIFT_WR, 3_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0, 1, 2]),
        "a WR generalized lift answers the tagged platform"
    );
}

#[test]
fn the_perpetual_plats_answer_their_letters() {
    for (special, use_line) in [(87_i32, false), (89_i32, true)] {
        let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
        map.sectors.get_mut(1).expect("the plat").tag = 3_i32;
        wire(&mut map, 0, (special, 3_i32, TWO_SIDED));
        let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
        assert_eq!(
            found.reachable,
            sectors(&[0, 1, 2]),
            "special {special} drops the platform for the player facing it: use_line \
             {use_line} does not change a hall whose start stands in front"
        );
    }
}

#[test]
fn a_monstercross_teleport_jumps_for_nobody() {
    let mut map = hall(&[cell(0_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
    wire(&mut map, 0, (70_i32, 0_i32, TWO_SIDED | MONSTER_CROSS_BITS));
    map.linedefs.get_mut(0).expect("the divider").args = [5_i32, 0_i32, 0_i32, 0_i32, 0_i32];
    hexen_start(&mut map);
    stand(&mut map, TELEPORT_DEST, middle(1), HEXEN_PRESENCE);
    if let Some(thing) = map.things.last_mut() {
        thing.tid = 5_i32;
    }
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        found.reachable,
        sectors(&[0]),
        "the pad's tid answers the far room, but only a monster's crossing trips it"
    );
    wire(&mut map, 0, (70_i32, 0_i32, TWO_SIDED | WALK_BITS));
    let crossed = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert_eq!(
        crossed.reachable,
        sectors(&[0, 1]),
        "under playercross the same pad jumps the step"
    );
}
