//! Unit tests of the plan group: the connection and feature names the
//! result carries, the theme role tables and their notes, the plan paths
//! that prefix a refusal, the resolved thing specs, the report's fields
//! and what `finalize` fills in.
//!
//! [`args`], [`plan_args`], [`feature_plan`], [`request`],
//! [`compiled_room`] and [`issue`] are the fixtures the topics share.
//! [`naming`] checks the wire names, the dressing roles and the plan
//! paths; [`build`] the resolved specs, the report frame and the
//! finalize steps.

mod build;
mod naming;

use serde_json::from_str;

use crate::plan::CompiledPlan;
use crate::plan::compile;
use crate::plan::emit::MapIdentity;
use crate::plan::things::ThingSpot;
use crate::tools::plan::PlanTheme;
use crate::tools::plan::params::{BuildPlanParams, KindSource, ThingRequest};
use crate::wad::classify::MapFormat;
use crate::wad::map::validate::{Issue, Severity};
use crate::wad::name::LumpName;

/// A request parsed from `json`.
pub(super) fn args(json: &str) -> BuildPlanParams {
    from_str(json).expect("test requests parse")
}

/// A two-room, one-door request with the ids the path tests need.
pub(super) fn plan_args() -> BuildPlanParams {
    args(
        r#"{"map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
                {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128}}
            ],
            "connections": [
                {"from": "a", "to": "b", "kind": "opening"},
                {"id": "d1", "from": "a", "to": "b", "kind": "door"}
            ]}"#,
    )
}

/// A two-room, one-door request carrying an exit at an unknown room
/// and a switch opening an unknown connection id.
pub(super) fn feature_plan() -> BuildPlanParams {
    args(
        r#"{"map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
                {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128}}
            ],
            "connections": [
                {"id": "d1", "from": "a", "to": "b", "kind": "door", "style": "remote"}
            ],
            "features": [
                {"kind": "exit", "room": "ghost", "style": "walk_over"},
                {"kind": "switch", "room": "a", "opens": "nope"}
            ]}"#,
    )
}

/// A request row with the shape tests' defaults.
pub(super) fn request(kind: KindSource, path: &str) -> ThingRequest {
    ThingRequest {
        kind,
        spot: ThingSpot::Point((8_i64, 8_i64)),
        count: 1_u32,
        angle: 0_i32,
        skills: None,
        multiplayer_only: false,
        path: path.to_owned(),
    }
}

/// The identity a `Doom`-layout build test compiles into, with the
/// vanilla sky flat.
pub(super) fn doom_identity(name: LumpName) -> MapIdentity {
    MapIdentity {
        name,
        format: MapFormat::Doom,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    }
}

/// The one-room compiled plan both finalize tests grow a tree from.
pub(super) fn compiled_room(format: MapFormat) -> CompiledPlan {
    let plan = args(
        r#"{"map": "MAP05",
            "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
            "connections": []}"#,
    );
    let model = plan.to_model().expect("the wire maps onto the model");
    let theme = PlanTheme { names: Vec::new() };
    let identity = MapIdentity {
        name: LumpName::parse("MAP05").expect("a valid name"),
        format,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    };
    compile(&model, identity, &theme).expect("one room compiles")
}

/// An [`Issue`] row of `severity` carrying a numbered message.
pub(super) fn issue(severity: Severity, number: usize) -> Issue {
    Issue {
        severity,
        code: "synthetic",
        message: format!("synthetic issue {number}"),
        vertices: Vec::new(),
        linedefs: Vec::new(),
        sidedefs: Vec::new(),
        sectors: Vec::new(),
        things: Vec::new(),
        x: None,
        y: None,
    }
}
