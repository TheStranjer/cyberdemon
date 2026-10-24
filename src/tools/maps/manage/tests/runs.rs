//! End-to-end tests of the four runs in [`super::super::runs`]: whole
//! `create_map`, `copy_map`, `rename_map` and `remove_map` calls against
//! seeded sessions, with the `rename_gl` helper's own checks.

use serde_json::json;

use super::{
    base_session, carrying, code, copy_plan, count_named, create_plan, doom_plan, layer_map,
    marker, name, seed, value, working_names,
};
use crate::session::Session;
use crate::tools::maps::manage::{copy_run, create_run, remove_run, rename_gl, rename_run};
use crate::wad::classify::analyze;

#[test]
fn create_run_appends_a_doom_block_and_adopts_the_active_map() {
    let mut session = base_session(layer_map());
    let report = create_run(&mut session, doom_plan("MAP05")).expect("fresh map");
    assert_eq!(
        value(&report),
        json!({
            "map": "MAP05", "format": "doom", "namespace": null, "warnings": [],
            "active_map": "MAP05",
        }),
        "the documented result",
    );
    assert_eq!(
        session.undo_labels(),
        vec!["create_map MAP05"],
        "one undoable edit",
    );
    assert!(session.is_dirty(), "and unsaved work");
    let appended = working_names(&session);
    let start = appended
        .iter()
        .position(|text| text == "MAP05")
        .expect("MAP05");
    assert_eq!(
        &appended[start..],
        [
            "MAP05", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
            "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the empty block sits at the end of the working archive",
    );
    session.undo().expect("undo the create");
    assert!(session.working().is_empty(), "the block went back");
}

#[test]
fn create_run_refuses_working_duplicates_replaces_and_overrides_layers() {
    let mut session = base_session(layer_map());
    create_run(&mut session, doom_plan("MAP01")).expect("the layer copy is overridable");
    let error = create_run(&mut session, doom_plan("MAP01")).expect_err("now it is working");
    assert_eq!(code(&error), "already_exists", "working duplicate code");
    assert!(
        error.to_string().contains("replace"),
        "the message names the way out: {error}"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["create_map MAP01"],
        "a refusal leaves no history",
    );
    let mut replace = doom_plan("MAP01");
    replace.replace = true;
    create_run(&mut session, replace).expect("replace lands");
    let names = working_names(&session);
    assert_eq!(
        count_named(&names, "MAP01"),
        1,
        "the old block was discarded"
    );
    assert_eq!(session.undo_labels().len(), 2, "replace is its own step");
}

#[test]
fn create_run_builds_hexen_and_udmf_blocks_and_keeps_the_active_map() {
    let mut session = Session::default();
    session.set_active_map(Some(name("MAP01")));
    let hexen = create_run(&mut session, create_plan("E1M1", "hexen", None)).expect("hexen");
    assert_eq!(value(&hexen)["format"], json!("hexen"), "the hexen report");
    assert_eq!(
        value(&hexen)["active_map"],
        json!("MAP01"),
        "a set active map is kept",
    );
    assert!(
        value(&hexen)["warnings"]
            .as_array()
            .expect("warnings")
            .is_empty(),
        "E1M1 is a loadable name",
    );
    assert!(
        working_names(&session).contains(&"BEHAVIOR".to_owned()),
        "the hexen block carries its BEHAVIOR",
    );
    let udmf =
        create_run(&mut session, create_plan("SLICK", "udmf", Some("GZDoom"))).expect("udmf");
    assert_eq!(
        value(&udmf)["namespace"],
        json!("gzdoom"),
        "lowered namespace"
    );
    let names = working_names(&session);
    let tail: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .skip(names.len().saturating_sub(3))
        .collect();
    assert_eq!(
        tail,
        vec!["SLICK", "TEXTMAP", "ENDMAP"],
        "the udmf block sits at the very end",
    );
}

#[test]
fn copy_run_copies_a_layer_block_and_renames_the_gl_marker() {
    let mut session = base_session(layer_map());
    let report = copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false))
        .expect("the copy lands");
    assert_eq!(
        value(&report),
        json!({"from": "MAP01", "to": "MAP09", "lumps": 5_u64}),
        "the documented result, GL run counted",
    );
    assert_eq!(
        working_names(&session),
        vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
        "block plus renamed GL run, header renamed",
    );
    assert_eq!(
        session.undo_labels(),
        vec!["copy_map MAP01 -> MAP09"],
        "label"
    );
}

