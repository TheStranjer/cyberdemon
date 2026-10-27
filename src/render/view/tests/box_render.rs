//! The box map end to end: where its walls, floor and ceiling land, how
//! the floor samples its flat on the world grid, how the depth buffer
//! grows with distance up a column, what an `F_SKY1` ceiling and floor
//! paint, and what the background keeps where no wall reaches.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use super::{fixture_palette, identity_table, solid};
use crate::gfx::image::IndexedImage;
use crate::render::scene::Scene;
use crate::render::settings::{Lighting, RenderSettings, Switch};
use crate::render::view::{ViewCamera, render_view};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The box's geometry: a 512x256 one-sector footprint, all four
/// sidedefs `WALL1`-middle facing inward.
pub(super) fn box_map() -> Map {
    let name = LumpName::parse("MAP01").expect("test names are valid");
    let mut map = Map::new_empty(name, MapFormat::Doom);
    map.vertices = [
        (0_i32, 0_i32),
        (0_i32, 256_i32),
        (512_i32, 256_i32),
        (512_i32, 0_i32),
        (0_i32, 0_i32),
    ]
    .into_iter()
    .map(|(x, y)| Vertex {
        x,
        y,
        ..Default::default()
    })
    .collect();
    map.linedefs = (0..4_usize)
        .map(|index| Linedef {
            v1: index,
            v2: index.saturating_add(1),
            flags: 1_u32,
            front: Some(index),
            ..Default::default()
        })
        .collect();
    map.sidedefs = (0..4_usize)
        .map(|_| Sidedef {
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: "WALL1".to_owned(),
            sector: 0,
            ..Default::default()
        })
        .collect();
    map.sectors = vec![Sector {
        floor_height: 0,
        ceiling_height: 128,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "CEIL3".to_owned(),
        light: 255,
        ..Default::default()
    }];
    map
}

/// A fullbright scene over [`box_map`]: `WALL1` middles (index 77),
/// `FLOOR1` (index 1), `CEIL3` (index 200) and an identity `COLORMAP`.
pub(super) fn box_scene() -> Scene {
    let mut textures = BTreeMap::new();
    textures.insert("WALL1".to_owned(), solid(64, 128, 77));
    let mut flats = BTreeMap::new();
    flats.insert("FLOOR1".to_owned(), solid(64, 64, 1));
    flats.insert("CEIL3".to_owned(), solid(64, 64, 200));
    let map = box_map();
    let geometry = SectorIndex::new(&map);
    Scene {
        map,
        geometry,
        palette: fixture_palette(),
        colormaps: vec![identity_table()],
        textures,
        flats,
        sky: solid(64, 64, 0),
        sky_flat: "F_SKY1".to_owned(),
        settings: RenderSettings {
            lighting: Lighting::Fullbright,
            ..RenderSettings::default()
        },
        problems: Vec::new(),
    }
}

/// The east-facing camera the box is checked from: 32x64, fov 90,
/// eye 41 at `(64, 96)`.
pub(super) fn box_camera() -> ViewCamera {
    ViewCamera {
        x: 64,
        y: 96,
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    }
}

#[test]
fn a_box_render_lands_walls_floor_and_ceiling_where_the_math_says() {
    let scene = box_scene();
    let rendered = render_view(&scene, &box_camera(), 32, 64);
    assert!(rendered.masked.is_empty(), "one-sided lines mask nothing");
    let canvas = &rendered.canvas;
    assert_eq!(
        canvas.pixel(0, 63),
        Some([1_u8, 254, 1, 255]),
        "the bottom row is the solid FLOOR1 flat"
    );
    assert_eq!(
        canvas.pixel(0, 0),
        Some([200_u8, 55, 200, 255]),
        "the top of the north-leaning column is the solid CEIL3 flat"
    );
    assert_eq!(
        canvas.pixel(0, 30),
        Some([77_u8, 178, 77, 255]),
        "between its plane rows the north wall paints"
    );
    assert_eq!(
        canvas.pixel(16, 32),
        Some([77_u8, 178, 77, 255]),
        "the centre column at the horizon is the east wall"
    );
    assert_eq!(
        canvas.pixel(16, 0),
        Some([200_u8, 55, 200, 255]),
        "the centre column's ceiling projects inside the frame"
    );
}

/// A `64x64` two-tone flat: columns `0..32` index 1, `32..64` index 2.
fn half_flat() -> IndexedImage {
    let mut indices = Vec::new();
    for _row in 0..64_usize {
        indices.extend_from_slice(&[1_u8; 32]);
        indices.extend_from_slice(&[2_u8; 32]);
    }
    let opaque = vec![true; indices.len()];
    IndexedImage::new(64, 64, indices, opaque).expect("64x64 builds")
}

