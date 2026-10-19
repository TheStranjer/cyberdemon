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
//!
//! The module also holds the thing-flag, sector-type and linedef stages
//! of the cross-family `Doom` ↔ `Hexen` conversion: [`things_to_hexen`]
//! and [`things_from_hexen`] translate [`Thing`] presence flags,
//! [`sectors_to_hexen`] and [`sectors_from_hexen`] the Boom-style sector
//! effects, each returning a [`FieldReport`] of the elements it changed
//! and the values with no equivalent it dropped, and
//! [`linedefs_to_hexen`] and [`linedefs_from_hexen`] run the table over
//! the [`Linedef`] specials in either direction, returning a
//! [`LinesReport`] of the specials that made the trip and the ids, flags
//! and values that could not. They are pure in-place stages over the
//! format-neutral [`Map`], run by the cross-family directions of
//! [`super::convert`].

use core::str::FromStr as _;
use std::sync::LazyLock;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::map::flags::{ACTIVATION_MASK, ACTIVATION_SHIFT, MONSTER_ACTIVATE};
use crate::wad::map::model::{Linedef, Map, Sector, Thing};

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

/// What one cross-family field-translation stage changed and noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FieldReport {
    /// Elements whose translated fields differ from the originals.
    pub changed: usize,
    /// Lossy steps the caller should know about.
    pub warnings: Vec<String>,
}

/// The `Doom`-family linedef bit 512: `passuse`, which the `Hexen` family
/// spends on `repeatspecial`.
const DOOM_PASSUSE: u32 = 0x200;

/// The `Hexen`-family linedef bit 512: `repeatspecial`.
const HEXEN_REPEAT: u32 = 0x200;

/// The linedef flag bits the two families share: `blocking` through
/// `mapped`.
const DOOM_FLAG_BITS: u32 = 0x1FF;

/// The `Hexen`-family bits 0x4000|0x8000: `blockplayers` and
/// `blockeverything`, which no `Doom`-family bit holds.
const HEXEN_BLOCK_BITS: u32 = 0xC000;

/// The binary `Hexen` line special that stores a line id:
/// `Line_SetIdentification`.
const SET_IDENTIFICATION: i32 = 121_i32;

/// The largest value a binary `Hexen` byte field stores.
const MAX_BYTE: i32 = 255_i32;

/// The `extra` key the forward stage parks a `Doom` `passuse` bit under,
/// spelled as the `UDMF` writers write the flag.
const PASSUSE_KEY: &str = "passuse";

/// One linedef whose special the translation stage could not carry
/// across the family boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Untranslated {
    /// The index of the line in [`Map::linedefs`].
    pub linedef: usize,
    /// The special the line carried when the stage gave up on it.
    pub special: i32,
}

/// What one cross-family linedef translation stage changed and noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LinesReport {
    /// Lines whose special moved through the translation table.
    pub translated: usize,
    /// Lines carrying a special this stage could not translate.
    pub untranslated: Vec<Untranslated>,
    /// Lossy steps the caller should know about.
    pub warnings: Vec<String>,
}

/// The low nibble the `Doom` and `Hexen` thing flags share: `easy`,
/// `medium`, `hard` and `ambush` keep their bit in both families.
const THING_SKILL_BITS: u32 = 0x0F;
/// The `Doom` thing bit 16: `notsingle`.
const THING_DOOM_NOT_SINGLE: u32 = 16_u32;
/// The `Doom` thing bit 32: `notdm`.
const THING_DOOM_NOT_DM: u32 = 32_u32;
/// The `Doom` thing bit 64: `notcoop`.
const THING_DOOM_NOT_COOP: u32 = 64_u32;
/// The `Doom` thing bit 128: `friend`.
const THING_DOOM_FRIEND: u32 = 128_u32;
/// The `Hexen` thing bit 16: `dormant`.
const THING_HEXEN_DORMANT: u32 = 16_u32;
/// The `Hexen` thing bits 32|64|128: `class1`, `class2` and `class3`
/// combined.
const THING_HEXEN_CLASSES: u32 = 0xE0;
/// The `Hexen` thing bit 256: `single`.
const THING_HEXEN_SINGLE: u32 = 256_u32;
/// The `Hexen` thing bit 512: `coop`.
const THING_HEXEN_COOP: u32 = 512_u32;
/// The `Hexen` thing bit 1024: `dm`.
const THING_HEXEN_DM: u32 = 1024_u32;
/// The `Hexen` thing bit 2048: `translucent`.
const THING_HEXEN_TRANSLUCENT: u32 = 2048_u32;
/// The `Hexen` thing bit 4096: `invisible`.
const THING_HEXEN_INVISIBLE: u32 = 4096_u32;
/// The `Hexen` thing bit 8192: `friend`.
const THING_HEXEN_FRIEND: u32 = 8192_u32;
/// The `Hexen` thing bit 0x4000: `standing`.
const THING_HEXEN_STANDING: u32 = 0x4000;

/// The highest Boom-style sector special: the `Doom` family packs its
/// sector effects into bits 0 to 9.
const SECTOR_BOOM_MAX: u32 = 0x3FF;
/// The five-bit field of a Boom-style sector's basic type.
const SECTOR_TYPE_MASK: u32 = 0x1F;
/// The `Doom` basic sector type that marks the sector secret.
const SECTOR_SECRET_TYPE: u32 = 9_u32;
/// The offset a translated generalized `Hexen` sector type sits behind.
const SECTOR_HEXEN_BASE: u32 = 64_u32;
/// The lowest `Hexen` type that reads back as translated (base 1).
const SECTOR_HEXEN_FLOOR: u32 = 65_u32;
/// The highest `Hexen` type that reads back as translated (base 17).
const SECTOR_HEXEN_TOP: u32 = 81_u32;
/// The `Hexen` secret bit a translated type-9 sector carries.
const SECTOR_HEXEN_SECRET: u32 = 1024_u32;
/// The `Hexen` bits 8 to 12: the moved damage, secret, friction and push
/// fields of a translated sector special.
const SECTOR_HEXEN_FIELDS: u32 = 0x1F00;
/// The seven low bits of a `Hexen` sector special: the type field.
const SECTOR_HEXEN_CORE: u32 = 0x7F;
/// The only bits a translated `Hexen` sector special may carry.
const SECTOR_HEXEN_ALLOWED: u32 = SECTOR_HEXEN_CORE | SECTOR_HEXEN_FIELDS;
/// The two-bit field mask.
const FIELD_TWO: u32 = 0b11;
/// The one-bit field mask.
const FIELD_ONE: u32 = 1_u32;
/// The number of problem indices one grouped sector warning lists.
const GROUP_LIST_LIMIT: usize = 20;

/// The field of `value` taken from bit `from`, widened by `mask`, and put
/// at bit `to`; every cross-family sector field move is one call.
fn move_field(value: u32, from: u32, mask: u32, to: u32) -> u32 {
    let taken = value.checked_shr(from).unwrap_or(0_u32) & mask;
    taken.checked_shl(to).unwrap_or(0_u32)
}

/// `tally` plus one when `differs`.
#[inline]
fn count_if(differs: bool, tally: usize) -> usize {
    tally.saturating_add(usize::from(differs))
}

/// The `Hexen` thing flags the `Doom` family bits of `flags` become: the
/// shared skill bits, all three class bits, a presence bit for every game
/// mode the thing was not excluded from, and `friend` at its `Hexen` bit.
const fn hexen_thing_flags(flags: u32) -> u32 {
    let mut out = (flags & THING_SKILL_BITS) | THING_HEXEN_CLASSES;
    if flags & THING_DOOM_NOT_SINGLE == 0 {
        out |= THING_HEXEN_SINGLE;
    }
    if flags & THING_DOOM_NOT_COOP == 0 {
        out |= THING_HEXEN_COOP;
    }
    if flags & THING_DOOM_NOT_DM == 0 {
        out |= THING_HEXEN_DM;
    }
    if flags & THING_DOOM_FRIEND != 0 {
        out |= THING_HEXEN_FRIEND;
    }
    out
}

/// The `Doom` thing flags the `Hexen` family bits of `flags` become: the
/// shared skill bits, an exclusion for every game mode or class the thing
/// may not appear in, and `friend` back at its `Doom` bit. The bits `Doom`
/// has no room for drop; the caller reports them.
const fn doom_thing_flags(flags: u32) -> u32 {
    let mut out = flags & THING_SKILL_BITS;
    let all_classes = flags & THING_HEXEN_CLASSES == THING_HEXEN_CLASSES;
    if flags & THING_HEXEN_SINGLE == 0 || !all_classes {
        out |= THING_DOOM_NOT_SINGLE;
    }
    if flags & THING_HEXEN_COOP == 0 {
        out |= THING_DOOM_NOT_COOP;
    }
    if flags & THING_HEXEN_DM == 0 {
        out |= THING_DOOM_NOT_DM;
    }
    if flags & THING_HEXEN_FRIEND != 0 {
        out |= THING_DOOM_FRIEND;
    }
    out
}

/// The `Hexen` generalized sector special the Boom-style `value` becomes:
/// the basic type behind [`SECTOR_HEXEN_BASE`] (the secret bit for type 9,
/// nothing for type 0) with the damage, secret, friction and push fields
/// moved to their `Hexen` bits.
fn hexen_sector_bits(value: u32) -> u32 {
    let base = value & SECTOR_TYPE_MASK;
    let mut out = match base {
        0_u32 => 0_u32,
        SECTOR_SECRET_TYPE => SECTOR_HEXEN_SECRET,
        _ => SECTOR_HEXEN_BASE.saturating_add(base),
    };
    out |= move_field(value, 5_u32, FIELD_TWO, 8_u32);
    out |= move_field(value, 7_u32, FIELD_ONE, 10_u32);
    out |= move_field(value, 8_u32, FIELD_ONE, 11_u32);
    out |= move_field(value, 9_u32, FIELD_ONE, 12_u32);
    out
}

