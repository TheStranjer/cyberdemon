//! The reverse linedef stage of the cross-family conversion:
//! [`linedefs_from_hexen`] folds a binary `Hexen` line id back onto the
//! model's tag field, searches the [`XlatRow`](super::XlatRow) rows of a
//! line's action with [`best_match`], [`row_matches`] and [`row_arguments`]
//! and writes the winning row's `Doom` special with the `tag`-slot argument
//! as the trigger tag.
//!
//! A line no row accepts clears to 0 and joins
//! [`LinesReport::untranslated`](super::LinesReport::untranslated); an
//! activation field the table never writes matches nothing; a `UDMF` line id
//! whose special translated has nowhere to ride on drops through
//! [`note_busy_id`]; and the `Hexen`-only blocking bits drop as the pass
//! masks every line down to [`DOOM_FLAG_BITS`].
//! [`linedefs_to_hexen`](super::linedefs_to_hexen) is the forward half and
//! [`expected_field`] the activation field both directions agree on.

use crate::error::CyberError;
use crate::wad::map::flags::{ACTIVATION_MASK, ACTIVATION_SHIFT, MONSTER_ACTIVATE};
use crate::wad::map::model::{Linedef, Map};

use super::lines_to::{
    DOOM_FLAG_BITS, HEXEN_BLOCK_BITS, HEXEN_REPEAT, SET_IDENTIFICATION, linedef_warning,
};
use super::table::{MAX_ARGS, XlatFlags, XlatRow, rows_for_zspecial};
use super::{LinesReport, Untranslated};

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
/// [`xlat_rows`](super::xlat_rows)' [`CyberError::Internal`] for malformed
/// embedded data.
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
pub(super) fn best_match(rows: &[&XlatRow], line: &Linedef) -> Option<(usize, i32)> {
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
pub(super) const fn expected_field(row: &XlatRow) -> u32 {
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
