//! The wall-mounted rows' point tests: the `at` point of a `switch`
//! or `exit` row picks the wall and centres the 32-unit panel there,
//! the fit refusal names the knob that moves it, and a point past the
//! map range is refused by name.

use crate::error::PlanRef;
use crate::plan::compile::compile;
use crate::plan::features::switches::panel_hint;
use crate::plan::features::{FeatureLineEnds, apply_features};
use crate::plan::model::Side;
use crate::wad::classify::MapFormat;

use super::super::{TAG_ONE, TestRoles, dress, map_name, refuse_features, wired};
use super::{action_of_line, c_room, ends, exit_at, stacked_doors, switch_at};

/// The row's point mounts the panel where the drawing put it and the
/// row reports the chosen line's endpoints beside its index.
#[test]
fn an_exit_at_a_point_mounts_there_and_reports_its_coordinates() {
    let mut plan = c_room();
    plan.features = vec![exit_at("c", Some(Side::West), Some((0_i64, 24_i64)))];
    let (compiled, rows) = wired(&plan);
    let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    let line = row.lines.first().copied().expect("one line");
    assert_eq!(
        ends(&compiled.map, line),
        ((0_i32, 8_i32), (0_i32, 40_i32)),
        "the 32-unit panel centred on the point, on the lower west wall"
    );
    assert_eq!(
        row.line_ends,
        vec![FeatureLineEnds {
            index: line,
            x1: 0_i32,
            y1: 8_i32,
            x2: 0_i32,
            y2: 40_i32,
        }],
        "the reported endpoints beside the index"
    );
    assert_eq!(
        action_of_line(&compiled.map, line).0,
        11_i32,
        "the panel ends the level"
    );
}

/// The same row with its point by the upper wall: the other run takes
/// the panel and the reported coordinates say so.
#[test]
fn the_same_exit_row_follows_its_point_to_the_other_wall() {
    let mut plan = c_room();
    plan.features = vec![exit_at("c", Some(Side::West), Some((4_i64, 100_i64)))];
    let (compiled, rows) = wired(&plan);
    let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
    let end = row.line_ends.first().expect("one reported line");
    assert_eq!(
        (end.y1.min(end.y2), end.y1.max(end.y2)),
        (84_i32, 116_i32),
        "the panel stands on the upper run: {end:?}"
    );
    assert_eq!(end.x1, end.x2, "still the vertical west wall");
    assert_eq!(
        ends(&compiled.map, end.index),
        ((0_i32, 84_i32), (0_i32, 116_i32)),
        "the index names the line the coordinates describe"
    );
}

/// A `switch` row reads its point the same way: the tied north segments
/// of the stacked fixture each answer the point nearest them.
#[test]
fn a_switch_at_a_point_mounts_on_the_nearest_of_the_tied_walls() {
    for (x, want) in [(20_i64, (4_i32, 36_i32)), (108_i64, (92_i32, 124_i32))] {
        let mut plan = stacked_doors();
        plan.features = vec![switch_at("a", "d1", Some(Side::North), (x, 120_i64))];
        let (compiled, rows) = wired(&plan);
        let row = rows
            .iter()
            .find(|r| r.kind == "switch")
            .expect("a switch row");
        let end = row.line_ends.first().expect("one reported line");
        assert_eq!(
            (end.x1.min(end.x2), end.x1.max(end.x2)),
            want,
            "the panel stands at the point's own segment"
        );
        assert_eq!(end.y1, end.y2, "a horizontal north wall");
        assert_eq!(
            action_of_line(&compiled.map, end.index),
            (103_i32, TAG_ONE),
            "the switch still fires the door's tag"
        );
    }
}

/// A point too near the wall's own end for the 32-unit panel refuses
/// the row, keeping the core's words and naming the knob that moves it.
#[test]
fn a_panel_that_does_not_fit_where_its_point_sits_names_the_knob() {
    let mut plan = c_room();
    plan.features = vec![exit_at("c", Some(Side::West), Some((0_i64, 4_i64)))];
    let mut compiled = compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles)
        .expect("the fixture compiles");
    let error = apply_features(&plan, &mut compiled, &dress()).expect_err("the panel cannot fit");
    assert_eq!(error.code(), "invalid_argument", "a fit refusal");
    let message = error.to_string();
    assert!(
        message.contains("room 'c'") && message.contains("does not fit linedef"),
        "the room is named beside the core's words: {message}"
    );
    let hint = error.hint();
    assert!(
        hint.contains("move the row's at nearer the middle of the wall"),
        "the hint names the knob: {hint}"
    );
}

/// Only an argument refusal is the row's doing: anything else keeps
/// its own code, message and default hint.
#[test]
fn the_panel_hint_rides_on_argument_refusals_alone() {
    let argued = panel_hint(
        crate::error::CyberError::InvalidArgument("the wall is too short".to_owned()),
        "cell",
    );
    assert_eq!(argued.code(), "invalid_argument", "the code stands");
    assert_eq!(
        argued.to_string(),
        "room 'cell': the wall is too short",
        "the room leads"
    );
    assert!(
        argued.hint().starts_with("move the row's at"),
        "the hint answers: {}",
        argued.hint()
    );
    assert_eq!(
        argued.plan_ref().as_ref().map(PlanRef::id),
        Some("cell"),
        "the refusal carries its room row"
    );
    let broken = panel_hint(
        crate::error::CyberError::Internal("a contract break".to_owned()),
        "cell",
    );
    assert_eq!(
        broken.code(),
        "internal",
        "an internal error is not the row's"
    );
    assert_eq!(
        broken.to_string(),
        "a contract break",
        "and reads unchanged"
    );
    assert_eq!(
        broken.plan_ref(),
        None,
        "a contract break is attributed to nothing"
    );
}

/// A row point the map cannot hold refuses, naming it.
#[test]
fn an_exit_point_past_the_map_range_is_refused_by_name() {
    let mut plan = c_room();
    plan.features = vec![exit_at(
        "c",
        Some(Side::West),
        Some((0x0001_0000_0000_i64, 24_i64)),
    )];
    let message = refuse_features(&plan).to_string();
    assert!(
        message.contains("the exit at point 4294967296, 24 is outside the map coordinate range"),
        "the refusal names the point: {message}"
    );
}
