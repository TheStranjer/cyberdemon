//! Projection tests: where a sprite patch lands on the raster, how its
//! offsets and the frame's edges clip it, which pixels the depth test
//! keeps and which the merged far-to-near order settles.

use alloc::vec;
use alloc::vec::Vec;

use super::{
    BLANK, CENTRE, EYE, HEIGHT, HORIZON, WIDTH, assert_each_texel_lands_on_its_own_pixel, camera,
    clear_columns, column_painted, columns_of, draw, fixture_scene, gradient, lit, masked_at,
    paint_sprite, painted, projection, raw, rows_of, solid, standing,
};
use crate::render::canvas::Canvas;
use crate::render::sprites::{Counts, Frame, Pass, SpriteItem, draw_transparent};
use crate::render::view::Projection;

#[test]
fn a_sprite_straight_ahead_is_placed_and_mapped_exactly() {
    // Depth 16 under focal2 32 puts k at one pixel per map unit, so
    // the 8x8 patch spans columns 12..20 and rows 24..32, each pixel
    // reading its own texel.
    let canvas = paint_sprite(standing(gradient(8, 8), 16, EYE));
    assert_eq!(
        columns_of(&canvas),
        (12_u32..20_u32).collect::<Vec<u32>>(),
        "centred on column 16"
    );
    assert_eq!(
        rows_of(&canvas),
        (24_u32..32_u32).collect::<Vec<u32>>(),
        "from the eye up by the patch's own height"
    );
    assert_each_texel_lands_on_its_own_pixel(&canvas);
}

#[test]
fn doubling_the_depth_halves_the_columns_and_the_height() {
    let near = paint_sprite(standing(gradient(8, 8), 16, EYE));
    let far = paint_sprite(standing(gradient(8, 8), 32, EYE));
    assert_eq!(columns_of(&near).len(), 8, "k = 1 eight texels across");
    assert_eq!(
        columns_of(&far),
        (14_u32..18_u32).collect::<Vec<u32>>(),
        "k = 1/2 half as many columns, still centred on 16"
    );
    assert_eq!(rows_of(&near).len(), 8, "eight rows near");
    assert_eq!(rows_of(&far).len(), 4, "half the height far");
}

#[test]
fn an_odd_raster_centres_the_sprite_on_its_middle_column() {
    // 33 columns: the centre is column 16 = 33 / 2 and, at depth 16,
    // k = 33/32, so the 8-unit patch spans 8.25 pixels symmetric about
    // that column and touches 11..=20.
    let scene = fixture_scene();
    let proj = Projection::new(&camera(), 33_u32, HEIGHT);
    let mut canvas = Canvas::new(33_u32, HEIGHT, BLANK);
    let slots = usize::try_from(33_u32.saturating_mul(HEIGHT)).expect("test sizes fit");
    let buffer = vec![i64::MAX; slots];
    let mut sprites = vec![standing(gradient(8, 8), 16, EYE)];
    let counts = draw_transparent(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &buffer),
        &mut [],
        &mut sprites,
    );
    assert_eq!(counts.sprites_drawn, 1, "the sprite paints");
    let columns: Vec<u32> = (0..33_u32)
        .filter(|column| column_painted(&canvas, *column))
        .collect();
    assert_eq!(
        columns,
        (11_u32..21_u32).collect::<Vec<u32>>(),
        "the pixels it touches are symmetric about column 16"
    );
}

#[test]
fn the_top_offset_places_the_patch_top_at_its_world_height() {
    // A 64-unit topoffset on base 0 tops the patch at world height 64;
    // at k = 1 that is row 32 - (64 - 41) = 9 and the 8-tall patch
    // runs down to row 17, never tiling past its own rows.
    let mut item = standing(solid(8, 8, 200), 16, 0);
    item.top_offset16 = raw(64);
    let canvas = paint_sprite(item);
    let rows = rows_of(&canvas);
    assert_eq!(rows.first(), Some(&9_u32), "the projected top row");
    assert_eq!(rows.last(), Some(&16_u32), "eight rows down");
    assert!(
        !painted(&canvas, CENTRE, HORIZON - 24),
        "row 8 is above the patch and keeps the background"
    );
    assert!(
        !painted(&canvas, CENTRE, HORIZON - 15),
        "row 17 is past the patch's last row"
    );
}

#[test]
fn a_sprite_behind_a_closer_wall_pixel_paints_nothing() {
    let scene = fixture_scene();
    let proj = projection();
    let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
    let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
    let wall = vec![10_i64; slots];
    let mut behind = vec![standing(solid(8, 8, 200), 20, EYE)];
    let counts = draw_transparent(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &wall),
        &mut [],
        &mut behind,
    );
    assert_eq!(
        counts,
        Counts::default(),
        "the recorded 10 is nearer than the sprite's 20, everywhere"
    );
    assert!(
        columns_of(&canvas).is_empty(),
        "behind an opaque wall nothing paints"
    );
}

#[test]
fn a_sprite_in_front_of_the_buffer_paints_and_leaves_it_untouched() {
    let scene = fixture_scene();
    let proj = projection();
    let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
    let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
    let open = vec![i64::MAX; slots];
    let before = open.clone();
    let mut in_front = vec![standing(solid(8, 8, 200), 20, EYE)];
    let counts = draw_transparent(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &open),
        &mut [],
        &mut in_front,
    );
    assert_eq!(
        counts.sprites_drawn, 1,
        "sky and background are `i64::MAX` deep, so the sprite is nearer"
    );
    assert!(
        painted(&canvas, CENTRE, 30),
        "in front of the wall the patch paints"
    );
    assert_eq!(open, before, "the pass never writes the buffer");
}

