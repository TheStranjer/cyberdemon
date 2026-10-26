//! The run behind [`Cyberdemon::build_map_from_plan`](super::Cyberdemon::build_map_from_plan):
//! the plan paths that name a compiler or feature offender, the session's
//! thing definitions the wire rows resolve against, the `finalize` steps
//! that grow the node run and the errors-only validation pass, and the
//! [`PlanReport`] the tool answers with, written into the working archive
//! as one [`Session::edit`] step by [`build_run`].
//!
//! The wire shape and its mapping onto [`crate::plan::model`] live in
//! [`super::params`]; the theme the run dresses with is resolved by
//! [`super::plan_theme`] in the group's root, beside the constants this
//! module reads.

use alloc::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::defs::Definitions;
use crate::error::{CyberError, PlanKind, PlanRef};
use crate::plan::CompiledPlan;
use crate::plan::compile;
use crate::plan::features::{FeatureRow, apply_features, playable_checks};
use crate::plan::things::{
    PLAYER_ONE_START, PlacementContext, PlannedThing, place, presence_flags, resolve_thing_kind,
};
use crate::plan::transform_plan;
use crate::session::Session;
use crate::tools::maps::{parse_format, resolve_namespace, write_block};
use crate::tools::nodes::{AllPlan, apply_all};
use crate::tools::plan::params::{BuildPlanParams, KindSource, ThingRequest};
use crate::tools::plan::{
    NEXT_STEPS, VALIDATION_ERROR_CAP, connection_label, feature_names, kind_name, plan_theme,
};
use crate::tools::validate::{Counts, resource_index, session_tables};
use crate::wad::classify::MapFormat;
use crate::wad::map::FlagFamily;
use crate::wad::map::Map;
use crate::wad::map::validate::{Issue, Outcome as ValidateOutcome, Severity, run as run_checks};
use crate::wad::map::write_map;
use crate::wad::name::LumpName;

/// The plan path naming `at`'s wire row: the row of the cited kind whose
/// id (or derived `from->to` connection label) matches, spelled `rooms[i]`,
/// `connections[i]` or `features[i]`. A reference naming an id the request
/// does not hold answers `None`, and its error keeps its own words.
pub(super) fn plan_path(args: &BuildPlanParams, at: &PlanRef) -> Option<String> {
    let name = at.id();
    match at.kind() {
        PlanKind::Room => args
            .rooms
            .iter()
            .position(|room| room.id == name)
            .map(|index| format!("rooms[{index}]")),
        PlanKind::Connection => args
            .connections
            .iter()
            .position(|connection| connection_label(connection) == name)
            .map(|index| format!("connections[{index}]")),
        PlanKind::Feature => args
            .features
            .iter()
            .position(|feature| {
                feature_names(feature)
                    .into_iter()
                    .flatten()
                    .any(|id| id == name)
            })
            .map(|index| format!("features[{index}]")),
    }
}

/// The compiler or feature error with its plan path prefixed, keeping the
/// code: only a refusal carrying a [`PlanRef`] is touched, and only an
/// argument or limit refusal — a contract break names its own offender.
/// An attached [`CyberError::with_hint`] hint rides along with the prefixed
/// rebuild.
pub(super) fn with_plan_path(error: CyberError, args: &BuildPlanParams) -> CyberError {
    let Some(at) = error.plan_ref() else {
        return error;
    };
    let Some(path) = plan_path(args, &at) else {
        return error;
    };
    if !error.is_invalid_argument() && !error.is_limit_exceeded() {
        return error;
    }
    error.map_message(|old| format!("{path}: {old}"))
}

/// The vertex bounds of the compiled map as one result object.
pub(super) fn bounds_of(map: &Map) -> Option<PlanBounds> {
    map.bounds()
        .map(|(x1, y1, x2, y2)| PlanBounds { x1, y1, x2, y2 })
}

/// The doomednum of a request row: its explicit `type`, the type its
/// `name` resolves to (refused as `invalid_argument` prefixed with the
/// wire path), or the player 1 start.
fn resolved_kind(request: &ThingRequest, definitions: &Definitions) -> Result<u32, CyberError> {
    match request.kind.clone() {
        KindSource::Type(kind) => Ok(kind),
        KindSource::PlayerOne => Ok(PLAYER_ONE_START),
        KindSource::Name(name) => resolve_thing_kind(definitions, &name).map_err(|problem| {
            CyberError::InvalidArgument(format!("{}: {problem}", request.path))
                .with_hint("get_thing_types with a search term")
        }),
    }
}

/// Every request row with its doomednum and presence flags resolved
/// against `definitions` and the compiled map's flag `family`.
pub(super) fn resolve_specs(
    requests: &[ThingRequest],
    family: FlagFamily,
    definitions: &Definitions,
) -> Result<Vec<PlannedThing>, CyberError> {
    requests
        .iter()
        .map(|request| {
            Ok(PlannedThing {
                kind: resolved_kind(request, definitions)?,
                spot: request.spot.clone(),
                count: request.count,
                angle: request.angle,
                flags: presence_flags(family, request.skills.as_deref(), request.multiplayer_only),
                path: request.path.clone(),
            })
        })
        .collect()
}