#[test]
fn copy_run_drops_the_gl_run_for_an_oversized_marker_name() {
    let mut session = base_session(layer_map());
    let report = copy_run(
        &mut session,
        &copy_plan("MAP01", "LONGNAME", "layer:0", false),
    )
    .expect("the copy lands");
    assert_eq!(
        value(&report)["lumps"],
        json!(3_u64),
        "the GL run is left out"
    );
    assert_eq!(
        working_names(&session),
        vec!["LONGNAME", "THINGS", "LINEDEFS"],
        "members only",
    );
}

#[test]
fn copy_run_refuses_missing_sources_and_existing_targets_and_replaces() {
    let mut session = base_session(layer_map());
    let missing = copy_run(&mut session, &copy_plan("MAP07", "MAP09", "merged", false))
        .expect_err("no MAP07 anywhere");
    assert_eq!(code(&missing), "not_found", "missing source code");
    assert!(session.working().is_empty(), "nothing was copied");
    assert!(session.undo_labels().is_empty(), "and nothing recorded");
    copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false)).expect("first copy");
    let clash = copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false))
        .expect_err("MAP09 is working now");
    assert_eq!(code(&clash), "already_exists", "existing target code");
    let names = working_names(&session);
    assert_eq!(
        count_named(&names, "MAP09"),
        1,
        "the refusal appended nothing"
    );
    copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", true)).expect("replace");
    assert_eq!(
        working_names(&session),
        vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
        "the replaced copy stands alone at the end",
    );
    assert_eq!(
        session.undo_labels().len(),
        2,
        "copy and replace are two steps",
    );
}

#[test]
fn copy_run_can_read_a_working_block_under_a_new_name() {
    let mut session = base_session(layer_map());
    copy_run(&mut session, &copy_plan("MAP01", "MAP09", "working", false))
        .expect_err("working holds no MAP01");
    copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false)).expect("layer copy");
    copy_run(&mut session, &copy_plan("MAP09", "MAP10", "working", false)).expect("re-copy");
    let names = working_names(&session);
    assert_eq!(count_named(&names, "MAP09"), 1, "the first copy stands");
    assert_eq!(count_named(&names, "MAP10"), 1, "the second arrived");
}

#[test]
fn rename_run_renames_the_block_and_marker_and_follows_the_active_map() {
    let mut session = base_session(layer_map());
    copy_run(&mut session, &copy_plan("MAP01", "MAP08", "merged", false)).expect("copy in");
    session.set_active_map(Some(name("MAP08")));
    let report = rename_run(&mut session, name("MAP08"), name("MAP02")).expect("renamed");
    assert_eq!(
        value(&report),
        json!({"from": "MAP08", "to": "MAP02"}),
        "the documented result",
    );
    assert_eq!(
        working_names(&session),
        vec!["MAP02", "THINGS", "LINEDEFS", "GL_MAP02", "GL_VERT"],
        "header and marker renamed in place",
    );
    assert_eq!(
        session.active_map(),
        Some(name("MAP02")),
        "the selection follows"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["rename_map MAP08 -> MAP02", "copy_map MAP01 -> MAP08"],
        "one step per call",
    );
    session.undo().expect("undo the rename");
    assert_eq!(
        working_names(&session),
        vec!["MAP08", "THINGS", "LINEDEFS", "GL_MAP08", "GL_VERT"],
        "the old names stand again",
    );
    assert_eq!(
        session.active_map(),
        Some(name("MAP02")),
        "the selection is not part of the history",
    );
}

