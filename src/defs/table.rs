//! Parsing of the tab-separated definition tables embedded in the binary.
//!
//! Every `data/defs/*.tsv` file has one shape: a header line naming the
//! columns, then one line per entry with exactly that many tab-separated
//! fields. [`parse_thing_table`], [`parse_special_table`] and
//! [`parse_sector_table`] turn the thing, linedef-special and sector-type
//! tables into [`ThingDef`], [`SpecialDef`] and [`SectorTypeDef`] rows and
//! report the first malformed line as an error string. The registry
//! ([`crate::defs::registry`]) parses the embedded tables once and surfaces
//! any failure as [`crate::error::CyberError::Internal`]: broken embedded
//! data is a build bug, never a panic.

/// The header every thing table must carry, column by column.
const THING_HEADER: [&str; 8] = [
    "type", "name", "sprite", "frame", "radius", "category", "flags", "game",
];

/// The number of tab-separated fields one thing row must carry.
const THING_FIELDS: usize = 8;

/// The header every linedef-special table must carry, column by column.
const SPECIAL_HEADER: [&str; 6] = ["special", "name", "category", "target", "args", "game"];

/// The number of tab-separated fields one special row must carry.
const SPECIAL_FIELDS: usize = 6;

/// The header every sector-type table must carry, column by column.
const SECTOR_HEADER: [&str; 2] = ["type", "name"];

/// The number of tab-separated fields one sector row must carry.
const SECTOR_FIELDS: usize = 2;

/// The flag text marking a hanging thing.
const FLAG_HANGING: &str = "hanging";

/// The flag text for "no flags".
const FLAG_NONE: &str = "-";

/// The target text for a linedef special that acts on tagged sectors.
const TARGET_SECTOR: &str = "sector";

/// The target text for a special that acts on the sector behind the line.
const TARGET_SECTOR_BACK: &str = "sector_back";

/// The text used for "no value" in the `target` and `args` columns.
const COLUMN_NONE: &str = "-";

/// The length of a Doom sprite name.
const SPRITE_LEN: usize = 4;

/// One thing type from a thing table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThingDef {
    /// The thing type number as editors write it into `THINGS`.
    pub kind: u32,
    /// The editor display name.
    pub name: String,
    /// The four-character sprite name, or empty when the thing has none.
    pub sprite: String,
    /// The sprite frame the editor draws by default.
    pub frame: char,
    /// The blocking radius in map units.
    pub radius: u32,
    /// The editor category path, e.g. `"Gore/Blood & Stuff"`.
    pub category: String,
    /// Whether the thing hangs from the ceiling.
    pub hanging: bool,
    /// The first game that has this thing (`"doom"`, `"doom2"`, ...).
    pub game: String,
}

/// One linedef special from a special table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecialDef {
    /// The special number as editors write it into `LINEDEFS`.
    pub number: u32,
    /// The editor display name.
    pub name: String,
    /// The editor category path, e.g. `"Doors/Local"`.
    pub category: String,
    /// What the special acts on: `"sector"` (tagged sectors),
    /// `"sector_back"` (the sector behind the line) or `"-"` (nothing).
    pub target: String,
    /// The argument names the special reads, empty for vanilla Doom.
    pub args: Vec<String>,
    /// The first game that has this special (`"doom"`, `"doom2"`, ...).
    pub game: String,
}

/// One sector type from a sector table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectorTypeDef {
    /// The sector type value as editors write it into `SECTORS`.
    pub number: u32,
    /// The editor display name.
    pub name: String,
}

/// Parse a whole thing table: `text` is the file contents, the first line
/// the header, every later non-blank line one [`ThingDef`] row.
///
/// The result keeps the file's row order.
///
/// # Errors
///
/// A `String` naming the header problem or the number and content of the
/// first malformed row.
#[inline]
pub fn parse_thing_table(text: &str) -> Result<Vec<ThingDef>, String> {
    let lines = rows_of("thing table", &THING_HEADER, text)?;
    let mut rows: Vec<ThingDef> = Vec::new();
    for (number, line) in lines {
        rows.push(parse_thing_row(line, number)?);
    }
    Ok(rows)
}

