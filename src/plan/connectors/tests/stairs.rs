//! Stairs and joins: the stacked step bands, the full-width openings
//! that drop their wall, the label order across a run and the helper
//! measurements behind all of them.

use super::{
    assert_band, compile, error_of, find_piece, first, notch_plan, opening, raise_b, stairs,
    two_rooms,
};
use crate::geom::Piece;
use crate::geom::arrange;
use crate::geom::label::{Label, LayerLabeler};
use crate::plan::connectors::final_layers;
use crate::plan::connectors::frame::{Frame, clamp_i32, clamp_i64, rounded_unit};
use crate::plan::connectors::pass::{distance_squared, midpoint};
use crate::plan::connectors::{LabelAllocator, connection_label, measure, plan_connectors};
use crate::plan::model::{ConnectionKind, DoorStyle};
use crate::plan::walls::{WallKey, build_walls};

#[test]
fn stairs_stack_steps_into_the_lower_room() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 64);
    plan.connections = vec![stairs("a", "b")];
    let connector = first(&plan);
    assert_eq!(connector.lower_room, Some(0), "room a is the lower one");
    assert_eq!(
        connector.step_floors,
        vec![16_i32, 32_i32, 48_i32, 64_i32],
        "floors increase from the lower room"
    );
    assert_eq!(
        connector.height, None,
        "stairs carry step floors, not a span"
    );
    assert_eq!(connector.layers.len(), 4, "one region per step");
    assert_eq!(connector.label_of(0), Some(2), "first step, first label");
    assert_eq!(connector.label_of(3), Some(5), "labels run consecutively");
    assert_eq!(connector.label_of(4), None, "past the stack is nothing");
    assert_band(
        &connector.layers.first().expect("step 0").polygon,
        (52, 64),
        (12, 32),
        "step 0 spans x 40..64: the run reaches 88 units into room a",
    );
    assert_band(
        &connector.layers.last().expect("step 3").polygon,
        (124, 64),
        (12, 32),
        "step 3 spans x 112..136: out of the 96-unit run, 80 units lie inside the lower room",
    );
}

#[test]
fn stair_step_floors_clamp_at_the_higher_floor() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 60);
    plan.connections = vec![stairs("a", "b")];
    let connector = first(&plan);
    assert_eq!(
        connector.step_floors,
        vec![16_i32, 32_i32, 48_i32, 60_i32],
        "the fourth step caps at room b's floor 60"
    );
}

#[test]
fn stairs_downhill_run_into_the_from_room_side() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 64);
    plan.connections = vec![stairs("b", "a")];
    let connector = first(&plan);
    assert_eq!(
        connector.lower_room,
        Some(0),
        "room a is still the lower one"
    );
    assert_eq!(connector.normal, (-1, 0), "b to a faces west");
    assert_eq!(
        connector.step_floors,
        vec![16_i32, 32_i32, 48_i32, 64_i32],
        "still up from a"
    );
    assert_band(
        &connector.layers.first().expect("step 0").polygon,
        (52, 64),
        (12, 32),
        "the run recedes into the lower room a, on the -normal side",
    );
}

#[test]
fn level_stairs_are_refused_and_a_steep_height_clamps() {
    let error = error_of(&two_rooms(16, vec![stairs("a", "b")]));
    assert!(
        error.to_string().contains("'a->b' has stairs")
            && error
                .to_string()
                .contains("floors are level; use an opening"),
        "level floors get the hint: {error}"
    );
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 64);
    let mut steep = stairs("a", "b");
    steep.kind = ConnectionKind::Stairs {
        step_height: 40_i32,
        step_depth: 24_i32,
    };
    plan.connections = vec![steep];
    let connector = first(&plan);
    assert_eq!(
        connector.step_floors,
        vec![24_i32, 48_i32, 64_i32],
        "step height clamps to 24"
    );
}

#[test]
fn an_absurd_step_count_is_refused() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 100_000_000);
    let mut endless = stairs("a", "b");
    endless.kind = ConnectionKind::Stairs {
        step_height: 0_i32,
        step_depth: 24_i32,
    };
    plan.connections = vec![endless];
    let error = error_of(&plan);
    assert_eq!(error.code(), "limit_exceeded", "code");
    assert!(
        error.to_string().contains("at most 4096"),
        "names the budget: {error}"
    );
}

