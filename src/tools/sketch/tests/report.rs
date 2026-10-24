//! The rows the report answers with: the region, gap and mark rows' tags
//! and scaled geometry, the ids a region drops when a list thinned them
//! out, and the next step a thinned report adds.

use alloc::collections::BTreeSet;

use crate::sketch::vectorize::RegionShape;
use crate::sketch::{Gap, Mark};
use crate::tools::sketch::report::{gap_row, mark_row, next_steps, region_row, tag};

use super::frame;

/// An identity-region fixture for the row builders.
fn shape() -> RegionShape {
    RegionShape {
        id: 2,
        points: vec![
            (10_i64, 10_i64),
            (90_i64, 10_i64),
            (90_i64, 90_i64),
            (10_i64, 90_i64),
        ],
        area_px: 6_400_u32,
        bbox: (10_u32, 10_u32, 89_u32, 89_u32),
        centroid: (50_u32, 50_u32),
        is_rectangle: true,
        holes: 0_u32,
        marks: vec![1_u32],
        gaps: vec![3_u32],
    }
}

#[test]
fn region_rows_carry_tags_and_the_scaled_geometry() {
    let shrunk = frame(1_u32, 2_u32);
    let shown: BTreeSet<u32> = [1_u32, 3_u32].into_iter().collect();
    let row = region_row(&shrunk, &shape(), &shown, &shown).expect("in range");
    assert_eq!(row.id, "R2", "the label id is prefixed");
    assert_eq!(row.points.first().copied(), Some((20_i64, 20_i64)));
    assert_eq!(
        row.bbox,
        (20_i64, 20_i64, 179_i64, 179_i64),
        "inclusive edges"
    );
    assert_eq!(row.area_px, 25_600_u32, "the area squares the scale");
    assert_eq!(row.centroid, (100_i64, 100_i64));
    assert_eq!(row.marks, vec!["M1".to_owned()], "mark ids");
    assert_eq!(row.gaps, vec!["G3".to_owned()], "gap ids");
    assert!(row.is_rectangle && row.holes == 0, "flags ride through");
}

#[test]
fn a_region_drops_the_ids_the_report_does_not_show() {
    let plain = frame(1_u32, 1_u32);
    let shown: BTreeSet<u32> = core::iter::once(9_u32).collect();
    let row = region_row(&plain, &shape(), &shown, &shown).expect("in range");
    let empty: Vec<String> = Vec::new();
    assert_eq!(row.gaps, empty, "gap 3 stayed out of the report");
    assert_eq!(row.marks, empty, "and so did mark 1");
}

#[test]
fn gap_and_mark_rows_scale_centres_and_colours() {
    let shrunk = frame(1_u32, 2_u32);
    let gap = Gap {
        id: 1_u32,
        centroid: (30_u32, 60_u32),
        bbox: (28_u32, 58_u32, 32_u32, 62_u32),
        width_px: 15_u32,
        between: Some((2_u32, 3_u32)),
    };
    let gap_row_data = gap_row(&shrunk, &gap).expect("in range");
    assert_eq!(gap_row_data.id, "G1");
    assert_eq!(gap_row_data.center, (60_i64, 120_i64));
    assert_eq!(gap_row_data.width_px, 30_u32);
    assert_eq!(
        gap_row_data.between,
        Some(["R2".to_owned(), "R3".to_owned()]),
        "the pair is labelled"
    );
    let mark = Mark {
        id: 2_u32,
        centroid: (10_u32, 20_u32),
        bbox: (8_u32, 18_u32, 12_u32, 22_u32),
        area: 25_u32,
        color: [192, 32, 32],
        region: Some(1_u32),
    };
    let mark_row_data = mark_row(&shrunk, &mark).expect("in range");
    assert_eq!(mark_row_data.id, "M2");
    assert_eq!(mark_row_data.color, "#C02020", "upper-case hex");
    assert_eq!(mark_row_data.area_px, 100_u32);
    assert_eq!(mark_row_data.region, Some("R1".to_owned()));
    assert_eq!(tag('R', 7_u32), "R7");
}

#[test]
fn a_thinned_report_adds_the_step_naming_the_knobs() {
    assert_eq!(next_steps(false).len(), 4_usize, "the standing four");
    let steps = next_steps(true);
    assert_eq!(steps.len(), 5_usize, "the thinning adds one");
    assert!(
        steps.last().is_some_and(|text| text.contains("max_gaps")),
        "the extra step names the knobs: {steps:?}"
    );
}
