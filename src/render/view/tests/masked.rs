//! Masked middles: the key, mapping, depth and window a two-sided
//! crossing collects, and how the transparent pass then draws them: far
//! to near, depth-tested against the caster's buffer, with clear texels,
//! absent keys and rows outside the texture's height leaving what stands
//! behind.

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::box_render::{box_camera, box_map, box_scene};
use super::{frac, raw, solid};
use crate::gfx::image::IndexedImage;
use crate::render::canvas::Canvas;
use crate::render::light::WallDir;
use crate::render::scene::Scene;
use crate::render::sprites::{Frame, Pass, draw_transparent};
use crate::render::view::projection::Projection;
use crate::render::view::{MaskedMiddle, ViewCamera, render_view};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

/// [`box_map`] split by a two-sided line at `x = 256` running
/// `(256,256) -> (256,0)`: sidedef 4 (sector 0) carries the `middle`
/// texture with an 8-unit y shift, sidedef 5 opens sector 1, whose
/// planes match sector 0's, so the crossing is transparent with no
/// upper or lower band.
fn divider_map(middle: &str) -> Map {
    let mut map = box_map();
    map.vertices.extend([
        Vertex {
            x: 256,
            y: 256,
            ..Default::default()
        },
        Vertex {
            x: 256,
            y: 0,
            ..Default::default()
        },
    ]);
    map.linedefs.push(Linedef {
        v1: 5,
        v2: 6,
        flags: 1_u32,
        front: Some(4),
        back: Some(5),
        ..Default::default()
    });
    map.sidedefs.push(Sidedef {
        upper: "-".to_owned(),
        lower: "-".to_owned(),
        middle: middle.to_owned(),
        sector: 0,
        y_offset: 8,
        ..Default::default()
    });
    map.sidedefs.push(Sidedef {
        upper: "-".to_owned(),
        lower: "-".to_owned(),
        middle: "-".to_owned(),
        sector: 1,
        ..Default::default()
    });
    map.sectors.push(Sector {
        floor_height: 0,
        ceiling_height: 128,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "CEIL3".to_owned(),
        light: 255,
        ..Default::default()
    });
    map
}

/// [`box_scene`] with the [`divider_map`]'s split and a 100x128
/// `MIDBAR` texture to go through it.
fn divider_scene(middle: &str) -> Scene {
    let mut scene = box_scene();
    scene.map = divider_map(middle);
    scene
        .textures
        .insert("MIDBAR".to_owned(), solid(100, 128, 90));
    scene
}

/// The camera straight down [`divider_scene`]'s middle, due east
/// from `(64, 128)` at eye 41.
fn divider_camera() -> ViewCamera {
    ViewCamera {
        y: 128,
        ..box_camera()
    }
}

#[test]
fn a_masked_middle_carries_its_key_mapping_depth_and_window() {
    let scene = divider_scene("MIDBAR");
    let rendered = render_view(&scene, &divider_camera(), 33, 64);
    let entries: Vec<_> = rendered
        .masked
        .iter()
        .filter(|entry| entry.column == 16)
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "the divider is the centre column's only transparent crossing"
    );
    let entry = entries.first().expect("the centre crossing");
    assert_eq!(entry.line, 4, "the crossing is the divider");
    assert_eq!(entry.side, 4, "faced from its front side");
    assert_eq!(entry.texture, "MIDBAR", "the scene-textures key");
    assert_eq!(
        entry.texcol, 28,
        "the mid-segment 128 units along wraps inside the 100-wide bar"
    );
    assert_eq!(entry.tex_top16, raw(128), "pegged to the lower ceiling");
    assert_eq!(entry.y_off16, raw(8), "the sidedef's vertical shift");
    assert_eq!(entry.depth16, raw(192), "t * focal2 is the depth");
    assert_eq!(
        entry.t.cmp_value(frac(64, 11)),
        Ordering::Equal,
        "the ray parameter stays exact"
    );
    assert_eq!(entry.light, 255_i32, "the faced sector's light");
    assert_eq!(entry.dir, WallDir::NorthSouth, "the divider's contrast");
    assert_eq!((entry.top, entry.bottom), (24, 35), "the open window");
}

#[test]
fn a_two_sided_line_with_no_or_missing_middle_collects_nothing() {
    for middle in ["-", "NOSUCH"] {
        let scene = divider_scene(middle);
        let rendered = render_view(&scene, &divider_camera(), 33, 64);
        assert!(
            rendered.masked.is_empty(),
            "`{middle}` on a two-sided line collects nothing"
        );
    }
}