#[test]
fn a_zero_thickness_opening_overlaps_both_rooms() {
    let plan = two_rooms(0, vec![opening("a", "b")]);
    let (walls, connectors) = compile(&plan);
    assert!(walls.layers.is_empty(), "no wall layers at thickness 0");
    let connector = connectors.first().expect("the opening");
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 64),
        (8, 32),
        "the depth-16 band straddles the shared line",
    );
    let stack = final_layers(&plan, &walls, &connectors);
    let pieces = arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("arranges");
    let west = find_piece(&pieces, (120, 32), (120, 96)).expect("cut into room a");
    assert_eq!(
        (west.left, west.right),
        (Label::Region(0), Label::Region(2)),
        "a's side"
    );
    let east = find_piece(&pieces, (136, 32), (136, 96)).expect("cut into room b");
    assert_eq!(
        (east.left, east.right),
        (Label::Region(2), Label::Region(1)),
        "b's side"
    );
}

#[test]
fn a_full_width_opening_paints_no_band_and_drops_its_wall() {
    let mut wide = opening("a", "b");
    wide.width = 128_i64;
    let plan = two_rooms(16, vec![wide]);
    plan.validate().expect("the plan validates");
    let (walls, connectors) = compile(&plan);
    let connector = connectors.first().expect("the opening");
    assert_eq!(connector.kind, ConnectionKind::Opening, "kind copied");
    assert!(
        connector.layers.is_empty(),
        "no band, so nothing owns a sector"
    );
    assert_eq!(connector.label_of(0), None, "no band, so no label taken");
    assert_eq!(
        connector.joined,
        Some(WallKey {
            pair: (0, 1),
            piece: ((128, 0), (128, 128)),
        }),
        "the key names the wall it opens end to end"
    );
    assert_eq!(
        connector.height,
        Some((0_i32, 128_i32)),
        "the span still rides along for the report"
    );
    assert_eq!(connector.band, (0, 1), "the run's direction reads back");
    assert_eq!(walls.layers.len(), 1, "the wall grew before the opening");
    let stack = final_layers(&plan, &walls, &connectors);
    assert_eq!(stack.len(), 2, "the rooms alone: wall and band both gone");
    let pieces = arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("arranges");
    let shared = find_piece(&pieces, (128, 0), (128, 128)).expect("the shared line");
    assert_eq!(
        (shared.left, shared.right),
        (Label::Region(0), Label::Region(1)),
        "the rooms meet on their own boundary"
    );
    assert!(
        find_piece(&pieces, (120, 0), (120, 128)).is_none(),
        "no wall face stands at x=120"
    );
    assert!(
        find_piece(&pieces, (136, 0), (136, 128)).is_none(),
        "and none at x=136"
    );
}

#[test]
fn a_full_width_opening_at_zero_thickness_joins_without_a_band() {
    let mut wide = opening("a", "b");
    wide.width = 128_i64;
    let (walls, connectors) = compile(&two_rooms(0, vec![wide]));
    assert!(walls.layers.is_empty(), "nothing to drop");
    let connector = connectors.first().expect("the opening");
    assert!(connector.joined.is_some(), "the run is open end to end");
    assert!(
        connector.layers.is_empty(),
        "and the band that used to sit there is gone"
    );
}

#[test]
fn a_full_width_opening_off_the_centre_keeps_its_band() {
    let mut wide = opening("a", "b");
    wide.width = 128_i64;
    wide.at = Some((128, 0));
    let connector = first(&two_rooms(16, vec![wide]));
    assert!(
        connector.joined.is_none(),
        "the band misses the far end of the run"
    );
    assert_eq!(connector.layers.len(), 1, "so it still paints a sector");
}

#[test]
fn a_door_over_the_whole_run_keeps_its_band() {
    let mut gate = opening("a", "b");
    gate.width = 128_i64;
    gate.kind = ConnectionKind::Door {
        style: DoorStyle::Normal,
    };
    let connector = first(&two_rooms(16, vec![gate]));
    assert!(connector.joined.is_none(), "a door needs its band sector");
    assert_eq!(
        connector.layers.len(),
        1,
        "the closed leaf still spans the wall"
    );
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 64),
        (8, 64),
        "the band covers the whole 128-unit run",
    );
}

