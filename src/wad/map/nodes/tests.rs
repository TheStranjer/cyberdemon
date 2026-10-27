//! Tests for the node-lump parsers: the two layouts' fixtures, their
//! cross-reference and truncation refusals, magic reporting and `JSON`
//! shapes.

use super::{
    BoundingBox, Child, Node, NodeMagic, Seg, Subsector, extended::extended_child, lump_format,
    lump_head, parse_extended_nodes, parse_vanilla_nodes, seg_vertex_indices,
    vanilla::vanilla_child,
};
use crate::fixed::Fixed;
use crate::wad::bytes::Writer;
use alloc::collections::BTreeSet;

/// One node magic with the letters it is stored as and the name it reports.
type MagicRow = (NodeMagic, &'static [u8; 4], &'static str);

/// Every magic the sniffer knows, spelled out: the digits of the `GL`
/// variants are the point, and `Vanilla` is the absence of all of them.
const MAGIC_ROWS: [MagicRow; 8] = [
    (NodeMagic::Xnod, b"XNOD", "xnod"),
    (NodeMagic::Znod, b"ZNOD", "znod"),
    (NodeMagic::Xgln, b"XGLN", "xgln"),
    (NodeMagic::Zgln, b"ZGLN", "zgln"),
    (NodeMagic::Xgl2, b"XGL2", "xgl2"),
    (NodeMagic::Zgl2, b"ZGL2", "zgl2"),
    (NodeMagic::Xgl3, b"XGL3", "xgl3"),
    (NodeMagic::Zgl3, b"ZGL3", "zgl3"),
];

/// Appends one 12-byte vanilla `SEGS` record.
fn put_seg(w: &mut Writer, fields: (u16, u16, i16, u16, i16, i16)) {
    let (v1, v2, angle, linedef, direction, offset) = fields;
    w.put_u16(v1);
    w.put_u16(v2);
    w.put_i16(angle);
    w.put_u16(linedef);
    w.put_i16(direction);
    w.put_i16(offset);
}

/// A vanilla bounding-box quad: `top`, `bottom`, `left`, `right`.
fn put_box(w: &mut Writer, edges: (i16, i16, i16, i16)) {
    let (top, bottom, left, right) = edges;
    w.put_i16(top);
    w.put_i16(bottom);
    w.put_i16(left);
    w.put_i16(right);
}

/// A vanilla node record with a unit-`dx` partition line.
fn put_node(w: &mut Writer, right_child: u16, left_child: u16) {
    w.put_i16(0);
    w.put_i16(0);
    w.put_i16(1);
    w.put_i16(0);
    put_box(w, (1, 0, 0, 1));
    put_box(w, (1, 0, 0, 1));
    w.put_u16(right_child);
    w.put_u16(left_child);
}

/// The three vanilla record lumps of a minimal run.
type LumpTrio = (Vec<u8>, Vec<u8>, Vec<u8>);

/// Two segs, two subsectors of one seg each and one node whose both
/// children are subsectors: the smallest interesting vanilla run.
fn vanilla_lumps() -> LumpTrio {
    let mut segs = Writer::new();
    put_seg(&mut segs, (0, 1, 0, 0, 0, 0));
    put_seg(&mut segs, (1, 2, -1, 1, 1, 64));
    let mut ssectors = Writer::new();
    ssectors.put_u16(1);
    ssectors.put_u16(0);
    ssectors.put_u16(1);
    ssectors.put_u16(1);
    let mut nodes = Writer::new();
    put_node(&mut nodes, 0x8001, 0x8000);
    (segs.into_inner(), ssectors.into_inner(), nodes.into_inner())
}

/// The [`Node`] the [`vanilla_lumps`] root decodes to.
fn expected_root_node() -> Node {
    Node {
        x: 0,
        y: 0,
        dx: 1,
        dy: 0,
        right: BoundingBox {
            top: 1,
            bottom: 0,
            left: 0,
            right: 1,
        },
        left: BoundingBox {
            top: 1,
            bottom: 0,
            left: 0,
            right: 1,
        },
        right_child: Child::Subsector(1),
        left_child: Child::Subsector(0),
    }
}

