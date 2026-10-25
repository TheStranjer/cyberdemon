//! `place_things`: the distribution run over the map.
//!
//! `place_things` runs the pure [`crate::wad::map::placement`]
//! distribution engine as one [`crate::session::Session::edit_map`]
//! step: the thing resolves through the same
//! [`crate::plan::things::resolve_thing_kind`] the plan builder uses,
//! the areas (sectors, a box, a circle) are laid on the shared clearance
//! grid and spread centre-outward, and every landed thing carries the
//! [`crate::plan::things::presence_flags`] of its `skills`/
//! `multiplayer_only` (`ambush` on top) and the snapped
//! [`crate::wad::map::placement::Facing`] of its `angle`. The wire
//! parsing lives in [`super::place_params`].
//!
//! [`place_request`]: super::place_params::place_request

use serde::Serialize;
use serde_json::{Map as JsonMap, Value};

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::plan::things::presence_flags;
use crate::session::{Scope, Session};
use crate::tools::edit_things::{check_binary_ranges, check_doom_plain};
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, edit_result_fragment};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::placement::{
    Area, DistributeSpec, Distribution, Facing, distribute, player_start, split_proportional,
    thing_clearance,
};
use crate::wad::map::query::check_indices;
use crate::wad::map::{Map, Thing};
use crate::wad::name::LumpName;

use super::invalid;
use super::place_params::{
    NeedSpec, PLACE_NEXT_STEPS, PLACE_UNKNOWN_RADIUS, PlaceRequest, PlaceThingsParams, WhereSpec,
    place_request, thing_bit,
};

/// What one [`place_on`] step wrote (or, in a dry run, would write).
#[derive(Debug)]
pub(super) struct PlacedStep {
    /// Indices of the thing rows the step appended.
    pub(super) indices: Vec<usize>,
    /// Where each landed, `[x, y]` pairs.
    pub(super) positions: Vec<[i32; 2]>,
    /// How many were asked for in total.
    pub(super) requested: usize,
    /// The shortfall notes.
    pub(super) warnings: Vec<String>,
}

impl PlacedStep {
    /// The tool's structured result for this step: `dry_run` says whether
    /// anything was written, `edit` carries the standard fragment.
    fn report(self, dry_run: bool, edit: JsonMap<String, Value>) -> PlaceReport {
        PlaceReport {
            placed: self.indices.len(),
            requested: self.requested,
            things: self.indices,
            positions: self.positions,
            warnings: self.warnings,
            next_steps: PLACE_NEXT_STEPS.as_slice(),
            dry_run,
            edit,
        }
    }
}

/// Structured result of [`Cyberdemon::place_things`].
#[derive(Serialize)]
pub(super) struct PlaceReport {
    /// How many things were (or, dry, would be) placed.
    placed: usize,
    /// How many were asked for in total.
    requested: usize,
    /// The indices of the placed thing rows, in placement order.
    things: Vec<usize>,
    /// Their positions as `[x, y]` pairs, same order.
    positions: Vec<[i32; 2]>,
    /// The shortfall notes (a fit-too-few is a warning, never an error).
    warnings: Vec<String>,
    /// What to look at next.
    next_steps: &'static [&'static str],
    /// Whether nothing changed because the call only reported.
    dry_run: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The areas a [`place_on`] call fills and each area's share of the
/// demand, parallel lists.
pub(super) type AreaPlan = (Vec<Area>, Vec<usize>);

/// The areas the request names, checked against `map`, and each area's
/// share of the demand.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a sector index the map does not hold.
pub(super) fn target_areas(map: &Map, request: &PlaceRequest) -> Result<AreaPlan, CyberError> {
    match request.target.clone() {
        WhereSpec::Sectors(list) => {
            check_sectors(map, &list)?;
            let areas: Vec<Area> = list.iter().map(|sector| Area::Sector(*sector)).collect();
            let needs = match request.demand {
                NeedSpec::Total(total) => split_proportional(map, &list, total),
                NeedSpec::PerSector(each) => vec![each; areas.len()],
            };
            Ok((areas, needs))
        }
        WhereSpec::Area(area) => {
            let need = match request.demand {
                NeedSpec::Total(total) => total,
                NeedSpec::PerSector(each) => each,
            };
            Ok((vec![area], vec![need]))
        }
    }
}

/// Refuse the first sector index `list` holds past the map's sectors,
/// through the shared [`check_indices`] guard.
///
/// [`check_indices`]: crate::wad::map::query::check_indices
pub(super) fn check_sectors(map: &Map, list: &[usize]) -> Result<(), CyberError> {
    check_indices(ElementKind::Sectors, list, map)
}

