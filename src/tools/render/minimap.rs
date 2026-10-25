//! The `render_minimap` tool: the square automap-style window - size,
//! centre and radius resolved against the map, the vanilla line colours
//! optionally taken from `PLAYPAL`, the [`Canvas`] painted through the
//! shared [`crate::render::topdown`] passes and published as a `PNG`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::Definitions;
use crate::error::CyberError;
use crate::fixed::midpoint;
use crate::gfx::codec::encode_png;
use crate::gfx::palette::Palette;
use crate::render::{
    AutomapColors, Canvas, PlayerMark, RenderSettings, Rgba, Window, gamma_table,
    paint_automap_lines, paint_player_arrow, paint_thing_dots,
};
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::maps::common;
use crate::tools::maps::params::PointParam;
use crate::tools::palette::load_palette;
use crate::wad::map::Map;

use super::PNG_MIME;

/// Default minimap output edge, in pixels.
const DEFAULT_MINIMAP_SIZE: u32 = 256;
/// Smallest minimap output edge, in pixels.
const MIN_MINIMAP_SIZE: u32 = 32;
/// Largest minimap output edge, in pixels.
const MAX_MINIMAP_SIZE: u32 = 1_024;
/// Smallest minimap window radius, in map units.
const MIN_MINIMAP_RADIUS: u32 = 1;
/// Largest minimap window radius, in map units (`0x1_0000`, over the whole
/// vanilla coordinate range).
const MAX_MINIMAP_RADIUS: u32 = 0x1_0000;
/// Map units of slack added when the default radius fits the whole map.
pub(super) const MINIMAP_FIT_PAD: i64 = 1;
/// Name suffix of the published minimap files.
const MINIMAP_SUFFIX: &str = "_minimap.png";
/// The minimap background: black, as the in-game automap draws it.
const MINIMAP_BACKGROUND: Rgba = [0, 0, 0, 255];

/// Arguments for `render_minimap`' `player` parameter: the automap player
/// marker's position (the shared `{x, y}` point) and facing.
#[derive(Debug, Deserialize, JsonSchema)]
struct PlayerParams {
    /// The marker's world position.
    #[serde(flatten)]
    at: PointParam,
    /// Facing in degrees, counter-clockwise from east.
    angle: i64,
}

/// Arguments for [`Cyberdemon::render_minimap`]. Every field is optional;
/// the view defaults to the whole map and the colours to the vanilla
/// automap palette entries.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenderMinimapParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    map: Option<String>,
    /// Square output edge in pixels, `32..=1024` (default `256`).
    #[serde(default)]
    size: Option<u32>,
    /// World point the square window centres on (default: the map centre).
    #[serde(default)]
    center: Option<PointParam>,
    /// Map units from the centre to a window edge, `1..=65536` (default:
    /// fit the whole map).
    #[serde(default)]
    radius: Option<u32>,
    /// Draw the white automap player arrow here, facing `angle` degrees
    /// counter-clockwise from east.
    #[serde(default)]
    player: Option<PlayerParams>,
    /// Mark things with `2x2` px category-coloured dots (default `false`).
    #[serde(default)]
    show_things: Option<bool>,
    /// Take the automap colours from `PLAYPAL` palette 0 when the session
    /// has one (default `true`); off, or without a `PLAYPAL`, uses the RGB
    /// defaults.
    #[serde(default)]
    palette_colors: Option<bool>,
}

/// One [`Cyberdemon::render_minimap`] call's fully resolved arguments.
#[derive(Debug)]
pub(super) struct Minimap {
    /// Square output edge in pixels.
    pub(super) size: u32,
    /// The square world window to frame.
    pub(super) window: Window,
    /// The automap colours to draw with.
    pub(super) colors: AutomapColors,
    /// Whether things get category-coloured dots.
    pub(super) show_things: bool,
    /// The player arrow to draw, when given.
    pub(super) player: Option<PlayerMark>,
}

