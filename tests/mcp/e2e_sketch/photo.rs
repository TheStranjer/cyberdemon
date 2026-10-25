//! The chain's photo slice: the photo-of-a-sketch regression, with no
//! harness-drawn plan in the way. First the library proof, with no client
//! and no asset round trip: [`apply_ops`] over the [`ruled_photo_sheet`]
//! fixture, proving the hairline-cleaning chain turns a photograph's uneven
//! light into one clean binarized sheet - walls kept pixel-for-pixel, ruling
//! dropped, and the shadowed top of the room classing exactly like the lit
//! bottom of it. Over that, the wire proof: `process_image` over the sheet's
//! `PNG` file reports both ops step by step and its published asset, fetched
//! back over `HTTP`, is the same clean sheet through the `PNG` encode/decode
//! round trip. The last link of the chain rides that same connection: the
//! cleaned sheet's own asset URL goes into `trace_sketch` with no other
//! parameter and comes back as the drawn room - one rectangle region
//! covering the interior, enclosing its probes, whole in area and hole-free,
//! with no second room-shaped region on the sheet. The sheet size and the
//! black pen come from [`crate::e2e_sketch`], the fixture and its measures
//! from [`crate::support`].

use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::gfx::ops::{ImageOp, OpsReport, apply_ops};
use serde_json::{Value, json};

use crate::e2e_sketch::{INK, SHEET_HEIGHT, SHEET_WIDTH, points_of};
use crate::support::{
    PHOTO_ROOM, PHOTO_RULING_PERIOD, PHOTO_RULING_PHASE, PHOTO_TICK, PHOTO_WALL_PX, asset_png,
    call, connect, ruled_photo_sheet, write_png,
};

// The chain proof, at library level: apply_ops over the ruled photo
// fixture, with no client and no asset round trip.

/// The two ops of the photo pass's cleaning chain, on the parameters the
/// [`ruled_photo_sheet`] fixture is tuned against. The background op
/// flattens the sheet's shadow and vignette and inks everything darker
/// than its own local paper, so the pale ruling and the near-black walls
/// alike come out black; the opening then drops every stroke thinner than
/// its `3x3` element, keeping the 3-px walls and erasing the 1-px ruling.
fn photo_chain() -> [ImageOp; 2] {
    [
        ImageOp::AdaptiveThreshold {
            radius: 6_u32,
            offset: 10_u8,
            invert: false,
        },
        ImageOp::ThinStrokeFilter {
            radius: 1_u32,
            level: None,
            invert: false,
        },
    ]
}

