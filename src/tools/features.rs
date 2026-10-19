//! Map-feature tools: `add_door`, `add_switch`, `add_exit`, `mark_secret`
//! and `set_light_effect` (see the `features` router on
//! [`crate::server::Cyberdemon`]).
//!
//! The `features` group builds complete game structures on an already
//! drawn map: the [`DoorSpec`] pipeline runs inside one
//! [`crate::session::Session::edit_map`] step, [`theme_skin`] resolves
//! role textures through [`crate::tools::themes::theme_texture`], and
//! the tag a [`DoorStyle::Remote`] door mints with [`next_free_tag`] is
//! exactly what `add_switch` looks for: pointing a switch's
//! `target_sector` at that door closes the remote loop. `add_exit`
//! finishes the level with the same machinery: its `switch` kind hands
//! the shared [`run_switch_step`] body the `exit`/`secret_exit` action,
//! and its `walk_over` kind dresses every two-sided boundary of a
//! sector with the family's walk-over level-end special as one
//! [`build_walk_over_exit`] edit step.
//!
//! `add_switch` composes the same pure engines: [`project_onto_line`]
//! and [`door_segment`] fit the panel to the wall,
//! [`crate::wad::map::ops::split_linedef`]
//! isolates the chunk as its own linedef (skipped when the line already
//! is exactly that chunk), [`switch_special`] is the per-family action
//! table and [`next_free_tag`] mints the tag an untagged target sector
//! answers with.
//!
//! `add_door` composes the pure engines: [`crate::wad::map::features`]
//! selects the wall ([`longest_boundary`], [`nearest_boundary`]), fits
//! the cut ([`door_wall`] through [`door_segment`]), measures a one-sided
//! wall's outward reach ([`outward_depth`], [`point_in_sector`]) and
//! dresses the finished doorway ([`door_specials`], [`apply_door`]), with
//! [`OverlayResult::linedef_map`] confirming the wall survived the
//! rebuild; [`crate::wad::map::overlay`] does every drawing — the void
//! strip that separates touching rooms and the door recess itself are one
//! [`apply_overlay`] paint each, all of it inside a single undoable edit.
//!
//! The two sector-special tools stay on this side of the `features` group
//! because they answer the same question the feature tables answer: what
//! number makes a plain map element do something. Both are one
//! [`crate::session::Session::edit_map`] step over [`Sector::special`]
//! (plus [`Sector::light`] for `set_light_effect`) with no drawing at all:
//! [`secret_edit`] is the per-family secret bit behind `mark_secret`,
//! [`light_special`] the per-family basic type behind `set_light_effect`,
//! and each refused index is named before anything is written.
//!
//! The opening rule: after the recess is drawn, every two-sided line
//! facing the door sector is a room-facing door line (the vanilla
//! multi-room doorway: each opening carries the action and acts on the
//! sector behind it), and every one-sided line facing it is a jamb that
//! [`apply_door`] closes into a floor-pinned track wall.
//!
//! [`longest_boundary`]: crate::wad::map::features::longest_boundary
//! [`nearest_boundary`]: crate::wad::map::features::nearest_boundary
//! [`door_wall`]: crate::wad::map::features::door_wall
//! [`door_segment`]: crate::wad::map::features::door_segment
//! [`outward_depth`]: crate::wad::map::features::outward_depth
//! [`point_in_sector`]: crate::wad::map::geometry::point_in_sector
//! [`door_specials`]: crate::wad::map::features::door_specials
//! [`apply_door`]: crate::wad::map::features::apply_door
//! [`switch_special`]: crate::wad::map::features::switch_special
//! [`next_free_tag`]: crate::wad::map::features::next_free_tag
//! [`apply_overlay`]: crate::wad::map::overlay::apply_overlay
//! [`OverlayResult::linedef_map`]: crate::wad::map::overlay::OverlayResult

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::geom::Pt;
use crate::geom::segment::{Delta, round_quotient};
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{edit_result_fragment, validate_texture_name};
use crate::tools::themes::theme_texture;
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::features::{
    DOOM_SECRET_SPECIAL, DoorLineSetup, DoorSides, DoorStyle, DoorTextures, DoorWall, LightEffect,
    SecretEdit, SwitchAction, VertexPair, apply_door, door_rect, door_segment, door_specials,
    door_wall, light_special, longest_boundary, nearest_boundary, next_free_tag, outward_depth,
    project_onto_line, secret_edit, switch_special, void_strip,
};
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily};
use crate::wad::map::geometry::{linedef_length, point_in_sector};
use crate::wad::map::ops::{self, Point};
use crate::wad::map::overlay::{self, Outside, OverlayRequest, OverlayResult, Paint, WallTextures};
use crate::wad::map::{Linedef, Map, Sector};
use crate::wad::name::LumpName;

/// The `add_door` default door width in map units: one `DOOR1` panel.
const DEFAULT_WIDTH: i32 = 64_i32;
/// The `add_switch` default panel width in map units: one switch face.
const DEFAULT_SWITCH_WIDTH: i32 = 32_i32;
/// The void strip `add_door` carves between touching rooms, in map units.
const STRIP_THICKNESS: i32 = 16_i32;
/// How far the outward ray may search for the room behind a one-sided wall.
const MAX_DEPTH: i32 = 256_i32;
/// The `add_door` default style name.
const DEFAULT_STYLE: &str = "normal";
/// The texture name meaning "nothing here".
const NO_TEXTURE: &str = "-";
/// The wall texture a carved strip's bare edges fall back to.
const DEFAULT_WALL_TEXTURE: &str = "STARTAN3";
/// Divisor turning a four-corner sum into the polygon centroid.
const CENTROID_DIVISOR: i128 = 4_i128;
/// Divisor of the two-endpoint sum that gives a midpoint coordinate.
const MIDPOINT_DIVISOR: i128 = 2_i128;

/// Arguments for [`Cyberdemon::add_door`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddDoorParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The two sectors the door joins, as `[sectorA, sectorB]`: exactly one
    /// of `between` and `linedef`. Sector A lends the recess its heights.
    #[serde(default)]
    between: Option<[usize; 2]>,
    /// The wall to cut, as a linedef index: exactly one of `between` and
    /// `linedef`. A two-sided line behaves like `between` on its two
    /// sectors; a one-sided one opens towards the space its front faces.
    #[serde(default)]
    linedef: Option<usize>,
    /// Where along the wall to centre the cut, `{x, y}` (optional: the cut
    /// centres on the wall's midpoint; `between` picks the shared line
    /// nearest here).
    #[serde(default)]
    at: Option<DoorPointParam>,
    /// The doorway's opening width in map units (default 64).
    #[serde(default)]
    width: Option<i32>,
    /// The recess depth in map units, one-sided walls only (default:
    /// measured by an outward ray, up to 256 units). Ignored for two-sided
    /// walls, whose recess always spans the carved strip.
    #[serde(default)]
    depth: Option<i32>,
    /// The door action (default `"normal"`): `normal`, `fast`, `stay_open`,
    /// `locked_red`, `locked_blue`, `locked_yellow` or `remote`.
    #[serde(default)]
    style: Option<String>,
    /// The moving door face texture (default: the theme's `door`).
    #[serde(default)]
    texture: Option<String>,
    /// The jamb track texture (default: the theme's `door_track`, or its
    /// coloured row for the locked styles).
    #[serde(default)]
    track_texture: Option<String>,
}

/// One `at` point of the feature tools.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, JsonSchema)]
struct DoorPointParam {
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
}

/// Arguments for [`Cyberdemon::add_switch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddSwitchParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The wall to mount the switch on, as a linedef index.
    linedef: usize,
    /// Where along the wall to centre the panel, `{x, y}` (optional: it
    /// centres on the wall's midpoint; a point off the wall is projected
    /// onto it).
    #[serde(default)]
    at: Option<DoorPointParam>,
    /// The switch face's width in map units (default 32).
    #[serde(default)]
    width: Option<i32>,
    /// What the switch does: `open_door`, `raise_floor`, `lower_floor`,
    /// `exit` or `secret_exit`.
    action: String,
    /// The sector the switch acts on (required by the door and floor
    /// actions, ignored by the exits). Its existing tag is the one the
    /// line answers with; an untagged sector gets a fresh one.
    #[serde(default)]
    target_sector: Option<usize>,
    /// Whether the line keeps working after use (default false: the
    /// `S1` specials and plain `playeruse`; true picks the `SR` specials
    /// and adds `repeatspecial`. Ignored by the exit actions).
    #[serde(default)]
    repeatable: bool,
}

/// Arguments for [`Cyberdemon::add_exit`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddExitParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// How the level ends: `switch` (an exit switch on `linedef`) or
    /// `walk_over` (every two-sided boundary of `sector`).
    kind: String,
    /// Whether this is the map's secret exit (default false).
    #[serde(default)]
    secret: bool,
    /// The wall to mount the exit switch on (required by `switch`,
    /// ignored by `walk_over`).
    #[serde(default)]
    linedef: Option<usize>,
    /// The sector whose boundaries trigger the exit (required by
    /// `walk_over`, ignored by `switch`).
    #[serde(default)]
    sector: Option<usize>,
}

/// The wall the parameters name, already checked for "exactly one".
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The shared boundary of two sectors.
    Between {
        /// The sector whose heights the recess copies.
        a: usize,
        /// The sector across the wall.
        b: usize,
    },
    /// One existing linedef.
    Linedef(
        /// The wall line's index.
        usize,
    ),
}

/// The theme-resolved (or explicitly given) textures a door wears.
#[derive(Debug)]
struct DoorSkins {
    /// The moving door face: the room-facing uppers.
    door: String,
    /// The jamb track.
    track: String,
    /// The recess floor flat; `"-"` means keep room A's flat.
    floor_flat: String,
    /// The plain wall texture bare carved edges fall back to.
    wall: String,
}

/// Everything [`build_door`] needs once the wire parameters resolved.
#[derive(Debug)]
struct DoorSpec {
    /// The wall the door is cut into.
    target: Target,
    /// The requested cut centre, if any.
    at: Option<Point>,
    /// The opening width in map units.
    width: i32,
    /// The requested recess depth, if any (one-sided walls only).
    depth: Option<i32>,
    /// The action to install.
    style: DoorStyle,
    /// The textures to dress with.
    skins: DoorSkins,
}

/// The wall the door is cut into, resolved against the map.
#[derive(Debug)]
struct Chosen {
    /// The sector the recess copies its heights from.
    room_a: usize,
    /// The sector across the shared boundary (room A itself when the wall
    /// is one-sided).
    room_b: usize,
    /// Index in [`Map::linedefs`] of the wall line (its pre-carve index
    /// while [`Chosen::touching`]).
    wall: usize,
    /// The cut centre on the wall line.
    centre: Point,
    /// Whether the wall separates two sectors: their whole shared boundary
    /// gets a void strip first.
    touching: bool,
}

/// The door rectangle, ready to paint, with what the drawing needs.
#[derive(Debug)]
struct Placed {
    /// Index of the wall the rectangle's near edge sits on (current map).
    wall: usize,
    /// The room A sector the recess record copies.
    room_a: usize,
    /// The rectangle's corners, in order.
    rect: Vec<Point>,
    /// The tag minted for a [`DoorStyle::Remote`] door's sector.
    tag: Option<i32>,
}

/// The lines around a drawn door recess, split by role.
#[derive(Debug)]
struct Groups {
    /// The two-sided room-facing lines that carry the action.
    lines: Vec<usize>,
    /// The one-sided recess walls that become the tracks.
    jambs: Vec<usize>,
}

/// The drawn recess and its roles, ready for [`dress_door`].
#[derive(Debug)]
struct Dress {
    /// Index of the door sector the recess became.
    door: usize,
    /// The openings and the jambs.
    groups: Groups,
    /// The tag the door's own lines answer with (remote doors).
    tag: i32,
}

/// What one [`build_door`] step did, as the tool reports it.
#[derive(Debug, Serialize)]
struct BuiltDoor {
    /// Index of the door sector the recess became.
    door_sector: usize,
    /// Indices of the room-facing lines carrying the action.
    lines: Vec<usize>,
    /// Indices of the jamb (track) lines.
    jambs: Vec<usize>,
    /// The tag the door sector carries (`remote` doors only).
    tag: Option<i32>,
    /// What the drawings reported, plus the void-strip note.
    notes: Vec<String>,
}

/// Structured result of [`Cyberdemon::add_door`].
#[derive(Serialize)]
struct DoorReport {
    /// What the step built, inlined next to the style and edit fragment.
    #[serde(flatten)]
    built: BuiltDoor,
    /// The canonical style name the door got.
    style: &'static str,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// The refusal for a wall with no room within reach behind it.
fn no_room() -> CyberError {
    invalid(&format!(
        "no room behind this wall within {MAX_DEPTH} units; move the rooms closer or give \
         a larger depth"
    ))
}

/// The `invalid_argument` for `kind` `index` past the map's `count`.
fn out_of_range(kind: &str, index: usize, count: usize) -> CyberError {
    invalid(&format!(
        "{kind} {index} is out of range: the map holds {count} of them; re-read the map \
         and pass current indices"
    ))
}

/// The server-bug refusal for a paint that lost the door wall.
fn wall_lost(wall: usize) -> CyberError {
    CyberError::Internal(format!(
        "the door wall linedef {wall} vanished while the recess was drawn (server bug)"
    ))
}

/// The server-bug refusal for a paint that reported no sector.
fn sector_lost() -> CyberError {
    CyberError::Internal("the door recess paint reported no sector (server bug)".to_owned())
}

/// The canonical wire name of `style`.
const fn style_name(style: DoorStyle) -> &'static str {
    match style {
        DoorStyle::Normal => "normal",
        DoorStyle::Fast => "fast",
        DoorStyle::StayOpen => "stay_open",
        DoorStyle::LockedRed => "locked_red",
        DoorStyle::LockedBlue => "locked_blue",
        DoorStyle::LockedYellow => "locked_yellow",
        DoorStyle::Remote => "remote",
    }
}

/// Every style name, in [`DoorStyle::all`] order.
fn style_names() -> Vec<&'static str> {
    DoorStyle::all().map(style_name).to_vec()
}

/// The [`DoorStyle`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
fn style_of(text: &str) -> Result<DoorStyle, CyberError> {
    let wanted = text.to_ascii_lowercase();
    DoorStyle::all()
        .into_iter()
        .find(|style| style_name(*style) == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "style {text:?} is not a door style: pass one of {}",
                style_names().join(", ")
            ))
        })
}

/// The theme role whose texture dresses `style`'s jambs: the coloured
/// tracks for the locked doors, the plain one for every other style
/// (mirrors [`DoorStyle::track_role_name`]).
const fn track_role(style: DoorStyle) -> Role {
    match style {
        DoorStyle::LockedRed => Role::DoorTrackRed,
        DoorStyle::LockedBlue => Role::DoorTrackBlue,
        DoorStyle::LockedYellow => Role::DoorTrackYellow,
        DoorStyle::Normal | DoorStyle::Fast | DoorStyle::StayOpen | DoorStyle::Remote => {
            Role::DoorTrack
        }
    }
}

/// The `between`/`linedef` parameter pair as one [`Target`]: refuses
/// unless exactly one of the two was passed.
fn target_of(args: &AddDoorParams) -> Result<Target, CyberError> {
    match (args.between, args.linedef) {
        (Some([a, b]), None) => Ok(Target::Between { a, b }),
        (None, Some(line)) => Ok(Target::Linedef(line)),
        _ => Err(invalid(
            "pass exactly one of between ([sectorA, sectorB] for the two rooms the door \
             joins) and linedef (the wall to cut)",
        )),
    }
}

