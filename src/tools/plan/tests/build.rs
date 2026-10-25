//! The build's parts: one compiled plan through the same pieces, the
//! thing rows' resolution and the player-start check, the report frame's
//! fields and the finalized node build and validation view.

use crate::defs::definitions_for;
use crate::plan::compile;
use crate::plan::things::{PlannedThing, Skill, ThingSpot};
use crate::plan::{FeatureLineEnds, FeatureRow};
use crate::tools::plan::PlanTheme;
use crate::tools::plan::VALIDATION_ERROR_CAP;
use crate::tools::plan::build::{
    ensure_player_start, finalize_nodes, reporting, resolve_specs, validation_view,
};
use crate::tools::plan::params::{KindSource, ThingRequest};
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::FlagFamily;
use crate::wad::map::validate::{Issue, Outcome as ValidateOutcome, Severity};
use crate::wad::name::LumpName;

use super::{args, compiled_room, issue, request};

/// A minimal compile for the integration-free smoke: one room,
/// no connections, roles answering nothing.
#[test]
fn a_single_room_plan_compiles_through_the_same_parts() {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": []}"#,
    );
    let model = plan.to_model().expect("the wire maps onto the model");
    let theme = PlanTheme { names: Vec::new() };
    let name = LumpName::parse("MAP05").expect("a valid name");
    let compiled = compile(&model, name, MapFormat::Doom, &theme).expect("one room compiles");
    assert_eq!(compiled.map.sectors.len(), 1, "just the room");
    assert_eq!(compiled.room_sectors.get("a"), Some(&vec![0_usize]));
    assert_eq!(
        compiled.map.bounds().expect("vertices"),
        (0_i32, 0_i32, 64_i32, 64_i32)
    );
}

#[test]
fn resolve_specs_reads_names_keeps_numbers_and_folds_flags() {
    let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
    let numbered = request(KindSource::Type(2035_u32), "things[0]");
    let named = ThingRequest {
        kind: KindSource::Name("imp".to_owned()),
        spot: ThingSpot::Room { id: "a".to_owned() },
        skills: Some(vec![Skill::Easy]),
        path: "things[1]".to_owned(),
        ..numbered
    };
    let start = request(KindSource::PlayerOne, "player_start");
    let specs = resolve_specs(&[numbered, named, start], FlagFamily::Doom, &definitions)
        .expect("the rows resolve");
    assert_eq!(
        specs.iter().map(|spec| spec.kind).collect::<Vec<u32>>(),
        vec![2035_u32, 3001_u32, 1_u32],
        "type passes, name resolves, player_start means type 1"
    );
    assert_eq!(
        specs.iter().map(|spec| spec.flags).collect::<Vec<u32>>(),
        vec![7_u32, 1_u32, 7_u32],
        "defaults are all-skill presence; easy-only stays easy-only"
    );
    assert_eq!(specs.first().expect("three rows").path, "things[0]");
}

#[test]
fn an_unresolvable_name_is_refused_with_the_row_path() {
    let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
    let bad = request(KindSource::Name("platypus".to_owned()), "things[2]");
    let error = resolve_specs(&[bad], FlagFamily::Doom, &definitions).expect_err("no such thing");
    assert_eq!(error.code(), "invalid_argument");
    let message = error.to_string();
    assert!(message.contains("things[2]"), "the path leads: {message}");
    assert!(message.contains("no thing named 'platypus'"), "{message}");
}

#[test]
fn ensure_player_start_reads_resolved_kinds_not_wire_names() {
    let monster = PlannedThing {
        kind: 3001_u32,
        spot: ThingSpot::Point((8_i64, 8_i64)),
        count: 1_u32,
        angle: 0_i32,
        flags: 7_u32,
        path: "things[0]".to_owned(),
    };
    let error = ensure_player_start(core::slice::from_ref(&monster)).expect_err("no type 1");
    assert_eq!(error.code(), "invalid_argument");
    assert_eq!(
        error.to_string(),
        "a map needs a player 1 start; pass player_start or a thing of type 1"
    );
    let start = PlannedThing {
        kind: 1_u32,
        ..monster.clone()
    };
    ensure_player_start(&[monster, start]).expect("the resolved type 1 satisfies the check");
}

