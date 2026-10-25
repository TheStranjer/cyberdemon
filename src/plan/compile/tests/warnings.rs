//! The advisory notes: a later room painting over an L-shaped one, rooms
//! that do not touch refusing to be connected, an unreachable room named
//! in the warning, a floor gap too tall for an opening and a fully covered
//! room earning its note.

use super::compile;
use super::{
    TestRoles, assert_no_gaps, assert_valid, compiled, map_name, opening, rect_room, two_rooms,
};
use crate::geom::label::{Label, Labeler as _, LayerLabeler, SAMPLE_SCALE};
use crate::plan::model::{Plan, Room, Shape};
use crate::plan::walls::room_layers;
use crate::wad::classify::MapFormat;

#[test]
fn a_later_room_paints_over_the_l_shaped_one() {
    let court = Room {
        id: "court".to_owned(),
        shape: Shape::Polygon {
            points: vec![(0, 0), (192, 0), (192, 64), (64, 64), (64, 192), (0, 192)],
        },
        floor: 0,
        ceiling: 128,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    };
    let lab = rect_room("lab", (96, 0), (96, 96));
    let mut passage = opening("court", "lab");
    passage.width = 32_i64;
    let plan = Plan {
        rooms: vec![court, lab],
        connections: vec![passage],
        features: Vec::new(),
        wall_thickness: 16_i64,
    };
    plan.validate().expect("the plan validates");
    let layers = room_layers(&plan);
    let labeler = LayerLabeler::new(&layers);
    let scale = |x: i64, y: i64| {
        (
            i128::from(x).saturating_mul(SAMPLE_SCALE),
            i128::from(y).saturating_mul(SAMPLE_SCALE),
        )
    };
    assert_eq!(
        labeler.label(scale(120, 40)),
        Label::Region(1),
        "the later rectangle wins where they overlap"
    );
    assert_eq!(
        labeler.label(scale(20, 120)),
        Label::Region(0),
        "the L survives uncovered"
    );
    let out = compiled(&plan);
    assert_eq!(out.map.sectors.len(), 3, "two rooms plus the band");
    assert!(
        out.room_sectors.contains_key("court") && out.room_sectors.contains_key("lab"),
        "both rooms own sectors: {:?}",
        out.room_sectors
    );
    assert_no_gaps(&out.map);
    assert_valid(&out.map);
}

#[test]
fn rooms_that_do_not_touch_cannot_be_connected() {
    let plan = Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (400, 0), (128, 128)),
        ],
        connections: vec![opening("a", "b")],
        features: Vec::new(),
        wall_thickness: 16_i64,
    };
    let error = compile(
        &plan,
        map_name("MAP01"),
        MapFormat::Doom,
        &TestRoles::full(),
    )
    .expect_err("nothing joins the two rooms");
    assert_eq!(error.code(), "invalid_argument", "placement is an argument");
    assert!(
        error.to_string().contains("do not touch"),
        "the message names the fault: {error}"
    );
}

#[test]
fn an_unreachable_room_is_warned_about_by_name() {
    let mut plan = two_rooms(16, vec![opening("a", "b")]);
    plan.rooms.push(rect_room("attic", (0, 200), (128, 128)));
    let out = compiled(&plan);
    assert_eq!(out.warnings.len(), 1, "only the orphan reports");
    assert!(
        out.warnings
            .first()
            .is_some_and(|note| note.contains("attic") && note.contains("'a'")),
        "the note names the room and the root: {:?}",
        out.warnings
    );
    assert_valid(&out.map);
}

#[test]
fn a_hundred_unit_floor_gap_through_an_opening_warns() {
    let mut plan = two_rooms(16, vec![opening("a", "b")]);
    if let Some(room) = plan.rooms.get_mut(1) {
        room.floor = 100_i32;
    }
    let out = compiled(&plan);
    assert_eq!(
        out.warnings,
        vec!["step taller than 24 between a and b; use stairs".to_owned()],
        "the exact height warning"
    );
    assert_no_gaps(&out.map);
    assert_valid(&out.map);
}

#[test]
fn a_fully_covered_room_earns_a_warning() {
    let plan = Plan {
        rooms: vec![
            rect_room("under", (0, 0), (64, 64)),
            rect_room("over", (0, 0), (128, 128)),
        ],
        connections: Vec::new(),
        features: Vec::new(),
        wall_thickness: 16_i64,
    };
    let out = compiled(&plan);
    assert_eq!(out.map.sectors.len(), 1, "only the covering room survives");
    assert_eq!(out.room_sectors.len(), 1, "one owner");
    assert!(
        out.room_sectors.contains_key("over"),
        "the buried room owns no sector"
    );
    assert!(
        out.warnings
            .iter()
            .any(|note| note.contains("under") && note.contains("painted over")),
        "the note names the buried room: {:?}",
        out.warnings
    );
    assert_valid(&out.map);
}
