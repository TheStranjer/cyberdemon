//! The animation and switch tables the content sweeps expand with: the
//! built-in `Doom` rows, the ranges and pairs a merged `ANIMATED` or
//! `SWITCHES` lump adds, the terminators and unreadable records those
//! lumps are allowed to carry, and what a range or a pair claims from the
//! name space it is resolved over.

use alloc::collections::BTreeSet;

use super::{
    animated_lump, animated_ranges, animated_record, lump, switch_pairs, switch_record,
    switches_lump,
};
use crate::wad::archive::Lump;
use crate::wad::classify::analyze;
use crate::wad::optimize::Context;
use crate::wad::optimize::animations::Animations;

/// No merged lumps: the built-in tables alone.
const BUILT_INS: Vec<Lump> = Vec::new();

/// The tables the merged directory `lumps` adds to the built-ins. The same
/// directory stands in for every snapshot: these tests read tables, not
/// overrides.
fn tables(lumps: &[Lump]) -> Animations {
    let analysis = analyze(lumps);
    let context = Context {
        working: lumps,
        working_info: &analysis.lumps,
        working_maps: &analysis.maps,
        base: &[],
        base_info: &[],
        base_maps: &[],
        merged: lumps,
        merged_info: &analysis.lumps,
        merged_maps: &analysis.maps,
    };
    Animations::read(&context)
}

/// What the tables `lumps` yields over `order` when `used` are the names
/// in use, in name order.
fn keeps(order: &[&str], used: &[&str], lumps: &[Lump]) -> Vec<String> {
    let space: Vec<String> = order.iter().map(|text| (*text).to_owned()).collect();
    let direct: BTreeSet<String> = used.iter().map(|text| (*text).to_owned()).collect();
    tables(lumps).expand(&space, &direct).into_iter().collect()
}

/// The kept names, spelled out for the asserts.
fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|text| (*text).to_owned()).collect()
}

#[test]
fn a_used_vanilla_flat_keeps_the_whole_range_it_sits_in() {
    let order = ["RROCK04", "RROCK05", "RROCK06", "RROCK07", "RROCK08"];
    assert_eq!(
        keeps(&order, &["RROCK07"], &BUILT_INS),
        names(&["RROCK05", "RROCK06", "RROCK07", "RROCK08"]),
        "the range's own run, and no neighbour outside it"
    );
}

#[test]
fn an_unwatched_range_keeps_nothing() {
    assert_eq!(
        keeps(&["NUKAGE1", "NUKAGE2", "NUKAGE3"], &["FLOOR4"], &BUILT_INS),
        names(&["FLOOR4"]),
        "an animation nobody uses adds no keeps"
    );
}

#[test]
fn a_switch_pair_keeps_both_states_and_nothing_between() {
    let order = ["SW1PANEL", "BRICK12", "SW2PANEL"];
    assert_eq!(
        keeps(&order, &["SW1PANEL"], &BUILT_INS),
        names(&["SW1PANEL", "SW2PANEL"]),
        "the used state and its partner, never the names in between"
    );
}

#[test]
fn a_declared_range_is_kept_whole_without_anything_using_it() {
    let lumps = [animated_ranges(&[("CUSTOM1", "CUSTOM3")])];
    let order = ["CUSTOM1", "CUSTOM2", "CUSTOM3", "SPARE"];
    assert_eq!(
        keeps(&order, &[], &lumps),
        names(&["CUSTOM1", "CUSTOM2", "CUSTOM3"]),
        "the lump declares the animation, so every frame of it stays"
    );
}

#[test]
fn a_declared_endpoint_stays_when_the_other_one_is_absent() {
    let lumps = [animated_ranges(&[("STANDAL", "GONE")])];
    assert_eq!(
        keeps(&["STANDAL", "SIBLING"], &[], &lumps),
        names(&["STANDAL"]),
        "a declared name is kept outright, and the range it cannot resolve adds nothing"
    );
}

