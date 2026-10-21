//! Plan tools: `build_map_from_plan` (see the `plan` router on
//! [`crate::server::Cyberdemon`]).
//!
//! One declarative call: rooms with footprints and sector properties,
//! connections joining them, a wall thickness and a theme. The wire
//! shape and its mapping onto [`crate::plan::model`] live in
//! [`params`]; the pure [`crate::plan::compile`] engine grows the thick
//! walls, places every connector band, emits the fresh [`Map`] and
//! dresses it in the [`ThemeRoles`] this module resolves for the call.
//! The [`crate::plan::things`] engine then stands the requested things
//! in: names resolve against the session's game definitions (the same
//! overlay the validate tools read), pinned points travel with the
//! rooms through [`crate::plan::units`], room fills land row-major on
//! the clearance grid, and every built map keeps a player 1 start. The
//! [`crate::plan::features`] step runs right after the compile and
//! installs the plan's game structures in fixed pipeline order —
//! doors, switches, lifts, teleporters, exits, lights, secrets: door
//! connectors take a fresh tag on their band sector and the style's
//! linedef specials on their room-facing lines (locked doors re-dress
//! their jambs in the theme's coloured track), `switch` rows mount
//! panels firing a remote door's tag, `lift` rows dress the room's own
//! sector as a tagged platform whose lower-facing edges carry the ride,
//! `teleporter` rows stand the pad and destination squares (with their
//! own type-14 things), `exit` rows end the level, and the room
//! `light_effect` and `secret` flags paint the family's sector
//! specials — lights before secrets, so a lit secret keeps both. Every
//! installed feature is reported as a `features` row. After the things
//! stand in, [`crate::plan::playable_checks`] adds its advisory warnings
//! — a locked door whose key colour stands nowhere among the placed
//! things, a plan with no `exit` feature and a remote door no `switch`
//! row opens — never errors, in plan order.
//! The compiled block is written into the working archive as exactly
//! one [`Session::edit`] step through
//! [`crate::tools::maps::write_block`], and the map becomes the
//! session's active map. The `finalize` parameter grows that one step:
//! before the write it runs the [`crate::tools::nodes`] whole-set build
//! (`SEGS`/`SSECTORS`/`NODES` plus `REJECT` and `BLOCKMAP` on a binary
//! map, the extended `ZNODES` lump on a `UDMF` one) on the compiled
//! model, and it reports an errors-only pass of
//! [`crate::wad::map::validate`] over the finished map — validation
//! findings never roll the build back; by then the doors are wired, so
//! the pass judges the finished specials and tags. Before the
//! compiler, the pure
//! [`crate::plan::units`] transform carries the
//! drawn plan into map units: the `units` scale/origin/mirror and the
//! `grid` snap, so a sketch measured in pixels lands grid-aligned at
//! map size.
//!
//! Compiler refusals already name the offending id;
//! [`with_plan_path`] prefixes them with the plan path (`rooms[0]`,
//! `connections[1]`, `features[2]`) so the caller can point at the wire
//! element that broke.

pub(in crate::tools) mod params;

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use serde::Serialize;
use serde_json::Value;

use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::plan::CompiledPlan;
use crate::plan::compile;
use crate::plan::features::{FeatureDress, FeatureRow, apply_features, playable_checks};
use crate::plan::model::ConnectionKind;
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::plan::things::{
    PLAYER_ONE_START, PlacementContext, PlannedThing, place, presence_flags, resolve_thing_kind,
};
use crate::plan::transform_plan;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::defs::collect;
use crate::tools::maps::{parse_format, resolve_namespace, write_block};
use crate::tools::nodes::{AllPlan, apply_all};
use crate::tools::plan::params::{
    BuildPlanParams, ConnectionParam, FeatureParam, KindSource, ThingRequest,
};
use crate::tools::themes::theme_roles;
use crate::tools::validate::{Counts, resource_index, session_tables};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::FlagFamily;
use crate::wad::map::Map;
use crate::wad::map::validate::{Issue, Outcome as ValidateOutcome, Severity, run as run_checks};
use crate::wad::map::write_map;
use crate::wad::name::LumpName;

/// The texture name meaning "nothing here" in a resolved theme.
const NO_TEXTURE: &str = "-";
/// Error rows the finalized result lists at most; the counts stay exact.
const VALIDATION_ERROR_CAP: usize = 20_usize;
/// The steps every result hands the caller.
const NEXT_STEPS: [&str; 2] = [
    "render_topdown to compare with the sketch",
    "render_view from the player start",
];
/// Every plan dressing role and the session theme role answering it.
const THEME_ROLES: [(TextureRole, Role); 8] = [
    (TextureRole::Floor, Role::Floor),
    (TextureRole::Ceiling, Role::Ceiling),
    (TextureRole::Wall, Role::Wall),
    (TextureRole::Door, Role::Door),
    (TextureRole::DoorFloor, Role::DoorFloor),
    (TextureRole::DoorTrack, Role::DoorTrack),
    (TextureRole::WindowSill, Role::WindowSill),
    (TextureRole::Step, Role::Step),
];
/// The theme roles the feature step dresses on top of the eight
/// compiler roles: the coloured tracks of the locked door styles and
/// the switch and exit-switch panels.
const FEATURE_ROLES: [Role; 5] = [
    Role::DoorTrackRed,
    Role::DoorTrackBlue,
    Role::DoorTrackYellow,
    Role::Switch,
    Role::ExitSwitch,
];

