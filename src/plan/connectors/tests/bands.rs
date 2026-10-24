//! Band placement: the one rectangle an opening, door or window cuts,
//! the piece the connector was chosen on and the refusals around them.

use super::{
    assert_band, compile, error_of, find_piece, first, notch_plan, opening, raise_b, rect_room,
    two_rooms,
};
use crate::geom::arrange;
use crate::geom::label::{Label, LayerLabeler};
use crate::plan::connectors::final_layers;
use crate::plan::model::{ConnectionKind, DoorStyle, KeyColor, Plan};

#[test]
fn an_opening_cuts_one_centred_band_and_gaps_the_wall() {
    let plan = two_rooms(16, vec![opening("a", "b")]);
    plan.validate().expect("the plan validates");
    let (walls, connectors) = compile(&plan);
    assert_eq!(connectors.len(), 1, "one connection, one geometry");
    let connector = connectors.first().expect("the opening");
    assert_eq!(connector.connection_index, 0, "plan order");
    assert_eq!(connector.id, "a->b", "unnamed shows from->to");
    assert_eq!((connector.from, connector.to), (0, 1), "resolved indices");
    assert_eq!(connector.kind, ConnectionKind::Opening, "kind copied");
    assert_eq!(connector.band, (0, 1), "unit direction along the boundary");
    assert_eq!(connector.normal, (1, 0), "unit normal from a towards b");
    assert_eq!(connector.lower_room, None, "not stairs");
    assert!(connector.step_floors.is_empty(), "not stairs");
    assert_eq!(connector.height, Some((0_i32, 128_i32)), "opening span");
    assert_eq!(
        connector.label_of(0),
        Some(2),
        "labels start past the rooms"
    );
    assert_eq!(connector.label_of(1), None, "a single band has one label");
    assert_eq!(connector.layers.len(), 1, "exactly one connector layer");
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 64),
        (8, 32),
        "64 along y by 16 across x, centred on the boundary",
    );
    let stack = final_layers(&plan, &walls, &connectors);
    assert_eq!(stack.len(), 4, "two rooms, one wall, one band");
    assert_eq!(
        stack.last().expect("the connector paints topmost").label,
        Label::Region(2),
        "connectors sit above the wall voids"
    );
    let pieces = arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("the stack arranges");
    let west = find_piece(&pieces, (120, 32), (120, 96)).expect("west wall face in the gap");
    assert_eq!(west.left, Label::Region(0), "room a faces the opening");
    assert_eq!(
        west.right,
        Label::Region(2),
        "the connector label shows on both sides"
    );
    let east = find_piece(&pieces, (136, 32), (136, 96)).expect("east wall face in the gap");
    assert_eq!(
        east.left,
        Label::Region(2),
        "the connector label shows on both sides"
    );
    assert_eq!(east.right, Label::Region(1), "room b faces the opening");
    assert!(
        find_piece(&pieces, (128, 32), (128, 96)).is_none(),
        "the wall line has a gap where the opening is"
    );
}

#[test]
fn a_door_gets_the_same_band_as_an_opening() {
    let mut door = opening("a", "b");
    door.kind = ConnectionKind::Door {
        style: DoorStyle::Locked(KeyColor::Red),
    };
    let connector = first(&two_rooms(16, vec![door]));
    assert_eq!(
        connector.layers.len(),
        1,
        "geometry identical to an opening"
    );
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 64),
        (8, 32),
        "the door band is the opening band",
    );
    assert_eq!(
        connector.height,
        Some((0_i32, 128_i32)),
        "the span rides along"
    );
    assert!(matches!(
        connector.kind,
        ConnectionKind::Door {
            style: DoorStyle::Locked(KeyColor::Red)
        }
    ));
}

#[test]
fn at_selects_the_nearer_of_two_shared_pieces() {
    let mut near = opening("a", "b");
    near.at = Some((128, 16));
    near.width = 24;
    let connector = first(&notch_plan(vec![near]));
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 16),
        (8, 12),
        "the lower, nearer run wins",
    );
}

#[test]
fn at_selects_the_farther_piece_from_the_other_end() {
    let mut far = opening("a", "b");
    far.at = Some((128, 110));
    far.width = 24;
    let connector = first(&notch_plan(vec![far]));
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 110),
        (8, 12),
        "the upper run wins for a point near it",
    );
}

#[test]
fn without_at_the_longest_shared_piece_and_its_midpoint_win() {
    let mut narrow = opening("a", "b");
    narrow.width = 32_i64;
    let connector = first(&notch_plan(vec![narrow]));
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 96),
        (8, 16),
        "the 64-unit run, centred on its rounded midpoint",
    );
}

#[test]
fn non_touching_rooms_are_refused_with_a_hint() {
    let plan = Plan {
        rooms: vec![
            rect_room("a", (0, 0), (64, 64)),
            rect_room("b", (200, 0), (64, 64)),
        ],
        connections: vec![opening("a", "b")],
        features: Vec::new(),
        wall_thickness: 16,
    };
    let error = error_of(&plan);
    assert_eq!(error.code(), "invalid_argument", "code");
    assert!(
        error
            .to_string()
            .contains("rooms a and b do not touch; move them together or add a corridor room"),
        "message carries the hint: {error}"
    );
}

#[test]
fn a_connection_naming_an_unknown_room_is_refused() {
    let error = error_of(&two_rooms(16, vec![opening("a", "ghost")]));
    assert!(
        error.to_string().contains("unknown room 'ghost'"),
        "names the missing room: {error}"
    );
}

#[test]
fn a_wider_than_the_boundary_connection_is_refused() {
    let mut wide = opening("a", "b");
    wide.id = Some("gate".to_owned());
    wide.width = 200;
    let error = error_of(&two_rooms(16, vec![wide]));
    assert_eq!(error.code(), "invalid_argument", "code");
    let message = error.to_string();
    assert!(
        message.contains("'gate'"),
        "names the connection: {message}"
    );
    assert!(message.contains("200"), "names the width: {message}");
    assert!(
        message.contains("128"),
        "names the boundary length: {message}"
    );
}

#[test]
fn a_window_stores_its_glass_span() {
    let mut plan = two_rooms(16, Vec::new());
    raise_b(&mut plan, 32);
    if let Some(room) = plan.rooms.get_mut(1) {
        room.ceiling = 200_i32;
    }
    let mut view = opening("a", "b");
    view.id = Some("w1".to_owned());
    view.kind = ConnectionKind::Window {
        sill: 32_i32,
        lintel: 24_i32,
    };
    plan.connections = vec![view];
    let connector = first(&plan);
    assert_eq!(
        connector.height,
        Some((64_i32, 104_i32)),
        "max floor 32 + sill 32, min ceiling 128 - lintel 24"
    );
    assert_eq!(connector.layers.len(), 1, "one region rectangle");
    assert_band(
        &connector.layers.first().expect("band").polygon,
        (128, 64),
        (8, 32),
        "the window band is placed like an opening",
    );
}

#[test]
fn a_window_thin_under_the_sill_is_refused() {
    let mut low = opening("a", "b");
    low.id = Some("slit".to_owned());
    low.kind = ConnectionKind::Window {
        sill: 120_i32,
        lintel: 24_i32,
    };
    let error = error_of(&two_rooms(16, vec![low]));
    assert_eq!(error.code(), "invalid_argument", "code");
    let message = error.to_string();
    assert!(
        message.contains("'slit'"),
        "names the connection: {message}"
    );
    assert!(message.contains("120"), "names the floor: {message}");
    assert!(message.contains("104"), "names the ceiling: {message}");
}
