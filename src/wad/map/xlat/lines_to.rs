//! The forward linedef stage of the cross-family conversion.
//!
//! [`linedefs_to_hexen`] runs the embedded table onto every
//! [`Linedef`] special, writing the row's
//! [`XlatRow`] action, template args and trigger bits,
//! parking a `Doom` `passuse` bit in the line's `extra` bag and settling a
//! tag no argument slot took as the `Hexen`-family line id.
//!
//! A special the table does not cover, and under `byte_limited` a row whose
//! action number, argument or `tag` slot value a binary `Hexen` byte cannot
//! store, clear to 0 and join [`LinesReport`] as an
//! [`Untranslated`] row, with the warnings this
//! module's `linedef_warning`, `show_indices` and `too_wide_note` builders
//! phrase. The constants here are the linedef bits and fields both
//! directions read; [`linedefs_from_hexen`](super::linedefs_from_hexen) is
//! the reverse.

use crate::error::CyberError;
use crate::wad::map::flags::{
    ACTIVATION_SHIFT, MONSTER_ACTIVATE, PASS_USE, REPEAT_SPECIAL, SHARED_LINEDEF_BITS,
};
use crate::wad::map::model::{Linedef, Map};

use super::lines_from::expected_field;
use super::table::{XlatFlags, XlatRow, rows_for_doom};
use super::{LinesReport, Untranslated, show_indices};

/// The binary `Hexen` line special that stores a line id:
/// `Line_SetIdentification`.
pub(super) const SET_IDENTIFICATION: i32 = 121_i32;

/// The largest value a binary `Hexen` byte field stores.
const MAX_BYTE: i32 = 255_i32;

/// The `extra` key the forward stage parks a `Doom` `passuse` bit under,
/// spelled as the `UDMF` writers write the flag.
const PASSUSE_KEY: &str = "passuse";

/// The grouped warning for the linedefs a stage lost: up to
/// [`GROUP_LIST_LIMIT`](super::GROUP_LIST_LIMIT) indices, then the total.
pub(super) fn linedef_warning(reason: &str, consequence: &str, indices: &[usize]) -> String {
    let total = indices.len();
    let names = show_indices(indices);
    format!("{total} linedefs had {reason} (indices {names}): {consequence}")
}

/// Translate every [`Linedef`] of `map` in place from the `Doom` to the
/// `Hexen` family, running the embedded table forward.
///
/// A line whose special has a row becomes the row's `ZDoom` action: the
/// row's argument template moves into [`Linedef::args`] with the line's
/// trigger tag replacing the row's `tag` slot, and the row's activation
/// moves to the `Hexen` activation field plus the `repeatspecial` and
/// `monsteractivate` bits; the tag has then found its home and leaves
/// [`Linedef::tag`]. A special with no row translates to nothing: the
/// special clears to 0, the line joins [`LinesReport::untranslated`] and
/// one grouped warning names the lost lines. Under `byte_limited` a row
/// whose action number, one of whose arguments or whose `tag` slot value
/// exceeds a binary `Hexen` byte is refused the same way, except that the
/// line earns its own warning naming the action it would have run and the
/// field too wide to store; a `UDMF` target keeps that action. Every
/// cleared line keeps its tag as the line id for
/// [`linedefs_from_hexen`](super::linedefs_from_hexen)
/// to read back as a trigger tag, and so does a tag with no argument slot
/// to enter (a row without a `tag` slot): `UDMF` carries it in the tag
/// field, while a binary `Hexen` target has no room for one next to a
/// special and drops it with a warning naming the line. `Doom`'s `passuse`
/// bit, whose value `repeatspecial` owns in the `Hexen` family, moves to
/// the line's `extra` bag under the `passuse` key. Sidedefs, sectors,
/// vertices and things stand untouched.
///
/// # Errors
///
/// [`xlat_rows`](super::xlat_rows)' [`CyberError::Internal`] for malformed
/// embedded data.
#[inline]
pub fn linedefs_to_hexen(map: &mut Map, byte_limited: bool) -> Result<LinesReport, CyberError> {
    let mut report = LinesReport::default();
    let mut unmatched: Vec<usize> = Vec::new();
    for (index, line) in map.linedefs.iter_mut().enumerate() {
        let old = line.special;
        let tag = line.tag;
        share_passuse(line);
        let row = row_for(old)?;
        match forward_line(line, tag, row, byte_limited) {
            Forward::Applied => report.translated = report.translated.saturating_add(1_usize),
            Forward::NoRow => {
                note_forward_loss(line, old, index, &mut report);
                unmatched.push(index);
            }
            Forward::TooWide { row: vetoed, field } => {
                note_forward_loss(line, old, index, &mut report);
                let warning = too_wide_note(index, old, vetoed, &field);
                report.warnings.push(warning);
            }
            Forward::Plain => {}
        }
        keep_line_id(line, index, byte_limited, &mut report.warnings);
    }
    if !unmatched.is_empty() {
        report.warnings.push(linedef_warning(
            "Doom specials with no ZDoom translation",
            "they were set to 0",
            &unmatched,
        ));
    }
    Ok(report)
}

