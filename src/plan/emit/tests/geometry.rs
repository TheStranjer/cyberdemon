//! Emission geometry: whole plans build to sectors, vertices and lines
//! that match the arrangement's faces — clean sidedness, the flip rule,
//! shared corner vertices, door and stair bookkeeping, flat conventions,
//! the coordinate-range refusal and the zero-thickness join.

use super::{
    assert_line_orientation, assert_rows_are_plain, assert_sidedness_matches_faces,
    assert_sides_are_placeholders, compile, door, door_pair, emit_error, faces_sector,
    geometry_issues, line_flags, lines_between, map_name, on_map_border, opening, rect_room,
    rooms_plan, two_rooms, vertical_at,
};
use crate::geom::Label;
use crate::plan::emit::compiled_plan;
use crate::plan::model::{ConnectionKind, DoorStyle};
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::{Linedef, MapFormat};

#[test]
fn an_opening_walled_between_two_rooms_emits_three_clean_sectors() {
    let plan = two_rooms(16, vec![opening("a", "b")]);
    plan.validate().expect("the plan validates");
    let (emitted, _) = compile(&plan);
    let map = &emitted.map;
    assert_eq!(map.name, map_name("MAP01"), "the map carries its identity");
    assert_eq!(map.format, MapFormat::Doom, "in the requested layout");
    assert!(map.things.is_empty(), "the emitter draws no things");
    assert_eq!(map.sectors.len(), 3, "two rooms plus the connector band");
    assert_eq!(
        emitted.sector_of_label.get(&Label::Region(0)),
        Some(&0),
        "room a, first in plan order"
    );
    assert_eq!(
        emitted.sector_of_label.get(&Label::Region(1)),
        Some(&1),
        "room b, second"
    );
    assert_eq!(
        emitted.sector_of_label.get(&Label::Region(2)),
        Some(&2),
        "the connector follows the rooms in allocator order"
    );
    let band = map.sectors.get(2).expect("the band sector");
    assert_eq!(band.floor_height, 0_i32, "the higher floor");
    assert_eq!(band.ceiling_height, 128_i32, "the lower ceiling");
    assert_eq!(band.light, 160_i32, "the dimmer light");
    assert_sides_are_placeholders(map);
    assert_sidedness_matches_faces(&emitted);
    assert!(geometry_issues(map).is_empty(), "the emission is clean");
}

#[test]
fn every_line_fronts_its_pieces_region_and_keeps_the_flip() {
    let plan = two_rooms(16, vec![opening("a", "b")]);
    let (emitted, _) = compile(&plan);
    for (piece, line) in emitted.pieces.iter().zip(&emitted.map.linedefs) {
        assert_line_orientation(&emitted, piece, line);
    }
    assert_eq!(
        emitted.map.linedefs.len(),
        emitted.pieces.len(),
        "one line per piece"
    );
}

#[test]
fn rooms_touching_at_a_corner_share_one_vertex_row() {
    let plan = rooms_plan(
        vec![
            rect_room("a", (0, 0), (64, 64)),
            rect_room("b", (64, 64), (64, 64)),
        ],
        0,
    );
    let (emitted, _) = compile(&plan);
    let map = &emitted.map;
    assert_eq!(map.sectors.len(), 2, "one sector per room");
    assert_eq!(
        map.vertices.len(),
        7,
        "the corner both rooms share is emitted once"
    );
    let shared = map
        .vertices
        .iter()
        .filter(|vertex| vertex.x == 64_i32 && vertex.y == 64_i32)
        .count();
    assert_eq!(shared, 1, "exactly one vertex row at (64, 64)");
    assert_eq!(map.linedefs.len(), 8, "four walls per room, none shared");
    assert!(
        map.linedefs
            .iter()
            .all(|line| line.back.is_none() && line.flags == BLOCKING),
        "point-touching rooms share no line and every wall blocks"
    );
    assert!(geometry_issues(map).is_empty(), "the emission is clean");
}

