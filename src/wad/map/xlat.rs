//! The Doom ↔ ZDoom linedef-special translation tables. No MCP concerns.
//!
//! `ZDoom` keeps the vanilla `Doom` linedef specials working by translating
//! each one into a generalized action number plus a fixed argument template:
//! the table pairs every legacy special with its `zspecial`, the activation
//! conditions the trigger keeps, and the arguments to write, with the
//! line's tag dropped into the single slot the table marks `tag`.
//! [`parse_xlat_table`] reads the tab-separated rows of
//! `data/xlat/doom_to_zdoom.tsv`; [`xlat_rows`], [`rows_for_doom`] and
//! [`rows_for_zspecial`] hand out the embedded rows for the cross-family
//! conversion built on top of this module. The embedded table is parsed
//! once on first use behind a `LazyLock`; a parse failure means the shipped
//! data is broken, so every accessor reports it as
//! [`crate::error::CyberError::Internal`], never a
//! panic, exactly like [`crate::defs::registry`].

use core::str::FromStr as _;
use std::sync::LazyLock;

use crate::error::CyberError;

/// The number of tab-separated fields one translation row must carry.
const XLAT_FIELDS: usize = 5;

/// The header the translation table must carry, column by column.
const XLAT_HEADER: [&str; XLAT_FIELDS] = ["doom", "activation", "zspecial", "name", "args"];

/// The argument slots a `ZDoom` action takes; the template is padded to it.
const MAX_ARGS: usize = 5;

/// The character separating argument entries in the args column.
const ARG_SEP: char = ',';

/// The args column entry naming the slot the line's tag fills.
const TAG_WORD: &str = "tag";

/// The activation column text meaning "no activation" (the scrollers).
const NO_ACTIVATION: &str = "0";

/// The parsed result of one args column: the five-slot argument template,
/// the number of entries the column actually wrote and the index of the
/// `tag` slot, when the column carries one.
type ArgsTemplate = ([i32; MAX_ARGS], usize, Option<usize>);

/// The Doom-to-ZDoom translation table, compiled into the binary from
/// `data/xlat/doom_to_zdoom.tsv`.
const DOOM_TO_ZDOOM_TSV: &str = include_str!("../../../data/xlat/doom_to_zdoom.tsv");

/// The activation conditions of a translated special, as the `ZDoom`
/// trigger needs them: what may fire the line and whether it repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XlatFlags(
    /// The raw activation bits, [`XlatFlags::WALK`] through
    /// [`XlatFlags::MONST`] combined.
    pub u32,
);

impl XlatFlags {
    /// A player walking over the line activates it.
    pub const WALK: Self = Self(1_u32);

    /// Pressing use on the line activates it.
    pub const USE: Self = Self(2_u32);

    /// Shooting the line activates it.
    pub const SHOOT: Self = Self(4_u32);

    /// Monsters walk-activate the line like players.
    pub const MONWALK: Self = Self(8_u32);

    /// The action repeats instead of triggering once.
    pub const REP: Self = Self(0x10_u32);

    /// Monsters may trigger the action like players.
    pub const MONST: Self = Self(0x20_u32);

    /// Parse an activation column: `|`-joined flag words (`WALK|MONST|REP`)
    /// or `0` for no activation at all.
    ///
    /// # Errors
    ///
    /// A `String` naming the first word that is not a known flag.
    #[inline]
    pub fn parse(words: &str) -> Result<Self, String> {
        if words == NO_ACTIVATION {
            return Ok(Self(0_u32));
        }
        let mut bits = 0_u32;
        for word in words.split('|') {
            bits |= Self::bit(word)?;
        }
        Ok(Self(bits))
    }

    /// Whether every bit set in `other` is set here.
    #[must_use]
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no activation bit is set.
    #[must_use]
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0_u32
    }

    /// The bit of one activation word.
    ///
    /// # Errors
    ///
    /// A `String` naming the unknown word.
    fn bit(word: &str) -> Result<u32, String> {
        match word {
            "WALK" => Ok(Self::WALK.0),
            "USE" => Ok(Self::USE.0),
            "SHOOT" => Ok(Self::SHOOT.0),
            "MONWALK" => Ok(Self::MONWALK.0),
            "REP" => Ok(Self::REP.0),
            "MONST" => Ok(Self::MONST.0),
            unknown => Err(format!(
                "unknown activation word {unknown:?}: use WALK, USE, SHOOT, MONWALK, REP or MONST"
            )),
        }
    }
}