/// The middle row of the room's top wall bar and the columns of it the
/// samples read: `PHOTO_ROOM`'s box carries [`PHOTO_WALL_PX`] 3-px bars,
/// so its top bar is rows 90, 91 and 92, centred on 91 — the middle line
/// a `3x3` opening erodes to and dilates back.
const TOP_WALL: (u32, u32, u32) = (91_u32, 130_u32, 410_u32);
/// The middle row of the room's bottom wall bar (rows 297, 298 and 299)
/// and the same sampled columns.
const BOTTOM_WALL: (u32, u32, u32) = (298_u32, 130_u32, 410_u32);
/// The middle column of the room's left wall bar (columns 120, 121 and
/// 122) and the rows of it the samples read.
const LEFT_WALL: (u32, u32, u32) = (121_u32, 100_u32, 290_u32);
/// The middle column of the room's right wall bar (columns 417, 418 and
/// 419) and the same sampled rows.
const RIGHT_WALL: (u32, u32, u32) = (418_u32, 100_u32, 290_u32);
/// The paper spans the ruling is sampled in: the sheet but for the 5 px
/// either side of the room's wall box, where the bars themselves stand.
const RULING_SPANS: [(u32, u32); 2] = [(8_u32, 115_u32), (425_u32, 592_u32)];
/// The eight rows ending on the top wall bar's centre row, and the eight
/// ending on the bottom bar's: six rows of paper over two rows of wall in
/// each, the first band deep in the fixture's shadow and the second in
/// full light.
const LIGHT_BAND_TOPS: (u32, u32) = (84_u32, 291_u32);
/// The classes those eight rows must show where a wall bar stands under
/// them: six of paper, then the bar's own row above its centre and its
/// centre row. The shadowed band shows them no less than the lit one.
const WALL_BAND: [bool; 8] = [false, false, false, false, false, false, true, true];
/// The classes of the same eight rows over bare paper, either side of the
/// room: paper all the way down, in the shadow no less than in the light.
const PAPER_BAND: [bool; 8] = [false; 8];
/// The columns the light bands are read down with the room's bars under
/// them: the room's own inside, clear of its side bars, which run from the
/// top wall to the bottom one so their two bands are not mirrors.
const WALL_BAND_COLUMNS: (u32, u32) = (124_u32, 416_u32);
/// The columns the same bands are read down over bare paper, either side
/// of the room's box.
const PAPER_BAND_COLUMNS: [(u32, u32); 2] = [(100_u32, 120_u32), (420_u32, 440_u32)];
/// The stride between the light-band samples: dense enough to cross both
/// walls and to catch a band of invented ink a few pixels wide.
const BAND_STRIDE: u32 = 3_u32;
/// The vignette's outer block at each top corner: `(0, 0, 84, 44)` and its
/// mirror at the right, the darkest paper on the sheet and the only place
/// where its hard luminance steps are stacked.
const VIGNETTE: (u32, u32) = (84_u32, 44_u32);
/// How far inside the sheet's frame no ink may stand: the background
/// window's reach at the clipped border, where the shadow ramp's darkest
/// rows would otherwise print a band the opening keeps.
const FRAME_MARGIN: u32 = 4_u32;
/// The opaque white both ops paint for paper.
const CLEAN_PAPER: [u8; 4] = [255_u8, 255, 255, 255];

/// `adaptive_threshold` then `thin_stroke_filter` over the ruled photo:
/// the shadowed room's walls stand in clean black, its ruling is gone, and
/// the shadowed band over the top wall classifies exactly like the lit band
/// over the bottom one — the uneven-light regression, with no client and no
/// asset round trip in the way.
#[test]
fn apply_ops_cleans_the_ruled_photo() {
    let sheet = ruled_photo_sheet(SHEET_WIDTH, SHEET_HEIGHT);
    let (clean, report) = apply_ops(sheet, &photo_chain()).expect("the photo chain runs");
    assert_eq!(
        (clean.width, clean.height),
        (SHEET_WIDTH, SHEET_HEIGHT),
        "a binarize-then-open chain never changes the shape"
    );
    assert_cleaning_report(&report);
    assert_the_walls_stand(&clean);
    assert_nothing_else_stands(&clean, &report);
    assert_the_ruling_is_gone(&clean);
    assert_the_light_is_even(&clean);
    assert_the_vignette_is_paper(&clean);
    assert_the_frame_is_paper(&clean);
}

/// One `u64` measurement of a report step, read by field name.
fn measurement(step: &Value, field: &str) -> u64 {
    step[field].as_u64().expect("a count measurement")
}

/// The chain's two report steps: the binarizer's ink with no global level
/// of its own, then the opening on the Otsu level of that black-and-white
/// sheet, which drops many times more ink than it keeps and splits the
/// first step's ink exactly between what it dropped and what it kept.
fn assert_cleaning_report(report: &OpsReport) {
    let steps = &report.steps;
    assert_eq!(steps.len(), 2_usize, "one entry per op");
    assert_eq!(steps[0]["op"], json!("adaptive_threshold"), "the first op");
    assert_eq!(
        steps[0]["otsu"],
        json!(false),
        "the binarizer picks no global level"
    );
    assert_eq!(steps[1]["op"], json!("thin_stroke_filter"), "the second op");
    assert_eq!(
        steps[1]["otsu"],
        json!(true),
        "the opening thresholds on Otsu's level"
    );
    let ink = measurement(&steps[0], "ink_pixels");
    let kept = measurement(&steps[1], "kept_ink_pixels");
    let dropped = measurement(&steps[1], "removed_ink_pixels");
    assert!(ink > 0_u64, "the binarizer inks the sheet: {ink} pixels");
    assert_eq!(
        dropped.saturating_add(kept),
        ink,
        "the opening only splits the ink the binarizer painted"
    );
    assert!(
        dropped > kept.saturating_mul(5_u64),
        "the ~29k-px ruling outweighs the ~3k-px walls five times over: \
         {dropped} dropped against {kept} kept"
    );
    let darkest = measurement(&steps[0], "background_min");
    let lightest = measurement(&steps[0], "background_max");
    assert!(
        lightest.saturating_sub(darkest) > 100_u64,
        "the background the op flattened really is uneven: {darkest} to {lightest}"
    );
}

