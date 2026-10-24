//! The records topic: what the `GL_NODES` lump holds and every refusal the
//! run answers with.
//!
//! [`node_records_keep_the_vanilla_layout_and_flags`] reads the 28-byte
//! record and its flagged child words; the other three walk the limits:
//! oversized vertex and child words, a tree pointing at a leaf it does not
//! hold, a map with nothing to bound and a seg no region edge can keep.

use alloc::collections::BTreeMap;

use super::{point, run_of, seg, square_room, two_rooms, vertex};
use crate::bsp::gl::regions::close_region;
use crate::bsp::gl::to_gl;
use crate::bsp::gl::write::{VertexTable, child_word};
use crate::bsp::{BBox, BspTree, BuiltNode, Child, Partition, Point};
use crate::wad::map::Map;

#[test]
fn oversized_records_and_broken_trees_refuse() {
    let mut huge = Map::default();
    for x in 0..0x8001_i64 {
        vertex(&mut huge, x, 0);
    }
    let tree = BspTree {
        segs: Vec::new(),
        subsectors: vec![(0, 0)],
        nodes: Vec::new(),
        root: Child::Subsector(0),
        warnings: Vec::new(),
    };
    let error = to_gl(&huge, &tree).unwrap_err();
    assert_eq!(
        error.code(),
        "limit_exceeded",
        "the vertex space is 15 bits"
    );
    assert!(
        error.to_string().contains("map vertices"),
        "the field: {error}"
    );

    let mut table = VertexTable {
        points: BTreeMap::new(),
        added: vec![(0_i32, 0_i32); 0x8000_usize],
    };
    let full = table.word(Point { x: 1, y: 1 }).unwrap_err();
    assert_eq!(full.code(), "limit_exceeded", "GL_VERT fills at 0x8000");

    let empty = Map::default();
    let naked = to_gl(&empty, &tree).unwrap_err();
    assert_eq!(naked.code(), "internal", "no vertices to bound");
    assert!(
        naked.to_string().contains("no vertices"),
        "the hint: {naked}"
    );

    let dangling = BspTree {
        segs: Vec::new(),
        subsectors: vec![(0, 0)],
        nodes: Vec::new(),
        root: Child::Subsector(7),
        warnings: Vec::new(),
    };
    let broken = to_gl(&square_room(), &dangling).unwrap_err();
    assert_eq!(broken.code(), "internal", "a leaf that is not there");
    assert!(broken.to_string().contains('7'), "the index: {broken}");
}

#[test]
fn a_seg_outside_its_region_is_reported_not_dropped() {
    // A real seg parallel to, but away from, every edge of the
    // region cannot bound it: the builder's convexity rule excludes
    // this, and the writer says so rather than losing the seg.
    let region = vec![point(0, 0), point(0, 256), point(256, 0)];
    let stray = seg(9, 1, (128, 128, 64, 128));
    let error = close_region(&region, core::slice::from_ref(&stray)).unwrap_err();
    assert_eq!(error.code(), "internal", "unplaceable seg");
    assert!(
        error.to_string().contains("split_cost"),
        "the message names the fix: {error}"
    );
}

#[test]
fn node_records_keep_the_vanilla_layout_and_flags() {
    let map = two_rooms();
    let run = run_of(&map);
    assert_eq!(run.nodes.len(), 28, "one 28-byte record");
    let fields: Vec<i16> = run
        .nodes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes(pair[..2].try_into().unwrap()))
        .collect();
    assert_eq!(fields.first().copied().unwrap(), 256_i16, "partition x");
    let children: Vec<u16> = run
        .nodes
        .chunks_exact(2)
        .skip(12)
        .map(|pair| u16::from_le_bytes(pair[..2].try_into().unwrap()))
        .collect();
    assert_eq!(
        children,
        vec![0x8000_u16, 0x8001_u16],
        "both children are leaves, flagged"
    );
    assert!(run.pvs.is_empty(), "the GL_PVS lump stays empty");
}

#[test]
fn an_oversized_child_word_refuses() {
    let tree = BspTree {
        segs: Vec::new(),
        subsectors: vec![(0, 0)],
        nodes: vec![BuiltNode {
            partition: Partition::default(),
            right_bbox: BBox::default(),
            left_bbox: BBox::default(),
            right: Child::Node(0x8000_usize),
            left: Child::Subsector(0),
        }],
        root: Child::Node(0),
        warnings: Vec::new(),
    };
    let error = child_word(Child::Node(0x8000_usize)).unwrap_err();
    assert_eq!(
        error.code(),
        "limit_exceeded",
        "children share the flag bit"
    );
    assert!(
        error.to_string().contains("node child"),
        "the field: {error}"
    );
    let reached = to_gl(&square_room(), &tree).unwrap_err();
    assert_eq!(
        reached.code(),
        "internal",
        "the walk stops at the missing node"
    );
}
