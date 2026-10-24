//! `bulk_edit` through the shared fixture: the wire parts
//! [`requested_edits`] folds into the engine request, the one labeled step
//! [`bulk_in_session`] writes with its preview, `max` guard, thing-name
//! resolution, flag edits and stock checks, the refusal [`bulk_run`]
//! reports through the state, and the windows [`bulk_report`] pages.

use serde_json::{Map as JsonMap, json};

use crate::session::Scope;
use crate::state::AppState;
use crate::tools::maps::common;
use crate::tools::query::bulk::{
    BULK_NODES_STEP, bulk_in_session, bulk_report, bulk_run, requested_edits,
};
use crate::tools::query::{INDEX_PAGE, PREVIEW_PAGE};
use crate::wad::map::bulk::edits::{Applied, Change};

use super::{bulk_args, bulk_session, one_filter, parsed};

#[test]
fn requested_edits_folds_the_wire_parts_into_the_engine_request() {
    let args = bulk_args(json!({
        "kind": "things",
        "set": {"angle": 90_i64},
        "add": {"x": 4_i64},
        "flags_set": ["ambush"],
        "flags_clear": [],
    }));
    let edits = requested_edits(&args);
    assert_eq!(
        edits.set.get("angle"),
        Some(&json!(90_i64)),
        "the set map rides"
    );
    assert_eq!(edits.add.get("x"), Some(&4_i64), "the amounts ride");
    assert_eq!(edits.flags_set, vec!["ambush".to_owned()], "the names ride");
    assert!(edits.flags_clear.is_empty(), "an empty list stays empty");
    assert!(!edits.is_empty(), "a full request is not empty");
}

#[test]
fn bulk_in_session_edits_every_match_in_one_labeled_step() {
    let mut session = bulk_session();
    let report = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "sectors", "set": {"light": 96_i64}, "add": {"tag": 2_i64}})),
    )
    .expect("the edit runs");
    let shown = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(shown["matched"], json!(2_u64), "both sectors matched");
    assert_eq!(shown["changed"], json!(2_u64), "both rows moved");
    assert_eq!(shown["indices"], json!([0_u64, 1_u64]), "ascending");
    assert_eq!(shown["truncated"], json!(false), "both fit the page");
    assert_eq!(shown["dry_run"], json!(false), "the edit wrote");
    assert_eq!(shown["map"], json!("MAP01"), "the fragment names the block");
    assert_eq!(
        shown["nodes_invalidated"],
        json!(false),
        "lights and tags are properties"
    );
    assert_eq!(
        shown["copied_to_working"],
        json!(true),
        "the map came from the layer"
    );
    let row = shown["preview"].get(0).expect("a preview row");
    assert_eq!(
        row["after"],
        json!({"light": 96_i64, "tag": 2_i64}),
        "set and add ride one pass: {row}"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["bulk_edit sectors (2)"],
        "one labeled step"
    );
    assert!(
        !report.next_steps.contains(&BULK_NODES_STEP),
        "a property step names no node build: {report:?}"
    );
}

#[test]
fn bulk_in_session_previews_without_writing_or_recording() {
    let mut session = bulk_session();
    let report = bulk_in_session(
        &mut session,
        &bulk_args(json!({
            "kind": "sectors",
            "where": [one_filter("index", "eq", &json!(1_i64))],
            "add": {"light": 8_i64},
            "dry_run": true,
        })),
    )
    .expect("the preview runs");
    let shown = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        shown["matched"],
        json!(1_u64),
        "the index filter selected one"
    );
    assert_eq!(shown["changed"], json!(1_u64), "its light would move");
    assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(
        shown["preview"][0]["before"],
        json!({"light": 0_i64}),
        "the past previews"
    );
    assert_eq!(
        shown["preview"][0]["after"],
        json!({"light": 8_i64}),
        "and the future"
    );
    assert_eq!(
        shown["copied_to_working"],
        json!(false),
        "a preview copies nothing"
    );
    assert_eq!(
        shown["nodes_invalidated"],
        json!(false),
        "and invalidates nothing"
    );
    assert!(session.working().is_empty(), "the working archive stands");
    assert!(session.undo_labels().is_empty(), "no history entry");
    assert_eq!(
        report.next_steps.len(),
        2_usize,
        "the preview carries its steps"
    );
}

#[test]
fn bulk_in_session_refuses_an_empty_request_before_any_read() {
    let mut session = bulk_session();
    let error = bulk_in_session(&mut session, &bulk_args(json!({"kind": "sectors"})))
        .expect_err("no change was asked");
    assert_eq!(error.code(), "invalid_argument", "empty request code");
    assert!(
        error.to_string().contains("nothing to change"),
        "the message names the parameters: {error}"
    );
    assert!(session.undo_labels().is_empty(), "and nothing was recorded");
}

#[test]
fn bulk_in_session_guards_the_matched_set_with_max() {
    let mut session = bulk_session();
    let error = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "vertices", "max": 1_i64, "add": {"x": 4_i64}})),
    )
    .expect_err("two vertices match, max is one");
    assert_eq!(error.code(), "limit_exceeded", "over max code");
    let message = error.to_string();
    assert!(
        message.contains("max") && message.contains("dry_run") && message.contains("where"),
        "the refusal names max, dry_run and the narrowing: {message}"
    );
    assert!(
        session.undo_labels().is_empty(),
        "the guard recorded nothing"
    );
    assert!(session.working().is_empty(), "and wrote nothing");
}