/// The extended payload (magic excluded): one added vertex, two
/// subsectors of one seg, two segs reaching the added vertex and one
/// node with subsector children 0 and 1.
fn extended_payload() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_u32(6);
    w.put_u32(1);
    w.put_i32(0x0001_0000);
    w.put_i32(-0x0004_0000);
    w.put_u32(2);
    w.put_u32(1);
    w.put_u32(1);
    w.put_u32(2);
    for (v1, v2, linedef, side) in [(6, 0, 0_u16, 0_u8), (6, 1, 1, 1)] {
        w.put_u32(v1);
        w.put_u32(v2);
        w.put_u16(linedef);
        w.put_u8(side);
    }
    w.put_u32(1);
    w.put_i16(0);
    w.put_i16(0);
    w.put_i16(64);
    w.put_i16(0);
    put_box(&mut w, (256, 0, 0, 256));
    put_box(&mut w, (256, 0, 0, 256));
    w.put_u32(0x8000_0000);
    w.put_u32(0x8000_0001);
    w.into_inner()
}

#[test]
fn vanilla_lumps_parse_to_the_expected_structure() {
    let (segs, ssectors, nodes) = vanilla_lumps();
    let set = parse_vanilla_nodes(&segs, &ssectors, &nodes).expect("the fixture parses");
    assert_eq!(set.format, super::NodesFormat::Vanilla, "vanilla layout");
    assert!(set.vertices_extra.is_empty(), "no extra vertices");
    assert_eq!(
        set.segs,
        vec![
            Seg {
                v1: 0,
                v2: 1,
                angle: 0,
                linedef: 0,
                direction: 0,
                offset: 0,
            },
            Seg {
                v1: 1,
                v2: 2,
                angle: -1,
                linedef: 1,
                direction: 1,
                offset: 64,
            },
        ],
        "the two segs"
    );
    assert_eq!(
        set.subsectors,
        vec![
            Subsector {
                seg_count: 1,
                first_seg: 0
            },
            Subsector {
                seg_count: 1,
                first_seg: 1
            },
        ],
        "the two subsectors"
    );
    assert_eq!(set.nodes, vec![expected_root_node()], "one node");
    assert_eq!(set.root(), Some(0), "the last node is the root");
    assert_eq!(set.depth(), Some(1), "one node level");
}

#[test]
fn empty_vanilla_lumps_parse_empty() {
    let set = parse_vanilla_nodes(b"", b"", b"").expect("empty inputs parse");
    assert!(set.segs.is_empty(), "no segs");
    assert!(set.subsectors.is_empty(), "no subsectors");
    assert!(set.nodes.is_empty(), "no nodes");
    assert_eq!(set.root(), None, "no root without nodes");
    assert_eq!(set.depth(), None, "no depth without nodes");
}

#[test]
fn nested_vanilla_children_walk_the_depth() {
    let (segs, _, _) = vanilla_lumps();
    let mut ssectors = Writer::new();
    ssectors.put_u16(2);
    ssectors.put_u16(0);
    let mut nodes = Writer::new();
    put_node(&mut nodes, 0x8000, 0x8000);
    put_node(&mut nodes, 0, 0x8000);
    let set = parse_vanilla_nodes(&segs, &ssectors.into_inner(), &nodes.into_inner())
        .expect("a two-level tree parses");
    assert_eq!(set.root(), Some(1), "the last node is the root");
    assert_eq!(set.depth(), Some(2), "root plus one interior level");
}

#[test]
fn vanilla_truncations_are_format_errors() {
    let (_, ssectors, nodes) = vanilla_lumps();
    let segs_error = parse_vanilla_nodes(b"12345", &ssectors, &nodes)
        .expect_err("5 bytes are no 12-byte record");
    assert_eq!(segs_error.code(), "format_error", "SEGS remainder code");
    assert!(
        segs_error.to_string().contains("SEGS"),
        "SEGS named: {segs_error}"
    );
    let ssectors_error =
        parse_vanilla_nodes(b"", b"123", &nodes).expect_err("3 bytes are no 4-byte record");
    assert!(
        ssectors_error.to_string().contains("SSECTORS"),
        "SSECTORS named: {ssectors_error}"
    );
    let nodes_error =
        parse_vanilla_nodes(b"", b"", b"123").expect_err("3 bytes are no 28-byte record");
    assert!(
        nodes_error.to_string().contains("NODES"),
        "NODES named: {nodes_error}"
    );
}

