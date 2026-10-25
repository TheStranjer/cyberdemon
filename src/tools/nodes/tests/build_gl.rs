//! The `gl` variant: the classic lumps plus the `GL_*` block right after
//! the map, its marker name for a long map name, the block a second build
//! replaces and `undo` removes, the `UDMF` refusal, the counts a later
//! `get_nodes` reads back and the whole-row measure behind them.

use crate::session::{Scope, Session};
use crate::tools::nodes::build_gl::gl_lump_names;
use crate::tools::nodes::build_nodes::nodes_build_run;
use crate::tools::nodes::get_nodes::{GL_NODES_RECORD, GL_SEGS_RECORD, nodes_run, whole_records};
use crate::tools::nodes::{Detail, Request};
use crate::wad::classify::{MapBlock, analyze};

use super::{build_params, l_room_session, lump_names, name, udmf_room_session};

/// The `MAP01` block of the working archive.
fn working_block(session: &Session) -> MapBlock {
    let analysis = analyze(&session.working().lumps);
    analysis
        .maps
        .iter()
        .rev()
        .find(|block| block.name == name("MAP01"))
        .expect("the block was copied in")
        .clone()
}

#[test]
fn gl_build_writes_classic_lumps_and_a_gl_block_right_after_the_map() {
    let mut session = l_room_session();
    let report = nodes_build_run(&mut session, &build_params(None, Some("gl"), None))
        .expect("the L room gl-builds");
    assert_eq!(report.body.variant, "gl", "the echo");
    assert_eq!(report.body.segs, 7, "the classic run is written alongside");
    assert_eq!(report.body.subsectors, 2, "two leaves");
    assert_eq!(report.body.nodes, 1, "one cut");
    assert_eq!(
        report.body.split_vertices,
        Some(1),
        "the cut point joined VERTEXES"
    );
    assert_eq!(
        report.body.gl_segs,
        Some(9),
        "seven real segs plus two minisegs"
    );
    assert_eq!(
        report.body.minisegs,
        Some(2),
        "one per side of the doorway plane"
    );
    assert_eq!(
        report.body.gl_vertices,
        Some(0),
        "the classic run appended (128, 0), so the GL build reuses a map vertex"
    );
    assert!(
        report.body.format.is_none() && report.body.bytes.is_none(),
        "no extended keys"
    );
    let names = lump_names(&session);
    let head = names
        .iter()
        .position(|lump| lump == "GL_MAP01")
        .expect("the marker opens the run");
    assert_eq!(
        head,
        working_block(&session).end,
        "right after the map block"
    );
    let run_end = head.checked_add(6).expect("six lumps fit");
    assert_eq!(
        &names[head..run_end],
        [
            "GL_MAP01", "GL_VERT", "GL_SEGS", "GL_SSECT", "GL_NODES", "GL_PVS"
        ],
        "the full run in glBSP order"
    );
    assert_eq!(
        working_block(&session).gl,
        Some((head, run_end)),
        "analyze attaches it"
    );
}

#[test]
fn a_second_gl_build_replaces_the_block_and_undo_removes_it() {
    let mut session = l_room_session();
    nodes_build_run(&mut session, &build_params(None, Some("GL"), None)).expect("first");
    let first_names = lump_names(&session);
    nodes_build_run(&mut session, &build_params(None, Some("gl"), None)).expect("second");
    let second_names = lump_names(&session);
    assert_eq!(
        first_names, second_names,
        "the block was replaced, not duplicated"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["build nodes", "build nodes"],
        "each build is one undo step"
    );
    session.undo().expect("undo the second build");
    assert_eq!(
        lump_names(&session),
        first_names,
        "the first build stands again"
    );
    session.undo().expect("undo the first build too");
    assert!(
        lump_names(&session)
            .iter()
            .all(|lump| !lump.starts_with("GL_")),
        "undo took the whole GL run away: {:?}",
        lump_names(&session)
    );
}

#[test]
fn a_long_map_name_marks_the_gl_run_gl_level() {
    let names = gl_lump_names(name("CUSTOMMA")).expect("eight-char names parse");
    assert_eq!(names[0].to_string(), "GL_LEVEL", "the fallback marker");
    let short = gl_lump_names(name("E1M1")).expect("episode names fit");
    assert_eq!(short[0].to_string(), "GL_E1M1", "embedded when it fits");
}

#[test]
fn gl_refuses_a_udmf_block_without_writing() {
    let mut session = udmf_room_session();
    let error = nodes_build_run(&mut session, &build_params(None, Some("gl"), None))
        .expect_err("no classic run in a TEXTMAP");
    assert_eq!(
        error.code(),
        "invalid_argument",
        "the classic half is impossible"
    );
    assert!(
        error.to_string().contains("UDMF maps use extended nodes"),
        "the shared hint travels: {error}"
    );
    assert!(session.working().is_empty(), "nothing was written");
    assert!(session.undo_labels().is_empty(), "and nothing was recorded");
}

#[test]
fn a_gl_builds_read_back_through_get_nodes_counts() {
    let mut session = l_room_session();
    let built = nodes_build_run(&mut session, &build_params(None, Some("gl"), None))
        .expect("the build runs");
    let request = Request {
        map: None,
        scope: Scope::Merged,
        detail: Detail::Summary,
    };
    let (report, _publish) = nodes_run(&session, &request).expect("the read works");
    assert_eq!(report.format, "vanilla", "the classic run reads as always");
    assert!(report.gl.present, "the GL block is attached");
    assert_eq!(report.gl.magic.as_deref(), Some("gNd2"), "the v2 magic");
    assert_eq!(
        report.gl.segs, built.body.gl_segs,
        "the counts agree: GL_SEGS"
    );
    assert_eq!(report.gl.subsectors, Some(2), "GL_SSECT rows");
    assert_eq!(report.gl.nodes, Some(1), "GL_NODES rows");
    assert_eq!(report.gl.vertices, Some(0), "GL_VERT rows: none needed");
}

#[test]
fn whole_records_counts_only_whole_rows() {
    assert_eq!(whole_records(20, GL_SEGS_RECORD), Some(2), "two seg rows");
    assert_eq!(whole_records(21, GL_SEGS_RECORD), None, "a trailing byte");
    assert_eq!(whole_records(0, GL_NODES_RECORD), Some(0), "empty is whole");
}
