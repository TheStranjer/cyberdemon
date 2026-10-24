//! The thinning of the two long lists: the floor the caller asks for or the
//! measured stroke implies, the ranking and caps, the counts and warnings
//! naming what stayed out, and the original-pixel reading both lists share.

use crate::sketch::{Gap, Mark};
use crate::tools::sketch::TraceSketchParams;
use crate::tools::sketch::report::{GapRow, MarkRow};
use crate::tools::sketch::request::parse_request;
use crate::tools::sketch::thin::{
    Cut, GAP_WORDS, Listing, MARK_WORDS, box_edge, effective_floor, keep_gaps, keep_marks,
    omission_warnings,
};

use super::{args, frame};

/// A cut with the asked cap and floor, thinning on.
fn cut(cap: usize, floor: u32) -> Cut {
    Cut {
        floor,
        cap,
        verbose: false,
    }
}

/// Three bridges: widths `10`, `4` and `20` in scan order, boxes to
/// match.
fn gaps() -> Vec<Gap> {
    [
        (1_u32, 10_u32, (4_u32, 4_u32, 13_u32, 9_u32)),
        (2_u32, 4_u32, (20_u32, 4_u32, 23_u32, 7_u32)),
        (3_u32, 20_u32, (30_u32, 4_u32, 49_u32, 23_u32)),
    ]
    .into_iter()
    .map(|(id, width_px, bbox)| Gap {
        id,
        centroid: (5_u32, 6_u32),
        bbox,
        width_px,
        between: Some((1_u32, 2_u32)),
    })
    .collect()
}

/// Three specks: areas `9`, `64` and `25` in scan order, box edges
/// `3`, `8` and `5`.
fn marks() -> Vec<Mark> {
    [
        (1_u32, 9_u32, (4_u32, 4_u32, 6_u32, 6_u32)),
        (2_u32, 64_u32, (10_u32, 10_u32, 17_u32, 17_u32)),
        (3_u32, 25_u32, (20_u32, 20_u32, 24_u32, 24_u32)),
    ]
    .into_iter()
    .map(|(id, area, bbox)| Mark {
        id,
        centroid: (5_u32, 6_u32),
        bbox,
        area,
        color: [0_u8, 0_u8, 0_u8],
        region: Some(1_u32),
    })
    .collect()
}

/// The gap rows' ids, in report order.
fn gap_ids(listing: &Listing<Gap, GapRow>) -> Vec<String> {
    listing.rows.iter().map(|row| row.id.clone()).collect()
}

/// The mark rows' ids, in report order.
fn mark_ids(listing: &Listing<Mark, MarkRow>) -> Vec<String> {
    listing.rows.iter().map(|row| row.id.clone()).collect()
}

#[test]
fn the_floor_asks_the_stroke_and_explicit_values_win() {
    let asked = TraceSketchParams {
        noise_floor_px: Some(3_u32),
        ..args("/x.png")
    };
    let request = parse_request(&asked).expect("the wire parses");
    assert_eq!(request.max_gaps, 50_usize, "fifty gaps by default");
    assert_eq!(request.max_marks, 50_usize, "and fifty marks");
    assert_eq!(
        effective_floor(&request, 7_u32),
        3_u32,
        "the asked floor beats the stroke"
    );
    let plain = parse_request(&args("/x.png")).expect("the bare wire parses");
    assert_eq!(
        effective_floor(&plain, 7_u32),
        7_u32,
        "unset asks for the measured stroke"
    );
    let off = TraceSketchParams {
        noise_floor_px: Some(0_u32),
        ..args("/x.png")
    };
    let kept = parse_request(&off).expect("an explicit zero");
    assert_eq!(effective_floor(&kept, 7_u32), 0_u32, "zero keeps every row");
}

#[test]
fn verbose_lifts_the_floor_and_both_caps() {
    let loud = TraceSketchParams {
        verbose: true,
        ..args("/x.png")
    };
    let request = parse_request(&loud).expect("the verbose wire parses");
    let plain = frame(1_u32, 1_u32);
    assert_eq!(
        effective_floor(&request, 7_u32),
        0_u32,
        "verbose drops nothing"
    );
    let every = Cut {
        floor: 40_u32,
        cap: 1_usize,
        verbose: true,
    };
    assert_eq!(
        gap_ids(&keep_gaps(&plain, &gaps(), &every).expect("in range")),
        vec!["G3".to_owned(), "G1".to_owned(), "G2".to_owned()],
        "every row stands, ranked by width"
    );
    assert_eq!(
        mark_ids(&keep_marks(&plain, &marks(), &every).expect("in range")),
        vec!["M2".to_owned(), "M3".to_owned(), "M1".to_owned()],
        "marks too, by ink"
    );
}

