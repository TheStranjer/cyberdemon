//! The whole-set build: how one call's parameters resolve against the
//! map's format, the `finalize` defaults the plan builder runs, the node
//! body that result reports, the one undoable step writing tree, `REJECT`
//! and `BLOCKMAP` - the `gl` run attached in the same step, the `UDMF`
//! block left to `ZNODES` - and the refusals that write nothing.

use crate::tools::nodes::build_all::{AllPlan, BuildAllNodesParams, all_nodes_run, plan_all};
use crate::tools::nodes::build_nodes::{DEFAULT_SPLIT_COST, Variant};
use crate::tools::nodes::{apply_all, find_lump};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::Map;
use crate::wad::map::build_reject::RejectMode;
use serde_json::json;

use super::{
    l_room_model, l_room_session, lump_names, sidedless_session, udmf_room_session, working_map,
};

/// `build_all_nodes` arguments from optional pieces.
fn all_params(
    map: Option<&str>,
    variant: Option<&str>,
    reject_mode: Option<&str>,
) -> BuildAllNodesParams {
    BuildAllNodesParams {
        map: map.map(str::to_owned),
        variant: variant.map(str::to_owned),
        reject_mode: reject_mode.map(str::to_owned),
        compress: None,
    }
}

#[test]
fn plan_all_defaults_follow_the_map_format() {
    let binary = Map::default();
    let plan = plan_all(&binary, &all_params(None, None, None)).expect("binary plans");
    assert_eq!(
        plan.nodes.variant,
        Variant::Classic,
        "binary defaults to classic"
    );
    assert_eq!(
        plan.reject,
        Some(RejectMode::Zero),
        "binary maps write a reject"
    );
    assert!(plan.blockmap, "and the blockmap");
    assert_eq!(
        plan.nodes.split_cost, DEFAULT_SPLIT_COST,
        "the builder default"
    );
    assert!(plan.nodes.compress, "compressed by default");
    let udmf = Map {
        format: MapFormat::Udmf,
        ..Map::default()
    };
    let leaves = plan_all(&udmf, &all_params(None, None, None)).expect("UDMF plans");
    assert_eq!(
        leaves.nodes.variant,
        Variant::Extended,
        "UDMF defaults to extended"
    );
    assert_eq!(leaves.reject, None, "ZDoom builds the reject itself");
    assert!(!leaves.blockmap, "and the blockmap");
    let asked = plan_all(
        &udmf,
        &all_params(None, Some("extended"), Some("COMPONENTS")),
    )
    .expect("an explicit mode plans a reject");
    assert_eq!(
        asked.reject,
        Some(RejectMode::Components),
        "the explicit mode"
    );
    let forced = plan_all(&binary, &all_params(None, Some("extended"), None))
        .expect("an explicit layout wins");
    assert_eq!(
        forced.nodes.variant,
        Variant::Extended,
        "classic is not forced"
    );
    assert!(forced.blockmap, "a binary map keeps its blockmap");
    let bad =
        plan_all(&binary, &all_params(None, Some("znodes"), None)).expect_err("not a variant");
    assert_eq!(bad.code(), "invalid_argument", "bad variant code");
    let bad_mode =
        plan_all(&binary, &all_params(None, None, Some("opaque"))).expect_err("not a mode");
    assert_eq!(bad_mode.code(), "invalid_argument", "bad mode code");
}

#[test]
fn the_finalize_plan_is_plan_all_without_any_parameter() {
    let binary_model = Map::default();
    let udmf_model = Map {
        format: MapFormat::Udmf,
        ..Map::default()
    };
    for model in [binary_model, udmf_model] {
        assert_eq!(
            AllPlan::for_model(&model),
            plan_all(&model, &all_params(None, None, None)).expect("no parameters"),
            "the whole-set defaults are the parameterless plan_all for {model:?}",
        );
    }
}

#[test]
fn the_finalize_defaults_follow_the_map_format() {
    let binary = AllPlan::for_model(&Map::default());
    assert_eq!(
        binary.nodes.variant,
        Variant::Classic,
        "binary maps take the vanilla run"
    );
    assert_eq!(
        binary.nodes.split_cost, DEFAULT_SPLIT_COST,
        "the builder default cost"
    );
    assert!(binary.nodes.compress, "compressed wherever it applies");
    assert_eq!(binary.reject, Some(RejectMode::Zero), "and a zero reject");
    assert!(binary.blockmap, "and a fresh blockmap");
    let leaves = AllPlan::for_model(&Map {
        format: MapFormat::Udmf,
        ..Map::default()
    });
    assert_eq!(
        leaves.nodes.variant,
        Variant::Extended,
        "UDMF takes the extended lump"
    );
    assert_eq!(leaves.reject, None, "ZDoom builds the reject itself");
    assert!(!leaves.blockmap, "and the blockmap");
}

#[test]
fn apply_all_reports_the_node_body_as_plan_finalize_json() {
    let mut map = l_room_model();
    let plan = AllPlan::for_model(&map);
    let built = apply_all(&mut map, &plan).expect("the L room fully builds");
    let body = built.into_nodes_json().expect("the summary serializes");
    assert_eq!(body["variant"], json!("classic"), "the binary default");
    assert_eq!(body["segs"], json!(7_u64), "the L room's seg count");
    assert_eq!(body["subsectors"], json!(2_u64), "one cut, two leaves");
    assert_eq!(body["nodes"], json!(1_u64), "one interior node");
    assert!(
        body.get("reject").is_none() && body.get("blockmap").is_none(),
        "the plan result carries the nodes body only"
    );
    for wanted in lumps::BINARY_NODE_LUMPS {
        assert!(
            map.other.iter().any(|lump| lump.name == wanted),
            "apply_all installed {wanted:?} into the model",
        );
    }
}

