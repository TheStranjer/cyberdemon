//! The three readers: the `scope` and `detail` parameters, the shapes
//! [`crate::tools::nodes::get_nodes::read_node_data`] sorts a block into,
//! the reports they answer with, the `glBSP` run's counts and the
//! `full` assets, plus the locked runs of `get_nodes`, `get_reject` and
//! `get_blockmap` over a session's block.

use crate::session::Scope;
use crate::tools::maps::common::Directory;
use crate::tools::nodes::get_nodes::{
    GlInfo, NodeData, gl_info, lump_magic, nodes_asset_body, nodes_outcome, nodes_run,
    read_node_data,
};
use crate::tools::nodes::get_tables::{
    blockmap_run, blockmap_summary, empty_blockmap_report, reject_run,
};
use crate::tools::nodes::{Detail, json_bytes, parse_detail, parse_scope};
use crate::wad::bytes::Writer;
use crate::wad::classify::{MapBlock, analyze};
use crate::wad::map::{Blockmap, Map};

use super::{
    block_session, lump, marker, name, node_bytes, request, seg_bytes, vanilla_map, xnod_bytes,
};

#[test]
fn parse_scope_defaults_to_the_merged_view() {
    assert_eq!(parse_scope(None).expect("absent"), Scope::Merged);
    assert_eq!(
        parse_scope(Some("WORKING")).expect("working"),
        Scope::Working
    );
    let error = parse_scope(Some("nope")).expect_err("not a scope");
    assert_eq!(error.code(), "invalid_argument", "bad scope code");
}

#[test]
fn parse_detail_accepts_the_two_names_and_defaults_to_summary() {
    assert_eq!(parse_detail(None).expect("absent"), Detail::Summary);
    assert_eq!(parse_detail(Some("Full")).expect("full"), Detail::Full);
    assert_eq!(
        parse_detail(Some("SUMMARY")).expect("summary"),
        Detail::Summary
    );
    let error = parse_detail(Some("everything")).expect_err("not a detail level");
    assert_eq!(error.code(), "invalid_argument", "bad detail code");
    let message = error.to_string();
    assert!(
        message.contains("summary") && message.contains("full"),
        "the message names both values: {message}"
    );
}

#[test]
fn lump_magic_needs_four_printable_letters() {
    assert_eq!(
        lump_magic(b"gNd2 rest").as_deref(),
        Some("gNd2"),
        "a glBSP magic"
    );
    assert_eq!(lump_magic(b"..!").as_deref(), None, "too short");
    assert_eq!(
        lump_magic(b"\x01\x02\x03\x04").as_deref(),
        None,
        "not printable"
    );
}

#[test]
fn read_node_data_sorts_the_block_into_its_three_shapes() {
    let empty = Map::default();
    assert!(matches!(
        read_node_data(&empty).expect("no lumps"),
        NodeData::Empty
    ));
    let vanilla = read_node_data(&vanilla_map()).expect("the vanilla fixture parses");
    assert!(matches!(vanilla, NodeData::Parsed { .. }), "records parse");
    let xnodes = Map {
        other: vec![lump("NODES", &xnod_bytes())],
        ..Map::default()
    };
    let extended = read_node_data(&xnodes).expect("the XNOD fixture parses");
    assert!(
        matches!(extended, NodeData::Parsed { .. }),
        "extended parses"
    );
    let gl = Map {
        other: vec![lump("NODES", b"XGLN extended gl nodes")],
        ..Map::default()
    };
    let magic = read_node_data(&gl).expect("a GL magic is reported, not parsed");
    match magic {
        NodeData::GlMagic { format, bytes } => {
            assert_eq!(format, "xgln", "the lowercase magic");
            assert_eq!(bytes, 22, "the lump size travels with it");
        }
        NodeData::Empty | NodeData::Parsed { .. } => panic!("expected a GL magic"),
    }
}