/// Reads the minimap's inputs under the session lock: the map, its thing
/// definitions, the settings (for `gamma`) and the palette the automap
/// colours can come from.
struct PreparedMinimap {
    /// The map to render, from the merged view.
    map: Map,
    /// The session's thing definitions.
    defs: Definitions,
    /// The session's render settings.
    settings: RenderSettings,
    /// The merged view's `PLAYPAL` palette 0, when there is one.
    palette: Option<Palette>,
}

/// [`Cyberdemon::render_minimap`]'s result: the framed window and the
/// published `PNG` asset.
#[derive(Serialize)]
pub(super) struct MinimapReport {
    /// The rendered map's name.
    map: String,
    /// The square output edge in pixels.
    size: u32,
    /// The world centre of the framed window.
    center: PointReport,
    /// Map units from the centre to a window edge.
    radius: i64,
    /// The published minimap.
    asset: AssetRef,
}

/// The `center` object of a [`MinimapReport`].
#[derive(Serialize)]
struct PointReport {
    /// World `x`.
    x: i64,
    /// World `y`.
    y: i64,
}

/// The refusal for a map with no bounds to frame when the window was not
/// given outright.
fn minimap_needs_window(map: &Map) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{} has no vertices: pass center {{x, y}} and radius to render_minimap so the window \
         is known",
        map.name
    ))
}

/// The square output edge: the validated `requested` size, else
/// [`DEFAULT_MINIMAP_SIZE`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an edge outside the documented range.
pub(super) fn resolve_size(requested: Option<u32>) -> Result<u32, CyberError> {
    let size = requested.unwrap_or(DEFAULT_MINIMAP_SIZE);
    if (MIN_MINIMAP_SIZE..=MAX_MINIMAP_SIZE).contains(&size) {
        return Ok(size);
    }
    Err(CyberError::InvalidArgument(format!(
        "render_minimap size must be within {MIN_MINIMAP_SIZE}..={MAX_MINIMAP_SIZE} pixels, \
         got {size}"
    )))
}

/// The window radius: the validated `requested` value, else half the map's
/// larger span rounded up plus [`MINIMAP_FIT_PAD`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a radius outside the documented
/// range, and [`minimap_needs_window`] for an empty map.
pub(super) fn resolve_radius(requested: Option<u32>, map: &Map) -> Result<i64, CyberError> {
    if let Some(value) = requested {
        if !(MIN_MINIMAP_RADIUS..=MAX_MINIMAP_RADIUS).contains(&value) {
            return Err(CyberError::InvalidArgument(format!(
                "render_minimap radius must be within {MIN_MINIMAP_RADIUS}..={MAX_MINIMAP_RADIUS} \
                 map units, got {value}"
            )));
        }
        return Ok(i64::from(value));
    }
    let (min_x, min_y, max_x, max_y) = map.bounds().ok_or_else(|| minimap_needs_window(map))?;
    let span_x = i64::from(max_x).saturating_sub(i64::from(min_x));
    let span_y = i64::from(max_y).saturating_sub(i64::from(min_y));
    let half = span_x
        .max(span_y)
        .saturating_add(1)
        .checked_div(2)
        .unwrap_or(1);
    Ok(half.saturating_add(MINIMAP_FIT_PAD).max(1))
}

/// The window centre: the `requested` point, else the midpoint of the
/// map's bounds.
///
/// # Errors
///
/// [`minimap_needs_window`] for an empty map without a given centre.
pub(super) fn resolve_centre(
    requested: Option<&PointParam>,
    map: &Map,
) -> Result<(i64, i64), CyberError> {
    if let Some(point) = requested {
        return Ok(point.to_wide());
    }
    let (min_x, min_y, max_x, max_y) = map.bounds().ok_or_else(|| minimap_needs_window(map))?;
    Ok((
        midpoint(i64::from(min_x), i64::from(max_x)),
        midpoint(i64::from(min_y), i64::from(max_y)),
    ))
}