/// The [`Point`] a wire parameter names.
const fn point_of(param: DoorPointParam) -> Point {
    Point {
        x: param.x,
        y: param.y,
    }
}

/// The sector the sidedef `slot` opens into, when sidedef and sector
/// both resolve.
fn side_sector(map: &Map, slot: Option<usize>) -> Option<usize> {
    let sector = slot.and_then(|index| map.sidedefs.get(index))?.sector;
    map.sectors.get(sector).map(|_| sector)
}

/// The two endpoint coordinates of linedef `index`, when both vertices
/// resolve.
fn line_ends(map: &Map, index: usize) -> Option<(Point, Point)> {
    let line = map.linedefs.get(index)?;
    let start = map.vertices.get(line.v1)?;
    let finish = map.vertices.get(line.v2)?;
    Some((
        Point {
            x: start.x,
            y: start.y,
        },
        Point {
            x: finish.x,
            y: finish.y,
        },
    ))
}

/// The `v1` to `v2` vector of linedef `index` as an exact `i128` delta.
fn wall_run(map: &Map, index: usize) -> Option<Delta> {
    let (from, to) = line_ends(map, index)?;
    Some((
        i128::from(to.x).saturating_sub(i128::from(from.x)),
        i128::from(to.y).saturating_sub(i128::from(from.y)),
    ))
}

/// The front-side outward perpendicular `(dy, -dx)` of linedef `index`:
/// the [`outward_depth`] ray direction, pointing at the line's front.
fn outward_normal(map: &Map, index: usize) -> Option<Point> {
    let run = wall_run(map, index)?;
    Some(Point {
        x: narrow(run.1),
        y: narrow(run.0.saturating_neg()),
    })
}

/// An exact `i128` axis saturated into `i32`.
fn narrow(value: i128) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The mean of two coordinates, halves rounded away from zero and
/// saturated back into `i32`.
fn half_axis(first: i32, second: i32) -> i32 {
    let sum = i128::from(first).saturating_add(i128::from(second));
    narrow(round_quotient(sum, MIDPOINT_DIVISOR).unwrap_or(sum))
}

/// The centroid of a polygon: the mean of its corner coordinates.
fn centroid(points: &[Point]) -> Point {
    let mut sum_x = 0_i128;
    let mut sum_y = 0_i128;
    for corner in points {
        sum_x = sum_x.saturating_add(i128::from(corner.x));
        sum_y = sum_y.saturating_add(i128::from(corner.y));
    }
    Point {
        x: narrow(round_quotient(sum_x, CENTROID_DIVISOR).unwrap_or(sum_x)),
        y: narrow(round_quotient(sum_y, CENTROID_DIVISOR).unwrap_or(sum_y)),
    }
}

/// The squared distance from `from` to the midpoint of linedef `index`,
/// `None` when its vertices do not resolve.
fn mid_distance_sq(map: &Map, index: usize, from: Point) -> Option<i128> {
    let (a, b) = line_ends(map, index)?;
    let dx = i128::from(half_axis(a.x, b.x)).saturating_sub(i128::from(from.x));
    let dy = i128::from(half_axis(a.y, b.y)).saturating_sub(i128::from(from.y));
    Some(dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)))
}

/// Whether the line's front faces `room` and nothing sits behind it: a
/// one-sided wall of that room.
fn room_face(map: &Map, line: &Linedef, room: usize) -> bool {
    side_sector(map, line.front) == Some(room) && side_sector(map, line.back).is_none()
}

/// The door wall for `between` sectors: their shared boundary's nearest
/// line to `at` (else its longest), with both sector indices validated.
fn choose_between(map: &Map, a: usize, b: usize, at: Option<Point>) -> Result<Chosen, CyberError> {
    if map.sectors.get(a).is_none() {
        return Err(out_of_range("sector", a, map.sectors.len()));
    }
    if map.sectors.get(b).is_none() {
        return Err(out_of_range("sector", b, map.sectors.len()));
    }
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary: draw the rooms touching (or \
             give the door wall's linedef index); get_map_summary lists each sector's \
             neighbours"
        ))
    })?;
    let line = at
        .map_or_else(
            || longest_line(map, &boundary),
            |point| nearest_boundary(map, a, b, point),
        )
        .ok_or_else(|| {
            invalid(&format!(
                "no line of the shared boundary of sectors {a} and {b} has resolvable \
                 vertices: repair the dangling vertex references first"
            ))
        })?;
    Ok(Chosen {
        room_a: a,
        room_b: b,
        wall: line,
        centre: project_onto_line(map, line, at),
        touching: true,
    })
}

/// The longest line of `boundary` by floored integer length, falling
/// back to the lowest index when none resolves.
fn longest_line(map: &Map, boundary: &[usize]) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for index in boundary {
        let Some(length) = linedef_length(map, *index) else {
            continue;
        };
        if best.is_none_or(|(known, _)| length > known) {
            best = Some((length, *index));
        }
    }
    best.map(|(_, index)| index)
        .or_else(|| boundary.iter().copied().next())
}

/// The door wall for an explicit `linedef`: two-sided lines behave like
/// [`choose_between`] on their own sectors (honouring the given wall),
/// one-sided ones open towards the space their front faces.
fn choose_linedef(map: &Map, line: usize, at: Option<Point>) -> Result<Chosen, CyberError> {
    let Some(def) = map.linedefs.get(line) else {
        return Err(out_of_range("linedef", line, map.linedefs.len()));
    };
    match (side_sector(map, def.front), side_sector(map, def.back)) {
        (Some(a), Some(b)) if a != b => Ok(Chosen {
            room_a: a,
            room_b: b,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: true,
        }),
        (Some(a), Some(_)) => Err(invalid(&format!(
            "both sides of linedef {line} face sector {a}: a door wall separates two \
             sectors or bounds the space behind it, not one sector twice"
        ))),
        (Some(a), None) => Ok(Chosen {
            room_a: a,
            room_b: a,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: false,
        }),
        (None, Some(b)) => Err(invalid(&format!(
            "linedef {line} has only a back side (sector {b}): flip it or give it a \
             front sidedef before cutting a door into it (see add_sidedef)"
        ))),
        (None, None) => Err(invalid(&format!(
            "linedef {line} has no usable sidedef: a door wall needs a front side \
             facing its room (see add_sidedef)"
        ))),
    }
}

/// The wall the parameters pick, validated against `map`.
fn choose_wall(map: &Map, spec: &DoorSpec) -> Result<Chosen, CyberError> {
    match spec.target {
        Target::Between { a, b } => choose_between(map, a, b, spec.at),
        Target::Linedef(line) => choose_linedef(map, line, spec.at),
    }
}

/// The [`OverlayRequest`] carving `polygon` out as solid rock, dressing
/// bare sectors' new walls with `wall`.
fn void_request(polygon: &[Point], wall: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: to_pts(polygon),
        paint: Paint::Void,
        keep_interior: false,
        textures: WallTextures {
            wall: wall.to_owned(),
            upper: wall.to_owned(),
            lower: wall.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// A [`Point`] list widened to the arrangement's `i64` grid.
fn to_pts(points: &[Point]) -> Vec<Pt> {
    points
        .iter()
        .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
        .collect()
}

/// Carve the void strip along the whole shared boundary of the chosen
/// wall's sectors, and report the notes the carve left.
///
/// Touching rooms would share their wall with the door recess itself: the
/// strip (thickness [`STRIP_THICKNESS`]) turns them apart so the recess
/// has a real space to slide in.
fn carve_strip(
    map: &mut Map,
    chosen: &Chosen,
    skins: &DoorSkins,
) -> Result<Vec<String>, CyberError> {
    let (a, b) = (chosen.room_a, chosen.room_b);
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary to carve; re-read the map \
             and pass current indices"
        ))
    })?;
    let strip = void_strip(map, &boundary, STRIP_THICKNESS)?;
    let drawn = overlay::apply_overlay(map, &void_request(&strip.polygon, &skins.wall))?;
    let mut notes = vec![format!(
        "carved a {STRIP_THICKNESS}-unit void strip along the shared boundary of sectors \
         {a} and {b}: the door recess spans it"
    )];
    notes.extend(drawn.notes);
    Ok(notes)
}

/// The room's wall along a freshly carved strip: the one-sided wall of
/// `room` whose midpoint sits nearest `near`; `None` when the carve left
/// the room without a face to it.
fn strip_wall(map: &Map, room: usize, near: Point) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for (index, line) in map.linedefs.iter().enumerate() {
        if !room_face(map, line, room) {
            continue;
        }
        let Some(square) = mid_distance_sq(map, index, near) else {
            continue;
        };
        if best.is_none_or(|(known, _)| square < known) {
            best = Some((square, index));
        }
    }
    best.map(|(_, index)| index)
}

/// The refusal when the strip carve left room `room` unfaceable.
fn strip_face_lost(room: usize) -> CyberError {
    invalid(&format!(
        "sector {room} has no wall facing the carved strip: the void carve removed its \
         boundary; check the two sectors' shared boundary and re-read the map"
    ))
}

/// The recess depth behind a one-sided wall: the explicit `depth`, or
/// the distance the outward ray measures to the next sector's wall.
fn recess_depth(map: &Map, wall: &DoorWall, spec: &DoorSpec) -> Result<i32, CyberError> {
    match spec.depth {
        Some(depth) if depth > 0_i32 => Ok(depth),
        Some(_) => Err(invalid(
            "the door depth must be a positive number of map units (the recess the \
             door slides into)",
        )),
        None => outward_depth(map, wall.linedef, wall.point, MAX_DEPTH)
            .map_or_else(|| Err(no_room()), |hit| Ok(hit.0)),
    }
}

/// Refuse a one-sided recess whose far middle falls in the void: only a
/// recess that lands on (or inside) a sector can ever act as a door.
fn require_landing(map: &Map, rect: &[Point]) -> Result<(), CyberError> {
    let middle = centroid(rect);
    if point_in_sector(map, i64::from(middle.x), i64::from(middle.y)).is_some() {
        return Ok(());
    }
    Err(no_room())
}

/// The rectangle to paint: the cut fitted to the wall (a door too wide
/// for it refuses with the wall's available length, before any drawing),
/// grown to the strip's thickness or the measured recess depth.
fn place_rect(map: &Map, chosen: &Chosen, spec: &DoorSpec) -> Result<Placed, CyberError> {
    let wall = door_wall(map, chosen.wall, Some(chosen.centre), spec.width)?;
    let normal = outward_normal(map, chosen.wall).ok_or_else(|| wall_lost(chosen.wall))?;
    let (depth, grown_back) = if chosen.touching {
        (STRIP_THICKNESS, true)
    } else {
        (recess_depth(map, &wall, spec)?, false)
    };
    let rect = door_rect(&wall, normal, depth, grown_back);
    if !chosen.touching {
        require_landing(map, &rect)?;
    }
    Ok(Placed {
        wall: chosen.wall,
        room_a: chosen.room_a,
        rect,
        tag: None,
    })
}

/// The door sector record: room A's fields, the theme's recess floor
/// (room A's flat when the theme names none), no effect and the minted
/// tag.
fn door_record(map: &Map, placed: &Placed, spec: &DoorSpec) -> Sector {
    let mut record = map.sectors.get(placed.room_a).cloned().unwrap_or_default();
    record.special = 0_i32;
    record.tag = placed.tag.unwrap_or_default();
    if spec.skins.floor_flat != NO_TEXTURE {
        record.floor_flat.clone_from(&spec.skins.floor_flat);
    }
    record
}

/// Paint the door rectangle as the new sector.
fn draw_recess(
    map: &mut Map,
    placed: &Placed,
    spec: &DoorSpec,
) -> Result<OverlayResult, CyberError> {
    let request = OverlayRequest {
        polygon: to_pts(&placed.rect),
        paint: Paint::Sector(door_record(map, placed, spec)),
        keep_interior: false,
        textures: WallTextures {
            wall: spec.skins.track.clone(),
            upper: spec.skins.door.clone(),
            lower: spec.skins.door.clone(),
        },
        outside: Outside::Original,
    };
    overlay::apply_overlay(map, &request)
}

/// What a linedef is to the door sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Border {
    /// Two-sided, and one side opens into the door: a room-facing line
    /// that carries the action.
    Opening,
    /// Bounding the door with nothing resolvable on the far side: the
    /// recess's side wall, closed into a jamb.
    Jamb,
    /// Not a boundary of the door sector at all.
    Elsewhere,
}

/// Classify `line` against the door sector `door`.
fn door_border(map: &Map, line: &Linedef, door: usize) -> Border {
    let front = side_sector(map, line.front);
    let back = side_sector(map, line.back);
    match (front == Some(door), back == Some(door)) {
        (true, false) | (false, true) if front.is_some() && back.is_some() => Border::Opening,
        (true, false) | (false, true) => Border::Jamb,
        (true, true) | (false, false) => Border::Elsewhere,
    }
}

/// Split every line facing the door sector: two-sided ones (a real
/// sector on both sides) are the room-facing openings that carry the
/// action, the rest are the recess's side walls, closed into jambs
/// (indices ascending, in linedef order).
fn door_groups(map: &Map, door: usize) -> Groups {
    let mut groups = Groups {
        lines: Vec::new(),
        jambs: Vec::new(),
    };
    for (index, line) in map.linedefs.iter().enumerate() {
        match door_border(map, line, door) {
            Border::Opening => groups.lines.push(index),
            Border::Jamb => groups.jambs.push(index),
            Border::Elsewhere => {}
        }
    }
    groups
}

/// The door wall must still stand (as its first piece) after the paint:
/// [`OverlayResult::linedef_map`] remaps it into the rebuilt map.
fn require_wall_survives(drawn: &OverlayResult, wall: usize) -> Result<(), CyberError> {
    if drawn.linedef_map.get(wall).copied().flatten().is_some() {
        return Ok(());
    }
    Err(wall_lost(wall))
}

/// Install the style's action and the dressing on the drawn doorway.
fn dress_door(map: &mut Map, spec: &DoorSpec, dress: &Dress) -> Result<(), CyberError> {
    let setup = door_specials(map.flag_family(), spec.style, dress.tag);
    let sides = DoorSides {
        door_sector: dress.door,
        room_lines: dress.groups.lines.clone(),
        jamb_lines: dress.groups.jambs.clone(),
    };
    let textures = DoorTextures {
        door: spec.skins.door.clone(),
        track: spec.skins.track.clone(),
        floor_flat: door_floor(map, dress.door),
    };
    apply_door(map, &sides, &setup, &textures)
}

/// The recess floor as drawn (room A's flat when the theme named none).
fn door_floor(map: &Map, door: usize) -> String {
    map.sectors
        .get(door)
        .map_or_else(|| NO_TEXTURE.to_owned(), |sector| sector.floor_flat.clone())
}