/// The Boom-style value a translated `Hexen` special `value` decodes to,
/// or `None` when it is no type this table translates: a native `Hexen`
/// effect, a bare field or a number the translation never writes.
fn boom_sector_bits(value: u32) -> Option<u32> {
    if value & !SECTOR_HEXEN_ALLOWED != 0 {
        return None;
    }
    let core = value & SECTOR_HEXEN_CORE;
    if !(SECTOR_HEXEN_FLOOR..=SECTOR_HEXEN_TOP).contains(&core) {
        return None;
    }
    let mut out = core.saturating_sub(SECTOR_HEXEN_BASE);
    out |= move_field(value, 8_u32, FIELD_TWO, 5_u32);
    out |= move_field(value, 10_u32, FIELD_ONE, 7_u32);
    out |= move_field(value, 11_u32, FIELD_ONE, 8_u32);
    out |= move_field(value, 12_u32, FIELD_ONE, 9_u32);
    Some(out)
}

/// Drops the script fields one cross-family thing translation clears:
/// `tid`, `special` and `args` carry data only in the `Hexen` family
/// storage.
const fn clear_thing_script(spawned: &mut Thing) {
    spawned.tid = 0_i32;
    spawned.special = 0_i32;
    spawned.args = [0_i32; 5];
}

/// The per-bit counts of the `Hexen` thing flags that have no `Doom`
/// equivalent, gathered while [`things_from_hexen`] rewrites a map.
#[derive(Clone, Copy, Default)]
struct HexenDrops {
    /// Things that carried at least one of the bits.
    touched: usize,
    /// The count of `dormant` bits.
    dormant: usize,
    /// The count of `translucent` bits.
    translucent: usize,
    /// The count of `invisible` bits.
    invisible: usize,
    /// The count of `standing` bits.
    standing: usize,
}

impl HexenDrops {
    /// Counts the dropped bits `flags` carries.
    fn note(&mut self, flags: u32) {
        let mut hit = false;
        hit |= count_bit(flags, THING_HEXEN_DORMANT, &mut self.dormant);
        hit |= count_bit(flags, THING_HEXEN_TRANSLUCENT, &mut self.translucent);
        hit |= count_bit(flags, THING_HEXEN_INVISIBLE, &mut self.invisible);
        hit |= count_bit(flags, THING_HEXEN_STANDING, &mut self.standing);
        self.touched = count_if(hit, self.touched);
    }

    /// The single grouped warning naming the totals, or `None` when every
    /// thing translated without a drop.
    fn warning(&self) -> Option<String> {
        if self.touched == 0_usize {
            return None;
        }
        Some(format!(
            "{} things had Hexen-only flags with no Doom equivalent ({}): they were dropped",
            self.touched,
            self.parts()
        ))
    }

    /// The named per-bit counts, only the nonzero ones, comma-joined.
    fn parts(&self) -> String {
        let named = [
            ("dormant", self.dormant),
            ("translucent", self.translucent),
            ("invisible", self.invisible),
            ("standing", self.standing),
        ];
        named
            .iter()
            .filter(|entry| entry.1 > 0_usize)
            .map(|entry| format!("{} {}", entry.0, entry.1))
            .collect::<Vec<String>>()
            .join(", ")
    }
}

/// Adds one to `counter` when `flags` carries `bit`, answering whether it
/// did.
const fn count_bit(flags: u32, bit: u32, counter: &mut usize) -> bool {
    if flags & bit == 0_u32 {
        return false;
    }
    *counter = counter.saturating_add(1_usize);
    true
}

/// The grouped warning for the sectors whose `special` a stage cleared:
/// up to [`GROUP_LIST_LIMIT`] indices, then the total.
fn cleared_sectors(kind: &str, lost: &[usize]) -> String {
    let total = lost.len();
    let names: Vec<String> = lost
        .iter()
        .take(GROUP_LIST_LIMIT)
        .map(usize::to_string)
        .collect();
    let mut shown = names.join(", ");
    if total > GROUP_LIST_LIMIT {
        let rest = total.saturating_sub(GROUP_LIST_LIMIT);
        shown = format!("{shown}, and {rest} more");
    }
    format!(
        "{total} sectors had specials that are not {kind} (indices {shown}): they were set to 0"
    )
}

/// The grouped warning for the linedefs a stage lost: up to
/// [`GROUP_LIST_LIMIT`] indices, then the total.
fn linedef_warning(reason: &str, consequence: &str, indices: &[usize]) -> String {
    let total = indices.len();
    let names = show_indices(indices);
    format!("{total} linedefs had {reason} (indices {names}): {consequence}")
}

/// The first [`GROUP_LIST_LIMIT`] indices comma-joined, the rest counted.
fn show_indices(indices: &[usize]) -> String {
    let names: Vec<String> = indices
        .iter()
        .take(GROUP_LIST_LIMIT)
        .map(usize::to_string)
        .collect();
    if indices.len() <= GROUP_LIST_LIMIT {
        return names.join(", ");
    }
    let rest = indices.len().saturating_sub(GROUP_LIST_LIMIT);
    format!("{}, and {rest} more", names.join(", "))
}

/// Translate every [`Thing`] of `map` in place from `Doom` to `Hexen`
/// family flags.
///
/// The presence bits invert to their positive `Hexen` twins, every thing
/// joins all three classes, `friend` moves to its `Hexen` bit, and the
/// `tid`, `special` and `args` fields `Doom` has no room for are zeroed.
/// Coordinates, `angle`, `kind` and `z` stand untouched; the translation
/// drops nothing, so the report carries no warnings. `changed` counts the
/// things whose flags moved or whose script fields were not already zero.
#[must_use]
#[inline]
pub fn things_to_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    for spawned in &mut map.things {
        let original = spawned.flags;
        let scripted =
            spawned.tid != 0_i32 || spawned.special != 0_i32 || spawned.args != [0_i32; 5];
        spawned.flags = hexen_thing_flags(original);
        clear_thing_script(spawned);
        report.changed = count_if(spawned.flags != original || scripted, report.changed);
    }
    report
}

/// Translate every [`Thing`] of `map` in place from `Hexen` to `Doom`
/// family flags.
///
/// A thing keeps its skill and ambush bits; it gains `notsingle` unless
/// its `single` bit and all three class bits are set, and `notcoop` and
/// `notdm` unless the matching presence bit is set; `friend` returns to
/// its `Doom` bit. The `dormant`, `translucent`, `invisible` and
/// `standing` bits have no `Doom` equivalent and drop with one grouped
/// warning; the `tid`, `special` and `args` fields are zeroed. `changed`
/// counts the things whose flags moved.
#[must_use]
#[inline]
pub fn things_from_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut dropped = HexenDrops::default();
    for spawned in &mut map.things {
        let original = spawned.flags;
        dropped.note(original);
        spawned.flags = doom_thing_flags(original);
        clear_thing_script(spawned);
        report.changed = count_if(spawned.flags != original, report.changed);
    }
    if let Some(warning) = dropped.warning() {
        report.warnings.push(warning);
    }
    report
}

/// Translate every [`Sector::special`] of `map` in place to the `Hexen`
/// generalized form.
///
/// A Boom-style value (bits 0-4 type, 5-6 damage, 7 secret, 8 friction,
/// 9 push) becomes the type behind 64, the secret type
/// as the `Hexen` secret bit, and the four effect fields at their `Hexen`
/// positions. A value outside the Boom-style bits is cleared to 0 and
/// named in one grouped warning; [`Sector::tag`] is never touched.
#[must_use]
#[inline]
pub fn sectors_to_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut foreign: Vec<usize> = Vec::new();
    for (index, area) in map.sectors.iter_mut().enumerate() {
        if sector_to_hexen(area, &mut report.changed) {
            foreign.push(index);
        }
    }
    if !foreign.is_empty() {
        report
            .warnings
            .push(cleared_sectors("Boom-style generalized values", &foreign));
    }
    report
}

/// Rewrites one sector's `special` to its `Hexen` form, answering whether
/// the value needed clearing instead of translating.
fn sector_to_hexen(area: &mut Sector, changed: &mut usize) -> bool {
    let boom = match u32::try_from(area.special) {
        Ok(value) if value <= SECTOR_BOOM_MAX => Some(hexen_sector_bits(value)),
        Ok(_) | Err(_) => None,
    };
    if let Some(bits) = boom {
        let stored = i32::try_from(bits).unwrap_or(0_i32);
        *changed = count_if(area.special != stored, *changed);
        area.special = stored;
        false
    } else {
        *changed = count_if(area.special != 0_i32, *changed);
        area.special = 0_i32;
        true
    }
}

/// Translate every [`Sector::special`] of `map` in place back from the
/// `Hexen` generalized form to the Boom-style bits.
///
/// Types 65 through 81 return to
/// their Boom-style value with the four effect fields back at their
/// `Doom` positions, and a plain 0 stays 0. Any other value is no
/// translated Boom type: a native `Hexen` effect (or a Boom base type the
/// reverse table does not cover) clears to 0 and is named in one grouped
/// warning. [`Sector::tag`] is never touched.
#[must_use]
#[inline]
pub fn sectors_from_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut foreign: Vec<usize> = Vec::new();
    for (index, area) in map.sectors.iter_mut().enumerate() {
        if sector_from_hexen(area, &mut report.changed) {
            foreign.push(index);
        }
    }
    if !foreign.is_empty() {
        report
            .warnings
            .push(cleared_sectors("translated Boom types", &foreign));
    }
    report
}

