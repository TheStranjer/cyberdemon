//! `add_window`: the raised, unwalkable view strip between two rooms.
//!
//! `add_window` reuses `add_door`'s placement step for step: the same
//! [`crate::tools::features::wall::choose_wall`],
//! [`crate::tools::features::wall::carve_strip`] and
//! [`crate::tools::features::wall::strip_wall`] machinery picks the wall
//! and, between touching rooms, carves the 16-unit void strip first, and
//! [`crate::wad::map::features::door_wall`]/[`crate::wad::map::features::door_rect`]
//! fit the window rectangle that one
//! [`crate::wad::map::overlay::apply_overlay`]
//! [`crate::wad::map::overlay::Paint::Sector`] then paints. The pure
//! [`crate::wad::map::features`] window engine does the rest:
//! [`crate::wad::map::features::window_record`] computes the sill/lintel
//! heights (and refuses an opening under
//! [`crate::wad::map::features::MIN_OPENING`]),
//! [`crate::wad::map::features::boundary_lines`] finds the drawn strip's
//! room-facing lines and [`crate::wad::map::features::apply_window`]
//! blocks them off, dressing their room sides with the theme's
//! `window_sill` (below) and `wall` (above) roles resolved exactly like
//! a door's skins ([`crate::tools::features::theme_wall`],
//! [`crate::tools::maps::params::validate_texture_name`]). The whole
//! window is one undoable edit.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{edit_result_fragment, validate_texture_name};
use crate::tools::themes::theme_texture;
use crate::wad::classify::MapFormat;
use crate::wad::map::features::{
    WindowHeights, WindowSides, WindowTextures, apply_window, boundary_lines, door_rect, door_wall,
    outward_depth, window_record,
};
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, Paint, WallTextures};
use crate::wad::map::{Map, Sector};

use super::door::{
    Chosen, DEFAULT_WIDTH, DoorPointParam, MAX_DEPTH, Target, point_of, require_wall_survives,
    target_of,
};
use super::wall::{
    STRIP_THICKNESS, carve_strip, choose_wall, outward_normal, require_landing, strip_face_lost,
    strip_wall, to_pts,
};
use super::{current_map, no_room, theme_wall, wall_lost};

/// The `add_window` default sill height above the higher room floor.
const DEFAULT_SILL: i32 = 32_i32;

/// The `add_window` default lintel depth below the lower room ceiling.
const DEFAULT_LINTEL: i32 = 24_i32;

/// Arguments for [`Cyberdemon::add_window`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddWindowParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The two rooms the window looks between, as `[sectorA, sectorB]`:
    /// exactly one of `between` and `linedef`. Sector A lends the strip
    /// its flats and light.
    #[serde(default)]
    pub(super) between: Option<[usize; 2]>,
    /// The wall to cut, as a linedef index: exactly one of `between` and
    /// `linedef`. A two-sided line behaves like `between` on its two
    /// sectors; a one-sided one opens towards the space its front faces.
    #[serde(default)]
    pub(super) linedef: Option<usize>,
    /// Where along the wall to centre the cut, `{x, y}` (optional: the cut
    /// centres on the wall's midpoint; `between` picks the shared line
    /// nearest here).
    #[serde(default)]
    pub(super) at: Option<DoorPointParam>,
    /// The window's opening width in map units (default 64).
    #[serde(default)]
    pub(super) width: Option<i32>,
    /// How far the window floor sits above the higher room floor, in map
    /// units (default 32).
    #[serde(default)]
    pub(super) sill: Option<i32>,
    /// How far the window ceiling sits below the lower room ceiling, in
    /// map units (default 24).
    #[serde(default)]
    pub(super) lintel: Option<i32>,
}

/// The theme-resolved textures a window wears.
#[derive(Debug)]
pub(super) struct WindowSkins {
    /// The sill band: the room-side `lower` under the opening.
    pub(super) sill: String,
    /// The wall band: the room-side `upper`, and the strip's plug walls.
    pub(super) wall: String,
}

/// Everything [`build_window`] needs once the wire parameters resolved.
#[derive(Debug)]
pub(super) struct WindowSpec {
    /// The wall the window is cut into.
    pub(super) target: Target,
    /// The requested cut centre, if any.
    pub(super) at: Option<Point>,
    /// The window width in map units.
    pub(super) width: i32,
    /// The vertical trim of the opening.
    pub(super) trim: WindowHeights,
    /// The textures to dress with.
    pub(super) skins: WindowSkins,
}

/// The drawn window strip, ready to dress, and what the paint reported.
#[derive(Debug)]
struct Drawn {
    /// Index of the window sector the strip became.
    sector: usize,
    /// The two-sided room-facing lines [`boundary_lines`] found.
    lines: Vec<usize>,
    /// What the paint reported.
    notes: Vec<String>,
}

/// What one [`build_window`] step did, as the tool reports it.
#[derive(Debug, Serialize)]
pub(super) struct BuiltWindow {
    /// Index of the window sector the strip became.
    pub(super) window_sector: usize,
    /// Indices of the two-sided blocking lines around the glass.
    pub(super) lines: Vec<usize>,
    /// What the drawings reported, plus the void-strip note.
    pub(super) notes: Vec<String>,
}

