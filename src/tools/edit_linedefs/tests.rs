//! Shared builders for the `edit_linedefs` tests: the parsed-request
//! readers, the error-code shorthand, the two-room fixture, the resolved
//! endpoint form and the removal runners both topics use.
//!
//! [`add`] checks the endpoint forms, the side, family and record rules and
//! the append step; [`edit`] runs the request validation, the computed edits,
//! the side edits and the cascading removals.

mod add;
mod edit;

use serde_json::from_str;

use super::add::AddLinedefParams;
use super::edit::EditLinedefParams;
use super::endpoints::{EndpointSpec, endpoint_spec};
use super::remove::{RemoveLinedefParams, remove_in_place};
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::remove::RemovalReport;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
use crate::wad::name::LumpName;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// An add request parsed from `json`.
pub(super) fn add_args(json: &str) -> AddLinedefParams {
    from_str(json).expect("test add requests parse")
}

/// An edit request parsed from `json`.
pub(super) fn edit_args(json: &str) -> EditLinedefParams {
    from_str(json).expect("test edit requests parse")
}

/// A remove request parsed from `json`.
fn remove_args(json: &str) -> RemoveLinedefParams {
    from_str(json).expect("test remove requests parse")
}

/// A valid parsed test name.
fn name(text: &str) -> LumpName {
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

/// A sidedef with the given textures over `sector`.
pub(super) fn side(upper: &str, middle: &str, lower: &str, sector: usize) -> Sidedef {
    Sidedef {
        upper: upper.to_owned(),
        middle: middle.to_owned(),
        lower: lower.to_owned(),
        sector,
        ..Default::default()
    }
}

/// The two-room map the fixtures build: six vertices (`0,0` to
/// `512,256`), seven lines (`2` the two-sided step from vertex 4 to
/// vertex 1), eight sides over two sectors.
pub(super) fn fixture(format: MapFormat) -> Map {
    Map {
        name: name("MAP01"),
        format,
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
            side("-", "WALL1", "-", 0),
            side("-", "WALL1", "-", 0),
            side("-", "STEP1", "-", 0),
            side("-", "-", "-", 1),
            side("-", "WALL1", "-", 0),
            side("-", "WALL1", "-", 1),
            side("-", "WALL1", "-", 1),
            side("-", "WALL1", "-", 1),
        ],
        sectors: vec![Sector::default(), Sector::default()],
        ..Default::default()
    }
}

/// The resolved endpoint form of a parsed add request.
pub(super) fn spec_of(args: &AddLinedefParams) -> EndpointSpec<'_> {
    endpoint_spec(args).expect("test endpoint forms resolve")
}

/// Run `remove_in_place` on `map` with the request parsed from `json`.
pub(super) fn removal(json: &str, map: &mut Map) -> RemovalReport {
    let args = remove_args(json);
    let indices = args.index.clone().indices().expect("test indices parse");
    remove_in_place(map, &args, &indices).expect("the removal applies")
}

/// The error `remove_in_place` gives for the request parsed from
/// `json`.
pub(super) fn removal_error(json: &str, map: &mut Map) -> CyberError {
    let args = remove_args(json);
    let indices = args.index.clone().indices().expect("test indices parse");
    remove_in_place(map, &args, &indices).expect_err("the request must refuse")
}