/// The ink of a cleaned sheet, counted over the whole picture: every pixel
/// the two ops left as anything but the exact white of [`CLEAN_PAPER`].
fn ink_count(image: &RgbaImage) -> usize {
    image
        .pixels
        .chunks_exact(CLEAN_PAPER.len())
        .filter(|pixel| *pixel != CLEAN_PAPER)
        .count()
}

/// The room's own ink area, in pixels: its four [`PHOTO_WALL_PX`] bars
/// along the edges of `PHOTO_ROOM`, the four `bar x bar` corner blocks
/// counted once, plus the interior tick.
fn wall_area() -> u64 {
    let (x0, y0, x1, y1) = PHOTO_ROOM;
    let (tx0, ty0, tx1, ty1) = PHOTO_TICK;
    let bar = u64::from(PHOTO_WALL_PX);
    let long = u64::from(x1.saturating_sub(x0)).saturating_mul(bar);
    let short = u64::from(y1.saturating_sub(y0)).saturating_mul(bar);
    let corners = bar.saturating_mul(bar).saturating_mul(4_u64);
    let tick =
        u64::from(tx1.saturating_sub(tx0)).saturating_mul(u64::from(ty1.saturating_sub(ty0)));
    long.saturating_add(long)
        .saturating_add(short)
        .saturating_add(short)
        .saturating_sub(corners)
        .saturating_add(tick)
}

/// The cleaned sheet carries the opening's kept ink and nothing else: the
/// picture's own ink count, the report's `kept_ink_pixels` and the room's
/// area agree to the pixel, so no band or blob stands anywhere outside it.
fn assert_nothing_else_stands(image: &RgbaImage, report: &OpsReport) {
    let kept = measurement(&report.steps[1], "kept_ink_pixels");
    let ink = u64::try_from(ink_count(image)).expect("a test count");
    assert_eq!(
        ink, kept,
        "the opening kept exactly the ink the sheet carries"
    );
    assert_eq!(
        kept,
        wall_area(),
        "the kept ink is the room's four bars and its tick, and nothing more"
    );
}

/// Whether the cleaned sheet draws the pixel at `(column, row)` as ink.
fn draws_ink(image: &RgbaImage, column: u32, row: u32) -> bool {
    image.get(column, row) != Some(CLEAN_PAPER)
}

/// Every pixel of the cleaned sheet's row `row` between columns `low` and
/// `high` is the exact black the opening paints its kept ink in.
fn assert_row_is_ink(image: &RgbaImage, row: u32, low: u32, high: u32) {
    for column in low..high {
        assert_eq!(
            image.get(column, row),
            Some(INK),
            "the wall at ({column}, {row}) stands"
        );
    }
}

/// Every pixel of the cleaned sheet's column `column` between rows `low`
/// and `high` is the exact black of the opening's kept ink.
fn assert_column_is_ink(image: &RgbaImage, column: u32, low: u32, high: u32) {
    for row in low..high {
        assert_eq!(
            image.get(column, row),
            Some(INK),
            "the wall at ({column}, {row}) stands"
        );
    }
}

