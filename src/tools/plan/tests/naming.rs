//! The names: the connection classes and the dressing roles the build
//! resolves, the element references a plan path is read from, the labels a
//! feature row names and the bounds object the result carries.

use alloc::collections::BTreeMap;

use crate::error::{CyberError, PlanRef};
use crate::plan::features::FeatureDress;
use crate::plan::model::ConnectionKind;
use crate::plan::textures::{TextureRole, TextureRoles as _};
use crate::themes::Role;
use crate::tools::plan::PlanTheme;
use crate::tools::plan::build::{bounds_of, plan_path, with_plan_path};
use crate::tools::plan::{FEATURE_ROLES, THEME_ROLES, connection_label, feature_dress, kind_name};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::name::LumpName;

use super::{feature_plan, plan_args};

#[test]
fn connector_kinds_carry_their_wire_names() {
    use crate::plan::model::DoorStyle;
    assert_eq!(kind_name(ConnectionKind::Opening), "opening");
    assert_eq!(
        kind_name(ConnectionKind::Door {
            style: DoorStyle::Remote
        }),
        "door",
        "the style does not change the class name"
    );
    assert_eq!(
        kind_name(ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32
        }),
        "window"
    );
    assert_eq!(
        kind_name(ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32
        }),
        "stairs"
    );
}

#[test]
fn every_dressing_role_maps_to_a_theme_role() {
    let mapped: Vec<TextureRole> = THEME_ROLES.iter().map(|&(role, _)| role).collect();
    for role in [
        TextureRole::Floor,
        TextureRole::Ceiling,
        TextureRole::Wall,
        TextureRole::Door,
        TextureRole::DoorFloor,
        TextureRole::DoorTrack,
        TextureRole::WindowSill,
        TextureRole::Step,
    ] {
        assert_eq!(
            mapped.iter().filter(|slot| **slot == role).count(),
            1,
            "{role:?} appears exactly once"
        );
    }
    assert_eq!(mapped.len(), 8, "eight plan roles, eight answers");
}

#[test]
fn a_theme_that_answers_dash_leaves_the_role_undressed() {
    let theme = PlanTheme {
        names: vec![(TextureRole::Wall, "WALL1".to_owned())],
    };
    assert_eq!(theme.texture(TextureRole::Wall), Some("WALL1"));
    assert_eq!(
        theme.texture(TextureRole::Step),
        None,
        "absent means undressed"
    );
    let empty = PlanTheme { names: Vec::new() };
    assert_eq!(empty.texture(TextureRole::Floor), None);
}

#[test]
fn a_fully_named_feature_dress_carries_every_colour_and_notes_nothing() {
    let resolved: BTreeMap<Role, String> = [
        (Role::Door, "DOOR7"),
        (Role::DoorTrack, "DOORTRAK"),
        (Role::DoorTrackRed, "DOORRED"),
        (Role::DoorTrackBlue, "DOORBLU"),
        (Role::DoorTrackYellow, "DOORYEL"),
        (Role::Switch, "SW1STRTN"),
        (Role::ExitSwitch, "SW1EXIT"),
        (Role::Step, "STEP1"),
        (Role::Wall, "STARTAN3"),
    ]
    .into_iter()
    .map(|(role, name)| (role, name.to_owned()))
    .collect();
    let mut notes = Vec::new();
    let dress = feature_dress(&resolved, &mut notes);
    assert_eq!(
        dress,
        FeatureDress {
            door: "DOOR7".to_owned(),
            track: "DOORTRAK".to_owned(),
            track_red: "DOORRED".to_owned(),
            track_blue: "DOORBLU".to_owned(),
            track_yellow: "DOORYEL".to_owned(),
            switch: "SW1STRTN".to_owned(),
            exit_switch: "SW1EXIT".to_owned(),
            step: "STEP1".to_owned(),
            wall: "STARTAN3".to_owned(),
        },
        "every name carries through"
    );
    assert!(notes.is_empty(), "a named colour says nothing: {notes:?}");
}

#[test]
fn unnamed_feature_roles_note_themselves_like_dressing_roles() {
    let mut resolved = BTreeMap::new();
    resolved.insert(Role::DoorTrack, "-".to_owned());
    let mut notes = Vec::new();
    let dress = feature_dress(&resolved, &mut notes);
    assert_eq!(dress.door, "-", "an absent face is dashed like the rest");
    assert_eq!(dress.track_blue, "-", "and so is an absent colour");
    assert_eq!(dress.switch, "-", "and an absent switch panel");
    assert_eq!(
        dress.step, "-",
        "an absent step rides on silently for the lift"
    );
    assert_eq!(dress.wall, "-", "and an absent wall for the teleporter");
    assert_eq!(
        notes,
        FEATURE_ROLES
            .iter()
            .map(|role| format!(
                "the theme answers \"-\" for role {}: those faces stay undressed",
                role.as_str()
            ))
            .collect::<Vec<String>>(),
        "all five feature roles note themselves, in table order"
    );
}

