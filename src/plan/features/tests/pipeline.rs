//! The whole pipeline's tests: the rich plan's fixed row order and
//! survival through the rebuild, and the playable checks' key, exit and
//! remote-door warnings.

use super::super::{FeatureRow, playable_checks};
use super::lifts::{join, lift_feature};
use super::switches::{door_features, exit_feature, switch_feature};
use super::teleporters::teleporter_feature;
use super::{back_sector, door, door_plan, front_sector, line_at, rect_room, two_rooms, wired};
use crate::defs::{Definitions, definitions_for};
use crate::plan::model::{ConnectionKind, DoorStyle, ExitStyle, KeyColor, Plan, Side};
use crate::wad::game::Game;
use crate::wad::map::features::LightEffect;
use crate::wad::map::model::Thing;

/// A three-room plan asking for everything the pipeline installs:
/// a remote door and its switch, a lift, a teleporter, an exit and
/// a lit secret.
fn rich_plan() -> Plan {
    let mut upper = rect_room("b", (0, 128), (128, 128));
    upper.floor = 64_i32;
    let mut crypt = rect_room("c", (128, 0), (128, 128));
    crypt.secret = true;
    crypt.light_effect = Some(LightEffect::StrobeSlow);
    Plan {
        rooms: vec![rect_room("a", (0, 0), (128, 128)), upper, crypt],
        connections: vec![
            join(
                "d1",
                "a",
                "c",
                ConnectionKind::Door {
                    style: DoorStyle::Remote,
                },
            ),
            join("o1", "a", "b", ConnectionKind::Opening),
        ],
        features: vec![
            lift_feature("b", true),
            teleporter_feature("a", "b", 0, false),
            switch_feature("a", "d1", None, false),
            exit_feature("c", ExitStyle::Switch, false, Some(Side::East)),
        ],
        wall_thickness: 0_i64,
    }
}

/// The first row of class `kind`.
#[track_caller]
fn row<'rows>(rows: &'rows [FeatureRow], kind: &str) -> &'rows FeatureRow {
    rows.iter()
        .find(|entry| entry.kind == kind)
        .unwrap_or_else(|| panic!("a {kind} row"))
}

#[test]
fn the_rows_come_back_in_the_fixed_pipeline_order() {
    let (_, rows) = wired(&rich_plan());
    let kinds: Vec<&str> = rows.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            "door",
            "switch",
            "lift",
            "teleporter",
            "exit",
            "light",
            "secret"
        ],
        "the pipeline order is fixed whatever the plan's order"
    );
    assert_eq!(row(&rows, "door").tag, Some(1_i32), "the door tags first");
    assert_eq!(
        row(&rows, "switch").tag,
        Some(1_i32),
        "the switch reuses it"
    );
    assert_eq!(row(&rows, "lift").tag, Some(2_i32), "the lift mints next");
    assert_eq!(
        row(&rows, "teleporter").tag,
        Some(3_i32),
        "the teleporter follows"
    );
    assert_eq!(row(&rows, "exit").tag, None, "the exit carries no tag");
    assert_eq!(row(&rows, "light").tag, None, "and so does the light");
    assert_eq!(row(&rows, "secret").tag, None, "or the secret");
}

#[test]
fn the_rich_plan_survives_the_rebuild_lit_and_wired() {
    let (compiled, rows) = wired(&rich_plan());
    let crypt = compiled.room_sectors.get("c").expect("room c");
    let space = compiled
        .map
        .sectors
        .get(*crypt.first().expect("one sector"))
        .expect("the crypt");
    assert_eq!(
        space.special, 131_i32,
        "the lit secret survives in the order"
    );
    let platform = row(&rows, "lift")
        .sectors
        .first()
        .copied()
        .expect("one platform");
    let lower = *compiled
        .room_sectors
        .get("a")
        .expect("room a")
        .first()
        .expect("one");
    for &line in &row(&rows, "lift").lines {
        let def = line_at(&compiled.map, line);
        assert_eq!(
            def.special, 62_i32,
            "the lift lines still name the ride after the teleporter rebuild"
        );
        assert_eq!(
            back_sector(&compiled.map, def),
            Some(platform),
            "the rebuild leaves the platform behind ride line {line}"
        );
        assert_eq!(
            front_sector(&compiled.map, def),
            lower,
            "and the lower room faces ride line {line} from the front"
        );
    }
    let panel = row(&rows, "switch")
        .lines
        .first()
        .copied()
        .expect("one panel line");
    assert_eq!(
        line_at(&compiled.map, panel).special,
        103_i32,
        "the panel keeps its action"
    );
    let warnings = &compiled.warnings;
    assert!(
        warnings
            .iter()
            .any(|note| note.contains("sector") && note.contains("special 131")),
        "the boom note rides along: {warnings:?}"
    );
}