/// The whole door as one map edit: choose the wall, carve the touching
/// rooms apart, fit and paint the recess, split its boundary into doors
/// and jambs, and dress it with the style's action.
fn build_door(map: &mut Map, spec: &DoorSpec) -> Result<BuiltDoor, CyberError> {
    let mut chosen = choose_wall(map, spec)?;
    let mut notes = Vec::new();
    if chosen.touching {
        notes = carve_strip(map, &chosen, &spec.skins)?;
        let room = chosen.room_a;
        chosen.wall = strip_wall(map, room, chosen.centre).ok_or_else(|| strip_face_lost(room))?;
    }
    let mut placed = place_rect(map, &chosen, spec)?;
    placed.tag = (spec.style == DoorStyle::Remote).then(|| next_free_tag(map));
    let drawn = draw_recess(map, &placed, spec)?;
    require_wall_survives(&drawn, placed.wall)?;
    let door = drawn.sector.ok_or_else(sector_lost)?;
    notes.extend(drawn.notes);
    let dress = Dress {
        door,
        groups: door_groups(map, door),
        tag: placed.tag.unwrap_or_default(),
    };
    dress_door(map, spec, &dress)?;
    Ok(BuiltDoor {
        door_sector: dress.door,
        lines: dress.groups.lines,
        jambs: dress.groups.jambs,
        tag: placed.tag,
        notes,
    })
}

/// The wall texture a carved strip's bare edges fall back to: the
/// session theme's `Wall`, else [`DEFAULT_WALL_TEXTURE`].
fn theme_wall(session: &Session) -> Result<String, CyberError> {
    let themed = theme_texture(session, Role::Wall)?;
    Ok(if themed == NO_TEXTURE {
        DEFAULT_WALL_TEXTURE.to_owned()
    } else {
        themed
    })
}

/// One skin entry: the explicit name when given, else the theme's,
/// validated for the map's format.
fn skin(given: Option<&str>, themed: &str, format: MapFormat) -> Result<String, CyberError> {
    validate_texture_name(given.unwrap_or(themed), format)
}

/// The textures the door wears: theme roles by default (the coloured
/// track for the locked styles), explicit `texture`/`track_texture`
/// overrides validated for the map's format.
///
/// # Errors
///
/// [`theme_texture`]'s errors and [`validate_texture_name`]' refusals.
fn theme_skin(
    session: &Session,
    style: DoorStyle,
    args: &AddDoorParams,
    format: MapFormat,
) -> Result<DoorSkins, CyberError> {
    let door = theme_texture(session, Role::Door)?;
    let track = theme_texture(session, track_role(style))?;
    let floor = theme_texture(session, Role::DoorFloor)?;
    let wall = theme_wall(session)?;
    Ok(DoorSkins {
        door: skin(args.texture.as_deref(), &door, format)?,
        track: skin(args.track_texture.as_deref(), &track, format)?,
        floor_flat: skin(None, &floor, format)?,
        wall: skin(None, &wall, format)?,
    })
}

/// The map as the next edit sees it: the working archive's block when it
/// holds one, else the merged view's (which is what the edit copies in).
///
/// # Errors
///
/// [`common::load_map`]'s errors for the merged scope when the working
/// archive holds no such block.
fn current_map(session: &Session, name: LumpName) -> Result<Map, CyberError> {
    match common::load_map(session, name, Scope::Working) {
        Ok(map) => Ok(map),
        Err(CyberError::NotFound(_)) => common::load_map(session, name, Scope::Merged),
        Err(error) => Err(error),
    }
}

/// `add_door`'s work: resolve the parameters, then run the whole door as
/// one [`crate::session::Session::edit_map`] step.
fn add_door_run(session: &mut Session, args: &AddDoorParams) -> Result<DoorReport, CyberError> {
    let style = style_of(args.style.as_deref().unwrap_or(DEFAULT_STYLE))?;
    let target = target_of(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = current_map(session, name)?.format;
    let skins = theme_skin(session, style, args, format)?;
    let spec = DoorSpec {
        target,
        at: args.at.map(point_of),
        width: args.width.unwrap_or(DEFAULT_WIDTH),
        depth: args.depth,
        style,
        skins,
    };
    let label = format!("add {} door", style_name(style));
    let (built, report) = session.edit_map(name, &label, |map| build_door(map, &spec))?;
    Ok(DoorReport {
        built,
        style: style_name(style),
        edit: edit_result_fragment(&report),
    })
}

/// The canonical wire name of `action`.
const fn action_name(action: SwitchAction) -> &'static str {
    match action {
        SwitchAction::OpenDoor => "open_door",
        SwitchAction::Exit => "exit",
        SwitchAction::SecretExit => "secret_exit",
        SwitchAction::RaiseFloor => "raise_floor",
        SwitchAction::LowerFloor => "lower_floor",
    }
}

/// Every action name, in [`SwitchAction::all`] order.
fn action_names() -> Vec<&'static str> {
    SwitchAction::all().map(action_name).to_vec()
}

/// The [`SwitchAction`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
fn action_of(text: &str) -> Result<SwitchAction, CyberError> {
    let wanted = text.to_ascii_lowercase();
    SwitchAction::all()
        .into_iter()
        .find(|action| action_name(*action) == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "action {text:?} is not a switch action: pass one of {}",
                action_names().join(", ")
            ))
        })
}

/// The texture theme role the switch panel wears: the exit switch for
/// the level-changing actions, the plain panel for the sector-moving
/// ones.
const fn switch_role(action: SwitchAction) -> Role {
    match action {
        SwitchAction::Exit | SwitchAction::SecretExit => Role::ExitSwitch,
        SwitchAction::OpenDoor | SwitchAction::RaiseFloor | SwitchAction::LowerFloor => {
            Role::Switch
        }
    }
}

/// Everything [`build_switch`] needs once the wire parameters resolved.
#[derive(Debug)]
struct SwitchSpec {
    /// The wall to mount the panel on.
    linedef: usize,
    /// The requested panel centre, if any.
    at: Option<Point>,
    /// The panel width in map units.
    width: i32,
    /// The action to install.
    action: SwitchAction,
    /// The sector the switch acts on (sector-moving actions).
    target_sector: Option<usize>,
    /// Whether the line keeps working after use.
    repeatable: bool,
    /// The theme-resolved panel texture.
    texture: String,
}

/// The [`Map`] endpoints of the wall switch `linedef`, refused with
/// switch-role instructions when the index dangles or a vertex is gone
/// ([`door_segment`] speaks of doors, so the wall is checked first).
fn switch_span(map: &Map, index: usize) -> Result<(Point, Point), CyberError> {
    let count = map.linedefs.len();
    if index >= count {
        return Err(out_of_range("linedef", index, count));
    }
    line_ends(map, index).ok_or_else(|| {
        invalid(&format!(
            "switch linedef {index} references a vertex the map does not hold: resolve \
             its endpoints first"
        ))
    })
}

/// The `width`-wide panel chunk of the wall, centred on `at` (or the
/// midpoint): [`project_onto_line`] and [`door_segment`] on a wall
/// already checked for [`switch_span`], so only the width, zero-length
/// and fit refusals can reach the caller.
fn switch_cut(map: &Map, spec: &SwitchSpec) -> Result<(VertexPair, i32), CyberError> {
    switch_span(map, spec.linedef)?;
    if spec.width <= 0_i32 {
        return Err(invalid(
            "the switch width must be a positive number of map units (the default 32 \
             covers one switch panel)",
        ));
    }
    let centre = project_onto_line(map, spec.linedef, spec.at);
    door_segment(map, spec.linedef, centre, spec.width)
}

/// The isolated panel chunk: which linedef carries it and whether the
/// wall had to be cut.
#[derive(Debug)]
struct Chunk {
    /// Index in [`Map::linedefs`] of the switch line.
    line: usize,
    /// Whether [`build_switch`] split the wall to isolate the chunk.
    split: bool,
}

/// Make the `start`-`finish` chunk of linedef `linedef` its own linedef
/// with [`ops::split_linedef`], cutting only where the chunk does not
/// already meet an endpoint; the chunk keeps the wall's sides (split
/// copies them) and the pieces beyond its ends stand aside.
fn isolate_chunk(
    map: &mut Map,
    linedef: usize,
    start: Point,
    finish: Point,
) -> Result<Chunk, CyberError> {
    let (from, to) = switch_span(map, linedef)?;
    let mut chunk = Chunk {
        line: linedef,
        split: false,
    };
    if start != from {
        chunk.line = ops::split_linedef(map, chunk.line, Some(start))?.new_linedef;
        chunk.split = true;
    }
    if finish != to {
        ops::split_linedef(map, chunk.line, Some(finish))?;
        chunk.split = true;
    }
    Ok(chunk)
}

/// The refusal for a sector-moving action that names no target sector.
fn target_required(action: SwitchAction) -> CyberError {
    invalid(&format!(
        "the {} action needs target_sector: pass the index of the sector the switch \
         moves (add_door reports the door sector of its remote door)",
        action_name(action)
    ))
}

/// The tag the switch line answers with: the target sector's existing
/// one, or a fresh [`next_free_tag`] stamped onto the sector when it
/// carries none. Exit actions level-end and act on no tag.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a sector-moving action names no
/// `target_sector` or an index past the map's sectors.
fn switch_tag(map: &mut Map, spec: &SwitchSpec) -> Result<i32, CyberError> {
    if !spec.action.needs_target() {
        return Ok(0_i32);
    }
    let index = spec
        .target_sector
        .ok_or_else(|| target_required(spec.action))?;
    let current = map
        .sectors
        .get(index)
        .ok_or_else(|| out_of_range("sector", index, map.sectors.len()))?
        .tag;
    if current != 0_i32 {
        return Ok(current);
    }
    let fresh = next_free_tag(map);
    if let Some(sector) = map.sectors.get_mut(index) {
        sector.tag = fresh;
    }
    Ok(fresh)
}

/// The front sidedef index of the switch `line`, when the line has one
/// and the sidedef resolves.
fn switch_side(map: &Map, line: usize) -> Option<usize> {
    map.linedefs
        .get(line)?
        .front
        .filter(|index| map.sidedefs.get(*index).is_some())
}

/// Write the action onto the isolated line and the panel onto its front
/// side; the `Hexen` family replaces the activation field with the
/// setup's `playeruse` bits, the `Doom` family keeps the line's flags.
fn dress_switch(
    map: &mut Map,
    line: usize,
    setup: &DoorLineSetup,
    texture: &str,
) -> Result<(), CyberError> {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let Some(side) = switch_side(map, line) else {
        return Err(invalid(&format!(
            "linedef {line} has no front sidedef to wear the switch: give the wall a \
             front side facing the triggering room first (see add_sidedef)"
        )));
    };
    if let Some(def) = map.linedefs.get_mut(line) {
        def.special = setup.special;
        def.tag = setup.tag;
        def.args = setup.args;
        if hexen {
            def.flags = (def.flags & !ACTIVATION_MASK) | setup.flags_activation;
        }
    }
    if let Some(worn) = map.sidedefs.get_mut(side) {
        texture.clone_into(&mut worn.middle);
    }
    Ok(())
}

/// What one [`build_switch`] step did, as the tool reports it.
#[derive(Debug, Serialize)]
struct PlacedSwitch {
    /// Index of the linedef carrying the switch (new when split).
    linedef: usize,
    /// Whether the wall was split to isolate the panel.
    split: bool,
    /// The canonical action name the switch got.
    action: &'static str,
    /// The line special installed.
    special: i32,
    /// The tag the line answers with (`0` for the exit actions).
    tag: i32,
    /// The panel texture installed on the front middle.
    texture: String,
}

/// Structured result of [`Cyberdemon::add_switch`].
#[derive(Serialize)]
struct SwitchReport {
    /// What the step installed, inlined next to the edit fragment.
    #[serde(flatten)]
    placed: PlacedSwitch,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The whole switch as one map edit: fit the chunk, isolate it, resolve
/// the tag it answers with and dress the line with the table's action.
fn build_switch(map: &mut Map, spec: &SwitchSpec) -> Result<PlacedSwitch, CyberError> {
    let ((start, finish), _available) = switch_cut(map, spec)?;
    let chunk = isolate_chunk(map, spec.linedef, start, finish)?;
    let tag = switch_tag(map, spec)?;
    let setup = switch_special(map.flag_family(), spec.action, spec.repeatable, tag);
    dress_switch(map, chunk.line, &setup, &spec.texture)?;
    Ok(PlacedSwitch {
        linedef: chunk.line,
        split: chunk.split,
        action: action_name(spec.action),
        special: setup.special,
        tag,
        texture: spec.texture.clone(),
    })
}

/// The shared switch step's outcome: what was placed and the standard
/// `map`/`nodes_invalidated`/`copied_to_working` edit fragment.
type Switched = (PlacedSwitch, JsonMap<String, Value>);

/// The body both switch-mounting tools share: resolve the map, the
/// format-validated panel texture and the target tag, then run the
/// whole switch as one [`crate::session::Session::edit_map`] step.
fn run_switch_step(
    session: &mut Session,
    args: &AddSwitchParams,
    action: SwitchAction,
) -> Result<Switched, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let format = current_map(session, name)?.format;
    let texture = validate_texture_name(&theme_texture(session, switch_role(action))?, format)?;
    let spec = SwitchSpec {
        linedef: args.linedef,
        at: args.at.map(point_of),
        width: args.width.unwrap_or(DEFAULT_SWITCH_WIDTH),
        action,
        target_sector: args.target_sector,
        repeatable: args.repeatable,
        texture,
    };
    let label = format!("add {} switch", action_name(action));
    let (placed, report) = session.edit_map(name, &label, |map| build_switch(map, &spec))?;
    Ok((placed, edit_result_fragment(&report)))
}

/// `add_switch`'s work: parse the action and run the shared body.
fn add_switch_run(
    session: &mut Session,
    args: &AddSwitchParams,
) -> Result<SwitchReport, CyberError> {
    let action = action_of(&args.action)?;
    let (placed, edit) = run_switch_step(session, args, action)?;
    Ok(SwitchReport { placed, edit })
}

/// How [`Cyberdemon::add_exit`] triggers the level end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitKind {
    /// A wall switch carrying the exit action (the shared switch body).
    Switch,
    /// Every two-sided boundary of a sector, crossed by the player.
    WalkOver,
}

/// The canonical wire name of `kind`.
const fn kind_name(kind: ExitKind) -> &'static str {
    match kind {
        ExitKind::Switch => "switch",
        ExitKind::WalkOver => "walk_over",
    }
}

/// The [`ExitKind`] `text` names (case-insensitive), or the refusal
/// listing both.
fn kind_of(text: &str) -> Result<ExitKind, CyberError> {
    let wanted = text.to_ascii_lowercase();
    [ExitKind::Switch, ExitKind::WalkOver]
        .into_iter()
        .find(|kind| kind_name(*kind) == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "kind {text:?} is not an exit kind: pass \"switch\" (an exit switch on \
                 linedef) or \"walk_over\" (every two-sided boundary of sector)"
            ))
        })
}

/// The walk-over exit special flag family `family` installs for a
/// normal or `secret` level end: the `Doom` family's `W1` 52/124, the
/// `Hexen` family's portal 243/244.
const fn walk_over_special(family: FlagFamily, secret: bool) -> i32 {
    match family {
        FlagFamily::Doom => {
            if secret {
                124_i32
            } else {
                52_i32
            }
        }
        FlagFamily::Hexen => {
            if secret {
                244_i32
            } else {
                243_i32
            }
        }
    }
}

/// Whether `line` is two-sided (both sides resolving to sectors) with
/// `sector` opening on either of them.
fn sector_boundary(map: &Map, line: &Linedef, sector: usize) -> bool {
    let front = side_sector(map, line.front);
    let back = side_sector(map, line.back);
    front.is_some() && back.is_some() && (front == Some(sector) || back == Some(sector))
}

