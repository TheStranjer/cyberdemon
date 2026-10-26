//! The bucketed column walk: on a many-room map the caster tests far
//! fewer edges per column than the width times the edge count, every
//! column's bucketed hits still match a brute-force scan of all edges
//! exactly, and a column outside the buckets falls back to that scan.

use alloc::vec;
use alloc::vec::Vec;

use super::box_render::box_scene;
use crate::render::canvas::Canvas;
use crate::render::scene::Scene;
use crate::render::view::ViewCamera;
use crate::render::view::caster::Caster;
use crate::render::view::projection::Fraction;
use crate::render::view::texels::{edge, pierce, sky_column};
use crate::render::view::walk::{Edge, Hit, Walk, Window};
use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The grid's room side in map units.
const ROOM: i32 = 64;
/// The grid's room pitch in map units: the rooms stand apart, so none
/// shares a wall with another.
const PITCH: i32 = 256;
/// The raster width the work counter and parity scans cast over.
const WIDTH: u32 = 160;
/// The raster height.
const HEIGHT: u32 = 100;
/// The corners of one room, relative to its origin.
const CORNERS: [(i32, i32); 4] = [(0, 0), (ROOM, 0), (ROOM, ROOM), (0, ROOM)];
/// The four walls of one room as corner-index pairs.
const WALLS: [(usize, usize); 4] = [(0, 1), (1, 2), (2, 3), (3, 0)];

/// The camera at the centre of the grid's first room (`ROOM / 2`), due
/// east at eye 41.
fn grid_camera() -> ViewCamera {
    ViewCamera {
        x: ROOM.checked_div(2).expect("test rooms are even"),
        y: ROOM.checked_div(2).expect("test rooms are even"),
        z: 41,
        angle: 0,
        pitch: 0,
        fov: 90,
    }
}

/// A `side` by `side` grid of detached 64x64 one-sector rooms, each
/// walled by four one-sided `WALL1` lines.
fn grid_map(side: usize) -> Map {
    let name = LumpName::parse("MAP01").expect("test names are valid");
    let mut map = Map::new_empty(name, MapFormat::Doom);
    for row in 0..side {
        for column in 0..side {
            let x = i32::try_from(row)
                .expect("test grids fit")
                .saturating_mul(PITCH);
            let y = i32::try_from(column)
                .expect("test grids fit")
                .saturating_mul(PITCH);
            add_room(&mut map, x, y);
        }
    }
    map
}

/// Appends one [`ROOM`]x[`ROOM`] one-sector room standing at `(x, y)`,
/// walled by four one-sided `WALL1` lines facing inward.
fn add_room(map: &mut Map, x: i32, y: i32) {
    let first_vertex = map.vertices.len();
    let first_side = map.sidedefs.len();
    let sector = map.sectors.len();
    map.sectors.push(Sector {
        floor_height: 0,
        ceiling_height: 128,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "CEIL3".to_owned(),
        light: 255,
        ..Default::default()
    });
    for (dx, dy) in CORNERS {
        map.vertices.push(Vertex {
            x: x.saturating_add(dx),
            y: y.saturating_add(dy),
            ..Default::default()
        });
    }
    for (a, b) in WALLS {
        map.sidedefs.push(Sidedef {
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: "WALL1".to_owned(),
            sector,
            ..Default::default()
        });
        map.linedefs.push(Linedef {
            v1: first_vertex.saturating_add(a),
            v2: first_vertex.saturating_add(b),
            flags: 1_u32,
            front: Some(first_side.saturating_add(a)),
            ..Default::default()
        });
    }
}

/// [`box_scene`] over a [`grid_map`]: the `WALL1`, flats and fullbright
/// lighting all stand.
fn grid_scene(side: usize) -> Scene {
    let mut scene = box_scene();
    scene.map = grid_map(side);
    scene
}

/// The comparable key of a hit: what the buckets must reproduce
/// exactly, edge for edge and fraction for fraction.
type HitKey = (usize, Fraction, Fraction, bool, usize, Option<usize>);

/// The key of one hit.
fn key(hit: &Hit) -> HitKey {
    (hit.edge, hit.t, hit.s, hit.front, hit.side, hit.opposite)
}

