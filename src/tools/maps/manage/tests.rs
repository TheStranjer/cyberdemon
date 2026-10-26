//! Shared fixtures for the map-management tests: parsed names and lumps,
//! a seeded base-layer session and the working-archive readers the topics
//! assert against.
//!
//! [`plans`] checks the pure validators and block helpers of [`super`];
//! [`runs`] drives the four runs of [`runs`](super::runs).

mod plans;
mod runs;

use std::path::PathBuf;

use serde_json::Value;

use super::{CopyPlan, CreatePlan, plan_copy, plan_create};
use crate::error::CyberError;
use crate::session::{Layer, LayerRole, Session};
use crate::tools::maps::{CopyMapParams, CreateMapParams};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A lump named `text` carrying `payload` as its bytes.
pub(super) fn carrying(text: &str, payload: &str) -> Lump {
    Lump::new(name(text), payload.as_bytes().to_vec())
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A session whose only layer is a base archive of `lumps`.
pub(super) fn base_session(lumps: Vec<Lump>) -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ))
        .expect("no batch is open");
    session
}

/// A base layer: a `MAP01` block with its `GL` run and loose lumps.
pub(super) fn layer_map() -> Vec<Lump> {
    vec![
        marker("PLAYPAL"),
        marker("MAP01"),
        carrying("THINGS", "things"),
        carrying("LINEDEFS", "lines"),
        marker("GL_MAP01"),
        carrying("GL_VERT", "gl"),
        marker("ENDPAK"),
    ]
}

/// Appends `lumps` to the working archive as one seeded edit.
pub(super) fn seed(session: &mut Session, lumps: Vec<Lump>) {
    session
        .edit("seed", move |wad| {
            wad.lumps.extend(lumps);
            Ok(())
        })
        .expect("plain appends");
}

/// A `create_map` argument set.
pub(super) fn create_args(
    text: &str,
    format: Option<&str>,
    namespace: Option<&str>,
) -> CreateMapParams {
    CreateMapParams {
        name: text.to_owned(),
        format: format.map(str::to_owned),
        namespace: namespace.map(str::to_owned),
        replace: false,
    }
}

/// A validated `create_map` plan for `text` in `format`.
pub(super) fn create_plan(text: &str, format: &str, namespace: Option<&str>) -> CreatePlan {
    plan_create(&create_args(text, Some(format), namespace)).expect("valid plan")
}

/// A validated `create_map` plan for `text` with the default format.
pub(super) fn doom_plan(text: &str) -> CreatePlan {
    plan_create(&create_args(text, None, None)).expect("valid plan")
}

/// A `copy_map` argument set.
pub(super) fn copy_args(from: &str, to: &str, scope: Option<&str>, replace: bool) -> CopyMapParams {
    CopyMapParams {
        from: from.to_owned(),
        to: to.to_owned(),
        scope: scope.map(str::to_owned),
        replace,
    }
}

/// A validated `copy_map` plan.
pub(super) fn copy_plan(from: &str, to: &str, scope: &str, replace: bool) -> CopyPlan {
    plan_copy(&copy_args(from, to, Some(scope), replace)).expect("valid plan")
}

/// Serialize a report for whole-shape asserts.
pub(super) fn value<T: serde::Serialize>(report: &T) -> Value {
    serde_json::to_value(report).expect("reports serialize")
}

/// The working archive's directory names, in order.
pub(super) fn working_names(session: &Session) -> Vec<String> {
    session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect()
}

/// How many working lumps carry the name `text`.
pub(super) fn count_named(names: &[String], text: &str) -> usize {
    names.iter().filter(|seen| *seen == text).count()
}