/// One Doom-to-ZDoom translation row: what a vanilla linedef special
/// becomes and how it is triggered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XlatRow {
    /// The vanilla `Doom` linedef special this row translates.
    pub doom: i32,
    /// The activation conditions carried over to the translated action.
    pub activation: XlatFlags,
    /// The `ZDoom` action number to run.
    pub zspecial: i32,
    /// The action's `ZDoom` name, e.g. `Door_Raise`, for reports.
    pub name: String,
    /// The argument template: the row's entries zero-padded to five slots,
    /// the [`XlatRow::tag_slot`] holding `0` until the line's tag replaces
    /// it.
    pub args: [i32; MAX_ARGS],
    /// How many entries the args column actually wrote.
    pub arg_count: usize,
    /// The index in [`XlatRow::args`] where the line's tag goes, when the
    /// column carried a `tag` entry.
    pub tag_slot: Option<usize>,
}

/// Parse a whole translation table: `text` is the file contents, the first
/// line the header, every later non-blank line one [`XlatRow`].
///
/// The result keeps the file's row order.
///
/// # Errors
///
/// A `String` naming the header problem or the number and content of the
/// first malformed row.
#[inline]
pub fn parse_xlat_table(text: &str) -> Result<Vec<XlatRow>, String> {
    let lines = rows_of(&XLAT_HEADER, text)?;
    let mut rows: Vec<XlatRow> = Vec::new();
    for (number, line) in lines {
        rows.push(parse_xlat_row(line, number)?);
    }
    Ok(rows)
}

/// The parsed [`DOOM_TO_ZDOOM_TSV`], produced on first use. A parse failure
/// means the embedded data is broken: it is kept as an error string and
/// reported as [`CyberError::Internal`] by every accessor, never a panic.
static XLAT_DOOM_TO_ZDOOM: LazyLock<Result<Vec<XlatRow>, String>> =
    LazyLock::new(|| parse_xlat_table(DOOM_TO_ZDOOM_TSV));

/// Every embedded Doom-to-ZDoom translation row, in file order.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded table is malformed, which is
/// a build bug rather than a client problem.
#[inline]
pub fn xlat_rows() -> Result<&'static [XlatRow], CyberError> {
    XLAT_DOOM_TO_ZDOOM.as_deref().map_err(|reason| {
        CyberError::Internal(format!(
            "embedded Doom-to-ZDoom xlat table is malformed (this is a server bug): {reason}"
        ))
    })
}

/// The embedded rows translating the vanilla special `special`, in file
/// order. A special with no row has no translation.
///
/// # Errors
///
/// [`xlat_rows`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn rows_for_doom(special: i32) -> Result<Vec<&'static XlatRow>, CyberError> {
    Ok(xlat_rows()?
        .iter()
        .filter(|row| row.doom == special)
        .collect())
}

/// The embedded rows whose translated action is the `ZDoom` special
/// `zspecial`, in file order.
///
/// # Errors
///
/// [`xlat_rows`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn rows_for_zspecial(zspecial: i32) -> Result<Vec<&'static XlatRow>, CyberError> {
    Ok(xlat_rows()?
        .iter()
        .filter(|row| row.zspecial == zspecial)
        .collect())
}

/// The row iterator of the table: `header` the expected columns, `text`
/// the file contents. The iterator yields each non-blank row's 1-based
/// file line number and text; the header itself is checked here.
///
/// # Errors
///
/// A `String` naming the missing or wrong header line.
fn rows_of<'text>(
    header: &[&str],
    text: &'text str,
) -> Result<impl Iterator<Item = (usize, &'text str)>, String> {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return Err("xlat table is empty: no header line".to_owned());
    };
    check_header(first, header)?;
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
fn check_header(line: &str, expected: &[&str]) -> Result<(), String> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.as_slice() != expected {
        let wanted = expected.join("\t");
        return Err(format!(
            "bad xlat table header {line:?}: expected {wanted:?}"
        ));
    }
    Ok(())
}

