//! `add_linedef`: its arguments, the side it can create, the row and the
//! report its step produces, and the one-step work itself.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`AddLinedefParams`] to [`add_run`], which validates the request up front
//! with [`validate_add`] ([`endpoint_spec`] and [`check_side_pairs`], with
//! [`front_required`] for a call naming no front side), resolves the map and
//! runs [`add_in_place`] as one [`crate::session::Session::edit_map`] step.
//! Inside that step the endpoints come from [`place_endpoints`], a doubled
//! line is refused by [`check_duplicate`], the sides are referenced or
//! appended by [`resolve_sides`] (each through [`place_side`] over the sidedef
//! [`new_sidedef`] builds), the record [`build_line`] assembles faces the
//! parent's [`check_line`], and the [`AddRow`] it hands back becomes the
//! [`AddReport`] the call returns.

use alloc::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{FlagsInput, check_index, edit_result_fragment};
use crate::wad::classify::MapFormat;
use crate::wad::map::{FlagKind, Linedef, Map, Sidedef};

use super::edit::edit_row;
use super::endpoints::{EndpointSpec, Ends, endpoint_spec, place_endpoints};
use super::{
    LinedefDetail, ZERO_ARGS, check_binary_offsets, check_line, default_line_flags, new_texture,
};
use crate::tools::maps::params::PointParam;

/// Arguments for [`Cyberdemon::add_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddLinedefParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Start vertex index (with `v2`, one of the two endpoint forms).
    #[serde(default)]
    pub(super) v1: Option<usize>,
    /// End vertex index (with `v1`).
    #[serde(default)]
    pub(super) v2: Option<usize>,
    /// Start point `{x, y}` (with `to`, the other endpoint form; reuses a
    /// vertex already at exactly these coordinates, else creates one).
    #[serde(default)]
    pub(super) from: Option<PointParam>,
    /// End point `{x, y}` (with `from`).
    #[serde(default)]
    pub(super) to: Option<PointParam>,
    /// Front-side sidedef index (with `front_side`, one of the two front
    /// forms; one of them is required).
    #[serde(default)]
    pub(super) front: Option<usize>,
    /// Front side to create: `{sector, upper?, middle?, lower?, x_offset?,
    /// y_offset?}` (with `front`).
    #[serde(default)]
    pub(super) front_side: Option<NewSide>,
    /// Back-side sidedef index (with `back_side`, at most one form; no back
    /// keeps the line one-sided).
    #[serde(default)]
    pub(super) back: Option<usize>,
    /// Back side to create (with `back`).
    #[serde(default)]
    pub(super) back_side: Option<NewSide>,
    /// Flags: the raw bitset or names from the map's linedef flag table.
    /// Defaults to `blocking` (1) for a one-sided line, `twosided` (4)
    /// when a back side is given.
    #[serde(default)]
    pub(super) flags: Option<FlagsInput>,
    /// Line special number (default `0`).
    #[serde(default)]
    pub(super) special: Option<i32>,
    /// Trigger tag (default `0`; `Doom` family only).
    #[serde(default)]
    pub(super) tag: Option<i32>,
    /// The five script arguments (default zeros; `Hexen` family only).
    #[serde(default)]
    pub(super) args: Option<[i32; 5]>,
}

/// A sidedef to create with a new linedef, mirroring `add_sidedef`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct NewSide {
    /// Index of the sector this side opens into; it must exist.
    pub(super) sector: usize,
    /// Upper texture name (default `"-"`, no texture).
    #[serde(default)]
    pub(super) upper: Option<String>,
    /// Middle texture name (default `"-"`, no texture).
    #[serde(default)]
    pub(super) middle: Option<String>,
    /// Lower texture name (default `"-"`, no texture).
    #[serde(default)]
    pub(super) lower: Option<String>,
    /// Texture shift along the line in map units (default `0`).
    #[serde(default)]
    pub(super) x_offset: Option<i32>,
    /// Texture shift across the line in map units (default `0`).
    #[serde(default)]
    pub(super) y_offset: Option<i32>,
}

