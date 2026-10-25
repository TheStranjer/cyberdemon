//! The `sketch` tool group as seen by a real MCP client: `trace_sketch`
//! measures a synthetic three-room floor plan (an L-shaped room and two
//! stacked rectangles sharing a broken wall, a red dot and a black X) into
//! three regions with welded shared coordinates, one bridged gap between
//! the stacked rooms, two coloured marks, adjacency, and a decodable
//! debug `PNG`; the downscale path reports original-image pixels at 6x
//! the small sketch's coordinates; `units` add grid-snapped `map_points`
//! with a mirrored `y`; a `crop` leaves coordinates in original-image
//! space; `ink.mode: "color"` traces only the matching pen; a blank sheet
//! answers empty lists plus a warning and the debug image instead of an
//! error; a bad `crop`, an out-of-range `max_size`, `color` without a pen,
//! an out-of-range `max_gaps`/`max_marks` and non-image bytes report their
//! documented codes; and `plan_rooms` with a connection at the gap centre
//! build through `build_map_from_plan` into an error-free map.
//!
//! Plus the noise pass: a ruled, speckled scan of a three-room plan
//! answers at most 50 gap and 50 mark rows, largest first, under the
//! documented 20k-character bound, with `gaps_total`/`marks_total`,
//! `truncated`, warnings naming every omitted count with the parameter that
//! lifts it, region id lists and a debug overlay that cover exactly the
//! reported rows, `noise_floor_px` and `verbose` as the knobs, and the
//! sub-stroke dust dropped by the stroke-wide floor on its own.
//!
//! The helpers every topic draws its sheets with live here: the paper and
//! pen colours, the [`bar`] and [`cross`] painting primitives behind the
//! [`sketch`] and [`two_pens`] fixtures, the `PNG` writer and the standard
//! trace arguments, the asset and debug-overlay fetchers and the region,
//! point, colour and scaling readers the asserts run through. The tests
//! themselves are split by topic into [`noise`] (the bounded lists, the
//! floor and `verbose`), [`trace`] (the three-room measurements), [`params`]
//! (the knobs and the refusals) and [`build`] (the traced plan becoming a
//! map).

use std::path::Path;

use cyberdemon::gfx::codec::{decode_image, encode_png};
use cyberdemon::gfx::image::RgbaImage;
use serde_json::{Value, json};

use crate::support::http_get;

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

#[cfg(test)]
#[path = "sketch/build.rs"]
mod build;
#[cfg(test)]
#[path = "sketch/noise.rs"]
mod noise;
#[cfg(test)]
#[path = "sketch/params.rs"]
mod params;
#[cfg(test)]
#[path = "sketch/trace.rs"]
mod trace;
