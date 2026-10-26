//! Unit tests of the definitions registry: the per-game table selection
//! and sorting, the lookups the indexes must answer exactly as the linear
//! scans did, the overlay mutators keeping the indexes true, and the
//! fallback games' notes.

use super::{DefinitionTables, Definitions, definitions_for, game_label};
use crate::defs::boom::{SectorFamily, decode_sector_special};
use crate::defs::table::{Category, ThingDef};
use crate::wad::game::Game;

/// Whether any row in `defs` has this type number.
fn has_kind(defs: &Definitions, kind: u32) -> bool {
    defs.things.iter().any(|row| row.kind == kind)
}

/// A [`ThingDef`] row of type `kind` named `name`, for the index fixtures.
fn thing_def(kind: u32, name: &str) -> ThingDef {
    ThingDef {
        kind,
        name: name.to_owned(),
        sprite: String::new(),
        frame: 'A',
        radius: 16_u32,
        category: "Monsters".to_owned(),
        group: Category::Monsters,
        hanging: false,
        game: "doom".to_owned(),
        source: "test".to_owned(),
    }
}

/// Whether `numbers` are in ascending order.
fn is_ascending(numbers: &[u32]) -> bool {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted == numbers
}

#[test]
fn doom_excludes_the_doom_ii_only_things() {
    let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
    assert_eq!(defs.game, Game::Doom, "the game is echoed");
    assert_eq!(defs.things.len(), 97, "the doom rows of the thing table");
    assert!(!has_kind(&defs, 65), "Chaingunner is a DOOM II thing");
    assert!(has_kind(&defs, 3001), "Imp is a DOOM thing");
    assert!(defs.warnings.is_empty(), "DOOM has its own table");
}

#[test]
fn doom2_adds_the_doom_ii_only_things() {
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    assert_eq!(defs.things.len(), 123, "both games' rows");
    assert!(has_kind(&defs, 65), "Chaingunner is there");
    assert!(has_kind(&defs, 3001), "and the DOOM things as well");
    assert!(defs.warnings.is_empty(), "DOOM II has its own table");
}

#[test]
fn things_are_sorted_by_type_number() {
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    let sorts: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
    let mut sorted = sorts.clone();
    sorted.sort_unstable();
    assert_eq!(sorts, sorted, "type numbers ascend");
}

#[test]
fn doom_excludes_the_doom_ii_only_specials() {
    let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
    assert_eq!(
        defs.specials.len(),
        100,
        "the doom rows of the special table"
    );
    assert!(defs.special(1).is_some(), "DR Door is a DOOM special");
    assert!(defs.special(117).is_none(), "DR Door Fast is DOOM II only");
    let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
    assert!(is_ascending(&numbers), "special numbers ascend");
}

#[test]
fn doom2_adds_the_doom_ii_only_specials() {
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    assert_eq!(defs.specials.len(), 139, "both games' rows");
    let fast = defs.special(117).expect("DR Door Fast exists");
    assert_eq!(fast.name, "DR Door Fast", "name");
    assert_eq!(fast.category, "Doors/Local", "category");
    assert_eq!(fast.target, "sector_back", "target column");
    assert!(
        fast.args.is_empty(),
        "Doom-family specials take no arguments"
    );
    assert_eq!(fast.game, "doom2", "first game");
    assert!(defs.special(1).is_some(), "and the DOOM specials as well");
}

#[test]
fn sector_types_are_shared_by_the_doom_games() {
    let doom = definitions_for(Game::Doom).expect("the embedded table is valid");
    let doom2 = definitions_for(Game::Doom2).expect("the embedded table is valid");
    assert_eq!(doom.sector_types.len(), 16, "the shipped sector rows");
    assert_eq!(
        doom.sector_types, doom2.sector_types,
        "both games use the same table"
    );
    let damage = doom.sector_type(16).expect("type 16 exists");
    assert_eq!(damage.name, "20% Damage", "name lookup");
    assert!(doom.sector_type(6).is_none(), "type 6 is unassigned");
    let numbers: Vec<u32> = doom.sector_types.iter().map(|row| row.number).collect();
    assert!(is_ascending(&numbers), "type numbers ascend");
}

#[test]
fn the_boom_decoder_names_every_sector_table_row() {
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    for row in &defs.sector_types {
        let decoded =
            decode_sector_special(row.number, SectorFamily::Doom).expect("the Doom family decodes");
        assert_eq!(decoded.base, row.number, "the base is the row itself");
        assert_eq!(
            decoded.base_name, row.name,
            "boom and the table agree on type {}",
            row.number
        );
    }
}

