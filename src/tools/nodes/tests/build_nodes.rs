//! The tree builder's own two layouts: the `variant` names and the map
//! format's default they resolve to, the classic run's counts and its
//! split vertices (and the geometry edit that strips them again), the
//! cost knob, and the extended lump in `NODES` of a binary block and in
//! `ZNODES` of a `UDMF` one - with the refusals that write nothing.

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::nodes::build_nodes::{Variant, nodes_build_run};
use crate::tools::nodes::get_nodes::{NodeData, read_node_data};
use crate::tools::nodes::{BuildNodesParams, find_lump};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::nodes;
use serde_json::json;

use super::{
    build_params, l_room_session, name, sidedless_session, udmf_room_session, working_map,
};

/// `build_nodes` extended arguments with an explicit `compress`.
fn extended_params(compress: Option<bool>) -> BuildNodesParams {
    BuildNodesParams {
        map: None,
        variant: Some("extended".to_owned()),
        split_cost: None,
        compress,
    }
}

#[test]
fn variant_names_parse_and_follow_the_map_format() {
    assert_eq!(Variant::parse("").expect("empty"), Variant::Classic);
    assert_eq!(
        Variant::parse("Classic").expect("classic"),
        Variant::Classic
    );
    assert_eq!(
        Variant::parse("extended").expect("extended"),
        Variant::Extended
    );
    assert_eq!(Variant::parse("GL").expect("gl"), Variant::Gl);
    assert_eq!(
        Variant::for_format(MapFormat::Doom),
        Variant::Classic,
        "a Doom map defaults to the vanilla run"
    );
    assert_eq!(
        Variant::for_format(MapFormat::Hexen),
        Variant::Classic,
        "a Hexen map too"
    );
    assert_eq!(
        Variant::for_format(MapFormat::Udmf),
        Variant::Extended,
        "a UDMF map defaults to the only layout it reads"
    );
    assert_eq!(
        Variant::resolve(None, MapFormat::Udmf).expect("absent takes the format default"),
        Variant::Extended
    );
    assert_eq!(
        Variant::resolve(Some("extended"), MapFormat::Doom).expect("explicit wins"),
        Variant::Extended
    );
    let bad = Variant::parse("znodes").expect_err("not a variant");
    assert_eq!(bad.code(), "invalid_argument", "bad variant code");
    let message = bad.to_string();
    assert!(
        message.contains("classic") && message.contains("extended") && message.contains("gl"),
        "the message names all three: {message}"
    );
}

#[test]
fn nodes_build_run_writes_the_vanilla_run_and_is_undoable() {
    let mut session = l_room_session();
    let report =
        nodes_build_run(&mut session, &build_params(None, None, None)).expect("the L room builds");
    assert_eq!(report.body.variant, "classic", "the echo");
    assert_eq!(report.body.format, None, "classic carries no magic");
    assert_eq!(report.body.segs, 7, "six sides plus the cut piece");
    assert_eq!(
        report.body.subsectors, 2,
        "the concave corner needs two leaves"
    );
    assert_eq!(report.body.nodes, 1, "one cut suffices");
    assert_eq!(
        report.body.split_vertices,
        Some(1),
        "the bottom wall splits at x=128"
    );
    assert_eq!(
        report.body.new_vertices, None,
        "classic appends, it does not store"
    );
    assert!(report.body.warnings.is_empty(), "one sector only");
    assert_eq!(
        report.edit.get("map"),
        Some(&json!("MAP01")),
        "the fragment"
    );
    assert_eq!(
        report.edit.get("nodes_invalidated"),
        Some(&json!(false)),
        "appending split vertices is not a geometry change"
    );
    assert_eq!(report.edit.get("copied_to_working"), Some(&json!(true)));
    assert_eq!(
        session.undo_labels(),
        vec!["build nodes".to_owned()],
        "one step"
    );
    let map = working_map(&session);
    assert_eq!(map.vertices.len(), 7, "the split vertex joined VERTEXES");
    let run = find_lump(&map.other, lumps::NODES).expect("NODES kept");
    assert_eq!(run.size(), 28, "one node record");
    let segs = find_lump(&map.other, lumps::SEGS).expect("SEGS kept");
    assert_eq!(segs.size(), 84, "seven seg records");
    let again = nodes_build_run(&mut session, &build_params(None, Some("CLASSIC"), None))
        .expect("a rebuild runs");
    assert_eq!(
        again.body.split_vertices,
        Some(0),
        "the split point is reused, not duplicated"
    );
    assert_eq!(again.body.segs, 7, "the same tree");
}

#[test]
fn a_geometry_edit_after_a_build_strips_the_split_vertices() {
    let mut session = l_room_session();
    nodes_build_run(&mut session, &build_params(None, None, None)).expect("the L room builds");
    let ((), moved) = session
        .edit_map(name("MAP01"), "move corner", |model| {
            let vertex = model
                .vertices
                .first_mut()
                .ok_or_else(|| CyberError::InvalidArgument("no vertices".to_owned()))?;
            vertex.x = vertex.x.checked_add(100_i32).expect("the move fits");
            Ok(())
        })
        .expect("the move lands");
    assert!(moved.nodes_invalidated, "moving a used vertex is geometry");
    let after = working_map(&session);
    assert_eq!(
        after.vertices.len(),
        6,
        "the split vertex went with the invalidated nodes"
    );
    assert_eq!(
        find_lump(&after.other, lumps::NODES)
            .expect("placeholder kept")
            .size(),
        0,
        "and the run emptied"
    );
}

