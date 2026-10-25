//! The noise pass: the ruled, speckled scan's bounded lists under the
//! documented size bound, the totals and warnings counting what stayed out,
//! the largest-first ranking, `noise_floor_px` as a knob of its own,
//! `verbose` reporting and labelling every row, a region listing only the
//! ids the report carries and the caps outside their range. The noisy
//! sheet's own builders and the row and warning readers live here.

use std::path::Path;

use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::sketch::{INK, bar, debug_image, paper, region, write_png};
use crate::support::{call, call_err, connect};

/// The noise fixture's sheet size, pixels.
const NOISE_WIDTH: u32 = 400;
/// Its height.
const NOISE_HEIGHT: u32 = 800;
/// The pen thickness of the noise fixture, pixels.
const NOISE_PEN: u32 = 6;
/// The broken scratch rows of its noise field.
const SCRATCH_ROWS: u32 = 18;
/// The broken bar pairs per scratch row.
const SCRATCH_UNITS: u32 = 4;
/// The dust speckle rows and columns, each speckle `3 x 3`.
const DUST_ROWS: u32 = 9;
/// The dust columns.
const DUST_COLUMNS: u32 = 16;
/// The blot rows and columns, each blot `8 x 8`.
const BLOT_ROWS: u32 = 2;
/// The blot columns.
const BLOT_COLUMNS: u32 = 30;
/// The default row cap both lists share.
const DEFAULT_CAP: u64 = 50;
/// The documented size bound of a default trace's JSON result.
const SIZE_BOUND: usize = 20_000;

/// Paints an exact pixel rectangle in ink.
fn ink_rect(img: &mut RgbaImage, rect: (u32, u32, u32, u32)) {
    bar(img, 1, rect, INK);
}

/// The three-room plan: a 6-px box at `20..380 x 20..140`, a middle
/// wall whose 14-px door break sits at rows `60..74`, and a divider in
/// the right room whose 9-px break sits at columns `260..269`.
fn plan_rooms(img: &mut RgbaImage) {
    ink_rect(img, (20, 20, 380, 26));
    ink_rect(img, (20, 134, 380, 140));
    ink_rect(img, (20, 20, 26, 140));
    ink_rect(img, (374, 20, 380, 140));
    ink_rect(img, (195, 20, 201, 60));
    ink_rect(img, (195, 74, 201, 134));
    ink_rect(img, (201, 90, 260, 96));
    ink_rect(img, (269, 90, 374, 96));
}

/// The `index`-th slot of a pitch-spaced run, from `start`.
const fn slot(index: u32, pitch: u32, start: u32) -> u32 {
    index.saturating_mul(pitch).saturating_add(start)
}

/// The scratch field: `SCRATCH_ROWS` ruled rows, `SCRATCH_UNITS` bar
/// pairs deep.
fn scratches(img: &mut RgbaImage) {
    for row in 0..SCRATCH_ROWS {
        scratch_row(img, slot(row, 30, 200));
    }
}

/// One ruled row at `y`: four pairs of `30 x 6` bars, each pair split
/// by the 9-px break the `close_gaps: 8` radius bridges.
fn scratch_row(img: &mut RgbaImage, y: u32) {
    let bottom = y.saturating_add(NOISE_PEN);
    for unit in 0..SCRATCH_UNITS {
        let x = slot(unit, 95, 20);
        let first_end = x.saturating_add(30);
        let second_start = first_end.saturating_add(9);
        let second_end = second_start.saturating_add(30);
        ink_rect(img, (x, y, first_end, bottom));
        ink_rect(img, (second_start, y, second_end, bottom));
    }
}

/// The dust: `3 x 3` speckles in the clear bands between the scratch
/// rows, every one under the measured stroke.
fn dust(img: &mut RgbaImage) {
    for row in 0..DUST_ROWS {
        dust_row(img, slot(row, 60, 216));
    }
}