/// The prepared edges of `scene`'s map, in the order the caster builds.
fn prepared_edges(scene: &Scene) -> Vec<Edge> {
    scene
        .map
        .linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| edge(index, &scene.map, line))
        .collect()
}

/// The cast without buckets: every edge pierced, the same sort.
fn brute(edges: &[Edge], cam: (i64, i64), d: (i128, i128)) -> Vec<Hit> {
    let mut found: Vec<Hit> = edges
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let (t, s, front) = pierce(cam, d, item)?;
            let side = if front { item.front } else { item.back? };
            let opposite = if front { item.back } else { Some(item.front) };
            Some(Hit {
                edge: index,
                t,
                s,
                front,
                side,
                opposite,
            })
        })
        .collect();
    found.sort_unstable_by(|a, b| a.t.cmp_value(b.t).then(a.edge.cmp(&b.edge)));
    found
}

/// Casts every column of the [`WIDTH`]x[`HEIGHT`] raster through
/// [`Caster::draw_column`], the way `render_frame` does.
fn cast_all(caster: &Caster<'_>, scene: &Scene, height_rows: i64) {
    let mut canvas = Canvas::new(WIDTH, HEIGHT, caster.background);
    let pixels = WIDTH.saturating_mul(HEIGHT);
    let mut depth = vec![i64::MAX; usize::try_from(pixels).expect("test sizes fit")];
    let mut masked = Vec::new();
    for column in 0_i64..i64::from(WIDTH) {
        let d = caster.proj.direction(column);
        let mut walk = Walk {
            canvas: &mut canvas,
            masked: &mut masked,
            depth: &mut depth,
            column,
            d,
            skycol: sky_column(d, scene.sky.width),
            window: Window::open(height_rows),
        };
        caster.draw_column(&mut walk);
    }
}

#[test]
fn a_many_room_map_drops_most_of_the_per_column_edge_tests() {
    let scene = grid_scene(5);
    let edges = prepared_edges(&scene);
    assert_eq!(edges.len(), 100, "25 detached rooms of four walls prepare");
    let caster = Caster::new(&scene, &grid_camera(), WIDTH, HEIGHT);
    cast_all(&caster, &scene, i64::from(HEIGHT));
    let naive =
        u64::from(WIDTH).saturating_mul(u64::try_from(edges.len()).expect("test edge counts fit"));
    let tested = caster.edge_tests();
    assert!(tested > 0, "the walls ahead of the camera are still tested");
    assert!(
        tested.saturating_mul(16) < naive,
        "the buckets cut the work to well under a sixteenth of the scan: {tested} of {naive}"
    );
}

#[test]
fn every_bucketed_column_returns_exactly_the_brute_force_hits() {
    let scene = grid_scene(5);
    let camera = grid_camera();
    let cam = (i64::from(camera.x), i64::from(camera.y));
    let edges = prepared_edges(&scene);
    let caster = Caster::new(&scene, &camera, WIDTH, HEIGHT);
    let mut with_hits = 0_usize;
    for column in 0_i64..i64::from(WIDTH) {
        let d = caster.proj.direction(column);
        let bucketed: Vec<_> = caster.hits(column, d).iter().map(key).collect();
        let scanned: Vec<_> = brute(&edges, cam, d).iter().map(key).collect();
        assert_eq!(bucketed, scanned, "column {column} must keep the same walk");
        if !bucketed.is_empty() {
            with_hits = with_hits.saturating_add(1);
        }
    }
    assert!(with_hits > 0, "the grid ahead of the camera really is hit");
}

#[test]
fn a_column_outside_the_buckets_falls_back_to_the_full_scan() {
    let scene = grid_scene(5);
    let camera = grid_camera();
    let cam = (i64::from(camera.x), i64::from(camera.y));
    let edges = prepared_edges(&scene);
    let caster = Caster::new(&scene, &camera, WIDTH, HEIGHT);
    for column in [-1_i64, i64::from(WIDTH), i64::MAX] {
        let d = caster.proj.direction(column);
        let bucketed: Vec<_> = caster.hits(column, d).iter().map(key).collect();
        let scanned: Vec<_> = brute(&edges, cam, d).iter().map(key).collect();
        assert_eq!(bucketed, scanned, "column {column} falls back whole");
    }
}
