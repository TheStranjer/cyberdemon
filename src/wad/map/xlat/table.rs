//! The `Doom` ↔ ZDoom translation table itself: the embedded
//! `data/xlat/doom_to_zdoom.tsv`, the [`XlatFlags`] activation words and the
//! [`XlatRow`] one file line becomes, [`parse_xlat_table`]'s parse of the
//! five tab-separated columns through the shared [`crate::defs::tsv`]
//! reader, and the `LazyLock` accessor trio
//! [`xlat_rows`], [`rows_for_doom`] and [`rows_for_zspecial`] that hand the
//! parsed rows out. A malformed embedded table is kept as an error string
//! and reported as [`CyberError::Internal`](crate::error::CyberError::Internal)
//! by every accessor, never a panic.
//!
//! The linedef stages of the cross-family conversion read the rows through
//! those accessors: [`linedefs_to_hexen`](super::linedefs_to_hexen) writes
//! one row onto a line and
//! [`linedefs_from_hexen`](super::linedefs_from_hexen) searches the rows of
//! one action number in reverse.

use std::sync::LazyLock;

use crate::defs::tsv::{self, Row};
use crate::error::CyberError;

/// The number of tab-separated fields one translation row must carry.
const XLAT_FIELDS: usize = 5;

/// The header the translation table must carry, column by column.
const XLAT_HEADER: [&str; XLAT_FIELDS] = ["doom", "activation", "zspecial", "name", "args"];

/// The argument slots a `ZDoom` action takes; the template is padded to it.
pub(super) const MAX_ARGS: usize = 5;

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
pub(super) const DOOM_TO_ZDOOM_TSV: &str = include_str!("../../../../data/xlat/doom_to_zdoom.tsv");

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
    let table = tsv::Table::parse("xlat table", &XLAT_HEADER, text)?;
    table.rows().iter().map(parse_xlat_row).collect()
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
    tsv::embedded(&XLAT_DOOM_TO_ZDOOM, "Doom-to-ZDoom xlat").map(Vec::as_slice)
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

/// Parse one tab-separated [`XlatRow`].
///
/// # Errors
///
/// A `String` naming the line and the first field that failed to parse.
fn parse_xlat_row(row: &Row<'_>) -> Result<XlatRow, String> {
    let [doom, activation, zspecial, name, args] = row.columns::<XLAT_FIELDS>()?;
    let flags = XlatFlags::parse(activation).map_err(|reason| row.fail(reason.as_str()))?;
    let (values, arg_count, tag_slot) = parse_args(row, args)?;
    Ok(XlatRow {
        doom: row.i32("doom", doom)?,
        activation: flags,
        zspecial: row.i32("zspecial", zspecial)?,
        name: name.to_owned(),
        args: values,
        arg_count,
        tag_slot,
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
fn parse_args(row: &Row<'_>, text: &str) -> Result<ArgsTemplate, String> {
    let mut values: Vec<i32> = Vec::new();
    let mut tag_slot: Option<usize> = None;
    for entry in text.split(ARG_SEP) {
        if values.len() >= MAX_ARGS {
            return Err(row.fail(&format!(
                "args column {text:?} carries more than {MAX_ARGS} entries"
            )));
        }
        if entry == TAG_WORD {
            claim_tag(row, &mut tag_slot, values.len(), text)?;
            values.push(0_i32);
        } else {
            values.push(parse_arg_value(row, entry)?);
        }
    }
    let arg_count = values.len();
    values.resize(MAX_ARGS, 0_i32);
    let Ok(args) = <[i32; MAX_ARGS]>::try_from(values) else {
        return Err(row.fail(&format!(
            "args column {text:?} cannot fill {MAX_ARGS} slots"
        )));
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
    row: &Row<'_>,
    slot: &mut Option<usize>,
    index: usize,
    text: &str,
) -> Result<(), String> {
    if slot.replace(index).is_some() {
        return Err(row.fail(&format!(
            "args column {text:?} carries more than one {TAG_WORD:?} entry"
        )));
    }
    Ok(())
}

/// Parse one args entry that is not `tag`: a non-negative decimal that
/// fits an `i32`.
///
/// # Errors
///
/// A `String` naming the line and the entry.
fn parse_arg_value(row: &Row<'_>, text: &str) -> Result<i32, String> {
    let whole = text.parse::<u32>().map_err(|error| {
        row.fail(&format!(
            "args entry {text:?} is neither {TAG_WORD:?} nor a non-negative decimal: {error}"
        ))
    })?;
    i32::try_from(whole)
        .map_err(|error| row.fail(&format!("args entry {text:?} is too large: {error}")))
}
