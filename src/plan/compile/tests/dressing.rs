//! The dressing: the narrow band inheriting the rooms at its surfaces, the
//! outdoor band keeping the sky it reaches, a room's own wall override, the
//! door's leaf, track and flats, the window's sill, lintel and blocking,
//! the too-tall window refusal, the stair risers on their low sides and a
//! theme that answers nothing.

use super::compile;
use super::{
    TestRoles, assert_door_line, assert_no_bare_flats, assert_no_gaps, assert_valid,
    assert_window_line, compiled, connection, faces, is_riser, issues_for, lower_row,
    lower_sides_with, map_name, opening, two_rooms, wall_faces,
};
use crate::plan::model::{ConnectionKind, DoorStyle};
use crate::wad::classify::MapFormat;
use crate::wad::map::Sidedef;
use crate::wad::map::validate::Severity;

#[test]
fn a_narrow_band_inherits_the_room_standing_at_each_of_its_surfaces() {
    let mut plan = two_rooms(16, vec![opening("a", "b")]);
    if let Some(room) = plan.rooms.get_mut(0) {
        room.floor_flat = Some("floor4_8".to_owned());
        room.ceiling_flat = Some("ceil5_1".to_owned());
    }
    if let Some(room) = plan.rooms.get_mut(1) {
        room.floor = 16_i32;
        room.floor_flat = Some("FLAT10".to_owned());
    }
    let out = compiled(&plan);
    let band = out.map.sectors.get(2).expect("the band sector");
    assert_eq!(band.floor_height, 16_i32, "the higher floor spans the band");
    assert_eq!(
        band.floor_flat, "FLAT10",
        "so room b's floor continues through it"
    );
    assert_eq!(
        band.ceiling_flat, "CEIL5_1",
        "and room a owns the ceiling both rooms stand at"
    );
    assert_eq!(band.light, 160_i32, "the dimmer of the two room lights");
    assert_no_bare_flats(&out.map);
    assert_no_gaps(&out.map);
    assert_valid(&out.map);
}

#[test]
fn a_band_between_two_outdoor_rooms_keeps_the_sky_it_reaches() {
    let mut plan = two_rooms(16, vec![opening("a", "b")]);
    for room in &mut plan.rooms {
        room.outdoor = true;
    }
    if let Some(room) = plan.rooms.get_mut(1) {
        room.ceiling = 256_i32;
    }
    let out = compiled(&plan);
    let band = out.map.sectors.get(2).expect("the band sector");
    assert_eq!(band.ceiling_height, 128_i32, "the lower ceiling spans it");
    assert_eq!(
        band.ceiling_flat, "F_SKY1",
        "and the room standing at that ceiling opens to the sky"
    );
    assert_no_bare_flats(&out.map);
    assert_valid(&out.map);
}

#[test]
fn a_room_wall_override_beats_the_theme() {
    let mut plan = two_rooms(16, vec![opening("a", "b")]);
    if let Some(room) = plan.rooms.get_mut(1) {
        room.wall_texture = Some("brick".to_owned());
    }
    let out = compiled(&plan);
    let overridden = out
        .map
        .sidedefs
        .iter()
        .filter(|row| row.middle == "BRICK")
        .count();
    assert!(overridden > 0, "room b's walls take its override");
    let themed = out
        .map
        .sidedefs
        .iter()
        .filter(|row| row.middle == "WALROLE")
        .count();
    assert!(themed > 0, "room a and the jambs still take the role");
    assert_valid(&out.map);
}

#[test]
fn a_door_dresses_its_leaf_track_and_flats() {
    let kind = ConnectionKind::Door {
        style: DoorStyle::Normal,
    };
    let plan = two_rooms(16, vec![connection("a", "b", kind)]);
    let out = compiled(&plan);
    let map = &out.map;
    let info = out.connector_sectors.first().expect("the door");
    assert_eq!(
        info.kind,
        ConnectionKind::Door {
            style: DoorStyle::Normal
        },
        "the door kind survives placement"
    );
    assert_eq!(info.sectors, vec![2], "one closed band sector");
    let band = map.sectors.get(2).expect("the band sector");
    assert_eq!(band.floor_height, band.ceiling_height, "the door is closed");
    assert_eq!(band.floor_flat, "DFOROLE", "the track floor");
    assert_eq!(band.ceiling_flat, "DFOROLE", "and above it");
    assert_eq!(
        info.door_lines.len(),
        2,
        "the band meets each wall face once"
    );
    for &index in &info.door_lines {
        assert_door_line(map, map.linedefs.get(index).expect("recorded index"));
    }
    let jambs: Vec<(&Sidedef, u32)> = wall_faces(map)
        .into_iter()
        .filter(|&(row, _)| row.sector == 2)
        .collect();
    assert_eq!(
        jambs.len(),
        4,
        "the band's four reveal edges inside the wall"
    );
    for (row, flags) in jambs {
        assert_eq!(row.middle, "DTRROLE", "the jamb shows the track");
        assert_ne!(flags & 16_u32, 0, "the track is lower-unpegged");
    }
    let plain_walls = wall_faces(map)
        .into_iter()
        .filter(|&(row, _)| row.sector != 2);
    for (row, flags) in plain_walls {
        assert_eq!(row.middle, "WALROLE", "room walls keep the plain role");
        assert_eq!(flags & 16_u32, 0, "plain walls carry no track bit");
    }
    assert_no_gaps(map);
    assert_valid(map);
}

