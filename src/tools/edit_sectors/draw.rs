//! `create_sector_from_loop` and `carve_void`: their arguments, report
//! shapes and the validated requests each one-step edit hands to the pure
//! overlay engine [`crate::wad::map::overlay::apply_overlay`].
//!
//! The [`crate::server::Cyberdemon`] router methods hand their parsed
//! [`CreateSectorFromLoopParams`] and [`CarveVoidParams`] to [`create_run`]
//! and [`carve_run`], which resolve the map (the carve also resolving the
//! theme wall through [`theme_wall`]) and paint inside exactly one
//! [`crate::session::Session::edit_map`] step. [`build_request`] folds a
//! room out of the polygon [`polygon_of`] reads in either of its two forms,
//! the sector [`loop_sector`] describes under the parent's [`check_sector`],
//! the three wall faces [`wall_faces`] defaults and the outer rule
//! [`outer_sector_of`] parses; [`build_void_request`] folds a carve out of
//! the same polygon with [`crate::wad::map::overlay::Paint::Void`] and one
//! texture on every face.

use alloc::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::geom::Pt;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{PointParam, edit_result_fragment, validate_texture_name};
use crate::tools::themes::theme_texture;
use crate::wad::classify::MapFormat;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, OverlayResult, Paint, WallTextures};
use crate::wad::map::query::{ElementKind, missing_index};
use crate::wad::map::{Map, Sector};

use super::{
    COORD_MAX, COORD_MIN, DEFAULT_CEILING_FLAT, DEFAULT_CEILING_HEIGHT, DEFAULT_FLOOR_FLAT,
    DEFAULT_FLOOR_HEIGHT, DEFAULT_LIGHT, DEFAULT_WALL_TEXTURE, NO_TEXTURE, OUTER_AUTO, OUTER_NONE,
    check_sector, flat_or_default,
};

/// Arguments for [`Cyberdemon::create_sector_from_loop`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CreateSectorFromLoopParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The polygon's corners as points `{x, y}` (at least 3, implicitly
    /// closed): exactly one of `points` and `vertices`.
    #[serde(default)]
    pub(super) points: Option<Vec<PointParam>>,
    /// The polygon's corners as existing vertex indices (at least 3):
    /// exactly one of `points` and `vertices`.
    #[serde(default)]
    pub(super) vertices: Option<Vec<usize>>,
    /// Floor height in map units (default `0`).
    #[serde(default)]
    floor_height: Option<i32>,
    /// Ceiling height in map units (default `128`).
    #[serde(default)]
    ceiling_height: Option<i32>,
    /// Floor flat name (default `"FLOOR4_8"`).
    #[serde(default)]
    floor_flat: Option<String>,
    /// Ceiling flat name (default `"CEIL3_5"`).
    #[serde(default)]
    ceiling_flat: Option<String>,
    /// Light level, `0..=255` (default `160`).
    #[serde(default)]
    light: Option<i32>,
    /// Sector effect number (default `0`).
    #[serde(default)]
    special: Option<i32>,
    /// Tag matched by linedef actions (default `0`).
    #[serde(default)]
    tag: Option<i32>,
    /// Middle texture of new one-sided walls (default `"STARTAN3"`).
    #[serde(default)]
    wall_texture: Option<String>,
    /// Upper texture of new two-sided transitions (default: `wall_texture`).
    #[serde(default)]
    upper_texture: Option<String>,
    /// Lower texture of new two-sided transitions (default: `wall_texture`).
    #[serde(default)]
    lower_texture: Option<String>,
    /// The sector across the new walls: an existing sector index, `"auto"`
    /// (default: the sector found just outside each edge on the original
    /// map), or `"none"` (new walls stay one-sided).
    #[serde(default)]
    pub(super) outer_sector: Option<OuterSectorParam>,
    /// Keep existing sectors that sit strictly inside the polygon (a
    /// pillar, an inner room): only the area of the sectors the polygon's
    /// boundary touches is repainted (default `false`: everything inside
    /// is overwritten).
    #[serde(default)]
    keep_interior: bool,
}

/// Arguments for [`Cyberdemon::carve_void`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CarveVoidParams {
    /// The map to edit; defaults like [`CreateSectorFromLoopParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The polygon's corners as points `{x, y}` (at least 3, implicitly
    /// closed): exactly one of `points` and `vertices`.
    #[serde(default)]
    points: Option<Vec<PointParam>>,
    /// The polygon's corners as existing vertex indices (at least 3):
    /// exactly one of `points` and `vertices`.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
}

/// The `outer_sector` parameter: an existing sector index or a mode name.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum OuterSectorParam {
    /// Face every new wall's outer side at this existing sector.
    Index(
        /// The sector index.
        usize,
    ),
    /// `"auto"` (the default) or `"none"`.
    Mode(
        /// The mode name.
        String,
    ),
}