/// Every sampled wall-centreline pixel is kept ink: a bar as thick as the
/// opening's element erodes to its middle line and dilates back to its
/// full width, so the middle row of each horizontal bar and the middle
/// column of each vertical one stand.
fn assert_the_walls_stand(image: &RgbaImage) {
    assert_row_is_ink(image, TOP_WALL.0, TOP_WALL.1, TOP_WALL.2);
    assert_row_is_ink(image, BOTTOM_WALL.0, BOTTOM_WALL.1, BOTTOM_WALL.2);
    assert_column_is_ink(image, LEFT_WALL.0, LEFT_WALL.1, LEFT_WALL.2);
    assert_column_is_ink(image, RIGHT_WALL.0, RIGHT_WALL.1, RIGHT_WALL.2);
}

/// Every row of the sheet carrying the fixture's ruling, read off its
/// phase and period.
fn ruled_rows(height: u32) -> Vec<u32> {
    let phase = usize::try_from(PHOTO_RULING_PHASE).expect("a test phase");
    let period = usize::try_from(PHOTO_RULING_PERIOD).expect("a test period");
    let rows = usize::try_from(height).expect("a test height");
    (phase..rows)
        .step_by(period)
        .filter_map(|row| u32::try_from(row).ok())
        .collect()
}

/// One sampled column of the cleaned sheet is paper at every ruled row: a
/// 1-px line holds no `3x3` square, so the opening drops it however hard
/// the binarizer inked it.
fn assert_no_ruling_in_column(image: &RgbaImage, column: u32, rows: &[u32]) {
    for row in rows {
        assert_eq!(
            image.get(column, *row),
            Some(CLEAN_PAPER),
            "the ruling at ({column}, {row}) is gone"
        );
    }
}

/// One sampled span of the cleaned sheet: every column of it carries no
/// ruling.
fn assert_no_ruling_in_span(image: &RgbaImage, low: u32, high: u32, rows: &[u32]) {
    for column in low..high {
        assert_no_ruling_in_column(image, column, rows);
    }
}

/// Every sampled ruling pixel of the sheet, over its whole height and in
/// both paper margins, comes back the exact white of paper.
fn assert_the_ruling_is_gone(image: &RgbaImage) {
    let rows = ruled_rows(image.height);
    assert!(!rows.is_empty(), "the sheet carries ruled rows");
    for (low, high) in RULING_SPANS {
        assert_no_ruling_in_span(image, low, high, &rows);
    }
}

/// The strided columns of a sampled span.
fn sampled_columns(low: u32, high: u32, stride: u32) -> Vec<u32> {
    let step = usize::try_from(stride).expect("a test stride");
    let first = usize::try_from(low).expect("a test column");
    let last = usize::try_from(high).expect("a test column");
    (first..last)
        .step_by(step)
        .filter_map(|column| u32::try_from(column).ok())
        .collect()
}

/// One sampled column's two bands against the classes they must show: the
/// eight rows over the shadowed top wall and the eight over the fully lit
/// bottom one classify alike, pixel for pixel. The sheet's own paper
/// darkens by 60 luminance levels or more between the two bands, so a
/// single global level classes one of them wrong; only a local background
/// gives both the same rows of paper and of wall.
fn assert_band_matches(image: &RgbaImage, column: u32, band: [bool; 8]) {
    let (shadowed, lit) = LIGHT_BAND_TOPS;
    for (offset, ink) in band.iter().copied().enumerate() {
        let step = u32::try_from(offset).expect("a test offset");
        let in_shadow = shadowed.saturating_add(step);
        let in_light = lit.saturating_add(step);
        assert_eq!(
            draws_ink(image, column, in_shadow),
            ink,
            "the shadowed row {in_shadow} of column {column}"
        );
        assert_eq!(
            draws_ink(image, column, in_light),
            ink,
            "the lit row {in_light} of column {column}"
        );
    }
}

/// Every sampled column of one span shows the band's classes in both the
/// shadowed band and the lit one.
fn assert_bands_of_span_match(image: &RgbaImage, low: u32, high: u32, band: [bool; 8]) {
    for column in sampled_columns(low, high, BAND_STRIDE) {
        assert_band_matches(image, column, band);
    }
}