/// The automap colours: `palette`' vanilla automap entries when `wanted`
/// and a `PLAYPAL` is loaded, else the RGB defaults.
fn minimap_colors(wanted: bool, palette: Option<&Palette>) -> AutomapColors {
    match (wanted, palette) {
        (true, Some(found)) => AutomapColors::from_palette(found),
        (true, None) | (false, _) => AutomapColors::DEFAULT,
    }
}

/// The call's resolved arguments: size and radius validated, the centre
/// and radius defaulted from the map bounds, colours from the palette or
/// the defaults.
///
/// # Errors
///
/// [`resolve_size`], [`resolve_radius`] and [`resolve_centre`]'s errors.
pub(super) fn resolve_minimap(
    args: &RenderMinimapParams,
    map: &Map,
    palette: Option<&Palette>,
) -> Result<Minimap, CyberError> {
    let size = resolve_size(args.size)?;
    let radius = resolve_radius(args.radius, map)?;
    let (centre_x, centre_y) = resolve_centre(args.center.as_ref(), map)?;
    let player = args.player.as_ref().map(|mark| {
        let wide = mark.at.to_wide();
        PlayerMark {
            x: wide.0,
            y: wide.1,
            angle: mark.angle,
        }
    });
    Ok(Minimap {
        size,
        window: Window::new(size, centre_x, centre_y, radius),
        colors: minimap_colors(args.palette_colors.unwrap_or(true), palette),
        show_things: args.show_things.unwrap_or(false),
        player,
    })
}

/// Paints the whole minimap: black ground, automap lines, optional thing
/// dots and the player arrow on top.
pub(super) fn paint_minimap(map: &Map, defs: &Definitions, request: &Minimap) -> Canvas {
    let mut canvas = Canvas::new(request.size, request.size, MINIMAP_BACKGROUND);
    paint_automap_lines(map, &request.window, &request.colors, &mut canvas);
    if request.show_things {
        paint_thing_dots(map, defs, &request.window, &mut canvas);
    }
    if let Some(mark) = request.player.as_ref() {
        paint_player_arrow(mark, &request.window, &request.colors, &mut canvas);
    }
    canvas
}

/// Reads the minimap's inputs under the session lock.
///
/// # Errors
///
/// [`common::resolve_map_name`] and [`common::load_map`]'s errors, and the
/// definition-collection errors of [`crate::session::Session::definitions`].
fn load_minimap(state: &AppState, requested: Option<&str>) -> Result<PreparedMinimap, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let defs = session.definitions()?;
        Ok(PreparedMinimap {
            map,
            defs,
            settings: session.render_settings().clone(),
            palette: load_palette(session, 0).ok(),
        })
    })
}

/// `render_minimap`' work: resolve, paint, gamma, encode, publish.
///
/// # Errors
///
/// [`load_minimap`] and [`resolve_minimap`]'s errors,
/// [`encode_png`]' and [`AppState::publish`]' failures.
pub(super) fn minimap_report(
    state: &AppState,
    args: &RenderMinimapParams,
) -> Result<MinimapReport, CyberError> {
    let prepared = load_minimap(state, args.map.as_deref())?;
    let request = resolve_minimap(args, &prepared.map, prepared.palette.as_ref())?;
    let mut canvas = paint_minimap(&prepared.map, &prepared.defs, &request);
    canvas.apply_gamma(&gamma_table(prepared.settings.gamma));
    let png = encode_png(&canvas.to_rgba_image(), None)?;
    let asset = state.publish(
        &format!("{}{MINIMAP_SUFFIX}", prepared.map.name),
        PNG_MIME,
        png,
    )?;
    let (centre_x, centre_y) = request.window.centre();
    Ok(MinimapReport {
        map: prepared.map.name.to_string(),
        size: request.size,
        center: PointReport {
            x: centre_x,
            y: centre_y,
        },
        radius: request.window.radius(),
        asset,
    })
}