/// One dust row at `y`: `DUST_COLUMNS` `3 x 3` speckles.
fn dust_row(img: &mut RgbaImage, y: u32) {
    let bottom = y.saturating_add(3);
    for column in 0..DUST_COLUMNS {
        let x = slot(column, 24, 10);
        ink_rect(img, (x, y, x.saturating_add(3), bottom));
    }
}

/// The blots: `8 x 8` speckles in the band under the field, every one
/// over the measured stroke, beside one `16 x 16` blob.
fn blots(img: &mut RgbaImage) {
    for row in 0..BLOT_ROWS {
        blot_row(img, slot(row, 18, 726));
    }
    ink_rect(img, (20, 770, 36, 786));
}

/// One blot row at `y`: `BLOT_COLUMNS` `8 x 8` speckles.
fn blot_row(img: &mut RgbaImage, y: u32) {
    let bottom = y.saturating_add(8);
    for column in 0..BLOT_COLUMNS {
        let x = slot(column, 13, 8);
        ink_rect(img, (x, y, x.saturating_add(8), bottom));
    }
}

/// The noisy scan: a three-room plan over a ruled, speckled field, the
/// photo-of-a-sketch mess the caps and the noise floor exist for. The
/// west room holds one `8 x 8` blot and one `3 x 3` speck of its own,
/// so a region's id lists can be checked against the report's.
fn noisy_scan() -> RgbaImage {
    let mut img = paper(NOISE_WIDTH, NOISE_HEIGHT);
    plan_rooms(&mut img);
    ink_rect(&mut img, (60, 40, 68, 48));
    ink_rect(&mut img, (140, 100, 143, 103));
    scratches(&mut img);
    dust(&mut img);
    blots(&mut img);
    img
}

/// The noise fixture's trace arguments: an 8-px radius bridges its
/// 9-px and 14-px breaks and nothing else, at a fixed ink level.
fn noise_args(source: &str) -> Value {
    json!({
        "source": source,
        "close_gaps": 8_i64,
        "ink": {"mode": "dark", "level": 128_u64},
    })
}

/// The noisy scan's trace, with `extra` merged over the standard
/// arguments.
async fn traced_noisy_with(dir: &Path, extra: Value) -> Value {
    let client = connect().await;
    let file = write_png(dir, "noisy.png", &noisy_scan());
    call(&client, "trace_sketch", noise_args_with(&file, &extra)).await
}

/// The noise arguments with `extra`'s fields merged in.
fn noise_args_with(source: &str, extra: &Value) -> Value {
    let mut args = noise_args(source);
    let base = args.as_object_mut().expect("an argument object");
    for (field, value) in extra.as_object().expect("an extra object") {
        base.insert(field.clone(), value.clone());
    }
    args
}

/// The ids of one report list.
fn row_ids(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| row["id"].as_str())
        .map(str::to_owned)
        .collect()
}

/// One list's rows as numbers read off `field`.
fn row_numbers(rows: &[Value], field: &str) -> Vec<u64> {
    rows.iter().filter_map(|row| row[field].as_u64()).collect()
}

/// The warning carrying `phrase`, of the ones the report answers.
fn warning_carrying(report: &Value, phrase: &str) -> String {
    report["warnings"]
        .as_array()
        .expect("a warning list")
        .iter()
        .filter_map(Value::as_str)
        .find(|text| text.contains(phrase))
        .unwrap_or_else(|| panic!("no warning names {phrase}: {}", report["warnings"]))
        .to_owned()
}

/// The leading count of a warning line.
fn leading_count(line: &str) -> u64 {
    line.split_whitespace()
        .next()
        .and_then(|head| head.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("the line counts its rows: {line}"))
}

/// Whether the sequence never rises.
fn non_increasing(numbers: &[u64]) -> bool {
    numbers
        .windows(2)
        .all(|pair| match (pair.first(), pair.get(1)) {
            (Some(high), Some(low)) => high >= low,
            _ => false,
        })
}

