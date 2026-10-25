//! `add_door`: the door parameters, skins, spec and report beside the
//! whole-door build.
//!
//! `add_door` composes the pure engines: [`crate::wad::map::features`]
//! selects the wall ([`crate::wad::map::features::longest_boundary`],
//! [`crate::wad::map::features::nearest_boundary`]), fits the cut
//! ([`crate::wad::map::features::door_wall`] through
//! [`crate::wad::map::features::door_segment`]), measures a one-sided
//! wall's outward reach ([`crate::wad::map::features::outward_depth`],
//! [`crate::wad::map::geometry::point_in_sector`]) and dresses the
//! finished doorway ([`crate::wad::map::features::door_specials`],
//! [`crate::wad::map::features::apply_door`]), with
//! [`crate::wad::map::overlay::OverlayResult::linedef_map`] confirming
//! the wall survived the rebuild; [`crate::wad::map::overlay`] does
//! every drawing - the void strip that separates touching rooms and the
//! door recess itself are one [`crate::wad::map::overlay::apply_overlay`]
//! paint each, all of it inside a single undoable edit.
//!
//! The opening rule: after the recess is drawn, every two-sided line
//! facing the door sector is a room-facing door line (the vanilla
//! multi-room doorway: each opening carries the action and acts on the
//! sector behind it), and every one-sided line facing it is a jamb that
//! [`crate::wad::map::features::apply_door`] closes into a floor-pinned
//! track wall.
//!
//! [`choose_wall`]: crate::tools::features::wall::choose_wall
//! [`carve_strip`]: crate::tools::features::wall::carve_strip
//! [`strip_wall`]: crate::tools::features::wall::strip_wall

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{PointParam, edit_result_fragment};
use crate::wad::map::features::{
    DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, next_free_tag,
};
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, OverlayResult, Paint, WallTextures};
use crate::wad::map::{Linedef, Map, Sector};

use super::wall::{carve_strip, choose_wall, place_rect, strip_face_lost, strip_wall, to_pts};
use super::{NO_TEXTURE, current_map, invalid, sector_lost, theme_skin, wall_lost};

/// The `add_door` default door width in map units: one `DOOR1` panel.
pub(super) const DEFAULT_WIDTH: i32 = 64_i32;

/// How far the outward ray may search for the room behind a one-sided wall.
pub(super) const MAX_DEPTH: i32 = 256_i32;

/// The `add_door` default style name.
const DEFAULT_STYLE: &str = "normal";

/// Arguments for [`Cyberdemon::add_door`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddDoorParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The two sectors the door joins, as `[sectorA, sectorB]`: exactly one
    /// of `between` and `linedef`. Sector A lends the recess its heights.
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
    pub(super) at: Option<PointParam>,
    /// The doorway's opening width in map units (default 64).
    #[serde(default)]
    pub(super) width: Option<i32>,
    /// The recess depth in map units, one-sided walls only (default:
    /// measured by an outward ray, up to 256 units). Ignored for two-sided
    /// walls, whose recess always spans the carved strip.
    #[serde(default)]
    pub(super) depth: Option<i32>,
    /// The door action (default `"normal"`): `normal`, `fast`, `stay_open`,
    /// `locked_red`, `locked_blue`, `locked_yellow` or `remote`.
    #[serde(default)]
    pub(super) style: Option<String>,
    /// The moving door face texture (default: the theme's `door`).
    #[serde(default)]
    pub(super) texture: Option<String>,
    /// The jamb track texture (default: the theme's `door_track`, or its
    /// coloured row for the locked styles).
    #[serde(default)]
    pub(super) track_texture: Option<String>,
}