#[test]
fn a_capped_gap_list_keeps_the_widest_and_counts_the_rest() {
    let plain = frame(1_u32, 1_u32);
    let listing = keep_gaps(&plain, &gaps(), &cut(2_usize, 0_u32)).expect("in range");
    assert_eq!(
        gap_ids(&listing),
        vec!["G3".to_owned(), "G1".to_owned()],
        "widest first, the 4-px bridge last"
    );
    assert_eq!(listing.kept.total, 3_u32, "the analysis found three");
    assert_eq!(listing.kept.below_floor, 0_u32, "floor 0 drops nothing");
    assert_eq!(listing.kept.past_cap, 1_u32, "one stayed out");
}

#[test]
fn the_noise_floor_drops_the_short_bridge_and_the_tiny_speck() {
    let plain = frame(1_u32, 1_u32);
    let listing = keep_gaps(&plain, &gaps(), &cut(5_usize, 5_u32)).expect("in range");
    assert_eq!(
        gap_ids(&listing),
        vec!["G3".to_owned(), "G1".to_owned()],
        "the 4-px bridge is under the 5-px floor"
    );
    assert_eq!(listing.kept.below_floor, 1_u32, "and is counted");
    assert_eq!(listing.kept.past_cap, 0_u32, "what stands fits the cap");
    let specks = keep_marks(&plain, &marks(), &cut(5_usize, 4_u32)).expect("in range");
    assert_eq!(
        mark_ids(&specks),
        vec!["M2".to_owned(), "M3".to_owned()],
        "most ink first, the 3-px speck dropped"
    );
    assert_eq!(specks.kept.below_floor, 1_u32, "the speck is counted out");
    assert_eq!(
        specks.rows.first().map(|row| row.area_px),
        Some(64_u32),
        "the ranking row is the 64-px blot"
    );
}

#[test]
fn the_floor_and_the_ranking_read_original_pixels() {
    let shrunk = frame(1_u32, 4_u32);
    let listing = keep_gaps(&shrunk, &gaps(), &cut(5_usize, 50_u32)).expect("in range");
    assert_eq!(
        gap_ids(&listing),
        vec!["G3".to_owned()],
        "the 20-px bridge is 80 original pixels"
    );
    assert_eq!(
        listing.kept.below_floor, 2_u32,
        "the other two are under it"
    );
}

#[test]
fn the_omission_warnings_name_the_counts_and_the_parameters() {
    let plain = frame(1_u32, 1_u32);
    let gaps_cut = keep_gaps(&plain, &gaps(), &cut(1_usize, 5_u32)).expect("in range");
    let gap_lines = omission_warnings(&gaps_cut, &GAP_WORDS, 5_u32, 1_usize);
    assert_eq!(gap_lines.len(), 2_usize, "one line per reason");
    assert!(
        gap_lines[0].starts_with("1 gaps narrower than the 5 px noise floor")
            && gap_lines[0].contains("noise_floor_px 0"),
        "the floor line: {:?}",
        gap_lines[0]
    );
    assert!(
        gap_lines[1].starts_with("1 of 3 gaps stayed out past max_gaps 1")
            && gap_lines[1].contains("verbose: true"),
        "the cap line: {:?}",
        gap_lines[1]
    );
    let marks_cut = keep_marks(&plain, &marks(), &cut(1_usize, 4_u32)).expect("in range");
    let mark_lines = omission_warnings(&marks_cut, &MARK_WORDS, 4_u32, 1_usize);
    assert_eq!(mark_lines.len(), 2_usize, "the 3-px speck and one blot");
    assert!(
        mark_lines[0].starts_with("1 marks smaller than the 4 px noise floor"),
        "the mark floor line: {:?}",
        mark_lines[0]
    );
    assert!(
        mark_lines[1].starts_with("1 of 3 marks stayed out past max_marks 1"),
        "the mark line names its own parameter: {:?}",
        mark_lines[1]
    );
    let whole = keep_gaps(&plain, &gaps(), &cut(5_usize, 0_u32)).expect("in range");
    assert!(
        omission_warnings(&whole, &GAP_WORDS, 0_u32, 5_usize).is_empty(),
        "a list that loses nothing warns nothing"
    );
}

#[test]
fn box_edges_measure_the_longer_side() {
    assert_eq!(box_edge((4_u32, 4_u32, 6_u32, 6_u32)), 3_u32, "a 3x3 box");
    assert_eq!(
        box_edge((0_u32, 0_u32, 20_u32, 4_u32)),
        21_u32,
        "the wider edge wins"
    );
}
