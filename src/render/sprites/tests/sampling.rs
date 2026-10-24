//! Sampling tests: the texel a minified or a magnified sprite pixel reads,
//! the merged pass's own order and counts, and the integer helpers the
//! projection maths is built from: [`tex_row`], [`item_depth`],
//! [`Frame::nearer`] and [`strict_last_below`].

use alloc::vec;
use alloc::vec::Vec;

use super::{
    CENTRE, EYE, HORIZON, columns_of, gradient, lit, masked_at, paint_sprite, raw, rows_of, solid,
    standing,
};
use crate::render::canvas::Canvas;
use crate::render::sprites::placement::strict_last_below;
use crate::render::sprites::{Frame, Layer, item_depth, merged, tex_row};
use crate::render::view::ONE_16;

#[test]
fn a_minified_sprite_reads_the_texel_its_pixel_ends_in() {
    // k = 1/8 at depth 128: the 8-wide patch is one pixel across and
    // the 8-tall patch one row, and each pixel takes the last texel
    // whose span it touches.
    let canvas = paint_sprite(standing(gradient(8, 8), 128, EYE));
    assert_eq!(
        columns_of(&canvas),
        vec![15_u32, CENTRE],
        "one pixel across, and it straddles the centre column 16"
    );
    assert_eq!(
        rows_of(&canvas),
        vec![HORIZON - 1],
        "one row tall, ending at the eye's row"
    );
    assert_eq!(
        canvas.pixel(CENTRE, HORIZON - 1),
        Some(lit(63)),
        "the pixel's far edge has passed every texel boundary but the last"
    );
    assert_eq!(
        canvas.pixel(15, HORIZON - 1),
        Some(lit(59)),
        "and its neighbour ends inside texel column 3"
    );
}

#[test]
fn a_magnified_sprite_repeats_one_texel_over_several_pixels() {
    // k = 2 at depth 8: one texel is two pixels wide, and a pixel that
    // straddles a boundary takes the texel it ends in.
    let canvas = paint_sprite(standing(gradient(8, 8), 8, EYE));
    assert_eq!(
        columns_of(&canvas),
        (8_u32..24_u32).collect::<Vec<u32>>(),
        "sixteen pixels across"
    );
    assert_eq!(
        canvas.pixel(8, 16),
        Some(lit(0)),
        "the first pixel ends inside texel 0"
    );
    assert_eq!(
        canvas.pixel(9, 16),
        Some(lit(0)),
        "and so does the one after it"
    );
    assert_eq!(
        canvas.pixel(10, 16),
        Some(lit(1)),
        "then texel 1 takes over"
    );
    assert_eq!(
        canvas.pixel(23, 16),
        Some(lit(7)),
        "the last pixel ends in the last texel"
    );
}

#[test]
fn the_merged_order_is_far_to_near_with_masked_first_on_ties() {
    let masked = vec![masked_at(1, 1, 10), masked_at(0, 0, 90)];
    let sprite = standing(solid(1, 1, 1), 40, EYE);
    let sprites = vec![sprite];
    let order: Vec<i128> = merged(&masked, &sprites)
        .into_iter()
        .map(Layer::depth16)
        .collect();
    assert_eq!(
        order,
        vec![raw(90), raw(40), raw(10)],
        "one descending ordering over both kinds"
    );
    let tied = vec![masked_at(2, 2, 40)];
    let kinds: Vec<bool> = merged(&tied, &sprites)
        .iter()
        .map(|layer| matches!(layer, Layer::Sprite(_)))
        .collect();
    assert_eq!(
        kinds,
        vec![false, true],
        "an equal depth keeps the sprite on top"
    );
}

#[test]
fn texture_rows_round_down_and_are_never_tiled() {
    assert_eq!(tex_row(raw(3), 128), Some(3), "a plain row");
    assert_eq!(
        tex_row(raw(4).saturating_sub(1), 128),
        Some(3),
        "the last fraction of a unit rounds down"
    );
    assert_eq!(tex_row(raw(4), 4), None, "one past the last row is no row");
    assert_eq!(tex_row(-raw(1), 4), None, "above the first row is no row");
    assert_eq!(tex_row(raw(0), 0), None, "an empty image has no rows");
}

#[test]
fn depths_floor_to_whole_map_units_and_saturate() {
    assert_eq!(item_depth(raw(50)), 50, "exact units");
    assert_eq!(
        item_depth(raw(51).saturating_sub(1)),
        50,
        "a partial unit rounds down"
    );
    assert_eq!(item_depth(-raw(1)), -1, "below zero keeps the floor");
    assert_eq!(
        item_depth(raw(i64::MAX).saturating_add(ONE_16)),
        i64::MAX,
        "a depth past i64::MAX units saturates"
    );
}

#[test]
fn nearer_reads_the_buffer_in_row_major_order() {
    let mut canvas = Canvas::new(4, 2, [0_u8, 0, 0, 255]);
    let depth = vec![i64::MAX; 8_usize];
    let frame = Frame::new(&mut canvas, &depth);
    assert!(frame.nearer(3, 1, 100), "anything beats the open buffer");
    assert!(
        !frame.nearer(3, 1, i64::MAX),
        "never strictly nearer than i64::MAX"
    );
    assert!(
        !frame.nearer(4, 1, 100),
        "a column past the row's end names no slot"
    );
    assert!(!frame.nearer(0, 2, 100), "below the frame is nothing");
    assert!(!frame.nearer(0, -1, 100), "above the frame is nothing");
}

#[test]
fn a_span_edge_belongs_to_the_span_it_has_not_reached() {
    assert_eq!(
        strict_last_below(raw(4), raw(1)),
        3,
        "four whole spans: the last one starts at index 3"
    );
    assert_eq!(
        strict_last_below(raw(4).saturating_sub(1), raw(1)),
        3,
        "one fraction short of a boundary has passed the previous one"
    );
    assert_eq!(
        strict_last_below(raw(1), raw(4)),
        0,
        "a quarter of the way in is still index 0"
    );
    assert_eq!(
        strict_last_below(0, raw(1)),
        -1,
        "at the very start no span lies wholly behind"
    );
    assert_eq!(
        strict_last_below(raw(-1), raw(1)),
        -2,
        "left of the start the index keeps counting down"
    );
}
