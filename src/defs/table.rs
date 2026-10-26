//! Parsing of the tab-separated definition tables embedded in the binary.
//!
//! [`parse_thing_table`], [`parse_special_table`] and [`parse_sector_table`]
//! read the thing, linedef-special and sector-type files through the shared
//! [`crate::defs::tsv`] reader, turning them into [`ThingDef`],
//! [`SpecialDef`] and [`SectorTypeDef`] rows and reporting the first
//! malformed line as an error string. Each row's editor category is
//! classified into a [`Category`] once here, so the server's category
//! decisions read the enum instead of matching the path text. The registry
//! ([`crate::defs::registry`]) parses the embedded tables once and surfaces
//! any failure as [`crate::error::CyberError::Internal`]: broken embedded
//! data is a build bug, never a panic.

use crate::defs::tsv::{Row, Table};

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

/// The free-form target texts a special table uses, named in error hints.
const TARGET_HINTS: &str =
    "\"sector\", \"sector_or_back\", \"thing\", \"line\", \"ex_5thing\" or \"no\"";

/// The text used for "no value" in the `target` and `args` columns.
const COLUMN_NONE: &str = "-";

/// The [`ThingDef::source`] label of rows that come from the embedded tables.
const SOURCE_BUILTIN: &str = "builtin";

/// The prefix of one `argN=Name` entry in the args column.
const ARG_PREFIX: &str = "arg";

/// The character separating argument entries in the args column.
const ARG_SEP: char = ';';

/// The character separating an argument's position from its name.
const ARG_ASSIGN: char = '=';

/// The length of a Doom sprite name.
const SPRITE_LEN: usize = 4;

/// The known editor category prefixes and the [`Category`] each names,
/// as the tables write them.
const CATEGORY_PREFIXES: [(&str, Category); 8] = [
    ("Monsters", Category::Monsters),
    ("Player Starts", Category::PlayerStarts),
    ("Weapons", Category::Weapons),
    ("Ammunition", Category::Ammunition),
    ("Health", Category::HealthAndArmour),
    ("Powerups", Category::Powerups),
    ("Keys", Category::Keys),
    ("Doors", Category::Doors),
];

/// The semantic group an editor category path stands under.
///
/// The tables' category column is a display path (`"Doors/Local"`,
/// `"Gore/Blood & Stuff"`); every decision the server takes on a category
/// reads this enum instead of the text, and [`Category::parse`] — where the
/// hardcoded category words live — runs once per row in the table readers
/// and in the definition overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// `"Monsters"`: the things that fight.
    Monsters,
    /// `"Player Starts"`.
    PlayerStarts,
    /// `"Weapons"`.
    Weapons,
    /// `"Ammunition"`.
    Ammunition,
    /// `"Health And Armour"` and any other `"Health"` path.
    HealthAndArmour,
    /// `"Powerups"`.
    Powerups,
    /// `"Keys"`: cards, skulls and every other keyed item.
    Keys,
    /// `"Doors"`, sub-paths (`"Doors/Local"`, `"Doors/Remote"`) included.
    Doors,
    /// Every other path: decorations, gore, lighting, customs, ...
    Other,
}

impl Category {
    /// The group the editor category path `text` stands under: the first
    /// known prefix it starts with, else [`Category::Other`].
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Self {
        CATEGORY_PREFIXES
            .iter()
            .find(|entry| text.starts_with(entry.0))
            .map_or(Self::Other, |entry| entry.1)
    }
}

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
    /// The semantic group [`Category::parse`] classified `category` into,
    /// read by every category decision instead of the path text.
    pub group: Category,
    /// Whether the thing hangs from the ceiling.
    pub hanging: bool,
    /// The first game that has this thing (`"doom"`, `"doom2"`, ...).
    pub game: String,
    /// Where this row came from: `"builtin"` for the embedded tables,
    /// `"dehacked"`, `"decorate"` or `"zscript"` for session definitions
    /// overlaid by [`crate::defs::custom`].
    pub source: String,
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
    /// The semantic group [`Category::parse`] classified `category` into,
    /// read by every category decision instead of the path text.
    pub group: Category,
    /// What the special acts on: a free-form table hint such as `"sector"`
    /// (tagged sectors), `"sector_or_back"`, `"sector_back"`, `"thing"`,
    /// `"line"`, `"ex_5thing"`, `"no"` or `"-"` (nothing), passed through
    /// unchanged.
    pub target: String,
    /// The argument names the special reads, in order: index 0 is `arg1`.
    /// Empty for the Doom-family tables and for specials that take none.
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
    let table = Table::parse("thing table", &THING_HEADER, text)?;
    table.rows().iter().map(parse_thing_row).collect()
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
    let table = Table::parse("special table", &SPECIAL_HEADER, text)?;
    table.rows().iter().map(parse_special_row).collect()
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
    let table = Table::parse("sector table", &SECTOR_HEADER, text)?;
    table.rows().iter().map(parse_sector_row).collect()
}

