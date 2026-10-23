//! `transform_selection`'s parameters, request types and working step:
//! one scale/rotate/translate over a mixed vertex-and-thing selection.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`TransformSelectionParams`] to [`transform_run`], which builds the
//! engine's [`Operations`] through [`operations_of`] (and [`rotation_of`])
//! and its [`SelectionRequest`] through [`selection_request`], expands the
//! selection against the map as it stands and runs the whole transform as
//! one [`crate::session::Session::edit_map`] step.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::wad::map::transform::{
    Operations, Point, Rotation, Scale, SelectionRequest, TransformReport, apply_transform,
    expand_selection,
};

use super::current_map;

/// The `selection` parameter of
/// [`Cyberdemon::transform_selection`](crate::server::Cyberdemon): the
/// index lists and expansion switches, before anything is resolved
/// against a map.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SelectionInput {
    /// Indices into the map's vertex block to move.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
    /// Indices into the map's linedef block; each drags in its two
    /// endpoints.
    #[serde(default)]
    linedefs: Option<Vec<usize>>,
    /// Indices into the map's sector block; each drags in the vertices of
    /// all its boundary linedefs and, by default, its things.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// Indices into the map's thing block to move.
    #[serde(default)]
    things: Option<Vec<usize>>,
    /// Select every vertex and thing of the map; with it set the index
    /// lists add nothing (default `false`).
    #[serde(default)]
    all: Option<bool>,
    /// Whether things standing in a listed sector join the selection,
    /// judged before anything moves (default `true`).
    #[serde(default)]
    include_things_in_sectors: Option<bool>,
}

/// The `scale` parameter: the exact fraction `num / den` (both above
/// zero), e.g. 3/2 for 150%.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ScaleInput {
    /// Numerator; above zero.
    num: i32,
    /// Denominator; above zero.
    den: i32,
}

/// The `rotate` parameter: exactly one of the two fields,
/// counter-clockwise.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RotateInput {
    /// Whole degrees; give exactly one of this and `millidegrees`.
    #[serde(default)]
    degrees: Option<i64>,
    /// Thousandths of a degree; give exactly one of this and `degrees`.
    /// Thing facings take the nearest whole degree.
    #[serde(default)]
    millidegrees: Option<i64>,
}

/// The `translate` parameter: the whole-unit shift applied last.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TranslateInput {
    /// `X` shift in map units.
    dx: i32,
    /// `Y` shift in map units.
    dy: i32,
}

/// A bare `{x, y}` point parameter (`origin`).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct PointInput {
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
}

impl From<&PointInput> for Point {
    #[inline]
    fn from(input: &PointInput) -> Self {
        Self {
            x: i64::from(input.x),
            y: i64::from(input.y),
        }
    }
}

/// Arguments for [`Cyberdemon::transform_selection`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TransformSelectionParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Which elements to move (expanded into vertices and things).
    pub(super) selection: SelectionInput,
    /// Exact-fraction scaling about the origin, applied first.
    #[serde(default)]
    pub(super) scale: Option<ScaleInput>,
    /// Counter-clockwise rotation about the origin, applied second.
    #[serde(default)]
    pub(super) rotate: Option<RotateInput>,
    /// Whole-unit shift, applied last.
    #[serde(default)]
    pub(super) translate: Option<TranslateInput>,
    /// The pivot of `scale` and `rotate`; defaults to the centre of the
    /// selection's bounding box, rounded to nearest.
    #[serde(default)]
    pub(super) origin: Option<PointInput>,
}

/// Structured result of
/// [`Cyberdemon::transform_selection`](crate::server::Cyberdemon).
#[derive(Serialize)]
pub(super) struct TransformOutcome {
    /// What the transform engine did.
    #[serde(flatten)]
    transform: TransformReport,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The `selection` parameter as the engine's expansion request, defaults
/// applied.
pub(super) fn selection_request(input: &SelectionInput) -> SelectionRequest {
    SelectionRequest {
        vertices: input.vertices.clone().unwrap_or_default(),
        linedefs: input.linedefs.clone().unwrap_or_default(),
        sectors: input.sectors.clone().unwrap_or_default(),
        things: input.things.clone().unwrap_or_default(),
        all: input.all.unwrap_or(false),
        include_things_in_sectors: input.include_things_in_sectors.unwrap_or(true),
    }
}

/// The `rotate` parameter as a rotation; it must carry exactly one of
/// `degrees` and `millidegrees`.
pub(super) fn rotation_of(input: &RotateInput) -> Result<Rotation, CyberError> {
    match (input.degrees, input.millidegrees) {
        (Some(_), Some(_)) | (None, None) => Err(CyberError::InvalidArgument(
            "rotate needs exactly one of degrees and millidegrees; pass one number and drop \
             the other"
                .to_owned(),
        )),
        (Some(degrees), None) => Ok(Rotation::from_degrees(degrees)),
        (None, Some(millidegrees)) => Ok(Rotation::from_millidegrees(millidegrees)),
    }
}

/// The call's operations, validated and refusing the empty set: a
/// transform with nothing to do is never what the caller meant.
pub(super) fn operations_of(args: &TransformSelectionParams) -> Result<Operations, CyberError> {
    let ops = Operations {
        scale: args
            .scale
            .as_ref()
            .map(|input| Scale::new(input.num, input.den))
            .transpose()?,
        rotation: args.rotate.as_ref().map(rotation_of).transpose()?,
        translate: args.translate.as_ref().map(|input| Point {
            x: i64::from(input.dx),
            y: i64::from(input.dy),
        }),
    };
    if ops.is_empty() {
        return Err(CyberError::InvalidArgument(
            "no operation: pass at least one of scale, rotate and translate".to_owned(),
        ));
    }
    Ok(ops)
}

/// `n` and its noun, singular at one and plural otherwise (`"vertex"`
/// pluralises irregularly, so both forms are given).
pub(super) fn counted(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// `transform_selection`'s work: validate the operations, expand the
/// selection against the map as it now stands (an early, side-effect-free
/// refusal of bad indices and empty expansions), then run the whole
/// transform as one undoable step.
pub(super) fn transform_run(
    session: &mut Session,
    args: &TransformSelectionParams,
) -> Result<TransformOutcome, CyberError> {
    let ops = operations_of(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let request = selection_request(&args.selection);
    let pivot = args.origin.as_ref().map(Point::from);
    let expanded = expand_selection(&current_map(session, name)?, &request)?;
    let label = format!(
        "transform {} and {}",
        counted(expanded.vertices.len(), "vertex", "vertices"),
        counted(expanded.things.len(), "thing", "things")
    );
    let (transform, report) = session.edit_map(name, &label, |map| {
        let moved = expand_selection(map, &request)?;
        apply_transform(map, &moved, &ops, pivot)
    })?;
    Ok(TransformOutcome {
        transform,
        edit: edit_result_fragment(&report),
    })
}
