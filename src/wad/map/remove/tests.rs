//! Shared builders for the element-removal engine's tests: the single-index
//! [`Removal`] constructors and the linedef-endpoint reader the cascade and
//! apply topics check against.
//!
//! [`cascade`] grows seeded removals and applies the expanded sets;
//! [`apply`] turns the pruning knobs off and reads the refusals.

mod apply;
mod cascade;

use alloc::collections::BTreeSet;

use super::Removal;
use crate::wad::map::{Linedef, Map};

/// A removal of the one vertex `vertex` and nothing else.
pub(super) fn vertex_removal(vertex: usize) -> Removal {
    Removal {
        vertices: BTreeSet::from([vertex]),
        ..Default::default()
    }
}

/// A removal of the one linedef `linedef` and nothing else.
pub(super) fn linedef_removal(linedef: usize) -> Removal {
    Removal {
        linedefs: BTreeSet::from([linedef]),
        ..Default::default()
    }
}

/// A removal of the one sidedef `sidedef` and nothing else.
pub(super) fn sidedef_removal(sidedef: usize) -> Removal {
    Removal {
        sidedefs: BTreeSet::from([sidedef]),
        ..Default::default()
    }
}

/// A removal of the one sector `sector` and nothing else.
pub(super) fn sector_removal(sector: usize) -> Removal {
    Removal {
        sectors: BTreeSet::from([sector]),
        ..Default::default()
    }
}

/// The `(start_x, start_y, stop_x, stop_y)` of `line` in `map`.
pub(super) fn ends_of(map: &Map, line: &Linedef) -> (i32, i32, i32, i32) {
    let start = &map.vertices[line.v1];
    let stop = &map.vertices[line.v2];
    (start.x, start.y, stop.x, stop.y)
}