/// The uneven-light regression: the shadowed band over the room's top wall
/// and the lit band over its bottom wall show the same classes down the
/// room's inside, and both bands stay clean paper down the bare margins
/// outside it.
fn assert_the_light_is_even(image: &RgbaImage) {
    let (low, high) = WALL_BAND_COLUMNS;
    assert_bands_of_span_match(image, low, high, WALL_BAND);
    for (first, last) in PAPER_BAND_COLUMNS {
        assert_bands_of_span_match(image, first, last, PAPER_BAND);
    }
}

/// Every pixel of the cleaned sheet's row `row` between columns `low` and
/// `high` is the exact white of paper.
fn assert_row_is_paper(image: &RgbaImage, row: u32, low: u32, high: u32) {
    for column in low..high {
        assert!(
            !draws_ink(image, column, row),
            "the sheet at ({column}, {row}) is paper"
        );
    }
}

/// Every pixel of the cleaned sheet's column `column` between rows `low`
/// and `high` is the exact white of paper.
fn assert_column_is_paper(image: &RgbaImage, column: u32, low: u32, high: u32) {
    for row in low..high {
        assert!(
            !draws_ink(image, column, row),
            "the sheet at ({column}, {row}) is paper"
        );
    }
}

/// Both corner vignettes, out to their outer block, come back uniformly
/// paper: their steps stay under the op's `offset` at every depth, so the
/// darkest paper on the sheet is still background, and the ruling crossing
/// them is still only a hairline for the opening to drop.
fn assert_the_vignette_is_paper(image: &RgbaImage) {
    let (wide, high) = VIGNETTE;
    let right = image.width.saturating_sub(wide);
    for row in 0_u32..high {
        assert_row_is_paper(image, row, 0_u32, wide);
        assert_row_is_paper(image, row, right, image.width);
    }
}

/// The two frame rows `band` in from the sheet's top and bottom edges,
/// across its whole width, are paper.
fn assert_frame_rows_are_paper(image: &RgbaImage, band: u32) {
    let bottom = image.height.saturating_sub(band.saturating_add(1_u32));
    assert_row_is_paper(image, band, 0_u32, image.width);
    assert_row_is_paper(image, bottom, 0_u32, image.width);
}

/// The two frame columns `band` in from the sheet's left and right edges,
/// down its whole height, are paper.
fn assert_frame_columns_are_paper(image: &RgbaImage, band: u32) {
    let right = image.width.saturating_sub(band.saturating_add(1_u32));
    assert_column_is_paper(image, band, 0_u32, image.height);
    assert_column_is_paper(image, right, 0_u32, image.height);
}

/// No ink within [`FRAME_MARGIN`] pixels of any frame edge: neither the
/// clipped window at the shadow ramp's dark end nor a step at the sheet's
/// border is allowed to grow a band the opening would then keep.
fn assert_the_frame_is_paper(image: &RgbaImage) {
    for band in 0_u32..FRAME_MARGIN {
        assert_frame_rows_are_paper(image, band);
        assert_frame_columns_are_paper(image, band);
    }
}

// The wire proof: the same cleaning chain run through `process_image`,
// read off the tool's JSON report and off the published PNG fetched
// over HTTP, including the encode/decode round trip.

/// The `process_image` request over `source`: the photo cleaning chain
/// as wire parameters, the same `radius` and `offset` [`photo_chain`]
/// runs at library level. `quantize: false` keeps the proof free of a
/// session — the quantized preview is the one thing that wants a
/// `PLAYPAL` — and the chain's report and its full `RGBA`
/// `processed` picture are what this test reads.
fn photo_chain_request(source: &str) -> Value {
    json!({
        "source": source,
        "ops": [
            { "op": "adaptive_threshold", "radius": 6_u64, "offset": 10_u64 },
            { "op": "thin_stroke_filter", "radius": 1_u64 },
        ],
        "quantize": false,
    })
}

