//! Shared builders for the `edit_sectors` tests: the parsed-request
//! readers, the error-code shorthand and the three-sector fixture both
//! topics run through.
//!
//! [`edit`] checks the append, the property edit and the removal with their
//! refusals, ranges and cascades; [`draw`] runs the loop and carve
//! parameters onto the same fixture.

mod draw;
mod edit;

use serde_json::from_str;

use super::draw::{CarveVoidParams, CreateSectorFromLoopParams};
use super::edit::{AddSectorParams, EditSectorParams, RemoveSectorParams};
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// An add request parsed from `json`.
pub(super) fn add_args(json: &str) -> AddSectorParams {
    from_str(json).expect("test add requests parse")
}

/// An edit request parsed from `json`.
pub(super) fn edit_args(json: &str) -> EditSectorParams {
    from_str(json).expect("test edit requests parse")
}

/// A remove request parsed from `json`.
pub(super) fn remove_args(json: &str) -> RemoveSectorParams {
    from_str(json).expect("test remove requests parse")
}

/// A vertex at `x`/`y`.
fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
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

/// A one-sided line from `v1` to `v2` over front sidedef `front`.
fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1,
        front: Some(front),
        ..Default::default()
    }
}

/// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 4,
        front: Some(front),
        back: Some(back),
        ..Default::default()
    }
}

/// A sector with the given heights, light and tag.
pub(super) fn room(floor_height: i32, ceiling_height: i32, light: i32, tag: i32) -> Sector {
    Sector {
        floor_height,
        ceiling_height,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "FLOOR1".to_owned(),
        light,
        tag,
        ..Default::default()
    }
}

/// The shared test map: a hall (sector 0, sidedefs 0/1), a raised room
/// (sector 1, sidedefs 2/3) split by the two-sided line `1`, and an
/// unreferenced sector `2`.
pub(super) fn test_map() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("a valid name"),
        format: MapFormat::Doom,
        vertices: vec![
            vertex(0, 0),
            vertex(128, 0),
            vertex(128, 128),
            vertex(0, 128),
        ],
        linedefs: vec![
            solid_line(0, 1, 0),
            open_line(1, 2, 1, 2),
            solid_line(2, 3, 3),
        ],
        sidedefs: vec![side(0), side(0), side(1), side(1)],
        sectors: vec![
            room(0, 128, 160, 0),
            room(32, 128, 192, 1),
            room(0, 128, 160, 0),
        ],
        ..Default::default()
    }
}

/// A loop request parsed from `json`.
pub(super) fn loop_args(json: &str) -> CreateSectorFromLoopParams {
    from_str(json).expect("test loop requests parse")
}

/// A carve request parsed from `json`.
pub(super) fn carve_args(json: &str) -> CarveVoidParams {
    from_str(json).expect("test carve requests parse")
}

/// The three-corner points parameter list, as JSON.
const THREE_POINTS: &str =
    r#""points": [{"x": 0, "y": 0}, {"x": 128, "y": 0}, {"x": 64, "y": 128}]"#;

/// The points form over [`THREE_POINTS`], as a JSON object.
pub(super) fn three_points_json(tail: &str) -> String {
    format!("{{{THREE_POINTS}{tail}}}")
}
