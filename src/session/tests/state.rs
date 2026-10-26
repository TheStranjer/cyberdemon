//! The rest of the session's state: the active map, the render settings and
//! the texture theme that ride through an undo and a replaced archive, the
//! layer-only view under a working override, what the merged view reflects,
//! which game the archives identify and the copy-on-write step that brings a
//! layer's map into the working archive.

use super::{append_marker, layer, loaded, lump, marker, name, payload, pwad, tagged};
use crate::render::{Lighting, RenderSettings};
use crate::session::{LayerRole, Scope, Session, Source};
use crate::themes::Role;
use crate::wad::game::Game;

#[test]
fn active_map_is_set_and_cleared() {
    let mut session = loaded();
    session.set_active_map(Some(name("MAP01")));
    assert_eq!(session.active_map(), Some(name("MAP01")));
    session.set_active_map(None);
    assert_eq!(session.active_map(), None, "cleared again");
}

#[test]
fn a_fresh_session_starts_on_the_default_render_settings() {
    let session = Session::default();
    assert_eq!(
        session.render_settings(),
        &RenderSettings::default(),
        "the render tools' defaults are the documented ones"
    );
    assert_eq!(session.render_settings().width, 640, "the default width");
}

#[test]
fn render_settings_survive_an_undo_and_a_replaced_archive() {
    let mut session = loaded();
    let mut settings = session.render_settings().clone();
    settings
        .set_resolution(1_280, 800)
        .expect("a supported resolution");
    settings.set_gamma(3).expect("a supported level");
    session.set_render_settings(settings.clone());
    session
        .edit("add MARK", append_marker)
        .expect("an edit to undo");
    session.undo().expect("undo it");
    session
        .replace_working(pwad(vec![lump("PLAYPAL")]), None)
        .expect("no batch is open");
    assert_eq!(
        session.render_settings(),
        &settings,
        "settings are session state: no edit, undo or new archive touches them"
    );
}

#[test]
fn render_settings_mut_changes_the_stored_settings() {
    let mut session = Session::default();
    let mut expected = session.render_settings().clone();
    expected.lighting = Lighting::Fullbright;
    session
        .render_settings_mut()
        .set_lighting("fullbright")
        .expect("a supported model");
    assert_eq!(
        session.render_settings(),
        &expected,
        "the mutable accessor reaches the stored value"
    );
}

#[test]
fn the_texture_theme_is_session_state_outside_the_history() {
    let mut session = Session::default();
    assert!(
        session.texture_theme().name.is_none() && session.texture_theme().overrides.is_empty(),
        "a fresh session follows the game default with no overrides"
    );
    session.texture_theme_mut().name = Some("hell".to_owned());
    session
        .texture_theme_mut()
        .overrides
        .insert(Role::Wall, "WALL1".to_owned());
    session
        .edit("add MARK", append_marker)
        .expect("an edit to undo");
    session.undo().expect("undo it");
    session
        .replace_working(pwad(vec![lump("PLAYPAL")]), None)
        .expect("no batch is open");
    assert_eq!(
        session.texture_theme().name.as_deref(),
        Some("hell"),
        "no edit, undo or new archive touches the theme"
    );
    assert!(
        session.texture_theme().overrides.contains_key(&Role::Wall),
        "the overrides ride along"
    );
}

#[test]
fn base_lumps_are_the_merged_view_without_the_working_archive() {
    let mut session = loaded();
    session
        .replace_working(pwad(vec![payload("PLAYPAL", "working copy")]), None)
        .expect("no batch is open");
    assert_eq!(
        tagged(&session.base_lumps()),
        vec![
            "PLAYPAL@L0",
            "GAMEINFO@L1",
            "F_START@L0",
            "FLOOR_1@L0",
            "FLOOR_2@L0",
            "F_SKY1@L1",
            "F_END@L0",
            "MAP01@L0",
            "THINGS@L0",
        ],
        "the layers folded in load order, no working override and no working arrival"
    );
    assert_ne!(
        tagged(&session.merged()),
        tagged(&session.base_lumps()),
        "the working archive does change the merged view"
    );
    let base_playpal = session
        .base_lumps()
        .into_iter()
        .find(|entry| entry.lump.name == name("PLAYPAL"))
        .expect("the base has a PLAYPAL");
    assert_eq!(
        base_playpal.source,
        Source::Layer(0),
        "the base keeps the layer's"
    );
    assert_eq!(base_playpal.lump.data.as_ref(), b"PLAYPAL", "and its bytes");
}

#[test]
fn base_lumps_of_a_fresh_session_are_empty() {
    assert!(
        Session::default().base_lumps().is_empty(),
        "no layers, no base"
    );
}

#[test]
fn the_merged_view_reflects_working_archive_edits() {
    let mut session = loaded();
    session
        .edit("replace floor 1", |wad| {
            wad.lumps.push(marker("F_START"));
            wad.lumps.push(lump("FLOOR_1"));
            wad.lumps.push(marker("F_END"));
            Ok(())
        })
        .expect("a region override");
    assert_eq!(
        tagged(&session.merged()),
        vec![
            "PLAYPAL@L0",
            "GAMEINFO@L1",
            "F_START@L0",
            "FLOOR_1@W",
            "FLOOR_2@L0",
            "F_SKY1@L1",
            "F_END@L0",
            "MAP01@L0",
            "THINGS@L0",
        ],
        "the override landed where the base flat stood"
    );
}