/// The resolved [`TextureRole`] names one [`crate::plan::compile`] call
/// dresses with: a role the theme answered `"-"` holds no name, so the
/// compiler leaves its faces undressed instead of inventing one.
struct PlanTheme {
    /// The (role, name) pairs the theme answered, in [`THEME_ROLES`]
    /// order.
    names: Vec<(TextureRole, String)>,
}

impl TextureRoles for PlanTheme {
    fn texture(&self, role: TextureRole) -> Option<&str> {
        self.names
            .iter()
            .find_map(|pair| (pair.0 == role).then_some(pair.1.as_str()))
    }
}

/// What one call's theme resolution hands the build: the compiler's
/// [`PlanTheme`], the [`crate::plan::features`] step's [`FeatureDress`]
/// and the notes for the roles that answered `"-"`.
struct ResolvedTheme {
    /// The eight dressing names for the compiler.
    plan: PlanTheme,
    /// The door and track names for the feature step.
    features: FeatureDress,
    /// The advisory notes, dressing roles first, coloured tracks after.
    notes: Vec<String>,
}

/// The compiler's theme for this call, the [`crate::plan::features`]
/// step's dressed names, and the notes for the roles that answered
/// `"-"`.
///
/// # Errors
///
/// [`theme_roles`]' refusals for an unknown `theme` override.
fn plan_theme(session: &Session, theme: Option<&str>) -> Result<ResolvedTheme, CyberError> {
    let mut wanted: Vec<Role> = THEME_ROLES.iter().map(|&(_, role)| role).collect();
    wanted.extend(FEATURE_ROLES);
    let resolved = theme_roles(session, theme, &wanted)?;
    let mut names = Vec::new();
    let mut notes = Vec::new();
    for (plan_role, role) in THEME_ROLES {
        match resolved.get(&role).map(String::as_str) {
            Some(name) if name != NO_TEXTURE => names.push((plan_role, name.to_owned())),
            _ => notes.push(undressed_note(role)),
        }
    }
    let features = feature_dress(&resolved, &mut notes);
    Ok(ResolvedTheme {
        plan: PlanTheme { names },
        features,
        notes,
    })
}

/// The advisory note for a theme role the session answered `"-"`.
fn undressed_note(role: Role) -> String {
    format!(
        "the theme answers \"-\" for role {}: those faces stay undressed",
        role.as_str()
    )
}

/// The names the feature step dresses with: the plain door face and
/// track ride on the compiler's own answers, the lift's `step` side and
/// the teleporter's `wall` fallback ride on the compiler's `step` and
/// `wall` answers (a role missing there was already noted by the
/// compiler loop), and a coloured track or switch panel the theme left
/// unnamed notes itself like a missing dressing role — its jambs keep
/// the plain track the compiler dressed and its switch panels keep the
/// wall's own middle.
fn feature_dress(resolved: &BTreeMap<Role, String>, notes: &mut Vec<String>) -> FeatureDress {
    FeatureDress {
        door: resolved_name(resolved, Role::Door),
        track: resolved_name(resolved, Role::DoorTrack),
        track_red: unthemed_note(resolved, Role::DoorTrackRed, notes),
        track_blue: unthemed_note(resolved, Role::DoorTrackBlue, notes),
        track_yellow: unthemed_note(resolved, Role::DoorTrackYellow, notes),
        switch: unthemed_note(resolved, Role::Switch, notes),
        exit_switch: unthemed_note(resolved, Role::ExitSwitch, notes),
        step: resolved_name(resolved, Role::Step),
        wall: resolved_name(resolved, Role::Wall),
    }
}

/// `role`'s resolved name, `"-"` when the theme answers nothing, noting
/// itself when it does not.
fn unthemed_note(resolved: &BTreeMap<Role, String>, role: Role, notes: &mut Vec<String>) -> String {
    let name = resolved_name(resolved, role);
    if name == NO_TEXTURE {
        notes.push(undressed_note(role));
    }
    name
}

/// `role`'s resolved name, `"-"` when the theme answers nothing.
fn resolved_name(resolved: &BTreeMap<Role, String>, role: Role) -> String {
    resolved
        .get(&role)
        .cloned()
        .unwrap_or_else(|| NO_TEXTURE.to_owned())
}

