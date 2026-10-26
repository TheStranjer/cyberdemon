//! Unit tests of the definition table readers: the thing, special and
//! sector row parses with every malformed column, the [`Category`]
//! classification each row carries, and the shipped embedded tables
//! parsing row for row.
//!
//! [`Category`]: crate::defs::table::Category

use super::{Category, ThingDef, parse_sector_table, parse_special_table, parse_thing_table};

/// A complete two-row table for the parser tests.
const MINI_TABLE: &str = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                          3001\tImp\tTROO\tA\t20\tMonsters\t-\tdoom\n\
                          63\tHanging Swaying Body\tGOR1\tA\t16\tGore/Hanging & Impaled \
                          Bodies\thanging\tdoom\n";

/// The header line of a valid special table.
const SPECIAL_MINI: &str = "special\tname\tcategory\ttarget\targs\tgame\n";

/// The expected rows of [`MINI_TABLE`].
fn mini_rows() -> Vec<ThingDef> {
    parse_thing_table(MINI_TABLE).expect("the mini table is valid")
}

#[test]
fn parses_rows_with_numbers_flags_and_frame() {
    let rows = mini_rows();
    assert_eq!(rows.len(), 2, "both rows parsed");
    let imp = rows.first().expect("the mini table has a first row");
    assert_eq!(imp.kind, 3001, "type number");
    assert_eq!(imp.name, "Imp", "name");
    assert_eq!(imp.sprite, "TROO", "sprite");
    assert_eq!(imp.frame, 'A', "frame character");
    assert_eq!(imp.radius, 20, "radius");
    assert_eq!(imp.category, "Monsters", "category");
    assert_eq!(imp.group, Category::Monsters, "the Monsters group");
    assert!(!imp.hanging, "the - flag means not hanging");
    assert_eq!(imp.game, "doom", "game column");
    assert_eq!(imp.source, "builtin", "table rows are builtin");
    let hanging = rows.get(1).expect("the mini table has a second row");
    assert!(hanging.hanging, "the hanging flag parses to true");
    assert_eq!(hanging.kind, 63, "second row type number");
    assert_eq!(hanging.group, Category::Other, "a gore path is no group");
}

#[test]
fn keeps_row_order_and_skips_blank_lines() {
    let text = MINI_TABLE.replace("63\t", "\n63\t");
    let rows = parse_thing_table(&text).expect("a blank line is skipped");
    assert_eq!(rows.len(), 2, "the blank line adds no row");
    assert_eq!(rows.first().map(|row| row.kind), Some(3001), "order kept");
}

#[test]
fn a_missing_header_is_rejected() {
    let error = parse_thing_table("").expect_err("an empty table is invalid");
    assert!(
        error.contains("empty"),
        "the message says the table is empty: {error}"
    );
}

#[test]
fn a_wrong_header_line_is_rejected() {
    let text = "type\tname\tsprite\tframe\tradius\tgame\n3001\tImp\tTROO\tA\t20\tdoom\n";
    let error = parse_thing_table(text).expect_err("a six-column header is invalid");
    assert!(
        error.contains("header"),
        "the message names the header: {error}"
    );
}

#[test]
fn a_row_with_the_wrong_field_count_names_the_line() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                3001\tImp\tTROO\tA\t20\tMonsters\t-\n";
    let error = parse_thing_table(text).expect_err("a seven-field row is invalid");
    assert!(
        error.contains("line 2"),
        "the message names the line: {error}"
    );
}

#[test]
fn a_non_numeric_type_names_the_line_and_column() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                x\tImp\tTROO\tA\t20\tMonsters\t-\tdoom\n";
    let error = parse_thing_table(text).expect_err("a non-numeric type is invalid");
    assert!(error.contains("line 2"), "line named: {error}");
    assert!(error.contains("\"type\""), "column named: {error}");
}

#[test]
fn a_non_numeric_radius_is_rejected() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                3001\tImp\tTROO\tA\twide\tMonsters\t-\tdoom\n";
    let error = parse_thing_table(text).expect_err("a non-numeric radius is invalid");
    assert!(error.contains("\"radius\""), "column named: {error}");
}