/// Refuse a plan whose resolved rows hold no player 1 start; the check
/// runs after name resolution, so a `type: 1` or a `name` answering to
/// type `1` satisfies it.
pub(super) fn ensure_player_start(specs: &[PlannedThing]) -> Result<(), CyberError> {
    if specs.iter().any(|spec| spec.kind == PLAYER_ONE_START) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(
        "a map needs a player 1 start; pass player_start or a thing of type 1".to_owned(),
    ))
}

/// Stand every planned thing in the compiled map: validate and resolve
/// the wire rows against `definitions`, refuse a plan without a player
/// start, run the pure [`place`] engine and extend the map's things and
/// the plan's warnings. The number of thing rows placed (the player
/// start included).
///
/// # Errors
///
/// [`super::params`]' shape refusals, the resolution refusals naming
/// `things[i]` or `player_start`, [`ensure_player_start`]' refusal and
/// [`place`]'s placement refusals.
fn place_planned(
    definitions: &Definitions,
    args: &BuildPlanParams,
    compiled: &mut CompiledPlan,
) -> Result<usize, CyberError> {
    let requests = args.to_things()?;
    let family = compiled.map.flag_family();
    let specs = resolve_specs(&requests, family, definitions)?;
    ensure_player_start(&specs)?;
    let context = PlacementContext {
        map: &compiled.map,
        room_sectors: &compiled.room_sectors,
        definitions,
        units: args.to_units(),
        grid: args.grid,
    };
    let placement = place(&context, &specs)?;
    compiled.warnings.extend(placement.warnings);
    let placed = placement.things.len();
    compiled.map.things.extend(placement.things);
    Ok(placed)
}

/// The finalized node build: the format's whole-set default plan
/// ([`AllPlan::for_model`] — `classic` plus a `zero` `REJECT` and a
/// fresh `BLOCKMAP` for a binary map, the compressed extended `ZNODES`
/// lump for a `UDMF` one) applied in place to the compiled model before
/// `write_map` turns the grown block into lumps, so the node run joins
/// the one `Session::edit` the write already is. The build summary as
/// `NodesBuildBody`-shaped `JSON`.
///
/// # Errors
///
/// [`apply_all`]'s refusals (a tree the builder cannot cut, limits the
/// record layouts refuse) and the summary's serialization.
pub(super) fn finalize_nodes(compiled: &mut CompiledPlan) -> Result<Value, CyberError> {
    let plan = AllPlan::for_model(&compiled.map);
    let built = apply_all(&mut compiled.map, &plan)?;
    built.into_nodes_json()
}

/// The finalized validation's result frame: the `Severity::Error` rows
/// of `outcome` capped at [`VALIDATION_ERROR_CAP`], with the
/// per-severity counts over every issue the checks found.
pub(super) fn validation_view(outcome: &ValidateOutcome) -> Value {
    let counts = Counts::of(&outcome.issues);
    let errors: Vec<&Issue> = outcome
        .issues
        .iter()
        .filter(|issue| issue.severity == Severity::Error)
        .take(VALIDATION_ERROR_CAP)
        .collect();
    serde_json::json!({"errors": errors, "counts": counts})
}

/// The finalized validation: every `validate_map` check over the
/// finished model, with the session's definition tables and the merged
/// view's resource index exactly as [`crate::tools::validate`] reads
/// them, reported through [`validation_view`]. Findings never fail the
/// call: the map was built to plan, and the caller fixes error rows
/// with the element tools.
///
/// # Errors
///
/// [`session_tables`]' and [`resource_index`]' read errors.
fn finalize_validation(session: &Session, map: &Map) -> Result<Value, CyberError> {
    let tables = session_tables(session)?;
    let resources = resource_index(session)?;
    let outcome = run_checks(map, &tables.context(&resources), None);
    Ok(validation_view(&outcome))
}

/// The bounding box of the compiled map's vertices.
#[derive(Debug, PartialEq, Serialize)]
pub(super) struct PlanBounds {
    /// Smallest vertex `X`.
    x1: i32,
    /// Smallest vertex `Y`.
    y1: i32,
    /// Largest vertex `X`.
    x2: i32,
    /// Largest vertex `Y`.
    y2: i32,
}

/// One placed connection as the result reports it.
#[derive(Serialize)]
pub(super) struct ConnectionReport {
    /// The connection's id, or its `from->to` label when unnamed.
    id: String,
    /// The kind class: `opening`, `door`, `window` or `stairs`.
    kind: &'static str,
    /// The sectors this connector owns (one band, or one per stair
    /// step), in plan order. Empty for an `opening` whose width spans
    /// the whole shared edge: no wall stands there, so the rooms join on
    /// their own boundary line and the opening owns no sector.
    sectors: Vec<usize>,
}