#[test]
fn vanilla_dangling_references_are_format_errors() {
    let (segs, ssectors, _) = vanilla_lumps();
    let mut leaf_beyond = Writer::new();
    put_node(&mut leaf_beyond, 0x8005, 0x8000);
    let error = parse_vanilla_nodes(&segs, &ssectors, &leaf_beyond.into_inner())
        .expect_err("subsector 5 does not exist");
    assert_eq!(error.code(), "format_error", "dangling leaf code");
    assert!(
        error.to_string().contains("subsector"),
        "leaf named: {error}"
    );
    let mut forward = Writer::new();
    put_node(&mut forward, 1, 0x8000);
    let forward_error = parse_vanilla_nodes(&segs, &ssectors, &forward.into_inner())
        .expect_err("node 0 cannot reference node 1");
    assert!(
        forward_error.to_string().contains("earlier node"),
        "the backwards rule is named: {forward_error}"
    );
    let mut over = Writer::new();
    over.put_u16(9);
    over.put_u16(0);
    let mut node_zero = Writer::new();
    put_node(&mut node_zero, 0x8000, 0x8000);
    let range_error = parse_vanilla_nodes(&segs, &over.into_inner(), &node_zero.into_inner())
        .expect_err("the subsector reaches past the segs");
    assert!(
        range_error.to_string().contains("past the"),
        "the seg range is named: {range_error}"
    );
}

#[test]
fn xnod_bytes_parse_to_the_expected_structure() {
    let mut bytes = b"XNOD".to_vec();
    bytes.extend_from_slice(&extended_payload());
    let set = parse_extended_nodes(&bytes).expect("the fixture parses");
    assert_eq!(
        set.format,
        super::NodesFormat::Xnod,
        "raw payload keeps its magic"
    );
    assert_eq!(
        set.vertices_extra,
        vec![(Fixed(0x0001_0000), Fixed(-0x0004_0000))],
        "one added 16.16 vertex"
    );
    assert_eq!(
        set.subsectors,
        vec![
            Subsector {
                seg_count: 1,
                first_seg: 0
            },
            Subsector {
                seg_count: 1,
                first_seg: 1
            },
        ],
        "first_seg is the running total"
    );
    assert_eq!(
        set.segs,
        vec![
            Seg {
                v1: 6,
                v2: 0,
                angle: 0,
                linedef: 0,
                direction: 0,
                offset: 0,
            },
            Seg {
                v1: 6,
                v2: 1,
                angle: 0,
                linedef: 1,
                direction: 1,
                offset: 0,
            },
        ],
        "segs reference the extra vertex 6"
    );
    assert_eq!(set.nodes.len(), 1, "one node");
    assert_eq!(
        set.nodes.first().expect("the root").right_child,
        Child::Subsector(0),
        "bit 31 flags the leaf"
    );
    assert_eq!(set.root(), Some(0), "the last node is the root");
    assert_eq!(set.depth(), Some(1), "one level");
}

#[test]
fn znod_inflates_to_the_same_nodeset_as_xnod() {
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    let payload = extended_payload();
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&payload).expect("encoding fits");
    let mut bytes = b"ZNOD".to_vec();
    bytes.extend_from_slice(&encoder.finish().expect("the encoder closes"));
    let zipped = parse_extended_nodes(&bytes).expect("the ZNOD fixture parses");
    let mut plain = b"XNOD".to_vec();
    plain.extend_from_slice(&payload);
    let raw = parse_extended_nodes(&plain).expect("the XNOD fixture parses");
    assert_eq!(
        zipped.format,
        super::NodesFormat::Znod,
        "compressed report name"
    );
    assert_eq!(raw.format, super::NodesFormat::Xnod, "raw report name");
    assert_eq!(zipped.vertices_extra, raw.vertices_extra, "same vertices");
    assert_eq!(zipped.subsectors, raw.subsectors, "same subsectors");
    assert_eq!(zipped.segs, raw.segs, "same segs");
    assert_eq!(zipped.nodes, raw.nodes, "same nodes");
}