/// The exact `colour` pixels of a decoded picture.
fn count_pixels(img: &RgbaImage, colour: [u8; 4]) -> usize {
    (0..img.height)
        .flat_map(|row| (0..img.width).map(move |column| (column, row)))
        .filter(|&(column, row)| img.get(column, row) == Some(colour))
        .count()
}

/// The overlay's gap-disc colour.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
/// The overlay's mark-box colour.
const CYAN: [u8; 4] = [0, 255, 255, 255];

#[tokio::test]
async fn a_noisy_scan_stays_under_the_size_bound_with_bounded_lists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let report = traced_noisy_with(dir.path(), json!({})).await;
    let gaps = report["gaps"].as_array().expect("a gap list");
    let marks = report["marks"].as_array().expect("a mark list");
    assert_eq!(report["stroke_px"], json!(6_u64), "the pen measures 6 px");
    assert_eq!(
        report["noise_floor_px"], report["stroke_px"],
        "the default floor is the measured stroke"
    );
    assert_eq!(gaps.len(), 50_usize, "the default gap cap");
    assert_eq!(marks.len(), 50_usize, "and the default mark cap");
    assert_eq!(report["truncated"], json!(true), "rows stayed out");
    let text = serde_json::to_string(&report).expect("a serializable report");
    assert!(
        text.len() < SIZE_BOUND,
        "the result is {} characters, the bound is {SIZE_BOUND}",
        text.len()
    );
    let mapped = traced_noisy_with(
        dir.path(),
        json!({"units": {"scale_num": 4_i64, "flip_y": true}}),
    )
    .await;
    let mapped_text = serde_json::to_string(&mapped).expect("a serializable report");
    assert!(
        mapped_text.len() < SIZE_BOUND,
        "map coordinates cost {} characters, still under {SIZE_BOUND}",
        mapped_text.len()
    );
    assert_eq!(mapped["truncated"], json!(true), "and the same cut");
    assert!(
        !mapped["gaps"][0]["map_center"].is_null(),
        "the ranked rows keep their map centres: {}",
        mapped["gaps"][0]
    );
}

#[tokio::test]
async fn the_totals_count_everything_and_the_warnings_count_what_stayed_out() {
    let dir = tempfile::tempdir().expect("temp dir");
    let report = traced_noisy_with(dir.path(), json!({})).await;
    let gaps = report["gaps"].as_array().expect("a gap list");
    let gaps_total = report["gaps_total"].as_u64().expect("a gap total");
    let marks_total = report["marks_total"].as_u64().expect("a mark total");
    assert!(gaps_total > 70_u64, "every wall break counts: {gaps_total}");
    assert!(marks_total > 200_u64, "every speckle counts: {marks_total}");
    let gap_cap_line = warning_carrying(&report, "gaps stayed out past max_gaps");
    assert_eq!(
        leading_count(&gap_cap_line)
            .checked_add(DEFAULT_CAP)
            .expect("a test sum"),
        gaps_total,
        "the gap line counts what its cap left out: {gap_cap_line}"
    );
    assert!(
        !report["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|text| text
                .as_str()
                .is_some_and(|line| line.contains("gaps narrower"))),
        "no bridge is under the stroke floor here: {}",
        report["warnings"]
    );
    let mark_floor_line = warning_carrying(&report, "marks smaller than the 6 px noise floor");
    let mark_cap_line = warning_carrying(&report, "marks stayed out past max_marks");
    assert_eq!(
        leading_count(&mark_floor_line)
            .checked_add(DEFAULT_CAP)
            .and_then(|sum| sum.checked_add(leading_count(&mark_cap_line)))
            .expect("a test sum"),
        marks_total,
        "floor, shown and omitted make every mark"
    );
    assert!(
        mark_floor_line.contains("noise_floor_px 0") && mark_cap_line.contains("verbose: true"),
        "both lines name the parameter that changes it: {mark_floor_line} / {mark_cap_line}"
    );
    assert!(
        report["next_steps"]
            .as_array()
            .expect("steps")
            .iter()
            .any(|step| step.as_str().is_some_and(|text| text.contains("max_gaps"))),
        "the step list says so too: {}",
        report["next_steps"]
    );
    assert!(gaps.len() < usize::try_from(gaps_total).expect("a small total"));
}