#[test]
fn an_empty_sprite_is_accepted() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                14\tTeleport Destination\t\tB\t20\tTeleports\t-\tdoom\n";
    let rows = parse_thing_table(text).expect("an empty sprite is valid");
    assert_eq!(
        rows.first().map(|row| row.sprite.as_str()),
        Some(""),
        "the sprite stays empty"
    );
}

#[test]
fn a_sprite_of_the_wrong_length_is_rejected() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                3001\tImp\tTROOO\tA\t20\tMonsters\t-\tdoom\n";
    let error = parse_thing_table(text).expect_err("a five-character sprite is invalid");
    assert!(
        error.contains("sprite name"),
        "the message names the field: {error}"
    );
}

#[test]
fn a_multi_character_frame_is_rejected() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                3001\tImp\tTROO\tAB\t20\tMonsters\t-\tdoom\n";
    let error = parse_thing_table(text).expect_err("a two-character frame is invalid");
    assert!(
        error.contains("frame"),
        "the message names the field: {error}"
    );
}

#[test]
fn an_unknown_flag_lists_the_accepted_texts() {
    let text = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                3001\tImp\tTROO\tA\t20\tMonsters\tdormant\tdoom\n";
    let error = parse_thing_table(text).expect_err("an unknown flag is invalid");
    assert!(error.contains("\"-\""), "the none flag is named: {error}");
    assert!(
        error.contains("\"hanging\""),
        "the hanging flag is named: {error}"
    );
}

#[test]
fn the_shipped_doom_table_parses() {
    let rows = parse_thing_table(crate::defs::registry::DOOM_THINGS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 123, "the shipped table's row count");
    let doom: usize = rows.iter().filter(|row| row.game == "doom").count();
    assert_eq!(doom, 97, "rows of the first game");
    assert_eq!(
        rows.len().saturating_sub(doom),
        26,
        "the rest are Doom II additions"
    );
}

#[test]
fn special_rows_parse_target_and_args() {
    let text = format!("{SPECIAL_MINI}1\tDR Door\tDoors/Local\tsector_back\t-\tdoom\n");
    let rows = parse_special_table(&text).expect("a valid special row");
    let door = rows.first().expect("one row");
    assert_eq!(door.number, 1, "special number");
    assert_eq!(door.name, "DR Door", "name");
    assert_eq!(door.category, "Doors/Local", "category");
    assert_eq!(door.group, Category::Doors, "a sub-path stands in Doors");
    assert_eq!(door.target, "sector_back", "target column");
    assert!(door.args.is_empty(), "- means no arguments");
    assert_eq!(door.game, "doom", "game column");
}

#[test]
fn special_args_name_positions_in_order() {
    let text = format!(
        "{SPECIAL_MINI}13\tDoor_LockedRaise\tDoors\tsector_or_back\targ1=Sector Tag;arg2=Speed;\
         arg3=Delay;arg4=Lock\thexen\n"
    );
    let rows = parse_special_table(&text).expect("numbered argument names are valid");
    let door = rows.first().expect("one row");
    assert_eq!(
        door.target, "sector_or_back",
        "free-form target passes through"
    );
    let args: Vec<&str> = door.args.iter().map(String::as_str).collect();
    assert_eq!(
        args,
        vec!["Sector Tag", "Speed", "Delay", "Lock"],
        "index 0 is arg1 and names keep their spaces"
    );
}

#[test]
fn a_misnumbered_argument_entry_names_the_line_and_entry() {
    let text = format!("{SPECIAL_MINI}13\tDoor\tDoors\tsector\targ1=Tag;arg3=Speed\thexen\n");
    let error = parse_special_table(&text).expect_err("arg2 is missing");
    assert!(error.contains("line 2"), "the line is named: {error}");
    assert!(
        error.contains("\"arg2=\""),
        "the wanted prefix is named: {error}"
    );
}

#[test]
fn an_argument_without_a_name_is_rejected() {
    let text = format!("{SPECIAL_MINI}13\tDoor\tDoors\tsector\targ1=\thexen\n");
    let error = parse_special_table(&text).expect_err("an empty name is invalid");
    assert!(error.contains("no name"), "the problem is named: {error}");
}

