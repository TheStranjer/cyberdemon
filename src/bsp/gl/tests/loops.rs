//! The loop topic: what one built tree turns into closed `GL_SEGS` loops.
//!
//! [`a_single_convex_room_is_its_own_closed_loop`] and
//! [`two_rooms_need_no_minisegs_and_partner_their_shared_segs`] measure the
//! wall-only runs, [`the_l_room_grows_paired_minisegs_across_the_doorway`]
//! the miniseg pair across an uncovered partition, and the last two check
//! [`close_region`] and [`push_gap`] directly: the filler over a partially
//! covered edge and the `1/16`-unit snap that swallows a hairline gap.

use super::{
    assert_loops_clockwise, assert_loops_close, assert_partner_symmetry, endpoint, rows, run_of,
    subsector_ranges,
};
use crate::bsp::Point;
use crate::bsp::fixtures::{
    UNIT, l_room, point, square_room, tree_of, two_rooms, whole_seg as seg,
};
use crate::bsp::gl::regions::{GlSeg, close_region, push_gap};
use crate::bsp::gl::{MAGIC_GND2, MINISEG_LINEDEF, NO_PARTNER, to_gl};

/// The no-linedef flag of a parsed miniseg row.
const MINI: u16 = MINISEG_LINEDEF;

#[test]
fn a_single_convex_room_is_its_own_closed_loop() {
    let map = square_room();
    let run = run_of(&map);
    assert_eq!(run.seg_count, 4, "the ring, nothing more");
    assert_eq!(run.minisegs, 0, "every edge is a wall");
    assert_eq!(run.new_vertices, 0, "all corners are map vertices");
    assert_eq!(run.vertices, MAGIC_GND2.to_vec(), "an empty vertex lump");
    assert!(run.nodes.is_empty(), "no partition, no GL node");
    assert_eq!(subsector_ranges(&run), vec![(0, 4)], "one closed leaf");
    assert!(
        rows(&run).iter().all(|row| row.linedef != MINI),
        "no miniseg words"
    );
    assert_loops_close(&run);
    assert_loops_clockwise(&map, &run);
}

#[test]
fn two_rooms_need_no_minisegs_and_partner_their_shared_segs() {
    let map = two_rooms();
    let run = run_of(&map);
    assert_eq!(run.seg_count, 8, "four walls per room, no filler");
    assert_eq!(run.minisegs, 0, "the shared line is covered on both sides");
    assert_eq!(run.new_vertices, 0, "flush corners");
    assert_eq!(run.nodes.len(), 28, "one GL node");
    assert_eq!(
        subsector_ranges(&run),
        vec![(0, 4), (4, 4)],
        "two closed leaves"
    );
    assert_loops_close(&run);
    assert_loops_clockwise(&map, &run);
    let rows = rows(&run);
    assert_partner_symmetry(&rows);
    let shared: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|pair| pair.1.linedef == 3)
        .map(|pair| pair.0)
        .collect();
    assert_eq!(shared.len(), 2, "front and back segs of the shared line");
    assert_eq!(
        rows[shared[0]].partner,
        u16::try_from(shared[1]).unwrap(),
        "front sees back"
    );
    assert_eq!(
        rows[shared[1]].partner,
        u16::try_from(shared[0]).unwrap(),
        "and back sees front"
    );
    let sides: Vec<u16> = shared.iter().map(|&i| rows[i].side).collect();
    assert!(
        sides.contains(&0) && sides.contains(&1),
        "the paired shared segs carry front and back sides: {sides:?}"
    );
    assert!(
        rows.iter()
            .filter(|row| row.linedef != 3)
            .all(|row| row.partner == NO_PARTNER),
        "outer walls face the void: no partner"
    );
}