/// Parse a whole linedef-special table, one [`SpecialDef`] per non-blank
/// row after the header, in file order.
///
/// # Errors
///
/// A `String` naming the header problem or the number and content of the
/// first malformed row.
#[inline]
pub fn parse_special_table(text: &str) -> Result<Vec<SpecialDef>, String> {
    let lines = rows_of("special table", &SPECIAL_HEADER, text)?;
    let mut rows: Vec<SpecialDef> = Vec::new();
    for (number, line) in lines {
        rows.push(parse_special_row(line, number)?);
    }
    Ok(rows)
}

/// Parse a whole sector-type table, one [`SectorTypeDef`] per non-blank
/// row after the header, in file order.
///
/// # Errors
///
/// A `String` naming the header problem or the number and content of the
/// first malformed row.
#[inline]
pub fn parse_sector_table(text: &str) -> Result<Vec<SectorTypeDef>, String> {
    let lines = rows_of("sector table", &SECTOR_HEADER, text)?;
    let mut rows: Vec<SectorTypeDef> = Vec::new();
    for (number, line) in lines {
        rows.push(parse_sector_row(line, number)?);
    }
    Ok(rows)
}

/// The row iterator of one table: `label` names the table in errors,
/// `header` the expected columns, `text` the file contents. The iterator
/// yields each non-blank row's 1-based file line number and text; the
/// header itself is checked here.
///
/// # Errors
///
/// A `String` naming the missing or wrong header line.
fn rows_of<'text>(
    label: &str,
    header: &[&str],
    text: &'text str,
) -> Result<impl Iterator<Item = (usize, &'text str)>, String> {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return Err(format!("{label} is empty: no header line"));
    };
    check_header(first, header, label)?;
    // The header is line 1, so the `enumerate` index plus two is the file's
    // one-based line number of the row being parsed.
    Ok(lines
        .enumerate()
        .filter(|&(_, line)| !line.is_empty())
        .map(|(offset, line)| (offset.saturating_add(2), line)))
}

/// Check the header names exactly `expected`, tab-separated.
///
/// # Errors
///
/// A `String` showing both the found and the expected header.
fn check_header(line: &str, expected: &[&str], label: &str) -> Result<(), String> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.as_slice() != expected {
        let wanted = expected.join("\t");
        return Err(format!("bad {label} header {line:?}: expected {wanted:?}"));
    }
    Ok(())
}

/// Parse one tab-separated [`ThingDef`] row, `number` its 1-based file line
/// for error text.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_thing_row(line: &str, number: usize) -> Result<ThingDef, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let Ok([kind, name, sprite, frame, radius, category, flags, game]) =
        <[&str; THING_FIELDS]>::try_from(fields)
    else {
        return Err(format!(
            "thing table line {number}: expected {THING_FIELDS} tab-separated fields, found \
             {line:?}"
        ));
    };
    Ok(ThingDef {
        kind: parse_number("thing table", "type", kind, number)?,
        name: name.to_owned(),
        sprite: parse_sprite(sprite, number)?,
        frame: parse_frame(frame, number)?,
        radius: parse_number("thing table", "radius", radius, number)?,
        category: category.to_owned(),
        hanging: parse_flags(flags, number)?,
        game: game.to_owned(),
    })
}

/// Parse one tab-separated [`SpecialDef`] row, `number` its 1-based file
/// line for error text.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_special_row(line: &str, number: usize) -> Result<SpecialDef, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let Ok([special, name, category, target, args, game]) =
        <[&str; SPECIAL_FIELDS]>::try_from(fields)
    else {
        return Err(format!(
            "special table line {number}: expected {SPECIAL_FIELDS} tab-separated fields, found \
             {line:?}"
        ));
    };
    Ok(SpecialDef {
        number: parse_number("special table", "special", special, number)?,
        name: name.to_owned(),
        category: category.to_owned(),
        target: parse_target(target, number)?,
        args: parse_args(args, number)?,
        game: game.to_owned(),
    })
}

/// Parse one tab-separated [`SectorTypeDef`] row, `number` its 1-based file
/// line for error text.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_sector_row(line: &str, number: usize) -> Result<SectorTypeDef, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let Ok([kind, name]) = <[&str; SECTOR_FIELDS]>::try_from(fields) else {
        return Err(format!(
            "sector table line {number}: expected {SECTOR_FIELDS} tab-separated fields, found \
             {line:?}"
        ));
    };
    Ok(SectorTypeDef {
        number: parse_number("sector table", "type", kind, number)?,
        name: name.to_owned(),
    })
}