/// Rewrites one sector's `special` back to its Boom-style value,
/// answering whether the value needed clearing instead.
fn sector_from_hexen(area: &mut Sector, changed: &mut usize) -> bool {
    if area.special == 0_i32 {
        return false;
    }
    let restored = u32::try_from(area.special).ok().and_then(boom_sector_bits);
    if let Some(bits) = restored {
        let stored = i32::try_from(bits).unwrap_or(0_i32);
        *changed = count_if(area.special != stored, *changed);
        area.special = stored;
        false
    } else {
        *changed = changed.saturating_add(1_usize);
        area.special = 0_i32;
        true
    }
}

/// Translate every [`Linedef`] of `map` in place from the `Doom` to the
/// `Hexen` family, running the embedded table forward.
///
/// A line whose special has a row becomes the row's `ZDoom` action: the
/// row's argument template moves into [`Linedef::args`] with the line's
/// trigger tag replacing the row's `tag` slot, and the row's activation
/// moves to the `Hexen` activation field plus the `repeatspecial` and
/// `monsteractivate` bits; the tag has then found its home and leaves
/// [`Linedef::tag`]. A special with no row — or, under `byte_limited`,
/// one whose action number, arguments or tag exceed a binary `Hexen`
/// byte — translates to nothing: the special clears to 0, the line joins
/// [`LinesReport::untranslated`] and its tag stays as the line id for
/// [`linedefs_from_hexen`] to read back as a trigger tag. A tag with no
/// argument slot to enter (a row without a `tag` slot) stays a line id
/// the same way: `UDMF` carries it in the tag field, while a binary
/// `Hexen` target has no room for one next to a special and drops it
/// with a warning naming the line. `Doom`'s `passuse` bit, whose value
/// `repeatspecial` owns in the `Hexen` family, moves to the line's
/// `extra` bag under the `passuse` key. Sidedefs, sectors, vertices and
/// things stand untouched.
///
/// # Errors
///
/// [`xlat_rows`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn linedefs_to_hexen(map: &mut Map, byte_limited: bool) -> Result<LinesReport, CyberError> {
    let mut report = LinesReport::default();
    for (index, line) in map.linedefs.iter_mut().enumerate() {
        let old = line.special;
        let tag = line.tag;
        share_passuse(line);
        let row = if old == 0_i32 {
            None
        } else {
            rows_for_doom(old)?.into_iter().next()
        };
        let usable = row.filter(|found| !byte_limited || fits_bytes(found, tag));
        if let Some(found) = usable {
            apply_hexen(line, found, tag);
            report.translated = report.translated.saturating_add(1_usize);
        } else {
            note_forward_loss(line, old, index, &mut report);
        }
        keep_line_id(line, index, byte_limited, &mut report.warnings);
    }
    if !report.untranslated.is_empty() {
        let lost: Vec<usize> = report
            .untranslated
            .iter()
            .map(|gone| gone.linedef)
            .collect();
        report.warnings.push(linedef_warning(
            "Doom specials with no ZDoom translation",
            "they were set to 0",
            &lost,
        ));
    }
    Ok(report)
}

/// Keep the flag bits the families share; a `Doom` `passuse` bit moves to
/// the line's `extra` bag under [`PASSUSE_KEY`], because the `Hexen`
/// family owns bit 512 for `repeatspecial` and the `UDMF` writers carry
/// the verbatim key wherever a `passuse` flag cannot sit.
fn share_passuse(line: &mut Linedef) {
    if line.flags & DOOM_PASSUSE != 0_u32 {
        line.extra.insert(PASSUSE_KEY.to_owned(), "true".to_owned());
    }
    line.flags &= DOOM_FLAG_BITS;
}

/// Report one forward step's loss: a special the table did not carry
/// clears to 0 and joins [`LinesReport::untranslated`]; a plain line has
/// nothing to report.
fn note_forward_loss(line: &mut Linedef, old: i32, index: usize, report: &mut LinesReport) {
    if old == 0_i32 {
        return;
    }
    line.special = 0_i32;
    report.untranslated.push(Untranslated {
        linedef: index,
        special: old,
    });
}

/// Whether one row's translated shape fits binary `Hexen` byte fields:
/// the action number and every argument value the line would carry, the
/// tag taking its `tag` slot's place. Slots past [`XlatRow::arg_count`]
/// hold the parser's zero padding, so checking all five is checking
/// exactly the written template.
fn fits_bytes(row: &XlatRow, tag: i32) -> bool {
    let in_byte = |value: i32| (0_i32..=MAX_BYTE).contains(&value);
    in_byte(row.zspecial)
        && row.args.iter().enumerate().all(|(slot, &value)| {
            in_byte(if row.tag_slot == Some(slot) {
                tag
            } else {
                value
            })
        })
}

/// Write one matched row's translated shape onto the line: the template
/// args with the tag in the row's slot, the `ZDoom` action number and the
/// row's trigger bits over the already-masked shared `Doom` bits. The tag
/// field empties when the row took it; a row without a `tag` slot keeps
/// the tag for [`keep_line_id`] to settle as a line id.
fn apply_hexen(line: &mut Linedef, row: &XlatRow, tag: i32) {
    let mut args = row.args;
    if let Some(cell) = row.tag_slot.and_then(|slot| args.get_mut(slot)) {
        *cell = tag;
    }
    line.args = args;
    if row.tag_slot.is_some() {
        line.tag = 0_i32;
    }
    line.special = row.zspecial;
    line.flags |= hexen_trigger_bits(row);
}

/// The `Hexen` trigger bits the row's activation moves to: its expected
/// field at the activation shift, `repeatspecial` and `monsteractivate`
/// when the row carries `REP` and `MONST`.
const fn hexen_trigger_bits(row: &XlatRow) -> u32 {
    let mut bits = match expected_field(row).checked_shl(ACTIVATION_SHIFT) {
        Some(field) => field,
        None => 0_u32,
    };
    if row.activation.contains(XlatFlags::REP) {
        bits |= HEXEN_REPEAT;
    }
    if row.activation.contains(XlatFlags::MONST) {
        bits |= MONSTER_ACTIVATE;
    }
    bits
}

/// Settle a tag no argument slot took as the `Hexen`-family line id. A
/// `UDMF` target keeps it in [`Linedef::tag`] directly; a binary `Hexen`
/// target stores ids only in `Line_SetIdentification`, so a plain line
/// becomes special 121 carrying the id in `args[0]`, and an id on a line
/// whose special the translation used, or one over a byte's range, drops
/// with a warning naming the line.
fn keep_line_id(line: &mut Linedef, index: usize, byte_limited: bool, warnings: &mut Vec<String>) {
    let id = line.tag;
    if !byte_limited || id == 0_i32 {
        return;
    }
    line.tag = 0_i32;
    if line.special != 0_i32 {
        warnings.push(format!(
            "linedef {index} line id {id} was dropped: the line already uses special {}, and \
             binary Hexen stores a line id only in Line_SetIdentification (special \
             {SET_IDENTIFICATION})",
            line.special
        ));
        return;
    }
    if !(0_i32..=MAX_BYTE).contains(&id) {
        warnings.push(format!(
            "linedef {index} line id {id} was dropped: Hexen keeps it in one byte (0 to \
             {MAX_BYTE}); lower the id or convert to UDMF instead"
        ));
        return;
    }
    let [_, arg1, arg2, arg3, arg4] = line.args;
    line.args = [id, arg1, arg2, arg3, arg4];
    line.special = SET_IDENTIFICATION;
}

/// Translate every [`Linedef`] of `map` in place back from the `Hexen` to
/// the `Doom` family, running the embedded table best-effort in reverse.
///
/// `source_udmf` says how the line ids reached the model. In a `UDMF`
/// `Hexen`-family map [`Linedef::tag`] already holds the id; a line whose
/// special matches a row has nowhere to put that id once the translated
/// special takes the `Doom` tag field, so the id drops with a warning
/// naming the line. A line no row matches keeps its id, which simply
/// stands on as the `Doom` trigger tag. A binary `Hexen` map stores the
/// id in `Line_SetIdentification` (special 121): each such line folds to
/// its id first, leaving a plain line whose tag simply stays and becomes
/// the `Doom` trigger tag.
///
/// A line with a special then looks for rows with [`rows_for_zspecial`]
/// and matches each one field by field: the activation field, the repeat
/// and monster bits and every argument outside the row's `tag` slot must
/// agree with the row, the value inside the slot becomes the `Doom`
/// trigger tag (a row without a `tag` slot gives tag 0) and, when several
/// rows match, the lowest source special wins. A match writes the row's
/// `Doom` special, that tag and zeroed args; anything else — an unknown
/// or negative special, an activation field 4 or 5, arguments fitting no
/// row — clears to special 0, joins [`LinesReport::untranslated`] and is
/// named in one grouped warning, its tag left standing as the trigger
/// tag. Every line ends with the shared `Doom` flag bits kept: the
/// activation, repeat and monster bits are consumed and `blockplayers`
/// and `blockeverything` drop with one grouped warning naming the lines
/// that carried either. The `extra` bag stands untouched, so a forwarded
/// `passuse` key rides on to a `UDMF` target. Sidedefs, sectors, vertices
/// and things stand untouched.
///
/// # Errors
///
/// [`xlat_rows`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn linedefs_from_hexen(map: &mut Map, source_udmf: bool) -> Result<LinesReport, CyberError> {
    let mut report = LinesReport::default();
    let mut blocked: Vec<usize> = Vec::new();
    for (index, line) in map.linedefs.iter_mut().enumerate() {
        if !source_udmf {
            fold_line_id(line);
        }
        restore_special(line, index, source_udmf, &mut report)?;
        if line.flags & HEXEN_BLOCK_BITS != 0_u32 {
            blocked.push(index);
        }
        line.flags &= DOOM_FLAG_BITS;
    }
    if !report.untranslated.is_empty() {
        let lost: Vec<usize> = report
            .untranslated
            .iter()
            .map(|gone| gone.linedef)
            .collect();
        report.warnings.push(linedef_warning(
            "ZDoom specials with no Doom translation",
            "they were set to 0",
            &lost,
        ));
    }
    if !blocked.is_empty() {
        report.warnings.push(linedef_warning(
            "Hexen-only blocking flags with no Doom equivalent",
            "they were dropped",
            &blocked,
        ));
    }
    Ok(report)
}

