//! The closure topic: an overlay whose ring grazes an existing diagonal
//! wall can lose the grazed piece, and the notes must then name the open
//! sector, its free ends' vertices, coordinates and linedefs - and tell
//! the caller what to do. A clean draw stays silent, and a ring the map
//! was handed broken in is never blamed on the paint.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::wad::map::common::fixtures::{name, one_sided, room, side, solid_line, vertex};
use crate::wad::map::overlay::apply_overlay;
use crate::wad::map::{Map, MapFormat};

use super::{empty_map, rect, room_request, void_request};

/// A hall `(0,0)-(448,320)` with its top-right corner cut by the diagonal
/// `(448,96)-(320,320)` (direction `(-4,7)`), every wall facing sector 0.
fn sloped_hall() -> Map {
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.sectors = vec![room(0, 128, 160, 0)];
    let mut sidedefs = Vec::new();
    let points = [
        (0_i32, 0_i32),
        (448_i32, 0_i32),
        (448_i32, 96_i32),
        (320_i32, 320_i32),
        (0_i32, 320_i32),
    ];
    map.vertices = points.iter().map(|&(x, y)| vertex(x, y)).collect();
    for (i, next) in (1..points.len()).chain(core::iter::once(0)).enumerate() {
        map.linedefs.push(one_sided(i, next, &mut sidedefs, 0));
    }
    map.sidedefs = sidedefs;
    map
}

/// The note about an opened ring, or the empty string when there is none.
fn open_note(notes: &[String]) -> String {
    notes
        .iter()
        .find(|note| note.contains("not closed"))
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_loop_overlapping_a_diagonal_wall_notes_the_open_ring_with_coordinates() {
    let mut map = sloped_hall();
    let result =
        apply_overlay(&mut map, &room_request(&rect(96, 32, 352, 272))).expect("the counter draws");
    let note = open_note(&result.notes);
    assert!(
        note.contains("sector 0"),
        "the note names the open sector: {note}"
    );
    assert!(
        note.contains("x 320, y 320"),
        "the note gives the corner where the ring breaks: {note}"
    );
    assert!(
        note.contains("x 347, y 272"),
        "and the other free end: {note}"
    );
    assert!(
        note.contains("linedef"),
        "the note names the open linedefs: {note}"
    );
    assert!(
        note.contains("overlapped"),
        "the note hints at the overlapping linedef: {note}"
    );
    assert!(
        note.contains("split_linedef") && note.contains("render_topdown"),
        "the note says what to do next: {note}"
    );
}

#[test]
fn an_overlap_that_opens_the_painted_sector_says_so() {
    let mut map = sloped_hall();
    let quad = vec![(444_i64, 103), (288, 376), (274, 368), (430, 95)];
    let result = apply_overlay(&mut map, &room_request(&quad)).expect("the strip draws");
    assert!(
        result
            .notes
            .iter()
            .any(|note| note.contains("sector 1 (the sector this call paints)")),
        "the painted sector's own gap is marked as its own: {:?}",
        result.notes
    );
    assert!(
        result.notes.iter().any(|note| note.contains("sector 0")),
        "the hall's gap is noted too: {:?}",
        result.notes
    );
}

#[test]
fn a_void_carve_over_a_diagonal_wall_notes_the_ring_it_opens() {
    let mut map = sloped_hall();
    let result =
        apply_overlay(&mut map, &void_request(&rect(96, 32, 352, 272))).expect("the rock draws");
    for note in &result.notes {
        assert!(
            !note.contains("(the sector this call paints)"),
            "a carve paints no sector: {note}"
        );
    }
    let note = open_note(&result.notes);
    assert!(
        !note.is_empty(),
        "the opened hall ring is noted: {:?}",
        result.notes
    );
}

#[test]
fn a_clean_drawing_over_the_diagonal_leaves_no_opened_ring_note() {
    let mut map = sloped_hall();
    let result = apply_overlay(&mut map, &room_request(&rect(64, 64, 200, 200)))
        .expect("the inner room draws clear of the diagonal");
    assert!(
        result.notes.iter().all(|note| !note.contains("not closed")),
        "the snug room closes every ring: {:?}",
        result.notes
    );
}

#[test]
fn a_ring_the_map_was_handed_broken_in_is_not_blamed_on_the_paint() {
    let mut map = empty_map();
    map.sectors = vec![room(0, 128, 160, 0)];
    map.vertices = vec![
        vertex(0, 0),
        vertex(128, 0),
        vertex(128, 128),
        vertex(0, 128),
    ];
    map.sidedefs = vec![
        side("-", "-", "W1", 0),
        side("-", "-", "W1", 0),
        side("-", "-", "W1", 0),
    ];
    map.linedefs = vec![
        solid_line(0, 1, 0),
        solid_line(1, 2, 1),
        solid_line(2, 3, 2),
    ];
    let result = apply_overlay(&mut map, &room_request(&rect(400, 400, 500, 500)))
        .expect("the floating room draws");
    assert_eq!(result.sector, Some(1), "a second sector");
    assert!(
        result.notes.iter().all(|note| !note.contains("not closed")),
        "the pre-existing hole is not this paint's doing: {:?}",
        result.notes
    );
}