#[tokio::test]
async fn the_capped_lists_come_back_largest_first() {
    let dir = tempfile::tempdir().expect("temp dir");
    let report =
        traced_noisy_with(dir.path(), json!({"max_gaps": 4_i64, "max_marks": 3_i64})).await;
    let gaps = report["gaps"].as_array().expect("a gap list");
    let marks = report["marks"].as_array().expect("a mark list");
    assert_eq!(gaps.len(), 4_usize, "four bridges");
    assert_eq!(marks.len(), 3_usize, "three specks");
    let widths = row_numbers(gaps, "width_px");
    assert!(non_increasing(&widths), "widest first: {widths:?}");
    assert_eq!(
        widths.first().copied(),
        Some(14_u64),
        "the door break outwidens every scratch"
    );
    assert_eq!(
        gaps.first().expect("the widest gap")["between"],
        json!(["R1", "R2"]),
        "the one break between real rooms leads the list"
    );
    let areas = row_numbers(marks, "area_px");
    assert!(non_increasing(&areas), "most ink first: {areas:?}");
    assert_eq!(areas.first().copied(), Some(256_u64), "the 16x16 blob");
    assert_eq!(
        marks.get(1).expect("a second mark")["id"],
        json!("M1"),
        "the room's own blot is the biggest of the equal-area specks"
    );
    assert_eq!(report["truncated"], json!(true), "both lists were cut");
}

#[tokio::test]
async fn the_noise_floor_is_a_parameter_of_its_own() {
    let dir = tempfile::tempdir().expect("temp dir");
    let kept = traced_noisy_with(dir.path(), json!({"noise_floor_px": 0_i64})).await;
    assert_eq!(kept["noise_floor_px"], json!(0_u64), "the applied floor");
    assert_eq!(
        kept["marks_total"],
        traced_noisy_with(dir.path(), json!({})).await["marks_total"],
        "the floor never changes what was measured"
    );
    assert!(
        !warning_carrying_all(&kept).contains("noise floor"),
        "a zero floor has nothing to explain: {}",
        kept["warnings"]
    );
    let strict = traced_noisy_with(
        dir.path(),
        json!({"noise_floor_px": 100_i64, "max_gaps": 5_i64}),
    )
    .await;
    assert_eq!(strict["gaps"], json!([]), "nothing reaches a 100 px floor");
    assert_eq!(strict["marks"], json!([]), "no mark either");
    assert_eq!(
        strict["truncated"],
        json!(false),
        "the floor, not the cap, chose"
    );
    let line = warning_carrying(&strict, "gaps narrower than the 100 px noise floor");
    assert_eq!(
        leading_count(line.as_str()),
        strict["gaps_total"].as_u64().expect("a total"),
        "and the warning says how many: {line}"
    );
}

/// Every warning of a report as one text.
fn warning_carrying_all(report: &Value) -> String {
    report["warnings"]
        .as_array()
        .expect("a warning list")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<&str>>()
        .join(" | ")
}