/// Parse one tab-separated [`XlatRow`], `number` its 1-based file line for
/// error text.
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_xlat_row(line: &str, number: usize) -> Result<XlatRow, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let Ok([doom, activation, zspecial, name, args]) = <[&str; XLAT_FIELDS]>::try_from(fields)
    else {
        return Err(format!(
            "xlat table line {number}: expected {XLAT_FIELDS} tab-separated fields, found \
             {line:?}"
        ));
    };
    let flags = XlatFlags::parse(activation)
        .map_err(|reason| format!("xlat table line {number}: {reason}"))?;
    let (values, arg_count, tag_slot) = parse_args(args, number)?;
    Ok(XlatRow {
        doom: parse_number("doom", doom, number)?,
        activation: flags,
        zspecial: parse_number("zspecial", zspecial, number)?,
        name: name.to_owned(),
        args: values,
        arg_count,
        tag_slot,
    })
}

/// Parse a decimal column into its number; `column` names the field in
/// error text.
///
/// # Errors
///
/// A `String` naming the line and column.
fn parse_number(column: &str, text: &str, number: usize) -> Result<i32, String> {
    i32::from_str(text).map_err(|error| {
        format!("xlat table line {number}: column {column:?} is {text:?}: {error}")
    })
}

/// Parse the args column: 1 to 5 comma-separated entries, each `tag` or a
/// non-negative decimal, at most one of them `tag`. The result is the
/// template zero-padded to five slots, the number of written entries and
/// the index of the `tag` slot, when the column carries one.
///
/// # Errors
///
/// A `String` naming the line and the offending column.
fn parse_args(text: &str, number: usize) -> Result<ArgsTemplate, String> {
    let mut values: Vec<i32> = Vec::new();
    let mut tag_slot: Option<usize> = None;
    for entry in text.split(ARG_SEP) {
        if values.len() >= MAX_ARGS {
            return Err(format!(
                "xlat table line {number}: args column {text:?} carries more than {MAX_ARGS} \
                 entries"
            ));
        }
        if entry == TAG_WORD {
            claim_tag(&mut tag_slot, values.len(), number, text)?;
            values.push(0_i32);
        } else {
            values.push(parse_arg_value(entry, number)?);
        }
    }
    let arg_count = values.len();
    values.resize(MAX_ARGS, 0_i32);
    let Ok(args) = <[i32; MAX_ARGS]>::try_from(values) else {
        return Err(format!(
            "xlat table line {number}: args column {text:?} cannot fill {MAX_ARGS} slots"
        ));
    };
    Ok((args, arg_count, tag_slot))
}

/// Record the args position `index` as the `tag` slot in `slot`, refusing
/// a second `tag` entry.
///
/// # Errors
///
/// A `String` naming the line and column when a `tag` slot was claimed
/// before.
fn claim_tag(
    slot: &mut Option<usize>,
    index: usize,
    number: usize,
    text: &str,
) -> Result<(), String> {
    if slot.replace(index).is_some() {
        return Err(format!(
            "xlat table line {number}: args column {text:?} carries more than one {TAG_WORD:?} \
             entry"
        ));
    }
    Ok(())
}