#[test]
fn a_room_ref_names_its_wire_row_and_an_unknown_one_nothing() {
    let plan = plan_args();
    assert_eq!(
        plan_path(&plan, &PlanRef::room("b")).as_deref(),
        Some("rooms[1]"),
        "the plan's own id finds the row"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::room("ghost")),
        None,
        "an unknown name finds no path"
    );
}

#[test]
fn a_connection_ref_names_the_row_by_its_id_or_derived_label() {
    let plan = plan_args();
    assert_eq!(
        plan_path(&plan, &PlanRef::connection("a->b")).as_deref(),
        Some("connections[0]"),
        "the derived label names the unnamed connection"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::connection("d1")).as_deref(),
        Some("connections[1]"),
        "a carried id names its own row"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::connection("nope")),
        None,
        "an id no connection carries finds no path"
    );
}

#[test]
fn connection_labels_prefer_the_id_over_from_arrow_to() {
    let plan = plan_args();
    let unnamed = plan.connections.first().expect("the opening");
    assert_eq!(connection_label(unnamed), "a->b");
    let named = plan.connections.get(1).expect("the door");
    assert_eq!(connection_label(named), "d1");
}

#[test]
fn plan_paths_reach_feature_rows_by_their_named_ids() {
    let plan = feature_plan();
    assert_eq!(
        plan_path(&plan, &PlanRef::feature("ghost")).as_deref(),
        Some("features[0]"),
        "a feature's room names the row"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::feature("nope")).as_deref(),
        Some("features[1]"),
        "the opens value names the row"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::room("a")).as_deref(),
        Some("rooms[0]"),
        "a room ref names the room, not the switch that references it"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::connection("d1")).as_deref(),
        Some("connections[0]"),
        "a connection ref names the connection, not the feature naming it"
    );
    assert_eq!(
        plan_path(&plan, &PlanRef::feature("nowhere")),
        None,
        "an id no feature row names finds no path"
    );
}

#[test]
fn feature_refusals_carry_their_plan_path() {
    let plan = feature_plan();
    let prefixed = with_plan_path(
        CyberError::invalid("switch feature opens unknown connection 'nope'")
            .at_plan(PlanRef::feature("nope")),
        &plan,
    );
    assert_eq!(prefixed.code(), "invalid_argument", "the code stands");
    assert_eq!(
        prefixed.to_string(),
        "features[1]: switch feature opens unknown connection 'nope'",
        "the path leads"
    );
}

#[test]
fn with_plan_path_prefixes_refusals_and_keeps_the_rest() {
    let plan = plan_args();
    let invalid = with_plan_path(
        CyberError::invalid("room 'a' has a 0x64 rectangle").at_plan(PlanRef::room("a")),
        &plan,
    );
    assert_eq!(invalid.code(), "invalid_argument", "the code stands");
    assert_eq!(
        invalid.to_string(),
        "rooms[0]: room 'a' has a 0x64 rectangle",
        "the path leads"
    );
    let limit = with_plan_path(
        CyberError::LimitExceeded("connection 'd1' needs 9999 steps".to_owned())
            .at_plan(PlanRef::connection("d1")),
        &plan,
    );
    assert_eq!(limit.code(), "limit_exceeded", "limits keep their code");
    assert!(limit.to_string().starts_with("connections[1]: "), "{limit}");
    let unhandled = with_plan_path(
        CyberError::Internal("room 'a' broke the server".to_owned()).at_plan(PlanRef::room("a")),
        &plan,
    );
    assert!(
        matches!(&unhandled, CyberError::AtPlan { inner, at }
            if matches!(**inner, CyberError::Internal(_)) && at.id() == "a"),
        "a contract break keeps its own shape: {unhandled:?}"
    );
    assert_eq!(
        unhandled.message(),
        "room 'a' broke the server",
        "and its own words"
    );
    let unattributed = with_plan_path(
        CyberError::invalid("plan has no rooms; add at least one room"),
        &plan,
    );
    assert_eq!(
        unattributed.to_string(),
        "plan has no rooms; add at least one room",
        "no element reference, no prefix"
    );
}

#[test]
fn bounds_fold_the_vertex_box_into_the_result_object() {
    assert_eq!(
        bounds_of(&Map::default()),
        None,
        "an empty map has no bounds"
    );
    let mut map = Map::new_empty(
        LumpName::parse("MAP01").expect("a valid name"),
        MapFormat::Doom,
    );
    map.vertices.push(crate::wad::map::Vertex {
        x: -64_i32,
        y: 8_i32,
        ..Default::default()
    });
    let bounds = bounds_of(&map).expect("one vertex bounds itself");
    assert_eq!(
        serde_json::to_value(&bounds).expect("it serializes"),
        serde_json::json!({"x1": -64_i64, "y1": 8_i64, "x2": -64_i64, "y2": 8_i64}),
        "the documented field names"
    );
}