/// The lines [`build_walk_over_exit`] dressed and the special they carry.
#[derive(Debug)]
struct WalkOverExit {
    /// The level-end special installed on every boundary line.
    special: i32,
    /// Indices of the two-sided lines that now end the level.
    lines: Vec<usize>,
}

/// Set one boundary line's level-end special; in the `Hexen` family
/// clear its activation field to the `playercross` value (the field's
/// zero), keeping the other flag bits, the tag and the args.
fn mark_walk_over(map: &mut Map, index: usize, special: i32, hexen: bool) {
    if let Some(line) = map.linedefs.get_mut(index) {
        line.special = special;
        if hexen {
            line.flags &= !ACTIVATION_MASK;
        }
    }
}

/// Dress every two-sided boundary of `sector` with the family's
/// walk-over level end. The `Doom` family's `W1` 52/124 read the line's
/// tag as the destination map number, so existing tags stay as they are
/// (a plain 0 ends the game, or the secret level on 124).
fn build_walk_over_exit(
    map: &mut Map,
    sector: usize,
    secret: bool,
) -> Result<WalkOverExit, CyberError> {
    if map.sectors.get(sector).is_none() {
        return Err(out_of_range("sector", sector, map.sectors.len()));
    }
    let special = walk_over_special(map.flag_family(), secret);
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let lines: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| sector_boundary(map, line, sector))
        .map(|(index, _line)| index)
        .collect();
    if lines.is_empty() {
        return Err(invalid(&format!(
            "sector {sector} has no two-sided boundary to walk over: draw a shared wall \
             to the next sector (add_door, or create_sector_from_loop overlapping it) or \
             trigger the exit with kind \"switch\" instead"
        )));
    }
    for index in &lines {
        mark_walk_over(map, *index, special, hexen);
    }
    Ok(WalkOverExit { special, lines })
}

/// Structured result of [`Cyberdemon::add_exit`].
#[derive(Serialize)]
struct ExitReport {
    /// The canonical trigger name: `switch` or `walk_over`.
    kind: &'static str,
    /// Indices of the linedefs carrying the level-end action.
    lines: Vec<usize>,
    /// The line special installed on them.
    special: i32,
    /// Whether this is the map's secret exit.
    secret: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The `switch` kind: the shared [`run_switch_step`] body with the
/// exit action on the required `linedef`.
fn exit_via_switch(session: &mut Session, args: &AddExitParams) -> Result<ExitReport, CyberError> {
    let Some(linedef) = args.linedef else {
        return Err(invalid(
            "kind \"switch\" requires linedef: the index of the wall to mount the exit \
             switch on (assemble_map lists the linedefs)",
        ));
    };
    let action = if args.secret {
        SwitchAction::SecretExit
    } else {
        SwitchAction::Exit
    };
    let wire = AddSwitchParams {
        map: args.map.clone(),
        linedef,
        at: None,
        width: None,
        action: action_name(action).to_owned(),
        target_sector: None,
        repeatable: false,
    };
    let (placed, edit) = run_switch_step(session, &wire, action)?;
    Ok(ExitReport {
        kind: kind_name(ExitKind::Switch),
        lines: vec![placed.linedef],
        special: placed.special,
        secret: args.secret,
        edit,
    })
}

/// The `walk_over` kind: one [`build_walk_over_exit`] edit step over
/// the sector's boundaries.
fn exit_via_walkover(
    session: &mut Session,
    args: &AddExitParams,
) -> Result<ExitReport, CyberError> {
    let Some(sector) = args.sector else {
        return Err(invalid(
            "kind \"walk_over\" requires sector: the index of the sector whose two-sided \
             boundaries the player crosses to end the level (get_map_summary lists the \
             sectors)",
        ));
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = if args.secret {
        "add secret walk_over exit"
    } else {
        "add walk_over exit"
    };
    let secret = args.secret;
    let (built, report) =
        session.edit_map(name, label, |map| build_walk_over_exit(map, sector, secret))?;
    Ok(ExitReport {
        kind: kind_name(ExitKind::WalkOver),
        lines: built.lines,
        special: built.special,
        secret,
        edit: edit_result_fragment(&report),
    })
}

/// `add_exit`'s work: parse the trigger kind and run its path, each a
/// single [`crate::session::Session::edit_map`] step.
fn add_exit_run(session: &mut Session, args: &AddExitParams) -> Result<ExitReport, CyberError> {
    match kind_of(&args.kind)? {
        ExitKind::Switch => exit_via_switch(session, args),
        ExitKind::WalkOver => exit_via_walkover(session, args),
    }
}

/// The lowest light level the vanilla range allows.
const LIGHT_MIN: i32 = 0_i32;
/// The highest light level the vanilla range allows (`u8::MAX`).
const LIGHT_MAX: i32 = 255_i32;

/// Arguments for [`Cyberdemon::mark_secret`].
#[derive(Debug, Deserialize, JsonSchema)]
struct MarkSecretParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The sectors to change, as a non-empty list of indices (duplicates
    /// collapse, the edit runs in ascending order).
    sectors: Vec<usize>,
    /// Whether the sectors are secret (default true); `false` clears the
    /// flag instead of setting it.
    #[serde(default = "default_secret")]
    secret: bool,
}

/// Default [`MarkSecretParams::secret`]: marking, not clearing.
const fn default_secret() -> bool {
    true
}

/// Arguments for [`Cyberdemon::set_light_effect`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetLightParams {
    /// The map to edit; defaults like [`MarkSecretParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The sectors to change, as a non-empty list of indices (duplicates
    /// collapse, the edit runs in ascending order).
    sectors: Vec<usize>,
    /// The light effect: `none`, `flicker`, `strobe_fast`, `strobe_slow`,
    /// `glow` or `random_off`.
    effect: String,
    /// The static light level to give the sectors, `0..=255` (optional:
    /// without it only the effect changes).
    #[serde(default)]
    light: Option<i32>,
}

/// The sectors a special tool was asked to change: ascending, without
/// duplicates and never empty.
fn sector_selection(list: &[usize]) -> Result<Vec<usize>, CyberError> {
    if list.is_empty() {
        return Err(invalid(
            "sectors is empty: pass at least one sector index (get_map_summary lists the \
             map's sectors)",
        ));
    }
    let mut indices = list.to_vec();
    indices.sort_unstable();
    indices.dedup();
    Ok(indices)
}

/// Refuses a light level outside the vanilla `0..=255`, every format alike.
fn check_light_level(level: i32) -> Result<(), CyberError> {
    if (LIGHT_MIN..=LIGHT_MAX).contains(&level) {
        return Ok(());
    }
    Err(invalid(&format!(
        "light {level} is out of range: the light level holds {LIGHT_MIN}..={LIGHT_MAX}"
    )))
}

/// The [`LightEffect`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
fn effect_of(text: &str) -> Result<LightEffect, CyberError> {
    let wanted = text.to_ascii_lowercase();
    LightEffect::all()
        .into_iter()
        .find(|effect| effect.name() == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "effect {text:?} is not a light effect: pass one of {}",
                effect_names().join(", ")
            ))
        })
}

/// Every effect name, in [`LightEffect::all`] order.
fn effect_names() -> Vec<&'static str> {
    LightEffect::all().map(LightEffect::name).to_vec()
}

/// Everything [`mark_secrets`] needs once the wire parameters resolved.
#[derive(Debug)]
struct SecretSpec {
    /// The sectors to change, ascending and deduplicated.
    indices: Vec<usize>,
    /// Whether they become secret or plain again.
    secret: bool,
}

/// What one [`mark_secrets`] step wrote.
#[derive(Debug)]
struct SecretStep {
    /// Indices of the sectors whose `special` the step changed.
    changed: Vec<usize>,
    /// The Boom-bit notes for sectors a vanilla engine cannot read.
    warnings: Vec<String>,
    /// The flag family whose bits were written.
    family: &'static str,
}

/// Everything [`set_lights`] needs once the wire parameters resolved.
#[derive(Debug)]
struct LightSpec {
    /// The sectors to change, ascending and deduplicated.
    indices: Vec<usize>,
    /// The effect to install.
    effect: LightEffect,
    /// The light level to give them, when one was asked for.
    light: Option<i32>,
}

/// What one [`set_lights`] step wrote.
#[derive(Debug)]
struct LightStep {
    /// Indices of the sectors whose special or light the step changed.
    changed: Vec<usize>,
    /// The effect the step installed.
    effect: LightEffect,
    /// The basic sector type the effect installs in this family.
    special: i32,
    /// The flag family that wrote it.
    family: &'static str,
}

/// The `(special, light)` pair of sector `index`, or the `invalid_argument`
/// naming the index past the map's sectors.
fn sector_state(map: &Map, index: usize) -> Result<(i32, i32), CyberError> {
    map.sectors
        .get(index)
        .map(|row| (row.special, row.light))
        .ok_or_else(|| out_of_range("sector", index, map.sectors.len()))
}

/// The Boom-bit note for sector `index`, whose special now carries the
/// secret flag on top of a vanilla `Doom` type: the effect and the secret
/// survive only in ports that read bits, so a map that has to work in
/// vanilla wants its own sector with the vanilla type instead.
fn boom_note(index: usize, flagged: i32) -> String {
    format!(
        "sector {index} now carries special {flagged}: vanilla Doom ignores Boom bits, so \
         that engine reads neither the old effect nor the secret (only a Boom-compatible \
         or ZDoom port does); mark it with the vanilla type {DOOM_SECRET_SPECIAL} instead \
         if it has to stay hidden from every engine"
    )
}

/// Store `special` in sector `index`, reporting whether the row changed.
fn write_special(map: &mut Map, index: usize, special: i32) -> bool {
    let Some(row) = map.sectors.get_mut(index) else {
        return false;
    };
    let changed = row.special != special;
    row.special = special;
    changed
}

/// Store the special `effect` and the level `light` in sector `index`,
/// reporting whether the row changed.
fn write_light(map: &mut Map, index: usize, effect: i32, light: i32) -> bool {
    let Some(row) = map.sectors.get_mut(index) else {
        return false;
    };
    let changed = row.special != effect || row.light != light;
    row.special = effect;
    row.light = light;
    changed
}

/// The checked `(index, edit)` pair of one sector a [`mark_secrets`] call
/// names.
type SecretPlan = (usize, SecretEdit);

/// The planned rows of a [`mark_secrets`] call: every selected sector's
/// secret edit, all computed before any of them is written.
fn planned_secrets(map: &Map, spec: &SecretSpec) -> Result<Vec<SecretPlan>, CyberError> {
    let family = map.flag_family();
    let mut plans = Vec::with_capacity(spec.indices.len());
    for index in &spec.indices {
        let (before, _) = sector_state(map, *index)?;
        plans.push((*index, secret_edit(family, before, spec.secret)));
    }
    Ok(plans)
}

/// The checked `(index, special, light)` triple of every sector a
/// [`set_lights`] call names, computed before any row is written.
type LightPlan = (usize, i32, i32);

/// The planned rows of a [`set_lights`] call, all computed up front.
fn planned_lights(map: &Map, spec: &LightSpec) -> Result<Vec<LightPlan>, CyberError> {
    let family = map.flag_family();
    let mut plans = Vec::with_capacity(spec.indices.len());
    for index in &spec.indices {
        let (before, level) = sector_state(map, *index)?;
        plans.push((
            *index,
            light_special(family, before, spec.effect),
            spec.light.unwrap_or(level),
        ));
    }
    Ok(plans)
}

/// Set or clear the family's secret flag on every selected sector, keeping
/// every bit the flag does not own.
fn mark_secrets(map: &mut Map, spec: &SecretSpec) -> Result<SecretStep, CyberError> {
    let plans = planned_secrets(map, spec)?;
    let mut step = SecretStep {
        changed: Vec::new(),
        warnings: Vec::new(),
        family: map.flag_family().as_str(),
    };
    for (index, edit) in plans {
        if edit.boom_warning {
            step.warnings.push(boom_note(index, edit.special));
        }
        if write_special(map, index, edit.special) {
            step.changed.push(index);
        }
    }
    Ok(step)
}

/// Install the effect (and the asked-for light level) on every selected
/// sector, keeping the bits above the family's basic-type field.
fn set_lights(map: &mut Map, spec: &LightSpec) -> Result<LightStep, CyberError> {
    let plans = planned_lights(map, spec)?;
    let family = map.flag_family();
    let mut step = LightStep {
        changed: Vec::new(),
        effect: spec.effect,
        special: spec.effect.basic_special(family),
        family: family.as_str(),
    };
    for (index, effect, light) in plans {
        if write_light(map, index, effect, light) {
            step.changed.push(index);
        }
    }
    Ok(step)
}