#[test]
fn extended_malformations_are_format_errors() {
    let payload = extended_payload();
    let short_magic = parse_extended_nodes(b"XNO").expect_err("3 bytes hold no magic");
    assert_eq!(short_magic.code(), "format_error", "short magic code");
    let unknown = parse_extended_nodes(b"WTF? payload").expect_err("an unknown magic is refused");
    assert!(
        unknown.to_string().contains("WTF?"),
        "the magic is named: {unknown}"
    );
    let broken =
        parse_extended_nodes(b"ZNODnot-zlib").expect_err("a broken zlib stream is refused");
    assert!(
        broken.to_string().contains("inflate"),
        "the failure is named: {broken}"
    );
    let cut = payload.len().checked_sub(8).expect("the payload is longer");
    let mut truncated = b"XNOD".to_vec();
    truncated.extend_from_slice(payload.get(..cut).expect("computed above"));
    let cut_error = parse_extended_nodes(&truncated).expect_err("the last node is cut");
    assert_eq!(cut_error.code(), "format_error", "truncated payload code");
    let mut trailing = b"XNOD".to_vec();
    trailing.extend_from_slice(&payload);
    trailing.extend_from_slice(&[0_u8; 4]);
    let error = parse_extended_nodes(&trailing).expect_err("trailing junk");
    assert!(
        error.to_string().contains("trail"),
        "trailing bytes named: {error}"
    );
}

#[test]
fn extended_subsector_totals_must_match_the_segs() {
    let mut payload = extended_payload();
    payload[20] = 2;
    let mut bytes = b"XNOD".to_vec();
    bytes.extend_from_slice(&payload);
    let error =
        parse_extended_nodes(&bytes).expect_err("the subsector counts now total three, not two");
    assert!(
        error.to_string().contains("claim"),
        "the mismatch is named: {error}"
    );
}

#[test]
fn lump_format_names_every_known_magic() {
    assert_eq!(
        MAGIC_ROWS.len(),
        NodeMagic::ALL.len(),
        "the rows cover every magic the sniffer knows",
    );
    for (magic, letters, reported) in MAGIC_ROWS {
        assert_eq!(lump_format(letters), magic, "the bare {letters:?} magic");
        assert_eq!(magic.as_str(), reported, "{letters:?} reports {reported:?}");
        assert_eq!(
            magic.bytes(),
            Some(letters),
            "{reported} stores its own letters"
        );
        let stored = magic.stored_name().expect("a magic names four letters");
        assert_eq!(
            stored.as_bytes(),
            letters.as_slice(),
            "{reported} is reported as the stored {stored:?}",
        );
        let mut with_payload = letters.to_vec();
        with_payload.extend_from_slice(b"more bytes follow");
        assert_eq!(lump_format(&with_payload), magic, "{reported} plus payload");
    }
}

#[test]
fn only_the_two_zdoom_magics_parse_and_the_rest_are_gl_family() {
    for (magic, letters, _) in MAGIC_ROWS {
        let zdoom = letters == b"XNOD" || letters == b"ZNOD";
        assert_eq!(
            magic.is_extended(),
            zdoom,
            "{letters:?} parses as an extended run exactly when ZDoom wrote it",
        );
        assert_eq!(
            magic.is_gl(),
            !zdoom,
            "{letters:?} is a reported-only GL magic otherwise",
        );
    }
    assert!(
        !NodeMagic::Vanilla.is_extended() && !NodeMagic::Vanilla.is_gl(),
        "plain record data is neither",
    );
}

