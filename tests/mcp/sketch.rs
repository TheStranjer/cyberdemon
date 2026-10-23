//! The `sketch` tool group as seen by a real MCP client: `trace_sketch`
//! measures a synthetic three-room floor plan (an L-shaped room and two
//! stacked rectangles sharing a broken wall, a red dot and a black X) into
//! three regions with welded shared coordinates, one bridged gap between
//! the stacked rooms, two coloured marks, adjacency, and a decodable
//! debug `PNG`; the downscale path reports original-image pixels at 6×
//! the small sketch's coordinates; `units` add grid-snapped `map_points`
//! with a mirrored `y`; a `crop` leaves coordinates in original-image
//! space; `ink.mode: "color"` traces only the matching pen; a blank sheet
//! answers empty lists plus a warning and the debug image instead of an
//! error; a bad `crop`, an out-of-range `max_size`, `color` without a pen,
//! an out-of-range `max_gaps`/`max_marks` and non-image bytes report their
//! documented codes; and `plan_rooms` with a connection at the gap centre
//! build through `build_map_from_plan` into an error-free map.
//!
//! Plus CD-180's noise pass: a ruled, speckled scan of a three-room plan
//! answers at most 50 gap and 50 mark rows, largest first, under the
//! documented 20k-character bound, with `gaps_total`/`marks_total`,
//! `truncated`, warnings naming every omitted count with the parameter that
//! lifts it, region id lists and a debug overlay that cover exactly the
//! reported rows, `noise_floor_px` and `verbose` as the knobs, and the
//! sub-stroke dust dropped by the stroke-wide floor on its own.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::codec::{decode_image, encode_png};
    use cyberdemon::gfx::image::RgbaImage;
    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{call, call_err, connect, http_get};

    /// The opaque paper white of the fixtures.
    const PAPER: [u8; 4] = [255, 255, 255, 255];
    /// The opaque black pen of the fixtures.
    const INK: [u8; 4] = [0, 0, 0, 255];
    /// The opaque red pen of the fixtures.
    const RED: [u8; 4] = [255, 0, 0, 255];

    /// A blank `width` x `height` sheet.
    fn paper(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width.checked_mul(height).expect("test size"))
            .expect("test pixels fit")
            .checked_mul(4)
            .expect("test bytes fit");
        RgbaImage::new(width, height, vec![255_u8; count]).expect("fixture")
    }

    /// Paints the half-open rectangle `(x0, y0)..(x1, y1)` scaled by `k`.
    fn bar(img: &mut RgbaImage, k: u32, rect: (u32, u32, u32, u32), colour: [u8; 4]) {
        let (x0, y0, x1, y1) = rect;
        let points = (y0.checked_mul(k).expect("test span")..y1.checked_mul(k).expect("test span"))
            .flat_map(|y| {
                (x0.checked_mul(k).expect("test span")..x1.checked_mul(k).expect("test span"))
                    .map(move |x| (x, y))
            });
        for (column, row) in points {
            assert!(img.set(column, row, colour), "fixture in bounds");
        }
    }

    /// Draws an `X` centred on `(spec.0, spec.1)` with `spec.2`-long
    /// diagonals; every ray point takes a `k`-wide pen dab, so the stroke
    /// stays continuous at any scale.
    fn cross(img: &mut RgbaImage, k: u32, spec: (u32, u32, u32), colour: [u8; 4]) {
        let (cx, cy, arm) = spec;
        let dabs = (0..=arm).flat_map(|d| {
            let px = cx.checked_add(d).expect("test span");
            let mx = cx.checked_sub(d).expect("test span");
            let py = cy.checked_add(d).expect("test span");
            let my = cy.checked_sub(d).expect("test span");
            [(px, py), (px, my), (mx, py), (mx, my)]
        });
        for (x, y) in dabs {
            let far_x = x.checked_add(1).expect("test span");
            let far_y = y.checked_add(1).expect("test span");
            bar(img, k, (x, y, far_x, far_y), colour);
        }
    }

    /// The three-room sketch drawn `k` times as large: an L-shaped room
    /// with 4*k-px walls at `(20, 20)..(153, 163)`, two stacked rectangles
    /// at `x 240..360` sharing the wall row `157..160` with a 30-px break
    /// at `x 281..310`, a 9x9 red dot in the L, and a black `X` in the
    /// upper rectangle.
    fn sketch(k: u32) -> RgbaImage {
        let mut img = paper(
            k.checked_mul(400).expect("test size"),
            k.checked_mul(300).expect("test size"),
        );
        bar(&mut img, k, (20, 20, 24, 164), INK);
        bar(&mut img, k, (20, 20, 94, 24), INK);
        bar(&mut img, k, (90, 20, 94, 94), INK);
        bar(&mut img, k, (90, 90, 154, 94), INK);
        bar(&mut img, k, (150, 90, 154, 164), INK);
        bar(&mut img, k, (20, 160, 154, 164), INK);
        bar(&mut img, k, (240, 20, 244, 281), INK);
        bar(&mut img, k, (357, 20, 361, 281), INK);
        bar(&mut img, k, (240, 20, 361, 24), INK);
        bar(&mut img, k, (240, 277, 361, 281), INK);
        bar(&mut img, k, (240, 157, 281, 161), INK);
        bar(&mut img, k, (311, 157, 361, 161), INK);
        bar(&mut img, k, (50, 100, 59, 109), RED);
        cross(&mut img, k, (300, 90, 5), INK);
        img
    }

    /// Two closed rectangles, one drawn in red and one in black.
    fn two_pens() -> RgbaImage {
        let mut img = paper(250, 120);
        bar(&mut img, 1, (30, 30, 101, 34), RED);
        bar(&mut img, 1, (30, 86, 101, 90), RED);
        bar(&mut img, 1, (30, 30, 34, 90), RED);
        bar(&mut img, 1, (97, 30, 101, 90), RED);
        bar(&mut img, 1, (150, 30, 221, 34), INK);
        bar(&mut img, 1, (150, 86, 221, 90), INK);
        bar(&mut img, 1, (150, 30, 154, 90), INK);
        bar(&mut img, 1, (217, 30, 221, 90), INK);
        img
    }

    /// Encodes and writes one test picture, answering its path text.
    fn write_png(dir: &Path, name: &str, img: &RgbaImage) -> String {
        let png = encode_png(img, None).expect("test PNGs encode");
        let path = dir.join(name);
        std::fs::write(&path, png).expect("write the test sketch");
        path.display().to_string()
    }

    /// The standard trace arguments over `source` (close the 30-px door).
    fn trace_args(source: &str) -> Value {
        json!({"source": source, "close_gaps": 16_i64})
    }

    /// Fetches a published asset's bytes over `HTTP`.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// The debug asset of a trace report, decoded.
    async fn debug_image(report: &Value) -> RgbaImage {
        let url = report["debug"]["url"].as_str().expect("a debug url");
        decode_image(&fetch_asset(url).await).expect("the debug overlay is a PNG")
    }

    /// The `(x, y)` points of one `regions`/`plan_rooms` row.
    fn points_of(row: &Value) -> Vec<(i64, i64)> {
        row["points"]
            .as_array()
            .expect("a point list")
            .iter()
            .map(|point| {
                let pair = point.as_array().expect("an [x, y] pair");
                (
                    pair.first().and_then(Value::as_i64).expect("an x"),
                    pair.get(1).and_then(Value::as_i64).expect("a y"),
                )
            })
            .collect()
    }

    /// The region rows of a report by id.
    fn region<'res>(report: &'res Value, id: &str) -> &'res Value {
        report["regions"]
            .as_array()
            .expect("a region list")
            .iter()
            .find(|row| row["id"] == json!(id))
            .unwrap_or_else(|| panic!("no region {id} in {}", report["regions"]))
    }

    /// One `regions` row must be an axis box of four points, hole-free.
    fn assert_rectangle(report: &Value, id: &str) {
        let row = region(report, id);
        assert!(row["is_rectangle"].as_bool().expect("a flag"), "{id}");
        assert_eq!(points_of(row).len(), 4, "{id} is a rectangle");
        assert_eq!(row["holes"], json!(0_u64), "{id} holds no island");
    }

    /// Each `#RRGGBB` channel of `color` within 40 of the wanted value.
    fn assert_channels_close(color: &str, want: [u64; 3]) {
        let got = hex_channels(color);
        for (one, two) in want.into_iter().zip(got.clone()) {
            let drift = one.max(two).saturating_sub(one.min(two));
            assert!(
                drift <= 40,
                "{color} must be within 40 of {want:?}, got {got:?}"
            );
        }
    }

    /// One traced report row must be the `other` row's points at six
    /// times the size, within twelve pixels.
    fn assert_points_at_six_times(small_row: &Value, big_row: &Value) {
        assert_eq!(small_row["id"], big_row["id"], "scan order stands");
        let mut small_points = points_of(small_row);
        let mut big_points = points_of(big_row);
        small_points.sort_unstable();
        big_points.sort_unstable();
        assert_eq!(small_points.len(), big_points.len(), "same corner count");
        let wanted: Vec<(i64, i64)> = small_points
            .iter()
            .map(|&(x, y)| {
                (
                    x.checked_mul(6).expect("test product"),
                    y.checked_mul(6).expect("test product"),
                )
            })
            .collect();
        for (one, many) in wanted.iter().zip(big_points.iter()) {
            let drift_x = many.0.checked_sub(one.0).expect("test range").abs();
            let drift_y = many.1.checked_sub(one.1).expect("test range").abs();
            assert!(drift_x <= 12 && drift_y <= 12, "{one:?} at 6x is {many:?}");
        }
    }

    /// Pairs the small and big `regions` lists and compares each.
    fn assert_scaled_regions(small: &[Value], big: &[Value]) {
        assert_eq!(big.len(), small.len(), "the same rooms");
        for (small_row, big_row) in small.iter().zip(big.iter()) {
            assert_points_at_six_times(small_row, big_row);
        }
    }

    /// A region row's `map_points` are all on the 8-grid, east of and
    /// below the origin (the mirror of image space).
    fn assert_map_grid(row: &Value) {
        let mapped = row["map_points"].as_array().expect("map points for an id");
        assert_eq!(mapped.len(), points_of(row).len(), "one map point each");
        for point in mapped {
            let pair = point.as_array().expect("a pair");
            let x = pair.first().and_then(Value::as_i64).expect("an x");
            let y = pair.get(1).and_then(Value::as_i64).expect("a y");
            assert_eq!(x.checked_rem(8).expect("grid"), 0, "x snaps to 8: {x}");
            assert_eq!(y.checked_rem(8).expect("grid"), 0, "y snaps to 8: {y}");
            assert!(x >= 0, "the sketch is east of the origin: {x}");
            assert!(y <= 0, "flip_y mirrors the rows below zero: {y}");
        }
    }

    /// Every region row carries grid-snapped, mirrored `map_points`.
    fn assert_all_map_grids(report: &Value) {
        let regions = report["regions"].as_array().expect("regions");
        for row in regions {
            assert_map_grid(row);
        }
    }

    /// The report without its `debug` asset (whose url differs per run).
    fn strip_debug(mut report: Value) -> Value {
        report
            .as_object_mut()
            .expect("an object")
            .remove("debug")
            .expect("the debug field");
        report
    }

    /// The largest `x` of a point list.
    fn max_x(points: &[(i64, i64)]) -> i64 {
        points.iter().map(|point| point.0).max().expect("a point")
    }

    /// The largest `y` of a point list.
    fn max_y(points: &[(i64, i64)]) -> i64 {
        points.iter().map(|point| point.1).max().expect("a point")
    }

    /// The smallest `y` of a point list.
    fn min_y(points: &[(i64, i64)]) -> i64 {
        points.iter().map(|point| point.1).min().expect("a point")
    }

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
            usize::try_from(every["marks_total"].as_u64().expect("a total"))
                .expect("a small total"),
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

    #[tokio::test]
    async fn three_rooms_trace_to_regions_a_gap_two_marks_and_a_debug_png() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        assert_eq!(report["image"]["width"], json!(400_u64), "the source sheet");
        assert_eq!(report["image"]["height"], json!(300_u64));
        assert_eq!(
            report["image"]["analysed_scale"],
            json!({"num": 1_u64, "den": 1_u64}),
            "a small sheet is analysed at full size"
        );
        assert_eq!(report["stroke_px"], json!(4_u64), "the pen measures 4 px");
        assert!(
            report["threshold"].is_number(),
            "the detected level is reported: {}",
            report["threshold"]
        );
        assert_eq!(report["close_gaps_px"], json!(16_u64), "the asked radius");
        let regions = report["regions"].as_array().expect("a region list");
        assert_eq!(regions.len(), 3, "three enclosed areas");
        assert_eq!(report["warnings"], json!([]), "a clean sketch is silent");
        assert_eq!(report["next_steps"].as_array().expect("steps").len(), 4);
    }

    #[tokio::test]
    async fn the_l_room_keeps_six_points_and_the_rectangles_four() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        let l_shape = points_of(region(&report, "R1"));
        assert_eq!(l_shape.len(), 6, "the L ring has six corners");
        assert!(
            !region(&report, "R1")["is_rectangle"]
                .as_bool()
                .expect("a flag")
        );
        assert_rectangle(&report, "R2");
        assert_rectangle(&report, "R3");
        assert_eq!(
            max_y(&l_shape),
            164,
            "coordinates are original-image pixels, in pixel-corner space"
        );
    }

    #[tokio::test]
    async fn the_shared_wall_carries_one_coordinate_and_one_adjacency() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        let upper = points_of(region(&report, "R2"));
        let lower = points_of(region(&report, "R3"));
        assert_eq!(
            max_y(&upper),
            min_y(&lower),
            "the shared wall is one y for both regions"
        );
        let adjacency = report["adjacency"].as_array().expect("an adjacency list");
        assert_eq!(adjacency.len(), 1, "only the stacked rooms touch");
        let pair = adjacency.first().expect("the pair");
        assert_eq!(pair["a"], json!("R2"), "the lower label first");
        assert_eq!(pair["b"], json!("R3"));
        assert!(
            pair["shared_px"].as_u64().is_some_and(|span| span > 0),
            "the shared border is measured: {pair}"
        );
    }

    #[tokio::test]
    async fn the_door_break_is_one_gap_between_the_stacked_rooms() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        let gaps = report["gaps"].as_array().expect("a gap list");
        assert_eq!(gaps.len(), 1, "exactly the door break was bridged");
        let door = gaps.first().expect("the door row");
        assert_eq!(door["id"], json!("G1"), "scan-order labels");
        assert_eq!(
            door["between"],
            json!(["R2", "R3"]),
            "the pair it separates"
        );
        assert!(
            (24_u64..=36_u64).contains(&door["width_px"].as_u64().expect("a width")),
            "the 30-px break measures about 30, got {}",
            door["width_px"]
        );
        assert_eq!(
            region(&report, "R2")["gaps"],
            json!(["G1"]),
            "listed on both rooms"
        );
        assert_eq!(region(&report, "R3")["gaps"], json!(["G1"]));
        assert_eq!(
            region(&report, "R1")["gaps"],
            json!([]),
            "the L holds no break"
        );
    }

    #[tokio::test]
    async fn the_dot_and_the_x_come_back_as_colour_and_region_tagged_marks() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        let marks = report["marks"].as_array().expect("a mark list");
        assert_eq!(marks.len(), 2, "the dot and the X: {marks:?}");
        let red = marks
            .iter()
            .find(|mark| mark["color"] == json!("#FF0000"))
            .expect("the red mark");
        let black = marks
            .iter()
            .find(|mark| mark["color"] == json!("#000000"))
            .expect("the black mark");
        assert_eq!(red["region"], json!("R1"), "the dot sits in the L");
        assert_eq!(black["region"], json!("R2"), "the X sits in the upper room");
        assert!(
            (70_u64..=95_u64).contains(&red["area_px"].as_u64().expect("an area")),
            "the 9x9 dot covers about 81 px, got {}",
            red["area_px"]
        );
        let r1_marks = region(&report, "R1")["marks"].as_array().expect("a list");
        assert!(r1_marks.contains(&red["id"]), "the room lists its mark");
        assert_channels_close(red["color"].as_str().expect("hex"), [255_u64, 0_u64, 0_u64]);
    }

    /// The three `#RRGGBB` channels of a report colour as numbers.
    fn hex_channels(text: &str) -> Vec<u64> {
        let digits = text.strip_prefix('#').expect("a hex colour");
        let mut out: Vec<u64> = Vec::new();
        let mut rest = digits;
        while let Some((head, tail)) = rest.split_at_checked(2) {
            out.push(u64::from_str_radix(head, 16).expect("hex digits"));
            rest = tail;
        }
        out
    }

    #[tokio::test]
    async fn the_debug_overlay_is_a_decodable_png_of_the_analysed_size() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        let overlay = debug_image(&report).await;
        assert_eq!(
            (overlay.width, overlay.height),
            (400_u32, 300_u32),
            "at analysis scale, 1/1 here"
        );
        let painted = (0..overlay.height)
            .flat_map(|row| (0..overlay.width).map(move |column| (column, row)))
            .filter(|&(column, row)| overlay.get(column, row) != Some(PAPER))
            .count();
        assert!(
            painted > 500,
            "the overlay draws regions, ids and marks: {painted}"
        );
    }

    #[tokio::test]
    async fn a_downscaled_sketch_reports_original_pixel_coordinates() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let small_file = write_png(dir.path(), "small.png", &sketch(1));
        let big_file = write_png(dir.path(), "big.png", &sketch(6));
        let small = call(&client, "trace_sketch", trace_args(&small_file)).await;
        let mut big_args = trace_args(&big_file);
        big_args["max_size"] = json!(600_u64);
        big_args["close_gaps"] = json!(100_i64);
        let big = call(&client, "trace_sketch", big_args).await;
        assert_eq!(big["image"]["width"], json!(2_400_u64), "the source sheet");
        assert_eq!(
            big["image"]["analysed_scale"],
            json!({"num": 1_u64, "den": 4_u64}),
            "600 of 2400 is exactly a quarter"
        );
        assert!(
            (20_u64..=28_u64).contains(&big["stroke_px"].as_u64().expect("a stroke")),
            "the 24-px pen still measures about 24, got {}",
            big["stroke_px"]
        );
        let big_regions = big["regions"].as_array().expect("regions");
        let small_regions = small["regions"].as_array().expect("regions");
        assert_scaled_regions(small_regions, big_regions);
    }

    #[tokio::test]
    async fn units_add_grid_snapped_map_coordinates() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let mut with_units = trace_args(&file);
        with_units["units"] = json!({"scale_num": 4_i64, "flip_y": true});
        let report = call(&client, "trace_sketch", with_units).await;
        assert_all_map_grids(&report);
        assert!(
            !report["gaps"][0]["map_center"].is_null(),
            "the gap centre maps too"
        );
        assert!(
            !report["marks"][0]["map_center"].is_null(),
            "and the mark centres"
        );
    }

    #[tokio::test]
    async fn without_units_the_map_coordinate_fields_stay_null() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let report = call(&client, "trace_sketch", trace_args(&file)).await;
        assert!(
            report["regions"][0]["map_points"].is_null(),
            "no units, no map"
        );
        assert!(report["gaps"][0]["map_center"].is_null());
        assert!(report["marks"][0]["map_center"].is_null());
    }

    #[tokio::test]
    async fn a_crop_leaves_coordinates_in_original_image_pixels() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let whole = call(&client, "trace_sketch", trace_args(&file)).await;
        // A 1-px inset that keeps every wall at least `close_gaps` pixels
        // from the crop edge, so the closing behaves as on the full sheet
        // and only the crop offset differs.
        let mut cropped_args = trace_args(&file);
        cropped_args["crop"] = json!({"x": 1_u64, "y": 1_u64, "width": 390_u64, "height": 297_u64});
        let cropped = call(&client, "trace_sketch", cropped_args).await;
        assert_eq!(
            cropped["image"]["width"],
            json!(400_u64),
            "the source sheet"
        );
        assert_eq!(
            cropped["regions"], whole["regions"],
            "the offset is added back, so the rows match the whole-sheet run"
        );
    }

    #[tokio::test]
    async fn colour_ink_traces_only_the_matching_pen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "pens.png", &two_pens());
        let mut args = json!({
            "source": file,
            "ink": {"mode": "color", "color": "#FF0000"},
        });
        args["close_gaps"] = json!(16_i64);
        let report = call(&client, "trace_sketch", args).await;
        assert!(report["threshold"].is_null(), "color mode applies no level");
        let regions = report["regions"].as_array().expect("a region list");
        assert_eq!(regions.len(), 1, "only the red rectangle encloses");
        let red_ring = points_of(&regions[0]);
        assert!(
            max_x(&red_ring) <= 101,
            "the traced ring is the red room: {red_ring:?}"
        );
    }

    #[tokio::test]
    async fn a_blank_sheet_measures_nothing_but_is_not_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "blank.png", &paper(200, 150));
        let report = call(&client, "trace_sketch", json!({"source": file})).await;
        assert_eq!(report["regions"], json!([]), "no enclosed areas");
        assert_eq!(report["gaps"], json!([]), "nothing to bridge");
        assert_eq!(report["marks"], json!([]), "nothing to mark");
        assert_eq!(report["plan_rooms"], json!([]), "no rooms to plan");
        let warnings = report["warnings"].as_array().expect("a warning list");
        assert!(
            !warnings.is_empty(),
            "the empty sheet explains itself: {warnings:?}"
        );
        assert!(
            report["debug"]["url"].is_string(),
            "the debug image still ships"
        );
    }

    #[tokio::test]
    async fn bad_crop_max_size_and_pen_are_invalid_arguments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let mut outside = trace_args(&file);
        outside["crop"] = json!({"x": 380_u64, "y": 0_u64, "width": 40_u64, "height": 100_u64});
        let (crop_code, crop_message, _) = call_err(&client, "trace_sketch", outside).await;
        assert_eq!(crop_code, "invalid_argument", "a crop past the edge");
        assert!(crop_message.contains("crop.x"), "{crop_message}");
        let mut small_max = trace_args(&file);
        small_max["max_size"] = json!(64_u64);
        let (size_code, size_message, _) = call_err(&client, "trace_sketch", small_max).await;
        assert_eq!(size_code, "invalid_argument", "max_size below the floor");
        assert!(size_message.contains("max_size"), "{size_message}");
        let no_pen = json!({"source": file, "ink": {"mode": "color"}});
        let (pen_code, pen_message, _) = call_err(&client, "trace_sketch", no_pen).await;
        assert_eq!(pen_code, "invalid_argument", "color without a colour");
        assert!(pen_message.contains("ink.color"), "{pen_message}");
    }

    #[tokio::test]
    async fn bytes_that_are_no_image_report_a_format_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"not an image at all").expect("write the fixture");
        let (code, _message, _) = call_err(
            &client,
            "trace_sketch",
            json!({"source": path.display().to_string()}),
        )
        .await;
        assert_eq!(code, "format_error", "no format could be decoded");
    }

    #[tokio::test]
    async fn a_zero_grid_is_refused_naming_the_parameter() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let mut grid_args = trace_args(&file);
        grid_args["grid"] = json!(0_i64);
        let (code, message, _) = call_err(&client, "trace_sketch", grid_args).await;
        assert_eq!(code, "invalid_argument", "snapping needs a grid");
        assert!(message.contains("grid"), "{message}");
    }

    #[tokio::test]
    async fn plan_rooms_and_the_gap_opening_build_an_error_free_map() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let base = iwad_features(dir.path());
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let mut args = trace_args(&file);
        args["units"] = json!({"scale_num": 4_i64, "flip_y": true});
        let report = call(&client, "trace_sketch", args).await;
        let door = report["gaps"][0].as_object().expect("the gap row");
        let centre = door["center"].as_array().expect("a [x, y] centre");
        let request = json!({
            "map": "MAP05",
            "rooms": report["plan_rooms"],
            "connections": [{
                "from": "R2",
                "to": "R3",
                "kind": "opening",
                "at": centre,
            }],
            "units": {"scale_num": 4_i64, "flip_y": true},
            "player_start": {"room": "R1"},
        });
        let built = call(&client, "build_map_from_plan", request).await;
        let rooms = built["rooms"].as_object().expect("the room bookkeeping");
        assert_eq!(rooms.len(), 3, "all three traced shapes became rooms");
        assert!(rooms.contains_key("R1") && rooms.contains_key("R2") && rooms.contains_key("R3"));
        let validation = call(
            &client,
            "validate_map",
            json!({"map": "MAP05", "min_severity": "error"}),
        )
        .await;
        assert_eq!(
            validation["issues"],
            json!([]),
            "the traced map carries no errors: {validation}"
        );
    }

    #[tokio::test]
    async fn the_same_sketch_traced_twice_answers_identically() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let file = write_png(dir.path(), "sketch.png", &sketch(1));
        let first = call(&client, "trace_sketch", trace_args(&file)).await;
        let second = call(&client, "trace_sketch", trace_args(&file)).await;
        assert_eq!(
            strip_debug(first),
            strip_debug(second),
            "measurement is deterministic"
        );
    }
}
