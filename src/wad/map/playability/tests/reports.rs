//! What the finished report says about a map beyond the walk itself: the
//! narrow lines a player just misses, the missing and crowded player
//! starts, the specials outside the tables and the tags answering nothing,
//! the plain [`feature_targets`] read and the report's own defaults.

use alloc::vec;

use super::super::{FeatureTargets, Playability, Teleporter, analyze_playability, feature_targets};
use crate::plan::model::ConnectionKind;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::{name, spawn};
use crate::wad::map::flags::{BLOCKING, FlagFamily, PLAYER_USE_BITS, Skill, TWO_SIDED};

use super::{
    TELEPORT_DEST, built, cell, hall, hexen_start, link, middle, notes_about, plan, room, sectors,
    slab, stand, wire,
};

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
    wire(&mut map, 0, (10_i32, 0_i32, TWO_SIDED | PLAYER_USE_BITS));
    hexen_start(&mut map);
    let found = analyze_playability(&map, FlagFamily::Hexen, Skill::Medium);
    assert!(
        notes_about(&found, "special 10").is_empty(),
        "a closing door is not an unhandled special: {:?}",
        found.warnings
    );
}

#[test]
fn a_doom_door_close_special_is_ignored_in_silence_too() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (50_i32, 0_i32, TWO_SIDED));
    let found = analyze_playability(&map, FlagFamily::Doom, Skill::Medium);
    assert!(
        notes_about(&found, "special 50").is_empty(),
        "S1 Door Close is the Doom family's own shutter row: known, ignored, and \
         never warned as unhandled or tagless: {:?}",
        found.warnings
    );
    assert!(
        found.doors_never_opened.is_empty(),
        "and it opens nothing the walk waits for"
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
fn feature_targets_reads_nothing_from_a_plain_hall() {
    let map = hall(&[cell(0_i32), cell(0_i32)]);
    let found = feature_targets(&map, FlagFamily::Doom);
    assert!(
        found.doors.is_empty() && found.door_lines.is_empty(),
        "no doors"
    );
    assert!(
        found.lifts.is_empty() && found.lift_lines.is_empty(),
        "no lifts"
    );
    assert!(found.teleporters.is_empty(), "no teleporters");
    assert_eq!(found, FeatureTargets::default(), "and so the default is it");
}

#[test]
fn feature_targets_names_a_manual_door_with_its_line() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    wire(&mut map, 0, (1_i32, 0_i32, TWO_SIDED));
    let found = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(found.doors, sectors(&[1]), "the band behind the use line");
    assert_eq!(found.door_lines, sectors(&[0]), "the line carrying it");
    assert!(
        found.lifts.is_empty() && found.lift_lines.is_empty(),
        "no lift"
    );
    assert!(found.teleporters.is_empty(), "and no teleporter");
}

#[test]
fn feature_targets_names_a_lifts_platform_and_its_call() {
    let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the platform").tag = 3_i32;
    wire(&mut map, 0, (21_i32, 3_i32, TWO_SIDED));
    let found = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(found.lifts, sectors(&[1]), "the tagged platform");
    assert_eq!(found.lift_lines, sectors(&[0]), "called from the divider");
    assert!(
        found.doors.is_empty() && found.door_lines.is_empty(),
        "no door"
    );
    assert!(found.teleporters.is_empty(), "and no teleporter");
}

#[test]
fn feature_targets_names_a_teleporter_and_where_it_lands() {
    let mut map = hall(&[cell(0_i32), cell(0_i32)]);
    map.sectors.get_mut(1).expect("the far room").tag = 5_i32;
    wire(&mut map, 0, (97_i32, 5_i32, TWO_SIDED | BLOCKING));
    stand(&mut map, TELEPORT_DEST, middle(1), 7);
    let found = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(
        found.teleporters,
        vec![Teleporter {
            line: 0,
            destination: Some(1),
        }],
        "the pad's line and the room its destination thing stands in"
    );
    assert!(
        found.doors.is_empty() && found.lifts.is_empty(),
        "a teleport moves no sector: {found:?}"
    );
    assert!(
        found.door_lines.is_empty() && found.lift_lines.is_empty(),
        "and no line"
    );

    map.things.retain(|thing| thing.kind != TELEPORT_DEST);
    let stranded = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(
        stranded.teleporters,
        vec![Teleporter {
            line: 0,
            destination: None,
        }],
        "strip the destination thing and the pad is still a teleporter"
    );
}

#[test]
fn feature_targets_follows_the_flag_family_it_is_given() {
    let mut map = hall(&[cell(0_i32), cell(96_i32), cell(96_i32)]);
    map.sectors.get_mut(1).expect("the platform").tag = 3_i32;
    wire(&mut map, 0, (21_i32, 3_i32, TWO_SIDED));
    let doom = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(doom.lifts, sectors(&[1]), "special 21 is a Doom lift call");
    let at_hexen = feature_targets(&map, FlagFamily::Hexen);
    assert!(
        at_hexen.lifts.is_empty() && at_hexen.lift_lines.is_empty(),
        "and 21 is nobody's action in the Hexen table: {at_hexen:?}"
    );
}

#[test]
fn feature_targets_answers_for_every_feature_a_map_holds() {
    let mut map = hall(&[cell(0_i32), slab(0_i32), cell(96_i32), cell(0_i32)]);
    map.sectors.get_mut(2).expect("the platform").tag = 6_i32;
    map.sectors.get_mut(3).expect("the far room").tag = 5_i32;
    wire(&mut map, 0, (1_i32, 0_i32, TWO_SIDED));
    wire(&mut map, 1, (21_i32, 6_i32, TWO_SIDED));
    wire(&mut map, 3, (97_i32, 5_i32, BLOCKING));
    stand(&mut map, TELEPORT_DEST, middle(3), 7);
    let found = feature_targets(&map, FlagFamily::Doom);
    assert_eq!(found.doors, sectors(&[1]), "the band the door line opens");
    assert_eq!(found.door_lines, sectors(&[0]), "on its own line");
    assert_eq!(found.lifts, sectors(&[2]), "the platform the lift calls");
    assert_eq!(found.lift_lines, sectors(&[1]), "on its own line");
    assert_eq!(
        found.teleporters,
        vec![Teleporter {
            line: 3,
            destination: Some(3),
        }],
        "and the pad whose tag answers the far room: {found:?}"
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
