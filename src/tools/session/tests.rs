//! Unit tests of the session group: `save_wad`'s dedupe report, the
//! `steps` bound, both history runs over a real [`crate::session::Session`]
//! and the active-map rule that follows an undo out of the merged view.
//!
//! The fixtures are built with the library's own types: a session whose
//! working archive holds markers, one `MAP01` block or a twin-payload pair,
//! and an `IWAD` layer for the case a step-back cannot clear.

use std::path::PathBuf;

use super::merge_save::{SaveWadParams, save_wad_report};
use super::undo::{clear_missing_active_map, merged_holds_map, parse_steps, redo_run, undo_run};
use crate::session::history::HISTORY_LIMIT;
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A parsed test name.
fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An empty marker lump named `text`.
fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A lump named `text` carrying `body`.
fn carrying(text: &str, body: &str) -> Lump {
    Lump::new(name(text), body.as_bytes().to_vec())
}

/// A `PWAD` holding `lumps`.
fn pwad(lumps: Vec<Lump>) -> Wad {
    Wad {
        kind: WadKind::Pwad,
        lumps,
    }
}

#[test]
fn save_wad_report_dedupes_only_when_asked_and_reports_the_saving() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let state = AppState::default();
    state
        .with_session(|session| {
            session
                .replace_working(
                    pwad(vec![
                        carrying("FIRST", "same-bytes"),
                        carrying("SECOND", "same-bytes"),
                        marker("NOTE"),
                    ]),
                    None,
                )
                .expect("no batch is open");
            Ok(())
        })
        .expect("seeding the session works");
    let plain_path = dir.path().join("plain.wad");
    let plain_args = SaveWadParams {
        path: Some(plain_path.display().to_string()),
        mode: None,
        kind: None,
        dedupe: false,
    };
    let plain = save_wad_report(&state, &plain_args).expect("the plain save lands");
    assert_eq!(
        plain.bytes_saved_by_dedupe, 0,
        "nothing shared, nothing saved"
    );
    assert!(!plain.dirty, "a pwad save marks the archive saved");
    let dedup_path = dir.path().join("dedup.wad");
    let dedup_args = SaveWadParams {
        path: Some(dedup_path.display().to_string()),
        mode: None,
        kind: None,
        dedupe: true,
    };
    let deduped = save_wad_report(&state, &dedup_args).expect("the dedupe save lands");
    assert_eq!(deduped.bytes_saved_by_dedupe, 10, "the twin's payload");
    assert_eq!(
        plain.bytes.saturating_sub(deduped.bytes),
        10,
        "the file really is smaller by the saved bytes"
    );
    assert_eq!(
        Wad::read_file(&dedup_path).expect("it re-opens"),
        Wad::read_file(&plain_path).expect("so does the plain save"),
        "dedupe changes the layout, never the archive"
    );
}

/// Appends an empty marker named `text` as one `add_lump TEXT` edit.
fn add_marker(session: &mut Session, text: &str) {
    let lump = marker(text);
    session
        .edit(&format!("add_lump {text}"), move |wad| {
            wad.lumps.push(lump);
            Ok(())
        })
        .expect("appends never fail");
}

/// Appends marker lumps named `texts`, each as its own edit.
fn add_markers(session: &mut Session, texts: &[&str]) {
    for text in texts {
        add_marker(session, text);
    }
}

/// Appends a minimal `MAP01` block (header plus `THINGS`) as one edit.
fn build_map01(session: &mut Session) {
    session
        .edit("build MAP01", |wad| {
            wad.lumps.push(marker("MAP01"));
            wad.lumps.push(marker("THINGS"));
            Ok(())
        })
        .expect("appends never fail");
}

#[test]
fn parse_steps_defaults_to_one_and_bounds_at_the_history_limit() {
    assert_eq!(parse_steps(None).expect("absent"), 1, "absent means one");
    assert_eq!(
        parse_steps(Some(1)).expect("lowest"),
        1,
        "the lowest accepted count"
    );
    assert_eq!(
        parse_steps(Some(HISTORY_LIMIT)).expect("highest"),
        HISTORY_LIMIT,
        "the highest accepted count"
    );
}