/// The first wire step: the local binarizer echoes its `radius` and
/// `offset`, picks no global level, inks the sheet, and its flattened
/// background's extremes prove the local-means window really ran over
/// the fixture's gradient. Returns the step's ink count.
fn assert_photo_chain_flatten_step(step: &Value) -> u64 {
    assert_eq!(
        step["op"],
        json!("adaptive_threshold"),
        "the first op echoes"
    );
    assert_eq!(step["radius"], json!(6_u64), "the asked radius echoes");
    assert_eq!(step["offset"], json!(10_u64), "and the asked offset");
    assert_eq!(
        step["otsu"],
        json!(false),
        "the binarizer picks no global level"
    );
    let ink = step["ink_pixels"].as_u64().expect("an ink count");
    assert!(ink > 0_u64, "the binarizer inks the sheet: {ink} pixels");
    let darkest = step["background_min"].as_u64().expect("a floor");
    let lightest = step["background_max"].as_u64().expect("a ceiling");
    assert!(
        lightest > darkest,
        "the flattened background really is uneven: {darkest} to {lightest}",
    );
    ink
}

/// The second wire step: the opening echoes its `radius`, thresholds on
/// `Otsu`'s reported level, drops many times more ink than it keeps, and
/// splits the binarizer's ink exactly between what it dropped and what
/// it kept.
fn assert_photo_chain_opening_step(step: &Value, ink: u64) {
    assert_eq!(step["op"], json!("thin_stroke_filter"), "the second op");
    assert_eq!(step["radius"], json!(1_u64), "the asked radius echoes");
    assert_eq!(
        step["otsu"],
        json!(true),
        "the opening thresholds on Otsu's level"
    );
    assert!(step["level"].is_number(), "the used level reports: {step}");
    let kept = step["kept_ink_pixels"].as_u64().expect("a kept count");
    let removed = step["removed_ink_pixels"]
        .as_u64()
        .expect("a removed count");
    assert!(
        removed > kept.saturating_mul(5_u64),
        "the ruling outweighs the walls five times over: \
         {removed} dropped against {kept} kept",
    );
    assert_eq!(
        removed.saturating_add(kept),
        ink,
        "the opening only splits the ink the binarizer painted",
    );
}

/// The wire report's two steps, one per op, in order.
fn assert_photo_chain_steps(report: &Value) {
    let steps = report["steps"].as_array().expect("a step list");
    assert_eq!(steps.len(), 2_usize, "one entry per op");
    let ink = assert_photo_chain_flatten_step(&steps[0]);
    assert_photo_chain_opening_step(&steps[1], ink);
}

/// The uneven-light regression through the whole wire path: over the
/// ruled photo written to a file, `process_image` reports both cleaning
/// steps, publishes a full-size `PNG` that `HTTP` serves back intact,
/// and the fetched picture keeps every wall centrelined pixel in clean
/// black, returns every sampled ruling pixel as clean paper, classes
/// the shadowed band over the top wall exactly like the lit band over
/// the bottom one, and stays free of ink near the frame — including
/// through the `PNG` encode/decode round trip.
#[tokio::test]
async fn photo_chain_reports_both_ops_and_publishes_a_clean_png() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let sheet = ruled_photo_sheet(SHEET_WIDTH, SHEET_HEIGHT);
    let file = write_png(dir.path(), "PHOTO.png", &sheet);
    let report = call(&client, "process_image", photo_chain_request(&file)).await;
    assert_eq!(report["width"], json!(600_u64), "the sheet's width stands");
    assert_eq!(report["height"], json!(400_u64), "and its height");
    assert_photo_chain_steps(&report);
    let url = report["processed"]["url"]
        .as_str()
        .expect("a processed url");
    let clean = asset_png(url).await;
    assert_eq!(
        clean.width, SHEET_WIDTH,
        "the published PNG decodes at the asked width"
    );
    assert_eq!(
        clean.height, SHEET_HEIGHT,
        "and at the asked height, unchanged"
    );
    assert_the_walls_stand(&clean);
    assert_the_ruling_is_gone(&clean);
    assert_the_light_is_even(&clean);
    assert_the_frame_is_paper(&clean);
}