/// Fold one binary `Hexen` `Line_SetIdentification` onto the line id the
/// `UDMF` side of the model uses: `args[0]` becomes [`Linedef::tag`] and
/// the id's own special and slot zero, the same fold the within-family
/// binary-to-`UDMF` conversion makes.
const fn fold_line_id(line: &mut Linedef) {
    if line.special != SET_IDENTIFICATION {
        return;
    }
    let [id, arg1, arg2, arg3, arg4] = line.args;
    line.tag = id;
    line.special = 0_i32;
    line.args = [0_i32, arg1, arg2, arg3, arg4];
}

/// Drop a `UDMF` line's id when a row matched its special: the `Doom` tag
/// field holds the trigger tag the translated special needs, so the id has
/// nowhere to ride on and a warning names the line. A line no row matches
/// never reaches this: its id stands on as the `Doom` trigger tag.
fn note_busy_id(line: &mut Linedef, index: usize, warnings: &mut Vec<String>) {
    let id = line.tag;
    if id == 0_i32 || line.special == 0_i32 {
        return;
    }
    line.tag = 0_i32;
    warnings.push(format!(
        "linedef {index} line id {id} was dropped: the line already uses special {}, and Doom \
         has no line ids; its tag field is the trigger tag",
        line.special
    ));
}

/// Run one line's special through the table in reverse: a plain line keeps
/// its tag (a folded or homeless line id) as the `Doom` trigger tag, a
/// matched row writes its `Doom` special, the argument in its `tag` slot
/// as that tag and zeroed args (a `UDMF` source line's own id drops
/// first, the field belongs to the translated trigger tag), and anything
/// else clears to 0 and joins [`LinesReport::untranslated`] with its tag
/// left standing as the trigger tag.
fn restore_special(
    line: &mut Linedef,
    index: usize,
    source_udmf: bool,
    report: &mut LinesReport,
) -> Result<(), CyberError> {
    let old = line.special;
    if old == 0_i32 {
        return Ok(());
    }
    let rows = rows_for_zspecial(old)?;
    let found = best_match(&rows, line)
        .and_then(|(row_index, tag)| rows.get(row_index).copied().map(|row| (row, tag)));
    if let Some((row, tag)) = found {
        if source_udmf {
            note_busy_id(line, index, &mut report.warnings);
        }
        line.special = row.doom;
        line.tag = tag;
        line.args = [0_i32; MAX_ARGS];
        report.translated = report.translated.saturating_add(1_usize);
        return Ok(());
    }
    line.special = 0_i32;
    report.untranslated.push(Untranslated {
        linedef: index,
        special: old,
    });
    Ok(())
}

/// The best row of `rows` for `line` by the reverse table match: the
/// index into `rows` and the `Doom` trigger tag the line's argument in
/// the row's `tag` slot carries. The table reuses action numbers under
/// different conditions, so several rows can match one line; the lowest
/// [`XlatRow::doom`] wins and file order breaks exact duplicates.
fn best_match(rows: &[&XlatRow], line: &Linedef) -> Option<(usize, i32)> {
    let mut best: Option<(usize, i32)> = None;
    for (index, &row) in rows.iter().enumerate() {
        let Some(tag) = row_matches(row, line) else {
            continue;
        };
        let beats = best
            .is_none_or(|(found, _)| rows.get(found).is_some_and(|prior| row.doom < prior.doom));
        if beats {
            best = Some((index, tag));
        }
    }
    best
}

/// Whether one row describes `line`'s trigger: the activation field the
/// row expects, the repeat and monster bits equal to the row's `REP` and
/// `MONST`, and every argument outside the row's `tag` slot agreeing with
/// the template. The value inside the slot is free and becomes the
/// candidate `Doom` trigger tag.
fn row_matches(row: &XlatRow, line: &Linedef) -> Option<i32> {
    let flags = line.flags;
    if activation_field(flags) != expected_field(row) {
        return None;
    }
    if row.activation.contains(XlatFlags::REP) != (flags & HEXEN_REPEAT != 0_u32) {
        return None;
    }
    if row.activation.contains(XlatFlags::MONST) != (flags & MONSTER_ACTIVATE != 0_u32) {
        return None;
    }
    row_arguments(row, line)
}

/// The line's argument slots against the row's template: the `tag` slot
/// (when the row has one) is free and becomes the candidate tag, every
/// other slot must carry its template value.
fn row_arguments(row: &XlatRow, line: &Linedef) -> Option<i32> {
    let mut tag = 0_i32;
    for (slot, &value) in line.args.iter().enumerate() {
        if row.tag_slot == Some(slot) {
            tag = value;
            continue;
        }
        if value != slot_argument(row, slot) {
            return None;
        }
    }
    Some(tag)
}

/// The value the line's argument slot `slot` must carry for `row`: the
/// template entry while the row wrote slots, zero beyond them.
fn slot_argument(row: &XlatRow, slot: usize) -> i32 {
    if slot < row.arg_count {
        row.args.get(slot).copied().unwrap_or(0_i32)
    } else {
        0_i32
    }
}

/// The activation field value the row expects from the line: `WALK` or no
/// activation names the cross field 0, `USE` the use field 1, `MONWALK`
/// the monster-cross field 2 and `SHOOT` the impact field 3. The shipped
/// table never combines those four in one row; a synthetic row that did
/// would decide by this order.
const fn expected_field(row: &XlatRow) -> u32 {
    if row.activation.contains(XlatFlags::USE) {
        1_u32
    } else if row.activation.contains(XlatFlags::MONWALK) {
        2_u32
    } else if row.activation.contains(XlatFlags::SHOOT) {
        3_u32
    } else {
        0_u32
    }
}

