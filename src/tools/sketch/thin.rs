//! The thinning of `trace_sketch`'s two long lists: which gaps and marks
//! the report shows, and what it says about the rest.
//!
//! One [`Cut`] carries a list's noise floor, cap and `verbose` flag:
//! [`thin`] drops the candidates under the floor, ranks the rest by size
//! (stably, so equal sizes keep scan order) and cuts to the cap, counting
//! every row that stayed out. [`keep_gaps`] and [`keep_marks`] do that over
//! the analysis's [`Gap`]s and [`Mark`]s, measuring them in original pixels
//! through [`Frame`] so the floor reads the way the report does, and build
//! the rows only for what survives. [`omission_warnings`] phrases what a
//! list lost in the words of [`GAP_WORDS`] and [`MARK_WORDS`], and
//! [`thin_all`] runs both cuts for one traced picture.

use alloc::collections::BTreeSet;

use crate::error::CyberError;
use crate::sketch::{Analysis, Gap, Mark};

use super::report::{GapRow, MarkRow, gap_row, mark_row};
use super::request::{Frame, Request};

/// How one measurement list is thinned before it is reported.
pub(super) struct Cut {
    /// Smallest extent, in original pixels, worth a row.
    pub(super) floor: u32,
    /// The most rows to report.
    pub(super) cap: usize,
    /// Whether the floor and the cap are lifted.
    pub(super) verbose: bool,
}

impl Cut {
    /// The cut of one list capped at `cap` rows under `request`, with the
    /// floor the reported `stroke_px` implies.
    fn of(request: &Request, stroke_px: u32, cap: usize) -> Self {
        Self {
            floor: effective_floor(request, stroke_px),
            cap,
            verbose: request.verbose,
        }
    }
}

/// The noise floor actually applied: `0` under `verbose` (every row is
/// asked for), else the `noise_floor_px` the caller gave, else the measured
/// stroke width.
pub(super) fn effective_floor(request: &Request, stroke_px: u32) -> u32 {
    if request.verbose {
        return 0_u32;
    }
    request.noise_floor_px.unwrap_or(stroke_px)
}

/// One measurement with the two original-pixel numbers the thinning reads.
struct Candidate<Measurement> {
    /// Largest box edge: the extent the noise floor tests.
    extent: u32,
    /// The measure the ranking sorts by, a width for gaps and an ink area
    /// for marks.
    size: u32,
    /// The measurement itself.
    item: Measurement,
}

/// One list after the floor, the ranking and the cap.
pub(super) struct Kept<Measurement> {
    /// The reported measurements, largest first; a tie keeps scan order.
    pub(super) items: Vec<Measurement>,
    /// Measurements the analysis found, before the floor and the cap.
    pub(super) total: u32,
    /// Measurements dropped for sitting under the noise floor.
    pub(super) below_floor: u32,
    /// Measurements the cap left out.
    pub(super) past_cap: u32,
}

/// Drops the candidates under the floor, ranks the rest by size (the sort
/// is stable, so equal sizes keep scan order) and cuts to the cap.
fn thin<Measurement>(candidates: Vec<Candidate<Measurement>>, cut: &Cut) -> Kept<Measurement> {
    let total = candidates.len();
    let standing = reach_floor(candidates, cut);
    let below_floor = total.saturating_sub(standing.len());
    let mut ranked = rank(standing);
    let past_cap = capped_out(ranked.len(), cut);
    Kept {
        items: shown(&mut ranked, cut),
        total: as_rows(total),
        below_floor: as_rows(below_floor),
        past_cap: as_rows(past_cap),
    }
}

/// The candidates reaching the floor: all of them under `verbose`.
fn reach_floor<Measurement>(
    candidates: Vec<Candidate<Measurement>>,
    cut: &Cut,
) -> Vec<Candidate<Measurement>> {
    if cut.verbose {
        return candidates;
    }
    candidates
        .into_iter()
        .filter(|candidate| candidate.extent >= cut.floor)
        .collect()
}

/// The biggest `size` first, ties in the order they came in.
fn rank<Measurement>(mut candidates: Vec<Candidate<Measurement>>) -> Vec<Candidate<Measurement>> {
    candidates.sort_by_key(|candidate| core::cmp::Reverse(candidate.size));
    candidates
}

/// How many ranked rows the cap leaves out (`0` under `verbose`).
const fn capped_out(ranked: usize, cut: &Cut) -> usize {
    if cut.verbose {
        return 0_usize;
    }
    ranked.saturating_sub(cut.cap)
}

/// The items of the first `cap` ranked rows (every row under `verbose`).
fn shown<Measurement>(ranked: &mut Vec<Candidate<Measurement>>, cut: &Cut) -> Vec<Measurement> {
    if !cut.verbose {
        ranked.truncate(cut.cap);
    }
    core::mem::take(ranked)
        .into_iter()
        .map(|candidate| candidate.item)
        .collect()
}