/// Parse a decimal column into its number; `table` names the table and
/// `column` the field in error text.
///
/// # Errors
///
/// A `String` naming the line and column.
fn parse_number(table: &str, column: &str, text: &str, number: usize) -> Result<u32, String> {
    text.parse::<u32>()
        .map_err(|error| format!("{table} line {number}: column {column:?} is {text:?}: {error}"))
}

/// Parse the sprite-name column: empty, or exactly four characters.
///
/// # Errors
///
/// A `String` naming the line when the name has any other length.
fn parse_sprite(text: &str, number: usize) -> Result<String, String> {
    if text.is_empty() || text.chars().count() == SPRITE_LEN {
        return Ok(text.to_owned());
    }
    Err(format!(
        "thing table line {number}: sprite name {text:?} is neither empty nor four characters"
    ))
}

/// Parse the frame column: exactly one character.
///
/// # Errors
///
/// A `String` naming the line when the text is not one character.
fn parse_frame(text: &str, number: usize) -> Result<char, String> {
    let mut chars = text.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return Err(format!(
            "thing table line {number}: frame {text:?} is not exactly one character"
        ));
    };
    Ok(first)
}

/// Parse the flags column into `hanging`.
///
/// # Errors
///
/// A `String` naming the line and the accepted flag texts.
fn parse_flags(text: &str, number: usize) -> Result<bool, String> {
    if text == FLAG_NONE {
        return Ok(false);
    }
    if text == FLAG_HANGING {
        return Ok(true);
    }
    Err(format!(
        "thing table line {number}: unknown flags {text:?}: use {FLAG_NONE:?} for none or \
         {FLAG_HANGING:?} for a hanging thing"
    ))
}

/// Parse the target column: the accepted vocabulary of linedef actions.
///
/// # Errors
///
/// A `String` naming the line and the accepted texts.
fn parse_target(text: &str, number: usize) -> Result<String, String> {
    if text == COLUMN_NONE || text == TARGET_SECTOR || text == TARGET_SECTOR_BACK {
        return Ok(text.to_owned());
    }
    Err(format!(
        "special table line {number}: unknown target {text:?}: use {COLUMN_NONE:?} for none, \
         {TARGET_SECTOR:?} for tagged sectors or {TARGET_SECTOR_BACK:?} for the sector behind \
         the line"
    ))
}

/// Parse the args column: `-` when the special takes no arguments, else
/// the whitespace-separated argument names.
///
/// # Errors
///
/// A `String` naming the line when the text is blank without being `-`.
fn parse_args(text: &str, number: usize) -> Result<Vec<String>, String> {
    if text == COLUMN_NONE {
        return Ok(Vec::new());
    }
    let names: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    if names.is_empty() {
        return Err(format!(
            "special table line {number}: args column {text:?} is neither {COLUMN_NONE:?} nor \
             space-separated argument names"
        ));
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::{ThingDef, parse_sector_table, parse_special_table, parse_thing_table};

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
        assert!(!imp.hanging, "the - flag means not hanging");
        assert_eq!(imp.game, "doom", "game column");
        let hanging = rows.get(1).expect("the mini table has a second row");
        assert!(hanging.hanging, "the hanging flag parses to true");
        assert_eq!(hanging.kind, 63, "second row type number");
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
        assert_eq!(door.target, "sector_back", "target column");
        assert!(door.args.is_empty(), "- means no arguments");
        assert_eq!(door.game, "doom", "game column");
    }

    #[test]
    fn special_args_split_on_whitespace() {
        let text =
            format!("{SPECIAL_MINI}125\tLine teleport\tTeleports\tsector\tside tag\thexen\n");
        let rows = parse_special_table(&text).expect("named arguments are valid");
        let args: Vec<&str> = rows
            .first()
            .expect("one row")
            .args
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(args, vec!["side", "tag"], "the argument names split out");
    }

    #[test]
    fn an_unknown_special_target_is_rejected() {
        let text = format!("{SPECIAL_MINI}1\tDoor\tDoors\tthing\t-\tdoom\n");
        let error = parse_special_table(&text).expect_err("thing is not a target");
        assert!(
            error.contains("\"sector_back\""),
            "accepted texts named: {error}"
        );
        assert!(error.contains("line 2"), "the line is named: {error}");
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
}