/// The result of `add_linedef`'s step: the new row plus what the call
/// appended to get there.
#[derive(Debug)]
pub(super) struct AddRow {
    /// Position in [`Map::linedefs`] after the append.
    pub(super) index: usize,
    /// The created linedef's fields.
    pub(super) detail: LinedefDetail,
    /// Indices of the vertices this call appended, ascending.
    pub(super) created_vertices: Vec<usize>,
    /// Indices of the sidedefs this call appended, ascending.
    pub(super) created_sidedefs: Vec<usize>,
}

/// Structured result of [`Cyberdemon::add_linedef`].
#[derive(Serialize)]
pub(super) struct AddReport {
    /// Index of the created linedef.
    index: usize,
    /// The created linedef's fields.
    linedef: LinedefDetail,
    /// Vertices created for the `from`/`to` endpoints.
    created_vertices: Vec<usize>,
    /// Sidedefs created from `front_side`/`back_side`.
    created_sidedefs: Vec<usize>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The sides after resolving them against the map.
struct Sides {
    /// The front sidedef index.
    front: usize,
    /// The back sidedef index, `None` for a one-sided line.
    back: Option<usize>,
    /// Indices of the sidedefs this call appended, ascending.
    created: Vec<usize>,
}

/// The sidedef the `front_side`/`back_side` parameter describes for a map
/// of `format`.
pub(super) fn new_sidedef(side: &NewSide, format: MapFormat) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: side.x_offset.unwrap_or_default(),
        y_offset: side.y_offset.unwrap_or_default(),
        upper: new_texture(side.upper.as_ref(), format)?,
        lower: new_texture(side.lower.as_ref(), format)?,
        middle: new_texture(side.middle.as_ref(), format)?,
        sector: side.sector,
        extra: BTreeMap::new(),
    })
}