#[test]
fn a_door_closes_onto_its_floor_and_books_two_door_lines() {
    let plan = two_rooms(16, vec![door("a", "b", DoorStyle::Fast)]);
    let (emitted, connectors) = compile(&plan);
    let compiled = compiled_plan(&plan, &connectors, emitted);
    assert!(
        compiled.warnings.is_empty(),
        "warnings stay empty until the reporting slice"
    );
    assert_eq!(
        compiled.room_sectors.get("a"),
        Some(&vec![0_usize]),
        "one sector per room, as a list for later slices"
    );
    assert_eq!(
        compiled.room_sectors.get("b"),
        Some(&vec![1_usize]),
        "and for the other room too"
    );
    let info = compiled.connector_sectors.first().expect("one placed door");
    assert_eq!(info.id, "d1", "the connection's own id");
    assert_eq!(
        info.kind,
        ConnectionKind::Door {
            style: DoorStyle::Fast
        },
        "the kind rides along"
    );
    assert_eq!(info.sectors, vec![2], "the band's sector");
    let band = compiled.map.sectors.get(2).expect("the band sector row");
    assert_eq!(band.floor_height, 0_i32, "the higher floor");
    assert_eq!(
        band.ceiling_height, band.floor_height,
        "a closed door starts flat with its floor"
    );
    assert_eq!(
        info.door_lines.len(),
        2,
        "the band meets each wall face once"
    );
    let mut joins: Vec<(usize, usize)> = info
        .door_lines
        .iter()
        .map(|&index| door_pair(&compiled.map, index))
        .collect();
    joins.sort_unstable();
    assert_eq!(
        joins,
        vec![(0, 2), (1, 2)],
        "each door line joins the band to one room"
    );
}

#[test]
fn a_joined_opening_books_no_sector_and_no_door_line() {
    let mut arch = opening("a", "b");
    arch.id = Some("arch".to_owned());
    arch.width = 128_i64;
    let plan = two_rooms(16, vec![arch]);
    let (emitted, connectors) = compile(&plan);
    assert_eq!(emitted.map.sectors.len(), 2, "the rooms alone own the map");
    assert_eq!(
        emitted
            .sector_of_label
            .values()
            .copied()
            .collect::<Vec<usize>>(),
        vec![0_usize, 1],
        "no connector label is allocated"
    );
    let compiled = compiled_plan(&plan, &connectors, emitted);
    let info = compiled.connector_sectors.first().expect("the arch");
    assert_eq!(info.id, "arch", "the connection still reports");
    assert_eq!(info.kind, ConnectionKind::Opening, "as the opening it is");
    assert!(info.sectors.is_empty(), "and it owns no sector");
    assert!(
        info.door_lines.is_empty(),
        "nor a line to hang a special on"
    );
    assert!(
        geometry_issues(&compiled.map).is_empty(),
        "the join is clean"
    );
}

#[test]
fn a_doors_void_facing_reveals_are_one_sided() {
    let plan = two_rooms(16, vec![door("a", "b", DoorStyle::Fast)]);
    let (emitted, _) = compile(&plan);
    let reveals: Vec<&Linedef> = emitted
        .map
        .linedefs
        .iter()
        .filter(|line| faces_sector(&emitted.map, line, 2))
        .collect();
    assert_eq!(
        reveals.len(),
        4,
        "the band's four reveal edges face the wall void, split where the rooms' own line ran"
    );
    assert!(
        reveals.iter().all(|line| line.flags & BLOCKING != 0),
        "the void-facing door reveals block"
    );
}

#[test]
fn stairs_emit_one_riser_facing_sector_per_step() {
    let mut plan = two_rooms(16, Vec::new());
    if let Some(room) = plan.rooms.get_mut(0) {
        room.light = 96_i32;
    }
    if let Some(room) = plan.rooms.get_mut(1) {
        room.floor = 64_i32;
        room.ceiling = 160_i32;
        room.light = 192_i32;
    }
    let mut climb = opening("a", "b");
    climb.kind = ConnectionKind::Stairs {
        step_height: 16_i32,
        step_depth: 24_i32,
    };
    plan.connections = vec![climb];
    let (emitted, connectors) = compile(&plan);
    assert_eq!(emitted.map.sectors.len(), 6, "two rooms plus four steps");
    let mut floors = Vec::new();
    for id in [2_u32, 3_u32, 4_u32, 5_u32] {
        let index = *emitted
            .sector_of_label
            .get(&Label::Region(id))
            .expect("every step keeps its label");
        assert_eq!(
            index,
            usize::try_from(id).expect("small ids"),
            "the steps take the sectors right after the rooms"
        );
        let step = emitted.map.sectors.get(index).expect("the step sector");
        floors.push(step.floor_height);
        assert_eq!(
            step.ceiling_height, 160_i32,
            "risers face up under the higher room's ceiling"
        );
        assert_eq!(step.light, 96_i32, "the dimmer of the two room lights");
    }
    assert_eq!(
        floors,
        vec![16_i32, 32_i32, 48_i32, 64_i32],
        "four steps, strictly increasing, lower room first"
    );
    let compiled = compiled_plan(&plan, &connectors, emitted);
    let info = compiled.connector_sectors.first().expect("the stair run");
    assert_eq!(info.sectors, vec![2, 3, 4, 5], "steps in allocator order");
    assert!(
        geometry_issues(&compiled.map).is_empty(),
        "the stair map is clean"
    );
}