#[test]
fn read_node_data_falls_back_to_znodes_and_refuses_magicless_ones() {
    let znodes = Map {
        other: vec![marker("NODES"), lump("ZNODES", &xnod_bytes())],
        ..Map::default()
    };
    assert!(
        matches!(
            read_node_data(&znodes).expect("the ZNODES parses"),
            NodeData::Parsed { .. }
        ),
        "an empty NODES defers to ZNODES",
    );
    let junk = Map {
        other: vec![lump("ZNODES", b"no magic here")],
        ..Map::default()
    };
    let error = read_node_data(&junk).expect_err("ZNODES needs a magic");
    assert_eq!(error.code(), "format_error", "magicless ZNODES code");
    assert!(
        error.to_string().contains("ZNODES"),
        "the lump is named: {error}"
    );
}

#[test]
fn reports_answer_absent_gl_and_parsed_shapes() {
    let gl = GlInfo {
        present: false,
        magic: None,
        segs: None,
        subsectors: None,
        nodes: None,
        vertices: None,
        lumps: alloc::collections::BTreeMap::new(),
    };
    let (none, none_set) = nodes_outcome(name("MAP01"), NodeData::Empty, gl.clone());
    assert!(none_set.is_none(), "absent data has nothing to publish");
    assert_eq!(none.format, "none", "absent data reports none");
    assert_eq!(none.bytes, 0, "and zero bytes");
    assert_eq!(none.segs, Some(0), "with zero counts");
    assert_eq!(none.root, None, "no root to name");
    let magic = NodeData::GlMagic {
        format: "zgl3",
        bytes: 40,
    };
    let (gl_report, gl_set) = nodes_outcome(name("MAP01"), magic, gl.clone());
    assert!(gl_set.is_none(), "an unparsed magic has no set");
    assert_eq!(gl_report.format, "zgl3", "the magic travels");
    assert_eq!(gl_report.bytes, 40, "with its size");
    assert_eq!(gl_report.nodes, None, "unparsed counts are null");
    assert_eq!(gl_report.depth, None, "and no depth");
    let parsed = read_node_data(&vanilla_map()).expect("parses");
    let (parsed_report, parsed_set) = nodes_outcome(name("MAP01"), parsed, gl);
    assert!(parsed_set.is_some(), "the parsed set travels back");
    assert_eq!(parsed_report.format, "vanilla", "the parsed layout");
    assert_eq!(parsed_report.segs, Some(1), "one seg");
    assert_eq!(parsed_report.subsectors, Some(1), "one subsector");
    assert_eq!(parsed_report.nodes, Some(1), "one node");
    assert_eq!(
        parsed_report.extra_vertices,
        Some(0),
        "vanilla adds no vertices"
    );
    assert_eq!(parsed_report.root, Some(0), "the only node is the root");
    assert_eq!(parsed_report.depth, Some(1), "one level deep");
}

#[test]
fn the_full_nodes_asset_carries_every_parsed_record() {
    let parsed = read_node_data(&vanilla_map()).expect("parses");
    let NodeData::Parsed { set, .. } = parsed else {
        panic!("the vanilla fixture parses");
    };
    let json = serde_json::to_value(nodes_asset_body(&set)).expect("the asset serializes");
    assert_eq!(json["format"], serde_json::json!("vanilla"), "the layout");
    assert_eq!(
        json["segs"].as_array().expect("segs").len(),
        1,
        "one seg row"
    );
    assert_eq!(
        json["nodes"][0]["left_child"],
        serde_json::json!({ "subsector": 0_u64 }),
        "tagged children",
    );
    assert_eq!(json["extra_vertices"], serde_json::json!([]), "none here");
}

