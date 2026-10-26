//! Shared builders for the `edit_vertices` tests: the parsed-request
//! readers, the error-code shorthand and the two-room fixture both topics
//! run through.
//!
//! [`add_move`] checks the append, the coordinate ranges and the whole move
//! path; [`remove`] runs the removal step, its `conflict` refusal and the
//! pruning flags that reach the cascade.

mod add_move;
mod remove;

use serde_json::from_str;

use super::{AddVertexParams, MoveVertexParams, RemoveVertexParams};
use crate::error::CyberError;
use crate::session::{Layer, LayerRole, Session};
use crate::wad::archive::{Wad, WadKind};
use crate::wad::classify::MapFormat;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;
use std::path::PathBuf;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// An add request parsed from `json`.
pub(super) fn add_args(json: &str) -> AddVertexParams {
    from_str(json).expect("test add requests parse")
}

/// A move request parsed from `json`.
pub(super) fn move_args(json: &str) -> MoveVertexParams {
    from_str(json).expect("test move requests parse")
}

/// A remove request parsed from `json`.
pub(super) fn remove_args(json: &str) -> RemoveVertexParams {
    from_str(json).expect("test remove requests parse")
}

/// A valid parsed test name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A vertex at `x`/`y`.
fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Default::default()
    }
}

/// A blocking one-sided line from `v1` to `v2` over sidedef `front`.
pub(super) fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1,
        front: Some(front),
        ..Default::default()
    }
}

/// A two-sided step line from `v1` to `v2` over sidedefs `front`/`back`.
fn step_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4,
        front: Some(front),
        back: Some(back),
        ..Default::default()
    }
}

/// A sidedef opening into `sector`.
fn side(sector: usize) -> Sidedef {
    Sidedef {
        sector,
        ..Default::default()
    }
}

/// The two-room map the test fixtures build: a `0,0` to `512,256` hall
/// and raised back room, six vertices, seven lines (`2` the two-sided
/// step), eight sides over two sectors.
fn two_room() -> Map {
    Map {
        name: name("MAP01"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            solid_line(0, 5, 0),
            solid_line(5, 4, 1),
            step_line(4, 1, 2, 3),
            solid_line(1, 0, 4),
            solid_line(4, 3, 5),
            solid_line(3, 2, 6),
            solid_line(2, 1, 7),
        ],
        sidedefs: vec![
            side(0),
            side(0),
            side(0),
            side(1),
            side(0),
            side(1),
            side(1),
            side(1),
        ],
        sectors: vec![Sector::default(), Sector::default()],
        ..Default::default()
    }
}

/// The two-room map in `format`.
pub(super) fn fixture(format: MapFormat) -> Map {
    let mut map = two_room();
    map.format = format;
    map
}

/// A session whose only layer is an empty archive (no map blocks).
pub(super) fn empty_session() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: Vec::new(),
            },
        ))
        .expect("no batch is open");
    session
}