#[test]
fn split_cost_travels_into_the_build_options() {
    let mut session = l_room_session();
    let cheap = nodes_build_run(&mut session, &build_params(None, None, Some(1_i64)))
        .expect("a cheap split still builds the L");
    assert_eq!(cheap.body.segs, 7, "one valid partition either way");
    assert_eq!(cheap.body.subsectors, 2, "the same leaves");
    let mut rerun = l_room_session();
    let dear = nodes_build_run(&mut rerun, &build_params(Some("map01"), None, Some(64_i64)))
        .expect("an expensive split still builds the L");
    assert_eq!(
        dear.body.split_vertices,
        Some(1),
        "the cut is still the only choice"
    );
    assert_eq!(
        dear.edit.get("map"),
        Some(&json!("MAP01")),
        "lowercase resolved"
    );
}

/// The working archive's decoded `MAP01` block's parsed extended node
/// run, whichever lump carries it.
fn working_run(session: &Session) -> nodes::NodeSet {
    let map = working_map(session);
    match read_node_data(&map).expect("the written lump reads back") {
        NodeData::Parsed { set, .. } => set,
        NodeData::Empty | NodeData::GlMagic { .. } => panic!("the build wrote a parsed run"),
    }
}

/// The L room's extended run: seven segs in two leaves, one node, the
/// cut point stored at full `16.16` in the lump.
fn assert_extended_tree(set: &nodes::NodeSet, format: nodes::NodesFormat) {
    assert_eq!(set.format, format, "the parsed layout");
    assert_eq!(set.segs.len(), 7, "seven segs");
    assert_eq!(set.subsectors.len(), 2, "two leaves");
    assert_eq!(set.nodes.len(), 1, "one node");
    assert_eq!(
        set.vertices_extra.first().map(|pair| (pair.0.0, pair.1.0)),
        Some((
            128_i32.checked_mul(0x1_0000).expect("128 fits 16.16"),
            0_i32
        )),
        "the cut point keeps its full 16.16 form"
    );
}

/// The binary block of an extended build: `NODES` carries the lump,
/// `SEGS` and `SSECTORS` sit emptied beside it and `VERTEXES` never
/// grew.
fn assert_binary_lump(session: &Session, magic: &[u8], lump_size: usize) {
    let map = working_map(session);
    assert_eq!(
        map.vertices.len(),
        6,
        "the split vertex did NOT join VERTEXES"
    );
    let run = find_lump(&map.other, lumps::NODES).expect("NODES kept");
    assert!(run.data.starts_with(magic), "the extended magic leads");
    assert_eq!(run.size(), lump_size, "the lump holds what was written");
    assert_eq!(
        find_lump(&map.other, lumps::SEGS)
            .expect("SEGS kept")
            .size(),
        0,
        "SEGS is emptied: ZDoom ignores it when NODES is extended"
    );
    assert_eq!(
        find_lump(&map.other, lumps::SSECTORS)
            .expect("SSECTORS kept")
            .size(),
        0,
        "and so SSECTORS"
    );
}

/// The working archive's lump names, in directory order.
fn working_names(session: &Session) -> Vec<String> {
    session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect()
}

/// The position of the name `wanted` in `names`.
fn position(names: &[String], wanted: &str) -> usize {
    names
        .iter()
        .position(|name| name == wanted)
        .unwrap_or_else(|| panic!("no {wanted} lump in {names:?}"))
}

#[test]
fn extended_build_writes_a_znod_nodes_lump_and_leaves_vertices_alone() {
    let mut session = l_room_session();
    let report =
        nodes_build_run(&mut session, &extended_params(None)).expect("the L room builds extended");
    assert_eq!(report.body.variant, "extended", "the echo");
    assert_eq!(report.body.format, Some("ZNOD"), "compressed by default");
    assert_eq!(report.body.segs, 7, "the cut piece counts");
    assert_eq!(report.body.subsectors, 2, "the arms are leaves");
    assert_eq!(report.body.nodes, 1, "one cut suffices");
    assert_eq!(
        report.body.new_vertices,
        Some(1),
        "the split point rides in the lump"
    );
    assert_eq!(report.body.split_vertices, None, "VERTEXES gain nothing");
    assert!(report.body.warnings.is_empty(), "one sector only");
    assert_eq!(
        report.edit.get("map"),
        Some(&json!("MAP01")),
        "the fragment"
    );
    assert_eq!(
        report.edit.get("nodes_invalidated"),
        Some(&json!(false)),
        "a build is not geometry"
    );
    assert_eq!(report.edit.get("copied_to_working"), Some(&json!(true)));
    assert_eq!(
        session.undo_labels(),
        vec!["build nodes".to_owned()],
        "one step"
    );
    let lump_size = report.body.bytes.expect("the lump size is reported");
    assert!(lump_size > 4, "magic plus a payload: {lump_size}");
    assert_binary_lump(&session, b"ZNOD", lump_size);
    assert_extended_tree(&working_run(&session), nodes::NodesFormat::Znod);
}

