//! Column codec tests: whole-lump round trips, the `DeePsea` tall-patch
//! readings, the run splitting the encoder chooses and the per-column
//! decode and paint steps.

use super::{build, full, patch_lump, post, speckle, table_end, tall, top_run_only};
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::{Patch, decode_column, decode_patch, encode_patch, paint_post};
use crate::wad::bytes::u32_at;
use crate::wad::classify::is_doom_patch;

#[test]
fn decodes_a_hand_built_two_by_three_patch() {
    let column0 = [post(0, &[5, 7]), vec![0xFF_u8]].concat();
    let column1 = [post(2, &[9]), vec![0xFF_u8]].concat();
    let bytes = patch_lump(2, 3, (-3, -2), &[&column0, &column1]);
    let patch = decode_patch(&bytes).expect("the hand-built patch decodes");
    assert_eq!(patch.left_offset, -3, "left offset from the header");
    assert_eq!(patch.top_offset, -2, "top offset from the header");
    let expected = IndexedImage::new(
        2,
        3,
        vec![5, 0, 7, 0, 0, 9],
        vec![true, false, true, false, false, true],
    )
    .expect("2x3 is a valid shape");
    assert_eq!(
        patch.image, expected,
        "post pixels land in the right rows, holes stay transparent"
    );
}

#[test]
fn round_trips_a_speckled_patch_with_holes_and_edges() {
    let patch = Patch {
        left_offset: -4,
        top_offset: -6,
        image: build(17, 40, speckle),
    };
    let bytes = encode_patch(&patch).expect("17x40 is encodable");
    assert!(is_doom_patch(&bytes), "an encoded patch classifies");
    let back = decode_patch(&bytes).expect("the bytes we just wrote decode");
    assert_eq!(
        back, patch,
        "speckles, holes, full and empty columns and the negative offsets round trip"
    );
    assert_eq!(
        back.image.get(16, 0),
        Some((0, false)),
        "the empty column stays transparent"
    );
    assert_eq!(
        back.image.get(3, 39),
        Some((7, true)),
        "the full column stays opaque"
    );
}

#[test]
fn round_trips_a_tall_patch() {
    let patch = Patch {
        left_offset: 0,
        top_offset: 0,
        image: build(8, 300, tall),
    };
    let bytes = encode_patch(&patch).expect("8x300 is encodable");
    assert!(is_doom_patch(&bytes), "a tall patch still classifies");
    let column = usize::try_from(u32_at(&bytes, 8).expect("column 0 offset")).expect("test offset");
    let body = &bytes[column..];
    assert!(body.len() > 27, "column 0 holds all three posts");
    assert_eq!(body[0], 10, "the first post is absolute row 10");
    assert!(
        body.windows(4).any(|word| word == [254, 0, 0, 0]),
        "a filler post carries the column past row 254"
    );
    assert_eq!(body[18], 6, "relative topdelta reaches row 260");
    assert_eq!(body[27], 30, "relative topdelta reaches row 290");
    let back = decode_patch(&bytes).expect("the tall bytes decode");
    assert_eq!(back, patch, "tall posts round trip identically");
}

#[test]
fn decodes_a_hand_made_dee_psea_column() {
    let mut column = post(200, &[1, 1]);
    column.extend_from_slice(&post(50, &[2, 2, 2]));
    column.extend_from_slice(&post(40, &[3]));
    column.push(0xFF);
    let bytes = patch_lump(1, 300, (0, 0), &[&column]);
    let patch = decode_patch(&bytes).expect("the DeePsea column decodes");
    assert_eq!(patch.image.get(0, 200), Some((1, true)), "absolute top 200");
    assert_eq!(
        patch.image.get(0, 201),
        Some((1, true)),
        "second pixel of the run"
    );
    assert_eq!(
        patch.image.get(0, 250),
        Some((2, true)),
        "relative 50 lands at 250"
    );
    assert_eq!(patch.image.get(0, 252), Some((2, true)), "run of three");
    assert_eq!(
        patch.image.get(0, 290),
        Some((3, true)),
        "relative 40 lands at 290"
    );
    assert_eq!(
        patch.image.get(0, 100),
        Some((0, false)),
        "gaps stay transparent"
    );
    assert_eq!(
        patch.image.get(0, 299),
        Some((0, false)),
        "below the last post too"
    );
}