#[test]
fn depth_test_against_a_wall_pixel_keeps_the_wall_and_counts_both_layers() {
    // One pixel stands 15 units deep behind an opaque face: the masked
    // bar at 20 and the sprite at 30 both lose it, and both still
    // paint the open pixels around it.
    let scene = fixture_scene();
    let proj = projection();
    let slots = usize::try_from(WIDTH.saturating_mul(HEIGHT)).expect("test sizes fit");
    let mut buffer = vec![i64::MAX; slots];
    let wall = usize::try_from(30_u32.saturating_mul(WIDTH).saturating_add(CENTRE)).expect("fits");
    if let Some(slot) = buffer.get_mut(wall) {
        *slot = 15;
    }
    let mut canvas = Canvas::new(WIDTH, HEIGHT, BLANK);
    let mut bar = masked_at(CENTRE, 30, 20);
    bar.top = 29;
    let sprite = standing(solid(8, 8, 111), 30, EYE);
    let counts = draw_transparent(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &buffer),
        &mut [bar],
        &mut [sprite],
    );
    assert_eq!(
        canvas.pixel(CENTRE, 30),
        Some(BLANK),
        "the nearer wall keeps its own pixel"
    );
    assert_eq!(
        counts,
        Counts {
            masked_columns: 1,
            sprites_drawn: 1
        },
        "both layers painted their other pixels"
    );
}

#[test]
fn a_nearer_sprite_covers_a_masked_column_on_the_same_pixel() {
    let masked = masked_at(CENTRE, 30, 40);
    let sprite = standing(solid(8, 8, 111), 30, EYE);
    let (canvas, counts) = draw(&mut [masked], &mut [sprite], i64::MAX);
    assert_eq!(
        counts,
        Counts {
            masked_columns: 1,
            sprites_drawn: 1
        },
        "both layers gained pixels"
    );
    assert_eq!(
        canvas.pixel(CENTRE, 30),
        Some(lit(111)),
        "the nearer sprite covers the farther bar"
    );
}

#[test]
fn a_nearer_masked_column_covers_a_farther_sprite() {
    let masked = masked_at(CENTRE, 30, 20);
    let sprite = standing(solid(8, 8, 111), 30, EYE);
    let (canvas, counts) = draw(&mut [masked], &mut [sprite], i64::MAX);
    assert_eq!(
        canvas.pixel(CENTRE, 30),
        Some(lit(64)),
        "the nearer bar wins the shared pixel"
    );
    assert_eq!(counts.masked_columns, 1, "the masked column painted");
    assert_eq!(counts.sprites_drawn, 1, "and the sprite painted elsewhere");
}

#[test]
fn clear_texels_leave_the_pixels_behind_a_sprite() {
    // The 8-wide patch at k = 1 covers columns 12..20 and is opaque
    // over its first two texel columns, so only 12 and 13 paint.
    let canvas = paint_sprite(standing(clear_columns(8, 8, 111, 2), 16, EYE));
    assert_eq!(
        canvas.pixel(12, 24),
        Some(lit(111)),
        "texel column 0 is opaque and paints"
    );
    assert_eq!(canvas.pixel(13, 24), Some(lit(111)), "so is texel column 1");
    assert_eq!(
        canvas.pixel(14, 24),
        Some(BLANK),
        "texel column 2 is a hole: what stands behind shows through"
    );
    assert_eq!(
        canvas.pixel(19, 24),
        Some(BLANK),
        "and so are the rest of them"
    );
    assert_eq!(
        rows_of(&canvas),
        (24_u32..32_u32).collect::<Vec<u32>>(),
        "the opaque band runs the patch's full height"
    );
}

#[test]
fn a_sprite_wider_than_the_frame_is_clipped_to_it() {
    // The 40-unit patch at k = 1 spans columns -4..36: the frame sees
    // 0..32 of them, and each visible column still reads the texel its
    // own span ends in.
    let canvas = paint_sprite(standing(gradient(40, 4), 16, EYE));
    let all: Vec<u32> = (0..WIDTH).collect();
    assert_eq!(columns_of(&canvas), all, "the whole width");
    assert_eq!(
        rows_of(&canvas),
        (28_u32..32_u32).collect::<Vec<u32>>(),
        "four rows up from the eye"
    );
    assert_eq!(
        canvas.pixel(0, 28),
        Some(lit(4)),
        "column 0 shows texel 4: the patch starts four pixels off-screen"
    );
    assert_eq!(
        canvas.pixel(31, 28),
        Some(lit(35)),
        "the last column ends inside texel 35"
    );
}

#[test]
fn a_sprite_outside_the_frame_paints_nothing() {
    let far_right = SpriteItem {
        tx16: raw(1_000),
        ..standing(solid(8, 8, 200), 16, EYE)
    };
    let under_the_floor = standing(solid(8, 8, 200), 16, 1);
    let (canvas, counts) = draw(&mut [], &mut [far_right, under_the_floor], i64::MAX);
    assert_eq!(
        counts,
        Counts::default(),
        "neither a sprite off to the side nor one below the frame draws"
    );
    assert!(
        columns_of(&canvas).is_empty(),
        "and no pixel of the frame changed"
    );
}

#[test]
fn items_at_or_behind_the_camera_draw_nothing() {
    let behind = standing(solid(8, 8, 200), -16, EYE);
    let mut on_plane = behind.clone();
    on_plane.tz16 = 0_i128;
    let mut past_the_range = behind.clone();
    past_the_range.tz16 = i128::MAX;
    let (canvas, counts) = draw(&mut [], &mut [behind, on_plane, past_the_range], i64::MAX);
    assert_eq!(
        counts,
        Counts::default(),
        "nothing in the list gained a pixel"
    );
    assert!(
        columns_of(&canvas).is_empty(),
        "and no pixel changed either"
    );
}