/// The activation field value the `Hexen`-family flag bits carry; fields
/// 4 and 5 exist in the format but no table row uses them, so lines in
/// one match nothing.
const fn activation_field(flags: u32) -> u32 {
    match (flags & ACTIVATION_MASK).checked_shr(ACTIVATION_SHIFT) {
        Some(field) => field,
        None => 0_u32,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DOOM_TO_ZDOOM_TSV, FieldReport, LinesReport, MAX_ARGS, Untranslated, XlatFlags, XlatRow,
        best_match, fits_bytes, linedefs_from_hexen, linedefs_to_hexen, parse_xlat_table,
        rows_for_doom, rows_for_zspecial, sectors_from_hexen, sectors_to_hexen, things_from_hexen,
        things_to_hexen, xlat_rows,
    };
    use crate::wad::map::model::{Linedef, Map, Sector, Thing};

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

    /// A map with one thing per flag value.
    fn thing_map(flags: &[u32]) -> Map {
        Map {
            things: flags
                .iter()
                .copied()
                .map(|bits| Thing {
                    flags: bits,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// A map with one sector per special value.
    fn sector_map(specials: &[i32]) -> Map {
        Map {
            sectors: specials
                .iter()
                .copied()
                .map(|special| Sector {
                    special,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The flags of `map`'s things.
    fn flags_of(map: &Map) -> Vec<u32> {
        map.things.iter().map(|spawned| spawned.flags).collect()
    }

    /// The specials of `map`'s sectors.
    fn specials_of(map: &Map) -> Vec<i32> {
        map.sectors.iter().map(|area| area.special).collect()
    }

    #[test]
    fn things_to_hexen_translates_the_presence_bits() {
        let mut map = thing_map(&[0_u32, 15, 16, 128]);
        let report = things_to_hexen(&mut map);
        assert_eq!(
            flags_of(&map),
            vec![2016_u32, 2031, 1760, 10208],
            "flags 0 becomes single|coop|dm|all classes = 2016; the skill bits 1|2|4|8 stand; \
             notsingle 16 clears the 256 single bit; friend 128 arrives at 8192"
        );
        assert_eq!(
            report.changed, 4,
            "all four flag sets differ from their source"
        );
        assert!(report.warnings.is_empty(), "the forward trip drops nothing");
    }

    #[test]
    fn things_to_hexen_leaves_a_translated_thing_uncounted() {
        // 8672 = 32|64|128|256|8192: the shape this stage writes for a
        // friend thing with single presence, and its fixed point. A value
        // like 2016 is NOT that shape: read as Doom bits its 32, 64 and
        // 128 exclude it from deathmatch and coop and mark it a friend,
        // so the second pass drops those presence bits and adds 8192.
        let mut map = thing_map(&[8672_u32, 2016]);
        let report = things_to_hexen(&mut map);
        assert_eq!(flags_of(&map), vec![8672_u32, 8672], "both settle on 8672");
        assert_eq!(report.changed, 1, "only the moving thing counts as changed");
    }

    #[test]
    fn things_to_hexen_zeroes_the_script_fields() {
        let mut map = thing_map(&[8672_u32, 8672]);
        if let Some(first) = map.things.first_mut() {
            first.tid = 7_i32;
            first.special = 4_i32;
            first.args = [1_i32, 2_i32, 3_i32, 4_i32, 5_i32];
            first.x = 64_i32;
            first.y = -128_i32;
            first.z = 32_i32;
            first.angle = 90_i32;
            first.kind = 3001_i32;
        }
        let report = things_to_hexen(&mut map);
        let first = map.things.first().expect("two things");
        assert_eq!(first.tid, 0_i32, "the tid goes");
        assert_eq!(first.special, 0_i32, "the special goes");
        assert_eq!(first.args, [0_i32; 5], "the args go");
        assert_eq!(first.flags, 8672_u32, "and the translated shape stands");
        assert_eq!(
            (first.x, first.y, first.z, first.angle, first.kind),
            (64_i32, -128_i32, 32_i32, 90_i32, 3001_i32),
            "coordinates, height, facing and kind are untouched"
        );
        assert_eq!(report.changed, 1, "a nonzero script field alone counts");
    }

    #[test]
    fn things_from_hexen_folds_the_hexen_bits_back() {
        let mut map = thing_map(&[10208_u32, 1984, 8192]);
        let report = things_from_hexen(&mut map);
        assert_eq!(
            flags_of(&map),
            vec![128_u32, 16, 240],
            "a fully translated friend reads back as Doom friend 128; the missing class1 bit \
             forces notsingle 16; a bare 8192 thing has no mode or class and is excluded from \
             all of them as well"
        );
        assert_eq!(report.changed, 3, "all three flag sets move");
        assert!(report.warnings.is_empty(), "nothing was dropped");
    }

    #[test]
    fn things_from_hexen_warns_once_about_the_dropped_bits() {
        let mut map = thing_map(&[16_u32, 2064, 4096, 0x4000, 7, 120]);
        let report = things_from_hexen(&mut map);
        assert_eq!(report.warnings.len(), 1, "one grouped warning");
        assert_eq!(
            report.warnings.first().expect("one warning"),
            "5 things had Hexen-only flags with no Doom equivalent (dormant 3, translucent 1, \
             invisible 1, standing 1): they were dropped",
            "the totals and the nonzero per-bit parts, nothing else"
        );
        assert_eq!(report.changed, 5, "the 120 thing keeps its flags");
    }

    #[test]
    fn things_from_hexen_zeroes_the_script_fields_without_counting() {
        let mut map = thing_map(&[120_u32]);
        if let Some(first) = map.things.first_mut() {
            first.tid = 9_i32;
            first.special = 2_i32;
            first.args = [5_i32; 5];
        }
        let report = things_from_hexen(&mut map);
        let first = map.things.first().expect("one thing");
        assert_eq!(first.tid, 0_i32, "the tid goes");
        assert_eq!(first.special, 0_i32, "the special goes");
        assert_eq!(first.args, [0_i32; 5], "the args go");
        assert_eq!(first.flags, 120_u32, "16|32|64|8 reads back as itself");
        assert_eq!(
            report.changed, 0,
            "the reverse counts flag differences only"
        );
        assert_eq!(report.warnings.len(), 1, "the dormant bit still warns");
    }

    #[test]
    fn thing_flags_survive_a_there_and_back_trip() {
        let choices = [8_u32, 16_u32, 32_u32, 64_u32, 128_u32];
        let mut originals: Vec<u32> = vec![0_u32];
        for bit in choices {
            let grown: Vec<u32> = originals.iter().map(|sum| sum | bit).collect();
            originals.extend(grown);
        }
        assert_eq!(
            originals.len(),
            32,
            "every presence combination plus ambush and friend"
        );
        for original in originals {
            let mut map = thing_map(&[original]);
            let there = things_to_hexen(&mut map);
            let back = things_from_hexen(&mut map);
            assert_eq!(flags_of(&map), vec![original], "{original} comes home");
            assert!(
                there.warnings.is_empty() && back.warnings.is_empty(),
                "the trip warns nothing for {original}"
            );
        }
    }

    #[test]
    fn sectors_to_hexen_moves_the_sector_fields() {
        let mut map = sector_map(&[0_i32, 1_i32, 9_i32, 33_i32, 999_i32]);
        let report = sectors_to_hexen(&mut map);
        assert_eq!(
            specials_of(&map),
            vec![0_i32, 65_i32, 1_024_i32, 321_i32, 8_007_i32],
            "plain stays plain, base 1 sits behind 64, base 9 becomes the secret bit, 33 moves \
             its damage field to 65|256, and 999 carries all five inputs at once"
        );
        assert_eq!(report.changed, 4, "only the plain sector stands");
        assert!(report.warnings.is_empty(), "in-range values warn nothing");
    }

    #[test]
    fn sectors_to_hexen_clears_values_the_boom_bits_cannot_hold() {
        let mut map = sector_map(&[1_024_i32, -1_i32, 0_i32]);
        let report = sectors_to_hexen(&mut map);
        assert_eq!(
            specials_of(&map),
            vec![0_i32, 0_i32, 0_i32],
            "each odd value is cleared"
        );
        assert_eq!(report.changed, 2, "the plain sector did not move");
        assert_eq!(report.warnings.len(), 1, "one grouped warning");
        let warning = report.warnings.first().expect("one warning");
        assert!(
            warning.starts_with(
                "2 sectors had specials that are not Boom-style generalized \
                                values (indices 0, 1)"
            ) && warning.ends_with(": they were set to 0"),
            "the total opens and the fix closes it: {warning}"
        );
    }

    #[test]
    fn sector_warnings_list_at_most_twenty_indices() {
        let specials = vec![2048_i32; 25];
        let mut map = sector_map(&specials);
        let report = sectors_to_hexen(&mut map);
        assert_eq!(report.warnings.len(), 1, "still one warning");
        assert_eq!(report.changed, 25, "all twenty-five cleared to 0");
        let warning = report.warnings.first().expect("one warning");
        assert!(
            warning.starts_with("25 sectors") && warning.contains("19, and 5 more"),
            "twenty are listed, the total is stated: {warning}"
        );
        assert!(
            !warning.contains(", 20,") && !warning.contains(" 20, and"),
            "index 20 is not listed: {warning}"
        );
    }

    #[test]
    fn sectors_from_hexen_restores_the_boom_values() {
        let mut map = sector_map(&[0_i32, 65_i32, 81_i32, 1_345_i32]);
        let report = sectors_from_hexen(&mut map);
        assert_eq!(
            specials_of(&map),
            vec![0_i32, 1_i32, 17_i32, 161_i32],
            "0 stays 0, 65 decodes to base 1, 81 to base 17, and 65|256|1024 restores the \
             damage and secret fields of its Boom value 161"
        );
        assert_eq!(report.changed, 3, "the plain sector did not move");
        assert!(report.warnings.is_empty(), "translated values warn nothing");
    }

    #[test]
    fn sectors_from_hexen_clears_types_that_are_not_translated() {
        let mut map = sector_map(&[1_024_i32, 200_i32, 10_i32, -3_i32]);
        let report = sectors_from_hexen(&mut map);
        assert_eq!(
            specials_of(&map),
            vec![0_i32, 0_i32, 0_i32, 0_i32],
            "none of them decodes"
        );
        assert_eq!(report.changed, 4, "all four moved to 0");
        assert_eq!(report.warnings.len(), 1, "one grouped warning");
        let warning = report.warnings.first().expect("one warning");
        assert!(
            warning.starts_with(
                "4 sectors had specials that are not translated Boom types \
                                 (indices 0, 1, 2, 3)"
            ),
            "every index and the total are named: {warning}"
        );
    }

    /// Trips `base` with each of `field_sets` through both sector stages.
    fn trip_sector_base(base: i32, field_sets: &[i32]) {
        for fields in field_sets {
            let original = base | fields;
            let mut map = sector_map(&[original]);
            let there = sectors_to_hexen(&mut map);
            let back = sectors_from_hexen(&mut map);
            assert_eq!(specials_of(&map), vec![original], "{original} comes home");
            assert!(
                there.warnings.is_empty() && back.warnings.is_empty(),
                "the trip warns nothing for {original}"
            );
        }
    }

    #[test]
    fn boom_sector_types_survive_a_there_and_back_trip() {
        // Type 9 is absent by design: it travels as a bare secret bit and
        // reads back as a native Hexen secret, not its Boom value.
        let bases = [
            1_i32, 2_i32, 3_i32, 4_i32, 5_i32, 6_i32, 7_i32, 8_i32, 10_i32, 11_i32, 12_i32, 13_i32,
            14_i32, 15_i32, 16_i32, 17_i32,
        ];
        let field_sets = [
            0_i32, 32_i32, 64_i32, 128_i32, 256_i32, 512_i32, 96_i32, 160_i32, 800_i32,
        ];
        for base in bases {
            trip_sector_base(base, &field_sets);
        }
    }

    #[test]
    fn sector_translation_never_touches_the_tags() {
        let mut map = sector_map(&[1_i32, 1_024_i32]);
        for (index, area) in map.sectors.iter_mut().enumerate() {
            let offset = i32::try_from(index).unwrap_or(0_i32);
            area.tag = offset.saturating_add(40_i32);
        }
        let there = sectors_to_hexen(&mut map);
        let back = sectors_from_hexen(&mut map);
        assert_eq!(there.changed, 2, "the type moves, the odd value clears");
        assert_eq!(
            back.changed, 1,
            "the type comes back; the cleared sector stands"
        );
        assert_eq!(
            specials_of(&map),
            vec![1_i32, 0_i32],
            "the specials round trip"
        );
        let tags: Vec<i32> = map.sectors.iter().map(|area| area.tag).collect();
        assert_eq!(
            tags,
            vec![40_i32, 41_i32],
            "the tags stand through both stages"
        );
    }

    #[test]
    fn empty_maps_translate_to_empty_reports() {
        let mut empty = Map::default();
        assert_eq!(things_to_hexen(&mut empty), FieldReport::default());
        assert_eq!(things_from_hexen(&mut empty), FieldReport::default());
        assert_eq!(sectors_to_hexen(&mut empty), FieldReport::default());
        assert_eq!(sectors_from_hexen(&mut empty), FieldReport::default());
        assert_eq!(
            linedefs_to_hexen(&mut empty, true).expect("the table parses"),
            LinesReport::default(),
            "a binary-target forward pass over no lines"
        );
        assert_eq!(
            linedefs_from_hexen(&mut empty, false).expect("the table parses"),
            LinesReport::default(),
            "a binary-source reverse pass over no lines"
        );
    }

    #[test]
    fn the_field_report_serializes_its_two_fields() {
        let report = FieldReport {
            changed: 2,
            warnings: vec!["two".to_owned()],
        };
        let text = serde_json::to_string(&report).expect("FieldReport serializes");
        assert_eq!(
            text, r#"{"changed":2,"warnings":["two"]}"#,
            "the wire shape the wiring slice will publish"
        );
    }

    #[test]
    fn the_lines_report_serializes_its_three_fields() {
        let report = LinesReport {
            translated: 1,
            untranslated: vec![Untranslated {
                linedef: 2,
                special: 560,
            }],
            warnings: vec!["gone".to_owned()],
        };
        let text = serde_json::to_string(&report).expect("LinesReport serializes");
        assert_eq!(
            text,
            r#"{"translated":1,"untranslated":[{"linedef":2,"special":560}],"warnings":["gone"]}"#,
            "the wire shape the wiring slice will publish"
        );
    }

    #[test]
    fn untranslated_serializes_under_its_plain_field_names() {
        let lost = serde_json::to_value(Untranslated {
            linedef: 3,
            special: 999,
        })
        .expect("Untranslated serializes");
        assert_eq!(
            lost,
            serde_json::json!({"linedef": 3_i32, "special": 999_i32}),
            "plain field names, no serde renames"
        );
    }

    #[test]
    fn the_default_lines_report_is_all_zero_and_empty() {
        let report = LinesReport::default();
        assert_eq!(report.translated, 0, "a fresh report translated nothing");
        assert!(report.untranslated.is_empty(), "and lost no lines");
        assert!(report.warnings.is_empty(), "and warns about nothing");
        assert_eq!(
            serde_json::to_value(&report).expect("LinesReport serializes"),
            serde_json::json!({"translated": 0_i32, "untranslated": [], "warnings": []}),
            "the empty wire shape"
        );
    }

    /// One test linedef: `(special, tag, args, flags)`.
    type LineSpec = (i32, i32, [i32; MAX_ARGS], u32);

    /// The all-zero argument array.
    const ZERO_ARGS: [i32; MAX_ARGS] = [0_i32; MAX_ARGS];

    /// A map with one linedef per spec row.
    fn line_map(lines: &[LineSpec]) -> Map {
        Map {
            linedefs: lines
                .iter()
                .map(|&(special, tag, args, flags)| Linedef {
                    flags,
                    special,
                    tag,
                    args,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The one line of a one-line map.
    fn only_line(map: &Map) -> &Linedef {
        map.linedefs.first().expect("the map has one line")
    }

    #[test]
    fn the_row_1_line_round_trips_through_both_source_shapes() {
        let original = Linedef {
            flags: 0x121_u32,
            special: 1_i32,
            tag: 5_i32,
            ..Default::default()
        };
        for byte_limited in [false, true] {
            let mut map = Map {
                linedefs: vec![original.clone()],
                ..Default::default()
            };
            let there = linedefs_to_hexen(&mut map, byte_limited).expect("the table parses");
            let hexen = only_line(&map);
            assert_eq!(hexen.special, 12_i32, "Door_Raise");
            assert_eq!(
                hexen.args,
                [0_i32, 16_i32, 150_i32, 5_i32, 0_i32],
                "the template with the tag in slot 3"
            );
            assert_eq!(hexen.tag, 0_i32, "the tag moved to the args");
            assert_eq!(
                hexen.flags, 0x2721_u32,
                "Doom bits plus the use field, repeat and monster bits"
            );
            assert_eq!(there.translated, 1, "the row translated");
            assert!(
                there.untranslated.is_empty() && there.warnings.is_empty(),
                "the forward trip is lossless: {there:?}"
            );
            let back = linedefs_from_hexen(&mut map, !byte_limited).expect("the table parses");
            assert_eq!(back.translated, 1, "and read back");
            assert!(
                back.untranslated.is_empty() && back.warnings.is_empty(),
                "the reverse trip is lossless: {back:?}"
            );
            assert_eq!(
                map.linedefs[0], original,
                "there and back with byte_limited={byte_limited}"
            );
        }
    }

    #[test]
    fn the_door_lockedraise_line_finds_row_26_by_slot_3() {
        let mut map = line_map(&[(26_i32, 9_i32, ZERO_ARGS, 0x100_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 13_i32, "Door_LockedRaise");
        assert_eq!(
            hexen.args,
            [0_i32, 16_i32, 150_i32, 130_i32, 9_i32],
            "the locked-kind 130 sits in slot 3"
        );
        assert_eq!(
            hexen.flags, 0x700_u32,
            "mapped plus the use field and repeat, no monster"
        );
        assert_eq!(there.translated, 1, "row 26 translated");
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, 26_i32, "slot-3 130 separates row 26");
        assert_eq!(doom.tag, 9_i32, "read from the tag slot 4");
        assert_eq!(doom.flags, 0x100_u32, "the trigger bits cleared");
        assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
        assert_eq!(back.translated, 1, "exactly one row matched");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "lossless: {back:?}"
        );
    }

    #[test]
    fn an_arg_only_scroller_restores_with_tag_zero() {
        let mut map = line_map(&[(48_i32, 0_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 100_i32, "Scroll_Texture_Left");
        assert_eq!(
            hexen.args,
            [64_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "the speed template, no tag slot"
        );
        assert_eq!(
            hexen.flags, 0_u32,
            "no activation: the cross field stays clear"
        );
        assert_eq!(there.translated, 1, "row 48 translated");
        let mut handbuilt = line_map(&[(100_i32, 0_i32, hexen.args, 0_u32)]);
        let back = linedefs_from_hexen(&mut handbuilt, true).expect("the table parses");
        let doom = only_line(&handbuilt);
        assert_eq!(
            doom.special, 48_i32,
            "zdoom 100 with the speed reads as Doom 48"
        );
        assert_eq!(doom.tag, 0_i32, "a row without a tag slot gives tag 0");
        assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
        assert_eq!(back.translated, 1, "the scroller matched");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "lossless: {back:?}"
        );
    }

    #[test]
    fn a_binary_line_id_folds_back_to_the_trigger_tag() {
        let mut map = line_map(&[(0_i32, 7_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(
            hexen.special, 121_i32,
            "Line_SetIdentification carries the id"
        );
        assert_eq!(
            hexen.args,
            [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "in args[0]"
        );
        assert_eq!(hexen.tag, 0_i32, "binary Hexen models never tag lines");
        assert_eq!(there.translated, 0, "a plain line translates nothing");
        assert!(there.warnings.is_empty(), "an id in a byte never warns");
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, 0_i32, "the id special folded away");
        assert_eq!(doom.tag, 7_i32, "the id became the trigger tag again");
        assert_eq!(doom.args, ZERO_ARGS, "args[0] was zeroed by the fold");
        assert_eq!(back.translated, 0, "nothing needed the table");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "the fold is lossless: {back:?}"
        );
    }

    #[test]
    fn a_udmf_line_keeps_its_id_in_the_tag_field() {
        let mut map = line_map(&[(0_i32, 7_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
        assert_eq!(
            only_line(&map).special,
            0_i32,
            "UDMF writes no Line_SetIdentification"
        );
        assert_eq!(only_line(&map).tag, 7_i32, "the tag is the line id here");
        assert_eq!(there.translated, 0, "a plain line translates nothing");
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(
            only_line(&map).tag,
            7_i32,
            "rule 1: a plain line's id stays as the trigger tag"
        );
        assert_eq!(back.translated, 0, "nothing needed the table");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "the plain tag is no loss: {back:?}"
        );
    }

    #[test]
    fn the_teleport_rows_split_on_activation_bits() {
        let cases = [
            (39_i32, 0x2000_u32),
            (97_i32, 0x2200),
            (125_i32, 0x0800),
            (126_i32, 0x0A00),
        ];
        for (source, trigger) in cases {
            let mut map = line_map(&[(source, 7_i32, ZERO_ARGS, 0_u32)]);
            let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
            let hexen = only_line(&map);
            assert_eq!(hexen.special, 70_i32, "all four share Teleport");
            assert_eq!(
                hexen.args,
                [0_i32, 7_i32, 0_i32, 0_i32, 0_i32],
                "the tag rides in slot 1"
            );
            assert_eq!(
                hexen.flags, trigger,
                "the field and bits row {source} asks for"
            );
            assert_eq!(there.translated, 1, "Doom {source} translated");
            let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
            let doom = only_line(&map);
            assert_eq!(doom.special, source, "and matched back to itself");
            assert_eq!(doom.tag, 7_i32, "read from the tag slot");
            assert_eq!(back.translated, 1, "one row matched for {source}");
            assert!(
                back.untranslated.is_empty() && back.warnings.is_empty(),
                "unambiguous for {source}: {back:?}"
            );
        }
    }

    #[test]
    fn an_unknown_zspecial_clears_and_is_reported() {
        let mut map = line_map(&[(
            560_i32,
            3_i32,
            [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
            0x201_u32,
        )]);
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, 0_i32, "no row, no special");
        assert_eq!(
            doom.flags, 0x1_u32,
            "the repeat bit is consumed, blocking stays"
        );
        assert_eq!(
            doom.tag, 3_i32,
            "a still-nonzero tag is the Doom trigger tag"
        );
        assert_eq!(back.translated, 0, "nothing translated");
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 560
            }],
            "the line and its old special are recorded"
        );
        assert_eq!(back.warnings.len(), 1, "one grouped warning");
        assert_eq!(
            back.warnings[0],
            "1 linedefs had ZDoom specials with no Doom translation (indices 0): they were set \
             to 0"
        );
    }

    #[test]
    fn activation_fields_the_table_never_writes_are_unmatched() {
        for field in [4_u32, 5] {
            let flags = field.checked_shl(10).unwrap_or(0_u32);
            let mut map =
                line_map(&[(12_i32, 0_i32, [0_i32, 16_i32, 150_i32, 5_i32, 0_i32], flags)]);
            let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
            let doom = only_line(&map);
            assert_eq!(doom.special, 0_i32, "field {field} matches no row");
            assert_eq!(
                back.untranslated,
                vec![Untranslated {
                    linedef: 0,
                    special: 12
                }],
                "the field-{field} line is reported"
            );
            assert_eq!(back.translated, 0, "nothing translated");
            assert_eq!(
                doom.flags & 0x1C00,
                0_u32,
                "the activation field bits cleared too"
            );
        }
    }

    #[test]
    fn a_busy_udmf_id_drops_with_a_warning_and_the_line_still_translates() {
        let mut map = line_map(&[(243_i32, 4_i32, ZERO_ARGS, 0x400_u32)]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(
            doom.special, 11_i32,
            "zdoom 243 with the use field is row 11"
        );
        assert_eq!(
            doom.tag, 0_i32,
            "the row has no tag slot, so the freed id does not return"
        );
        assert_eq!(back.translated, 1, "the special itself translated");
        assert!(back.untranslated.is_empty(), "a match is no miss");
        assert_eq!(
            back.warnings.len(),
            1,
            "one busy-id warning, no grouped miss"
        );
        assert!(
            back.warnings[0].starts_with("linedef 0 line id 4 was dropped")
                && back.warnings[0].contains("already uses special 243"),
            "the line, its id and its special are named: {}",
            back.warnings[0]
        );
    }

    #[test]
    fn an_unmatched_udmf_line_keeps_its_id_as_the_doom_trigger_tag() {
        let mut map = line_map(&[(10_i32, 4_i32, ZERO_ARGS, 0_u32)]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(only_line(&map).tag, 4_i32, "no row took the tag field");
        assert_eq!(
            only_line(&map).special,
            0_i32,
            "and the door row needs 16 in slot 1"
        );
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 10
            }],
            "the special is reported"
        );
        assert_eq!(back.warnings.len(), 1, "only the grouped miss warns");
        assert!(
            back.warnings[0].starts_with("1 linedefs had ZDoom specials"),
            "{}",
            back.warnings[0]
        );
    }

    #[test]
    fn a_set_identification_from_a_udmf_source_matches_no_row() {
        // UDMF has no Line_SetIdentification: a zdoom line hand-written with
        // special 121 is just a special the table carries no row for.
        let mut map = line_map(&[(121_i32, 7_i32, ZERO_ARGS, 0_u32)]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, 0_i32, "the unmatched path clears the special");
        assert_eq!(doom.tag, 7_i32, "its id stands on as the Doom tag");
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 121
            }],
            "121 lands among the unmatched"
        );
        assert_eq!(back.warnings.len(), 1, "only the grouped miss warns");
    }

    #[test]
    fn hexen_only_blocking_bits_drop_with_one_grouped_warning() {
        let mut map = line_map(&[
            (0_i32, 0_i32, ZERO_ARGS, 0x8001_u32),
            (0_i32, 0_i32, ZERO_ARGS, 0x4100),
            (0_i32, 0_i32, ZERO_ARGS, 0x1),
        ]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let flags: Vec<u32> = map.linedefs.iter().map(|line| line.flags).collect();
        assert_eq!(
            flags,
            vec![0x1_u32, 0x100, 0x1],
            "the shared Doom bits kept"
        );
        assert_eq!(back.translated, 0, "plain lines translate nothing");
        assert_eq!(back.warnings.len(), 1, "one grouped warning");
        assert_eq!(
            back.warnings[0],
            "2 linedefs had Hexen-only blocking flags with no Doom equivalent (indices 0, 1): \
             they were dropped"
        );
    }

    #[test]
    fn best_match_gives_a_tie_to_the_lower_source_special() {
        let row_a = XlatRow {
            doom: 5_i32,
            activation: XlatFlags::WALK,
            zspecial: 999_i32,
            name: "Tie_First".to_owned(),
            args: ZERO_ARGS,
            arg_count: 2,
            tag_slot: Some(1),
        };
        let row_b = XlatRow {
            doom: 2_i32,
            ..row_a.clone()
        };
        let line = Linedef {
            special: 999_i32,
            args: [0_i32, 9_i32, 0_i32, 0_i32, 0_i32],
            ..Default::default()
        };
        let rows = [&row_a, &row_b];
        let matched = best_match(&rows, &line);
        assert_eq!(
            matched.map(|(index, _)| index),
            Some(1_usize),
            "the later, lower-doom row wins the tie"
        );
        assert_eq!(
            matched.map(|(_, tag)| tag),
            Some(9_i32),
            "and the tag comes from its own slot"
        );
        assert_eq!(best_match(&[], &line), None, "no rows, no match");
    }

    #[test]
    fn linedef_group_warnings_list_at_most_twenty_indices() {
        let lines: Vec<LineSpec> = (0..25_usize)
            .map(|_| (560_i32, 0_i32, ZERO_ARGS, 0_u32))
            .collect();
        let mut map = line_map(&lines);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(back.untranslated.len(), 25, "every line is recorded");
        assert_eq!(back.warnings.len(), 1, "still one warning");
        let warning = back.warnings.first().expect("one warning");
        assert!(
            warning.starts_with("25 linedefs") && warning.contains("19, and 5 more"),
            "twenty are listed, the total is stated: {warning}"
        );
        assert!(
            !warning.contains(", 20,"),
            "index 20 is not listed: {warning}"
        );
    }

    #[test]
    fn passuse_rides_in_the_extra_bag_through_both_stages() {
        let mut map = line_map(&[(0_i32, 0_i32, ZERO_ARGS, 0x201_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(
            hexen.flags, 0x1_u32,
            "bit 512 is repeatspecial in this family"
        );
        assert_eq!(
            hexen.extra.get("passuse").map(String::as_str),
            Some("true"),
            "the key rode over"
        );
        assert_eq!(there.translated, 0, "a plain line translates nothing");
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.flags, 0x1_u32, "no Hexen bits to consume");
        assert_eq!(
            doom.extra.get("passuse").map(String::as_str),
            Some("true"),
            "the reverse stage never touches extra"
        );
        assert_eq!(back.translated, 0, "and nothing to translate back");
        assert!(
            there.warnings.is_empty() && back.warnings.is_empty(),
            "carrying passuse warns nothing"
        );
    }

    #[test]
    fn an_unknown_doom_special_clears_and_keeps_its_tag_as_a_line_id() {
        let mut map = line_map(&[(9999_i32, 3_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 121_i32, "the plain id carrier takes over");
        assert_eq!(
            hexen.args,
            [3_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "the trigger tag rides on as the id"
        );
        assert_eq!(hexen.tag, 0_i32, "binary Hexen models never tag lines");
        assert_eq!(there.translated, 0, "the special found no row");
        assert_eq!(
            there.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 9999
            }],
            "the old Doom special is recorded"
        );
        assert_eq!(there.warnings.len(), 1, "one grouped warning");
        assert!(
            there.warnings[0]
                .starts_with("1 linedefs had Doom specials with no ZDoom translation (indices 0)"),
            "{}",
            there.warnings[0]
        );
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        assert_eq!(only_line(&map).special, 0_i32, "the id carrier folded away");
        assert_eq!(
            only_line(&map).tag,
            3_i32,
            "and the id reads back as the trigger tag"
        );
        assert_eq!(back.translated, 0, "nothing to match");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "the miss was already reported: {back:?}"
        );
    }

    #[test]
    fn byte_limited_refuses_actions_the_hexen_bytes_cannot_hold() {
        let mut narrow = line_map(&[(100_i32, 5_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut narrow, true).expect("the table parses");
        assert_eq!(
            there.translated, 0,
            "Stairs_BuildUpDoomCrush needs action 273"
        );
        assert_eq!(
            there.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 100
            }],
            "the vetoed line is reported"
        );
        assert_eq!(
            only_line(&narrow).special,
            121_i32,
            "the line fell back to a plain id carrier"
        );
        assert_eq!(
            only_line(&narrow).args,
            [5_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "its tag rides on as the id"
        );
        let door = rows_for_doom(26).expect("the table parses")[0];
        assert!(
            fits_bytes(door, 9_i32),
            "row 26's template fits the bytes with a narrow tag"
        );
        assert!(
            !fits_bytes(door, 300_i32),
            "the wide tag landing in its slot 4 vetoes the line"
        );
        assert!(
            !fits_bytes(door, -1_i32),
            "a negative tag never fits a u8 slot"
        );
        let stairs = rows_for_doom(100).expect("the table parses")[0];
        assert!(!fits_bytes(stairs, 5_i32), "zspecial 273 is the veto here");
    }

    #[test]
    fn a_stairs_line_trips_udmf_untouched_by_the_byte_limit() {
        let mut map = line_map(&[(100_i32, 5_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
        assert_eq!(there.translated, 1, "UDMF args are not byte-limited");
        assert_eq!(
            only_line(&map).special,
            273_i32,
            "the stairs action rides on"
        );
        assert_eq!(
            only_line(&map).args,
            [5_i32, 32_i32, 16_i32, 0_i32, 0_i32],
            "the tag took slot 0"
        );
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(back.translated, 1, "and zspecial 273 reads back in UDMF");
        assert_eq!(
            only_line(&map).special,
            100_i32,
            "row 100, not the USE twin 127"
        );
        assert_eq!(only_line(&map).tag, 5_i32, "the tag returned from slot 0");
        assert_eq!(
            only_line(&map).args,
            ZERO_ARGS,
            "args zeroed on the way back"
        );
        assert!(
            back.warnings.is_empty(),
            "the UDMF trip was lossless: {back:?}"
        );
    }

    #[test]
    fn a_tag_with_no_slot_rides_the_udmf_id_and_warns_on_the_way_back() {
        let mut map = line_map(&[(11_i32, 4_i32, ZERO_ARGS, 0x100_u32)]);
        let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 243_i32, "Exit_Normal has no tag argument");
        assert_eq!(hexen.tag, 4_i32, "the trigger tag rides on as the line id");
        assert_eq!(there.translated, 1, "row 11 translated");
        assert!(there.warnings.is_empty(), "UDMF loses nothing yet");
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(
            doom.special, 11_i32,
            "zdoom 243 with the use field is row 11"
        );
        assert_eq!(doom.tag, 0_i32, "the id had nowhere to end up");
        assert_eq!(
            doom.flags, 0x100_u32,
            "the use field bit was consumed, mapped kept"
        );
        assert_eq!(back.translated, 1, "the special itself translated");
        assert_eq!(back.warnings.len(), 1, "the busy-id warning names the loss");
        assert!(
            back.warnings[0].contains("linedef 0 line id 4"),
            "{}",
            back.warnings[0]
        );
    }

    #[test]
    fn a_homeless_tag_drops_at_once_for_a_binary_target() {
        let mut map = line_map(&[(11_i32, 4_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        assert_eq!(there.translated, 1, "Exit_Normal itself fits the bytes");
        assert_eq!(
            there.warnings.len(),
            1,
            "binary Hexen drops the homeless id at once"
        );
        assert!(
            there.warnings[0].contains("linedef 0 line id 4 was dropped")
                && there.warnings[0].contains("already uses special 243"),
            "{}",
            there.warnings[0]
        );
        assert_eq!(only_line(&map).tag, 0_i32, "the dropped id left the model");
        let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
        assert_eq!(
            only_line(&map).special,
            11_i32,
            "the line itself still came home"
        );
        assert_eq!(back.translated, 1, "and through the table");
        assert!(
            back.warnings.is_empty(),
            "the loss was already reported: {back:?}"
        );
    }

    #[test]
    fn a_wide_homeless_tag_drops_with_the_byte_range_hint() {
        let mut map = line_map(&[(0_i32, 300_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        assert_eq!(there.translated, 0, "a plain line translates nothing");
        assert_eq!(there.warnings.len(), 1, "the id itself does not fit a byte");
        assert!(
            there.warnings[0].starts_with("linedef 0 line id 300 was dropped")
                && there.warnings[0].contains("one byte"),
            "{}",
            there.warnings[0]
        );
        let doom = only_line(&map);
        assert_eq!(doom.tag, 0_i32, "the doomed id left with the warning");
        assert_eq!(doom.special, 0_i32, "and no id carrier was written");
    }

    #[test]
    fn a_wide_tag_on_a_translated_line_drops_with_the_busy_warning() {
        let mut map = line_map(&[(48_i32, 300_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
        assert_eq!(there.translated, 1, "zspecial 100 and arg 64 fit the bytes");
        assert_eq!(
            only_line(&map).special,
            100_i32,
            "the line translated anyway"
        );
        assert_eq!(
            only_line(&map).tag,
            0_i32,
            "the homeless id left with the warning"
        );
        assert_eq!(
            there.warnings.len(),
            1,
            "one drop, the used special being the reason"
        );
        assert!(
            there.warnings[0].starts_with("linedef 0 line id 300 was dropped")
                && there.warnings[0].contains("already uses special 100"),
            "{}",
            there.warnings[0]
        );
    }

    #[test]
    fn the_reverse_pass_mixes_every_branch() {
        let mut map = line_map(&[
            (
                12_i32,
                0_i32,
                [0_i32, 16_i32, 150_i32, 5_i32, 0_i32],
                0x2600_u32,
            ),
            (0_i32, 2_i32, ZERO_ARGS, 0_u32),
            (560_i32, 0_i32, ZERO_ARGS, 0x4000),
        ]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(back.translated, 1, "line 0 came back through the table");
        assert_eq!(map.linedefs[0].special, 1_i32, "row 1 matched");
        assert_eq!(map.linedefs[0].tag, 5_i32, "its tag read from slot 3");
        assert_eq!(
            map.linedefs[1].tag, 2_i32,
            "the plain id stayed the trigger tag"
        );
        assert_eq!(
            map.linedefs[1].special, 0_i32,
            "rule 1: nothing to translate"
        );
        assert_eq!(
            map.linedefs[2].special, 0_i32,
            "the unmatched special cleared"
        );
        assert_eq!(map.linedefs[2].flags, 0_u32, "blockplayers has no Doom bit");
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 2,
                special: 560
            }],
            "only the miss is recorded"
        );
        assert_eq!(
            back.warnings.len(),
            2,
            "the grouped miss and the grouped flag drop"
        );
        assert!(
            back.warnings[0].contains("(indices 2)"),
            "{}",
            back.warnings[0]
        );
        assert!(
            back.warnings[1].contains("(indices 2)"),
            "{}",
            back.warnings[1]
        );
    }

    /// One row of the embedded table there and back: Doom `special`
    /// `row.doom` with trigger tag 13 forward through `UDMF`, then reverse.
    /// The special always comes back; the tag returns from the row's `tag`
    /// slot when the row has one and ends 0 when it has none (the homeless
    /// line id drops on the way back).
    fn trip_table_row(row: &XlatRow) {
        let mut map = line_map(&[(row.doom, 13_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
        assert_eq!(there.translated, 1, "doom {} translated", row.doom);
        assert!(
            there.untranslated.is_empty() && there.warnings.is_empty(),
            "the forward trip of row {} is lossless: {there:?}",
            row.doom
        );
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        assert_eq!(back.translated, 1, "doom {} came back", row.doom);
        assert!(back.untranslated.is_empty(), "row {} found", row.doom);
        let doom = only_line(&map);
        assert_eq!(
            doom.special, row.doom,
            "row {} restores its own special",
            row.doom
        );
        let wanted = if row.tag_slot.is_some() {
            13_i32
        } else {
            0_i32
        };
        assert_eq!(doom.tag, wanted, "the tag of row {}", row.doom);
        assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
        assert_eq!(doom.flags, 0_u32, "the trigger bits were consumed");
        let lost = back
            .warnings
            .iter()
            .filter(|note| note.contains("line id 13 was dropped"))
            .count();
        if row.tag_slot.is_some() {
            assert!(
                back.warnings.is_empty(),
                "lossless trip: {:?}",
                back.warnings
            );
        } else {
            assert_eq!(
                lost, 1_usize,
                "row {} carries its tag as a busy id: {:?}",
                row.doom, back.warnings
            );
        }
    }

    #[test]
    fn every_table_row_round_trips_through_udmf() {
        let rows = xlat_rows().expect("the embedded table parses");
        for row in rows {
            trip_table_row(row);
        }
        let tagless = rows.iter().filter(|row| row.tag_slot.is_none()).count();
        assert_eq!(tagless, 6_usize, "the six arg-only rows have no tag slot");
    }

    #[test]
    fn the_crushing_stairs_row_drops_its_wide_action_at_the_byte_limit() {
        // Going in: binary Hexen has no byte for zspecial 273.
        let mut entering = line_map(&[(100_i32, 13_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut entering, true).expect("the table parses");
        assert_eq!(there.translated, 0, "row 100's action does not fit a byte");
        assert_eq!(
            there.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 100
            }],
            "the vetoed Doom special is reported"
        );
        assert_eq!(there.warnings.len(), 1, "one grouped miss");
        let hexen = only_line(&entering);
        assert_eq!(hexen.special, 121_i32, "the cleared line carries its id");
        assert_eq!(
            hexen.args,
            [13_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "the trigger tag rode on as the line id"
        );
        // Coming out: a line still carrying zdoom 273 that fits no row.
        let mut leaving =
            line_map(&[(273_i32, 13_i32, [1_i32, 2_i32, 3_i32, 4_i32, 5_i32], 0_u32)]);
        let back = linedefs_from_hexen(&mut leaving, false).expect("the table parses");
        assert_eq!(back.translated, 0, "neither row 100 nor row 127 accepts it");
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 273
            }],
            "273 is reported leaving as well"
        );
        assert_eq!(back.warnings.len(), 1, "one grouped miss");
        let doom = only_line(&leaving);
        assert_eq!(doom.special, 0_i32, "the special ends 0");
        assert_eq!(doom.tag, 13_i32, "its tag stands as the trigger tag");
    }
}