#[test]
fn all_nodes_run_writes_tree_reject_and_blockmap_in_one_undoable_step() {
    let mut session = l_room_session();
    let report = all_nodes_run(&mut session, &all_params(None, None, None))
        .expect("the L room fully builds");
    assert_eq!(report.variant, "classic", "the binary default");
    assert_eq!(report.nodes.segs, 7, "the classic count");
    assert_eq!(report.nodes.nodes, 1, "one cut");
    let reject = report.reject.expect("binary maps write a reject");
    assert_eq!(reject.mode, "zero", "the default mode");
    assert_eq!(reject.sectors, 1, "one sector");
    assert_eq!(reject.bytes, 1, "one byte of bits");
    let blockmap = report.blockmap.expect("and a blockmap");
    assert!(blockmap.bytes > 0, "the grid is written: {blockmap:?}");
    assert_eq!(
        report.edit.get("map"),
        Some(&json!("MAP01")),
        "the fragment"
    );
    assert_eq!(
        report.edit.get("copied_to_working"),
        Some(&json!(true)),
        "the map came in to edit"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["build all nodes".to_owned()],
        "all three lumps are one step"
    );
    let map = working_map(&session);
    for wanted in lumps::BINARY_NODE_LUMPS {
        assert!(
            find_lump(&map.other, wanted).is_some_and(|lump| !lump.data.is_empty()),
            "{wanted:?} was written",
        );
    }
    session.undo().expect("undo the whole build");
    assert!(
        session.working().is_empty(),
        "undo restored the empty node lumps"
    );
}

#[test]
fn all_nodes_gl_run_attaches_the_gl_block_in_the_same_step() {
    let mut session = l_room_session();
    let report = all_nodes_run(&mut session, &all_params(None, Some("gl"), None))
        .expect("the gl build runs");
    assert_eq!(report.variant, "gl", "the echo");
    assert_eq!(
        report.nodes.gl_segs,
        Some(9),
        "seven real segs plus two minisegs"
    );
    assert!(report.reject.is_some(), "binary maps also get the reject");
    assert!(report.blockmap.is_some(), "and the blockmap");
    assert_eq!(
        session.undo_labels(),
        vec!["build all nodes".to_owned()],
        "tree, GL run, reject and grid: one step"
    );
    let names = lump_names(&session);
    assert!(
        names.contains(&"GL_MAP01".to_owned()),
        "the run is attached"
    );
    session.undo().expect("undo the whole build");
    assert!(
        lump_names(&session)
            .iter()
            .all(|lump| !lump.starts_with("GL_")),
        "undo took the GL run with everything else",
    );
}

#[test]
fn all_nodes_udmf_run_writes_znodes_only_unless_asked_for_a_reject() {
    let mut session = udmf_room_session();
    let report = all_nodes_run(&mut session, &all_params(None, None, None))
        .expect("the UDMF room fully builds");
    assert_eq!(report.variant, "extended", "the UDMF default");
    assert_eq!(report.nodes.format, Some("ZNOD"), "compressed by default");
    assert!(report.reject.is_none(), "ZDoom builds the reject itself");
    assert!(report.blockmap.is_none(), "and the blockmap");
    let names = lump_names(&session);
    assert!(names.contains(&"ZNODES".to_owned()), "ZNODES landed");
    assert!(
        !names
            .iter()
            .any(|lump| lump == "BLOCKMAP" || lump == "REJECT"),
        "neither side lump was added: {names:?}",
    );
    let asked = all_nodes_run(
        &mut session,
        &all_params(None, Some("EXTENDED"), Some("components")),
    )
    .expect("an explicit mode adds the matrix");
    assert_eq!(
        asked.reject.expect("the reject reports").mode,
        "components",
        "the echo"
    );
    let map = working_map(&session);
    assert!(
        find_lump(&map.other, lumps::REJECT).is_some_and(|lump| lump.size() == 1),
        "one sector's matrix landed in the block",
    );
    assert_eq!(session.undo_labels().len(), 2, "two builds, two steps");
}

#[test]
fn all_nodes_run_refuses_without_writing_anything() {
    let mut empty = sidedless_session();
    let error =
        all_nodes_run(&mut empty, &all_params(None, None, None)).expect_err("no segs to tree");
    assert_eq!(error.code(), "invalid_argument", "sidedless map code");
    assert!(error.to_string().contains("sidedefs"), "the hint: {error}");
    assert!(empty.working().is_empty(), "the refusal wrote nothing");
    assert!(empty.undo_labels().is_empty(), "and recorded nothing");
    let mut session = udmf_room_session();
    let forced = all_nodes_run(&mut session, &all_params(None, Some("classic"), None))
        .expect_err("classic has no place in UDMF");
    assert_eq!(
        forced.code(),
        "invalid_argument",
        "explicit classic on UDMF code"
    );
    assert!(
        session.working().is_empty(),
        "and again nothing was written"
    );
    let bad_mode = all_nodes_run(&mut session, &all_params(None, None, Some("opaque")))
        .expect_err("not a mode");
    assert_eq!(bad_mode.code(), "invalid_argument", "bad reject mode code");
    assert!(
        session.undo_labels().is_empty(),
        "no refusal entered history"
    );
}
