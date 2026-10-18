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
//! The module also holds the thing-flag and sector-type stages of the
//! cross-family `Doom` ↔ `Hexen` conversion: [`things_to_hexen`] and
//! [`things_from_hexen`] translate [`Thing`] presence flags,
//! [`sectors_to_hexen`] and [`sectors_from_hexen`] the Boom-style sector
//! effects, each returning a [`FieldReport`] of the elements it changed
//! and the values with no equivalent it dropped. They are pure in-place
//! stages over the format-neutral [`Map`], reserved for the cross-family
//! wiring that follows this slice.

use core::str::FromStr as _;
use std::sync::LazyLock;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::map::model::{Map, Sector, Thing};

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

#[cfg(test)]
mod tests {
    use super::{
        DOOM_TO_ZDOOM_TSV, FieldReport, XlatFlags, XlatRow, parse_xlat_table, rows_for_doom,
        rows_for_zspecial, sectors_from_hexen, sectors_to_hexen, things_from_hexen,
        things_to_hexen, xlat_rows,
    };
    use crate::wad::map::model::{Map, Sector, Thing};

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
}
