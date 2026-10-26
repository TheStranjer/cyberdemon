//! The `build_scene` preparation the 3D view runs on: the fixture `IWAD`
//! folding into a scene whose settings, palette, wall textures, flat and 34
//! `COLORMAP` tables answer what the map wears, with the fallback sky a map
//! showing no sky plane gets; the sky a map shows and the session lacks;
//! the checkerboard fallback and the report for textures the session cannot
//! compose; the `not_found` refusals for a session without a `PLAYPAL` and a
//! map it lacks; the generated `COLORMAP` when the lump is missing or too
//! short; and the Heretic session picking its episode sky.

use std::fs;
use std::path::Path;

use cyberdemon::error::CyberError;
use cyberdemon::render::{Lighting, RenderSettings};
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::{Lump, LumpName, Wad};

use crate::fixtures::{iwad, iwad_heretic, iwad_view_textures, two_room_textured_map};

/// A parsed test lump name.
fn parsed_name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A session over the fixture archive `file`, with every lump named in
/// `drop` removed and `extra` appended last.
fn fixture_session(file: &Path, drop: &[&str], extra: Vec<Lump>) -> cyberdemon::session::Session {
    let bytes = fs::read(file).expect("the fixture WAD is readable");
    let mut wad = Wad::parse(&bytes).expect("the fixture WAD parses");
    wad.lumps
        .retain(|lump| !drop.contains(&lump.name.to_string().as_str()));
    wad.lumps.extend(extra);
    let mut session = cyberdemon::session::Session::default();
    session
        .set_iwad(cyberdemon::session::Layer::new(
            file.to_path_buf(),
            cyberdemon::session::LayerRole::Iwad,
            wad,
        ))
        .expect("no batch is open");
    session
}

/// Builds a scene over `session` with the default settings.
fn scene_of(
    session: &cyberdemon::session::Session,
    map: &str,
) -> Result<cyberdemon::render::Scene, CyberError> {
    cyberdemon::render::build_scene(session, parsed_name(map), &RenderSettings::default())
}

/// The `code` of a [`CyberError`], for compact asserts.
fn error_code(error: &CyberError) -> &str {
    error.code()
}

/// The identity lighting table, for `COLORMAP` assertions.
fn identity_table() -> [u8; 256] {
    let mut table = [0_u8; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        *entry = u8::try_from(index).expect("the table covers exactly the u8 range");
    }
    table
}

/// The fixture map's two composed wall textures.
fn assert_wall_textures(scene: &cyberdemon::render::Scene) {
    let textures: Vec<&String> = scene.textures.keys().collect();
    assert_eq!(textures, ["STEP1", "WALL1"], "both mapped textures");
    assert_eq!(
        scene.textures["WALL1"].width, 64,
        "the WALL1 record's width"
    );
    assert_eq!(
        scene.textures["WALL1"].height, 128,
        "the WALL1 record's height"
    );
    assert_eq!(
        scene.textures["WALL1"].get(0, 0),
        Some((100, true)),
        "the tiled WALLP1 patch is opaque"
    );
    assert_eq!(
        scene.textures["STEP1"].height, 32,
        "the STEP1 record's height"
    );
    assert_eq!(
        scene.textures["STEP1"].get(0, 0),
        Some((150, true)),
        "its STEPP1 tiles drew too"
    );
}

/// The fallback sky a map that names no sky plane carries: it is never
/// sampled, and it is reported as no problem.
fn assert_fallback_sky(scene: &cyberdemon::render::Scene) {
    assert_eq!(scene.sky_flat, "F_SKY1", "the sky ceiling marker");
    assert_eq!(
        (scene.sky.width, scene.sky.height),
        (64, 64),
        "the fallback sky"
    );
    assert_eq!(scene.sky.get(0, 0), Some((0, true)), "brightest corner");
    assert_eq!(
        scene.sky.get(8, 0),
        Some((252, true)),
        "the darkest cell beside it (252..=255 tie at the lowest luminance)"
    );
}

#[test]
fn build_scene_folds_the_fixture_iwad_into_a_scene() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad(dir.path()), &[], Vec::new());
    let settings = RenderSettings {
        gamma: 2,
        lighting: Lighting::Fullbright,
        ..RenderSettings::default()
    };
    let scene = cyberdemon::render::build_scene(&session, parsed_name("MAP01"), &settings)
        .expect("the fixture scene builds");
    assert_eq!(scene.map.name.to_string(), "MAP01", "the map");
    assert_eq!(scene.settings, settings, "the settings ride along");
    assert_eq!(
        scene.palette.color(3),
        Some([3_u8, 252_u8, 3_u8]),
        "the palette"
    );
    assert_wall_textures(&scene);
    let flats: Vec<&String> = scene.flats.keys().collect();
    assert_eq!(flats, ["FLOOR1"], "the one flat both rooms use");
    assert_eq!(
        scene.flats["FLOOR1"].get(0, 0),
        Some((1, true)),
        "its pixels"
    );
    assert_eq!(scene.colormaps.len(), 34, "the 34 lighting tables");
    assert_eq!(scene.colormaps[0], identity_table(), "the fixture's map");
    assert_fallback_sky(&scene);
    assert_eq!(scene.problems, Vec::<String>::new(), "nothing is missing");
}