#[test]
fn game_of_a_fresh_session_is_unknown() {
    assert_eq!(
        Session::default().game(),
        Game::Unknown,
        "no archives, no game"
    );
}

#[test]
fn game_reads_the_iwad_layer_first() {
    let mut session = loaded();
    session
        .edit("add MAP01", |wad| {
            wad.lumps.push(marker("MAP01"));
            Ok(())
        })
        .expect("an edit");
    assert_eq!(
        session.game(),
        Game::Doom2,
        "the base layer's MAP01 identifies DOOM II"
    );
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "doom1",
            vec![marker("E1M1"), lump("THINGS")],
        ))
        .expect("no batch is open");
    assert_eq!(
        session.game(),
        Game::Doom,
        "the episode base wins even with a MAP01 in the working archive"
    );
}

#[test]
fn game_falls_back_to_the_merged_view_without_an_iwad() {
    let mut session = Session::default();
    session
        .edit("add MAP01", |wad| {
            wad.lumps.push(marker("MAP01"));
            Ok(())
        })
        .expect("an edit");
    assert_eq!(
        session.game(),
        Game::Doom2,
        "the working archive names the game"
    );
}

#[test]
fn effective_game_falls_back_to_doom_ii_only_when_unidentified() {
    assert_eq!(
        Session::default().effective_game(),
        Game::Doom2,
        "no archives: DOOM II rows"
    );
    let mut session = Session::default();
    session
        .set_iwad(layer(LayerRole::Iwad, "plain", vec![marker("PLAYPAL")]))
        .expect("no batch is open");
    assert_eq!(
        session.effective_game(),
        Game::Doom2,
        "an unidentifiable IWAD also stands for DOOM II"
    );
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "hexen",
            vec![marker("TITLE"), marker("WINNOWR")],
        ))
        .expect("no batch is open");
    assert_eq!(
        session.effective_game(),
        Game::Hexen,
        "a detected game wins"
    );
}

#[test]
fn custom_sources_returns_the_merged_texts_and_the_include_notes() {
    let mut session = Session::default();
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "base",
            vec![
                marker("PLAYPAL"),
                crate::wad::archive::Lump::new(
                    name("DECORATE"),
                    b"#include \"MISSING\"\nactor SessionTest 9101 { }".to_vec(),
                ),
            ],
        ))
        .expect("no batch is open");
    let (texts, notes) = session.custom_sources().expect("the merged scope lists");
    assert_eq!(texts.decorate.len(), 1, "the DECORATE text");
    assert_eq!(notes.len(), 1, "the missing include is noted: {notes:?}");
    assert!(
        notes
            .first()
            .expect("one note")
            .contains("not in the merged view"),
        "the note says what is missing: {notes:?}"
    );
    assert_eq!(session.effective_game(), Game::Doom2, "an unnamed base");
}

#[test]
fn definitions_overlay_the_merged_custom_sources_on_the_effective_game() {
    let mut session = Session::default();
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "doom2.wad",
            vec![
                marker("MAP01"),
                crate::wad::archive::Lump::new(
                    name("DECORATE"),
                    b"actor SessionTest 9101 { }".to_vec(),
                ),
            ],
        ))
        .expect("no batch is open");
    let defs = session.definitions().expect("the overlay parses");
    let row = defs
        .things
        .iter()
        .find(|def| def.kind == 9101)
        .expect("the DECORATE thing");
    assert_eq!(row.name, "SessionTest", "the custom name reads through");
    assert_eq!(row.source, "decorate", "the row carries its source");
}

#[test]
fn layer_role_labels_are_the_documented_words() {
    assert_eq!(LayerRole::Iwad.as_str(), "iwad", "the wire word");
    assert_eq!(LayerRole::Resource.as_str(), "resource", "the wire word");
    assert_eq!(LayerRole::Iwad.label(), "IWAD", "the message label");
    assert_eq!(LayerRole::Resource.label(), "resource", "the message label");
}

#[test]
fn copy_map_to_working_brings_a_layer_map_into_the_working_archive() {
    let mut session = loaded();
    let copied = session
        .copy_map_to_working(name("MAP01"))
        .expect("the base holds MAP01");
    assert!(copied, "the working archive had no copy yet");
    assert_eq!(
        tagged(
            &session
                .lumps_in(Scope::Working)
                .expect("the working archive")
        ),
        vec!["MAP01@W", "THINGS@W"],
        "the whole block was appended, in view order"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["copy_map MAP01"],
        "the copy is one undoable edit"
    );
    assert!(session.is_dirty(), "and unsaved work");
    assert!(
        !session
            .copy_map_to_working(name("MAP01"))
            .expect("the working copy is already editable"),
        "the second call copies nothing"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["copy_map MAP01"],
        "and records no history for it"
    );
    assert_eq!(
        session.undo().expect("undo the copy"),
        "copy_map MAP01",
        "the label comes back"
    );
    assert!(session.working().is_empty(), "the block went back");
}

#[test]
fn copy_map_to_working_reports_not_found_for_an_unknown_map() {
    let mut session = loaded();
    let error = session
        .copy_map_to_working(name("MAP07"))
        .expect_err("no archive holds MAP07");
    assert_eq!(error.code(), "not_found", "unknown map code");
    assert!(session.working().is_empty(), "nothing was copied");
    assert!(session.undo_labels().is_empty(), "and nothing recorded");
    assert!(!session.is_dirty(), "the session is still clean");
}