#[test]
fn the_first_terminator_ends_the_animated_list() {
    let mut bytes = animated_record("CUSTOM1", "CUSTOM3", 0);
    bytes.push(0xFF);
    bytes.extend_from_slice(&animated_record("OTHER01", "OTHER03", 0));
    let lumps = [lump("ANIMATED", &bytes)];
    let order = ["CUSTOM1", "CUSTOM2", "CUSTOM3", "OTHER01", "OTHER03"];
    assert_eq!(
        keeps(&order, &[], &lumps),
        names(&["CUSTOM1", "CUSTOM2", "CUSTOM3"]),
        "the rows below the terminating record are never read"
    );
}

#[test]
fn a_kind_terminator_ends_the_animated_list() {
    let lumps = [animated_lump(&[animated_record(
        "CUSTOM1", "CUSTOM3", 0xFF,
    )])];
    assert!(
        keeps(&["CUSTOM1", "CUSTOM2", "CUSTOM3"], &[], &lumps).is_empty(),
        "the record that ends the list declares no range"
    );
}

#[test]
fn a_blank_animated_record_is_skipped_not_stopped_at() {
    let lumps = [animated_lump(&[
        animated_record("", "", 0),
        animated_record("CUSTOM1", "CUSTOM3", 0),
    ])];
    assert_eq!(
        keeps(&["CUSTOM1", "CUSTOM2", "CUSTOM3"], &[], &lumps),
        names(&["CUSTOM1", "CUSTOM2", "CUSTOM3"]),
        "the padding record costs nothing"
    );
}

#[test]
fn an_overlong_animated_name_is_not_a_name() {
    let lumps = [animated_lump(&[animated_record(
        "TOOLONGNAME",
        "CUSTOM3",
        0,
    )])];
    assert!(
        keeps(&["CUSTOM1", "CUSTOM2", "CUSTOM3"], &[], &lumps).is_empty(),
        "a nine-character name field makes the whole record unreadable"
    );
}

#[test]
fn a_switches_lump_adds_pairs() {
    let lumps = [switch_pairs(&[("MYOFFA", "MYONA")])];
    let order = ["MYOFFA", "MYOFFB", "MYONA"];
    assert_eq!(
        keeps(&order, &["MYONA"], &lumps),
        names(&["MYOFFA", "MYONA"]),
        "the lump's own pair, and no neighbour"
    );
}

#[test]
fn the_zero_episode_ends_the_switches_list() {
    let lumps = [switches_lump(&[
        switch_record(1, "MYOFFA", "MYONA"),
        switch_record(0, "", ""),
        switch_record(1, "MYOFFB", "MYONB"),
    ])];
    let order = ["MYOFFA", "MYONA", "MYOFFB", "MYONB"];
    assert_eq!(
        keeps(&order, &["MYOFFB"], &lumps),
        names(&["MYOFFB"]),
        "the lump's row below the zero episode never loaded: its partner is no keep"
    );
    assert_eq!(
        keeps(&order, &["MYONA"], &lumps),
        names(&["MYOFFA", "MYONA"]),
        "the row above it did"
    );
}

#[test]
fn a_switches_record_without_names_is_skipped_not_stopped_at() {
    let lumps = [switches_lump(&[
        switch_record(1, "", "MYONA"),
        switch_record(1, "MYOFFA", "MYONA"),
    ])];
    assert_eq!(
        keeps(&["MYOFFA", "MYONA"], &["MYOFFA"], &lumps),
        names(&["MYOFFA", "MYONA"]),
        "the half-empty row costs nothing"
    );
}

#[test]
fn a_declared_pair_needs_one_of_its_states_in_use() {
    let lumps = [switch_pairs(&[("MYOFFA", "MYONA")])];
    assert!(
        keeps(&["MYOFFA", "MYONA"], &[], &lumps).is_empty(),
        "a switch nobody uses keeps nothing: only ANIMATED declares an outright keep"
    );
}