#[test]
fn heretic_ships_its_own_tables() {
    let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
    assert_eq!(defs.game, Game::Heretic, "the game is echoed");
    assert_eq!(defs.things.len(), 84, "the heretic thing rows");
    assert_eq!(defs.specials.len(), 105, "the heretic special rows");
    assert_eq!(defs.sector_types.len(), 47, "the heretic sector rows");
    assert!(defs.warnings.is_empty(), "heretic has its own tables");
    let kinds: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
    assert!(is_ascending(&kinds), "thing types ascend");
    let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
    assert!(is_ascending(&numbers), "special numbers ascend");
    let types: Vec<u32> = defs.sector_types.iter().map(|row| row.number).collect();
    assert!(is_ascending(&types), "sector types ascend");
}

#[test]
fn heretic_lookups_name_the_heretic_rows() {
    let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
    let gargoyle = defs.thing(66).expect("type 66 is the Gargoyle");
    assert_eq!(gargoyle.name, "Gargoyle", "name");
    assert_eq!(gargoyle.sprite, "IMPX", "sprite");
    assert_eq!(gargoyle.frame, 'A', "frame");
    assert_eq!(gargoyle.radius, 20, "radius");
    assert_eq!(gargoyle.category, "Monsters", "category");
    assert_eq!(gargoyle.game, "heretic", "game column");
    let lich = defs.thing(6).expect("type 6 is the Iron Lich");
    assert_eq!(lich.radius, 40, "a wide boss radius");
    let corpse = defs.thing(51).expect("type 51 is the Hanging Corpse");
    assert!(corpse.hanging, "the gore rows carry the hanging flag");
    let door = defs.special(100).expect("special 100 is a heretic door");
    assert_eq!(door.name, "WR Door Triple Speed", "name");
    assert_eq!(door.category, "Doors/Remote", "category");
    assert_eq!(door.target, "sector", "target column");
    assert!(door.args.is_empty(), "heretic specials take no arguments");
    let exit = defs.special(11).expect("special 11 is S1 Exit");
    assert_eq!(exit.target, "-", "an exit acts on no sector");
    let scroll = defs.sector_type(20).expect("sector type 20 exists");
    assert_eq!(scroll.name, "Scroll East (5)", "name");
    let damage = defs.sector_type(5).expect("sector type 5 exists");
    assert_eq!(damage.name, "5% Lava Damage", "heretic damage naming");
    assert!(defs.sector_type(6).is_none(), "type 6 is unassigned");
    assert!(
        defs.sector_type(17).is_none(),
        "types 17 to 19 are unassigned"
    );
    assert!(defs.sector_type(51).is_some(), "the table ends at type 51");
    assert!(defs.thing(3_001).is_none(), "no Doom monsters in here");
}

#[test]
fn heretic_tables_hold_no_doom_rows() {
    let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
    assert!(
        defs.things.iter().all(|row| row.game == "heretic"),
        "no Doom thing rows leaked in"
    );
    assert!(
        defs.specials.iter().all(|row| row.game == "heretic"),
        "no Doom special rows leaked in"
    );
    assert!(!has_kind(&defs, 3001), "the Imp is not a heretic thing");
    assert!(
        defs.special(117).is_none(),
        "DR Door Fast is a Doom II special"
    );
    let start = defs.thing(5).expect("heretic has a type 5");
    assert_eq!(start.name, "Fire Gargoyle", "not the Doom Blue Keycard");
    let pod = defs.thing(2035).expect("heretic has a type 2035");
    assert_eq!(pod.name, "Gas Pod", "not the Doom Barrel");
    assert_eq!(pod.sprite, "PPOD", "and its own sprite");
    let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let stairs = doom2.special(100).expect("DOOM II has a special 100");
    assert_eq!(stairs.name, "W1 Build Stairs 16 + Crush", "its own meaning");
}

#[test]
fn the_doom_tables_hold_no_heretic_rows() {
    let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    assert!(
        doom2.things.iter().all(|row| row.game != "heretic"),
        "the Doom thing sets stay as they were"
    );
    assert_eq!(doom2.things.len(), 123, "DOOM II keeps its own count");
    assert_eq!(doom2.specials.len(), 139, "DOOM II keeps its own count");
    assert_eq!(
        doom2.sector_types.len(),
        16,
        "the Doom sector table is untouched"
    );
    assert_eq!(
        doom2.thing(5).expect("DOOM II has a type 5").name,
        "Blue Keycard",
        "type 5 still means the Doom thing"
    );
}

#[test]
fn hexen_ships_its_own_tables() {
    let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
    assert_eq!(defs.game, Game::Hexen, "the game is echoed");
    assert_eq!(defs.things.len(), 233, "the hexen thing rows");
    assert_eq!(defs.specials.len(), 86, "the hexen special rows");
    assert_eq!(defs.sector_types.len(), 46, "the hexen sector rows");
    assert!(defs.warnings.is_empty(), "hexen has its own tables");
    let kinds: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
    assert!(is_ascending(&kinds), "thing types ascend");
    let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
    assert!(is_ascending(&numbers), "special numbers ascend");
    let types: Vec<u32> = defs.sector_types.iter().map(|row| row.number).collect();
    assert!(is_ascending(&types), "sector types ascend");
}