/// The wall the parameters name, already checked for "exactly one".
#[derive(Debug, Clone, Copy)]
pub(super) enum Target {
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
pub(super) struct DoorSkins {
    /// The moving door face: the room-facing uppers.
    pub(super) door: String,
    /// The jamb track.
    pub(super) track: String,
    /// The recess floor flat; `"-"` means keep room A's flat.
    pub(super) floor_flat: String,
    /// The plain wall texture bare carved edges fall back to.
    pub(super) wall: String,
}

/// Everything [`build_door`] needs once the wire parameters resolved.
#[derive(Debug)]
pub(super) struct DoorSpec {
    /// The wall the door is cut into.
    pub(super) target: Target,
    /// The requested cut centre, if any.
    pub(super) at: Option<Point>,
    /// The opening width in map units.
    pub(super) width: i32,
    /// The requested recess depth, if any (one-sided walls only).
    pub(super) depth: Option<i32>,
    /// The action to install.
    pub(super) style: DoorStyle,
    /// The textures to dress with.
    pub(super) skins: DoorSkins,
}

/// The wall the door is cut into, resolved against the map.
#[derive(Debug)]
pub(super) struct Chosen {
    /// The sector the recess copies its heights from.
    pub(super) room_a: usize,
    /// The sector across the shared boundary (room A itself when the wall
    /// is one-sided).
    pub(super) room_b: usize,
    /// Index in [`Map::linedefs`] of the wall line (its pre-carve index
    /// while [`Chosen::touching`]).
    pub(super) wall: usize,
    /// The cut centre on the wall line.
    pub(super) centre: Point,
    /// Whether the wall separates two sectors: their whole shared boundary
    /// gets a void strip first.
    pub(super) touching: bool,
}

/// The door rectangle, ready to paint, with what the drawing needs.
#[derive(Debug)]
pub(super) struct Placed {
    /// Index of the wall the rectangle's near edge sits on (current map).
    pub(super) wall: usize,
    /// The room A sector the recess record copies.
    pub(super) room_a: usize,
    /// The rectangle's corners, in order.
    pub(super) rect: Vec<Point>,
    /// The tag minted for a [`DoorStyle::Remote`] door's sector.
    pub(super) tag: Option<i32>,
}

/// The lines around a drawn door recess, split by role.
#[derive(Debug)]
pub(super) struct Groups {
    /// The two-sided room-facing lines that carry the action.
    pub(super) lines: Vec<usize>,
    /// The one-sided recess walls that become the tracks.
    pub(super) jambs: Vec<usize>,
}

/// The drawn recess and its roles, ready for [`dress_door`].
#[derive(Debug)]
pub(super) struct Dress {
    /// Index of the door sector the recess became.
    pub(super) door: usize,
    /// The openings and the jambs.
    pub(super) groups: Groups,
    /// The tag the door's own lines answer with (remote doors).
    pub(super) tag: i32,
}

/// What one [`build_door`] step did, as the tool reports it.
#[derive(Debug, Serialize)]
pub(super) struct BuiltDoor {
    /// Index of the door sector the recess became.
    pub(super) door_sector: usize,
    /// Indices of the room-facing lines carrying the action.
    pub(super) lines: Vec<usize>,
    /// Indices of the jamb (track) lines.
    pub(super) jambs: Vec<usize>,
    /// The tag the door sector carries (`remote` doors only).
    pub(super) tag: Option<i32>,
    /// What the drawings reported, plus the void-strip note.
    pub(super) notes: Vec<String>,
}

/// Structured result of [`Cyberdemon::add_door`].
#[derive(Serialize)]
pub(super) struct DoorReport {
    /// What the step built, inlined next to the style and edit fragment.
    #[serde(flatten)]
    built: BuiltDoor,
    /// The canonical style name the door got.
    style: &'static str,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The canonical wire name of `style`.
pub(super) const fn style_name(style: DoorStyle) -> &'static str {
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
pub(super) fn style_names() -> Vec<&'static str> {
    DoorStyle::all().map(style_name).to_vec()
}

/// The [`DoorStyle`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
pub(super) fn style_of(text: &str) -> Result<DoorStyle, CyberError> {
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
pub(super) const fn track_role(style: DoorStyle) -> Role {
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
/// unless exactly one of the two was passed. `joins` phrases what the two
/// sectors are for the message ("the door joins", "the window looks
/// between").
pub(super) fn target_of(
    between: Option<[usize; 2]>,
    linedef: Option<usize>,
    joins: &str,
) -> Result<Target, CyberError> {
    match (between, linedef) {
        (Some([a, b]), None) => Ok(Target::Between { a, b }),
        (None, Some(line)) => Ok(Target::Linedef(line)),
        _ => Err(invalid(&format!(
            "pass exactly one of between ([sectorA, sectorB] for the two rooms {joins}) \
             and linedef (the wall to cut)",
        ))),
    }
}

/// The [`Point`] a wire parameter names.
pub(super) const fn point_of(param: PointParam) -> Point {
    Point {
        x: param.x,
        y: param.y,
    }
}

/// The door sector record: room A's fields, the theme's recess floor
/// (room A's flat when the theme names none), no effect and the minted
/// tag.
pub(super) fn door_record(map: &Map, placed: &Placed, spec: &DoorSpec) -> Sector {
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
    let (front, back) = map.line_sectors(line);
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
pub(super) fn door_groups(map: &Map, door: usize) -> Groups {
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
pub(super) fn require_wall_survives(drawn: &OverlayResult, wall: usize) -> Result<(), CyberError> {
    if drawn.linedef_map.get(wall).copied().flatten().is_some() {
        return Ok(());
    }
    Err(wall_lost(wall))
}

/// Install the style's action and the dressing on the drawn doorway.
pub(super) fn dress_door(map: &mut Map, spec: &DoorSpec, dress: &Dress) -> Result<(), CyberError> {
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
pub(super) fn build_door(map: &mut Map, spec: &DoorSpec) -> Result<BuiltDoor, CyberError> {
    let mut chosen = choose_wall(map, spec.target, spec.at)?;
    let mut notes = Vec::new();
    if chosen.touching {
        notes = carve_strip(map, &chosen, &spec.skins.wall, "the door recess")?;
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

/// `add_door`'s work: resolve the parameters, then run the whole door as
/// one [`crate::session::Session::edit_map`] step.
pub(super) fn add_door_run(
    session: &mut Session,
    args: &AddDoorParams,
) -> Result<DoorReport, CyberError> {
    let style = style_of(args.style.as_deref().unwrap_or(DEFAULT_STYLE))?;
    let target = target_of(args.between, args.linedef, "the door joins")?;
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