/// Structured result of [`Cyberdemon::mark_secret`].
#[derive(Serialize)]
struct SecretToolReport {
    /// Whether the sectors were marked (`true`) or cleared (`false`).
    secret: bool,
    /// The flag family whose secret bit was written: `doom` or `hexen`.
    family: &'static str,
    /// Indices of the sectors whose special actually changed, ascending.
    sectors: Vec<usize>,
    /// The Boom-bit notes, empty unless a vanilla type was flagged.
    warnings: Vec<String>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::set_light_effect`].
#[derive(Serialize)]
struct LightToolReport {
    /// The canonical effect name the sectors now carry.
    effect: &'static str,
    /// The basic sector type the effect installs in this family.
    special: i32,
    /// The flag family that wrote it: `doom` or `hexen`.
    family: &'static str,
    /// The light level applied (`null` when the call asked for none).
    light: Option<i32>,
    /// Indices of the sectors whose special or light actually changed.
    sectors: Vec<usize>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// `mark_secret`'s work: resolve the selection and run the whole flagging
/// as one [`crate::session::Session::edit_map`] step.
fn mark_secret_run(
    session: &mut Session,
    args: &MarkSecretParams,
) -> Result<SecretToolReport, CyberError> {
    let spec = SecretSpec {
        indices: sector_selection(&args.sectors)?,
        secret: args.secret,
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = if spec.secret {
        "mark secret"
    } else {
        "clear secret"
    };
    let (step, report) = session.edit_map(name, label, |map| mark_secrets(map, &spec))?;
    Ok(SecretToolReport {
        secret: spec.secret,
        family: step.family,
        sectors: step.changed,
        warnings: step.warnings,
        edit: edit_result_fragment(&report),
    })
}

/// `set_light_effect`'s work: resolve the effect and the level, then run
/// the whole repaint as one [`crate::session::Session::edit_map`] step.
fn set_light_run(
    session: &mut Session,
    args: &SetLightParams,
) -> Result<LightToolReport, CyberError> {
    let effect = effect_of(&args.effect)?;
    args.light.map_or(Ok(()), check_light_level)?;
    let spec = LightSpec {
        indices: sector_selection(&args.sectors)?,
        effect,
        light: args.light,
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = "set light effect";
    let (step, report) = session.edit_map(name, label, |map| set_lights(map, &spec))?;
    Ok(LightToolReport {
        effect: step.effect.name(),
        special: step.special,
        family: step.family,
        light: spec.light,
        sectors: step.changed,
        edit: edit_result_fragment(&report),
    })
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
#[tool_router(router = features_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Cuts a working door into a map in one undoable edit.
    #[tool(
        description = "Cuts a working door into a map in one undoable edit: the doorway, the recess it slides in, the use action and the dressing. Name the wall exactly one way (both or neither is invalid_argument): between: [sectorA, sectorB] for the two rooms the door joins (they must share a two-sided boundary; sector A lends the recess its heights) or linedef: N for the wall to cut (a two-sided line behaves like between on its two sectors; a one-sided line opens towards the space its front faces). at: {x, y} optionally centres the cut there (between picks the shared line nearest here; a point off the wall is projected onto it). width (default 64) is the opening in map units: a door wider than its wall is invalid_argument naming the available length. depth is for one-sided walls only (a two-sided door's recess always spans the carved strip): by default an outward ray measures it, up to 256 units to the next sector; an explicit positive depth must leave the recess's far middle inside a sector. Both refusals say \"no room behind this wall within 256 units; move the rooms closer or give a larger depth\" (invalid_argument). Two-sided doors between touching rooms: a 16-unit void strip is carved along their WHOLE shared boundary first (the notes say so), then the door recess spans the strip, so each room gets a real wall face to the doorway. style (default normal) picks the action: normal (opens on use, waits, closes: Doom DR 1 / Hexen Door_Raise 12 speed 16 delay 150), fast (double speed: 117 / 12 speed 64), stay_open (opens once and stays open: D1 31 / Door_Open 11), locked_red, locked_blue, locked_yellow (key-card use: 28/26/27 / Door_LockedRaise 13 with the matching key class), or remote (no action on the door's own lines: the recess is closed and its sector tagged with a fresh unused tag so a switch can raise it; until a trigger exists the door stays inert - Doom-family plain doors need no tag and act on the sector behind the line, so their result tag is null). Textures default through the session's theme roles: door (the moving face, the upper of every room-facing side), door_track (door_track_red/blue/yellow for the locked styles) on the jambs, and door_floor on the recess floor (room A's flat when the theme names none); texture and track_texture override, validated for the map's format. The dressing flips every room-facing line to front its room, top-pegs it and clears its blocking bit, and closes every jamb to a one-sided, floor-pinned track wall. Geometry changes, so the node lumps are invalidated (run build_nodes before playing the map in vanilla Doom). Out-of-range sector or linedef indices are invalid_argument. Returns {door_sector, lines, jambs, style, tag (number for remote, else null), notes, map, nodes_invalidated, copied_to_working}."
    )]
    fn add_door(
        &self,
        Parameters(args): Parameters<AddDoorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| add_door_run(session, &args)),
        )
    }

    /// Mounts a working switch on a wall chunk in one undoable edit.
    #[tool(
        description = "Mounts a working switch on a wall in one undoable edit: a width-wide panel chunk isolated as its own linedef, wearing the theme's switch texture and a use action. linedef: N is the wall. at: {x, y} optionally centres the panel there (a point off the wall is projected onto it); without it the panel sits at the wall's midpoint. width (default 32) is the panel: a longer wall is split so the chunk becomes its own linedef (the result's linedef names it and split is true), a line already exactly that chunk is dressed as it stands (split false), and a wall too short for the width is invalid_argument naming its available length. action (case-insensitive) picks what it does: open_door (opens the tagged door, closing it after its wait: Doom S1 103, SR 61 when repeatable; Hexen Door_Open 11 speed 16), raise_floor (raises the tagged floor to the highest neighbouring floor: S1 18, SR 69; Floor_RaiseToNearest 25 speed 8), lower_floor (lowers it to the lowest neighbouring floor: S1 23, SR 60; Floor_LowerToLowest 21 speed 8), exit (ends the level: S1 11; Portal 243) or secret_exit (ends it through the secret exit: S1 51; Portal 244). An unknown action is invalid_argument listing all five. open_door, raise_floor and lower_floor require target_sector - the sector they act on: missing or out of range is invalid_argument naming it. The sector's existing tag is what the line answers with; an untagged sector (tag 0) is stamped with a fresh unused tag (the reported tag), so an add_door remote door is triggered by passing its door_sector: the tags match. exit and secret_exit ignore target_sector and repeatable (the Doom family ships no SR exit and a level change fires once). repeatable (default false) picks the SR specials in the Doom family and adds repeatspecial activation in the Hexen family, whose switch lines also get their activation field replaced with playeruse (a Doom-family line keeps its flags). The panel texture comes from the session's theme: the switch role, or exit_switch for the level-changing actions; the isolated line's front middle wears it, and a line with no front sidedef is invalid_argument (see add_sidedef). Result: {linedef, split, action, special, tag, texture} plus the standard edit fragment; nodes_invalidated is true when the wall had to be split (the map's geometry changed)."
    )]
    fn add_switch(
        &self,
        Parameters(args): Parameters<AddSwitchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| add_switch_run(session, &args)),
        )
    }

    /// Ends the level from a wall switch or a walk-over line in one undoable edit.
    #[tool(
        description = "Places a level end in one undoable edit, as an exit switch on a wall or as lines the player walks over. kind (required, case-insensitive) picks the trigger. \"switch\" is the add_switch code path with the exit action: linedef (required; missing is invalid_argument naming it) is the wall the panel mounts on, the panel width defaults to 32 (a longer wall is split and the reported lines name the new chunk), it wears the theme's exit_switch texture, and the Doom family gets S1 11 (51 when secret) with the line answering tag 0 - the Hexen family gets Portal 243 (244) with playeruse activation. \"walk_over\" dresses a whole sector: sector (required; missing is invalid_argument naming it) - every two-sided linedef that has the sector on either side becomes the level end. The Doom family gets W1 52 (124 when secret); vanilla reads such a line's tag as the destination map number, so each line's existing tag is kept (0 ends the game, or the secret level on 124; set a destination with edit_linedef). The Hexen family gets special 243 (244) with its activation field set to playercross (cross, not use: the field's zero value); the lines' other flag bits, tags and args stay. An unknown kind is invalid_argument listing both. secret (default false) picks the map's secret exit for either kind. kind switch ignores sector; kind walk_over ignores linedef, textures nothing and changes no geometry, so its nodes_invalidated is false. A walk-over sector with no two-sided boundary is invalid_argument naming it. Returns {kind, lines, special, secret, map, nodes_invalidated, copied_to_working}: lines are the linedefs carrying the action, special the installed number. Look at the result with render_topdown (action lines render blue) and check it with validate_map."
    )]
    fn add_exit(
        &self,
        Parameters(args): Parameters<AddExitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| add_exit_run(session, &args)),
        )
    }

    /// Marks whole sectors secret in one undoable edit.
    #[tool(
        description = "Marks whole sectors secret (or unmarks them) in a single undoable edit: a pure edit of Sector::special on sectors that already exist, so no geometry, no textures and no tags are touched and nodes_invalidated stays false. map defaults to the active map (set_active_map, else the merged view's only map). sectors is a non-empty list of sector indices (duplicates collapse); an empty list or an index the map does not hold is invalid_argument naming it, and the refusal changes nothing. secret (default true) marks; secret false clears. Which bit means secret depends on the map's flag family. Doom family (binary Doom, UDMF in the doom/heretic/strife namespace): a plain sector (special 0) becomes the vanilla secret type 9 and an unmark turns that 9 back to 0; a Boom generalized sector (32 and above) gains or loses the Boom secret bit 128 with every other bit kept; any other vanilla type (1..=31) also gets bit 128, and the result then carries the warning \"vanilla Doom ignores Boom bits\" because vanilla engines compare the whole special, losing the effect without gaining the secret - give those rooms their own sector with special 9 instead. Hexen family (binary Hexen, UDMF in any other namespace): bit 1024 is set or cleared on top of the sector's own special, so the effect survives, and nothing is warned about. Returns {secret, family (\"doom\"|\"hexen\"), sectors (the indices whose special actually changed, ascending), warnings, map, nodes_invalidated, copied_to_working}: an already-marked sector is honestly reported as unchanged. One undo entry rolls the call back, labelled \"mark secret\" (or \"clear secret\" when secret is false). Check it with assemble_map (the sector row's special) or get_sector_specials (its Boom decoding names the \"secret\" flag)."
    )]
    fn mark_secret(
        &self,
        Parameters(args): Parameters<MarkSecretParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| mark_secret_run(session, &args)),
        )
    }

    /// Installs a vanilla light effect on whole sectors in one undoable edit.
    #[tool(
        description = "Installs a light effect on whole sectors in a single undoable edit: a pure edit of Sector::special, and of Sector::light when you give it, so no geometry, no textures and no tags are touched and nodes_invalidated stays false. map defaults to the active map (set_active_map, else the merged view's only map). sectors is a non-empty list of sector indices (duplicates collapse); an empty list or an index the map does not hold is invalid_argument naming it, and the refusal changes nothing. effect (case-insensitive) is one of none, flicker, strobe_fast, strobe_slow, glow or random_off; an unknown name is invalid_argument listing all six. The Doom family (binary Doom, UDMF in the doom/heretic/strife namespace) writes the vanilla types flicker 17, strobe_fast 2, strobe_slow 3, glow 8, random_off 1 and none 0 into the special's low five bits and keeps every bit from 32 up, so a Boom generalized sector keeps its generalized behaviour. The Hexen family (binary Hexen, UDMF in any other namespace) writes the ZDoom Doom-equivalents, the Doom number plus 64: flicker 81, strobe_fast 66, strobe_slow 67, glow 72, random_off 65 and none 0, into the low eight bits and keeps every bit from 256 up - its damage and trigger flags and the secret bit 1024 included. light (optional, 0..=255) also sets the sector's light level, the base the effect flashes around; outside that range is invalid_argument. Returns {effect, special (the basic type this family stores), family (\"doom\"|\"hexen\"), light (the level applied, null when you passed none), sectors (the indices that actually changed, ascending), map, nodes_invalidated, copied_to_working}. A sector already carrying exactly what you asked for is honestly reported as unchanged, and one undo entry (labelled \"set light effect\") rolls the whole call back. Check it with assemble_map and render_topdown (fill_sectors greys each sector by its light)."
    )]
    fn set_light_effect(
        &self,
        Parameters(args): Parameters<SetLightParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| set_light_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{
        AddDoorParams, AddExitParams, AddSwitchParams, Chosen, DoorSkins, DoorSpec, DoorStyle,
        Dress, ExitKind, Groups, LIGHT_MAX, LIGHT_MIN, LightSpec, MarkSecretParams, Placed,
        SecretSpec, SetLightParams, SwitchSpec, Target, action_name, action_names, action_of,
        build_door, build_switch, build_walk_over_exit, centroid, check_light_level,
        choose_between, choose_linedef, door_groups, door_record, dress_door, dress_switch,
        effect_names, effect_of, half_axis, isolate_chunk, kind_name, kind_of, mark_secrets,
        out_of_range, recess_depth, sector_selection, set_lights, side_sector, strip_face_lost,
        strip_wall, style_name, style_names, style_of, switch_cut, switch_role, switch_span,
        switch_tag, target_of, track_role, walk_over_special,
    };
    use crate::error::CyberError;
    use crate::themes::Role;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::features::LightEffect;
    use crate::wad::map::features::SwitchAction;
    use crate::wad::map::features::door_wall;
    use crate::wad::map::features::switch_special;
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::ops::Point;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The message of a refusal that must be an `InvalidArgument`.
    fn refuse(error: &CyberError) -> String {
        assert_eq!(code(error), "invalid_argument", "refusal code");
        error.to_string()
    }

    /// A request parsed from `json`.
    fn args(json: &str) -> AddDoorParams {
        from_str(json).expect("test requests parse")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A sidedef over `sector` wearing `WALL1` in the middle.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: "WALL1".to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A one-sided blocking line from `v1` to `v2` over `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1_u32,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided line from `v1` to `v2` over `front`/`back`.
    fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4_u32,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sector with the given heights and tag.
    fn room(floor_height: i32, ceiling_height: i32, tag: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "CEIL1".to_owned(),
            light: 176_i32,
            tag,
            ..Default::default()
        }
    }

    /// The two-room test map: a hall (sector 0) and a raised room (sector
    /// 1, tag 1) sharing the line 2, wound like the repo's fixtures (the
    /// front side bounds the line's right).
    fn two_rooms() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                solid_line(0, 5, 0),
                solid_line(5, 4, 1),
                open_line(4, 1, 2, 3),
                solid_line(1, 0, 4),
                solid_line(4, 3, 5),
                solid_line(3, 2, 6),
                solid_line(2, 1, 7),
            ],
            sidedefs: vec![
                side(0),
                side(0),
                side(0),
                side(1),
                side(0),
                side(1),
                side(1),
                side(1),
            ],
            sectors: vec![room(0, 128, 0), room(32, 128, 1)],
            ..Default::default()
        }
    }

    /// The single-room test map: a 128 square, every wall one-sided,
    /// nothing behind any of them.
    fn one_room() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(0, 128),
                vertex(128, 128),
                vertex(128, 0),
            ],
            linedefs: vec![
                solid_line(0, 1, 0),
                solid_line(1, 2, 0),
                solid_line(2, 3, 0),
                solid_line(3, 0, 0),
            ],
            sidedefs: vec![side(0), side(0), side(0), side(0)],
            sectors: vec![room(0, 128, 0)],
            ..Default::default()
        }
    }

    /// Room A (sector 0, `x 0..128`) touching a hall (sector 1, east of
    /// `x 128`); A's west wall is the one-sided door wall, its east line
    /// the two-sided boundary the outward ray stops at 128 units.
    fn room_and_hall() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(0, 128),
                vertex(128, 128),
                vertex(128, 0),
                vertex(192, 128),
                vertex(192, 0),
            ],
            linedefs: vec![
                solid_line(0, 1, 0),
                solid_line(1, 2, 1),
                open_line(2, 3, 2, 3),
                solid_line(3, 0, 4),
                solid_line(2, 4, 5),
                solid_line(4, 5, 6),
                solid_line(5, 3, 7),
            ],
            sidedefs: vec![
                side(0),
                side(0),
                side(0),
                side(1),
                side(0),
                side(1),
                side(1),
                side(1),
            ],
            sectors: vec![room(0, 128, 0), room(0, 128, 0)],
            ..Default::default()
        }
    }

    /// The skins the tests dress doors with.
    fn skins() -> DoorSkins {
        DoorSkins {
            door: "DOOR1".to_owned(),
            track: "DOORTRAK".to_owned(),
            floor_flat: "FLAT10".to_owned(),
            wall: "WALL1".to_owned(),
        }
    }

    /// A full spec over `target`.
    fn spec(target: Target) -> DoorSpec {
        DoorSpec {
            target,
            at: None,
            width: 64_i32,
            depth: None,
            style: DoorStyle::Normal,
            skins: skins(),
        }
    }

    #[test]
    fn styles_round_trip_and_refuse_unknown_names() {
        for style in DoorStyle::all() {
            assert_eq!(style_of(style_name(style)).expect("canonical"), style);
            assert_eq!(
                style_of(&style_name(style).to_ascii_uppercase()).expect("case-insensitive"),
                style
            );
        }
        let message = refuse(&style_of("sliding").unwrap_err());
        assert!(
            message.contains(
                "normal, fast, stay_open, locked_red, locked_blue, locked_yellow, remote"
            ),
            "the refusal lists every name: {message}"
        );
        assert_eq!(style_names().len(), DoorStyle::all().len(), "one name each");
    }

    #[test]
    fn locked_styles_get_their_coloured_track_roles() {
        assert_eq!(track_role(DoorStyle::LockedRed), Role::DoorTrackRed);
        assert_eq!(track_role(DoorStyle::LockedBlue), Role::DoorTrackBlue);
        assert_eq!(track_role(DoorStyle::LockedYellow), Role::DoorTrackYellow);
        for style in [
            DoorStyle::Normal,
            DoorStyle::Fast,
            DoorStyle::StayOpen,
            DoorStyle::Remote,
        ] {
            assert_eq!(track_role(style), Role::DoorTrack, "{style:?}");
        }
    }

    #[test]
    fn exactly_one_of_between_and_linedef_resolves() {
        let pair = args(r#"{"between": [0, 1]}"#);
        assert!(matches!(
            target_of(&pair).expect("between"),
            Target::Between { a: 0, b: 1 }
        ));
        let single = args(r#"{"linedef": 4}"#);
        assert!(matches!(
            target_of(&single).expect("linedef"),
            Target::Linedef(4)
        ));
        let both = args(r#"{"between": [0, 1], "linedef": 4}"#);
        assert!(refuse(&target_of(&both).unwrap_err()).contains("exactly one"));
        let neither = args("{}");
        assert!(refuse(&target_of(&neither).unwrap_err()).contains("exactly one"));
    }

    #[test]
    fn choose_validates_sector_and_linedef_indices() {
        let map = two_rooms();
        let second = refuse(&choose_between(&map, 0, 99, None).unwrap_err());
        assert!(second.contains("sector 99 is out of range"), "{second}");
        let first = refuse(&choose_between(&map, 99, 0, None).unwrap_err());
        assert!(first.contains("sector 99 is out of range"), "{first}");
        let wall = refuse(&choose_linedef(&map, 99, None).unwrap_err());
        assert!(wall.contains("linedef 99 is out of range"), "{wall}");
        let self_pair = refuse(&choose_between(&map, 0, 0, None).unwrap_err());
        assert!(self_pair.contains("no two-sided boundary"), "{self_pair}");
        assert_eq!(
            code(&out_of_range("sector", 9, 2)),
            "invalid_argument",
            "the shared refusal's code"
        );
    }

    #[test]
    fn choose_linedef_reads_sidedness() {
        let map = two_rooms();
        let touching = choose_linedef(&map, 2, None).expect("the step line is two-sided");
        assert!(touching.touching, "its sectors differ");
        assert_eq!((touching.room_a, touching.room_b), (0_usize, 1_usize));
        assert_eq!(
            touching.centre,
            Point { x: 256, y: 128 },
            "the line's midpoint"
        );
        let solid = choose_linedef(&map, 0, None).expect("the west wall is one-sided");
        assert!(!solid.touching, "the hall owns its only side");
        assert_eq!((solid.room_a, solid.wall), (0_usize, 0_usize));
        let back_room = choose_linedef(&map, 5, None).expect("the east wall is one-sided");
        assert_eq!(back_room.room_a, 1_usize, "room 1 owns it");
    }

    #[test]
    fn choose_linedef_refuses_useless_walls() {
        let mut map = two_rooms();
        map.linedefs.push(open_line(0, 1, 0, 0));
        let doubled = map.linedefs.len() - 1;
        let same = refuse(&choose_linedef(&map, doubled, None).unwrap_err());
        assert!(same.contains("both sides of linedef"), "{same}");
        map.linedefs.push(solid_line(1, 2, 0));
        let frontless = map.linedefs.len() - 1;
        if let Some(line) = map.linedefs.get_mut(frontless) {
            line.front = None;
            line.back = Some(3);
        }
        let only_back = refuse(&choose_linedef(&map, frontless, None).unwrap_err());
        assert!(only_back.contains("only a back side"), "{only_back}");
        map.linedefs.push(solid_line(2, 3, 0));
        let naked = map.linedefs.len() - 1;
        if let Some(line) = map.linedefs.get_mut(naked) {
            line.front = None;
        }
        let sideless = refuse(&choose_linedef(&map, naked, None).unwrap_err());
        assert!(sideless.contains("no usable sidedef"), "{sideless}");
    }

    #[test]
    fn touching_doors_are_cut_apart_spanned_and_closed() {
        let mut map = two_rooms();
        let built = build_door(&mut map, &spec(Target::Between { a: 0, b: 1 }))
            .expect("a door between the rooms");
        assert_eq!(built.door_sector, 2, "the third sector");
        assert_eq!(built.lines.len(), 2, "one use line per room");
        assert_eq!(built.jambs.len(), 2, "a track wall on each side");
        assert_eq!(built.tag, None, "plain doors carry no tag");
        assert!(
            built.notes.iter().any(|note| note.contains("void strip")),
            "the strip note: {:?}",
            built.notes
        );
        let door = map.sectors.get(2).expect("the door sector");
        assert_eq!(door.ceiling_height, door.floor_height, "the door is closed");
        assert_eq!(door.floor_height, 0_i32, "room A's floor");
        assert_eq!(door.floor_flat, "FLAT10", "the theme's recess floor");
        assert_eq!(door.tag, 0_i32, "untagged");
        let mut fronts = Vec::new();
        for index in &built.lines {
            let line = map.linedefs.get(*index).expect("a room line");
            assert_eq!(line.special, 1_i32, "the DR action");
            assert_eq!(line.tag, 0_i32, "tag 0 acts on the sector behind");
            let room_side = map
                .sidedefs
                .get(line.front.expect("a front side"))
                .expect("front");
            assert_eq!(room_side.upper, "DOOR1", "the face wears the door");
            fronts.push(side_sector(&map, line.front).expect("the front sector"));
        }
        assert!(
            fronts.contains(&0_usize) && fronts.contains(&1_usize),
            "fronts face their rooms: {fronts:?}"
        );
    }

    #[test]
    fn jambs_are_one_sided_tracks() {
        let mut map = two_rooms();
        let built =
            build_door(&mut map, &spec(Target::Between { a: 0, b: 1 })).expect("a drawn doorway");
        for index in &built.jambs {
            let line = map.linedefs.get(*index).expect("a jamb");
            assert_eq!(line.back, None, "the jamb is one-sided");
            assert_eq!(line.flags & 1_u32, 1_u32, "blocking");
            let front = map
                .sidedefs
                .get(line.front.expect("a front side"))
                .expect("side");
            assert_eq!(front.middle, "DOORTRAK", "the track");
        }
    }

    #[test]
    fn remote_doors_carry_a_fresh_tag_and_no_action() {
        let mut map = two_rooms();
        let spec = DoorSpec {
            style: DoorStyle::Remote,
            ..spec(Target::Between { a: 0, b: 1 })
        };
        let built = build_door(&mut map, &spec).expect("a remote door");
        assert_eq!(built.tag, Some(2_i32), "room 1's tag 1 was outgrown");
        let door = map.sectors.get(built.door_sector).expect("the door");
        assert_eq!(door.tag, 2_i32, "the sector carries the minted tag");
        assert_eq!(door.ceiling_height, door.floor_height, "closed and inert");
        for index in &built.lines {
            let line = map.linedefs.get(*index).expect("a line");
            assert_eq!(line.special, 0_i32, "no action on the door's lines");
            assert_eq!(line.tag, 2_i32, "the Doom family pre-tags the use lines");
        }
    }

    #[test]
    fn a_too_wide_door_names_the_available_length() {
        let mut map = two_rooms();
        let spec = DoorSpec {
            width: 300_i32,
            ..spec(Target::Between { a: 0, b: 1 })
        };
        let message = refuse(&build_door(&mut map, &spec).unwrap_err());
        assert!(message.contains("256 map units long"), "{message}");
        assert!(message.contains("300-unit door"), "{message}");
        assert_eq!(map.sectors.len(), 2, "nothing was drawn");
    }

    #[test]
    fn a_one_sided_wall_with_nothing_behind_refuses() {
        let mut map = one_room();
        let message = refuse(&build_door(&mut map, &spec(Target::Linedef(0))).unwrap_err());
        assert_eq!(
            message,
            "no room behind this wall within 256 units; move the rooms closer or give a \
             larger depth",
            "the exact instruction"
        );
        assert_eq!(map.sectors.len(), 1, "nothing was drawn");
    }

    #[test]
    fn an_explicit_depth_must_land_inside_a_sector() {
        let mut map = one_room();
        let spec = DoorSpec {
            depth: Some(400_i32),
            ..spec(Target::Linedef(0))
        };
        let message = refuse(&build_door(&mut map, &spec).unwrap_err());
        assert!(message.contains("no room behind this wall"), "{message}");
        assert_eq!(map.sectors.len(), 1, "nothing was drawn");
    }

    #[test]
    fn a_one_sided_door_opens_a_passage_across_the_room() {
        let mut map = room_and_hall();
        let built =
            build_door(&mut map, &spec(Target::Linedef(0))).expect("the ray finds the hall");
        assert_eq!(built.lines.len(), 3, "the bay: the hall and both sides");
        assert_eq!(built.jambs.len(), 1, "the old west wall seals the bay");
        let door = map.sectors.get(built.door_sector).expect("the door");
        assert_eq!(door.ceiling_height, door.floor_height, "closed");
        for index in &built.lines {
            let line = map.linedefs.get(*index).expect("a line");
            assert!(line.front.is_some() && line.back.is_some(), "two-sided");
            assert_eq!(line.special, 1_i32, "every opening carries the action");
        }
        assert_eq!(
            map.sectors.get(built.door_sector).expect("door").floor_flat,
            "FLAT10"
        );
    }

    #[test]
    fn recess_depth_validates_the_explicit_number() {
        let map = one_room();
        let wall = door_wall(&map, 0, None, 64_i32).expect("the west wall carries 64");
        let mut probe = spec(Target::Linedef(0));
        probe.depth = Some(0_i32);
        assert!(
            refuse(&recess_depth(&map, &wall, &probe).unwrap_err()).contains("positive"),
            "zero is no depth"
        );
        probe.depth = Some(-8_i32);
        assert!(
            refuse(&recess_depth(&map, &wall, &probe).unwrap_err()).contains("positive"),
            "neither is a negative one"
        );
        probe.depth = Some(64_i32);
        assert_eq!(recess_depth(&map, &wall, &probe).expect("kept"), 64_i32);
        probe.depth = None;
        assert_eq!(
            code(&recess_depth(&map, &wall, &probe).unwrap_err()),
            "invalid_argument",
            "the ray finds nothing behind the room"
        );
    }

    #[test]
    fn strip_wall_needs_a_one_sided_room_face() {
        let map = two_rooms();
        assert_eq!(
            strip_wall(&map, 9, Point { x: 0, y: 0 }),
            None,
            "sector 9's walls are nobody's"
        );
        let west = strip_wall(&map, 0, Point { x: 0, y: 128 }).expect("the hall's west wall");
        assert_eq!(west, 0_usize, "the one-sided face at the point");
        let east = strip_wall(&map, 1, Point { x: 256, y: 128 }).expect("room 1's far wall");
        assert_eq!(
            east, 4_usize,
            "the nearest one-sided face (two-sided step lines never count, and the \
             north/south ties go to the lowest index)"
        );
    }

    #[test]
    fn the_strip_face_refusal_names_the_room() {
        let message = refuse(&strip_face_lost(3));
        assert!(message.contains("sector 3"), "{message}");
    }

    #[test]
    fn an_empty_map_finds_no_groups() {
        let map = Map::default();
        let groups = door_groups(&map, 0_usize);
        assert!(groups.lines.is_empty(), "nothing borders");
        assert!(groups.jambs.is_empty(), "and nothing frames it");
    }

    #[test]
    fn centroids_and_halves_stay_whole() {
        assert_eq!(
            centroid(&[
                Point { x: 0, y: 0 },
                Point { x: 3, y: 0 },
                Point { x: 3, y: 3 },
                Point { x: 0, y: 3 },
            ]),
            Point { x: 2, y: 2 },
            "the mean of the corners"
        );
        assert_eq!(half_axis(-1, 0), -1_i32, "halves round away from zero");
        assert_eq!(half_axis(0, 1), 1_i32, "up too");
        assert_eq!(half_axis(i32::MAX, i32::MAX), i32::MAX, "saturated");
    }

    #[test]
    fn the_recess_record_copies_the_room_and_tags_remote() {
        let map = two_rooms();
        let placed = Placed {
            wall: 0_usize,
            room_a: 1_usize,
            rect: vec![Point { x: 0, y: 0 }],
            tag: Some(7_i32),
        };
        let record = door_record(&map, &placed, &spec(Target::Linedef(0)));
        assert_eq!(record.floor_height, 32_i32, "room 1's floor");
        assert_eq!(record.ceiling_height, 128_i32, "and ceiling");
        assert_eq!(record.tag, 7_i32, "the minted tag");
        assert_eq!(record.special, 0_i32, "no effect");
        assert_eq!(record.floor_flat, "FLAT10", "the theme's recess floor");
        let plain = door_record(
            &map,
            &Placed {
                tag: None,
                ..placed
            },
            &spec(Target::Linedef(0)),
        );
        assert_eq!(plain.tag, 0_i32, "untagged without a tag");
    }

    #[test]
    fn dressing_an_emptied_recess_changes_nothing_more() {
        let mut map = two_rooms();
        let spec = DoorSpec {
            style: DoorStyle::Fast,
            ..spec(Target::Between { a: 0, b: 1 })
        };
        let built = build_door(&mut map, &spec).expect("a fast door");
        for index in &built.lines {
            assert_eq!(
                map.linedefs.get(*index).expect("line").special,
                117_i32,
                "the DR fast action"
            );
        }
        let before = map.clone();
        let dress = Dress {
            door: built.door_sector,
            groups: Groups {
                lines: Vec::new(),
                jambs: Vec::new(),
            },
            tag: 0_i32,
        };
        dress_door(&mut map, &spec, &dress).expect("an empty dressing is allowed");
        assert_eq!(map.sectors, before.sectors, "the door stays closed anyway");
    }

    #[test]
    fn a_hexen_map_takes_the_script_door() {
        let mut map = two_rooms();
        map.format = MapFormat::Hexen;
        let built = build_door(&mut map, &spec(Target::Between { a: 0, b: 1 }))
            .expect("a Hexen-family door");
        for index in &built.lines {
            let line = map.linedefs.get(*index).expect("a room line");
            assert_eq!(line.special, 12_i32, "Door_Raise");
            assert_eq!(line.args, [0_i32, 16_i32, 150_i32, 0_i32, 0_i32]);
            assert_ne!(line.flags & 0x400_u32, 0_u32, "playeruse");
            assert_ne!(line.flags & 0x2000_u32, 0_u32, "monsteractivate");
        }
    }

    #[test]
    fn an_off_centre_door_projects_onto_the_wall() {
        let mut map = two_rooms();
        let chosen: Chosen =
            choose_between(&map, 0, 1, Some(Point { x: 300, y: 64 })).expect("near the south end");
        assert_eq!(
            chosen.centre,
            Point { x: 256, y: 64 },
            "projected onto the shared line"
        );
        let mut spec = spec(Target::Between { a: 0, b: 1 });
        spec.at = Some(Point { x: 300, y: 64 });
        let built = build_door(&mut map, &spec).expect("the off-centre door builds");
        assert_eq!(built.lines.len(), 2);
        assert_eq!(built.jambs.len(), 2);
    }

    /// A `add_switch` request parsed from `json`.
    fn switch_args(json: &str) -> AddSwitchParams {
        from_str(json).expect("test switch requests parse")
    }

    /// A 32-unit open-door switch spec on linedef `linedef`.
    fn switch_spec(linedef: usize) -> SwitchSpec {
        SwitchSpec {
            linedef,
            at: None,
            width: 32_i32,
            action: SwitchAction::OpenDoor,
            target_sector: Some(1_usize),
            repeatable: false,
            texture: "SW1STRTN".to_owned(),
        }
    }

    /// The coordinates of the vertex at [`Linedef`]'s `end` of `line`
    /// (`"v1"` or `"v2"`).
    fn line_vertex(map: &Map, line: usize, end: &str) -> Point {
        let def = map.linedefs.get(line).expect("the linedef");
        let index = if end == "v1" { def.v1 } else { def.v2 };
        let vertex = map.vertices.get(index).expect("the vertex");
        Point {
            x: vertex.x,
            y: vertex.y,
        }
    }

    /// The front sidedef's `middle` texture of linedef `line`.
    fn front_middle(map: &Map, line: usize) -> String {
        let def = map.linedefs.get(line).expect("the linedef");
        let side = def.front.expect("a front side");
        map.sidedefs
            .get(side)
            .expect("the front sidedef")
            .middle
            .clone()
    }

    #[test]
    fn switch_actions_round_trip_and_refuse_unknown_names() {
        for action in SwitchAction::all() {
            assert_eq!(action_of(action_name(action)).expect("canonical"), action);
            assert_eq!(
                action_of(&action_name(action).to_ascii_uppercase()).expect("case-insensitive"),
                action
            );
        }
        let message = refuse(&action_of("press").unwrap_err());
        assert!(
            message.contains("open_door, exit, secret_exit, raise_floor, lower_floor"),
            "the refusal lists every name: {message}"
        );
        assert_eq!(
            action_names().len(),
            SwitchAction::all().len(),
            "one name each"
        );
    }

    #[test]
    fn exit_actions_wear_the_exit_panel() {
        assert_eq!(switch_role(SwitchAction::Exit), Role::ExitSwitch);
        assert_eq!(switch_role(SwitchAction::SecretExit), Role::ExitSwitch);
        for action in [
            SwitchAction::OpenDoor,
            SwitchAction::RaiseFloor,
            SwitchAction::LowerFloor,
        ] {
            assert_eq!(switch_role(action), Role::Switch, "{action:?}");
        }
    }

    #[test]
    fn add_switch_params_read_the_wire_shape() {
        let args = switch_args(
            r#"{"linedef": 3, "action": "raise_floor", "target_sector": 1,
               "at": {"x": 64, "y": 0}, "width": 48, "repeatable": true}"#,
        );
        assert_eq!(args.linedef, 3_usize);
        assert_eq!(args.action, "raise_floor");
        assert_eq!(args.target_sector, Some(1_usize));
        assert_eq!(args.width, Some(48_i32));
        assert!(args.repeatable, "the flag arrives");
        let minimal = switch_args(r#"{"linedef": 0, "action": "exit"}"#);
        assert_eq!(minimal.at, None, "no centre asked");
        assert_eq!(minimal.width, None, "the default width stands");
        assert!(!minimal.repeatable, "one-shot by default");
        assert!(
            from_str::<AddSwitchParams>(r#"{"action": "exit"}"#).is_err(),
            "the wall is required"
        );
        assert!(
            from_str::<AddSwitchParams>(r#"{"linedef": 0}"#).is_err(),
            "the action is required"
        );
    }

    #[test]
    fn a_switch_splits_the_wall_and_installs_the_action() {
        let mut map = two_rooms();
        let placed = build_switch(&mut map, &switch_spec(0)).expect("the west wall takes it");
        assert_eq!(placed.linedef, 7_usize, "the first split is the chunk");
        assert!(placed.split, "the long wall was cut");
        assert_eq!(placed.action, "open_door", "echoed canonically");
        assert_eq!(placed.special, 103_i32, "the S1 door action");
        assert_eq!(placed.tag, 1_i32, "sector 1's own tag");
        assert_eq!(placed.texture, "SW1STRTN", "what it wears");
        assert_eq!(map.linedefs.len(), 9_usize, "two outer pieces");
        assert_eq!(line_vertex(&map, 7, "v1"), Point { x: 0, y: 112 });
        assert_eq!(line_vertex(&map, 7, "v2"), Point { x: 0, y: 144 });
        let chunk = map.linedefs.get(7).expect("the chunk");
        assert_eq!(chunk.special, 103_i32);
        assert_eq!(chunk.tag, 1_i32, "the Doom line carries the tag");
        assert_eq!(chunk.args, [0_i32; 5]);
        assert_eq!(chunk.flags, 1_u32, "a Doom switch keeps the wall's flags");
        assert_eq!(front_middle(&map, 7), "SW1STRTN", "the panel on the front");
        assert_eq!(
            front_middle(&map, 0),
            "WALL1",
            "the remaining wall is plain"
        );
        assert_eq!(front_middle(&map, 8), "WALL1", "and so is the other piece");
    }

    #[test]
    fn an_exact_line_takes_the_switch_without_splitting() {
        let mut map = two_rooms();
        let spec = SwitchSpec {
            width: 256_i32,
            action: SwitchAction::Exit,
            target_sector: None,
            texture: "SW1EXIT".to_owned(),
            ..switch_spec(0)
        };
        let placed = build_switch(&mut map, &spec).expect("the whole wall is the panel");
        assert_eq!(placed.linedef, 0_usize, "the line stands as it is");
        assert!(!placed.split, "nothing was cut");
        assert_eq!(placed.special, 11_i32, "the S1 exit");
        assert_eq!(placed.tag, 0_i32, "an exit acts on no tag");
        assert_eq!(map.linedefs.len(), 7_usize, "no linedef was appended");
        assert_eq!(front_middle(&map, 0), "SW1EXIT");
    }

    #[test]
    fn an_untagged_target_sector_gets_a_fresh_tag() {
        let mut map = two_rooms();
        let spec = SwitchSpec {
            target_sector: Some(0_usize),
            ..switch_spec(0)
        };
        let placed = build_switch(&mut map, &spec).expect("sector 0 is untagged");
        assert_eq!(placed.tag, 2_i32, "room 1's tag 1 was outgrown");
        assert_eq!(
            map.sectors.first().expect("target").tag,
            2_i32,
            "the tag was stamped onto the sector"
        );
        assert_eq!(
            map.linedefs.get(7).expect("the chunk").tag,
            2_i32,
            "and answered by the line"
        );
    }

    #[test]
    fn targeted_actions_demand_their_sector() {
        for action in [
            SwitchAction::OpenDoor,
            SwitchAction::RaiseFloor,
            SwitchAction::LowerFloor,
        ] {
            let mut map = two_rooms();
            let spec = SwitchSpec {
                action,
                target_sector: None,
                ..switch_spec(0)
            };
            let message = refuse(&build_switch(&mut map, &spec).unwrap_err());
            assert!(message.contains("target_sector"), "{action:?}: {message}");
        }
        let mut map = two_rooms();
        let spec = SwitchSpec {
            target_sector: Some(99_usize),
            ..switch_spec(0)
        };
        let message = refuse(&build_switch(&mut map, &spec).unwrap_err());
        assert!(message.contains("sector 99 is out of range"), "{message}");
    }

    #[test]
    fn exits_ignore_their_parameters() {
        let mut map = two_rooms();
        let spec = SwitchSpec {
            action: SwitchAction::SecretExit,
            target_sector: Some(1_usize),
            ..switch_spec(0)
        };
        let placed = build_switch(&mut map, &spec).expect("a secret exit switch");
        assert_eq!(placed.special, 51_i32, "the S1 secret exit");
        assert_eq!(placed.tag, 0_i32, "the given target is ignored");
        assert_eq!(map.sectors.get(1).expect("sector 1").tag, 1_i32);
    }

    #[test]
    fn a_switch_names_the_wall_it_cannot_take() {
        let mut far_map = two_rooms();
        let far = refuse(&build_switch(&mut far_map, &switch_spec(99)).unwrap_err());
        assert!(far.contains("linedef 99 is out of range"), "{far}");
        let mut broken_map = two_rooms();
        broken_map.linedefs.get_mut(0).expect("the wall").v1 = 99;
        let broken = refuse(&build_switch(&mut broken_map, &switch_spec(0)).unwrap_err());
        assert!(broken.contains("references a vertex"), "{broken}");
        let mut map = two_rooms();
        let wide = SwitchSpec {
            width: 300_i32,
            ..switch_spec(0)
        };
        let over = refuse(&build_switch(&mut map, &wide).unwrap_err());
        assert!(over.contains("256 map units long"), "{over}");
        let thin = SwitchSpec {
            width: 0_i32,
            ..switch_spec(0)
        };
        let none = refuse(&build_switch(&mut map, &thin).unwrap_err());
        assert!(none.contains("positive"), "{none}");
        assert_eq!(map.linedefs.len(), 7_usize, "no refusal cut anything");
    }

    #[test]
    fn switch_cut_and_span_check_the_wall_before_the_door_engine() {
        let map = two_rooms();
        let message = refuse(&switch_span(&map, 99).unwrap_err());
        assert!(
            message.contains("linedef 99 is out of range") && !message.contains("door"),
            "switch words: {message}"
        );
        let spec = switch_spec(2);
        let ((start, finish), available) = switch_cut(&map, &spec).expect("the step line");
        assert_eq!(
            start,
            Point { x: 256, y: 144 },
            "centred on the midpoint, running from v1 (256, 256) down"
        );
        assert_eq!(finish, Point { x: 256, y: 112 }, "toward v2");
        assert_eq!(available, 256_i32, "the wall's length");
    }

    #[test]
    fn isolate_chunk_cuts_only_where_the_chunk_does_not_meet_an_endpoint() {
        let mut map = two_rooms();
        let flush = isolate_chunk(
            &mut map,
            0_usize,
            Point { x: 0, y: 0 },
            Point { x: 0, y: 64 },
        )
        .expect("a chunk flush with v1");
        assert_eq!(flush.line, 0_usize, "the original keeps the chunk");
        assert!(flush.split, "one cut at finish");
        assert_eq!(map.linedefs.len(), 8_usize, "only one piece was appended");
    }

    #[test]
    fn a_frontless_line_refuses_the_panel() {
        let mut map = two_rooms();
        map.linedefs.get_mut(0).expect("the wall").front = None;
        let message = refuse(&build_switch(&mut map, &switch_spec(0)).unwrap_err());
        assert!(message.contains("no front sidedef"), "{message}");
    }

    #[test]
    fn a_hexen_switch_acts_through_arguments_and_use_bits() {
        let mut map = two_rooms();
        map.format = MapFormat::Hexen;
        let placed = build_switch(&mut map, &switch_spec(0)).expect("the west wall takes it");
        assert_eq!(placed.special, 11_i32, "Door_Open");
        assert_eq!(placed.tag, 1_i32, "the tag reported is the sector's");
        let chunk = map.linedefs.get(7).expect("the chunk");
        assert_eq!(chunk.tag, 0_i32, "a Hexen line carries no tag");
        assert_eq!(
            chunk.args,
            [1_i32, 16_i32, 0_i32, 0_i32, 0_i32],
            "tag in args[0], speed 16 in args[1]"
        );
        assert_eq!(chunk.flags, 1_u32 | 0x400_u32, "the wall stands, playeruse");
        let mut again = two_rooms();
        again.format = MapFormat::Hexen;
        let spec = SwitchSpec {
            repeatable: true,
            ..switch_spec(0)
        };
        build_switch(&mut again, &spec).expect("a repeating switch");
        assert_eq!(
            again.linedefs.get(7).expect("the chunk").flags,
            1_u32 | 0x600_u32,
            "playeruse plus repeatspecial"
        );
    }

    #[test]
    fn switch_tag_mints_only_for_tagged_actions() {
        let mut map = two_rooms();
        let exit = SwitchSpec {
            action: SwitchAction::Exit,
            target_sector: None,
            ..switch_spec(0)
        };
        assert_eq!(switch_tag(&mut map, &exit).expect("no tag needed"), 0_i32);
        assert_eq!(
            switch_tag(&mut map, &switch_spec(0)).expect("sector 1's own tag"),
            1_i32
        );
    }

    #[test]
    fn dress_switch_refuses_a_line_the_map_does_not_hold() {
        let mut map = two_rooms();
        let setup = switch_special(FlagFamily::Doom, SwitchAction::OpenDoor, false, 1_i32);
        let message = refuse(&dress_switch(&mut map, 99, &setup, "SW1STRTN").unwrap_err());
        assert!(message.contains("linedef 99"), "{message}");
    }

    /// An `add_exit` request parsed from `json`.
    fn exit_args(json: &str) -> AddExitParams {
        from_str(json).expect("test exit requests parse")
    }

    #[test]
    fn exit_kinds_round_trip_and_refuse_unknown_names() {
        for kind in [ExitKind::Switch, ExitKind::WalkOver] {
            assert_eq!(kind_of(kind_name(kind)).expect("canonical"), kind);
            assert_eq!(
                kind_of(&kind_name(kind).to_ascii_uppercase()).expect("case-insensitive"),
                kind
            );
        }
        let message = refuse(&kind_of("stairs").unwrap_err());
        assert!(
            message.contains("\"switch\"") && message.contains("\"walk_over\""),
            "the refusal names both triggers: {message}"
        );
    }

    #[test]
    fn add_exit_params_read_the_wire_shape() {
        let mounted = exit_args(r#"{"kind": "switch", "linedef": 3, "secret": true}"#);
        assert_eq!(mounted.linedef, Some(3_usize));
        assert!(mounted.secret, "the flag arrives");
        assert_eq!(mounted.sector, None, "walk_over's selector stands aside");
        assert_eq!(mounted.map, None, "the map defaults like the other tools");
        let walked = exit_args(r#"{"kind": "walk_over", "sector": 1}"#);
        assert_eq!(walked.sector, Some(1_usize));
        assert!(!walked.secret, "the normal exit by default");
        assert!(
            from_str::<AddExitParams>(r#"{"sector": 1}"#).is_err(),
            "the kind is required"
        );
    }

    #[test]
    fn walk_over_specials_follow_the_family_and_secrecy() {
        assert_eq!(walk_over_special(FlagFamily::Doom, false), 52_i32);
        assert_eq!(walk_over_special(FlagFamily::Doom, true), 124_i32);
        assert_eq!(walk_over_special(FlagFamily::Hexen, false), 243_i32);
        assert_eq!(walk_over_special(FlagFamily::Hexen, true), 244_i32);
    }

    #[test]
    fn a_walk_over_exit_dresses_every_boundary_of_the_sector() {
        let mut map = two_rooms();
        if let Some(line) = map.linedefs.get_mut(2) {
            line.tag = 3_i32;
        }
        let built = build_walk_over_exit(&mut map, 0_usize, false).expect("sector 0's boundary");
        assert_eq!(built.lines, vec![2_usize], "the one shared two-sided line");
        assert_eq!(built.special, 52_i32, "the W1 exit");
        let line = map.linedefs.get(2).expect("the boundary");
        assert_eq!(line.special, 52_i32, "the line carries it");
        assert_eq!(line.tag, 3_i32, "the vanilla destination tag survives");
        assert_eq!(line.flags, 4_u32, "a Doom line's flags stay as they were");
        let from_behind =
            build_walk_over_exit(&mut map, 1_usize, true).expect("sector 1 shares the line");
        assert_eq!(from_behind.lines, vec![2_usize], "either side counts");
        assert_eq!(from_behind.special, 124_i32, "the secret W1");
    }

    #[test]
    fn a_hexen_walk_over_exit_sets_the_cross_activation() {
        let mut map = two_rooms();
        map.format = MapFormat::Hexen;
        if let Some(line) = map.linedefs.get_mut(2) {
            line.flags = 4_u32 | 0x400_u32 | 0x200_u32;
        }
        let built = build_walk_over_exit(&mut map, 0_usize, true).expect("a Hexen exit");
        assert_eq!(built.special, 244_i32, "the secret Portal");
        let line = map.linedefs.get(2).expect("the boundary");
        assert_eq!(line.special, 244_i32);
        assert_eq!(
            line.flags,
            4_u32 | 0x200_u32,
            "the activation field clears to playercross (its zero value); twosided and \
             repeatspecial keep their bits"
        );
    }

    #[test]
    fn walk_over_exits_refuse_bad_sectors_and_bare_rooms() {
        let mut map = two_rooms();
        let missing = refuse(&build_walk_over_exit(&mut map, 99_usize, false).unwrap_err());
        assert!(missing.contains("sector 99 is out of range"), "{missing}");
        let mut lonely = one_room();
        let bare = refuse(&build_walk_over_exit(&mut lonely, 0_usize, false).unwrap_err());
        assert!(
            bare.contains("sector 0 has no two-sided boundary"),
            "{bare}"
        );
        assert_eq!(lonely.linedefs.len(), 4, "a refusal wrote nothing");
        assert_eq!(lonely.sectors.len(), 1, "and drew nothing");
    }

    #[test]
    fn one_sided_lines_never_take_the_walk_over_action() {
        let mut map = two_rooms();
        let solid = 6_usize;
        let built = build_walk_over_exit(&mut map, 1_usize, false).expect("room 1's boundary");
        assert_eq!(
            built.lines,
            vec![2_usize],
            "the hall-facing step line only, not room 1's own walls"
        );
        assert_eq!(map.linedefs.get(solid).expect("a wall").special, 0_i32);
    }

    /// A `mark_secret` request parsed from `json`.
    fn secret_args(json: &str) -> MarkSecretParams {
        from_str(json).expect("test secret requests parse")
    }

    /// A `set_light_effect` request parsed from `json`.
    fn light_args(json: &str) -> SetLightParams {
        from_str(json).expect("test light requests parse")
    }

    /// A secret spec over `indices`.
    fn secret_spec(indices: &[usize], secret: bool) -> SecretSpec {
        SecretSpec {
            indices: indices.to_vec(),
            secret,
        }
    }

    /// A light spec over `indices` with an optional level.
    fn light_spec(indices: &[usize], effect: LightEffect, light: Option<i32>) -> LightSpec {
        LightSpec {
            indices: indices.to_vec(),
            effect,
            light,
        }
    }

    /// `two_rooms` with the given sector specials.
    fn rooms_special(specials: [i32; 2]) -> Map {
        let mut map = two_rooms();
        for (row, special) in map.sectors.iter_mut().zip(specials) {
            row.special = special;
        }
        map
    }

    /// The special of sector `index`.
    fn special_of(map: &Map, index: usize) -> i32 {
        map.sectors.get(index).expect("the sector exists").special
    }

    /// The light level of sector `index`.
    fn light_of(map: &Map, index: usize) -> i32 {
        map.sectors.get(index).expect("the sector exists").light
    }

    #[test]
    fn the_two_special_tools_read_their_wire_shapes() {
        let marked = secret_args(r#"{"sectors": [1, 0, 1]}"#);
        assert!(marked.secret, "marking is the default");
        assert_eq!(marked.map, None, "the map defaults like the other tools");
        assert_eq!(marked.sectors, vec![1_usize, 0_usize, 1_usize], "as listed");
        let cleared = secret_args(r#"{"sectors": [0], "secret": false, "map": "MAP01"}"#);
        assert!(!cleared.secret, "the flag arrives");
        assert_eq!(cleared.map.as_deref(), Some("MAP01"));
        let lit = light_args(r#"{"sectors": [0], "effect": "flicker", "light": 200}"#);
        assert_eq!(lit.effect, "flicker");
        assert_eq!(lit.light, Some(200_i32), "the level arrives");
        assert_eq!(
            light_args(r#"{"sectors": [0], "effect": "glow"}"#).light,
            None,
            "and stands aside when unasked"
        );
        assert!(
            from_str::<MarkSecretParams>(r#"{"secret": true}"#).is_err(),
            "the sectors are required"
        );
        assert!(
            from_str::<SetLightParams>(r#"{"sectors": [0]}"#).is_err(),
            "the effect is required"
        );
    }

    #[test]
    fn a_selection_sorts_itself_and_refuses_to_be_empty() {
        assert_eq!(
            sector_selection(&[2_usize, 0_usize, 2_usize]).expect("a sorted copy"),
            vec![0_usize, 2_usize],
            "ascending and deduplicated"
        );
        let message = refuse(&sector_selection(&[]).unwrap_err());
        assert!(message.contains("sectors is empty"), "{message}");
    }

    #[test]
    fn light_effects_round_trip_and_refuse_unknown_names() {
        for effect in LightEffect::all() {
            assert_eq!(effect_of(effect.name()).expect("canonical"), effect);
            assert_eq!(
                effect_of(&effect.name().to_ascii_uppercase()).expect("case-insensitive"),
                effect
            );
        }
        let message = refuse(&effect_of("pulsate").unwrap_err());
        assert!(
            message.contains("none, flicker, strobe_fast, strobe_slow, glow, random_off"),
            "the refusal lists every name: {message}"
        );
        assert_eq!(
            effect_names().len(),
            LightEffect::all().len(),
            "one name each"
        );
    }

    #[test]
    fn a_light_level_stays_in_the_vanilla_range() {
        assert!(
            check_light_level(LIGHT_MIN).is_ok(),
            "the floor of the range"
        );
        assert!(check_light_level(LIGHT_MAX).is_ok(), "and the top");
        let low = refuse(&check_light_level(-1_i32).unwrap_err());
        assert!(low.contains("light -1 is out of range"), "{low}");
        let high = refuse(&check_light_level(256_i32).unwrap_err());
        assert!(high.contains("light 256 is out of range"), "{high}");
    }

    #[test]
    fn a_plain_doom_sector_takes_the_vanilla_secret_type() {
        let mut map = rooms_special([0_i32, 1_i32]);
        let step = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("sector 0");
        assert_eq!(step.family, "doom", "the family echoes");
        assert_eq!(step.changed, vec![0_usize], "one row moved");
        assert!(step.warnings.is_empty(), "the vanilla type needs no note");
        assert_eq!(special_of(&map, 0), 9_i32, "the vanilla secret");
        assert_eq!(special_of(&map, 1), 1_i32, "the unlisted sector stands");
        let again = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("marked twice");
        assert!(
            again.changed.is_empty(),
            "the second call is honestly a no-op"
        );
        let cleared =
            mark_secrets(&mut map, &secret_spec(&[0_usize], false)).expect("cleared again");
        assert_eq!(cleared.changed, vec![0_usize]);
        assert_eq!(
            special_of(&map, 0),
            0_i32,
            "and the vanilla type clears back to plain"
        );
    }

    #[test]
    fn a_boom_doom_sector_gains_and_loses_only_bit_seven() {
        let mut map = rooms_special([32_i32, 4_097_i32]);
        let step =
            mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
        assert_eq!(step.changed, vec![0_usize, 1_usize], "both moved");
        assert!(
            step.warnings.is_empty(),
            "Boom bits are Boom's own business"
        );
        assert_eq!(special_of(&map, 0), 160_i32, "32 | 128");
        assert_eq!(
            special_of(&map, 1),
            4_225_i32,
            "4097 | 128 keeps the generalized field"
        );
        let cleared =
            mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], false)).expect("both cleared");
        assert_eq!(cleared.changed, vec![0_usize, 1_usize]);
        assert_eq!(special_of(&map, 0), 32_i32, "the flag came back off");
        assert_eq!(
            special_of(&map, 1),
            4_097_i32,
            "with every other bit where it was"
        );
    }

    #[test]
    fn a_vanilla_type_marked_secret_carries_a_boom_warning() {
        let mut map = rooms_special([17_i32, 9_i32]);
        let step =
            mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
        assert_eq!(
            step.changed,
            vec![0_usize],
            "the sector already secret by the vanilla type did not move"
        );
        assert_eq!(special_of(&map, 0), 145_i32, "17 | 128");
        assert_eq!(special_of(&map, 1), 9_i32, "and 9 stays 9");
        let note = step.warnings.first().expect("one warning");
        assert!(note.contains("sector 0"), "{note}");
        assert!(
            note.contains("vanilla Doom ignores Boom bits"),
            "the warning says what vanilla Doom does not read: {note}"
        );
        let back = mark_secrets(&mut map, &secret_spec(&[0_usize], false)).expect("cleared");
        assert_eq!(
            back.warnings.len(),
            0_usize,
            "clearing has nothing to warn about"
        );
        assert_eq!(
            special_of(&map, 0),
            17_i32,
            "the flicker is the number again"
        );
    }

    #[test]
    fn a_hexen_map_flags_secret_over_its_own_effect() {
        let mut map = rooms_special([81_i32, 1_i32]);
        map.format = MapFormat::Hexen;
        let step = mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true))
            .expect("both sectors of a Hexen map");
        assert_eq!(step.family, "hexen", "binary Hexen is its own family");
        assert!(step.warnings.is_empty(), "the Hexen bit never needs a note");
        assert_eq!(special_of(&map, 0), 1_105_i32, "81 | 1024 keeps the effect");
        assert_eq!(special_of(&map, 1), 1_025_i32, "1 | 1024 too");
        let cleared =
            mark_secrets(&mut map, &secret_spec(&[1_usize], false)).expect("one sector back");
        assert_eq!(cleared.changed, vec![1_usize]);
        assert_eq!(special_of(&map, 1), 1_i32, "bit 10 came off alone");
        assert_eq!(
            special_of(&map, 0),
            1_105_i32,
            "the unlisted sector stands untouched"
        );
    }

    #[test]
    fn a_zdoom_udmf_map_shares_the_hexen_secret_bit() {
        let mut map = rooms_special([0_i32, 1_i32]);
        map.format = MapFormat::Udmf;
        map.udmf_namespace = Some("zdoom".to_owned());
        let step = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("sector 0");
        assert_eq!(
            step.family, "hexen",
            "a zdoom namespace reads as Hexen family"
        );
        assert_eq!(special_of(&map, 0), 1_024_i32, "the bit binary Hexen uses");
    }

    #[test]
    fn a_doom_namespace_udmf_map_keeps_the_boom_bit() {
        let mut map = rooms_special([0_i32, 1_i32]);
        map.format = MapFormat::Udmf;
        map.udmf_namespace = Some("doom".to_owned());
        let step =
            mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
        assert_eq!(
            step.family, "doom",
            "the doom namespace is Doom family even in UDMF"
        );
        assert_eq!(
            special_of(&map, 0),
            9_i32,
            "the plain sector took the vanilla type"
        );
        assert_eq!(special_of(&map, 1), 129_i32, "the basic 1 took bit 7");
        assert_eq!(
            step.warnings.len(),
            1_usize,
            "one note: {:?}",
            step.warnings
        );
    }

    #[test]
    fn the_secret_step_names_the_sector_the_map_does_not_hold() {
        let mut map = rooms_special([0_i32, 0_i32]);
        let message =
            refuse(&mark_secrets(&mut map, &secret_spec(&[1_usize, 99_usize], true)).unwrap_err());
        assert!(message.contains("sector 99 is out of range"), "{message}");
        assert_eq!(special_of(&map, 0), 0_i32, "a refusal wrote nothing");
        assert_eq!(
            special_of(&map, 1),
            0_i32,
            "not even the sectors it could have reached"
        );
    }

    #[test]
    fn a_doom_light_effect_writes_the_type_and_keeps_the_generalized_bits() {
        let wanted = [
            (LightEffect::Flicker, 17_i32),
            (LightEffect::StrobeFast, 2_i32),
            (LightEffect::StrobeSlow, 3_i32),
            (LightEffect::Glow, 8_i32),
            (LightEffect::RandomOff, 1_i32),
            (LightEffect::None, 0_i32),
        ];
        for (effect, basic) in wanted {
            let mut map = rooms_special([4_105_i32, 9_i32]);
            let step = set_lights(&mut map, &light_spec(&[0_usize], effect, None))
                .expect("sector 0 takes the effect");
            assert_eq!(step.effect, effect, "{effect:?} echoes");
            assert_eq!(step.family, "doom", "{effect:?} reads its family");
            assert_eq!(step.special, basic, "{effect:?} is the vanilla type");
            assert_eq!(step.changed, vec![0_usize], "{effect:?} moved one row");
            assert_eq!(
                special_of(&map, 0),
                4_096_i32 | basic,
                "{effect:?} landed below 32, the generalized bit above it survived"
            );
            assert_eq!(light_of(&map, 0), 176_i32, "an unasked level stands");
            assert_eq!(special_of(&map, 1), 9_i32, "the unlisted sector stands");
        }
    }

    #[test]
    fn a_hexen_light_effect_is_the_doom_number_sixty_four_higher() {
        let wanted = [
            (LightEffect::Flicker, 81_i32),
            (LightEffect::StrobeFast, 66_i32),
            (LightEffect::StrobeSlow, 67_i32),
            (LightEffect::Glow, 72_i32),
            (LightEffect::RandomOff, 65_i32),
            (LightEffect::None, 0_i32),
        ];
        for (effect, basic) in wanted {
            let mut map = rooms_special([1_024_i32 | 256_i32, 0_i32]);
            map.format = MapFormat::Hexen;
            let step = set_lights(&mut map, &light_spec(&[0_usize], effect, Some(96_i32)))
                .expect("sector 0 takes the effect");
            assert_eq!(step.effect, effect, "{effect:?} echoes");
            assert_eq!(step.special, basic, "{effect:?} is the Doom number plus 64");
            assert_eq!(
                special_of(&map, 0),
                1_024_i32 | 256_i32 | basic,
                "{effect:?} landed below 256, the secret bit and the flag above it survived"
            );
            assert_eq!(
                light_of(&map, 0),
                96_i32,
                "and the asked-for level rode along"
            );
        }
    }

    #[test]
    fn a_light_repaint_that_changes_nothing_reports_nothing() {
        let mut map = rooms_special([17_i32, 17_i32]);
        let first = set_lights(
            &mut map,
            &light_spec(&[0_usize, 1_usize], LightEffect::Flicker, None),
        )
        .expect("both already flicker");
        assert!(first.changed.is_empty(), "the effect was already there");
        assert_eq!(
            set_lights(
                &mut map,
                &light_spec(&[0_usize], LightEffect::Flicker, Some(64_i32))
            )
            .expect("a level alone still counts")
            .changed,
            vec![0_usize],
            "the light moved even though the special did not"
        );
        assert_eq!(light_of(&map, 0), 64_i32);
        assert_eq!(light_of(&map, 1), 176_i32, "the unlisted level stands");
    }

    #[test]
    fn the_light_step_names_the_sector_the_map_does_not_hold() {
        let mut map = rooms_special([0_i32, 0_i32]);
        let message = refuse(
            &set_lights(
                &mut map,
                &light_spec(&[0_usize, 42_usize], LightEffect::Glow, None),
            )
            .unwrap_err(),
        );
        assert!(message.contains("sector 42 is out of range"), "{message}");
        assert_eq!(special_of(&map, 0), 0_i32, "a refusal wrote nothing");
    }
}