/// Parse one args entry that is not `tag`: a non-negative decimal that
/// fits an `i32`.
///
/// # Errors
///
/// A `String` naming the line and the entry.
fn parse_arg_value(text: &str, number: usize) -> Result<i32, String> {
    let whole = text.parse::<u32>().map_err(|error| {
        format!(
            "xlat table line {number}: args entry {text:?} is neither {TAG_WORD:?} nor a \
             non-negative decimal: {error}"
        )
    })?;
    i32::try_from(whole).map_err(|error| {
        format!("xlat table line {number}: args entry {text:?} is too large: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DOOM_TO_ZDOOM_TSV, XlatFlags, XlatRow, parse_xlat_table, rows_for_doom, rows_for_zspecial,
        xlat_rows,
    };

    /// The header line of a valid translation table.
    const HEADER: &str = "doom\tactivation\tzspecial\tname\targs\n";

    /// Assert the embedded table has exactly one row for `expected.doom`
    /// and it equals `expected` field by field.
    fn check_row(expected: &XlatRow) {
        let rows = rows_for_doom(expected.doom).expect("the embedded table is valid");
        assert_eq!(
            rows.len(),
            1,
            "exactly one row translates special {}",
            expected.doom
        );
        let found = rows.first().expect("the row exists");
        assert_eq!(
            *found, expected,
            "full field values of row {}",
            expected.doom
        );
    }

    #[test]
    fn the_embedded_table_parses_with_141_rows() {
        let rows = xlat_rows().expect("the embedded table must parse");
        assert_eq!(rows.len(), 141, "the shipped table's row count");
        assert_eq!(
            parse_xlat_table(DOOM_TO_ZDOOM_TSV).expect("parsing the text again works"),
            rows.to_vec(),
            "the static holds the same rows as a fresh parse"
        );
    }

    #[test]
    fn row_one_is_the_dr_door_translation() {
        check_row(&XlatRow {
            doom: 1,
            activation: XlatFlags(2_u32 | 0x10_u32 | 0x20_u32),
            zspecial: 12,
            name: "Door_Raise".to_owned(),
            args: [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
            arg_count: 4,
            tag_slot: Some(3),
        });
    }

    #[test]
    fn row_12_is_a_lone_tag_argument() {
        check_row(&XlatRow {
            doom: 12,
            activation: XlatFlags::WALK,
            zspecial: 234,
            name: "Light_MaxNeighbor".to_owned(),
            args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            arg_count: 1,
            tag_slot: Some(0),
        });
    }

    #[test]
    fn row_26_carries_five_arguments_with_the_tag_last() {
        check_row(&XlatRow {
            doom: 26,
            activation: XlatFlags(2_u32 | 0x10_u32),
            zspecial: 13,
            name: "Door_LockedRaise".to_owned(),
            args: [0_i32, 16_i32, 150_i32, 130_i32, 0_i32],
            arg_count: 5,
            tag_slot: Some(4),
        });
    }

    #[test]
    fn row_48_has_no_activation() {
        check_row(&XlatRow {
            doom: 48,
            activation: XlatFlags(0_u32),
            zspecial: 100,
            name: "Scroll_Texture_Left".to_owned(),
            args: [64_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            arg_count: 1,
            tag_slot: None,
        });
        assert!(
            rows_for_doom(48)
                .expect("the embedded table is valid")
                .first()
                .expect("the row exists")
                .activation
                .is_empty(),
            "0 means no activation"
        );
    }

    #[test]
    fn row_97_puts_the_tag_in_the_second_slot() {
        check_row(&XlatRow {
            doom: 97,
            activation: XlatFlags(1_u32 | 0x10_u32 | 0x20_u32),
            zspecial: 70,
            name: "Teleport".to_owned(),
            args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            arg_count: 2,
            tag_slot: Some(1),
        });
    }

    #[test]
    fn row_100_translates_to_the_crushing_stairs() {
        check_row(&XlatRow {
            doom: 100,
            activation: XlatFlags::WALK,
            zspecial: 273,
            name: "Stairs_BuildUpDoomCrush".to_owned(),
            args: [0_i32, 32_i32, 16_i32, 0_i32, 0_i32],
            arg_count: 5,
            tag_slot: Some(0),
        });
    }

    #[test]
    fn row_125_is_monster_walk_only() {
        check_row(&XlatRow {
            doom: 125,
            activation: XlatFlags::MONWALK,
            zspecial: 70,
            name: "Teleport".to_owned(),
            args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            arg_count: 2,
            tag_slot: Some(1),
        });
    }

    #[test]
    fn rows_for_doom_of_an_unknown_special_are_empty() {
        assert!(
            rows_for_doom(0)
                .expect("the embedded table is valid")
                .is_empty(),
            "the table has no vanilla special 0"
        );
        let one = rows_for_doom(1).expect("the embedded table is valid");
        assert_eq!(one.len(), 1, "special 1 has exactly one row");
    }

    #[test]
    fn rows_for_zspecial_collects_every_source_row() {
        let rows = rows_for_zspecial(206).expect("the embedded table is valid");
        let doom: Vec<i32> = rows.iter().map(|found| found.doom).collect();
        assert_eq!(
            doom,
            vec![
                10_i32, 21_i32, 62_i32, 88_i32, 120_i32, 121_i32, 122_i32, 123_i32
            ],
            "the eight plat rows translate to Plat_DownWaitUpStayLip"
        );
    }

    #[test]
    fn activation_flags_combine_and_test() {
        let flags = XlatFlags::parse("WALK|MONST|REP").expect("three known words");
        assert!(flags.contains(XlatFlags::WALK), "WALK is set");
        assert!(flags.contains(XlatFlags::REP), "REP is set");
        assert!(!flags.contains(XlatFlags::USE), "USE is not set");
        assert!(!flags.is_empty(), "three bits are not empty");
        assert!(
            XlatFlags::parse("0").expect("0 is none").is_empty(),
            "0 is empty"
        );
        let error = XlatFlags::parse("WALK|FLY").expect_err("FLY is no flag");
        assert!(error.contains("FLY"), "the unknown word is named: {error}");
    }

    #[test]
    fn a_wrong_header_line_is_rejected() {
        let error = parse_xlat_table("doom\tactivation\tzspecial\tname\n")
            .expect_err("a four-column header is invalid");
        assert!(error.contains("header"), "the header is named: {error}");
        let empty = parse_xlat_table("").expect_err("no text is invalid");
        assert!(empty.contains("empty"), "the message says so: {empty}");
    }

    #[test]
    fn a_row_with_four_fields_names_the_line() {
        let text = format!("{HEADER}1\tUSE\t12\tDoor_Raise\n");
        let error = parse_xlat_table(&text).expect_err("a four-field row is invalid");
        assert!(error.contains("line 2"), "the line is named: {error}");
    }

    #[test]
    fn a_non_numeric_doom_names_the_line_and_column() {
        let text = format!("{HEADER}x\tUSE\t12\tDoor_Raise\t0\n");
        let error = parse_xlat_table(&text).expect_err("a non-numeric doom is invalid");
        assert!(error.contains("line 2"), "the line is named: {error}");
        assert!(error.contains("\"doom\""), "the column is named: {error}");
    }

    #[test]
    fn an_unknown_activation_word_names_the_line_and_word() {
        let text = format!("{HEADER}1\tFLY\t12\tDoor_Raise\t0\n");
        let error = parse_xlat_table(&text).expect_err("FLY is no activation");
        assert!(error.contains("line 2"), "the line is named: {error}");
        assert!(error.contains("FLY"), "the word is named: {error}");
    }

    #[test]
    fn two_tag_entries_are_refused() {
        let text = format!("{HEADER}1\tUSE\t13\tDoor_LockedRaise\ttag,16,tag\n");
        let error = parse_xlat_table(&text).expect_err("only one tag is allowed");
        assert!(error.contains("line 2"), "the line is named: {error}");
        assert!(error.contains("tag"), "the entry is named: {error}");
    }

    #[test]
    fn six_args_are_refused() {
        let text = format!("{HEADER}1\tUSE\t13\tDoor_LockedRaise\t1,2,3,4,5,6\n");
        let error = parse_xlat_table(&text).expect_err("five slots is the cap");
        assert!(error.contains("line 2"), "the line is named: {error}");
        assert!(error.contains("args"), "the column is named: {error}");
    }

    #[test]
    fn a_negative_arg_is_refused() {
        let text = format!("{HEADER}1\tUSE\t12\tDoor_Raise\t0,-1,150\n");
        let error = parse_xlat_table(&text).expect_err("args are non-negative");
        assert!(error.contains("line 2"), "the line is named: {error}");
    }
}