/// Structured result of [`Cyberdemon::add_window`].
#[derive(Serialize)]
pub(super) struct WindowReport {
    /// What the step built, inlined next to the trim echo.
    #[serde(flatten)]
    built: BuiltWindow,
    /// The window width in map units.
    width: i32,
    /// The sill height the record used.
    sill: i32,
    /// The lintel height the record used.
    lintel: i32,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The textures a window wears: the theme's `window_sill` and `wall`
/// roles, validated for the map's format (the same resolution pattern
/// [`theme_skin`] runs for a door).
///
/// # Errors
///
/// [`theme_texture`]'s errors and [`validate_texture_name`]' refusals.
fn window_skins(session: &Session, format: MapFormat) -> Result<WindowSkins, CyberError> {
    let sill = theme_texture(session, Role::WindowSill)?;
    let wall = theme_wall(session)?;
    Ok(WindowSkins {
        sill: validate_texture_name(&sill, format)?,
        wall: validate_texture_name(&wall, format)?,
    })
}

/// The window rectangle to paint: the cut fitted to the wall (a window
/// too wide for it refuses with the wall's available length, before any
/// drawing), grown across the carved strip or the outward ray's recess
/// exactly as [`place_rect`] does for a door.
fn place_window(map: &Map, chosen: &Chosen, spec: &WindowSpec) -> Result<Vec<Point>, CyberError> {
    let wall = door_wall(map, chosen.wall, Some(chosen.centre), spec.width)?;
    let normal = outward_normal(map, chosen.wall).ok_or_else(|| wall_lost(chosen.wall))?;
    if chosen.touching {
        return Ok(door_rect(&wall, normal, STRIP_THICKNESS, true));
    }
    let depth = outward_depth(map, wall.linedef, wall.point, MAX_DEPTH)
        .map_or_else(|| Err(no_room()), |hit| Ok(hit.0))?;
    let rect = door_rect(&wall, normal, depth, false);
    require_landing(map, &rect)?;
    Ok(rect)
}

/// Paint the window rectangle as the new sector and find the room-facing
/// lines the paint created; the wall must still stand for the strip's
/// near edge to be where [`place_window`] fitted it.
fn draw_window(
    map: &mut Map,
    chosen: &Chosen,
    record: &Sector,
    spec: &WindowSpec,
) -> Result<Drawn, CyberError> {
    let rect = place_window(map, chosen, spec)?;
    let request = OverlayRequest {
        polygon: to_pts(&rect),
        paint: Paint::Sector(record.clone()),
        keep_interior: false,
        textures: WallTextures {
            wall: spec.skins.wall.clone(),
            upper: spec.skins.wall.clone(),
            lower: spec.skins.sill.clone(),
        },
        outside: Outside::Original,
    };
    let drawn = overlay::apply_overlay(map, &request)?;
    require_wall_survives(&drawn, chosen.wall)?;
    let sector = drawn.sector.ok_or_else(window_sector_lost)?;
    Ok(Drawn {
        sector,
        lines: boundary_lines(map, sector),
        notes: drawn.notes,
    })
}

/// The whole window as one map edit: choose the wall, check the opening
/// before anything is written, carve the touching rooms apart, fit and
/// paint the strip, then block off every room-facing line it created.
pub(super) fn build_window(map: &mut Map, spec: &WindowSpec) -> Result<BuiltWindow, CyberError> {
    let mut chosen = choose_wall(map, spec.target, spec.at)?;
    let record = window_record(map, chosen.room_a, chosen.room_b, spec.trim)?;
    let mut notes = Vec::new();
    if chosen.touching {
        notes = carve_strip(map, &chosen, &spec.skins.wall, "the window strip")?;
        let room = chosen.room_a;
        chosen.wall = strip_wall(map, room, chosen.centre).ok_or_else(|| strip_face_lost(room))?;
    }
    let drawn = draw_window(map, &chosen, &record, spec)?;
    notes.extend(drawn.notes);
    let sides = WindowSides {
        window_sector: drawn.sector,
        room_lines: drawn.lines.clone(),
    };
    let textures = WindowTextures {
        sill: spec.skins.sill.clone(),
        wall: spec.skins.wall.clone(),
    };
    apply_window(map, &sides, &textures)?;
    Ok(BuiltWindow {
        window_sector: drawn.sector,
        lines: drawn.lines,
        notes,
    })
}

/// The server-bug refusal for a window paint that reported no sector.
fn window_sector_lost() -> CyberError {
    CyberError::Internal("the window strip paint reported no sector (server bug)".to_owned())
}

/// `add_window`'s work: resolve the parameters, then run the whole window
/// as one [`crate::session::Session::edit_map`] step.
pub(super) fn add_window_run(
    session: &mut Session,
    args: &AddWindowParams,
) -> Result<WindowReport, CyberError> {
    let target = target_of(args.between, args.linedef, "the window looks between")?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = current_map(session, name)?.format;
    let skins = window_skins(session, format)?;
    let trim = WindowHeights {
        sill: args.sill.unwrap_or(DEFAULT_SILL),
        lintel: args.lintel.unwrap_or(DEFAULT_LINTEL),
    };
    let width = args.width.unwrap_or(DEFAULT_WIDTH);
    let spec = WindowSpec {
        target,
        at: args.at.map(point_of),
        width,
        trim,
        skins,
    };
    let (built, report) = session.edit_map(name, "add window", |map| build_window(map, &spec))?;
    Ok(WindowReport {
        built,
        width,
        sill: trim.sill,
        lintel: trim.lintel,
        edit: edit_result_fragment(&report),
    })
}