#[test]
fn parse_steps_refuses_zero_and_counts_past_the_limit() {
    let zero = parse_steps(Some(0)).expect_err("0 is refused");
    assert_eq!(zero.code(), "invalid_argument", "steps 0 code");
    let past = HISTORY_LIMIT
        .checked_add(1)
        .expect("a count past the limit");
    let error = parse_steps(Some(past)).expect_err("past the limit is refused");
    assert_eq!(error.code(), "invalid_argument", "steps > 100 code");
    assert!(
        error.to_string().contains("1..=100"),
        "the message names the accepted range: {error}"
    );
}

#[test]
fn undo_run_steps_back_over_the_stack_and_reports_it() {
    let mut session = Session::default();
    add_markers(&mut session, &["A", "B", "C"]);
    session
        .mark_saved(PathBuf::from("out.wad"))
        .expect("no batch is open");
    let report = undo_run(&mut session, 5).expect("three steps are available");
    assert_eq!(
        report.undone,
        vec!["add_lump C", "add_lump B", "add_lump A"],
        "newest first, and only three of the asked five ran"
    );
    assert_eq!(report.remaining_undo, 0, "the stack is empty now");
    assert_eq!(report.redo_available, 3, "all three can be replayed");
    assert!(report.dirty, "back to a revision before the saved one");
    assert!(!report.active_map_cleared, "no map was ever selected");
}

#[test]
fn undo_run_on_an_empty_stack_reports_not_found() {
    let mut session = Session::default();
    let error = undo_run(&mut session, 1).expect_err("nothing was edited");
    assert_eq!(error.code(), "not_found", "empty undo code");
    assert_eq!(error.to_string(), "nothing to undo", "empty undo message");
}

#[test]
fn redo_run_replays_the_undone_steps_in_order() {
    let mut session = Session::default();
    add_markers(&mut session, &["A", "B", "C"]);
    session
        .mark_saved(PathBuf::from("out.wad"))
        .expect("no batch is open");
    undo_run(&mut session, 3).expect("undo all three");
    let report = redo_run(&mut session, 2).expect("two steps are available");
    assert_eq!(
        report.redone,
        vec!["add_lump A", "add_lump B"],
        "the replay order, oldest undone first"
    );
    assert_eq!(report.remaining_redo, 1, "C still waits");
    assert_eq!(report.undo_available, 2, "A and B can be undone again");
    assert!(report.dirty, "not yet back at the saved revision");
    assert_eq!(session.working().len(), 2, "A and B are in the archive");
}

#[test]
fn redo_run_on_an_empty_stack_reports_not_found() {
    let mut session = Session::default();
    let error = redo_run(&mut session, 1).expect_err("nothing was undone");
    assert_eq!(error.code(), "not_found", "empty redo code");
    assert_eq!(error.to_string(), "nothing to redo", "empty redo message");
}

#[test]
fn undo_run_clears_an_active_map_the_history_removed() {
    let mut session = Session::default();
    build_map01(&mut session);
    session.set_active_map(Some(name("MAP01")));
    let report = undo_run(&mut session, 1).expect("the build is one step back");
    assert!(report.active_map_cleared, "the block left the merged view");
    assert_eq!(session.active_map(), None, "and the selection was dropped");
}

#[test]
fn undo_run_keeps_an_active_map_a_layer_still_provides() {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("doom.wad"),
            LayerRole::Iwad,
            pwad(vec![marker("MAP01"), marker("THINGS")]),
        ))
        .expect("no batch is open");
    session.set_active_map(Some(name("MAP01")));
    add_markers(&mut session, &["NOTE"]);
    let report = undo_run(&mut session, 1).expect("undo the note");
    assert!(!report.active_map_cleared, "the layer still holds the map");
    assert_eq!(
        session.active_map(),
        Some(name("MAP01")),
        "the selection stands"
    );
}

#[test]
fn merged_holds_map_reads_the_header_plus_members() {
    let mut session = Session::default();
    assert!(
        !merged_holds_map(&session, name("MAP01")),
        "nothing is loaded"
    );
    build_map01(&mut session);
    assert!(
        merged_holds_map(&session, name("MAP01")),
        "the working block counts"
    );
    assert!(
        !merged_holds_map(&session, name("MAP02")),
        "only MAP01 is there"
    );
}

#[test]
fn clear_missing_active_map_needs_a_selection_to_drop() {
    let mut session = Session::default();
    assert!(
        !clear_missing_active_map(&mut session),
        "no selection, nothing to clear"
    );
    session.set_active_map(Some(name("MAP07")));
    assert!(
        clear_missing_active_map(&mut session),
        "MAP07 is nowhere in the view"
    );
    assert_eq!(session.active_map(), None, "and it is gone");
}