/// The embedded `DOOM` thing tables the key check reads.
fn doom_defs() -> Definitions {
    definitions_for(Game::Doom).expect("the embedded tables parse")
}

/// Wire `plan`, stand things of these doomednums into the finished
/// map and run the playability checks.
#[track_caller]
fn checked(plan: &Plan, kinds: &[i32]) -> Vec<String> {
    let (mut compiled, _) = wired(plan);
    compiled.map.things.extend(kinds.iter().map(|&kind| Thing {
        kind,
        ..Default::default()
    }));
    playable_checks(plan, &compiled.map, &doom_defs())
}

/// The one-door `style` plan carrying an `exit` row, so only a key
/// or remote warning can speak; the door's id is `d1`.
fn door_feature_plan(style: DoorStyle) -> Plan {
    let mut plan = door_plan(style);
    plan.features = vec![exit_feature("b", ExitStyle::WalkOver, false, None)];
    plan
}

#[test]
fn locked_doors_without_their_keys_warn_per_door_in_connection_order() {
    let plan = two_rooms(vec![
        door("gate", DoorStyle::Locked(KeyColor::Blue), (128_i64, 32_i64)),
        door("vault", DoorStyle::Locked(KeyColor::Red), (128_i64, 96_i64)),
    ]);
    assert_eq!(
        checked(&plan, &[]),
        vec![
            "blue door at gate but no blue key in the plan".to_owned(),
            "red door at vault but no red key in the plan".to_owned(),
            "the map has no exit".to_owned(),
        ],
        "the key notes lead in connection order, the exit note follows"
    );
}

#[test]
fn the_matching_key_or_skull_key_silences_the_door_warning() {
    let plan = door_feature_plan(DoorStyle::Locked(KeyColor::Blue));
    for (kind, who) in [(5_i32, "the card"), (40_i32, "the skull key")] {
        assert!(checked(&plan, &[kind]).is_empty(), "{who} opens it");
    }
}

#[test]
fn a_wrong_colour_key_leaves_the_door_warned() {
    let plan = door_feature_plan(DoorStyle::Locked(KeyColor::Blue));
    assert_eq!(
        checked(&plan, &[6_i32]),
        vec!["blue door at d1 but no blue key in the plan".to_owned()],
        "a yellow card opens nothing"
    );
}

#[test]
fn an_unnamed_locked_door_warns_under_its_derived_label() {
    let mut plan = door_feature_plan(DoorStyle::Locked(KeyColor::Yellow));
    if let Some(first) = plan.connections.first_mut() {
        first.id = None;
    }
    assert_eq!(
        checked(&plan, &[]),
        vec!["yellow door at a->b but no yellow key in the plan".to_owned()]
    );
}

#[test]
fn a_plan_without_an_exit_row_says_so_plainly() {
    let plan = two_rooms(vec![door("d1", DoorStyle::Normal, (128_i64, 32_i64))]);
    assert_eq!(
        checked(&plan, &[]),
        vec!["the map has no exit".to_owned()],
        "a plain door needs no key and no switch"
    );
}

#[test]
fn an_unswitched_remote_door_names_itself_and_the_way_out() {
    let plan = door_feature_plan(DoorStyle::Remote);
    assert_eq!(
        checked(&plan, &[]),
        vec![
            "remote door at d1 is opened by no switch: add a switch feature whose \
             opens is d1"
                .to_owned()
        ]
    );
}

#[test]
fn a_switch_naming_the_remote_door_silences_it() {
    let plan = door_features(vec![
        switch_feature("a", "d1", Some(Side::West), false),
        exit_feature("b", ExitStyle::WalkOver, false, None),
    ]);
    assert!(checked(&plan, &[]).is_empty(), "the pair is complete");
}

#[test]
fn an_unnamed_remote_door_asks_for_an_id_a_switch_can_reference() {
    let mut plan = door_features(vec![exit_feature("b", ExitStyle::WalkOver, false, None)]);
    if let Some(first) = plan.connections.first_mut() {
        first.id = None;
    }
    assert_eq!(
        checked(&plan, &[]),
        vec![
            "remote door at a->b is opened by no switch: give the connection an id and \
             reference it in a switch feature's opens"
                .to_owned()
        ],
        "an unnamed door cannot be opened until it is named"
    );
}