#[test]
fn gl_info_reports_the_attached_run_and_its_magic() {
    let lumps = vec![
        marker("MAP01"),
        marker("THINGS"),
        marker("LINEDEFS"),
        marker("SIDEDEFS"),
        marker("VERTEXES"),
        marker("SECTORS"),
        marker("NODES"),
        marker("GL_MAP01"),
        lump("GL_VERT", b"gNd5 payload"),
        lump("GL_NODES", b"gl nodes"),
    ];
    let block = analyze(&lumps).maps.first().expect("a block").clone();
    let dir = Directory {
        entries: Vec::new(),
        analysis: analyze(&lumps),
        lumps,
    };
    let info = gl_info(&dir, &block);
    assert!(info.present, "a GL run is attached");
    assert_eq!(info.magic.as_deref(), Some("gNd5"), "the GL_VERT magic");
    assert_eq!(info.lumps.len(), 3, "header and two members");
    assert_eq!(info.lumps.get("GL_VERT").copied(), Some(12), "GL_VERT size");
    let block_without_gl = MapBlock { gl: None, ..block };
    assert!(
        !gl_info(&dir, &block_without_gl).present,
        "no run, no report"
    );
}

#[test]
fn nodes_run_reports_a_decoded_block_and_holds_back_the_asset() {
    let session = block_session(vec![
        lump("SEGS", &seg_bytes()),
        lump("SSECTORS", &[1, 0, 0, 0]),
        lump("NODES", &node_bytes()),
    ]);
    let (report, publish) =
        nodes_run(&session, &request(None, Scope::Merged, Detail::Summary)).expect("runs");
    assert_eq!(report.map, name("MAP01"), "the map travels");
    assert_eq!(report.format, "vanilla", "the parsed layout");
    assert_eq!(report.segs, Some(1), "one seg");
    assert!(publish.is_none(), "summary publishes nothing");
    let (full, held) = nodes_run(
        &session,
        &request(Some("map01"), Scope::Merged, Detail::Full),
    )
    .expect("full detail runs");
    assert!(full.asset.is_none(), "the slot fills only after delivery");
    let job = held.expect("full detail holds a body back");
    assert_eq!(job.filename, "MAP01_nodes.json", "the asset file name");
    assert!(job.data.starts_with(b"{"), "a JSON body");
}

#[test]
fn nodes_run_refuses_an_undecodable_block() {
    let session = block_session(vec![lump("NODES", b"123")]);
    let broken = nodes_run(&session, &request(None, Scope::Merged, Detail::Summary))
        .expect_err("3 bytes are no NODES record");
    assert_eq!(broken.code(), "format_error", "malformed nodes code");
    let missing = nodes_run(
        &session,
        &request(Some("MAP09"), Scope::Merged, Detail::Summary),
    )
    .expect_err("MAP09 is in no archive");
    assert_eq!(missing.code(), "not_found", "unknown map code");
    let bad_scope = nodes_run(&session, &request(None, Scope::Layer(4), Detail::Summary))
        .expect_err("layer 4 is not loaded");
    assert_eq!(bad_scope.code(), "not_found", "unloaded layer code");
}

#[test]
fn reject_run_measures_the_matrix_without_failing_on_sizes() {
    let session = block_session(vec![lump("REJECT", &[0b0000_0010_u8])]);
    let (report, publish) =
        reject_run(&session, &request(None, Scope::Merged, Detail::Summary)).expect("runs");
    assert_eq!(report.bytes, 1, "one byte stored");
    assert_eq!(report.expected_bytes, 1, "two sectors need one byte");
    assert_eq!(report.sectors, 2, "the decoded sector count");
    assert!(report.size_ok, "the sizes agree");
    assert!(!report.all_zero, "one pair is rejected");
    assert_eq!(report.rejected_pairs, 1, "and it counts");
    assert!(publish.is_none(), "summary publishes nothing");
    let (full, held) =
        reject_run(&session, &request(None, Scope::Merged, Detail::Full)).expect("full runs");
    assert!(full.asset.is_none(), "the slot fills only after delivery");
    let job = held.expect("full detail holds a body back");
    assert_eq!(job.filename, "MAP01_reject.json", "the asset file name");
    let text = String::from_utf8(job.data).expect("json is utf-8");
    assert!(
        text.contains("\"01\"") && text.contains("\"00\""),
        "the rows: {text}"
    );
    let empty = block_session(vec![marker("REJECT")]);
    let (zero, _) =
        reject_run(&empty, &request(None, Scope::Merged, Detail::Summary)).expect("empty is data");
    assert_eq!(zero.bytes, 0, "an empty lump reports zero bytes");
    assert!(zero.all_zero, "and nothing rejected");
}