/// Run the whole placement over `map` and append the thing rows: one
/// undoable step's work, shared by the live and dry runs.
///
/// # Errors
///
/// [`CyberError::NotFound`] for an unknown sector,
/// [`CyberError::InvalidArgument`] for `angle: "face_player_start"` on a
/// map without a player 1 start or `Hexen`-only fields on a `Doom`-family
/// map, [`CyberError::LimitExceeded`] for a type or coordinate no map
/// format can store.
pub(super) fn place_on(
    map: &mut Map,
    request: &PlaceRequest,
    definitions: &Definitions,
) -> Result<PlacedStep, CyberError> {
    if request.facing == Facing::PlayerStart && player_start(map).is_none() {
        return Err(invalid(
            "the map holds no player 1 start (thing type 1): add one with add_thing or choose \
             another angle",
        ));
    }
    let (areas, needs) = target_areas(map, request)?;
    let requested = needs
        .iter()
        .fold(0_usize, |acc, need| acc.saturating_add(*need));
    let clearance = request
        .clearance
        .unwrap_or_else(|| thing_clearance(definitions, request.kind, PLACE_UNKNOWN_RADIUS));
    let spec = DistributeSpec {
        areas,
        needs,
        clearance,
        facing: request.facing,
        unknown_radius: PLACE_UNKNOWN_RADIUS,
    };
    let spread = distribute(map, definitions, &spec);
    write_places(map, request, &spread, requested)
}

/// Append the distributed things, range-checked against the map's format.
fn write_places(
    map: &mut Map,
    request: &PlaceRequest,
    spread: &Distribution,
    requested: usize,
) -> Result<PlacedStep, CyberError> {
    let family = map.flag_family();
    let format = map.format;
    let bits = presence_bits(request, family);
    let mut step = PlacedStep {
        indices: Vec::new(),
        positions: Vec::new(),
        requested,
        warnings: spread.warnings.clone(),
    };
    for spot in &spread.placed {
        let spawn = placed_thing(request, bits, spot)?;
        check_doom_plain(family, &spawn)?;
        check_binary_ranges(format, &spawn)?;
        step.indices.push(map.things.len());
        step.positions.push([spawn.x, spawn.y]);
        map.things.push(spawn);
    }
    Ok(step)
}

/// The presence flags of one placed thing: the plan-shared skill,
/// multiplayer and ambush bits for `family`.
pub(super) fn presence_bits(request: &PlaceRequest, family: FlagFamily) -> u32 {
    let mut bits = presence_flags(family, request.skills.as_deref(), request.multiplayer_only);
    if request.ambush {
        bits |= thing_bit(family, "ambush");
    }
    bits
}

/// The [`Thing`] row for one distributed spot, `Hexen` script fields and
/// all.
fn placed_thing(
    request: &PlaceRequest,
    bits: u32,
    spot: &crate::wad::map::placement::Placed,
) -> Result<Thing, CyberError> {
    let Ok(kind) = i32::try_from(request.kind) else {
        return Err(CyberError::LimitExceeded(format!(
            "thing type {} is above the storable thing-type range; use a doomednum the game \
             defines",
            request.kind
        )));
    };
    let Ok(map_x) = i32::try_from(spot.at.0) else {
        return Err(coordinate_refusal("x", spot.at.0));
    };
    let Ok(map_y) = i32::try_from(spot.at.1) else {
        return Err(coordinate_refusal("y", spot.at.1));
    };
    Ok(Thing {
        tid: request.tid,
        x: map_x,
        y: map_y,
        angle: spot.angle,
        kind,
        flags: bits,
        special: request.special,
        args: request.args,
        ..Default::default()
    })
}

/// The `LimitExceeded` for one placement coordinate no format holds.
fn coordinate_refusal(axis: &str, value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "{axis} {value} is outside the 32-bit map coordinate range; place inside the map's \
         coordinates"
    ))
}

/// The edit fragment of a dry run: the map it inspected, nothing written.
pub(super) fn dry_edit(name: LumpName) -> JsonMap<String, Value> {
    [
        ("map".to_owned(), Value::String(name.to_string())),
        ("nodes_invalidated".to_owned(), Value::Bool(false)),
        ("copied_to_working".to_owned(), Value::Bool(false)),
    ]
    .into_iter()
    .collect()
}

/// `place_things`' work: resolve the request, then run the whole
/// distribution as one [`crate::session::Session::edit_map`] step (a dry
/// run computes it over the merged view's copy and only reports).
pub(super) fn place_things_run(
    session: &mut Session,
    args: &PlaceThingsParams,
) -> Result<PlaceReport, CyberError> {
    let definitions = session.definitions()?;
    let request = place_request(args, &definitions)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    if args.dry_run.get() {
        let mut map = common::load_map(session, name, Scope::Merged)?;
        let step = place_on(&mut map, &request, &definitions)?;
        return Ok(step.report(true, dry_edit(name)));
    }
    let (step, report) = session.edit_map(name, "place things", |map| {
        place_on(map, &request, &definitions)
    })?;
    Ok(step.report(false, edit_result_fragment(&report)))
}