#[test]
fn hexen_lookups_name_the_hexen_rows() {
    let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
    let ettin = defs.thing(10030).expect("type 10030 is the Ettin");
    assert_eq!(ettin.name, "Ettin", "name");
    assert_eq!(ettin.sprite, "ETTN", "sprite");
    assert_eq!(ettin.frame, 'A', "frame");
    assert_eq!(ettin.radius, 25, "radius");
    assert_eq!(ettin.category, "Monsters", "category");
    assert_eq!(ettin.game, "hexen", "game column");
    let chandelier = defs.thing(17).expect("type 17 is the lit Chandelier");
    assert!(chandelier.hanging, "the gore and chandelier rows hang");
    let locked = defs.special(13).expect("special 13 is Door_LockedRaise");
    assert_eq!(locked.name, "Door_LockedRaise", "name");
    assert_eq!(locked.category, "Doors", "category");
    assert_eq!(locked.target, "sector_or_back", "free-form target column");
    let args: Vec<&str> = locked.args.iter().map(String::as_str).collect();
    assert_eq!(
        args,
        vec!["Sector Tag", "Speed", "Delay", "Lock"],
        "argument names in order, index 0 is arg1"
    );
    let acs = defs.special(80).expect("special 80 is ACS_Execute");
    assert_eq!(acs.args.len(), 5, "five script arguments");
    let endgame = defs.special(75).expect("special 75 is Teleport_EndGame");
    assert!(endgame.args.is_empty(), "a no-argument special stays empty");
    assert_eq!(endgame.target, "-", "an exit acts on nothing");
    let quake = defs.special(120).expect("special 120 is Radius_Quake");
    assert_eq!(quake.target, "ex_5thing", "ex_5thing passes through");
    let sky = defs.sector_type(200).expect("sector type 200 exists");
    assert_eq!(sky.name, "Sky 2 (MAPINFO)", "name");
    assert!(
        defs.sector_type(5).is_none(),
        "Doom damage types are not rows"
    );
    assert!(defs.thing(3_001).is_none(), "no Doom monsters in here");
}

#[test]
fn hexen_tables_hold_no_doom_or_heretic_rows() {
    let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
    assert!(
        defs.things.iter().all(|row| row.game == "hexen"),
        "no Doom or Heretic thing rows leaked in"
    );
    assert!(
        defs.specials.iter().all(|row| row.game == "hexen"),
        "no Doom or Heretic special rows leaked in"
    );
    assert!(!has_kind(&defs, 3_001), "the Imp is not a hexen thing");
    assert!(
        !defs.things.iter().any(|row| row.name == "Gargoyle"),
        "the heretic Gargoyle is not in the hexen table"
    );
    let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    assert!(
        doom2.things.iter().all(|row| row.game != "hexen")
            && doom2.specials.iter().all(|row| row.game != "hexen"),
        "the Doom sets stay free of hexen rows"
    );
    let heretic = definitions_for(Game::Heretic).expect("the embedded tables are valid");
    assert!(
        heretic.things.iter().all(|row| row.game != "hexen")
            && heretic.specials.iter().all(|row| row.game != "hexen"),
        "the heretic sets stay free of hexen rows"
    );
}

#[test]
fn strife_and_unknown_fall_back_to_the_doom2_table() {
    for game in [Game::Strife, Game::Unknown] {
        let defs = definitions_for(game).expect("the embedded tables are valid");
        assert_eq!(defs.game, game, "the requested game is echoed");
        assert_eq!(defs.things.len(), 123, "the DOOM II things for {game:?}");
        assert_eq!(
            defs.specials.len(),
            139,
            "the DOOM II specials for {game:?}"
        );
        assert_eq!(defs.sector_types.len(), 16, "the sector table for {game:?}");
        assert!(
            defs.warnings.first().is_some_and(|w| w.contains("DOOM II")),
            "the warning names the fallback: {:?}",
            defs.warnings
        );
    }
}

#[test]
fn thing_returns_the_matching_definition() {
    let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
    let imp = defs.thing(3001).expect("DOOM has the Imp");
    assert_eq!(imp.name, "Imp", "name");
    assert_eq!(imp.sprite, "TROO", "sprite");
    assert_eq!(imp.frame, 'A', "frame");
    assert_eq!(imp.radius, 20, "radius");
    assert_eq!(imp.category, "Monsters", "category");
    assert!(!imp.hanging, "the Imp does not hang");
    assert_eq!(imp.game, "doom", "first game");
    assert!(
        defs.thing(65).is_none(),
        "Chaingunner is not in the DOOM set"
    );
    assert!(defs.thing(0).is_none(), "type 0 exists in no game");
}