#[test]
fn the_report_counts_placed_things_under_its_documented_field() {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": []}"#,
    );
    let model = plan.to_model().expect("the wire maps onto the model");
    let theme = PlanTheme { names: Vec::new() };
    let compiled = compile(
        &model,
        LumpName::parse("MAP05").expect("a valid name"),
        MapFormat::Doom,
        &theme,
    )
    .expect("one room compiles");
    let value = serde_json::to_value(reporting(&compiled, 5_usize, Vec::new(), None))
        .expect("it serializes");
    assert_eq!(
        value["things_placed"],
        serde_json::json!(5_u64),
        "the live field"
    );
    assert_eq!(
        value["features"],
        serde_json::json!([]),
        "a plan with no doors reports no feature rows"
    );
    assert_eq!(
        value["nodes"],
        serde_json::Value::Null,
        "null without finalize"
    );
    assert_eq!(
        value["validation"],
        serde_json::Value::Null,
        "null without finalize"
    );
}

#[test]
fn the_report_lists_feature_rows_after_their_connections() {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": []}"#,
    );
    let model = plan.to_model().expect("the wire maps onto the model");
    let theme = PlanTheme { names: Vec::new() };
    let compiled = compile(
        &model,
        LumpName::parse("MAP05").expect("a valid name"),
        MapFormat::Doom,
        &theme,
    )
    .expect("one room compiles");
    let rows = vec![FeatureRow {
        kind: "door",
        room: None,
        sectors: vec![2_usize],
        lines: vec![3_usize, 4_usize],
        tag: Some(1_i32),
        line_ends: vec![FeatureLineEnds {
            index: 3_usize,
            x1: 0_i32,
            y1: 8_i32,
            x2: 0_i32,
            y2: 40_i32,
        }],
    }];
    let value =
        serde_json::to_value(reporting(&compiled, 1_usize, rows, None)).expect("it serializes");
    assert_eq!(
        value["features"],
        serde_json::json!([
            {"kind": "door", "room": null, "sectors": [2_u64], "lines": [3_u64, 4_u64],
             "tag": 1_u64,
             "line_ends": [{"index": 3_u64, "x1": 0_i64, "y1": 8_i64, "x2": 0_i64, "y2": 40_i64}]}
        ]),
        "the documented row shape"
    );
}

#[test]
fn finalize_fills_the_nodes_and_validation_result_fields() {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": [],
            "finalize": true}"#,
    );
    assert!(plan.finalize, "the wire flag arrives");
    let model = plan.to_model().expect("the wire maps onto the model");
    let theme = PlanTheme { names: Vec::new() };
    let compiled = compile(
        &model,
        LumpName::parse("MAP05").expect("a valid name"),
        MapFormat::Doom,
        &theme,
    )
    .expect("one room compiles");
    let value = serde_json::to_value(reporting(
        &compiled,
        1_usize,
        Vec::new(),
        Some((
            serde_json::json!({"variant": "classic", "segs": 4_u64}),
            serde_json::json!({"errors": [], "counts": {"error": 0_u64}}),
        )),
    ))
    .expect("it serializes");
    assert_eq!(value["nodes"]["variant"], serde_json::json!("classic"));
    assert_eq!(value["nodes"]["segs"], serde_json::json!(4_u64));
    assert_eq!(value["validation"]["errors"], serde_json::json!([]));
}

#[test]
fn finalize_defaults_to_off_on_the_wire() {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": []}"#,
    );
    assert!(!plan.finalize, "absent means no node build");
}

