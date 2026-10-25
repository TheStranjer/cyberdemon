//! The door step's tests: every plan style's wiring in both flag
//! families, the shared tags and their connection order, the coloured
//! jambs, the painted-over and foreign-band refusals, the dressing
//! lookup and the door-core style map.

use super::super::{
    CompiledPlan, FeatureDress, FeatureRow, FeatureRun, apply_features, doors, wad_style,
};
use super::{
    BAND, FOUR_JAMBS, MONSTER, PLAYER_USE, REPEAT, TAG_ONE, TWO_DOOR_LINES, TestRoles, UseAction,
    assert_use_line, door, door_plan, dress, front_sector, line_at, map_name, two_rooms, wired,
    wired_dressed,
};
use crate::plan::compile::compile;
use crate::plan::model::{ConnectionKind, DoorStyle, KeyColor};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle as WadDoorStyle;
use crate::wad::map::model::Sidedef;

/// The band's one-sided jamb faces of `map`: their sidedef rows.
fn band_jambs(map: &Map) -> Vec<&Sidedef> {
    map.linedefs
        .iter()
        .filter(|line| line.back.is_none() && front_sector(map, line) == BAND)
        .map(|line| {
            let front = line.front.expect("one-sided lines have a front");
            map.sidedefs.get(front).expect("test rows")
        })
        .collect()
}

/// Assert one `Doom`-family door wiring: the row, the tagged band
/// and every door line facing its room with vanilla `special`.
#[track_caller]
fn assert_doom_door(style: DoorStyle, special: i32) {
    let (compiled, rows) = wired(&door_plan(style));
    let row = rows.first().expect("one door row");
    assert_eq!(row.kind, "door", "the class of {style:?}");
    assert_eq!(row.room, None, "a door belongs to the wall, not a room");
    assert_eq!(row.sectors, vec![BAND], "the band sector is reported");
    assert_eq!(row.tag, Some(TAG_ONE), "the shared tag of {style:?}");
    assert_eq!(
        row.lines.len(),
        TWO_DOOR_LINES,
        "the band meets each room once for {style:?}"
    );
    let band = compiled.map.sectors.get(BAND).expect("the band sector");
    assert_eq!(band.tag, TAG_ONE, "{style:?} stamps the shared tag");
    assert_eq!(
        band.special, 0_i32,
        "the Doom action lives on the lines, not the sector"
    );
    for &index in &row.lines {
        assert_use_line(&compiled.map, index, (special, TAG_ONE, [0_i32; 5]));
    }
}

#[test]
fn every_doom_door_style_wires_its_special_and_shared_tag() {
    let table = [
        (DoorStyle::Normal, 1_i32),
        (DoorStyle::Fast, 117_i32),
        (DoorStyle::StayOpen, 31_i32),
        (DoorStyle::Locked(KeyColor::Red), 28_i32),
        (DoorStyle::Locked(KeyColor::Blue), 26_i32),
        (DoorStyle::Locked(KeyColor::Yellow), 27_i32),
    ];
    for (style, special) in table {
        assert_doom_door(style, special);
    }
}

#[test]
fn a_remote_door_stamps_its_sector_and_writes_nothing_on_the_lines() {
    let (compiled, rows) = wired(&door_plan(DoorStyle::Remote));
    let row = rows.first().expect("one door row");
    assert_eq!(
        row.tag,
        Some(TAG_ONE),
        "the tagged sector is the remote's handle"
    );
    assert_eq!(row.lines.len(), TWO_DOOR_LINES, "the lines are reported");
    let band = compiled.map.sectors.get(BAND).expect("the band sector");
    assert_eq!(band.tag, TAG_ONE, "the sector carries the tag");
    for &index in &row.lines {
        let line = line_at(&compiled.map, index);
        assert_eq!(line.special, 0_i32, "a remote door keeps line {index} bare");
        assert_eq!(line.tag, 0_i32, "and untagged");
        assert_eq!(line.args, [0_i32; 5], "with no arguments");
    }
}

/// Assert one `Hexen`-family door wiring: every line carries the
/// script `action` and the style's activation `bits`.
#[track_caller]
fn assert_hexen_door(style: DoorStyle, action: UseAction, bits: u32) {
    let (compiled, rows) = wired_dressed(&door_plan(style), MapFormat::Hexen, &dress());
    let row = rows.first().expect("one door row");
    for &index in &row.lines {
        assert_use_line(&compiled.map, index, action);
        let flags = line_at(&compiled.map, index).flags;
        assert_eq!(flags & PLAYER_USE, PLAYER_USE, "{style:?} fires on use");
        assert_eq!(
            flags & 0x1C00_u32,
            PLAYER_USE,
            "the activation field holds playeruse alone for {style:?}"
        );
        assert_eq!(flags & bits, bits, "the {style:?} activation bits");
    }
}