#[test]
fn the_hanging_flag_survives_parsing() {
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    let keen = defs.thing(72).expect("Hanging Keen exists");
    assert!(keen.hanging, "the flags column marked it hanging");
    assert_eq!(keen.game, "doom2", "it is a DOOM II thing");
}

#[test]
fn game_labels_match_the_wire_names() {
    for game in [
        Game::Doom,
        Game::Doom2,
        Game::Heretic,
        Game::Hexen,
        Game::Strife,
        Game::Unknown,
    ] {
        assert_eq!(
            serde_json::to_value(game).expect("games serialize"),
            serde_json::Value::String(game_label(game).to_owned()),
            "warning text and wire name agree for {game:?}"
        );
    }
}

#[test]
fn indexed_lookups_answer_exactly_what_a_linear_scan_does() {
    for game in [
        Game::Doom,
        Game::Doom2,
        Game::Heretic,
        Game::Hexen,
        Game::Strife,
        Game::Unknown,
    ] {
        let defs = definitions_for(game).expect("the embedded tables are valid");
        for kind in 0..=2_000_u32 {
            let linear = defs.things.iter().find(|row| row.kind == kind);
            assert_eq!(defs.thing(kind), linear, "thing {kind} for {game:?}");
        }
        for kind in [2_001_u32, 30_000_u32, 4_000_000_u32] {
            let linear = defs.things.iter().find(|row| row.kind == kind);
            assert_eq!(defs.thing(kind), linear, "far thing {kind} for {game:?}");
        }
        for number in 0..=150_u32 {
            let linear = defs.specials.iter().find(|row| row.number == number);
            assert_eq!(
                defs.special(number),
                linear,
                "special {number} for {game:?}"
            );
        }
        for value in 0..=210_u32 {
            let linear = defs.sector_types.iter().find(|row| row.number == value);
            assert_eq!(
                defs.sector_type(value),
                linear,
                "sector {value} for {game:?}"
            );
        }
        for name in defs.things.iter().map(|row| row.name.clone()).chain([
            "imp".to_owned(),
            "IMP".to_owned(),
            "no such thing".to_owned(),
        ]) {
            let lowered = name.to_ascii_lowercase();
            let linear = defs
                .things
                .iter()
                .find(|row| row.name.to_ascii_lowercase() == lowered);
            assert_eq!(
                defs.thing_by_name(&name),
                linear,
                "name {name:?} for {game:?}"
            );
        }
    }
}

#[test]
fn a_duplicated_number_or_name_keeps_its_first_row() {
    let defs = Definitions::new(
        Game::Doom2,
        DefinitionTables {
            things: vec![
                thing_def(70, "First"),
                thing_def(70, "Second"),
                thing_def(71, "first"),
            ],
            ..DefinitionTables::default()
        },
    );
    assert_eq!(
        defs.thing(70).map(|row| row.name.as_str()),
        Some("First"),
        "the first row of a number wins, as in a linear first match"
    );
    assert_eq!(
        defs.thing_by_name("FIRST").map(|row| row.kind),
        Some(70),
        "the first row of a name wins, case-insensitively"
    );
}

#[test]
fn put_and_remove_keep_the_thing_indexes_true() {
    let mut defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    defs.remove_thing(3_001);
    assert!(
        defs.thing(3_001).is_none(),
        "the number leaves the lookup too"
    );
    assert!(defs.thing(3_002).is_some(), "the neighbours still answer");
    assert!(
        defs.thing_by_name("imp").is_none(),
        "the name index follows"
    );
    defs.put_thing(thing_def(3_001, "Imp"));
    let imp = defs.thing(3_001).expect("the re-placed row answers again");
    assert_eq!(imp.name, "Imp", "and is the one found");
    assert_eq!(
        defs.things.last().map(|row| row.kind),
        Some(3_001_u32),
        "put pushes to the end - the overlay sorts at the close of the run"
    );
}

#[test]
fn thing_mut_edits_the_indexed_row_in_place() {
    let mut defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    if let Some(row) = defs.thing_mut(8) {
        row.radius = 3_u32;
    }
    assert_eq!(
        defs.thing(8).map(|row| row.radius),
        Some(3_u32),
        "the edit stands under the same index"
    );
    assert!(
        defs.thing_mut(4_000_000).is_none(),
        "an unknown number mutates nothing"
    );
}

#[test]
fn reindex_follows_a_direct_reorder_of_the_table() {
    let mut defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    defs.things.reverse();
    defs.reindex_things();
    let imp = defs.thing(3_001).expect("the Imp still answers");
    assert_eq!(
        imp.name, "Imp",
        "after the reorder the index points at the row"
    );
    assert_eq!(
        defs.thing(5).map(|row| row.name.as_str()),
        Some("Blue Keycard"),
        "for every number"
    );
}