/// The first embedded row translating `special`: `None` for the plain
/// special 0 and for a special the table does not cover.
///
/// # Errors
///
/// [`rows_for_doom`]' [`CyberError::Internal`] for malformed embedded data.
fn row_for(special: i32) -> Result<Option<&'static XlatRow>, CyberError> {
    if special == 0_i32 {
        return Ok(None);
    }
    Ok(rows_for_doom(special)?.into_iter().next())
}

/// What one line's forward translation did with the row found for it.
enum Forward {
    /// The line carried no special to translate.
    Plain,
    /// The row's translated shape was written onto the line.
    Applied,
    /// No row covers the line's special.
    NoRow,
    /// A row covers the special, but a binary `Hexen` byte cannot store
    /// `field`, one of the values the row would have written.
    TooWide {
        /// The vetoed row, for the warning's action name and number.
        row: &'static XlatRow,
        /// The out-of-range value, already phrased as the field it is.
        field: String,
    },
}

/// Translate one line by the table `row` found for it: write the row's
/// shape when the target can store every value, name the field a binary
/// `Hexen` record cannot hold, and tell a plain line from a special the
/// table does not cover.
fn forward_line(
    line: &mut Linedef,
    tag: i32,
    row: Option<&'static XlatRow>,
    byte_limited: bool,
) -> Forward {
    let Some(found) = row else {
        return if line.special == 0_i32 {
            Forward::Plain
        } else {
            Forward::NoRow
        };
    };
    let too_wide = byte_limited.then(|| wide_field(found, tag)).flatten();
    too_wide.map_or_else(
        || {
            apply_hexen(line, found, tag);
            Forward::Applied
        },
        |field| Forward::TooWide { row: found, field },
    )
}

/// Keep the flag bits the families share; a `Doom` `passuse` bit moves to
/// the line's `extra` bag under [`PASSUSE_KEY`], because the `Hexen`
/// family owns bit 512 for `repeatspecial` and the `UDMF` writers carry
/// the verbatim key wherever a `passuse` flag cannot sit.
fn share_passuse(line: &mut Linedef) {
    if line.flags & PASS_USE != 0_u32 {
        line.extra.insert(PASSUSE_KEY.to_owned(), "true".to_owned());
    }
    line.flags &= SHARED_LINEDEF_BITS;
}

/// Report one forward step's loss: the special the stage could not carry
/// across clears to 0 and joins [`LinesReport::untranslated`] with the
/// `Doom` number it had.
fn note_forward_loss(line: &mut Linedef, old: i32, index: usize, report: &mut LinesReport) {
    line.special = 0_i32;
    report.untranslated.push(Untranslated {
        linedef: index,
        special: old,
    });
}

/// Whether `value` is one a binary `Hexen` field stores in its byte.
fn in_hexen_byte(value: i32) -> bool {
    (0_i32..=MAX_BYTE).contains(&value)
}

/// The first translated value of `row` a binary `Hexen` byte cannot hold,
/// phrased as the field it belongs to: the action number, then the five
/// argument slots with the line's `tag` taking the row's `tag` slot's
/// place. `None` when every value fits. Slots past [`XlatRow::arg_count`]
/// hold the parser's zero padding, so checking all five is checking exactly
/// the written template.
pub(super) fn wide_field(row: &XlatRow, tag: i32) -> Option<String> {
    if !in_hexen_byte(row.zspecial) {
        return Some("its action number".to_owned());
    }
    row.args
        .iter()
        .enumerate()
        .find_map(|(slot, &value)| wide_slot(row, slot, value, tag))
}

/// One argument slot of `row` as the field a byte cannot hold: the slot the
/// line's `tag` fills is named for the tag, any other for its own value.
fn wide_slot(row: &XlatRow, slot: usize, value: i32, tag: i32) -> Option<String> {
    let tagged = row.tag_slot == Some(slot);
    let stored = if tagged { tag } else { value };
    (!in_hexen_byte(stored)).then(|| {
        if tagged {
            format!("its tag {stored} in arg{slot}")
        } else {
            format!("its arg{slot} {stored}")
        }
    })
}

/// The per-line warning for a special the table covers but a binary `Hexen`
/// record cannot store: the line, the `Doom` special, the action it
/// translates to and the value too wide for one byte, with the target that
/// would have kept it.
pub(super) fn too_wide_note(index: usize, old: i32, row: &XlatRow, field: &str) -> String {
    format!(
        "linedef {index} special {old} was set to 0: it translates to {} (ZDoom action {}), and \
         {field} is more than a binary Hexen field can store (one byte, 0 to {MAX_BYTE}); convert \
         to a UDMF Hexen-family map to keep the action",
        row.name, row.zspecial
    )
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
        bits |= REPEAT_SPECIAL;
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
/// with a warning naming the line. The layout step of a within-family
/// `crate::wad::map::convert` conversion to a binary `Hexen` target runs
/// this on every line with `byte_limited` set.
pub(crate) fn keep_line_id(
    line: &mut Linedef,
    index: usize,
    byte_limited: bool,
    warnings: &mut Vec<String>,
) {
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