#[test]
fn special_targets_pass_through_free_form() {
    let text = format!(
        "{SPECIAL_MINI}120\tRadius_Quake\tScripting\tex_5thing\targ1=Intensity\thexen\n\
         100\tScroll_Texture_Left\tScrollers\tno\targ1=Speed\thexen\n\
         121\tLine_SetIdentification\tLines\tline\targ1=Line ID\thexen\n"
    );
    let rows = parse_special_table(&text).expect("any non-empty target is a hint");
    let targets: Vec<&str> = rows.iter().map(|row| row.target.as_str()).collect();
    assert_eq!(
        targets,
        vec!["ex_5thing", "no", "line"],
        "the texts arrive unchanged"
    );
}

#[test]
fn an_empty_special_target_is_rejected() {
    let text = format!("{SPECIAL_MINI}1\tDoor\tDoors\t\t-\tdoom\n");
    let error = parse_special_table(&text).expect_err("an empty target is invalid");
    assert!(error.contains("target"), "the column is named: {error}");
    assert!(
        error.contains("\"sector_or_back\""),
        "accepted hints are named: {error}"
    );
}

#[test]
fn a_blank_special_args_column_is_rejected() {
    let text = format!("{SPECIAL_MINI}1\tDoor\tDoors\tsector\t \tdoom\n");
    let error = parse_special_table(&text).expect_err("an empty args column is invalid");
    assert!(error.contains("args"), "the column is named: {error}");
}

#[test]
fn a_bad_special_number_names_the_column() {
    let text = format!("{SPECIAL_MINI}door\tDoor\tDoors\tsector\t-\tdoom\n");
    let error = parse_special_table(&text).expect_err("a non-numeric special is invalid");
    assert!(
        error.contains("\"special\"") && error.contains("special table line 2"),
        "table, line and column are named: {error}"
    );
}

#[test]
fn the_special_table_header_is_checked() {
    let error = parse_special_table("special\tname\ttarget\n")
        .expect_err("a three-column header is invalid");
    assert!(error.contains("header"), "the header is named: {error}");
}

#[test]
fn sector_rows_parse_number_and_name() {
    let text = "type\tname\n0\tNormal\n16\t20% Damage\n";
    let rows = parse_sector_table(text).expect("a valid sector table");
    assert_eq!(rows.len(), 2, "both rows parsed");
    assert_eq!(rows.first().map(|row| row.number), Some(0), "type 0");
    let damage = rows.get(1).expect("second row");
    assert_eq!(damage.number, 16, "type 16");
    assert_eq!(damage.name, "20% Damage", "name");
}

#[test]
fn a_bad_sector_type_names_the_line_and_column() {
    let text = "type\tname\nfast\tLight Flicker\n";
    let error = parse_sector_table(text).expect_err("a non-numeric type is invalid");
    assert!(
        error.contains("sector table line 2") && error.contains("\"type\""),
        "table, line and column are named: {error}"
    );
}

#[test]
fn a_sector_row_with_one_field_is_rejected() {
    let error = parse_sector_table("type\tname\n12\n").expect_err("a one-field row is invalid");
    assert!(error.contains("line 2"), "the line is named: {error}");
}

#[test]
fn the_shipped_doom_specials_table_parses() {
    let rows = parse_special_table(crate::defs::registry::DOOM_SPECIALS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 139, "the shipped table's row count");
    let doom: usize = rows.iter().filter(|row| row.game == "doom").count();
    assert_eq!(doom, 100, "rows of the first game");
    assert_eq!(
        rows.len().saturating_sub(doom),
        39,
        "the rest are Doom II additions"
    );
}

#[test]
fn the_shipped_doom_sectors_table_parses() {
    let rows = parse_sector_table(crate::defs::registry::DOOM_SECTORS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 16, "the shipped table's row count");
    assert_eq!(
        rows.first().map(|row| row.name.as_str()),
        Some("Normal"),
        "type 0"
    );
}

#[test]
fn the_shipped_heretic_things_table_parses() {
    let rows = parse_thing_table(crate::defs::registry::HERETIC_THINGS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 84, "the shipped table's row count");
    assert!(
        rows.iter().all(|row| row.game == "heretic"),
        "every row says heretic"
    );
    assert_eq!(
        rows.iter().filter(|row| row.hanging).count(),
        10,
        "the hanging rows"
    );
}

