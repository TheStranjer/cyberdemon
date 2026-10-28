//! The per-operation picture index: one build serves a whole scene render
//! (the counting guard), a shared patch component decodes once per
//! operation, a sprite rotation sweep re-reads nothing, and a refused
//! definition read reports itself on every call of the operation rather
//! than being cached away.

use std::collections::HashMap;

use super::{code, definition, lump, name, place, pnames_lump, session, texture_bytes, with_extra};
use crate::defs::definitions_for;
use crate::render::{RenderSettings, build_scene};
use crate::session::Session;
use crate::session::resolve::{PictureIndex, load_sprite_cached, load_texture};
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex, write_map};
use crate::wad::name::LumpName;

/// A parsed fixture name.
fn parsed(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A one-room map whose four walls name the two fixture textures and whose
/// planes carry the fixture flat.
fn dressed_map() -> Map {
    let mut map = Map::new_empty(parsed("MAP01"), MapFormat::Doom);
    let corners: [(i32, i32); 4] = [(0, 0), (64, 0), (64, 64), (0, 64)];
    map.vertices = corners
        .into_iter()
        .map(|(x, y)| Vertex {
            x,
            y,
            ..Default::default()
        })
        .collect();
    map.linedefs = (0..4_usize)
        .map(|edge| Linedef {
            v1: edge,
            v2: edge
                .checked_add(1)
                .expect("four walls")
                .checked_rem(4)
                .expect("four walls"),
            flags: 1,
            front: Some(edge),
            ..Default::default()
        })
        .collect();
    map.sidedefs = (0..4_usize)
        .map(|edge| Sidedef {
            middle: if edge.checked_rem(2).expect("even count") == 0 {
                "TWALL".to_owned()
            } else {
                "TWALL2".to_owned()
            },
            sector: 0,
            ..Default::default()
        })
        .collect();
    map.sectors = vec![Sector {
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "FLOOR1".to_owned(),
        ..Default::default()
    }];
    map
}

/// The fixture session dressed for a scene: a `PNAMES` list and a
/// `TEXTURE1` whose two records both draw the single `WALLP1` patch (one
/// of them twice), plus one map block that names them and the flat.
fn dressed_session() -> Session {
    let textures = texture_bytes(vec![
        definition("TWALL", 8, 4, vec![place(0, 0, 0), place(0, 4, 0)]),
        definition("TWALL2", 4, 4, vec![place(0, 0, 0)]),
    ]);
    let mut extra = vec![pnames_lump(&["WALLP1"]), lump("TEXTURE1", textures)];
    extra.extend(write_map(&dressed_map()).expect("the fixture map serializes"));
    with_extra(extra)
}

#[test]
fn one_scene_render_builds_exactly_one_picture_index() {
    let session = dressed_session();
    assert_eq!(
        session.picture_index_count(),
        0,
        "the fixture build indexed nothing"
    );
    let scene = build_scene(&session, parsed("MAP01"), &RenderSettings::default())
        .expect("the dressed fixture builds a scene");
    assert_eq!(
        session.picture_index_count(),
        1,
        "one index serves the whole render"
    );
    let wall = scene.textures.get("TWALL").expect("TWALL composed");
    assert_eq!(wall.get(3, 3), Some((0x11, true)), "the first tile drew");
    assert_eq!(wall.get(5, 0), Some((0x11, true)), "the second tile drew");
    let second = scene.textures.get("TWALL2").expect("TWALL2 composed");
    assert_eq!(
        second.get(0, 0),
        Some((0x11, true)),
        "its shared patch drew"
    );
    assert_eq!(
        scene.flats.get("FLOOR1").and_then(|flat| flat.get(0, 0)),
        Some((2, true)),
        "the last FLOOR1 copy won, as always"
    );
    assert!(
        !scene
            .problems
            .iter()
            .any(|problem| problem.starts_with("missing")),
        "every picture the map names loaded: {:?}",
        scene.problems
    );
    build_scene(&session, parsed("MAP01"), &RenderSettings::default())
        .expect("the scene builds again");
    assert_eq!(
        session.picture_index_count(),
        2,
        "the second render builds its own index"
    );
}

#[test]
fn a_shared_patch_component_decodes_once_per_operation() {
    let session = dressed_session();
    let index = PictureIndex::new(&session).expect("the fixture indexes");
    assert_eq!(index.decode_count(), 0, "nothing decoded yet");
    let first = load_texture(&index, name("TWALL")).expect("TWALL composes");
    assert_eq!(
        index.decode_count(),
        1,
        "WALLP1, placed twice, decoded once"
    );
    let second = load_texture(&index, name("TWALL2")).expect("TWALL2 composes");
    assert_eq!(
        index.decode_count(),
        1,
        "the second texture reused the cached WALLP1"
    );
    assert_eq!(first.image.get(0, 0), Some((0x11, true)));
    assert_eq!(second.image.get(3, 3), Some((0x11, true)));
}

#[test]
fn one_index_serves_a_whole_sprite_rotation_sweep() {
    let session = session();
    let index = PictureIndex::new(&session).expect("the fixture indexes");
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let imp = defs.thing(3001).expect("the embedded imp");
    let mut cache = HashMap::new();
    for (rotation, lump_name, mirrored) in [
        (1_u8, "TROOA1", false),
        (2_u8, "TROOA2A8", false),
        (8_u8, "TROOA2A8", true),
    ] {
        let sprite = load_sprite_cached(&index, imp, rotation, &mut cache)
            .expect("the fixture serves the rotation");
        assert_eq!(sprite.lump, name(lump_name), "rotation {rotation}");
        assert_eq!(sprite.mirrored, mirrored, "rotation {rotation} serving");
    }
    assert_eq!(
        session.picture_index_count(),
        1,
        "the sweep indexed the view once and rebuilt nothing"
    );
}

#[test]
fn a_refused_definition_lump_reports_on_every_texture_lookup() {
    let broken = with_extra(vec![
        pnames_lump(&["WALLP1"]),
        lump("TEXTURE1", vec![b'?'; 40]),
    ]);
    let index = PictureIndex::new(&broken).expect("the fixture indexes");
    for _attempt in 0_usize..2_usize {
        let error = load_texture(&index, name("ANY")).expect_err("junk is no texture lump");
        assert_eq!(code(&error), "format_error", "the refusal repeats");
    }
}