#[test]
fn build_scene_reports_a_sky_the_map_shows_and_the_session_lacks() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad_view_textures(dir.path()), &["SKY1"], Vec::new());
    let scene = scene_of(&session, "MAP01").expect("the sky room builds");
    assert!(
        scene.problems.contains(&"missing sky SKY1".to_owned()),
        "room 1's F_SKY1 ceiling asks for the sky the session has not: {:?}",
        scene.problems
    );
    assert_eq!(
        scene.sky.width, 64,
        "the sky falls back to the checkerboard"
    );
}

#[test]
fn build_scene_falls_back_for_textures_the_session_cannot_compose() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad(dir.path()), &["TEXTURE1"], Vec::new());
    let scene = scene_of(&session, "MAP01").expect("the scene still builds without records");
    assert!(
        scene.problems.contains(&"missing texture WALL1".to_owned())
            && scene.problems.contains(&"missing texture STEP1".to_owned()),
        "both names are reported: {:?}",
        scene.problems
    );
    let wall = &scene.textures["WALL1"];
    assert_eq!((wall.width, wall.height), (64, 64), "the checkerboard size");
    assert_eq!(wall.get(0, 0), Some((0, true)), "brightest cell");
    assert_eq!(wall.get(8, 8), Some((0, true)), "the diagonal cell matches");
    assert_eq!(
        wall.get(8, 0),
        Some((252, true)),
        "the orthogonal cell differs"
    );
    assert_eq!(
        scene.flats["FLOOR1"].get(0, 0),
        Some((1, true)),
        "the flats are untouched"
    );
}

#[test]
fn build_scene_refuses_a_session_without_a_playpal() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad(dir.path()), &["PLAYPAL"], Vec::new());
    let error = scene_of(&session, "MAP01").expect_err("rendering needs a palette");
    assert_eq!(error_code(&error), "not_found", "missing PLAYPAL code");
    assert!(
        error.to_string().contains("PLAYPAL"),
        "the refusal names the lump: {error}"
    );
}

#[test]
fn build_scene_refuses_a_map_the_session_lacks() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad(dir.path()), &[], Vec::new());
    let error = scene_of(&session, "MAP99").expect_err("the fixture has no MAP99");
    assert_eq!(error_code(&error), "not_found", "missing map code");
    assert!(
        error.to_string().contains("MAP99"),
        "the refusal names the map: {error}"
    );
}

#[test]
fn build_scene_generates_a_colormap_when_the_lump_is_missing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let session = fixture_session(&iwad(dir.path()), &["COLORMAP"], Vec::new());
    let scene = scene_of(&session, "MAP01").expect("the scene builds");
    assert_eq!(scene.colormaps.len(), 34, "the generated 34 tables");
    assert_eq!(scene.colormaps[0], identity_table(), "map 0 is identity");
    assert_ne!(scene.colormaps[1][255], 255, "map 1 darkens the top colour");
    assert!(
        scene.problems.contains(&"generated COLORMAP".to_owned()),
        "the generation is reported: {:?}",
        scene.problems
    );
}

#[test]
fn build_scene_regenerates_a_too_short_colormap() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let base = iwad(dir.path());
    let stub = vec![Lump::new(parsed_name("COLORMAP"), vec![0_u8; 10])];
    let session = fixture_session(&base, &["COLORMAP"], stub);
    let scene = scene_of(&session, "MAP01").expect("a stub COLORMAP still builds a scene");
    assert_eq!(scene.colormaps.len(), 34, "the replacement has 34 tables");
    assert!(
        scene.problems.contains(&"generated COLORMAP".to_owned()),
        "the 10-byte lump is refused and regenerated: {:?}",
        scene.problems
    );
}

#[test]
fn a_heretic_session_picks_its_episode_sky_and_reports_missing_pictures() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut outdoors = two_room_textured_map();
    outdoors.name = parsed_name("E2M1");
    let mut extra = write_doom_map(&outdoors).expect("the sky map serializes");
    extra.push(Lump::new(parsed_name("SKY2"), vec![7_u8; 4_096]));
    let session = fixture_session(&iwad_heretic(dir.path()), &[], extra);
    let scene = scene_of(&session, "E2M1").expect("the Heretic scene builds");
    assert_eq!(scene.sky.get(0, 0), Some((7, true)), "E2M1 skies over SKY2");
    assert!(
        !scene
            .problems
            .iter()
            .any(|problem| problem.starts_with("missing sky")),
        "the episode sky loaded: {:?}",
        scene.problems
    );
    assert!(
        scene.problems.contains(&"missing flat FLOOR1".to_owned())
            && scene.problems.contains(&"missing texture WALL1".to_owned()),
        "the Heretic fixture's pictures fall back and report: {:?}",
        scene.problems
    );
}