#[test]
fn finalize_nodes_installs_the_whole_binary_run_in_place() {
    let mut compiled = compiled_room(MapFormat::Doom);
    let before = compiled.map.vertices.len();
    let summary = finalize_nodes(&mut compiled).expect("a square builds a tree");
    assert_eq!(
        summary["variant"],
        serde_json::json!("classic"),
        "the binary default layout"
    );
    assert!(
        summary["segs"].as_u64().expect("a count") > 0,
        "segs written"
    );
    assert!(
        summary["subsectors"].as_u64().expect("a count") > 0,
        "subsectors written"
    );
    assert!(
        compiled.map.vertices.len() >= before,
        "the classic run only ever adds split vertices"
    );
    for wanted in [
        b"SEGS".as_slice(),
        b"SSECTORS",
        b"NODES",
        b"REJECT",
        b"BLOCKMAP",
    ] {
        assert!(
            compiled
                .map
                .other
                .iter()
                .any(|lump| lump.name.as_bytes() == wanted),
            "the binary finalize installs {wanted:?}: {:?}",
            compiled
                .map
                .other
                .iter()
                .map(|lump| lump.name.to_string())
                .collect::<Vec<String>>()
        );
    }
}

#[test]
fn finalize_nodes_gives_a_udmf_model_the_znodes_lump_only() {
    let mut compiled = compiled_room(MapFormat::Udmf);
    let summary = finalize_nodes(&mut compiled).expect("a square builds a tree");
    assert_eq!(
        summary["variant"],
        serde_json::json!("extended"),
        "the UDMF default layout"
    );
    assert_eq!(
        summary["format"],
        serde_json::json!("ZNOD"),
        "compressed by default"
    );
    assert!(
        compiled
            .map
            .other
            .iter()
            .any(|lump| lump.name.as_bytes() == b"ZNODES"),
        "the extended lump lands in ZNODES"
    );
    for absent in [b"REJECT".as_slice(), b"BLOCKMAP", b"SEGS", b"SSECTORS"] {
        assert!(
            !compiled
                .map
                .other
                .iter()
                .any(|lump| lump.name.as_bytes() == absent),
            "{absent:?} stays out of a UDMF block"
        );
    }
}

#[test]
fn validation_view_caps_the_error_rows_and_keeps_counts_exact() {
    let mut issues: Vec<Issue> = (0..VALIDATION_ERROR_CAP + 7_usize)
        .map(|number| issue(Severity::Error, number))
        .collect();
    issues.push(issue(Severity::Warning, 0_usize));
    issues.push(issue(Severity::Info, 0_usize));
    issues.push(issue(Severity::Warning, 1_usize));
    let outcome = ValidateOutcome {
        checks_run: Vec::new(),
        issues,
    };
    let value = validation_view(&outcome);
    let errors = value["errors"].as_array().expect("an error list");
    assert_eq!(
        errors.len(),
        VALIDATION_ERROR_CAP,
        "the rows are capped at the documented 20"
    );
    assert_eq!(
        value["counts"]["error"],
        serde_json::json!(27_u64),
        "the count keeps the whole truth"
    );
    assert_eq!(value["counts"]["warning"], serde_json::json!(2_u64));
    assert_eq!(value["counts"]["info"], serde_json::json!(1_u64));
    assert_eq!(
        errors.first().expect("a row")["code"],
        serde_json::json!("synthetic"),
        "rows keep the validate_map issue shape"
    );
}

#[test]
fn validation_view_of_a_clean_map_holds_no_rows() {
    let outcome = ValidateOutcome {
        checks_run: Vec::new(),
        issues: vec![issue(Severity::Info, 0_usize)],
    };
    let value = validation_view(&outcome);
    assert_eq!(value["errors"], serde_json::json!([]), "only the info row");
    assert_eq!(value["counts"]["error"], serde_json::json!(0_u64));
    assert_eq!(value["counts"]["info"], serde_json::json!(1_u64));
}
