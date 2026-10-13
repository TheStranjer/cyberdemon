//! Parsing of the tab-separated definition tables embedded in the binary.
//!
//! Every `data/defs/*.tsv` file has one shape: a header line naming the
//! columns, then one line per entry with exactly that many tab-separated
//! fields. [`parse_thing_table`] turns a thing table into [`ThingDef`] rows
//! and reports the first malformed line as an error string. The registry
//! ([`crate::defs::registry`]) parses the embedded tables once and surfaces
//! any failure as [`crate::error::CyberError::Internal`]: broken embedded
//! data is a build bug, never a panic.

/// The header every thing table must carry, column by column.
const THING_HEADER: [&str; 8] = [
    "type", "name", "sprite", "frame", "radius", "category", "flags", "game",
];

/// The number of tab-separated fields one thing row must carry.
const THING_FIELDS: usize = 8;

/// The flag text marking a hanging thing.
const FLAG_HANGING: &str = "hanging";

/// The flag text for "no flags".
const FLAG_NONE: &str = "-";

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
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Err("thing table is empty: no header line".to_owned());
    };
    check_header(header)?;
    let mut rows: Vec<ThingDef> = Vec::new();
    // The header is line 1, so the `enumerate` index plus two is the file's
    // one-based line number of the row being parsed.
    for (offset, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let number = offset.saturating_add(2);
        rows.push(parse_thing_row(line, number)?);
    }
    Ok(rows)
}

/// Check the header names exactly [`THING_HEADER`], tab-separated.
///
/// # Errors
///
/// A `String` showing both the found and the expected header.
fn check_header(line: &str) -> Result<(), String> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.as_slice() != THING_HEADER {
        let expected = THING_HEADER.join("\t");
        return Err(format!(
            "bad thing table header {line:?}: expected {expected:?}"
        ));
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
        kind: parse_number("type", kind, number)?,
        name: name.to_owned(),
        sprite: parse_sprite(sprite, number)?,
        frame: parse_frame(frame, number)?,
        radius: parse_number("radius", radius, number)?,
        category: category.to_owned(),
        hanging: parse_flags(flags, number)?,
        game: game.to_owned(),
    })
}

/// Parse a decimal column into its number.
///
/// # Errors
///
/// A `String` naming the line and column.
fn parse_number(column: &str, text: &str, number: usize) -> Result<u32, String> {
    text.parse::<u32>().map_err(|error| {
        format!("thing table line {number}: column {column:?} is {text:?}: {error}")
    })
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

#[cfg(test)]
mod tests {
    use super::{ThingDef, parse_thing_table};

    /// A complete two-row table for the parser tests.
    const MINI_TABLE: &str = "type\tname\tsprite\tframe\tradius\tcategory\tflags\tgame\n\
                              3001\tImp\tTROO\tA\t20\tMonsters\t-\tdoom\n\
                              63\tHanging Swaying Body\tGOR1\tA\t16\tGore/Hanging & Impaled \
                              Bodies\thanging\tdoom\n";

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
}