/// A synthetic single-row masked middle at `column`/`row`, drawing
/// texture `texture`'s texel column `texcol`: hung from world height
/// 128 with no vertical shift, at depth 50 map units. Under a level
/// [`box_camera`] projection with `t = 1` that row reads texture row
/// `128 - (41 + 2 * (32 - row))`, so row 32 lands on texel row 87 of
/// a 128-tall image.
fn masked_at(column: u32, row: i64, texture: &str, texcol: u32) -> MaskedMiddle {
    MaskedMiddle {
        column,
        line: 0,
        side: 0,
        top: row,
        bottom: row,
        depth16: raw(50),
        texture: texture.to_owned(),
        texcol,
        tex_top16: raw(128),
        y_off16: 0,
        light: 255,
        dir: WallDir::Other,
        t: frac(1, 1),
    }
}

/// A `width` by `height` indexed image of solid `index`, opaque
/// before texel column `clear_from` and transparent from it rightwards.
fn clear_columns(width: u32, height: u32, index: u8, clear_from: u32) -> IndexedImage {
    let index_row = vec![index; usize::try_from(width).expect("test widths fit")];
    let opaque_row: Vec<bool> = (0..width).map(|column| column < clear_from).collect();
    let mut indices = Vec::new();
    let mut opaque = Vec::new();
    for _row in 0..height {
        indices.extend_from_slice(&index_row);
        opaque.extend_from_slice(&opaque_row);
    }
    IndexedImage::new(width, height, indices, opaque).expect("test images build")
}

/// The [`box_camera`] projection over a 32x64 raster, for the
/// transparent pass's row maths.
fn pass_projection() -> Projection {
    Projection::new(&box_camera(), 32, 64)
}

/// Draws `masked` through the merged transparent pass with no sprites
/// in the list, and answers with the screen columns it painted: the
/// column count these fixtures check.
fn draw_masked(pass: &Pass<'_>, frame: &mut Frame<'_>, masked: &mut [MaskedMiddle]) -> usize {
    draw_transparent(pass, frame, masked, &mut []).masked_columns
}

#[test]
fn the_view_reports_what_the_transparent_pass_covered() {
    let scene = divider_scene("MIDBAR");
    let rendered = render_view(&scene, &divider_camera(), 33, 64);
    assert!(
        rendered.masked_columns >= 1,
        "the divider's masked middles reached the pass: {}",
        rendered.masked_columns
    );
    assert_eq!(
        rendered.sprites_drawn, 0,
        "no caller builds sprite items yet, so the count stays zero"
    );
}

#[test]
fn masked_items_paint_far_to_near_and_the_nearer_wins_a_shared_pixel() {
    let mut scene = box_scene();
    scene
        .textures
        .insert("FARBAR".to_owned(), solid(64, 128, 222));
    scene
        .textures
        .insert("NEARBAR".to_owned(), solid(64, 128, 111));
    let proj = pass_projection();
    // The nearer item comes first in the list: only the far-to-near
    // sort can leave it on top of the shared pixel.
    let mut masked = vec![
        masked_at(16, 32, "NEARBAR", 0),
        masked_at(16, 32, "FARBAR", 0),
    ];
    masked[1].depth16 = raw(80);
    let mut canvas = Canvas::new(32, 64, [9_u8, 9, 9, 255]);
    let depth = vec![i64::MAX; 32_usize.saturating_mul(64)];
    let painted = draw_masked(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &depth),
        &mut masked,
    );
    assert_eq!(
        masked.first().map(|entry| entry.depth16),
        Some(raw(80)),
        "the list comes back in the order it was drawn"
    );
    assert_eq!(
        canvas.pixel(16, 32),
        Some([111_u8, 144, 111, 255]),
        "the nearer bar covers the farther one"
    );
    assert_eq!(painted, 1, "one column gained at least one pixel");
}

