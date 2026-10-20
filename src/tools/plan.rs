//! Plan tools: `build_map_from_plan` (see the `plan` router on
//! [`crate::server::Cyberdemon`]).
//!
//! One declarative call: rooms with footprints and sector properties,
//! connections joining them, a wall thickness and a theme. The wire
//! shape and its mapping onto [`crate::plan::model`] live in
//! [`params`]; the pure [`crate::plan::compile`] engine grows the thick
//! walls, places every connector band, emits the fresh [`Map`] and
//! dresses it in the [`ThemeRoles`] this module resolves for the call.
//! The compiled block is written into the working archive as exactly
//! one [`Session::edit`] step through
//! [`crate::tools::maps::write_block`], and the map becomes the
//! session's active map. Geometry only: no things, specials or node
//! data yet — later slices grow the `things`/`finalize` parameters and
//! fill the reserved `nodes`/`validation` result fields. Before the
//! compiler, the pure [`crate::plan::units`] transform carries the
//! drawn plan into map units: the `units` scale/origin/mirror and the
//! `grid` snap, so a sketch measured in pixels lands grid-aligned at
//! map size.
//!
//! Compiler refusals already name the offending id;
//! [`with_plan_path`] prefixes them with the plan path (`rooms[0]`,
//! `connections[1]`) so the caller can point at the wire element that
//! broke.

mod params;

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use serde::Serialize;
use serde_json::Value;

use crate::error::CyberError;
use crate::plan::CompiledPlan;
use crate::plan::compile;
use crate::plan::model::ConnectionKind;
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::plan::transform_plan;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::{parse_format, resolve_namespace, write_block};
use crate::tools::plan::params::{BuildPlanParams, ConnectionParam};
use crate::tools::themes::theme_roles;
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::write_map;
use crate::wad::name::LumpName;

/// The texture name meaning "nothing here" in a resolved theme.
const NO_TEXTURE: &str = "-";
/// The steps every result hands the caller; the later slices extend it.
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