#[test]
fn a_full_width_opening_drops_only_the_run_it_spans() {
    let mut wide = opening("a", "b");
    wide.width = 64_i64;
    let plan = notch_plan(vec![wide]);
    let (walls, connectors) = compile(&plan);
    let connector = connectors.first().expect("the opening");
    let joined = connector.joined.expect("the 64-unit run opens fully");
    assert_eq!(joined.pair, (0, 1), "the key names the room pair");
    let (from, to) = joined.piece;
    let length = (to.0.saturating_sub(from.0))
        .abs()
        .max((to.1.saturating_sub(from.1)).abs());
    assert_eq!(length, 64_i64, "the longer of the two runs");
    let stack = final_layers(&plan, &walls, &connectors);
    let voids: usize = stack
        .iter()
        .filter(|layer| layer.label == Label::Void)
        .count();
    assert_eq!(voids, 1, "the 32-unit run keeps its wall");
    assert_eq!(stack.len(), 3, "two rooms plus the one surviving wall");
}

#[test]
fn the_pass_is_deterministic_and_labels_follow_the_rooms() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 64);
    let mut view = opening("b", "a");
    view.id = Some("v".to_owned());
    view.kind = ConnectionKind::Window {
        sill: 20_i32,
        lintel: 8_i32,
    };
    view.at = Some((128, 16));
    view.width = 32;
    plan.connections = vec![stairs("a", "b"), view];
    let walls = build_walls(&plan).expect("walls build");
    let once = plan_connectors(&plan, &walls).expect("first pass");
    let twice = plan_connectors(&plan, &walls).expect("second pass");
    assert_eq!(once, twice, "same plan, identical layer stacks and labels");
    let climb = once.first().expect("stairs");
    let glass = once.get(1).expect("window");
    assert_eq!(climb.label_of(3), Some(5), "the stairs took labels 2..=5");
    assert_eq!(glass.label_of(0), Some(6), "the window follows the steps");
    assert_eq!(
        glass.height,
        Some((84_i32, 120_i32)),
        "max floor 64 + sill 20, min ceiling 128 - lintel 8"
    );
}

#[test]
fn helpers_round_measure_and_clamp() {
    assert_eq!(clamp_i64(i128::from(i64::MAX)), i64::MAX, "in range");
    assert_eq!(clamp_i32(i64::from(i32::MAX)), i32::MAX, "in range");
    assert_eq!(
        clamp_i32(-i64::from(i32::MAX)),
        i32::MIN.saturating_add(1),
        "negative"
    );
    assert_eq!(rounded_unit(0, 128, 128), (0, 1), "exact unit");
    assert_eq!(
        rounded_unit(64, 64, 90),
        (1, 1),
        "a diagonal rounds to ones"
    );
    assert_eq!(
        rounded_unit(3, 0, 128),
        (0, 0),
        "a tiny numerator rounds to zero"
    );
    let mut allocator = LabelAllocator::new(3_usize);
    assert_eq!(allocator.take(2_usize), 3, "first pair of labels");
    assert_eq!(allocator.take(1_usize), 5, "continues past them");
    assert_eq!(connection_label(&opening("a", "b")), "a->b", "unnamed");
    let piece = Piece {
        a: (128, 0),
        b: (128, 128),
        left: Label::Region(0),
        right: Label::Region(1),
        origin: Vec::new(),
    };
    let boundary = measure(&piece);
    assert_eq!(
        (boundary.length, boundary.length_squared),
        (128, 0x4000),
        "measured"
    );
    assert_eq!(distance_squared(&boundary, (128, 64)), 0, "on the line");
    assert_eq!(distance_squared(&boundary, (124, 64)), 16, "beside it");
    assert_eq!(
        distance_squared(&boundary, (128, 200)),
        5184,
        "past the end"
    );
    assert_eq!(midpoint(&boundary), (128, 64), "rounded midpoint");
    let flat = Frame {
        centre: (0, 0),
        dx: 0,
        dy: 0,
        length: 0,
        nx: 0,
        ny: 0,
    };
    assert_eq!(
        flat.band(64, -16, 16),
        vec![(0, 0); 4],
        "a degenerate frame collapses"
    );
}