// The last link of the chain: the cleaned photo's own asset URL is a
// usable trace source, read back through the connection that published
// it — assets live in that connection's own store, so both calls ride
// one client.

/// The room's interior as drawn into [`ruled_photo_sheet`]: the paper
/// inside `PHOTO_ROOM`'s four [`PHOTO_WALL_PX`] bars, as an inclusive
/// `[x0, y0, x1, y1]` pixel box — `[123, 93, 417, 297]` for the fixture.
fn room_interior() -> [i64; 4] {
    let (x0, y0, x1, y1) = PHOTO_ROOM;
    let wall = i64::from(PHOTO_WALL_PX);
    [
        i64::from(x0).checked_add(wall).expect("a test interior"),
        i64::from(y0).checked_add(wall).expect("a test interior"),
        i64::from(x1).checked_sub(wall).expect("a test interior"),
        i64::from(y1).checked_sub(wall).expect("a test interior"),
    ]
}

/// How far a traced side may stand off the drawn interior side it is
/// asked to cover: the wall bars' ink and the ring's rounded corners.
const INTERIOR_SLACK: i64 = 4_i64;

/// The room's centre `(270, 195)` and four probes near its corners, all
/// deep inside the interior and clear of [`PHOTO_TICK`]'s mark.
const INTERIOR_PROBES: [(i64, i64); 5] = [
    (270_i64, 195_i64),
    (140_i64, 110_i64),
    (400_i64, 110_i64),
    (140_i64, 280_i64),
    (400_i64, 280_i64),
];

/// The narrowest area the unbroken interior can hold: 290 x 200 px,
/// far over anything a split or hole-heavy piece leaves and well under
/// the measured 63000 px of the grown region.
const MIN_ROOM_AREA: u64 = 58_000_u64;

/// One region's inclusive `[x0, y0, x1, y1]` `bbox` as four integers.
fn bbox_of(row: &Value) -> [i64; 4] {
    let list = row["bbox"].as_array().expect("an [x0, y0, x1, y1] box");
    assert_eq!(list.len(), 4_usize, "a box has four sides");
    let mut sides = [0_i64; 4];
    for (slot, value) in sides.iter_mut().zip(list) {
        *slot = value.as_i64().expect("a box side");
    }
    sides
}

/// Every side of the traced `bbox` stands within [`INTERIOR_SLACK`] of
/// the drawn interior side it is asked to cover.
fn assert_covers_the_interior(bbox: &[i64; 4]) {
    for (side, (got, want)) in bbox.iter().zip(room_interior()).enumerate() {
        let drift = got.saturating_sub(want).abs();
        assert!(
            drift <= INTERIOR_SLACK,
            "the box {bbox:?} stands {drift} off interior side {side} at {want}"
        );
    }
}

/// Whether the edge `start` to `end` straddles the east-going ray
/// `y = py` of `probe` and crosses it east of the point, in integers
/// only and with no division: comparing `rise x run` against
/// `(px - ax) x side` says on which side of the point the crossing
/// of the two rows' lines falls.
fn crosses_east(start: (i64, i64), end: (i64, i64), probe: (i64, i64)) -> bool {
    let (px, py) = probe;
    let (ax, ay) = start;
    let (bx, by) = end;
    if (ay > py) == (by > py) {
        return false;
    }
    let side = by.checked_sub(ay).expect("a straddling edge");
    let rise = py.checked_sub(ay).expect("a test row");
    let run = bx.checked_sub(ax).expect("a test column");
    let left = rise.checked_mul(run).expect("a test product");
    let right = px
        .checked_sub(ax)
        .expect("a test column")
        .checked_mul(side)
        .expect("a test product");
    if side > 0 { left > right } else { left < right }
}