/// The wire string naming `kind`'s class, as the result reports it.
const fn kind_name(kind: ConnectionKind) -> &'static str {
    match kind {
        ConnectionKind::Opening => "opening",
        ConnectionKind::Door { .. } => "door",
        ConnectionKind::Window { .. } => "window",
        ConnectionKind::Stairs { .. } => "stairs",
    }
}

/// The first `'…'`-quoted name of `message`, if it carries one.
fn quoted_first(message: &str) -> Option<&str> {
    message.split('\'').nth(1).filter(|name| !name.is_empty())
}

/// The label the compiler gives an unnamed connection: `from->to`.
fn connection_label(connection: &ConnectionParam) -> String {
    connection
        .id
        .clone()
        .unwrap_or_else(|| format!("{}->{}", connection.from, connection.to))
}

/// The ids a wire feature row names: its room, its teleporter rooms
/// and the connection it opens.
fn feature_names(feature: &FeatureParam) -> [Option<&str>; 4] {
    [
        feature.room.as_deref(),
        feature.from_room.as_deref(),
        feature.to_room.as_deref(),
        feature.opens.as_deref(),
    ]
}

/// The plan path naming the offender of a compiler message: the first
/// quoted room id wins, then the first quoted connection label, then
/// the first quoted id a feature row names.
fn plan_path(args: &BuildPlanParams, message: &str) -> Option<String> {
    let name = quoted_first(message)?;
    if let Some(index) = args.rooms.iter().position(|room| room.id == name) {
        return Some(format!("rooms[{index}]"));
    }
    if let Some(index) = args
        .connections
        .iter()
        .position(|connection| connection_label(connection) == name)
    {
        return Some(format!("connections[{index}]"));
    }
    args.features
        .iter()
        .position(|feature| {
            feature_names(feature)
                .into_iter()
                .flatten()
                .any(|id| id == name)
        })
        .map(|index| format!("features[{index}]"))
}

/// The compiler error with its plan path prefixed, keeping the code;
/// messages that name no known id and errors that are neither an
/// argument nor a limit refusal pass through unchanged. An attached
/// [`CyberError::with_hint`] hint rides along with the prefixed rebuild.
fn with_plan_path(error: CyberError, args: &BuildPlanParams) -> CyberError {
    let message = error.message();
    let Some(path) = plan_path(args, &message) else {
        return error;
    };
    match error.code() {
        "invalid_argument" | "limit_exceeded" => error.map_message(|old| format!("{path}: {old}")),
        _ => error,
    }
}

/// The vertex bounds of the compiled map as one result object.
fn bounds_of(map: &Map) -> Option<PlanBounds> {
    map.bounds()
        .map(|(x1, y1, x2, y2)| PlanBounds { x1, y1, x2, y2 })
}

/// The definitions game a plan run reads: the session's detected game,
/// with an unidentified session standing for DOOM II (as the validate
/// and defs tools do).
fn plan_game(found: Game) -> Game {
    if found == Game::Unknown {
        return Game::Doom2;
    }
    found
}