/// A list length as the `u32` the report counts rows in.
fn as_rows(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// One thinned list with the report rows of the measurements it kept.
pub(super) struct Listing<Measurement, Row> {
    /// The measurements that reached the report, with their counts.
    pub(super) kept: Kept<Measurement>,
    /// The rows of `kept.items`, in the same order.
    pub(super) rows: Vec<Row>,
}

/// The gaps the report shows, widest first, with what stayed behind.
///
/// # Errors
///
/// [`Frame`]'s mapping refusal for a centre or box no crop and scale can
/// carry.
pub(super) fn keep_gaps(
    frame: &Frame,
    gaps: &[Gap],
    cut: &Cut,
) -> Result<Listing<Gap, GapRow>, CyberError> {
    let candidates = gaps
        .iter()
        .copied()
        .map(|gap| {
            let width = frame.length(gap.width_px);
            Candidate {
                extent: width,
                size: width,
                item: gap,
            }
        })
        .collect();
    let kept = thin(candidates, cut);
    let rows = kept
        .items
        .iter()
        .map(|gap| gap_row(frame, gap))
        .collect::<Result<Vec<GapRow>, CyberError>>()?;
    Ok(Listing { kept, rows })
}

/// The marks the report shows, most ink first, with what stayed behind.
///
/// # Errors
///
/// [`Frame`]'s mapping refusal for a centre or box no crop and scale can
/// carry.
pub(super) fn keep_marks(
    frame: &Frame,
    marks: &[Mark],
    cut: &Cut,
) -> Result<Listing<Mark, MarkRow>, CyberError> {
    let candidates = marks
        .iter()
        .copied()
        .map(|mark| Candidate {
            extent: frame.length(box_edge(mark.bbox)),
            size: frame.area(mark.area),
            item: mark,
        })
        .collect();
    let kept = thin(candidates, cut);
    let rows = kept
        .items
        .iter()
        .map(|mark| mark_row(frame, mark))
        .collect::<Result<Vec<MarkRow>, CyberError>>()?;
    Ok(Listing { kept, rows })
}

/// The larger edge of an inclusive analysis-space box.
pub(super) fn box_edge(bbox: (u32, u32, u32, u32)) -> u32 {
    let (x0, y0, x1, y1) = bbox;
    let width = x1.saturating_sub(x0).saturating_add(1);
    let height = y1.saturating_sub(y0).saturating_add(1);
    width.max(height)
}

/// How one list's omissions are phrased in a warning.
pub(super) struct ListWords {
    /// The list's plural noun: `gaps` or `marks`.
    noun: &'static str,
    /// The parameter that raises the cap.
    param: &'static str,
    /// The phrase for sitting under the floor, read before `the N px`.
    under: &'static str,
}

/// The words the gap list's omissions are warned in.
pub(super) const GAP_WORDS: ListWords = ListWords {
    noun: "gaps",
    param: "max_gaps",
    under: "narrower than",
};

/// The words the mark list's omissions are warned in.
pub(super) const MARK_WORDS: ListWords = ListWords {
    noun: "marks",
    param: "max_marks",
    under: "smaller than",
};

/// The warnings naming what one thinned list left out, `floor` and `cap`
/// the values that did the leaving out.
pub(super) fn omission_warnings<Measurement, Row>(
    listing: &Listing<Measurement, Row>,
    words: &ListWords,
    floor: u32,
    cap: usize,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let noun = words.noun;
    let below = listing.kept.below_floor;
    let under = words.under;
    if below > 0 {
        warnings.push(format!(
            "{below} {noun} {under} the {floor} px noise floor were dropped: pass \
             noise_floor_px 0 to report them"
        ));
    }
    let past = listing.kept.past_cap;
    let total = listing.kept.total;
    let param = words.param;
    if past > 0 {
        warnings.push(format!(
            "{past} of {total} {noun} stayed out past {param} {cap}: the largest come first, \
             raise {param} or pass verbose: true for every row"
        ));
    }
    warnings
}

/// Both lists thinned for the report, with the bookkeeping of what stayed
/// out and the warnings naming it.
pub(super) struct Thinned {
    /// The reported gaps, widest first.
    pub(super) gaps: Listing<Gap, GapRow>,
    /// The reported marks, most ink first.
    pub(super) marks: Listing<Mark, MarkRow>,
    /// The noise floor applied, original pixels (`0` drops nothing).
    pub(super) noise_floor_px: u32,
    /// Whether either cap left rows out.
    pub(super) truncated: bool,
    /// The warnings naming what stayed out of the two lists.
    pub(super) warnings: Vec<String>,
}

/// Thins both lists over the measured picture and folds the omissions into
/// warnings.
///
/// # Errors
///
/// The rows' mapping refusals, see [`keep_gaps`] and [`keep_marks`].
pub(super) fn thin_all(
    frame: &Frame,
    analysis: &Analysis,
    request: &Request,
) -> Result<Thinned, CyberError> {
    let stroke_px = frame.length(analysis.stroke);
    let gap_cut = Cut::of(request, stroke_px, request.max_gaps);
    let mark_cut = Cut::of(request, stroke_px, request.max_marks);
    let gaps = keep_gaps(frame, &analysis.gaps, &gap_cut)?;
    let marks = keep_marks(frame, &analysis.marks, &mark_cut)?;
    let floor = gap_cut.floor;
    let mut warnings = omission_warnings(&gaps, &GAP_WORDS, floor, request.max_gaps);
    warnings.extend(omission_warnings(
        &marks,
        &MARK_WORDS,
        floor,
        request.max_marks,
    ));
    Ok(Thinned {
        truncated: gaps.kept.past_cap > 0 || marks.kept.past_cap > 0,
        noise_floor_px: floor,
        gaps,
        marks,
        warnings,
    })
}

/// The labels of the reported gaps, for the regions' id lists.
pub(super) fn gap_labels(gaps: &[Gap]) -> BTreeSet<u32> {
    gaps.iter().map(|gap| gap.id).collect()
}

/// The labels of the reported marks, for the regions' id lists.
pub(super) fn mark_labels(marks: &[Mark]) -> BTreeSet<u32> {
    marks.iter().map(|mark| mark.id).collect()
}
