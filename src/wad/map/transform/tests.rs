//! Shared builders for the selection transform's tests: the one-operation
//! [`Operations`] constructors, the exact [`Selection`] maker, the stored
//! coordinate reader and the default-origin transform wrapper both topics run
//! through.
//!
//! `maths` checks the point arithmetic, the default origin and the selection
//! expansion; `engine` runs whole transforms and reads every refusal.

mod engine;
mod maths;

use super::{Operations, Point, Rotation, Scale, Selection, TransformReport, apply_transform};
use crate::error::CyberError;
use crate::wad::map::Map;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A translate-only operation set.
pub(super) fn shifted(dx: i64, dy: i64) -> Operations {
    Operations {
        translate: Some(Point { x: dx, y: dy }),
        ..Default::default()
    }
}

/// A rotation-only operation set of whole `degrees`.
pub(super) fn turned(degrees: i64) -> Operations {
    Operations {
        rotation: Some(Rotation::from_degrees(degrees)),
        ..Default::default()
    }
}

/// A scale-only operation set of `num / den`.
pub(super) fn grown(num: i32, den: i32) -> Operations {
    Operations {
        scale: Some(Scale::new(num, den).expect("test fractions are positive")),
        ..Default::default()
    }
}

/// The point `(x, y)`.
pub(super) const fn point(x: i64, y: i64) -> Point {
    Point { x, y }
}

/// A selection of exactly these vertices and things.
pub(super) fn selection(vertices: &[usize], things: &[usize]) -> Selection {
    Selection {
        vertices: vertices.iter().copied().collect(),
        things: things.iter().copied().collect(),
    }
}

/// The coordinates of `map`'s vertex `index`.
pub(super) fn vertex_at(map: &Map, index: usize) -> (i32, i32) {
    let vertex = map.vertices.get(index).expect("test vertices exist");
    (vertex.x, vertex.y)
}

/// Transform `map`'s selection with `ops` about the default origin.
pub(super) fn transform(
    map: &mut Map,
    picked: &Selection,
    ops: &Operations,
) -> Result<TransformReport, CyberError> {
    apply_transform(map, picked, ops, None)
}