/// Structured result of [`Cyberdemon::build_map_from_plan`].
#[derive(Serialize)]
pub(super) struct PlanReport {
    /// The map block that was written.
    map: LumpName,
    /// The layout its block uses.
    format: MapFormat,
    /// Room id to the sector indices that room owns; a room fully
    /// painted over by a later one holds no entry.
    rooms: BTreeMap<String, Vec<usize>>,
    /// One row per connection, in plan order.
    connections: Vec<ConnectionReport>,
    /// One row per installed game feature, in pipeline order: doors
    /// first, in connection order, each carrying the band `sectors`,
    /// the room-facing `lines` (a remote's stay actionless: its tagged
    /// sector is the handle) and the shared `tag`; then switch rows,
    /// lift rows (the room's platform sector, its activation lines and
    /// the fresh tag), teleporter rows (pad and destination sectors,
    /// the pad's border lines and the destination tag), exit rows, and
    /// the light and secret rows naming the room sectors they painted
    /// (no lines, no tag). Each row's `line_ends` repeats its `lines`
    /// with the map-unit endpoints the finished map holds them at, so a
    /// wall-mounted panel can be checked against the plan that asked for
    /// it.
    features: Vec<FeatureRow>,
    /// The thing rows standing in the written map, player start
    /// included.
    things_placed: usize,
    /// The vertex bounding box; `null` for a map without vertices.
    bounds: Option<PlanBounds>,
    /// The compiler's advisories (unreachable and painted-over rooms,
    /// too-tall floor gaps), the notes for theme roles that answered
    /// `"-"`, the feature steps' notes and the playability checks (a
    /// locked door with no key of its colour placed, no `exit` feature,
    /// a `remote` door no `switch` row opens) — never errors.
    warnings: Vec<String>,
    /// The finalized node build's summary (`NodesBuildBody`-shaped:
    /// `variant`, `segs`, `subsectors`, `nodes` and the layout's extra
    /// counts); `null` when `finalize` was not asked for.
    nodes: Option<Value>,
    /// The finalized errors-only validation pass
    /// (`{"errors": [...], "counts": {...}}`); `null` when `finalize`
    /// was not asked for. Error rows never rolled the build back.
    validation: Option<Value>,
    /// What to look at next.
    next_steps: [&'static str; 2],
}

/// The result frame for a compiled plan, before the archive write: the
/// live counts, the installed [`FeatureRow`]s and whatever the
/// `finalize` step produced (`finished` is `None` without it, and then
/// `nodes` and `validation` report `null`).
pub(super) fn reporting(
    compiled: &CompiledPlan,
    things_placed: usize,
    features: Vec<FeatureRow>,
    finished: Option<(Value, Value)>,
) -> PlanReport {
    let (nodes, validation) = match finished {
        Some((built, checked)) => (Some(built), Some(checked)),
        None => (None, None),
    };
    PlanReport {
        map: compiled.map.name,
        format: compiled.map.format,
        rooms: compiled.room_sectors.clone(),
        connections: compiled
            .connector_sectors
            .iter()
            .map(|info| ConnectionReport {
                id: info.id.clone(),
                kind: kind_name(info.kind),
                sectors: info.sectors.clone(),
            })
            .collect(),
        features,
        things_placed,
        bounds: bounds_of(&compiled.map),
        warnings: compiled.warnings.clone(),
        nodes,
        validation,
        next_steps: NEXT_STEPS,
    }
}

/// `build_map_from_plan`'s work: resolve the parameters, carry the
/// drawn plan into map units, compile the plan, install its features,
/// stand the things in, append the advisory [`playable_checks`] notes,
/// finalize when asked, and write the block as one [`Session::edit`]
/// step, making the map the session's active one.
pub(super) fn build_run(
    session: &mut Session,
    args: &BuildPlanParams,
) -> Result<PlanReport, CyberError> {
    let name = LumpName::parse(args.map.as_str())?;
    let format = parse_format(args.format.as_deref())?;
    let namespace = if format == MapFormat::Udmf {
        resolve_namespace(args.namespace.as_deref(), format)?
    } else {
        None
    };
    let drawn = args.to_model()?;
    let units = args.to_units();
    units.check()?;
    let plan = transform_plan(&drawn, &units, args.grid)?;
    let theme = plan_theme(session, args.theme.as_deref())?;
    let mut compiled =
        compile(&plan, name, format, &theme.plan).map_err(|error| with_plan_path(error, args))?;
    compiled.map.udmf_namespace = namespace;
    compiled.warnings.extend(theme.notes);
    let features = apply_features(&plan, &mut compiled, &theme.features)
        .map_err(|error| with_plan_path(error, args))?;
    let definitions = session.definitions()?;
    let things_placed = place_planned(&definitions, args, &mut compiled)?;
    compiled
        .warnings
        .extend(playable_checks(&plan, &compiled.map, &definitions));
    let finished = if args.finalize {
        let built = finalize_nodes(&mut compiled)?;
        let checked = finalize_validation(session, &compiled.map)?;
        Some((built, checked))
    } else {
        None
    };
    let report = reporting(&compiled, things_placed, features, finished);
    let lumps = write_map(&compiled.map)?;
    let label = format!("build_map_from_plan {name}");
    let replace = args.replace;
    session.edit(&label, move |wad| write_block(wad, name, replace, lumps))?;
    session.set_active_map(Some(name));
    Ok(report)
}