#[test]
fn masked_pixels_respect_the_depth_buffer_without_writing_it() {
    let mut scene = box_scene();
    scene
        .textures
        .insert("NEARBAR".to_owned(), solid(64, 128, 111));
    let proj = pass_projection();
    let near_slot = 32_usize.saturating_mul(32).saturating_add(16);
    let front_slot = 30_usize.saturating_mul(32).saturating_add(16);
    let mut canvas = Canvas::new(32, 64, [9_u8, 9, 9, 255]);
    let mut depth = vec![60_i64; 32_usize.saturating_mul(64)];
    depth[near_slot] = 40;
    let mut behind = vec![masked_at(16, 32, "NEARBAR", 0)];
    assert_eq!(
        draw_masked(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &depth),
            &mut behind,
        ),
        0,
        "the recorded 40 is nearer than the item's 50"
    );
    assert_eq!(
        canvas.pixel(16, 32),
        Some([9_u8, 9, 9, 255]),
        "behind an opaque wall pixel nothing paints"
    );
    let mut in_front = vec![masked_at(16, 30, "NEARBAR", 0)];
    assert_eq!(
        draw_masked(
            &Pass::new(&scene, &proj),
            &mut Frame::new(&mut canvas, &depth),
            &mut in_front,
        ),
        1,
        "the recorded 60 is farther than the item's 50"
    );
    assert_eq!(
        canvas.pixel(16, 30),
        Some([111_u8, 144, 111, 255]),
        "in front of the wall the bar paints"
    );
    assert_eq!(depth[near_slot], 40, "the pass never writes the buffer");
    assert_eq!(depth[front_slot], 60, "not the pixel it covered either");
}

#[test]
fn clear_texels_and_absent_keys_leave_the_pixels_behind_them() {
    let mut scene = box_scene();
    scene
        .textures
        .insert("HALFBAR".to_owned(), clear_columns(4, 128, 111, 2));
    let proj = pass_projection();
    let mut canvas = Canvas::new(32, 64, [9_u8, 9, 9, 255]);
    let depth = vec![i64::MAX; 32_usize.saturating_mul(64)];
    let mut masked = vec![
        masked_at(8, 32, "HALFBAR", 0),
        masked_at(16, 32, "HALFBAR", 2),
        masked_at(24, 32, "GHOSTBAR", 0),
    ];
    let painted = draw_masked(
        &Pass::new(&scene, &proj),
        &mut Frame::new(&mut canvas, &depth),
        &mut masked,
    );
    assert_eq!(painted, 1, "only the opaque texel's column counts");
    assert_eq!(
        canvas.pixel(8, 32),
        Some([111_u8, 144, 111, 255]),
        "texel column 0 is opaque and paints"
    );
    assert_eq!(
        canvas.pixel(16, 32),
        Some([9_u8, 9, 9, 255]),
        "texel column 2 is a hole: what stands behind shows through"
    );
    assert_eq!(
        canvas.pixel(24, 32),
        Some([9_u8, 9, 9, 255]),
        "a texture key the scene lacks draws nothing"
    );
}

/// [`divider_map`] with a `64x4` `SHORTBAR` middle and an `F_SKY1`
/// on both sectors' ceilings: the window rows then stand on
/// infinitely deep sky (or the far wall), so only the texture's own
/// four rows can hide a texel, never the depth test or a wrap.
fn shortbar_scene() -> Scene {
    let mut scene = box_scene();
    scene.map = divider_map("SHORTBAR");
    scene.sky = solid(256, 128, 200);
    for entry in &mut scene.map.sectors {
        entry.ceiling_flat = "F_SKY1".to_owned();
    }
    scene
        .textures
        .insert("SHORTBAR".to_owned(), solid(64, 4, 90));
    scene
}

#[test]
fn masked_rows_outside_the_texture_height_keep_what_stands_behind() {
    let scene = shortbar_scene();
    let rendered = render_view(&scene, &divider_camera(), 33, 64);
    // Centre column: the bar hangs from world height 136 at
    // t = 64/11, so window rows 24..=35 read texture rows
    // 1, 13, ..., 129 - only row 24 lands inside the four.
    assert_eq!(
        rendered.canvas.pixel(16, 24),
        Some([90_u8, 165, 90, 255]),
        "the one in-range row paints the bar"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 25),
        Some([200_u8, 55, 200, 255]),
        "row 25 reads texture row 13: a tiling pass would wrap to texel 1 and paint"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 30),
        Some([77_u8, 178, 77, 255]),
        "the far east wall mid-window is deep enough to paint over, and stays, at texture row 72"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 35),
        Some([1_u8, 254, 1, 255]),
        "the window's last row, texture row 129, keeps the floor"
    );
    assert!(
        rendered.masked_columns >= 1,
        "column 16 gained pixels and is counted: {}",
        rendered.masked_columns
    );
}