/// The compiler's theme for this call and the notes for the roles that
/// answered `"-"`.
///
/// # Errors
///
/// [`theme_roles`]' refusals for an unknown `theme` override.
fn plan_theme(
    session: &Session,
    theme: Option<&str>,
) -> Result<(PlanTheme, Vec<String>), CyberError> {
    let wanted: Vec<Role> = THEME_ROLES.iter().map(|&(_, role)| role).collect();
    let resolved = theme_roles(session, theme, &wanted)?;
    let mut names = Vec::new();
    let mut notes = Vec::new();
    for (plan_role, role) in THEME_ROLES {
        match resolved.get(&role).map(String::as_str) {
            Some(name) if name != NO_TEXTURE => names.push((plan_role, name.to_owned())),
            _ => notes.push(format!(
                "the theme answers \"-\" for role {}: those faces stay undressed",
                role.as_str()
            )),
        }
    }
    Ok((PlanTheme { names }, notes))
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

/// The plan path naming the offender of a compiler message: the first
/// quoted room id wins, then the first quoted connection label.
fn plan_path(args: &BuildPlanParams, message: &str) -> Option<String> {
    let name = quoted_first(message)?;
    if let Some(index) = args.rooms.iter().position(|room| room.id == name) {
        return Some(format!("rooms[{index}]"));
    }
    args.connections
        .iter()
        .position(|connection| connection_label(connection) == name)
        .map(|index| format!("connections[{index}]"))
}

/// The compiler error with its plan path prefixed, keeping the code;
/// messages that name no known id and errors that are neither an
/// argument nor a limit refusal pass through unchanged.
fn with_plan_path(error: CyberError, args: &BuildPlanParams) -> CyberError {
    let message = error.to_string();
    let Some(path) = plan_path(args, &message) else {
        return error;
    };
    let prefixed = format!("{path}: {message}");
    match error {
        CyberError::InvalidArgument(_) => CyberError::InvalidArgument(prefixed),
        CyberError::LimitExceeded(_) => CyberError::LimitExceeded(prefixed),
        other @ (CyberError::NotFound(_)
        | CyberError::AlreadyExists(_)
        | CyberError::Conflict(_)
        | CyberError::Format(_)
        | CyberError::Io(_)
        | CyberError::Fetch(_)
        | CyberError::Unsupported(_)
        | CyberError::Internal(_)) => other,
    }
}

/// The vertex bounds of the compiled map as one result object.
fn bounds_of(map: &Map) -> Option<PlanBounds> {
    map.bounds()
        .map(|(x1, y1, x2, y2)| PlanBounds { x1, y1, x2, y2 })
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
    /// The vertex bounding box; `null` for a map without vertices.
    bounds: Option<PlanBounds>,
    /// The compiler's advisories (unreachable and painted-over rooms,
    /// too-tall floor gaps) plus the notes for theme roles that answered
    /// `"-"`.
    warnings: Vec<String>,
    /// Reserved for the node-building slice: `null` until it lands.
    nodes: Option<Value>,
    /// Reserved for the validation slice: `null` until it lands.
    validation: Option<Value>,
    /// What to look at next.
    next_steps: [&'static str; 2],
}

/// The result frame for a compiled plan, before the archive write.
fn reporting(compiled: &CompiledPlan) -> PlanReport {
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
        bounds: bounds_of(&compiled.map),
        warnings: compiled.warnings.clone(),
        nodes: None,
        validation: None,
        next_steps: NEXT_STEPS,
    }
}

/// `build_map_from_plan`'s work: resolve the parameters, carry the
/// drawn plan into map units, compile the plan, then write the block as
/// one [`Session::edit`] step and make the map the session's active one.
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
    let (theme, notes) = plan_theme(session, args.theme.as_deref())?;
    let mut compiled =
        compile(&plan, name, format, &theme).map_err(|error| with_plan_path(error, args))?;
    compiled.map.udmf_namespace = namespace;
    compiled.warnings.extend(notes);
    let report = reporting(&compiled);
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
        description = "Builds a complete map from a declarative plan - rooms with footprints and sector properties, connections joining them, thick walls grown between - and writes the compiled block into the working archive as one undoable edit. Geometry only today: the map has no things, door specials or node data until later slices add them. map is the new block's header name (1 to 8 printable ASCII, case-insensitive); when the working archive already holds that name the call fails already_exists unless replace: true discards the old block first (maps only lower layers hold are always overridable). format: \"doom\" (default), \"hexen\" or \"udmf\"; namespace: is the UDMF TEXTMAP's namespace declaration (letters, digits, underscores, lowercased, default \"zdoom\") and is ignored for binary formats. wall_thickness (default 16) is the map-unit wall grown between and around rooms; 0 lets rooms share their walls directly. units: {scale_num (1), scale_den (1), origin_x (0), origin_y (0), flip_y (false)} carries the plan's coordinates into map units before compiling: map = origin + coordinate * scale_num / scale_den, with y subtracted from origin_y when flip_y: true mirrors an image-space drawing; grid (default 8) then snaps every transformed coordinate to the nearest multiple (1 switches snapping off); a grid below 1 or a scale_den or scale_num of 0 is invalid_argument naming the parameter. Image-space recipe: pass units: {scale_num: 4, flip_y: true} to compile a sketch measured in pixels - one pixel becomes 4 map units with y mirrored. Only coordinates transform: wall_thickness and every connection width, sill, lintel, step_height and step_depth are map units already and never scale. rooms: each room has an id (unique in the plan, connections refer to it) and exactly one of rect: {x, y, w, h} (both edges positive) or points: [[x, y], ...] (a closed ring of at least 3 distinct points; rooms are painted in order, a later one covers an earlier one); with sector properties floor (0), ceiling (128), light (160), floor_flat, ceiling_flat, wall_texture (the theme's roles answer unset ones) and outdoor (false; an outdoor room's ceiling is the sky). Both or neither of rect/points is invalid_argument naming the plan path (rooms[2]). connections: from and to are room ids (distinct, existing), kind is opening (a plain gap), door (a band whose sector comes out closed - floor equals ceiling - and carries no linedef special until a later slice wires it: until then it is a solid threshold, style: normal/fast/stay_open/locked_red/locked_blue/locked_yellow/remote picks its dressing only), window (a glass band: sill 32 lifts its floor, lintel 24 drops its ceiling, under 16 units of glass is invalid_argument) or stairs (steps of step_height 16 and step_depth 24 from the lower room's floor to the higher one's); at: [x, y] places it on the shared boundary nearest that point (default: centred), width (64) is its clear width and must fit the boundary. Rooms that share no boundary fail invalid_argument naming the connection and saying they do not touch; every structural refusal names the offending id and is prefixed with its plan path (rooms[0]: ...). theme: dresses this one call from a named theme (unknown names are invalid_argument listing the game's themes) without touching the session's theme or overrides; the session theme answers by default, and a room's own flats/wall texture beat it. Returns {map, format, rooms: {id: [sector indices]}, connections: [{id, kind, sectors}], bounds {x1, y1, x2, y2}, warnings, nodes: null, validation: null, next_steps}: room_sectors lists what each room owns (a fully painted-over room holds no entry and warns), connections report each band's sectors in plan order, and nodes/validation stay null until the later slices fill them. Run undo to take the whole block back. Look at the result with render_topdown and check it with validate_map (no player start until things land)."
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
        BuildPlanParams, ConnectionKind, PlanTheme, THEME_ROLES, TextureRole, bounds_of,
        connection_label, kind_name, plan_path, quoted_first, with_plan_path,
    };
    use crate::error::CyberError;
    use crate::plan::compile;
    use crate::plan::textures::TextureRoles as _;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::Map;
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
}