#[test]
fn the_l_room_grows_paired_minisegs_across_the_doorway() {
    let map = l_room();
    let tree = tree_of(&map);
    let run = to_gl(&map, &tree).expect("the L fixture serializes");
    assert_eq!(tree.segs.len(), 7, "the classic run splits the bottom wall");
    assert_eq!(
        run.seg_count, 9,
        "seven real segs plus the two doorway minisegs"
    );
    assert_eq!(run.minisegs, 2, "one per side of the uncovered partition");
    assert_eq!(run.new_vertices, 1, "the doorway corner (128, 0) is new");
    let covered: usize = subsector_ranges(&run).iter().map(|entry| entry.1).sum();
    assert_eq!(covered, 9, "the leaves hold the whole pool");
    assert_loops_close(&run);
    assert_loops_clockwise(&map, &run);
    let rows = rows(&run);
    assert_partner_symmetry(&rows);
    let minis: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|pair| pair.1.linedef == MINI)
        .map(|pair| pair.0)
        .collect();
    assert_eq!(minis.len(), 2, "two minisegs");
    assert_eq!(
        rows[minis[0]].partner,
        u16::try_from(minis[1]).unwrap(),
        "they partner each other"
    );
    let mut corners = vec![
        endpoint(&map, &run, rows[minis[0]].v1),
        endpoint(&map, &run, rows[minis[0]].v2),
    ];
    corners.sort_unstable();
    assert_eq!(
        corners,
        vec![
            (128_i64.checked_mul(UNIT).unwrap(), 0_i64),
            (
                128_i64.checked_mul(UNIT).unwrap(),
                128_i64.checked_mul(UNIT).unwrap()
            ),
        ],
        "the miniseg spans the uncovered doorway stretch"
    );
}

#[test]
fn a_partial_edge_gets_one_filler_miniseg() {
    // Triangle (0,0),(0,256),(256,0) clockwise; one real seg covers
    // half of the bottom edge, so a miniseg fills the other half.
    let region = vec![point(0, 0), point(0, 256), point(256, 0)];
    let loop_segs = close_region(&region, &[seg(0, 0, (256, 0, 128, 0))]).unwrap();
    assert_eq!(loop_segs.len(), 4, "three edges plus the seg split");
    assert_eq!(
        loop_segs.iter().filter(|s| s.linedef.is_none()).count(),
        3,
        "two whole edges and the bottom's other half"
    );
    let ends: Vec<Point> = loop_segs.iter().map(|item| item.end).collect();
    let starts: Vec<Point> = loop_segs
        .iter()
        .cycle()
        .skip(1)
        .take(loop_segs.len())
        .map(|item| item.start)
        .collect();
    assert_eq!(ends, starts, "the loop closes end to start");
}

#[test]
fn hairline_gaps_snap_and_wide_gaps_emit_minisegs() {
    let mut cursor = point(0, 0);
    let mut segs: Vec<GlSeg> = Vec::new();
    let hair = Point { x: 4095, y: 0 };
    push_gap(&mut segs, &mut cursor, hair);
    assert!(
        segs.is_empty(),
        "a hairline gap at the loop head snaps away"
    );
    assert_eq!(cursor, hair, "and the cursor moves to the seg end");
    let far = point(128, 0);
    push_gap(&mut segs, &mut cursor, far);
    assert_eq!(segs.len(), 1, "half a unit of stretch earns a miniseg");
    let just_far = Point {
        x: far.x.checked_add(4097).unwrap(),
        y: 0,
    };
    push_gap(&mut segs, &mut cursor, just_far);
    assert_eq!(segs.len(), 2, "one raw unit past the snap is long again");
    let hair_again = Point {
        x: just_far.x.checked_add(4095).unwrap(),
        y: 0,
    };
    push_gap(&mut segs, &mut cursor, hair_again);
    assert_eq!(segs.len(), 2, "a hairline gap joins the last miniseg");
    assert_eq!(
        segs.last().map(|tail| tail.end),
        Some(hair_again),
        "its end moves"
    );
    segs.push(GlSeg {
        start: hair_again,
        end: point(64, 0),
        linedef: Some(1),
        side: 0,
    });
    cursor = point(64, 0);
    let near = Point {
        x: 64_i64.checked_mul(UNIT).unwrap().checked_sub(1).unwrap(),
        y: 0,
    };
    push_gap(&mut segs, &mut cursor, near);
    assert_eq!(
        segs.len(),
        4,
        "behind a real seg even a hairline gap must close the loop"
    );
    assert!(
        segs.last().is_some_and(|tail| tail.linedef.is_none()),
        "a filler miniseg"
    );
}