#[test]
fn blockmap_run_summarizes_or_zeros_and_refuses_malformed_lists() {
    let mut w = Writer::new();
    w.put_i16(0);
    w.put_i16(0);
    w.put_u16(1);
    w.put_u16(2);
    w.put_u16(6);
    w.put_u16(9);
    w.put_u16(0);
    w.put_u16(3);
    w.put_u16(0xFFFF);
    w.put_u16(0);
    w.put_u16(0xFFFF);
    let good = w.into_inner();
    let bytes = good.len();
    let session = block_session(vec![lump("BLOCKMAP", &good)]);
    let (report, held) =
        blockmap_run(&session, &request(None, Scope::Merged, Detail::Full)).expect("runs");
    assert_eq!(report.bytes, bytes, "the lump size");
    assert_eq!((report.columns, report.rows), (1, 2), "the grid");
    assert_eq!(report.blocks, 2, "two cells");
    assert_eq!(report.total_entries, 1, "one reference");
    assert_eq!(report.max_lines_per_block, 1, "in one list");
    assert_eq!(report.empty_blocks, 1, "beside an empty one");
    let job = held.expect("full detail holds a body back");
    assert_eq!(job.filename, "MAP01_blockmap.json", "the asset file name");
    let empty = block_session(vec![marker("BLOCKMAP")]);
    let (zero, _) =
        blockmap_run(&empty, &request(None, Scope::Merged, Detail::Full)).expect("empty is data");
    assert_eq!(zero.bytes, 0, "an empty lump reports zero bytes");
    assert_eq!(zero.blocks, 0, "and no cells");
    let broken = block_session(vec![lump("BLOCKMAP", b"12345678")]);
    let err = blockmap_run(&broken, &request(None, Scope::Merged, Detail::Summary))
        .expect_err("an empty grid with no lists is malformed");
    assert_eq!(err.code(), "format_error", "malformed blockmap code");
}

#[test]
fn blockmap_reports_summarize_share_and_empty_blocks() {
    let empty = empty_blockmap_report(name("MAP01"));
    assert_eq!(empty.bytes, 0, "absent data reports zero bytes");
    assert_eq!(empty.columns, 0, "and no grid");
    assert_eq!(empty.max_lines_per_block, 0, "no lists");

    let mut w = Writer::new();
    w.put_i16(-128);
    w.put_i16(-256);
    w.put_u16(1);
    w.put_u16(2);
    // Two cells: one list of two linedefs at word 6, one empty list
    // at word 10 (the table holds two words at 4 and 5).
    w.put_u16(6);
    w.put_u16(10);
    w.put_u16(0);
    w.put_u16(5);
    w.put_u16(9);
    w.put_u16(0xFFFF);
    w.put_u16(0);
    w.put_u16(0xFFFF);
    let parsed = Blockmap::parse(&w.into_inner()).expect("the fixture parses");
    let report = blockmap_summary(name("MAP01"), 24, &parsed);
    assert_eq!(report.blocks, 2, "two cells");
    assert_eq!(report.total_entries, 2, "two references");
    assert_eq!(report.max_lines_per_block, 2, "the wider list");
    assert_eq!(report.empty_blocks, 1, "the empty cell");
    assert_eq!(report.origin.x, -128, "the origin object");
    assert_eq!(report.origin.y, -256, "on both axes");
    let json = json_bytes(&parsed).expect("the asset serializes");
    assert!(
        String::from_utf8_lossy(&json).contains("\"blocks\""),
        "the full body carries the lists",
    );
}