#[test]
fn record_data_and_short_heads_sniff_as_vanilla() {
    assert_eq!(lump_format(b""), NodeMagic::Vanilla, "no bytes, no magic");
    assert_eq!(
        lump_format(b"abc"),
        NodeMagic::Vanilla,
        "three bytes are no magic",
    );
    assert_eq!(
        lump_format(b"NODE"),
        NodeMagic::Vanilla,
        "vanilla record data stays unnamed",
    );
    assert_eq!(
        lump_format(b"xnod and a payload"),
        NodeMagic::Vanilla,
        "the magics are stored uppercase",
    );
    assert_eq!(
        lump_format(b"WTF? and a payload"),
        NodeMagic::Vanilla,
        "an unknown head is plain data to the sniffer",
    );
    assert_eq!(
        NodeMagic::Vanilla.as_str(),
        "vanilla",
        "the report name of plain records",
    );
    assert_eq!(NodeMagic::Vanilla.bytes(), None, "and no letters");
    assert_eq!(NodeMagic::Vanilla.stored_name(), None, "and no stored name");
}

#[test]
fn lump_head_reports_the_letters_a_lump_starts_with() {
    assert_eq!(
        lump_head(b"gNd2 rest").as_deref(),
        Some("gNd2"),
        "a glBSP magic keeps its case",
    );
    assert_eq!(
        lump_head(b"XGL3 rest").as_deref(),
        Some("XGL3"),
        "a node magic's letters read the same way",
    );
    assert_eq!(lump_head(b"..!"), None, "too short");
    assert_eq!(lump_head(b"\x01\x02\x03\x04"), None, "not letters");
    assert_eq!(lump_head(b"a_b3"), None, "an underscore is no letter");
}

#[test]
fn child_words_split_the_flag_from_the_index() {
    assert_eq!(vanilla_child(0x8000), Child::Subsector(0), "the flag alone");
    assert_eq!(vanilla_child(0x8123), Child::Subsector(0x123), "leaf index");
    assert_eq!(vanilla_child(12), Child::Node(12), "plain node index");
    assert_eq!(
        extended_child(0x8000_0000),
        Child::Subsector(0),
        "the high flag"
    );
    assert_eq!(
        extended_child(0x8001_0002),
        Child::Subsector(0x1_0002),
        "leaf index below bit 31"
    );
    assert_eq!(
        extended_child(7),
        Child::Node(7),
        "plain extended node index"
    );
}

#[test]
fn seg_vertex_indices_lists_every_endpoint_of_a_whole_run() {
    let (segs, _, _) = vanilla_lumps();
    let named = seg_vertex_indices(&segs, 8, 4).expect("the fixture is a whole run");
    assert_eq!(
        named,
        BTreeSet::from([0_usize, 1, 2]),
        "the two records span vertices 0, 1 and 2"
    );
}

#[test]
fn seg_vertex_indices_stays_quiet_when_the_run_cannot_answer() {
    let (segs, _, _) = vanilla_lumps();
    assert!(
        seg_vertex_indices(b"", 8, 4).is_none(),
        "an empty SEGS lump names no vertex"
    );
    assert!(
        seg_vertex_indices(b"12345", 8, 4).is_none(),
        "five bytes are no 12-byte record"
    );
    assert!(
        seg_vertex_indices(&segs, 2, 4).is_none(),
        "a run naming vertex 2 is from before the map held two"
    );
    assert!(
        seg_vertex_indices(&segs, 8, 1).is_none(),
        "a run naming linedef 1 is from before the map held one"
    );
}

#[test]
fn parts_serialize_for_json_assets() {
    let (segs, ssectors, nodes) = vanilla_lumps();
    let set = parse_vanilla_nodes(&segs, &ssectors, &nodes).expect("parses");
    let seg = serde_json::to_value(set.segs.get(1).expect("two segs")).expect("a seg serializes");
    assert_eq!(seg["v2"], serde_json::json!(2_u64), "unsigned fields land");
    assert_eq!(
        seg["angle"],
        serde_json::json!(-1_i64),
        "signed fields land"
    );
    let child = serde_json::to_value(Child::Subsector(3)).expect("a child serializes");
    assert_eq!(
        child,
        serde_json::json!({ "subsector": 3_u64 }),
        "tagged leaf"
    );
    let format = serde_json::to_value(set.format).expect("a format serializes");
    assert_eq!(format, serde_json::json!("vanilla"), "lowercase name");
}