#[test]
fn a_hexen_door_takes_the_script_action_and_playeruse_activation() {
    assert_hexen_door(
        DoorStyle::Normal,
        (12_i32, 0_i32, [0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
        PLAYER_USE | REPEAT | MONSTER,
    );
    assert_hexen_door(
        DoorStyle::Locked(KeyColor::Blue),
        (13_i32, 0_i32, [0_i32, 16_i32, 150_i32, 130_i32, 0_i32]),
        PLAYER_USE | REPEAT,
    );
}

/// Assert one row of the two-door fixture: its `tag`, its `band`
/// sector answering the same tag and its lines sharing it.
#[track_caller]
fn assert_tagged_row(compiled: &CompiledPlan, row: &FeatureRow, band: usize, tag: i32) {
    assert_eq!(row.sectors, vec![band], "the {band}th sector is the band");
    assert_eq!(row.tag, Some(tag), "connection order tags {tag}");
    let space = compiled.map.sectors.get(band).expect("the band");
    assert_eq!(space.tag, tag, "the band answers its door's tag");
    assert!(
        row.lines
            .iter()
            .all(|&line| line_at(&compiled.map, line).tag == tag),
        "the lines share tag {tag}"
    );
}

#[test]
fn two_doors_take_consecutive_tags_in_connection_order() {
    let plan = two_rooms(vec![
        door("first", DoorStyle::Normal, (128_i64, 32_i64)),
        door(
            "second",
            DoorStyle::Locked(KeyColor::Red),
            (128_i64, 96_i64),
        ),
    ]);
    let (compiled, rows) = wired(&plan);
    assert_eq!(rows.len(), 2, "both doors report a row");
    assert_tagged_row(&compiled, rows.first().expect("two rows"), BAND, 1_i32);
    assert_tagged_row(&compiled, rows.get(1).expect("two rows"), 3_usize, 2_i32);
}

#[test]
fn the_run_records_each_doors_tag_under_its_connector_id() {
    let plan = two_rooms(vec![
        door("first", DoorStyle::Remote, (128_i64, 32_i64)),
        door("second", DoorStyle::Normal, (128_i64, 96_i64)),
    ]);
    let mut compiled =
        compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
    let mut run = FeatureRun::default();
    doors(&mut compiled, &dress(), &mut run).expect("the doors wire");
    assert_eq!(
        run.tags.get("first"),
        Some(&1_i32),
        "the switch slice's lookup"
    );
    assert_eq!(run.tags.get("second"), Some(&2_i32));
    assert_eq!(run.rows.len(), 2, "both rows landed");
}

/// Assert a locked door's band jamb middles wear the coloured
/// `track` the dress names for its key.
#[track_caller]
fn assert_locked_jambs(style: DoorStyle, track: &str) {
    let (compiled, _) = wired(&door_plan(style));
    let jambs = band_jambs(&compiled.map);
    assert_eq!(jambs.len(), FOUR_JAMBS, "the band's four reveal edges");
    assert!(
        jambs.iter().all(|row| row.middle == track),
        "{style:?} jambs wear {track}: {:?}",
        jambs
            .iter()
            .map(|row| row.middle.as_str())
            .collect::<Vec<&str>>()
    );
}

#[test]
fn a_locked_doors_jambs_wear_their_coloured_track() {
    assert_locked_jambs(DoorStyle::Locked(KeyColor::Red), "REDTRAK");
    assert_locked_jambs(DoorStyle::Locked(KeyColor::Blue), "BLUETRAK");
    assert_locked_jambs(DoorStyle::Locked(KeyColor::Yellow), "YELTRAK");
}

#[test]
fn plain_doors_keep_the_compilers_track() {
    let plain = [
        DoorStyle::Normal,
        DoorStyle::Fast,
        DoorStyle::StayOpen,
        DoorStyle::Remote,
    ];
    for style in plain {
        let (compiled, _) = wired(&door_plan(style));
        let jambs = band_jambs(&compiled.map);
        assert_eq!(jambs.len(), FOUR_JAMBS, "{style:?} keeps its four jambs");
        assert!(
            jambs.iter().all(|row| row.middle == "DTRROLE"),
            "{style:?} keeps the plain track the compiler dressed"
        );
    }
}

#[test]
fn an_unnamed_colour_leaves_the_locked_jambs_dressed_plain() {
    let themed = FeatureDress {
        track_blue: "-".to_owned(),
        ..dress()
    };
    let (compiled, _) = wired_dressed(
        &door_plan(DoorStyle::Locked(KeyColor::Blue)),
        MapFormat::Doom,
        &themed,
    );
    let jambs = band_jambs(&compiled.map);
    assert!(
        jambs.iter().all(|row| row.middle == "DTRROLE"),
        "the \"-\" role keeps the compiler's track"
    );
}

#[test]
fn non_door_connectors_install_nothing() {
    let mut passage = door("d1", DoorStyle::Normal, (128_i64, 32_i64));
    passage.kind = ConnectionKind::Opening;
    let (compiled, rows) = wired(&two_rooms(vec![passage]));
    assert!(rows.is_empty(), "an opening is no feature");
    let info = compiled.connector_sectors.first().expect("the opening");
    assert!(
        info.door_lines
            .iter()
            .all(|&index| line_at(&compiled.map, index).special == 0_i32),
        "an opening's lines stay bare"
    );
}

#[test]
fn a_door_band_painted_over_earns_no_row_and_stays_untouched() {
    let plan = door_plan(DoorStyle::Normal);
    let mut compiled =
        compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
    if let Some(info) = compiled.connector_sectors.first_mut() {
        info.sectors.clear();
    }
    let before = compiled.map.clone();
    let rows = apply_features(&plan, &mut compiled, &dress()).expect("nothing to wire");
    assert!(rows.is_empty(), "no sector, no row");
    assert_eq!(compiled.map, before, "the map stays as it was");
}

#[test]
fn a_band_the_compiled_map_does_not_hold_refuses_the_run() {
    let plan = door_plan(DoorStyle::Fast);
    let mut compiled =
        compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
    if let Some(info) = compiled.connector_sectors.first_mut() {
        info.sectors = vec![99_usize];
    }
    let error =
        apply_features(&plan, &mut compiled, &dress()).expect_err("the band sector is foreign");
    assert_eq!(error.code(), "internal", "a contract break stays internal");
    let message = error.to_string();
    assert!(
        message.contains("d1") && message.contains("99"),
        "the connector and the sector are named: {message}"
    );
}

#[test]
fn a_reclassified_connector_reports_no_row() {
    let plan = door_plan(DoorStyle::Normal);
    let (mut compiled, _) = wired(&plan);
    if let Some(info) = compiled.connector_sectors.first_mut() {
        info.kind = ConnectionKind::Opening;
    }
    let rows =
        apply_features(&plan, &mut compiled, &dress()).expect("a non-door connector is skipped");
    assert!(rows.is_empty(), "no door kind, no door row");
}

#[test]
fn dress_resolves_tracks_by_the_doors_role_names() {
    let themed = dress();
    assert_eq!(themed.texture("door"), Some("DOOROLE"));
    assert_eq!(themed.texture("door_track"), Some("DTRROLE"));
    assert_eq!(themed.texture("door_track_red"), Some("REDTRAK"));
    assert_eq!(themed.texture("door_track_blue"), Some("BLUETRAK"));
    assert_eq!(themed.texture("door_track_yellow"), Some("YELTRAK"));
    assert_eq!(themed.texture("switch"), Some("SWTROLE"));
    assert_eq!(themed.texture("exit_switch"), Some("EXTPANE"));
    assert_eq!(themed.texture("step"), Some("STEPROLE"));
    assert_eq!(themed.texture("wall"), Some("WALROLE"));
    assert_eq!(
        themed.texture("teleport_floor"),
        None,
        "no such dressing field"
    );
    assert_eq!(themed.texture(""), None, "the empty name answers nothing");
    let bare = FeatureDress::default();
    assert_eq!(
        bare.texture("door_track"),
        None,
        "unset fields answer nothing"
    );
}

#[test]
fn plan_styles_map_onto_the_door_core() {
    let table = [
        (DoorStyle::Normal, WadDoorStyle::Normal),
        (DoorStyle::Fast, WadDoorStyle::Fast),
        (DoorStyle::StayOpen, WadDoorStyle::StayOpen),
        (DoorStyle::Locked(KeyColor::Red), WadDoorStyle::LockedRed),
        (DoorStyle::Locked(KeyColor::Blue), WadDoorStyle::LockedBlue),
        (
            DoorStyle::Locked(KeyColor::Yellow),
            WadDoorStyle::LockedYellow,
        ),
        (DoorStyle::Remote, WadDoorStyle::Remote),
    ];
    for (style, core) in table {
        assert_eq!(wad_style(style), core, "{style:?}");
    }
}

#[test]
fn the_wired_map_still_holds_the_reported_geometry() {
    let (compiled, rows) = wired(&door_plan(DoorStyle::Normal));
    let info = compiled.connector_sectors.first().expect("the door");
    assert_eq!(rows.first().expect("one row").lines, info.door_lines);
    assert!(
        info.door_lines
            .iter()
            .all(|&index| line_at(&compiled.map, index).flags & 4_u32 == 4_u32),
        "use lines stay two-sided"
    );
}
