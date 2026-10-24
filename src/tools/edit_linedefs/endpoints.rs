//! The endpoint forms of `add_linedef`: the two shapes one call accepts,
//! the refusal they share and the pass that resolves them against a map.
//!
//! [`endpoint_spec`] reads the wire parameters
//! [`crate::tools::edit_linedefs::add::AddLinedefParams`] carries into
//! exactly one [`EndpointSpec`] - existing vertex indices or `from`/`to`
//! points - and [`place_endpoints`] turns that into [`Ends`]: indices checked
//! against the map's vertices, points reused through the parent's
//! [`find_vertex`] or appended by [`place_point`] and recorded in the order
//! they were created. Endpoints coming out of one point are the
//! [`zero_endpoint`] refusal.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::tools::maps::params::check_index;
use crate::wad::map::{Map, Vertex};

use super::add::AddLinedefParams;
use super::remove::PointParam;
use super::{check_binary_coords, find_vertex};

/// The two endpoint forms of an [`Cyberdemon::add_linedef`] call, checked
/// to be exactly one of them.
#[derive(Debug)]
pub(super) enum EndpointSpec<'req> {
    /// Existing vertex indices, as given.
    Indices {
        /// The start vertex.
        v1: usize,
        /// The end vertex.
        v2: usize,
    },
    /// Endpoints as points: existing vertices are reused, missing ones
    /// are created.
    Points {
        /// The start point.
        from: &'req PointParam,
        /// The end point.
        to: &'req PointParam,
    },
}

/// The endpoints after resolving them against the map.
pub(super) struct Ends {
    /// The start vertex index.
    pub(super) v1: usize,
    /// The end vertex index.
    pub(super) v2: usize,
    /// Indices of the vertices this call appended, ascending.
    pub(super) created: Vec<usize>,
}

/// The refusal for endpoints given in both forms or in neither.
fn endpoint_refusal() -> CyberError {
    CyberError::InvalidArgument(
        "add_linedef needs its two endpoints in exactly one form: pass v1 and v2 (existing \
         vertex indices) or from and to (points {x, y})"
            .to_owned(),
    )
}

/// The add call's endpoint form: exactly one of `v1`/`v2` or `from`/`to`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when both forms or neither is given,
/// or when one form is incomplete.
pub(super) fn endpoint_spec(args: &AddLinedefParams) -> Result<EndpointSpec<'_>, CyberError> {
    let points = args.from.is_some() || args.to.is_some();
    match (args.v1, args.v2) {
        (Some(_), Some(_)) if points => Err(endpoint_refusal()),
        (Some(v1), Some(v2)) => Ok(EndpointSpec::Indices { v1, v2 }),
        (None, None) => match (args.from.as_ref(), args.to.as_ref()) {
            (Some(from), Some(to)) => Ok(EndpointSpec::Points { from, to }),
            _ => Err(endpoint_refusal()),
        },
        _ => Err(endpoint_refusal()),
    }
}

/// The vertex `point` names for `map`: an existing one at exactly these
/// coordinates, else an appended one recorded in `created`.
fn place_point(
    map: &mut Map,
    point: PointParam,
    created: &mut Vec<usize>,
) -> Result<usize, CyberError> {
    check_binary_coords(map.format, point.x, point.y)?;
    find_vertex(map, point.x, point.y).map_or_else(
        || {
            let index = map.vertices.len();
            map.vertices.push(Vertex {
                x: point.x,
                y: point.y,
                extra: BTreeMap::new(),
            });
            created.push(index);
            Ok(index)
        },
        Ok,
    )
}

/// The `invalid_argument` for a pair of endpoints on one point.
fn zero_endpoint(v1: usize, v2: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "the two endpoints are one point (vertex {v1} = vertex {v2}): a linedef needs two \
         distinct endpoints; move one or name another vertex"
    ))
}

/// The resolved endpoints of an `add_linedef`: checked existing indices, or
/// points reused and created against `map`.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a vertex index the map does not hold,
/// [`CyberError::InvalidArgument`] for one endpoint or a coordinate above
/// the binary `i16` cap.
pub(super) fn place_endpoints(map: &mut Map, spec: &EndpointSpec<'_>) -> Result<Ends, CyberError> {
    let mut ends = match *spec {
        EndpointSpec::Indices { v1, v2 } => {
            check_index("vertex", v1, map.vertices.len())?;
            check_index("vertex", v2, map.vertices.len())?;
            Ends {
                v1,
                v2,
                created: Vec::new(),
            }
        }
        EndpointSpec::Points { from, to } => {
            let mut created = Vec::new();
            let v1 = place_point(map, *from, &mut created)?;
            let v2 = place_point(map, *to, &mut created)?;
            Ends { v1, v2, created }
        }
    };
    if ends.v1 == ends.v2 {
        return Err(zero_endpoint(ends.v1, ends.v2));
    }
    ends.created.sort_unstable();
    Ok(ends)
}