/// Parse one [`ThingDef`] row of a thing table.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_thing_row(row: &Row<'_>) -> Result<ThingDef, String> {
    let [kind, name, sprite, frame, radius, category, flags, game] =
        row.columns::<THING_FIELDS>()?;
    Ok(ThingDef {
        kind: row.u32("type", kind)?,
        name: name.to_owned(),
        sprite: parse_sprite(row, sprite)?,
        frame: parse_frame(row, frame)?,
        radius: row.u32("radius", radius)?,
        category: category.to_owned(),
        group: Category::parse(category),
        hanging: parse_flags(row, flags)?,
        game: game.to_owned(),
        source: SOURCE_BUILTIN.to_owned(),
    })
}

/// Parse one [`SpecialDef`] row of a special table.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_special_row(row: &Row<'_>) -> Result<SpecialDef, String> {
    let [special, name, category, target, args, game] = row.columns::<SPECIAL_FIELDS>()?;
    Ok(SpecialDef {
        number: row.u32("special", special)?,
        name: name.to_owned(),
        category: category.to_owned(),
        group: Category::parse(category),
        target: parse_target(row, target)?,
        args: parse_args(row, args)?,
        game: game.to_owned(),
    })
}

/// Parse one [`SectorTypeDef`] row of a sector table.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_sector_row(row: &Row<'_>) -> Result<SectorTypeDef, String> {
    let [kind, name] = row.columns::<SECTOR_FIELDS>()?;
    Ok(SectorTypeDef {
        number: row.u32("type", kind)?,
        name: name.to_owned(),
    })
}

/// Parse the sprite-name column: empty, or exactly four characters.
///
/// # Errors
///
/// A `String` naming the line when the name has any other length.
fn parse_sprite(row: &Row<'_>, text: &str) -> Result<String, String> {
    if text.is_empty() || text.chars().count() == SPRITE_LEN {
        return Ok(text.to_owned());
    }
    Err(row.fail(&format!(
        "sprite name {text:?} is neither empty nor four characters"
    )))
}

/// Parse the frame column: exactly one character.
///
/// # Errors
///
/// A `String` naming the line when the text is not one character.
fn parse_frame(row: &Row<'_>, text: &str) -> Result<char, String> {
    let mut chars = text.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return Err(row.fail(&format!("frame {text:?} is not exactly one character")));
    };
    Ok(first)
}

/// Parse the flags column into `hanging`.
///
/// # Errors
///
/// A `String` naming the line and the accepted flag texts.
fn parse_flags(row: &Row<'_>, text: &str) -> Result<bool, String> {
    if text == FLAG_NONE {
        return Ok(false);
    }
    if text == FLAG_HANGING {
        return Ok(true);
    }
    Err(row.fail(&format!(
        "unknown flags {text:?}: use {FLAG_NONE:?} for none or {FLAG_HANGING:?} for a hanging \
         thing"
    )))
}

/// Parse the target column: a free-form hint naming what the special acts
/// on (`sector`, `sector_or_back`, `thing`, `line`, `ex_5thing`, `no`, `-`,
/// ...); the text is passed through unchanged and only empty values are
/// refused.
///
/// # Errors
///
/// A `String` naming the line when the column is empty.
fn parse_target(row: &Row<'_>, text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err(row.fail(&format!(
            "empty target column: use {COLUMN_NONE:?} for nothing acted on or one of the hints \
             like {TARGET_HINTS}"
        )));
    }
    Ok(text.to_owned())
}

/// Parse the args column: `-` when the special takes no arguments, else
/// `arg1=Name;arg2=Name` entries listing each argument's name in order, so
/// the result's index 0 is `arg1`.
///
/// # Errors
///
/// A `String` naming the line and the first entry that does not carry its
/// position number, or an empty name.
fn parse_args(row: &Row<'_>, text: &str) -> Result<Vec<String>, String> {
    if text == COLUMN_NONE {
        return Ok(Vec::new());
    }
    text.split(ARG_SEP)
        .enumerate()
        .map(|(offset, part)| parse_arg_entry(row, part, offset))
        .collect()
}

/// Parse one `argN=Name` entry of the args column: `offset` is its
/// zero-based position, so the entry must start with `arg{offset + 1}=`.
///
/// # Errors
///
/// A `String` naming the line and the entry when the position prefix is
/// missing or wrong, or the name is empty.
fn parse_arg_entry(row: &Row<'_>, part: &str, offset: usize) -> Result<String, String> {
    let wanted = u32::try_from(offset)
        .ok()
        .and_then(|index| index.checked_add(1))
        .ok_or_else(|| row.fail(&format!("too many arguments near {part:?}")))?;
    let prefix = format!("{ARG_PREFIX}{wanted}{ARG_ASSIGN}");
    let Some(name) = part.strip_prefix(prefix.as_str()) else {
        return Err(row.fail(&format!(
            "args entry {part:?} does not start with {prefix:?}"
        )));
    };
    if name.is_empty() {
        return Err(row.fail(&format!(
            "args entry {part:?} gives argument {wanted} no name"
        )));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests;