#[test]
fn rename_run_keeps_gl_level_and_drops_oversized_markers() {
    let mut session = Session::default();
    seed(
        &mut session,
        vec![
            marker("E1M1"),
            carrying("THINGS", "t"),
            marker("GL_LEVEL"),
            carrying("GL_VERT", "g"),
        ],
    );
    rename_run(&mut session, name("E1M1"), name("ABCDEFGH")).expect("gl_level stays");
    assert_eq!(
        working_names(&session),
        vec!["ABCDEFGH", "THINGS", "GL_LEVEL", "GL_VERT"],
        "GL_LEVEL needs no rename, however long the new map name is",
    );
    seed(
        &mut session,
        vec![
            marker("MAP01"),
            carrying("THINGS", "x"),
            marker("GL_MAP01"),
            carrying("GL_VERT", "y"),
        ],
    );
    rename_run(&mut session, name("MAP01"), name("TOOLONG6")).expect("the run goes");
    assert_eq!(
        working_names(&session),
        vec![
            "ABCDEFGH", "THINGS", "GL_LEVEL", "GL_VERT", "TOOLONG6", "THINGS"
        ],
        "GL_<name> would not fit, so that block's GL run is dropped",
    );
}

#[test]
fn rename_run_refuses_missing_colliding_and_self_renames() {
    let mut session = base_session(layer_map());
    let error = rename_run(&mut session, name("MAP01"), name("MAP09"))
        .expect_err("MAP01 is only in the layer");
    assert_eq!(code(&error), "not_found", "missing in working code");
    assert!(
        error.to_string().contains("copy_map MAP01 MAP09"),
        "the message names the tool and the fix: {error}"
    );
    seed(&mut session, vec![marker("MAP09"), carrying("THINGS", "x")]);
    let taken = rename_run(&mut session, name("MAP09"), name("MAP09")).expect_err("self rename");
    assert_eq!(code(&taken), "invalid_argument", "self rename code");
    seed(&mut session, vec![marker("MAP07"), carrying("THINGS", "y")]);
    assert_eq!(
        rename_run(&mut session, name("MAP09"), name("MAP07"))
            .expect_err("MAP07 exists")
            .code(),
        "already_exists",
        "existing target code",
    );
    assert_eq!(
        rename_run(&mut session, name("MAP07"), name("MAP06"))
            .expect("free name")
            .to,
        name("MAP06"),
        "the refusal above changed nothing that this rename builds on",
    );
}

#[test]
fn rename_gl_renames_or_reports_the_run_it_drops() {
    let mut session = Session::default();
    seed(
        &mut session,
        vec![
            marker("MAP01"),
            carrying("THINGS", "t"),
            marker("GL_MAP01"),
            carrying("GL_VERT", "g"),
        ],
    );
    let analysis = analyze(&session.working().lumps);
    let gl = analysis.maps.first().expect("the block").gl;
    assert_eq!(gl, Some((2_usize, 4)), "the fixture run");
    let mut working = session.working().clone();
    let dropped = rename_gl(&mut working, gl, name("MAP02")).expect("renamed");
    assert_eq!(dropped, None, "GL_MAP02 fits");
    assert_eq!(
        working.lumps.get(2).expect("marker").name,
        name("GL_MAP02"),
        "the marker was renamed",
    );
    let mut again = session.working().clone();
    assert_eq!(
        rename_gl(&mut again, gl, name("LONGNAME")).expect("dropped"),
        Some((2_usize, 4)),
        "the oversized rename reports the run for removal",
    );
}

#[test]
fn remove_run_removes_the_block_and_clears_the_active_map() {
    let mut session = base_session(layer_map());
    copy_run(&mut session, &copy_plan("MAP01", "MAP08", "merged", false)).expect("copy in");
    session.set_active_map(Some(name("MAP08")));
    let report = remove_run(&mut session, name("MAP08")).expect("removed");
    assert_eq!(
        value(&report),
        json!({"map": "MAP08", "lumps_removed": 5_u64}),
        "block plus GL run",
    );
    assert!(session.working().is_empty(), "everything went");
    assert_eq!(session.active_map(), None, "and the selection cleared");
    assert_eq!(
        session.undo().expect("undo the removal"),
        "remove_map MAP08",
        "the label comes back",
    );
    assert_eq!(
        working_names(&session),
        vec!["MAP08", "THINGS", "LINEDEFS", "GL_MAP08", "GL_VERT"],
        "undo brings the block back",
    );
    let layer = remove_run(&mut session, name("MAP01")).expect_err("the layer never loses maps");
    assert_eq!(code(&layer), "not_found", "layer map code");
    assert!(
        layer.to_string().contains("read-only"),
        "the message explains: {layer}"
    );
}