#[test]
fn reaches_the_top_of_a_very_tall_patch() {
    let patch = Patch {
        left_offset: 0,
        top_offset: 0,
        image: build(1, 600, top_run_only),
    };
    let bytes = encode_patch(&patch).expect("1x600 is encodable");
    assert!(is_doom_patch(&bytes), "a 600-row patch classifies");
    let back = decode_patch(&bytes).expect("the relative filler chain decodes");
    assert_eq!(back, patch, "filler posts and relative deltas round trip");
}

#[test]
fn splits_long_runs_into_254_row_posts() {
    let patch = Patch {
        left_offset: 0,
        top_offset: 0,
        image: build(1, 300, full),
    };
    let bytes = encode_patch(&patch).expect("1x300 is encodable");
    assert!(is_doom_patch(&bytes), "a split column still classifies");
    let column = usize::try_from(u32_at(&bytes, 8).expect("column 0 offset")).expect("test offset");
    let body = &bytes[column..];
    assert!(body.len() > 308, "the split column is present in full");
    assert_eq!(body[0], 0, "the first post sits at row 0");
    assert_eq!(body[1], 254, "the run is split at 254 pixels");
    assert_eq!(body[258], 254, "the second post is absolute row 254");
    assert_eq!(body[259], 46, "and carries the remaining rows");
    assert_eq!(body[308], 0xFF, "then the column terminator");
    let back = decode_patch(&bytes).expect("the split column decodes");
    assert_eq!(back, patch, "split runs round trip identically");
}

#[test]
fn decodes_a_column_paints_its_posts() {
    let column = [post(0, &[1, 2]), post(3, &[9]), vec![0xFF_u8]].concat();
    let bytes = patch_lump(1, 5, (0, 0), &[&column]);
    let mut image =
        IndexedImage::new(1, 5, vec![0; 5], vec![false; 5]).expect("1x5 is a valid shape");
    decode_column(&mut image, &bytes, 0, table_end(1)).expect("the column decodes");
    assert_eq!(image.get(0, 0), Some((1, true)), "first pixel of post one");
    assert_eq!(image.get(0, 1), Some((2, true)), "second pixel of post one");
    assert_eq!(
        image.get(0, 2),
        Some((0, false)),
        "the gap stays transparent"
    );
    assert_eq!(
        image.get(0, 3),
        Some((9, true)),
        "absolute deltas stay absolute"
    );
}

#[test]
fn clips_posts_past_the_declared_height() {
    let bytes = patch_lump(
        1,
        2,
        (0, 0),
        &[&[post(1, &[4, 5, 6]), vec![0xFF_u8]].concat()],
    );
    let patch = decode_patch(&bytes).expect("overlong posts are tolerated");
    assert_eq!(
        patch.image.get(0, 1),
        Some((4, true)),
        "the first covered row"
    );
    assert_eq!(
        patch.image.get(0, 0),
        Some((0, false)),
        "row zero stays transparent"
    );
    assert_eq!(patch.image.height, 2, "the declared height is kept");
}

#[test]
fn paint_post_ignores_out_of_bounds_columns() {
    let mut image =
        IndexedImage::new(1, 2, vec![0, 0], vec![false, false]).expect("1x2 is a valid shape");
    paint_post(&mut image, 5, 0, &[7, 8]);
    assert_eq!(
        image.get(0, 1),
        Some((0, false)),
        "an out-of-range x paints nothing"
    );
    paint_post(&mut image, 0, 1, &[7, 8]);
    assert_eq!(
        image.get(0, 1),
        Some((7, true)),
        "the in-range row is painted"
    );
}
