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
//! error; a bad `crop`, an out-of-range `max_size`, `color` without a pen
//! and non-image bytes report their documented codes; and `plan_rooms`
//! with a connection at the gap centre build through
//! `build_map_from_plan` into an error-free map.

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