/// Structured result of [`Cyberdemon::create_sector_from_loop`].
#[derive(Serialize)]
pub(super) struct LoopToolReport {
    /// What the overlay engine built, inlined next to the edit fragment.
    #[serde(flatten)]
    result: OverlayResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::carve_void`].
#[derive(Serialize)]
pub(super) struct CarveToolReport {
    /// What the overlay engine built, inlined next to the fragment.
    #[serde(flatten)]
    result: OverlayResult,
    /// Indices of the sectors the carve left without a single sidedef
    /// (the same list `empty_sectors` reports; they stay in the map).
    removed_sectors_empty: Vec<usize>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The refusal for a call giving the polygon in both forms or in neither.
fn loop_form_refusal() -> CyberError {
    CyberError::InvalidArgument(
        "the polygon needs its corners in exactly one form: points [{x, y}, ...] \
         (at least 3 corners, implicitly closed) or vertices [index, ...] (at least 3 \
         existing vertex indices)"
            .to_owned(),
    )
}

/// The polygon's corners the two optional parameters give, in exactly
/// one of the two forms: points checked against the map's format, or
/// existing vertex indices resolved to their coordinates.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when both `points` and `vertices` or
/// neither is passed, or when a point coordinate leaves the binary `i16`
/// range; [`CyberError::NotFound`] for a vertex index the map does not
/// hold.
pub(super) fn polygon_of(
    points: Option<&Vec<PointParam>>,
    vertices: Option<&Vec<usize>>,
    map: &Map,
) -> Result<Vec<Pt>, CyberError> {
    match (points, vertices) {
        (Some(list), None) => points_of(map.format, list),
        (None, Some(indices)) => vertices_of(map, indices),
        _ => Err(loop_form_refusal()),
    }
}

/// The point form: coordinates checked for the format, widened to `i64`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] through [`check_points`] when a
/// coordinate leaves the binary `i16` range.
fn points_of(format: MapFormat, points: &[PointParam]) -> Result<Vec<Pt>, CyberError> {
    if format != MapFormat::Udmf {
        check_points(points)?;
    }
    Ok(points
        .iter()
        .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
        .collect())
}

/// The index form: every corner resolved to its vertex' coordinates,
/// through the shared [`check_index`] guard with the loop's own advice
/// kept as message context.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the first index past [`Map::vertices`].
///
/// [`check_index`]: crate::tools::maps::params::check_index
fn vertices_of(map: &Map, indices: &[usize]) -> Result<Vec<Pt>, CyberError> {
    let mut corners: Vec<Pt> = Vec::with_capacity(indices.len());
    for index in indices {
        let vertex = map.vertices.get(*index).ok_or_else(|| {
            missing_index(ElementKind::Vertices, *index, map.vertices.len()).map_message(
                |message| format!("{message}: pass existing indices, or draw with points"),
            )
        })?;
        corners.push((i64::from(vertex.x), i64::from(vertex.y)));
    }
    Ok(corners)
}

/// Refuses every point corner an `i16` binary `VERTEXES` column cannot hold.
fn check_points(points: &[PointParam]) -> Result<(), CyberError> {
    for corner in points {
        fits_coord("x", corner.x)?;
        fits_coord("y", corner.y)?;
    }
    Ok(())
}

/// Refuses a loop coordinate an `i16` binary `VERTEXES` column cannot hold.
fn fits_coord(field: &str, value: i32) -> Result<(), CyberError> {
    if (COORD_MIN..=COORD_MAX).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "the loop point {field} {value} is out of range for the binary map format: \
         coordinates hold {COORD_MIN}..={COORD_MAX}"
    )))
}

/// The sector the loop parameters describe, checked like `add_sector`'s.
fn loop_sector(args: &CreateSectorFromLoopParams, format: MapFormat) -> Result<Sector, CyberError> {
    let sector = Sector {
        floor_height: args.floor_height.unwrap_or(DEFAULT_FLOOR_HEIGHT),
        ceiling_height: args.ceiling_height.unwrap_or(DEFAULT_CEILING_HEIGHT),
        floor_flat: flat_or_default(args.floor_flat.as_ref(), DEFAULT_FLOOR_FLAT, format)?,
        ceiling_flat: flat_or_default(args.ceiling_flat.as_ref(), DEFAULT_CEILING_FLAT, format)?,
        light: args.light.unwrap_or(DEFAULT_LIGHT),
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        extra: BTreeMap::new(),
    };
    check_sector(format, &sector)?;
    Ok(sector)
}

/// A texture parameter validated and uppercased, `fallback` when absent.
fn texture_or_default(
    given: Option<&String>,
    fallback: &str,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(fallback.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// The three wall face textures: the one-sided wall and the two transition
/// faces, each defaulting to `wall_texture`.
fn wall_faces(
    args: &CreateSectorFromLoopParams,
    format: MapFormat,
) -> Result<(String, String, String), CyberError> {
    let wall = texture_or_default(args.wall_texture.as_ref(), DEFAULT_WALL_TEXTURE, format)?;
    let upper = args.upper_texture.as_ref().map_or_else(
        || Ok(wall.clone()),
        |name| validate_texture_name(name, format),
    )?;
    let lower = args.lower_texture.as_ref().map_or_else(
        || Ok(wall.clone()),
        |name| validate_texture_name(name, format),
    )?;
    Ok((wall, upper, lower))
}

/// The outer rule the `outer_sector` parameter gives, `"auto"` when absent.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a mode name other than `"auto"` or
/// `"none"`.
pub(super) fn outer_sector_of(given: Option<OuterSectorParam>) -> Result<Outside, CyberError> {
    match given {
        None => Ok(Outside::Original),
        Some(OuterSectorParam::Index(index)) => Ok(Outside::Face(index)),
        Some(OuterSectorParam::Mode(text)) => match text.to_ascii_lowercase().as_str() {
            OUTER_AUTO => Ok(Outside::Original),
            OUTER_NONE => Ok(Outside::Solid),
            other => Err(CyberError::InvalidArgument(format!(
                "outer_sector {other:?} is not a mode: pass an existing sector index, \
                 \"{OUTER_AUTO}\" to open each new wall onto the sector found outside it, \
                 or \"{OUTER_NONE}\" to keep new walls one-sided"
            ))),
        },
    }
}

/// The validated [`OverlayRequest`] the parameters and the map describe.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad corner form, an out-of-range
/// coordinate, a sector the format cannot store or a bad texture/mode;
/// [`CyberError::NotFound`] for an unknown vertex or outer-sector index.
pub(super) fn build_request(
    args: &CreateSectorFromLoopParams,
    map: &Map,
) -> Result<OverlayRequest, CyberError> {
    let (wall, upper, lower) = wall_faces(args, map.format)?;
    Ok(OverlayRequest {
        polygon: polygon_of(args.points.as_ref(), args.vertices.as_ref(), map)?,
        paint: Paint::Sector(loop_sector(args, map.format)?),
        keep_interior: args.keep_interior,
        textures: WallTextures { wall, upper, lower },
        outside: outer_sector_of(args.outer_sector.clone())?,
    })
}

/// The [`OverlayRequest`] the `carve_void` parameters and the map
/// describe: solid paint, the theme's wall texture standing in for walls
/// the surrounding sectors cannot dress from their own.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad corner form or an
/// out-of-range coordinate; [`CyberError::NotFound`] for an unknown
/// vertex index.
pub(super) fn build_void_request(
    args: &CarveVoidParams,
    map: &Map,
    wall: &str,
) -> Result<OverlayRequest, CyberError> {
    Ok(OverlayRequest {
        polygon: polygon_of(args.points.as_ref(), args.vertices.as_ref(), map)?,
        paint: Paint::Void,
        keep_interior: false,
        textures: WallTextures {
            wall: wall.to_owned(),
            upper: wall.to_owned(),
            lower: wall.to_owned(),
        },
        outside: Outside::Original,
    })
}

/// `create_sector_from_loop`'s work: resolve the map, then run the
/// one-step painting on it.
pub(super) fn create_run(
    session: &mut Session,
    args: &CreateSectorFromLoopParams,
) -> Result<LoopToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = "create sector from loop".to_owned();
    let (result, report) = session.edit_map(name, &label, |map| {
        let request = build_request(args, map)?;
        overlay::apply_overlay(map, &request)
    })?;
    Ok(LoopToolReport {
        result,
        edit: edit_result_fragment(&report),
    })
}

/// The wall texture a carve dresses bare sectors with: the session
/// theme's `Wall`, else the `create_sector_from_loop` default.
fn theme_wall(session: &Session) -> Result<String, CyberError> {
    let themed = theme_texture(session, Role::Wall)?;
    if themed == NO_TEXTURE {
        return Ok(DEFAULT_WALL_TEXTURE.to_owned());
    }
    Ok(themed)
}

/// `carve_void`'s work: resolve the map and the theme wall, then run the
/// one-step carving on it.
pub(super) fn carve_run(
    session: &mut Session,
    args: &CarveVoidParams,
) -> Result<CarveToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let wall = theme_wall(session)?;
    let label = "carve void".to_owned();
    let (result, report) = session.edit_map(name, &label, |map| {
        let request = build_void_request(args, map, &wall)?;
        overlay::apply_overlay(map, &request)
    })?;
    Ok(CarveToolReport {
        removed_sectors_empty: result.empty_sectors.clone(),
        result,
        edit: edit_result_fragment(&report),
    })
}
