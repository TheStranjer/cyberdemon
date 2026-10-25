//! Selection expansion: how one [`SelectionRequest`] becomes the vertex and
//! thing sets [`expand_selection`] hands to the transform.
//!
//! The named vertices and things are the seed, every named linedef adds its
//! two endpoints and every named sector the endpoints of all its boundary
//! linedefs plus, on request, the things standing in it, all judged on the
//! untouched map; a reference that dangles contributes nothing. Index lists
//! are checked against the map's blocks first and an expansion holding
//! neither a vertex nor a thing is refused, so a call that reaches the
//! transform always has something to move.

use alloc::collections::BTreeSet;

use crate::error::CyberError;
use crate::wad::map::query::{ElementKind, check_indices};

use super::super::geometry::{point_in_sector, sector_boundary};
use super::super::model::{Linedef, Map};
use super::{Selection, SelectionRequest};

/// Expand `request` against `map` into the vertex and thing indices to move.
///
/// The rules: `all` selects every vertex and thing; otherwise the named
/// vertices and things are the seed, every named linedef adds its two
/// endpoints, every named sector adds the endpoints of all its boundary
/// linedefs ([`sector_boundary`]) and, when `include_things_in_sectors`,
/// the things whose [`point_in_sector`] is one of them (judged on the
/// untouched map). References that dangle simply do not contribute.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a requested index the map does not hold,
/// naming the kind and index; [`CyberError::InvalidArgument`] when the
/// expansion holds no vertex and no thing.
#[inline]
pub fn expand_selection(map: &Map, request: &SelectionRequest) -> Result<Selection, CyberError> {
    check_requested(map, request)?;
    if request.all {
        return refuse_empty(Selection {
            vertices: (0..map.vertices.len()).collect(),
            things: (0..map.things.len()).collect(),
        });
    }
    let mut vertices: BTreeSet<usize> = request.vertices.iter().copied().collect();
    for index in &request.linedefs {
        add_endpoints(map, map.linedefs.get(*index), &mut vertices);
    }
    let mut things: BTreeSet<usize> = request.things.iter().copied().collect();
    if !request.sectors.is_empty() {
        add_sector_boundaries(map, &request.sectors, &mut vertices);
        if request.include_things_in_sectors {
            add_sector_things(map, &request.sectors, &mut things);
        }
    }
    refuse_empty(Selection { vertices, things })
}

/// Refuse unless every index `request` names exists in its block,
/// through the shared [`check_indices`] guard.
fn check_requested(map: &Map, request: &SelectionRequest) -> Result<(), CyberError> {
    check_indices(ElementKind::Vertices, &request.vertices, map)?;
    check_indices(ElementKind::Linedefs, &request.linedefs, map)?;
    check_indices(ElementKind::Sectors, &request.sectors, map)?;
    check_indices(ElementKind::Things, &request.things, map)
}

/// Add a resolvable linedef's two endpoints, skipping references that
/// dangle past the vertex block.
fn add_endpoints(map: &Map, picked: Option<&Linedef>, out: &mut BTreeSet<usize>) {
    let Some(line) = picked else {
        return;
    };
    for vertex in [line.v1, line.v2] {
        if vertex < map.vertices.len() {
            out.insert(vertex);
        }
    }
}

/// Add the endpoints of every boundary linedef of every sector in
/// `sectors`.
fn add_sector_boundaries(map: &Map, sectors: &[usize], out: &mut BTreeSet<usize>) {
    for sector in sectors {
        for (linedef, _) in sector_boundary(map, *sector) {
            add_endpoints(map, map.linedefs.get(linedef), out);
        }
    }
}

/// Add every thing standing in one of `sectors`, judged on the map as it
/// now is (before any transform).
fn add_sector_things(map: &Map, sectors: &[usize], out: &mut BTreeSet<usize>) {
    for (index, object) in map.things.iter().enumerate() {
        let containing = point_in_sector(map, i64::from(object.x), i64::from(object.y));
        if sectors.iter().any(|sector| Some(*sector) == containing) {
            out.insert(index);
        }
    }
}

/// Pass a non-empty selection through; refuse one that holds nothing.
fn refuse_empty(selection: Selection) -> Result<Selection, CyberError> {
    if selection.vertices.is_empty() && selection.things.is_empty() {
        return Err(CyberError::InvalidArgument(
            "nothing selected: pass at least one of vertices, linedefs, sectors, things \
             or all=true (the expanded selection must hold a vertex or a thing)"
                .to_owned(),
        ));
    }
    Ok(selection)
}