#[tokio::test]
async fn verbose_reports_every_row_and_labels_every_row() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "noisy.png", &noisy_scan());
    let capped = call(&client, "trace_sketch", noise_args(&file)).await;
    let every = call(
        &client,
        "trace_sketch",
        noise_args_with(&file, &json!({"verbose": true})),
    )
    .await;
    assert_eq!(
        every["noise_floor_px"],
        json!(0_u64),
        "verbose floors nothing"
    );
    assert_eq!(every["truncated"], json!(false), "and caps nothing");
    let gaps = every["gaps"].as_array().expect("a gap list");
    let marks = every["marks"].as_array().expect("a mark list");
    assert_eq!(
        gaps.len(),
        usize::try_from(every["gaps_total"].as_u64().expect("a total")).expect("a small total"),
        "every gap the analysis found"
    );
    assert_eq!(
        marks.len(),
        usize::try_from(every["marks_total"].as_u64().expect("a total")).expect("a small total"),
        "and every mark"
    );
    assert!(
        !warning_carrying_all(&every).contains("stayed out"),
        "a full report omits nothing: {}",
        every["warnings"]
    );
    let capped_overlay = debug_image(&capped).await;
    let loud_overlay = debug_image(&every).await;
    assert!(
        count_pixels(&capped_overlay, MAGENTA) < count_pixels(&loud_overlay, MAGENTA),
        "the capped overlay draws only the fifty discs it reported"
    );
    assert!(
        count_pixels(&capped_overlay, CYAN) < count_pixels(&loud_overlay, CYAN),
        "and only the fifty boxes it reported"
    );
}

#[tokio::test]
async fn a_region_lists_only_the_ids_the_report_carries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "noisy.png", &noisy_scan());
    let default = call(&client, "trace_sketch", noise_args(&file)).await;
    let gaps = default["gaps"].as_array().expect("a gap list");
    let marks = default["marks"].as_array().expect("a mark list");
    let shown_gaps = row_ids(gaps);
    let shown_marks = row_ids(marks);
    assert!(
        default["regions"]
            .as_array()
            .expect("a region list")
            .iter()
            .all(|room| listed(room, "gaps")
                .iter()
                .all(|id| shown_gaps.contains(id))
                && listed(room, "marks")
                    .iter()
                    .all(|id| shown_marks.contains(id))),
        "no region points at a row the report dropped: {}",
        default["regions"]
    );
    let room = region(&default, "R1");
    assert_eq!(
        listed(room, "marks"),
        vec!["M1".to_owned()],
        "the room's blot is listed, its sub-stroke speck is not"
    );
    assert_eq!(
        listed(room, "gaps"),
        vec!["G1".to_owned()],
        "the door break is the room's one reported bridge"
    );
    let loose = call(
        &client,
        "trace_sketch",
        noise_args_with(
            &file,
            &json!({"noise_floor_px": 0_i64, "max_marks": 500_i64}),
        ),
    )
    .await;
    let room_marks = listed(region(&loose, "R1"), "marks");
    assert_eq!(
        room_marks.len(),
        2_usize,
        "the floor off lists the speck too"
    );
    let shown_loose = row_ids(loose["marks"].as_array().expect("a mark list"));
    assert!(
        room_marks.iter().all(|id| shown_loose.contains(id)),
        "and every listed id comes back as a row: {room_marks:?} vs {}",
        shown_loose.len()
    );
}

/// One region row's id list.
fn listed(room: &Value, field: &str) -> Vec<String> {
    room[field]
        .as_array()
        .expect("an id list")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn caps_outside_their_range_are_invalid_arguments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "noisy.png", &noisy_scan());
    let mut zero = noise_args(&file);
    zero["max_gaps"] = json!(0_i64);
    let (gap_code, gap_message, _) = call_err(&client, "trace_sketch", zero).await;
    assert_eq!(gap_code, "invalid_argument", "max_gaps 0 is out of range");
    assert!(
        gap_message.contains("max_gaps") && gap_message.contains("verbose"),
        "the refusal names the parameter and the way out: {gap_message}"
    );
    let mut wide = noise_args(&file);
    wide["max_marks"] = json!(501_i64);
    let (mark_code, mark_message, _) = call_err(&client, "trace_sketch", wide).await;
    assert_eq!(mark_code, "invalid_argument", "max_marks above the ceiling");
    assert!(
        mark_message.contains("max_marks") && mark_message.contains("between 1 and 500"),
        "the refusal names the parameter and its range: {mark_message}"
    );
}