#[test]
fn outdoor_rooms_fall_back_to_sky_and_flat_overrides_win() {
    let mut court = rect_room("court", (0, 0), (128, 128));
    court.outdoor = true;
    court.floor_flat = Some("FLOOR4_8".to_owned());
    let mut lab = rect_room("lab", (160, 0), (128, 128));
    lab.ceiling_flat = Some("CEIL5_1".to_owned());
    lab.floor_flat = Some("ceiling1".to_owned());
    let plan = rooms_plan(vec![court, lab], 16);
    let (emitted, _) = compile(&plan);
    assert_eq!(
        emitted.map.sectors.len(),
        2,
        "no connector, no band sectors"
    );
    let sky = emitted.map.sectors.first().expect("the courtyard");
    assert_eq!(sky.ceiling_flat, "F_SKY1", "an outdoor room sees the sky");
    assert_eq!(sky.floor_flat, "FLOOR4_8", "the room's own flat is kept");
    let lab_sector = emitted.map.sectors.get(1).expect("the lab");
    assert_eq!(
        lab_sector.ceiling_flat, "CEIL5_1",
        "an explicit ceiling wins"
    );
    assert_eq!(
        lab_sector.floor_flat, "CEILING1",
        "override names follow the model's uppercase convention"
    );
    assert_sides_are_placeholders(&emitted.map);
    assert_rows_are_plain(&emitted.map);
    assert!(
        geometry_issues(&emitted.map).is_empty(),
        "the emission is clean"
    );
}

#[test]
fn vertices_beyond_the_i32_range_are_refused() {
    let far = rooms_plan(vec![rect_room("field", (0, 0), (3_000_000_000, 64))], 16);
    let high = emit_error(&far);
    assert_eq!(
        high.code(),
        "limit_exceeded",
        "an unmappable point is a limit"
    );
    let message = high.to_string();
    assert!(
        message.contains("3000000000"),
        "the message names the offending x: {message}"
    );
    let deep = rooms_plan(vec![rect_room("pit", (-3_000_000_000, 0), (128, 64))], 16);
    let low = emit_error(&deep);
    assert_eq!(low.code(), "limit_exceeded", "and below the range too");
    assert!(
        low.to_string().contains("-3000000000"),
        "the message names the point: {low}"
    );
}

#[test]
fn zero_thickness_rooms_share_two_sided_lines_through_the_band() {
    let plan = two_rooms(0, vec![opening("a", "b")]);
    let (emitted, _) = compile(&plan);
    let map = &emitted.map;
    assert_eq!(map.sectors.len(), 3, "the band still gets its own sector");
    let shared = lines_between(map, 0, 1);
    assert_eq!(
        shared.len(),
        2,
        "the rooms meet directly on x=128 below and above the band"
    );
    assert!(
        shared
            .iter()
            .all(|&index| line_flags(map, index) == TWO_SIDED),
        "the shared room lines are two-sided"
    );
    let west = lines_between(map, 0, 2);
    let east = lines_between(map, 2, 1);
    assert_eq!(
        west.iter()
            .filter(|&&index| vertical_at(map, index, 120_i32))
            .count(),
        1,
        "the depth-16 band cuts room a's side at x=120"
    );
    assert_eq!(
        east.iter()
            .filter(|&&index| vertical_at(map, index, 136_i32))
            .count(),
        1,
        "and room b's side at x=136"
    );
    for line in map.linedefs.iter().filter(|line| line.back.is_none()) {
        assert!(
            on_map_border(map, line),
            "the only void walls sit on the map's outer border: {line:?}"
        );
    }
    assert!(geometry_issues(map).is_empty(), "the emission is clean");
}