#[test]
fn the_floor_samples_its_flat_on_the_world_grid() {
    let mut scene = box_scene();
    scene.flats.insert("FLOOR1".to_owned(), half_flat());
    let camera = ViewCamera {
        pitch: -45,
        ..box_camera()
    };
    let rendered = render_view(&scene, &camera, 32, 64);
    // Centre column, eye 41 over floor 0: the row at `dy` steps below
    // the horizon (row 16) sits `41 * 32 / (2 * dy)` units east of x 64.
    assert_eq!(
        rendered.canvas.pixel(16, 18),
        Some([1_u8, 254, 1, 255]),
        "depth 328 lands on x 392, texel column 8"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 22),
        Some([2_u8, 253, 2, 255]),
        "depth 109 lands on x 173, texel column 45"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 30),
        Some([2_u8, 253, 2, 255]),
        "depth 47 lands on x 110, texel column 46"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 40),
        Some([1_u8, 254, 1, 255]),
        "depth 27 lands on x 91, texel column 27"
    );
}

#[test]
fn the_depth_buffer_grows_with_distance_up_the_column() {
    let scene = box_scene();
    let camera = ViewCamera {
        pitch: -45,
        ..box_camera()
    };
    let rendered = render_view(&scene, &camera, 32, 64);
    let centre = |row: usize| {
        *rendered
            .depth
            .get(row.saturating_mul(32).saturating_add(16))
            .expect("a frame pixel")
    };
    assert_eq!(centre(0), 87, "the ceiling 87 above the eye at 16 rows up");
    assert_eq!(
        centre(11),
        278,
        "climbing to the horizon the ceiling recedes"
    );
    assert_eq!(centre(17), 448, "the east wall's own depth covers its rows");
    assert_eq!(centre(18), 328, "the floor row just below the wall base");
    assert_eq!(centre(63), 13, "the near floor at the frame bottom");
    let mut previous = centre(63);
    for row in (18_usize..63_usize).rev() {
        let value = centre(row);
        assert!(
            value >= previous,
            "depth never shrinks climbing the floor run at row {row}"
        );
        previous = value;
    }
}

/// The box scene with its ceiling switched to the sky marker.
fn sky_box() -> Scene {
    let mut scene = box_scene();
    let sector = scene
        .map
        .sectors
        .get_mut(0)
        .expect("the box has one sector");
    sector.ceiling_flat = "F_SKY1".to_owned();
    sector.light = 0_i32;
    scene.sky = solid(256, 128, 200);
    scene
}

#[test]
fn a_sky_ceiling_paints_the_sky_texture_full_bright() {
    let mut scene = sky_box();
    scene.settings.lighting = Lighting::Flat;
    let rendered = render_view(&scene, &box_camera(), 32, 64);
    assert_eq!(
        rendered.canvas.pixel(0, 0),
        Some([200_u8, 55, 200, 255]),
        "a dark sector still gets the full-bright sky"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 0),
        Some([200_u8, 55, 200, 255]),
        "the centre column's sky band reads the same"
    );
    assert_eq!(
        *rendered.depth.get(16).expect("a frame pixel"),
        i64::MAX,
        "sky pixels stay infinitely deep"
    );
}

#[test]
fn a_sky_floor_paints_the_sky_too() {
    let mut scene = box_scene();
    scene.sky = solid(64, 128, 200);
    let sector = scene
        .map
        .sectors
        .get_mut(0)
        .expect("the box has one sector");
    sector.floor_flat = "F_SKY1".to_owned();
    let rendered = render_view(&scene, &box_camera(), 32, 64);
    assert_eq!(
        rendered.canvas.pixel(16, 63),
        Some([200_u8, 55, 200, 255]),
        "the F_SKY1 floor draws the sky, not the void"
    );
    assert_eq!(
        *rendered
            .depth
            .get(63_usize.saturating_mul(32).saturating_add(16))
            .expect("a frame pixel"),
        i64::MAX,
        "and it stays infinitely deep"
    );
}

#[test]
fn the_sky_switch_off_leaves_sky_rows_the_background() {
    let mut scene = sky_box();
    scene.settings.sky = Switch::new(false);
    let rendered = render_view(&scene, &box_camera(), 32, 64);
    assert_eq!(
        rendered.canvas.pixel(16, 0),
        Some([0_u8, 0, 0, 255]),
        "sky off: the ceiling rows keep the background colour"
    );
    assert_eq!(
        *rendered.depth.get(16).expect("a frame pixel"),
        i64::MAX,
        "and they keep the open depth"
    );
    assert_eq!(
        rendered.canvas.pixel(16, 32),
        Some([77_u8, 178, 77, 255]),
        "walls are untouched by the switch"
    );
}

#[test]
fn the_background_seals_the_frame_outside_every_wall() {
    let mut scene = box_scene();
    scene.map.linedefs.clear();
    scene.map.sidedefs.clear();
    let rendered = render_view(&scene, &box_camera(), 32, 64);
    assert_eq!(
        rendered.canvas.pixel(16, 32),
        Some([0_u8, 0, 0, 255]),
        "no lines means the plain background colour"
    );
    assert!(
        rendered.depth.iter().all(|slot| *slot == i64::MAX),
        "nothing painted means nothing nearer than the horizon"
    );
}