/// The thing definitions `build_map_from_plan` resolves names against:
/// the detected game's tables overlaid with the merged view's
/// `DEHACKED`, `DECORATE`, `ZScript` and `MAPINFO` customs.
///
/// # Errors
///
/// [`collect::custom_sources`]' read errors and [`session_definitions`]'
/// refusal for malformed embedded definition data.
fn plan_definitions(session: &Session) -> Result<Definitions, CyberError> {
    let (sources, _notes) = collect::custom_sources(session)?;
    session_definitions(plan_game(session.game()), &sources)
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
fn resolve_specs(
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
fn ensure_player_start(specs: &[PlannedThing]) -> Result<(), CyberError> {
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
/// [`params`]' shape refusals, the resolution refusals naming
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
fn finalize_nodes(compiled: &mut CompiledPlan) -> Result<Value, CyberError> {
    let plan = AllPlan::for_model(&compiled.map);
    let built = apply_all(&mut compiled.map, &plan)?;
    built.into_nodes_json()
}

/// The finalized validation's result frame: the `Severity::Error` rows
/// of `outcome` capped at [`VALIDATION_ERROR_CAP`], with the
/// per-severity counts over every issue the checks found.
fn validation_view(outcome: &ValidateOutcome) -> Value {
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
struct PlanBounds {
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
struct ConnectionReport {
    /// The connection's id, or its `from->to` label when unnamed.
    id: String,
    /// The kind class: `opening`, `door`, `window` or `stairs`.
    kind: &'static str,
    /// The sectors this connector owns (one band, or one per stair
    /// step), in plan order.
    sectors: Vec<usize>,
}

/// Structured result of [`Cyberdemon::build_map_from_plan`].
#[derive(Serialize)]
struct PlanReport {
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
    /// (no lines, no tag).
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
fn reporting(
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
fn build_run(session: &mut Session, args: &BuildPlanParams) -> Result<PlanReport, CyberError> {
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
    let definitions = plan_definitions(session)?;
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

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = plan_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Builds a whole map block from a declarative rooms-and-connections plan in one undoable edit.
    #[tool(
        description = "Builds a complete map from a declarative plan in one undoable edit: rooms (rect or point-loop footprints with heights, light, flats and theme roles), connections (opening, wired door, window, stairs) grown between rooms that share an edge, thick walls, feature rows (exit, switch, lift, teleporter), things resolved against the session's definitions, and a mandatory player start. units and grid map plan coordinates to map units. finalize also builds nodes and validates. This is the first tool for a new level, not the element editors."
    )]
    fn build_map_from_plan(
        &self,
        Parameters(args): Parameters<BuildPlanParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| build_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{
        BuildPlanParams, CompiledPlan, ConnectionKind, FEATURE_ROLES, PlanTheme, THEME_ROLES,
        TextureRole, VALIDATION_ERROR_CAP, bounds_of, connection_label, ensure_player_start,
        feature_dress, finalize_nodes, kind_name, plan_game, plan_path, quoted_first, reporting,
        resolve_specs, validation_view, with_plan_path,
    };
    use alloc::collections::BTreeMap;

    use crate::defs::definitions_for;
    use crate::error::CyberError;
    use crate::plan::FeatureRow;
    use crate::plan::compile;
    use crate::plan::features::FeatureDress;
    use crate::plan::textures::TextureRoles as _;
    use crate::plan::things::{PlannedThing, Skill, ThingSpot};
    use crate::themes::Role;
    use crate::tools::plan::params::{KindSource, ThingRequest};
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::FlagFamily;
    use crate::wad::map::Map;
    use crate::wad::map::validate::{Issue, Outcome as ValidateOutcome, Severity};
    use crate::wad::name::LumpName;

    /// A request parsed from `json`.
    fn args(json: &str) -> BuildPlanParams {
        from_str(json).expect("test requests parse")
    }

    /// A two-room, one-door request with the ids the path tests need.
    fn plan_args() -> BuildPlanParams {
        args(
            r#"{"map": "MAP05",
                "rooms": [
                    {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
                    {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128}}
                ],
                "connections": [
                    {"from": "a", "to": "b", "kind": "opening"},
                    {"id": "d1", "from": "a", "to": "b", "kind": "door"}
                ]}"#,
        )
    }

    #[test]
    fn connector_kinds_carry_their_wire_names() {
        use crate::plan::model::DoorStyle;
        assert_eq!(kind_name(ConnectionKind::Opening), "opening");
        assert_eq!(
            kind_name(ConnectionKind::Door {
                style: DoorStyle::Remote
            }),
            "door",
            "the style does not change the class name"
        );
        assert_eq!(
            kind_name(ConnectionKind::Window {
                sill: 32_i32,
                lintel: 24_i32
            }),
            "window"
        );
        assert_eq!(
            kind_name(ConnectionKind::Stairs {
                step_height: 16_i32,
                step_depth: 24_i32
            }),
            "stairs"
        );
    }

    #[test]
    fn every_dressing_role_maps_to_a_theme_role() {
        let mapped: Vec<TextureRole> = THEME_ROLES.iter().map(|&(role, _)| role).collect();
        for role in [
            TextureRole::Floor,
            TextureRole::Ceiling,
            TextureRole::Wall,
            TextureRole::Door,
            TextureRole::DoorFloor,
            TextureRole::DoorTrack,
            TextureRole::WindowSill,
            TextureRole::Step,
        ] {
            assert_eq!(
                mapped.iter().filter(|slot| **slot == role).count(),
                1,
                "{role:?} appears exactly once"
            );
        }
        assert_eq!(mapped.len(), 8, "eight plan roles, eight answers");
    }

    #[test]
    fn a_theme_that_answers_dash_leaves_the_role_undressed() {
        let theme = PlanTheme {
            names: vec![(TextureRole::Wall, "WALL1".to_owned())],
        };
        assert_eq!(theme.texture(TextureRole::Wall), Some("WALL1"));
        assert_eq!(
            theme.texture(TextureRole::Step),
            None,
            "absent means undressed"
        );
        let empty = PlanTheme { names: Vec::new() };
        assert_eq!(empty.texture(TextureRole::Floor), None);
    }

    #[test]
    fn a_fully_named_feature_dress_carries_every_colour_and_notes_nothing() {
        let resolved: BTreeMap<Role, String> = [
            (Role::Door, "DOOR7"),
            (Role::DoorTrack, "DOORTRAK"),
            (Role::DoorTrackRed, "DOORRED"),
            (Role::DoorTrackBlue, "DOORBLU"),
            (Role::DoorTrackYellow, "DOORYEL"),
            (Role::Switch, "SW1STRTN"),
            (Role::ExitSwitch, "SW1EXIT"),
            (Role::Step, "STEP1"),
            (Role::Wall, "STARTAN3"),
        ]
        .into_iter()
        .map(|(role, name)| (role, name.to_owned()))
        .collect();
        let mut notes = Vec::new();
        let dress = feature_dress(&resolved, &mut notes);
        assert_eq!(
            dress,
            FeatureDress {
                door: "DOOR7".to_owned(),
                track: "DOORTRAK".to_owned(),
                track_red: "DOORRED".to_owned(),
                track_blue: "DOORBLU".to_owned(),
                track_yellow: "DOORYEL".to_owned(),
                switch: "SW1STRTN".to_owned(),
                exit_switch: "SW1EXIT".to_owned(),
                step: "STEP1".to_owned(),
                wall: "STARTAN3".to_owned(),
            },
            "every name carries through"
        );
        assert!(notes.is_empty(), "a named colour says nothing: {notes:?}");
    }

    #[test]
    fn unnamed_feature_roles_note_themselves_like_dressing_roles() {
        let mut resolved = BTreeMap::new();
        resolved.insert(Role::DoorTrack, "-".to_owned());
        let mut notes = Vec::new();
        let dress = feature_dress(&resolved, &mut notes);
        assert_eq!(dress.door, "-", "an absent face is dashed like the rest");
        assert_eq!(dress.track_blue, "-", "and so is an absent colour");
        assert_eq!(dress.switch, "-", "and an absent switch panel");
        assert_eq!(
            dress.step, "-",
            "an absent step rides on silently for the lift"
        );
        assert_eq!(dress.wall, "-", "and an absent wall for the teleporter");
        assert_eq!(
            notes,
            FEATURE_ROLES
                .iter()
                .map(|role| format!(
                    "the theme answers \"-\" for role {}: those faces stay undressed",
                    role.as_str()
                ))
                .collect::<Vec<String>>(),
            "all five feature roles note themselves, in table order"
        );
    }

    #[test]
    fn quoted_first_reads_the_first_name_of_a_message() {
        assert_eq!(quoted_first("room 'a' has no edge"), Some("a"));
        assert_eq!(
            quoted_first("connection 'd1' cannot be placed: rooms a and b"),
            Some("d1")
        );
        assert_eq!(quoted_first("no names here"), None, "unquoted passes");
        assert_eq!(quoted_first("empty '' name"), None, "empty is not a name");
    }

    #[test]
    fn plan_paths_resolve_rooms_first_then_connection_labels() {
        let plan = plan_args();
        assert_eq!(
            plan_path(&plan, "room 'b' has a 0x0 rectangle").as_deref(),
            Some("rooms[1]")
        );
        assert_eq!(
            plan_path(&plan, "connection 'a->b' is too wide").as_deref(),
            Some("connections[0]"),
            "the derived label names the unnamed connection"
        );
        assert_eq!(
            plan_path(&plan, "connection 'd1' rooms do not touch").as_deref(),
            Some("connections[1]")
        );
        assert_eq!(
            plan_path(&plan, "room 'ghost' is not in the plan"),
            None,
            "an unknown name finds no path"
        );
    }

    #[test]
    fn connection_labels_prefer_the_id_over_from_arrow_to() {
        let plan = plan_args();
        let unnamed = plan.connections.first().expect("the opening");
        assert_eq!(connection_label(unnamed), "a->b");
        let named = plan.connections.get(1).expect("the door");
        assert_eq!(connection_label(named), "d1");
    }

    /// A two-room, one-door request carrying an exit at an unknown room
    /// and a switch opening an unknown connection id.
    fn feature_plan() -> BuildPlanParams {
        args(
            r#"{"map": "MAP05",
                "rooms": [
                    {"id": "a", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}},
                    {"id": "b", "rect": {"x": 128, "y": 0, "w": 128, "h": 128}}
                ],
                "connections": [
                    {"id": "d1", "from": "a", "to": "b", "kind": "door", "style": "remote"}
                ],
                "features": [
                    {"kind": "exit", "room": "ghost", "style": "walk_over"},
                    {"kind": "switch", "room": "a", "opens": "nope"}
                ]}"#,
        )
    }

    #[test]
    fn plan_paths_reach_feature_rows_by_their_named_ids() {
        let plan = feature_plan();
        assert_eq!(
            plan_path(&plan, "exit feature names unknown room 'ghost'").as_deref(),
            Some("features[0]"),
            "a feature's room names the row"
        );
        assert_eq!(
            plan_path(&plan, "switch feature opens unknown connection 'nope'").as_deref(),
            Some("features[1]"),
            "the opens value names the row"
        );
        assert_eq!(
            plan_path(&plan, "room 'a' is painted over").as_deref(),
            Some("rooms[0]"),
            "a live room wins over the switch that references it"
        );
        assert_eq!(
            plan_path(&plan, "connection 'd1' needs a remote door").as_deref(),
            Some("connections[0]"),
            "a real connection id wins over a feature naming it"
        );
    }

    #[test]
    fn feature_refusals_carry_their_plan_path() {
        let plan = feature_plan();
        let prefixed = with_plan_path(
            CyberError::InvalidArgument(
                "switch feature opens unknown connection 'nope'".to_owned(),
            ),
            &plan,
        );
        assert_eq!(prefixed.code(), "invalid_argument", "the code stands");
        assert_eq!(
            prefixed.to_string(),
            "features[1]: switch feature opens unknown connection 'nope'",
            "the path leads"
        );
    }

    #[test]
    fn with_plan_path_prefixes_refusals_and_keeps_the_rest() {
        let plan = plan_args();
        let invalid = with_plan_path(
            CyberError::InvalidArgument("room 'a' has a 0x64 rectangle".to_owned()),
            &plan,
        );
        assert_eq!(invalid.code(), "invalid_argument", "the code stands");
        assert_eq!(
            invalid.to_string(),
            "rooms[0]: room 'a' has a 0x64 rectangle",
            "the path leads"
        );
        let limit = with_plan_path(
            CyberError::LimitExceeded("connection 'd1' needs 9999 steps".to_owned()),
            &plan,
        );
        assert_eq!(limit.code(), "limit_exceeded", "limits keep their code");
        assert!(limit.to_string().starts_with("connections[1]: "), "{limit}");
        let unhandled = CyberError::Internal("room 'a' broke the server".to_owned());
        assert!(
            matches!(with_plan_path(unhandled, &plan), CyberError::Internal(text) if text == "room 'a' broke the server"),
            "internal errors pass through"
        );
        let nameless = with_plan_path(
            CyberError::InvalidArgument("plan has no rooms; add at least one room".to_owned()),
            &plan,
        );
        assert_eq!(
            nameless.to_string(),
            "plan has no rooms; add at least one room",
            "no quoted id, no prefix"
        );
    }

    #[test]
    fn bounds_fold_the_vertex_box_into_the_result_object() {
        assert_eq!(
            bounds_of(&Map::default()),
            None,
            "an empty map has no bounds"
        );
        let mut map = Map::new_empty(
            LumpName::parse("MAP01").expect("a valid name"),
            MapFormat::Doom,
        );
        map.vertices.push(crate::wad::map::Vertex {
            x: -64_i32,
            y: 8_i32,
            ..Default::default()
        });
        let bounds = bounds_of(&map).expect("one vertex bounds itself");
        assert_eq!(
            serde_json::to_value(&bounds).expect("it serializes"),
            serde_json::json!({"x1": -64_i64, "y1": 8_i64, "x2": -64_i64, "y2": 8_i64}),
            "the documented field names"
        );
    }

    /// A minimal compile for the integration-free smoke: one room,
    /// no connections, roles answering nothing.
    #[test]
    fn a_single_room_plan_compiles_through_the_same_parts() {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": []}"#,
        );
        let model = plan.to_model().expect("the wire maps onto the model");
        let theme = PlanTheme { names: Vec::new() };
        let name = LumpName::parse("MAP05").expect("a valid name");
        let compiled = compile(&model, name, MapFormat::Doom, &theme).expect("one room compiles");
        assert_eq!(compiled.map.sectors.len(), 1, "just the room");
        assert_eq!(compiled.room_sectors.get("a"), Some(&vec![0_usize]));
        assert_eq!(
            compiled.map.bounds().expect("vertices"),
            (0_i32, 0_i32, 64_i32, 64_i32)
        );
    }

    /// A request row with the shape tests' defaults.
    fn request(kind: KindSource, path: &str) -> ThingRequest {
        ThingRequest {
            kind,
            spot: ThingSpot::Point((8_i64, 8_i64)),
            count: 1_u32,
            angle: 0_i32,
            skills: None,
            multiplayer_only: false,
            path: path.to_owned(),
        }
    }

    #[test]
    fn resolve_specs_reads_names_keeps_numbers_and_folds_flags() {
        let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
        let numbered = request(KindSource::Type(2035_u32), "things[0]");
        let named = ThingRequest {
            kind: KindSource::Name("imp".to_owned()),
            spot: ThingSpot::Room { id: "a".to_owned() },
            skills: Some(vec![Skill::Easy]),
            path: "things[1]".to_owned(),
            ..numbered
        };
        let start = request(KindSource::PlayerOne, "player_start");
        let specs = resolve_specs(&[numbered, named, start], FlagFamily::Doom, &definitions)
            .expect("the rows resolve");
        assert_eq!(
            specs.iter().map(|spec| spec.kind).collect::<Vec<u32>>(),
            vec![2035_u32, 3001_u32, 1_u32],
            "type passes, name resolves, player_start means type 1"
        );
        assert_eq!(
            specs.iter().map(|spec| spec.flags).collect::<Vec<u32>>(),
            vec![7_u32, 1_u32, 7_u32],
            "defaults are all-skill presence; easy-only stays easy-only"
        );
        assert_eq!(specs.first().expect("three rows").path, "things[0]");
    }

    #[test]
    fn an_unresolvable_name_is_refused_with_the_row_path() {
        let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
        let bad = request(KindSource::Name("platypus".to_owned()), "things[2]");
        let error =
            resolve_specs(&[bad], FlagFamily::Doom, &definitions).expect_err("no such thing");
        assert_eq!(error.code(), "invalid_argument");
        let message = error.to_string();
        assert!(message.contains("things[2]"), "the path leads: {message}");
        assert!(message.contains("no thing named 'platypus'"), "{message}");
    }

    #[test]
    fn ensure_player_start_reads_resolved_kinds_not_wire_names() {
        let monster = PlannedThing {
            kind: 3001_u32,
            spot: ThingSpot::Point((8_i64, 8_i64)),
            count: 1_u32,
            angle: 0_i32,
            flags: 7_u32,
            path: "things[0]".to_owned(),
        };
        let error = ensure_player_start(core::slice::from_ref(&monster)).expect_err("no type 1");
        assert_eq!(error.code(), "invalid_argument");
        assert_eq!(
            error.to_string(),
            "a map needs a player 1 start; pass player_start or a thing of type 1"
        );
        let start = PlannedThing {
            kind: 1_u32,
            ..monster.clone()
        };
        ensure_player_start(&[monster, start]).expect("the resolved type 1 satisfies the check");
    }

    #[test]
    fn an_unidentified_session_stands_for_doom_ii() {
        assert_eq!(plan_game(Game::Unknown), Game::Doom2, "the fallback");
        assert_eq!(plan_game(Game::Doom), Game::Doom, "a detected game stands");
        assert_eq!(plan_game(Game::Hexen), Game::Hexen);
    }

    #[test]
    fn the_report_counts_placed_things_under_its_documented_field() {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": []}"#,
        );
        let model = plan.to_model().expect("the wire maps onto the model");
        let theme = PlanTheme { names: Vec::new() };
        let compiled = compile(
            &model,
            LumpName::parse("MAP05").expect("a valid name"),
            MapFormat::Doom,
            &theme,
        )
        .expect("one room compiles");
        let value = serde_json::to_value(reporting(&compiled, 5_usize, Vec::new(), None))
            .expect("it serializes");
        assert_eq!(
            value["things_placed"],
            serde_json::json!(5_u64),
            "the live field"
        );
        assert_eq!(
            value["features"],
            serde_json::json!([]),
            "a plan with no doors reports no feature rows"
        );
        assert_eq!(
            value["nodes"],
            serde_json::Value::Null,
            "null without finalize"
        );
        assert_eq!(
            value["validation"],
            serde_json::Value::Null,
            "null without finalize"
        );
    }

    #[test]
    fn the_report_lists_feature_rows_after_their_connections() {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": []}"#,
        );
        let model = plan.to_model().expect("the wire maps onto the model");
        let theme = PlanTheme { names: Vec::new() };
        let compiled = compile(
            &model,
            LumpName::parse("MAP05").expect("a valid name"),
            MapFormat::Doom,
            &theme,
        )
        .expect("one room compiles");
        let rows = vec![FeatureRow {
            kind: "door",
            room: None,
            sectors: vec![2_usize],
            lines: vec![3_usize, 4_usize],
            tag: Some(1_i32),
        }];
        let value =
            serde_json::to_value(reporting(&compiled, 1_usize, rows, None)).expect("it serializes");
        assert_eq!(
            value["features"],
            serde_json::json!([
                {"kind": "door", "room": null, "sectors": [2_u64], "lines": [3_u64, 4_u64],
                 "tag": 1_u64}
            ]),
            "the documented row shape"
        );
    }

    #[test]
    fn finalize_fills_the_nodes_and_validation_result_fields() {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": [],
                "finalize": true}"#,
        );
        assert!(plan.finalize, "the wire flag arrives");
        let model = plan.to_model().expect("the wire maps onto the model");
        let theme = PlanTheme { names: Vec::new() };
        let compiled = compile(
            &model,
            LumpName::parse("MAP05").expect("a valid name"),
            MapFormat::Doom,
            &theme,
        )
        .expect("one room compiles");
        let value = serde_json::to_value(reporting(
            &compiled,
            1_usize,
            Vec::new(),
            Some((
                serde_json::json!({"variant": "classic", "segs": 4_u64}),
                serde_json::json!({"errors": [], "counts": {"error": 0_u64}}),
            )),
        ))
        .expect("it serializes");
        assert_eq!(value["nodes"]["variant"], serde_json::json!("classic"));
        assert_eq!(value["nodes"]["segs"], serde_json::json!(4_u64));
        assert_eq!(value["validation"]["errors"], serde_json::json!([]));
    }

    #[test]
    fn finalize_defaults_to_off_on_the_wire() {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": []}"#,
        );
        assert!(!plan.finalize, "absent means no node build");
    }

    /// The one-room compiled plan both finalize tests grow a tree from.
    fn compiled_room(format: MapFormat) -> CompiledPlan {
        let plan = args(
            r#"{"map": "MAP05",
                "rooms": [{"id": "a", "rect": {"x": 0, "y": 0, "w": 64, "h": 64}}],
                "connections": []}"#,
        );
        let model = plan.to_model().expect("the wire maps onto the model");
        let theme = PlanTheme { names: Vec::new() };
        compile(
            &model,
            LumpName::parse("MAP05").expect("a valid name"),
            format,
            &theme,
        )
        .expect("one room compiles")
    }

    #[test]
    fn finalize_nodes_installs_the_whole_binary_run_in_place() {
        let mut compiled = compiled_room(MapFormat::Doom);
        let before = compiled.map.vertices.len();
        let summary = finalize_nodes(&mut compiled).expect("a square builds a tree");
        assert_eq!(
            summary["variant"],
            serde_json::json!("classic"),
            "the binary default layout"
        );
        assert!(
            summary["segs"].as_u64().expect("a count") > 0,
            "segs written"
        );
        assert!(
            summary["subsectors"].as_u64().expect("a count") > 0,
            "subsectors written"
        );
        assert!(
            compiled.map.vertices.len() >= before,
            "the classic run only ever adds split vertices"
        );
        for wanted in [
            b"SEGS".as_slice(),
            b"SSECTORS",
            b"NODES",
            b"REJECT",
            b"BLOCKMAP",
        ] {
            assert!(
                compiled
                    .map
                    .other
                    .iter()
                    .any(|lump| lump.name.as_bytes() == wanted),
                "the binary finalize installs {wanted:?}: {:?}",
                compiled
                    .map
                    .other
                    .iter()
                    .map(|lump| lump.name.to_string())
                    .collect::<Vec<String>>()
            );
        }
    }

    #[test]
    fn finalize_nodes_gives_a_udmf_model_the_znodes_lump_only() {
        let mut compiled = compiled_room(MapFormat::Udmf);
        let summary = finalize_nodes(&mut compiled).expect("a square builds a tree");
        assert_eq!(
            summary["variant"],
            serde_json::json!("extended"),
            "the UDMF default layout"
        );
        assert_eq!(
            summary["format"],
            serde_json::json!("ZNOD"),
            "compressed by default"
        );
        assert!(
            compiled
                .map
                .other
                .iter()
                .any(|lump| lump.name.as_bytes() == b"ZNODES"),
            "the extended lump lands in ZNODES"
        );
        for absent in [b"REJECT".as_slice(), b"BLOCKMAP", b"SEGS", b"SSECTORS"] {
            assert!(
                !compiled
                    .map
                    .other
                    .iter()
                    .any(|lump| lump.name.as_bytes() == absent),
                "{absent:?} stays out of a UDMF block"
            );
        }
    }

    /// An [`Issue`] row of `severity` carrying a numbered message.
    fn issue(severity: Severity, number: usize) -> Issue {
        Issue {
            severity,
            code: "synthetic",
            message: format!("synthetic issue {number}"),
            vertices: Vec::new(),
            linedefs: Vec::new(),
            sidedefs: Vec::new(),
            sectors: Vec::new(),
            things: Vec::new(),
            x: None,
            y: None,
        }
    }

    #[test]
    fn validation_view_caps_the_error_rows_and_keeps_counts_exact() {
        let mut issues: Vec<Issue> = (0..VALIDATION_ERROR_CAP + 7_usize)
            .map(|number| issue(Severity::Error, number))
            .collect();
        issues.push(issue(Severity::Warning, 0_usize));
        issues.push(issue(Severity::Info, 0_usize));
        issues.push(issue(Severity::Warning, 1_usize));
        let outcome = ValidateOutcome {
            checks_run: Vec::new(),
            issues,
        };
        let value = validation_view(&outcome);
        let errors = value["errors"].as_array().expect("an error list");
        assert_eq!(
            errors.len(),
            VALIDATION_ERROR_CAP,
            "the rows are capped at the documented 20"
        );
        assert_eq!(
            value["counts"]["error"],
            serde_json::json!(27_u64),
            "the count keeps the whole truth"
        );
        assert_eq!(value["counts"]["warning"], serde_json::json!(2_u64));
        assert_eq!(value["counts"]["info"], serde_json::json!(1_u64));
        assert_eq!(
            errors.first().expect("a row")["code"],
            serde_json::json!("synthetic"),
            "rows keep the validate_map issue shape"
        );
    }

    #[test]
    fn validation_view_of_a_clean_map_holds_no_rows() {
        let outcome = ValidateOutcome {
            checks_run: Vec::new(),
            issues: vec![issue(Severity::Info, 0_usize)],
        };
        let value = validation_view(&outcome);
        assert_eq!(value["errors"], serde_json::json!([]), "only the info row");
        assert_eq!(value["counts"]["error"], serde_json::json!(0_u64));
        assert_eq!(value["counts"]["info"], serde_json::json!(1_u64));
    }
}