#[test]
fn the_shipped_heretic_specials_table_parses() {
    let rows = parse_special_table(crate::defs::registry::HERETIC_SPECIALS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 105, "the shipped table's row count");
    assert!(
        rows.iter().all(|row| row.game == "heretic"),
        "every row says heretic"
    );
    assert_eq!(
        rows.iter().filter(|row| row.target == "-").count(),
        6,
        "the four exits and two scrolling walls act on no sector"
    );
}

#[test]
fn the_shipped_heretic_sectors_table_parses() {
    let rows = parse_sector_table(crate::defs::registry::HERETIC_SECTORS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 47, "the shipped table's row count");
    assert_eq!(
        rows.first().map(|row| row.name.as_str()),
        Some("Normal"),
        "type 0"
    );
    assert_eq!(
        rows.last().map(|row| row.name.as_str()),
        Some("Wind West (25)"),
        "the last row"
    );
}

#[test]
fn the_shipped_hexen_things_table_parses() {
    let rows = parse_thing_table(crate::defs::registry::HEXEN_THINGS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 233, "the shipped table's row count");
    assert!(
        rows.iter().all(|row| row.game == "hexen"),
        "every row says hexen"
    );
    assert_eq!(
        rows.iter().filter(|row| row.hanging).count(),
        22,
        "the hanging rows"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.group == Category::Monsters)
            .count(),
        18,
        "the monster rows"
    );
}

#[test]
fn the_shipped_hexen_specials_table_parses() {
    let rows = parse_special_table(crate::defs::registry::HEXEN_SPECIALS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 86, "the shipped table's row count");
    assert!(
        rows.iter().all(|row| row.game == "hexen"),
        "every row says hexen"
    );
    let locked = rows
        .iter()
        .find(|row| row.number == 13)
        .expect("Door_LockedRaise exists");
    let args: Vec<&str> = locked.args.iter().map(String::as_str).collect();
    assert_eq!(
        args,
        vec!["Sector Tag", "Speed", "Delay", "Lock"],
        "special 13 lists four argument names, arg1 first"
    );
    let endgame = rows
        .iter()
        .find(|row| row.number == 75)
        .expect("Teleport_EndGame exists");
    assert!(endgame.args.is_empty(), "special 75 takes no arguments");
    assert_eq!(
        rows.iter().filter(|row| row.args.len() == 5).count(),
        11,
        "the five-argument rows"
    );
    assert_eq!(
        rows.iter().filter(|row| row.args.is_empty()).count(),
        2,
        "the two rows with no arguments"
    );
}

#[test]
fn the_shipped_hexen_sectors_table_parses() {
    let rows = parse_sector_table(crate::defs::registry::HEXEN_SECTORS_TSV)
        .expect("the embedded table must parse");
    assert_eq!(rows.len(), 46, "the shipped table's row count");
    assert_eq!(
        rows.first().map(|row| row.name.as_str()),
        Some("Normal"),
        "type 0"
    );
    assert_eq!(
        rows.last().map(|row| row.name.as_str()),
        Some("Scroll SouthWest (25)"),
        "the last row"
    );
}

#[test]
fn categories_classify_the_known_groups() {
    let cases = [
        ("Monsters", Category::Monsters),
        ("Player Starts", Category::PlayerStarts),
        ("Weapons", Category::Weapons),
        ("Ammunition", Category::Ammunition),
        ("Health And Armour", Category::HealthAndArmour),
        ("Powerups", Category::Powerups),
        ("Keys", Category::Keys),
        ("Doors", Category::Doors),
        ("Doors/Local", Category::Doors),
        ("Doors/Remote", Category::Doors),
    ];
    for (text, want) in cases {
        assert_eq!(Category::parse(text), want, "{text} classifies");
    }
}

#[test]
fn every_other_path_stands_in_other() {
    let cases = [
        "",
        "Gore/Blood & Stuff",
        "Decorations/Tech",
        "Lighting",
        "Teleports",
        "Puzzle Items",
        "Custom (DeHackEd)",
        "monsters",
        "Monster",
        "Lifts",
        "Scripting",
    ];
    for text in cases {
        assert_eq!(Category::parse(text), Category::Other, "{text} is no group");
    }
    assert_eq!(
        Category::parse("Health Flood"),
        Category::HealthAndArmour,
        "the tables' only Health path is Health And Armour, and the prefix stands"
    );
}