#[test]
fn a_window_dresses_sill_and_lintel_and_blocks_the_way_through() {
    let kind = ConnectionKind::Window {
        sill: 32_i32,
        lintel: 24_i32,
    };
    let plan = two_rooms(16, vec![connection("a", "b", kind)]);
    let out = compiled(&plan);
    let map = &out.map;
    let info = out.connector_sectors.first().expect("the window");
    assert_eq!(info.sectors, vec![2]);
    let glass = map.sectors.get(2).expect("the glass box");
    assert_eq!(glass.floor_height, 32_i32, "the sill lifts the floor");
    assert_eq!(
        glass.ceiling_height, 104_i32,
        "the lintel drops the ceiling"
    );
    assert_eq!(
        info.door_lines.len(),
        2,
        "the band meets each wall face once"
    );
    for &index in &info.door_lines {
        assert_window_line(map, map.linedefs.get(index).expect("recorded index"));
    }
    assert_no_gaps(map);
    assert_valid(map);
}

#[test]
fn a_window_too_tall_for_the_gap_fails_the_compile() {
    let kind = ConnectionKind::Window {
        sill: 100_i32,
        lintel: 100_i32,
    };
    let plan = two_rooms(16, vec![connection("a", "b", kind)]);
    let error = compile(
        &plan,
        map_name("MAP01"),
        MapFormat::Doom,
        &TestRoles::full(),
    )
    .expect_err("there is no glass left");
    assert_eq!(error.code(), "invalid_argument", "the span is an argument");
    assert!(
        error.to_string().contains("glass"),
        "the message explains the squeeze: {error}"
    );
}

#[test]
fn stairs_dress_every_riser_on_its_low_side() {
    let kind = ConnectionKind::Stairs {
        step_height: 16_i32,
        step_depth: 24_i32,
    };
    let mut plan = two_rooms(16, vec![connection("a", "b", kind)]);
    if let Some(room) = plan.rooms.get_mut(1) {
        room.floor = 64_i32;
        room.ceiling = 160_i32;
    }
    let out = compiled(&plan);
    let map = &out.map;
    assert_eq!(map.sectors.len(), 6, "two rooms plus four steps");
    let info = out.connector_sectors.first().expect("the stair run");
    assert_eq!(info.sectors, vec![2, 3, 4, 5], "steps in plan order");
    let floors: Vec<i32> = (2_usize..6_usize)
        .map(|index| map.sectors.get(index).expect("a step sector").floor_height)
        .collect();
    assert_eq!(
        floors,
        vec![16_i32, 32_i32, 48_i32, 64_i32],
        "four rising steps"
    );
    let risered = faces(map)
        .into_iter()
        .filter(|&(_, _, one, two)| is_riser(one, two));
    let mut dressed = 0_usize;
    for (front, back, _, _) in risered {
        dressed = dressed.saturating_add(1);
        assert_eq!(
            lower_row(map, front, back).lower,
            "STEPROLE",
            "the riser faces the lower sector"
        );
    }
    assert_eq!(dressed, 6, "three room edges plus three step joins");
    assert_eq!(
        lower_sides_with(map, "STEPROLE"),
        6,
        "and the step texture dresses no other face"
    );
    assert_no_gaps(map);
    assert_valid(map);
}

#[test]
fn missing_roles_leave_the_dressed_map_bare_without_erroring() {
    let kind = ConnectionKind::Door {
        style: DoorStyle::Fast,
    };
    let plan = two_rooms(16, vec![connection("a", "b", kind)]);
    let out = compile(
        &plan,
        map_name("MAP01"),
        MapFormat::Doom,
        &TestRoles::missing(),
    )
    .expect("a theme that answers nothing is not an error");
    for row in &out.map.sidedefs {
        assert_eq!(
            (row.upper.as_str(), row.lower.as_str(), row.middle.as_str()),
            ("-", "-", "-"),
            "every face keeps the emission's placeholder"
        );
    }
    for space in &out.map.sectors {
        assert_eq!(
            (space.floor_flat.as_str(), space.ceiling_flat.as_str()),
            ("-", "-")
        );
    }
    let geometry: [&'static str; 5] = [
        "crossing_lines",
        "overlapping_lines",
        "two_sided_flag_mismatch",
        "zero_length_line",
        "duplicate_line",
    ];
    let sound = issues_for(&out.map, &geometry);
    assert!(
        sound.is_empty(),
        "an undressed map is still built sound: {sound:?}"
    );
    let bare = issues_for(&out.map, &["missing_flat"]);
    let planes = out.map.sectors.len().saturating_mul(2);
    assert_eq!(
        bare.len(),
        planes,
        "a theme that answers nothing leaves every plane undressed"
    );
    assert!(
        bare.iter()
            .all(|issue| issue.code == "missing_flat" && issue.severity == Severity::Error),
        "an undressed plane is an error, whatever the session carries: {bare:?}"
    );
}