#[test]
fn bulk_in_session_slides_vertices_as_geometry_and_things_as_property() {
    let mut session = bulk_session();
    let slid = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "vertices", "add": {"x": 8_i64}})),
    )
    .expect("the slide runs");
    let shown = serde_json::to_value(&slid).expect("the report serializes");
    assert_eq!(
        shown["changed"],
        json!(2_u64),
        "both referenced vertices moved"
    );
    assert_eq!(
        shown["nodes_invalidated"],
        json!(true),
        "moving referenced vertices is geometry"
    );
    assert!(
        slid.next_steps.contains(&BULK_NODES_STEP),
        "the refusal names the node build: {slid:?}"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["bulk_edit vertices (2)"],
        "one labeled step"
    );
    let moved = bulk_in_session(
        &mut session,
        &bulk_args(json!({
            "kind": "things",
            "where": [one_filter("type", "eq", &json!(3001_i64))],
            "add": {"x": 16_i64},
        })),
    )
    .expect("the thing move runs");
    let written = serde_json::to_value(&moved).expect("the report serializes");
    assert_eq!(written["changed"], json!(1_u64), "the imp slid");
    assert_eq!(
        written["nodes_invalidated"],
        json!(false),
        "thing moves never touch the nodes"
    );
    assert_eq!(
        written["copied_to_working"],
        json!(false),
        "the block was already editable"
    );
}

#[test]
fn bulk_in_session_resolves_the_thing_type_name_and_refuses_the_stray() {
    let mut session = bulk_session();
    let swapped = bulk_in_session(
        &mut session,
        &bulk_args(json!({
            "kind": "things",
            "where": [one_filter("name", "glob", &json!("imp*"))],
            "set": {"type": "demon"},
        })),
    )
    .expect("the name resolves against the session definitions");
    let shown = serde_json::to_value(&swapped).expect("the report serializes");
    assert_eq!(shown["matched"], json!(1_u64), "the glob found the imp");
    assert_eq!(
        shown["preview"][0]["after"],
        json!({"type": 3002_i64}),
        "the name landed as the doomednum"
    );
    let stray = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "things", "set": {"type": "platypus"}})),
    )
    .expect_err("nothing is named platypus");
    assert_eq!(stray.code(), "invalid_argument", "unknown name code");
    assert!(
        stray.to_string().contains("get_thing_types"),
        "the reference is named: {stray}"
    );
    assert_eq!(
        session.undo_labels().len(),
        1_usize,
        "the refusal recorded nothing"
    );
}

#[test]
fn bulk_in_session_sets_linedef_flags_by_name() {
    let mut session = bulk_session();
    let gated = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "linedefs", "flags_set": ["blockmonsters"]})),
    )
    .expect("the flag edit runs");
    let shown = serde_json::to_value(&gated).expect("the report serializes");
    assert_eq!(shown["changed"], json!(1_u64), "the fixture line moved");
    assert_eq!(
        shown["preview"][0]["after"],
        json!({"flags": 3_i64}),
        "blocking stands, blockmonsters goes on"
    );
    let map =
        common::load_map(&session, parsed("MAP01"), Scope::Merged).expect("the merged block reads");
    assert_eq!(
        map.linedefs.first().expect("the fixture line").flags,
        3_u32,
        "the bits landed"
    );
    let stray = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "linedefs", "flags_set": ["nope"]})),
    )
    .expect_err("nope is no flag");
    assert_eq!(stray.code(), "invalid_argument", "unknown flag code");
    assert_eq!(
        session.undo_labels().len(),
        1_usize,
        "the refusal recorded nothing"
    );
}

#[test]
fn bulk_in_session_checks_the_stock_and_allow_missing_escapes_it() {
    let mut session = bulk_session();
    let missing = bulk_in_session(
        &mut session,
        &bulk_args(json!({"kind": "sectors", "set": {"floor_flat": "slime9"}})),
    )
    .expect_err("the name is not part of the stock");
    assert_eq!(missing.code(), "not_found", "missing stock code");
    assert!(
        missing.to_string().contains("allow_missing"),
        "the escape hatch is named: {missing}"
    );
    let written = bulk_in_session(
        &mut session,
        &bulk_args(json!({
            "kind": "sectors",
            "set": {"floor_flat": "slime9"},
            "allow_missing": true,
        })),
    )
    .expect("the escape admits the name");
    let shown = serde_json::to_value(&written).expect("the report serializes");
    assert_eq!(shown["changed"], json!(2_u64), "both sectors took it");
    assert_eq!(
        session.undo_labels(),
        vec!["bulk_edit sectors (2)"],
        "one labeled step wrote the unstocked names"
    );
}

#[test]
fn bulk_run_reports_the_no_map_refusal_through_the_state() {
    let state = AppState::default();
    let error = bulk_run(
        &state,
        &bulk_args(json!({"kind": "sectors", "map": "MAP09", "set": {"light": 1_i64}})),
    )
    .expect_err("nothing is open");
    assert_eq!(
        error.code(),
        "not_found",
        "an explicit unknown map is missing"
    );
}

#[test]
fn bulk_report_pages_the_index_and_preview_windows() {
    let wide = INDEX_PAGE.checked_add(2_usize).expect("the window fits");
    let applied = Applied {
        matched: wide,
        changed: wide,
        indices: (0..wide).collect(),
        preview: (0..wide)
            .take(PREVIEW_PAGE.checked_add(5_usize).expect("window"))
            .map(|index| Change {
                index,
                before: json!({"light": 0_i64}),
                after: json!({"light": 1_i64}),
            })
            .collect(),
    };
    let report = bulk_report(applied, false, JsonMap::new());
    assert_eq!(report.matched, wide, "the counts travel whole");
    assert_eq!(report.indices.len(), INDEX_PAGE, "the index list is capped");
    assert!(report.truncated, "and says so");
    assert_eq!(report.preview.len(), PREVIEW_PAGE, "the preview is capped");
    assert_eq!(
        report.next_steps.len(),
        3_usize,
        "a plain written run carries the three steps"
    );
}