#[test]
fn an_uncompressed_extended_build_stays_raw_behind_xnod() {
    let mut session = l_room_session();
    let report =
        nodes_build_run(&mut session, &extended_params(Some(false))).expect("the raw build runs");
    assert_eq!(report.body.format, Some("XNOD"), "compress false stays raw");
    let lump_size = report.body.bytes.expect("a lump size");
    assert!(lump_size > 4, "magic plus a payload: {lump_size}");
    assert_binary_lump(&session, b"XNOD", lump_size);
    assert_extended_tree(&working_run(&session), nodes::NodesFormat::Xnod);
}

#[test]
fn extended_build_on_udmf_writes_znodes_between_the_markers() {
    let mut session = udmf_room_session();
    let report = nodes_build_run(&mut session, &extended_params(Some(false)))
        .expect("the UDMF room builds extended raw");
    assert_eq!(report.body.format, Some("XNOD"), "compress false stays raw");
    assert_eq!(
        report.body.new_vertices,
        Some(1),
        "the cut rides in the lump"
    );
    assert_eq!(report.body.subsectors, 2, "the same tree");
    let map = working_map(&session);
    let run = find_lump(&map.other, lumps::ZNODES).expect("ZNODES written");
    assert!(run.data.starts_with(b"XNOD"), "the raw magic leads");
    assert!(
        map.other
            .iter()
            .all(|lump| !matches!(lump.name.as_bytes(), b"NODES" | b"SEGS" | b"SSECTORS")),
        "no stale binary run is carried in the block",
    );
    let names = working_names(&session);
    assert!(
        position(&names, "TEXTMAP") < position(&names, "ZNODES")
            && position(&names, "ZNODES") < position(&names, "ENDMAP"),
        "ZNODES sits between the markers: {names:?}"
    );
    assert_extended_tree(&working_run(&session), nodes::NodesFormat::Xnod);
    let again = nodes_build_run(&mut session, &extended_params(None)).expect("a rebuild runs");
    assert_eq!(again.body.format, Some("ZNOD"), "the default compresses");
    let remade = working_map(&session);
    let stored: usize = remade
        .other
        .iter()
        .filter(|lump| lump.name.as_bytes() == b"ZNODES")
        .count();
    assert_eq!(stored, 1, "the stale lump is replaced, not stacked");
    assert_eq!(session.undo_labels().len(), 2, "two builds, two steps");
}

#[test]
fn build_nodes_on_udmf_without_a_variant_defaults_to_extended() {
    let mut session = udmf_room_session();
    let report = nodes_build_run(&mut session, &build_params(None, None, None))
        .expect("a parameterless UDMF build takes the extended layout");
    assert_eq!(report.body.variant, "extended", "the format default");
    assert_eq!(report.body.format, Some("ZNOD"), "compressed by default");
    assert_eq!(report.body.subsectors, 2, "the same tree");
    let map = working_map(&session);
    assert!(
        find_lump(&map.other, lumps::ZNODES).is_some(),
        "the run landed in ZNODES"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["build nodes".to_owned()],
        "one step"
    );
}

#[test]
fn classic_build_refuses_a_udmf_map_without_writing() {
    let mut session = udmf_room_session();
    let error = nodes_build_run(&mut session, &build_params(None, Some("classic"), None))
        .expect_err("UDMF has no place for the classic run");
    assert_eq!(error.code(), "invalid_argument", "classic on UDMF code");
    assert!(
        error.to_string().contains("UDMF maps use extended nodes"),
        "the message names the rule: {error}"
    );
    assert!(session.working().is_empty(), "the refusal wrote nothing");
    assert!(session.undo_labels().is_empty(), "and recorded nothing");
}

#[test]
fn nodes_build_run_refuses_without_writing_anything() {
    let mut empty = sidedless_session();
    let error =
        nodes_build_run(&mut empty, &build_params(None, None, None)).expect_err("no segs to tree");
    assert_eq!(error.code(), "invalid_argument", "sidedless map code");
    assert!(error.to_string().contains("sidedefs"), "the hint: {error}");
    let empty_extended = nodes_build_run(&mut empty, &extended_params(None))
        .expect_err("the extended variant needs sides too");
    assert_eq!(
        empty_extended.code(),
        "invalid_argument",
        "sidedless extended map code"
    );
    let mut session = l_room_session();
    let missing = nodes_build_run(&mut session, &build_params(Some("MAP09"), None, None))
        .expect_err("no MAP09 anywhere");
    assert_eq!(missing.code(), "not_found", "unknown map code");
    let weird = nodes_build_run(&mut session, &build_params(None, Some("znodes"), None))
        .expect_err("not a variant");
    assert_eq!(weird.code(), "invalid_argument", "unknown variant code");
    assert!(session.working().is_empty(), "no refusal wrote anything");
    assert!(session.undo_labels().is_empty(), "and none was recorded");
}
