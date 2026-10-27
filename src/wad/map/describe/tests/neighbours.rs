//! The neighbour rows: how [`Describe`] reads a room hall's shared
//! boundaries — an equal floor as an `opening`, a rise as `step_up` and
//! the trip back as `step_down`, a blocking divider as `impassable`, a
//! blocking one with light above the sill as `window`, several shared
//! lines grouped into one row, and a closed band between two rooms
//! answering `door` from both sides.

use alloc::vec;

use serde_json::{json, to_value};

use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::describe::{Describe, Neighbour};
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};

use super::{cell, described, doom_defs, hall, neighbour_across, slab, split_divider_map};

#[test]
fn equal_floors_meet_across_an_opening() {
    let map = hall(&[cell(0_i32), cell(0_i32)]);
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let from_a = describe.sector(0).expect("sector 0 exists");
    assert_eq!(
        from_a.neighbours,
        vec![Neighbour {
            sector: 1,
            via: "opening".to_owned(),
            lines: vec![0_usize],
            floor_delta: 0_i32,
        }],
        "the level gap between the rooms reads as an opening"
    );
    assert!(from_a.roles.is_empty(), "a full-height room is nothing");
    let from_b = describe.sector(1).expect("sector 1 exists");
    assert_eq!(
        to_value(&from_b.neighbours).expect("neighbours serialise"),
        json!([{
            "sector": 0_u64,
            "via": "opening",
            "lines": [0_u64],
            "floor_delta": 0_i64,
        }]),
        "and the row reads back in the ticket's names"
    );
}

#[test]
fn a_step_up_below_a_ledge_reads_as_a_step_down_above_it() {
    let map = hall(&[cell(0_i32), cell(16_i32)]);
    let low = described(&map, 0);
    assert_eq!(
        neighbour_across(&low, 1).via,
        "step_up",
        "the neighbour stands above"
    );
    assert_eq!(neighbour_across(&low, 1).floor_delta, 16_i32, "16 units");
    let high = described(&map, 1);
    assert_eq!(
        neighbour_across(&high, 0).via,
        "step_down",
        "and back down from the ledge"
    );
    assert_eq!(neighbour_across(&high, 0).floor_delta, -16_i32, "-16 units");
}

#[test]
fn a_blocking_divider_without_light_is_impassable_from_either_side() {
    let mut map = hall(&[cell(0_i32), (128_i32, 256_i32)]);
    map.linedefs.get_mut(0).expect("the divider").flags = TWO_SIDED | BLOCKING;
    let low = described(&map, 0);
    let from_below = neighbour_across(&low, 1);
    assert_eq!(from_below.via, "impassable", "a sealed divider is no way");
    assert_eq!(
        from_below.floor_delta, 128_i32,
        "and the tall step never gets its say"
    );
    let high = described(&map, 1);
    assert_eq!(
        neighbour_across(&high, 0).via,
        "impassable",
        "the same seal from the other side"
    );
}

#[test]
fn a_blocking_raised_sill_with_light_is_a_window() {
    let mut map = hall(&[cell(0_i32), (16_i32, 256_i32)]);
    map.linedefs.get_mut(0).expect("the divider").flags = TWO_SIDED | BLOCKING;
    let low = described(&map, 0);
    let through = neighbour_across(&low, 1);
    assert_eq!(through.via, "window", "112 units over the sill");
    assert_eq!(
        through.floor_delta, 16_i32,
        "and the delta still rides along"
    );
    let high = described(&map, 1);
    assert_eq!(
        neighbour_across(&high, 0).via,
        "window",
        "the glass reads the same from the raised side"
    );
}

#[test]
fn one_blocking_line_among_several_seals_the_whole_boundary() {
    let sealed = split_divider_map(TWO_SIDED, TWO_SIDED | BLOCKING);
    let facts = described(&sealed, 0);
    let across = neighbour_across(&facts, 1);
    assert_eq!(across.via, "impassable", "a partly sealed boundary seals");
    assert_eq!(
        across.lines,
        vec![6_usize, 7],
        "both shared lines ride along"
    );
    let open = split_divider_map(TWO_SIDED, TWO_SIDED);
    let wide = described(&open, 0);
    assert_eq!(
        neighbour_across(&wide, 1).via,
        "opening",
        "and unblocked it is a wide gap"
    );
    assert_eq!(
        neighbour_across(&wide, 1).lines,
        vec![6_usize, 7],
        "grouped into one neighbour in linedef order"
    );
}

#[test]
fn a_closed_band_between_two_rooms_reads_as_a_door() {
    let map = hall(&[cell(0_i32), slab(0_i32), cell(0_i32)]);
    let defs = doom_defs();
    let describe = Describe::new(&map, &defs, SKY_FLAT);
    let band = describe.sector(1).expect("the band exists");
    assert_eq!(band.roles, vec!["door".to_owned()], "no special needed");
    assert!(
        band.neighbours.iter().all(|entry| entry.via == "opening"),
        "the rooms beside it are plain floors: {:?}",
        band.neighbours
    );
    for room_index in [0_usize, 2] {
        let room_facts = describe.sector(room_index).expect("a room exists");
        let across = neighbour_across(&room_facts, 1);
        assert_eq!(
            across.via, "door",
            "the band beside room {room_index} is a door"
        );
        assert_eq!(across.floor_delta, 0_i32, "level with it");
    }
}