/// Whether `probe` stands inside the integer ring `ring` by the
/// even-odd ray-crossing rule: every edge that crosses the east-going
/// ray `y = py` past the point flips the verdict once, and the ring
/// closes from its last vertex back to its first.
fn inside_ring(probe: (i64, i64), ring: &[(i64, i64)]) -> bool {
    let mut inside = false;
    let mut start = match ring.last() {
        Some(point) => *point,
        None => return false,
    };
    for &end in ring {
        inside ^= crosses_east(start, end, probe);
        start = end;
    }
    inside
}

/// The region's `points` ring encloses the room's centre and every
/// interior probe: the polygon really covers the drawn room.
fn assert_encloses_the_room(region: &Value) {
    let ring = points_of(region);
    for probe in INTERIOR_PROBES {
        assert!(
            inside_ring(probe, &ring),
            "the probe {probe:?} stands inside the traced region: {ring:?}"
        );
    }
}

/// Whether the inclusive box `other` touches the inclusive box `core`.
fn boxes_touch(other: &[i64; 4], core: &[i64; 4]) -> bool {
    other[0] <= core[2] && other[2] >= core[0] && other[1] <= core[3] && other[3] >= core[1]
}

/// No other region grows into the room: every other `bbox` stays clear
/// of the interior inset by [`INTERIOR_SLACK`], so no overlap reaches
/// more than the slack past an interior side.
fn assert_no_second_room(regions: &[Value], room: &Value) {
    let interior = room_interior();
    let core = [
        interior[0]
            .checked_add(INTERIOR_SLACK)
            .expect("a test core"),
        interior[1]
            .checked_add(INTERIOR_SLACK)
            .expect("a test core"),
        interior[2]
            .checked_sub(INTERIOR_SLACK)
            .expect("a test core"),
        interior[3]
            .checked_sub(INTERIOR_SLACK)
            .expect("a test core"),
    ];
    let strangers: Vec<&Value> = regions
        .iter()
        .filter(|row| row["id"] != room["id"])
        .collect();
    for row in strangers {
        let other = bbox_of(row);
        assert!(
            !boxes_touch(&other, &core),
            "the region {} stays out of the room: {other:?} against the core {core:?}",
            row["id"]
        );
    }
}

/// The trace over the cleaned sheet: at least one region, one of them a
/// rectangle covering the drawn interior within [`INTERIOR_SLACK`] on
/// every side, enclosing its centre and corner probes, whole in area
/// and hole-free — and the sheet's background nowhere else room-shaped.
fn assert_room_traced(report: &Value) {
    let regions = report["regions"].as_array().expect("a region list");
    assert!(
        !regions.is_empty(),
        "the cleaned sheet traces a region: {}",
        report["regions"]
    );
    let room = regions
        .iter()
        .find(|row| row["is_rectangle"].as_bool().unwrap_or(false))
        .expect("one region traces as a rectangle");
    assert_covers_the_interior(&bbox_of(room));
    assert_encloses_the_room(room);
    assert!(
        room["area_px"].as_u64().expect("an area") >= MIN_ROOM_AREA,
        "the interior comes back whole: {}",
        room["area_px"]
    );
    assert_eq!(room["holes"], json!(0_u64), "the room holds no island");
    assert_no_second_room(regions, room);
}

/// The chain's last link over one connection: `process_image` cleans
/// the ruled photo into a published asset and `trace_sketch` takes that
/// asset's own URL with no other parameter — no `units`, no session,
/// and the 600x400 sheet sits under the default 1024 `max_size`, so
/// every number reads in sheet pixels — and recovers the drawn room.
#[tokio::test]
async fn the_cleaned_photo_traces_as_the_drawn_room() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let sheet = ruled_photo_sheet(SHEET_WIDTH, SHEET_HEIGHT);
    let file = write_png(dir.path(), "PHOTO.png", &sheet);
    let processed = call(&client, "process_image", photo_chain_request(&file)).await;
    let url = processed["processed"]["url"]
        .as_str()
        .expect("a processed url");
    let report = call(&client, "trace_sketch", json!({ "source": url })).await;
    assert_room_traced(&report);
}