/// Refuses a side named by both its index parameter and its create
/// parameter.
fn check_side_pairs(args: &AddLinedefParams) -> Result<(), CyberError> {
    if args.front.is_some() && args.front_side.is_some() {
        return Err(CyberError::InvalidArgument(
            "front conflicts with front_side: reference an existing sidedef or create a new \
             one, not both"
                .to_owned(),
        ));
    }
    if args.back.is_some() && args.back_side.is_some() {
        return Err(CyberError::InvalidArgument(
            "back conflicts with back_side: reference an existing sidedef or create a new \
             one, not both"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The refusal for a call without any front side.
fn front_required() -> CyberError {
    CyberError::InvalidArgument(
        "a linedef needs a front side: pass front (an existing sidedef index) or front_side \
         {sector, ...} (create one)"
            .to_owned(),
    )
}

/// Validates an `add_linedef` request up front: the endpoint form, the
/// side forms and the required front.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a wrong or missing endpoint form,
/// a side named twice, or no front side.
pub(super) fn validate_add(args: &AddLinedefParams) -> Result<EndpointSpec<'_>, CyberError> {
    let spec = endpoint_spec(args)?;
    check_side_pairs(args)?;
    if args.front.is_none() && args.front_side.is_none() {
        return Err(front_required());
    }
    Ok(spec)
}

/// The `already_exists` naming the linedef that runs between the two
/// vertices in either direction.
fn duplicate_line(index: usize, v1: usize, v2: usize) -> CyberError {
    CyberError::AlreadyExists(format!(
        "linedef {index} already runs between vertices {v1} and {v2} (in either direction): \
         edit it with edit_linedef or move an endpoint with move_vertex"
    ))
}

/// Refuses when a linedef between `v1` and `v2` already exists, either way
/// around.
fn check_duplicate(map: &Map, v1: usize, v2: usize) -> Result<(), CyberError> {
    map.linedefs
        .iter()
        .position(|line| (line.v1 == v1 && line.v2 == v2) || (line.v1 == v2 && line.v2 == v1))
        .map_or(Ok(()), |index| Err(duplicate_line(index, v1, v2)))
}

/// Resolves one side parameter pair: the referenced sidedef must exist;
/// the created one gets its sector checked, its textures validated and its
/// offsets range-checked, then it is appended and recorded in `created`.
fn place_side(
    map: &mut Map,
    reference: Option<usize>,
    side: Option<&NewSide>,
    created: &mut Vec<usize>,
) -> Result<Option<usize>, CyberError> {
    if let Some(existing) = reference {
        check_index("sidedef", existing, map.sidedefs.len())?;
        return Ok(Some(existing));
    }
    let Some(new) = side else {
        return Ok(None);
    };
    check_index("sector", new.sector, map.sectors.len())?;
    let created_side = new_sidedef(new, map.format)?;
    check_binary_offsets(map.format, &created_side)?;
    let index = map.sidedefs.len();
    map.sidedefs.push(created_side);
    created.push(index);
    Ok(Some(index))
}

/// The resolved sides of an `add_linedef`, front required.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a sidedef or sector index the map does not
/// hold, [`CyberError::InvalidArgument`] for an out-of-range texture name,
/// offset or missing front side.
fn resolve_sides(map: &mut Map, args: &AddLinedefParams) -> Result<Sides, CyberError> {
    let mut created = Vec::new();
    let front = place_side(map, args.front, args.front_side.as_ref(), &mut created)?;
    let back = place_side(map, args.back, args.back_side.as_ref(), &mut created)?;
    let Some(front_index) = front else {
        return Err(front_required());
    };
    created.sort_unstable();
    Ok(Sides {
        front: front_index,
        back,
        created,
    })
}

/// The linedef the `add_linedef` call describes over the resolved `ends`
/// and `sides`, checked against the map's family and format ranges.
fn build_line(
    map: &Map,
    args: &AddLinedefParams,
    ends: &Ends,
    sides: &Sides,
) -> Result<Linedef, CyberError> {
    let family = map.flag_family();
    let given = args.flags.clone();
    let line = Linedef {
        v1: ends.v1,
        v2: ends.v2,
        flags: given.map_or_else(
            || Ok(default_line_flags(sides.back.is_some())),
            |input| input.resolve(FlagKind::Linedef, family),
        )?,
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        args: args.args.unwrap_or(ZERO_ARGS),
        front: Some(sides.front),
        back: sides.back,
        extra: BTreeMap::new(),
    };
    check_line(map.format, family, &line)?;
    Ok(line)
}

/// `add_linedef`'s step: resolve endpoints and sides, refuse a duplicate,
/// append the linedef and report its row.
pub(super) fn add_in_place(
    map: &mut Map,
    args: &AddLinedefParams,
    spec: &EndpointSpec<'_>,
) -> Result<AddRow, CyberError> {
    let ends = place_endpoints(map, spec)?;
    check_duplicate(map, ends.v1, ends.v2)?;
    let sides = resolve_sides(map, args)?;
    let line = build_line(map, args, &ends, &sides)?;
    let index = map.linedefs.len();
    let detail = edit_row(index, &line, map.flag_family());
    map.linedefs.push(line);
    Ok(AddRow {
        index: detail.index,
        detail: detail.detail,
        created_vertices: ends.created,
        created_sidedefs: sides.created,
    })
}

/// The `add_linedef` history label for the chosen endpoint form.
fn add_label(spec: &EndpointSpec<'_>) -> String {
    match *spec {
        EndpointSpec::Indices { v1, v2 } => format!("add linedef from vertex {v1} to {v2}"),
        EndpointSpec::Points { from, to } => format!(
            "add linedef from {}, {} to {}, {}",
            from.x, from.y, to.x, to.y
        ),
    }
}

/// `add_linedef`'s work: validate, resolve the map, run the one-step add.
pub(super) fn add_run(
    session: &mut Session,
    args: &AddLinedefParams,
) -> Result<AddReport, CyberError> {
    let spec = validate_add(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = add_label(&spec);
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args, &spec))?;
    Ok(AddReport {
        index: row.index,
        linedef: row.detail,
        created_vertices: row.created_vertices,
        created_sidedefs: row.created_sidedefs,
        edit: edit_result_fragment(&report),
    })
}
