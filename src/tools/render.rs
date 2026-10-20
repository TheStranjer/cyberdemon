//! Render tools: the session-level defaults the rendering tools use, and
//! the first renderer built on them.
//!
//! Three settings tools, no rendering hidden in them:
//! [`Cyberdemon::set_render_resolution`] and
//! [`Cyberdemon::set_render_options`] change
//! [`crate::render::RenderSettings`] on the session,
//! [`Cyberdemon::get_render_settings`] reports them. Every change is
//! validated before it lands and every result is the full settings, so a
//! client never has to remember what it set earlier.
//!
//! [`Cyberdemon::render_topdown`] draws a whole map overview onto a
//! [`crate::render::Canvas`]: the flat editor/automap line map, sector
//! light fills, vertex marks and thing markers, or the isometric
//! wireframe. It resolves its defaults from the settings above, reads the
//! map and the session's thing definitions under the session lock, then
//! paints, gamma-corrects and publishes the `PNG` outside it, returning
//! the fitted `transform` so a client can convert pixels back to map
//! coordinates. On the top-down projection it also paints verification
//! overlays: a fetched reference picture placed through
//! [`crate::plan::Units`] under the map ([`load_underlay`] runs before
//! any session lock), a coordinate grid, element-index labels and free
//! [`crate::render::draw_text`] annotations on top, all drawn in the
//! [`crate::render::font`] bitmap type.
//!
//! [`Cyberdemon::render_minimap`] draws a compact automap-style square:
//! it reuses the line classification, colours and [`crate::render::Canvas`]
//! of the top-down view through [`crate::render::topdown`], frames a
//! centred [`crate::render::Window`] of the map and publishes the `PNG`
//! the same way.
//!
//! [`Cyberdemon::render_view`] casts a first-person frame: it prepares a
//! [`crate::render::Scene`] (map, palette, `COLORMAP`, textures, flats,
//! sky) under the session lock with the call's setting overrides, dresses
//! the map's things as [`crate::render::sprites::SpriteItem`]s through the
//! session's definitions and sprite resolvers, runs the integer column
//! caster in [`crate::render::view`] with those items, and publishes the
//! gamma-corrected `PNG` like its siblings. The report counts the layers
//! the transparent pass painted: masked-middle screen columns and sprites.

use std::collections::{HashMap, HashSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::{Definitions, ThingDef, session_definitions};
use crate::error::CyberError;
use crate::fixed::{Angle, FRACBITS, mul_div_round, rotate_point};
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::parse_hex_color;
use crate::gfx::palette::Palette;
use crate::gfx::patch::Patch;
use crate::gfx::sprite_rotation::rotation_for;
use crate::plan::Units;
use crate::render::canvas::intersect_x;
use crate::render::sprites::SpriteItem;
use crate::render::view::ONE_16;
use crate::render::{
    AutomapColors, Canvas, Edge, PlayerMark, Pt, RenderSettings, Rgba, Stroke, Switch, TextStyle,
    TopdownStyle, ViewCamera, ViewFrame, Window, automap_color, build_scene, camera_sector,
    check_pitch, default_eye_z, draw_text, gamma_table, paint_automap_lines, paint_player_arrow,
    paint_thing_dots, render_frame, scale_down, side_sector, thing_visual,
};
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::resolve::load_sprite_cached;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::palette::load_palette;
use crate::tools::plan::params::UnitsParam;
use crate::tools::{fail, ok};
use crate::wad::LumpName;
use crate::wad::map::geometry;
use crate::wad::map::{Linedef, Map, Thing, Vertex};

/// Arguments for [`Cyberdemon::set_render_resolution`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetRenderResolutionParams {
    /// Output width in pixels, `16..=4096`.
    width: u32,
    /// Output height in pixels, `16..=4096`; `width * height` must not
    /// exceed 16777216 pixels.
    height: u32,
}

/// Arguments for [`Cyberdemon::set_render_options`]. Every field is optional
/// and at least one has to be present; the fields left out keep the value
/// the session already has.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct SetRenderOptionsParams {
    /// Lighting model for 3D views: `"doom"` (sector light plus the
    /// `COLORMAP` distance fade), `"flat"` (sector light only) or
    /// `"fullbright"` (no shading).
    #[serde(default)]
    lighting: Option<String>,
    /// Whether top-down renders mark thing lumps.
    #[serde(default)]
    show_things: Option<bool>,
    /// Whether the 3D view draws sprites for things.
    #[serde(default)]
    show_sprites: Option<bool>,
    /// Whether the 3D view draws the sky texture; off means the plain
    /// background colour.
    #[serde(default)]
    sky: Option<bool>,
    /// Horizontal field of view in degrees, `10..=170`.
    #[serde(default)]
    fov: Option<u32>,
    /// Gamma level applied to every output channel, `0..=4`.
    #[serde(default)]
    gamma: Option<u8>,
    /// How top-down renders draw the map: `"editor"` or `"automap"`.
    #[serde(default)]
    topdown_style: Option<String>,
    /// Background colour as `"#RRGGBB"` hex.
    #[serde(default)]
    background: Option<String>,
}

impl SetRenderOptionsParams {
    /// Whether the call carries no option at all, which is refused rather
    /// than silently doing nothing.
    #[must_use]
    const fn is_empty(&self) -> bool {
        self.lighting.is_none()
            && self.show_things.is_none()
            && self.show_sprites.is_none()
            && self.sky.is_none()
            && self.fov.is_none()
            && self.gamma.is_none()
            && self.topdown_style.is_none()
            && self.background.is_none()
    }
}

/// The names a client can pass to [`Cyberdemon::set_render_options`], for the
/// message that refuses a call with no usable option.
const OPTION_NAMES: &str = "lighting, show_things, show_sprites, sky, fov, gamma, \
                            topdown_style, background";
/// Copies the options `args` carries onto `settings`, all or nothing: each
/// value is validated onto a copy and the copy is assigned only once every
/// field passed.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] from the first refused value; `settings`
/// then keeps what it had.
fn apply_options(
    settings: &mut RenderSettings,
    args: &SetRenderOptionsParams,
) -> Result<(), CyberError> {
    let mut candidate = settings.clone();
    if let Some(text) = args.lighting.as_deref() {
        candidate.set_lighting(text)?;
    }
    if let Some(value) = args.show_things {
        candidate.show_things = Switch::from(value);
    }
    if let Some(value) = args.show_sprites {
        candidate.show_sprites = Switch::from(value);
    }
    if let Some(value) = args.sky {
        candidate.sky = Switch::from(value);
    }
    if let Some(value) = args.fov {
        candidate.set_fov(value)?;
    }
    if let Some(value) = args.gamma {
        candidate.set_gamma(value)?;
    }
    if let Some(text) = args.topdown_style.as_deref() {
        candidate.set_topdown_style(text)?;
    }
    if let Some(text) = args.background.as_deref() {
        candidate.set_background(text)?;
    }
    *settings = candidate;
    Ok(())
}

/// `set_render_resolution`'s work: validate the size onto the session and
/// report the whole settings.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an edge outside the documented range
/// or a total over the pixel cap; [`CyberError::Internal`] on a poisoned
/// session lock.
fn resolution_report(
    state: &AppState,
    width: u32,
    height: u32,
) -> Result<RenderSettings, CyberError> {
    state.with_session(|session| {
        let settings = session.render_settings_mut();
        settings.set_resolution(width, height)?;
        Ok(session.render_settings().clone())
    })
}

/// `set_render_options`' work: refuse a call that changes nothing, otherwise
/// apply the given options to the session and report the whole settings.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when no option is given or a value is
/// refused; [`CyberError::Internal`] on a poisoned session lock.
fn options_report(
    state: &AppState,
    args: &SetRenderOptionsParams,
) -> Result<RenderSettings, CyberError> {
    if args.is_empty() {
        return Err(CyberError::InvalidArgument(format!(
            "set_render_options needs at least one option: pass any of {OPTION_NAMES} and leave \
             out the ones to keep"
        )));
    }
    state.with_session(|session| {
        apply_options(session.render_settings_mut(), args)?;
        Ok(session.render_settings().clone())
    })
}

/// The projection one [`Cyberdemon::render_topdown`] call draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Projection {
    /// The flat north-up overview: the default.
    Topdown,
    /// The `2:1` wireframe on the plane `(x - y, (x + y) / 2 - z)`.
    Isometric,
}

impl Projection {
    /// The `projection` result field and accepted parameter name.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Topdown => "topdown",
            Self::Isometric => "isometric",
        }
    }
}

/// Arguments for [`Cyberdemon::render_topdown`]. Every field is optional;
/// the defaults come from the session's render settings.
#[derive(Debug, Deserialize, JsonSchema)]
struct RenderTopdownParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    map: Option<String>,
    /// Output width in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    width: Option<u32>,
    /// Output height in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    height: Option<u32>,
    /// `"topdown"` (the default) or `"isometric"`.
    #[serde(default)]
    projection: Option<String>,
    /// The world rectangle to frame; defaults to the map bounds padded by
    /// 32 units. Needs `min_x < max_x` and `min_y < max_y`.
    #[serde(default)]
    bounds: Option<BoundsParams>,
    /// Draw thing markers (default: the `show_things` render setting).
    #[serde(default)]
    show_things: Option<bool>,
    /// Draw a `3x3` mark per vertex (default `false`).
    #[serde(default)]
    show_vertices: Option<bool>,
    /// Fill each sector with a grey of half its light level under the
    /// lines (default `false`).
    #[serde(default)]
    fill_sectors: Option<bool>,
    /// Line colours: `"editor"` or `"automap"` (default: the
    /// `topdown_style` render setting).
    #[serde(default)]
    style: Option<String>,
    /// Element indices to draw as highlights.
    #[serde(default)]
    highlight: Option<HighlightParams>,
    /// Reference image drawn under the map (top-down projection only).
    #[serde(default)]
    underlay: Option<UnderlayParams>,
    /// Grid spacing in map units, `8..=1024`, drawn under the map lines
    /// (top-down projection only).
    #[serde(default)]
    grid: Option<i64>,
    /// Which element indices to draw as text over the map (top-down
    /// projection only).
    #[serde(default)]
    labels: Option<LabelsParams>,
    /// Free text at map coordinates drawn over the map (top-down
    /// projection only). At most [`MAX_ANNOTATIONS`] rows of at most
    /// [`MAX_ANNOTATION_CHARS`] characters each.
    #[serde(default)]
    annotations: Option<Vec<AnnotationParams>>,
    /// Magnification of every label and annotation glyph, `1..=4`
    /// (default `2` when the output width is at least 1024 pixels, else `1`).
    #[serde(default)]
    label_scale: Option<u32>,
}

/// Arguments for `render_topdown`' `underlay` parameter: the reference
/// picture to draw under the map.
#[derive(Debug, Deserialize, JsonSchema)]
struct UnderlayParams {
    /// File to draw: absolute path, `file://` URI, `http(s)://` `URL` or
    /// this server's asset `URL`.
    source: String,
    /// How image pixels map to map units — exactly the object
    /// `build_map_from_plan` takes as `units` (no `grid`: the underlay
    /// never snaps). Default identity: `scale_num: 1`, `scale_den: 1`,
    /// origins `0`, `flip_y: false`.
    #[serde(default)]
    units: Option<UnitsParam>,
    /// Blend strength in percent, `0..=100` (default `50`); each texel's
    /// own alpha is scaled by this.
    #[serde(default)]
    opacity: Option<u8>,
}

/// Arguments for `render_topdown`' `labels` parameter: which element
/// indices to draw.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct LabelsParams {
    /// Draw each sector's index inside it.
    #[serde(default)]
    sectors: Option<bool>,
    /// Draw each linedef's index at its midpoint.
    #[serde(default)]
    linedefs: Option<bool>,
    /// Draw each thing's index beside its marker.
    #[serde(default)]
    things: Option<bool>,
    /// Draw each vertex's index beside its mark.
    #[serde(default)]
    vertices: Option<bool>,
}

/// Arguments for one `render_topdown`' `annotations` row.
#[derive(Debug, Deserialize, JsonSchema)]
struct AnnotationParams {
    /// World `x` of the text anchor.
    x: i64,
    /// World `y` of the text anchor.
    y: i64,
    /// The text; upper-cased, characters outside the font draw as `?`,
    /// at most [`MAX_ANNOTATION_CHARS`] characters.
    text: String,
    /// Text colour as `"#RRGGBB"` (default `"#FFFFFF"`).
    #[serde(default)]
    color: Option<String>,
}

/// Arguments for `render_topdown`'s `bounds` parameter: the world
/// rectangle to frame.
#[derive(Debug, Deserialize, JsonSchema)]
struct BoundsParams {
    /// World `x` of the west edge.
    min_x: i64,
    /// World `y` of the south edge.
    min_y: i64,
    /// World `x` of the east edge.
    max_x: i64,
    /// World `y` of the north edge.
    max_y: i64,
}

/// Arguments for `render_topdown`'s `highlight` parameter.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct HighlightParams {
    /// Linedef indices to draw red and thick.
    #[serde(default)]
    linedefs: Option<Vec<usize>>,
    /// Sector indices to tint red.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// Thing indices to ring in white.
    #[serde(default)]
    things: Option<Vec<usize>>,
    /// Vertex indices to draw red.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
}

/// The decoded [`UnderlayParams`]: the picture, its map placement and the
/// blend strength it draws at.
#[derive(Debug)]
struct Underlay {
    /// The decoded reference picture.
    image: RgbaImage,
    /// Image-pixel to map-unit transform, unsnapped
    /// ([`grid`](Units::grid) `1`).
    units: Units,
    /// Blend strength in percent, `0..=100`.
    opacity: u8,
    /// The underlay's map-space rectangle: the four image corners
    /// through [`Units::to_map`].
    rect: Rect,
}

/// Which element indices [`render_topdown`](Cyberdemon::render_topdown)
/// draws as text.
#[derive(Clone, Copy, Debug, Default)]
struct Labels {
    /// Sector indices.
    sectors: Switch,
    /// Linedef indices.
    linedefs: Switch,
    /// Thing indices.
    things: Switch,
    /// Vertex indices.
    vertices: Switch,
}

impl Labels {
    /// Whether any label pass has to run.
    const fn any(self) -> bool {
        self.sectors.is_on()
            || self.linedefs.is_on()
            || self.things.is_on()
            || self.vertices.is_on()
    }
}

/// One resolved `annotations` row.
#[derive(Debug)]
struct Annotation {
    /// World `x` of the text anchor.
    x: i64,
    /// World `y` of the text anchor.
    y: i64,
    /// The upper-cased text.
    text: String,
    /// The text colour.
    colour: Rgba,
}

/// A world rectangle as `(min_x, min_y, max_x, max_y)`; [`union_rect`]
/// folds these.
type Rect = (i64, i64, i64, i64);

/// The world rectangle a render frames; validation guarantees strictly
/// positive spans.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    /// West edge.
    min_x: i64,
    /// South edge.
    min_y: i64,
    /// East edge.
    max_x: i64,
    /// North edge.
    max_y: i64,
}

/// Resolved highlight lists, already checked against the map.
#[derive(Debug, Default)]
struct Highlights {
    /// Linedef indices to draw red and thick.
    linedefs: Vec<usize>,
    /// Sector indices to tint red.
    sectors: Vec<usize>,
    /// Thing indices to ring in white.
    things: Vec<usize>,
    /// Vertex indices to draw red.
    vertices: Vec<usize>,
}

/// One [`Cyberdemon::render_topdown`] call's fully resolved arguments.
#[derive(Debug)]
struct Request {
    /// Which projection to draw.
    projection: Projection,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// Line-colour style (top-down renders).
    style: TopdownStyle,
    /// The world rectangle to frame.
    bounds: Bounds,
    /// Background pixel colour from the render settings.
    background: Rgba,
    /// Thing-marker switch.
    show_things: Switch,
    /// Vertex-mark switch.
    show_vertices: Switch,
    /// Sector-fill switch.
    fill_sectors: Switch,
    /// Highlighted elements.
    highlight: Highlights,
    /// The reference picture drawn under the map, when given.
    underlay: Option<Underlay>,
    /// Grid spacing in map units, when given.
    grid: Option<i64>,
    /// Which element indices draw their numbers.
    labels: Labels,
    /// Free texts standing at map coordinates.
    annotations: Vec<Annotation>,
    /// Magnification of every label and annotation glyph.
    label_scale: u32,
}

/// The axis range a fitted view must place inside the canvas margins.
struct Plane {
    /// Smallest horizontal plane coordinate.
    min_u: i64,
    /// Largest horizontal plane coordinate.
    max_u: i64,
    /// Smallest vertical plane coordinate.
    min_v: i64,
    /// Largest vertical plane coordinate.
    max_v: i64,
}

/// The integer plane-to-pixel mapping one render fits:
/// `px = offset_x + u * num / den`, `py = offset_y - v * num / den`
/// (floor-rounded), `y` flipped so north is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    /// Scale numerator: the fitted pixel span.
    num: i64,
    /// Scale denominator: the matching plane span (`>= 1`).
    den: i64,
    /// Pixel column of plane coordinate `0`.
    offset_x: i64,
    /// Pixel row of plane coordinate `0`.
    offset_y: i64,
}

impl View {
    /// The plane distance `value` measures in pixels (floor-rounded).
    fn scale(self, value: i64) -> i64 {
        scale_down(value, self.num, self.den)
    }

    /// The pixel of plane point `(u, v)`.
    fn point(self, u: i64, v: i64) -> Pt {
        (
            self.offset_x.saturating_add(self.scale(u)),
            self.offset_y.saturating_sub(self.scale(v)),
        )
    }

    /// The pixel of world point `x`/`y`/`z` under the isometric projection.
    fn iso(self, x: i64, y: i64, z: i64) -> Pt {
        self.point(
            x.saturating_sub(y),
            half(x.saturating_add(y)).saturating_sub(z),
        )
    }

    /// The world point whose pixel lands at canvas `(col, row)`:
    /// [`point`](Self::point)'s mapping inverted in integer arithmetic,
    /// floor-rounded. `self.num` is at least `1` for every fitted view,
    /// so the division always has a non-zero denominator.
    fn unproject(self, col: i64, row: i64) -> (i64, i64) {
        (
            scale_down(col.saturating_sub(self.offset_x), self.den, self.num),
            scale_down(self.offset_y.saturating_sub(row), self.den, self.num),
        )
    }

    /// The `transform` object this view reports.
    fn report(self) -> TransformReport {
        TransformReport {
            scale_num: u64::try_from(self.num).unwrap_or(u64::MAX),
            scale_den: u64::try_from(self.den).unwrap_or(u64::MAX),
            offset_x: self.offset_x,
            offset_y: self.offset_y,
        }
    }
}

/// The largest integer ratio `num / den` that fits `plane` into the canvas
/// minus [`MARGIN_PX`] on every side, centred in what is left.
fn fit(plane: &Plane, width: u32, height: u32) -> View {
    let avail_w = i64::from(width)
        .saturating_sub(MARGIN_PX.saturating_mul(2))
        .max(1);
    let avail_h = i64::from(height)
        .saturating_sub(MARGIN_PX.saturating_mul(2))
        .max(1);
    let span_u = plane.max_u.saturating_sub(plane.min_u).max(1);
    let span_v = plane.max_v.saturating_sub(plane.min_v).max(1);
    let wide_fit = i128::from(avail_w).saturating_mul(i128::from(span_v));
    let tall_fit = i128::from(avail_h).saturating_mul(i128::from(span_u));
    let (num, den) = if wide_fit <= tall_fit {
        (avail_w, span_u)
    } else {
        (avail_h, span_v)
    };
    let (u0, u1) = (
        scale_down(plane.min_u, num, den),
        scale_down(plane.max_u, num, den),
    );
    let (v0, v1) = (
        scale_down(plane.min_v, num, den),
        scale_down(plane.max_v, num, den),
    );
    let slack_x = avail_w
        .saturating_sub(u1.saturating_sub(u0))
        .checked_div(2)
        .unwrap_or(0);
    let slack_y = avail_h
        .saturating_sub(v1.saturating_sub(v0))
        .checked_div(2)
        .unwrap_or(0);
    View {
        num,
        den,
        offset_x: MARGIN_PX.saturating_add(slack_x).saturating_sub(u0),
        offset_y: MARGIN_PX.saturating_add(slack_y).saturating_add(v1),
    }
}

/// The plane a top-down render frames: the world bounds themselves.
const fn bounds_plane(bounds: &Bounds) -> Plane {
    Plane {
        min_u: bounds.min_x,
        max_u: bounds.max_x,
        min_v: bounds.min_y,
        max_v: bounds.max_y,
    }
}

/// The plane an isometric render frames: the projected bounds spanning
/// `heights` (the map's floor-to-ceiling range).
fn iso_plane(bounds: &Bounds, heights: (i64, i64)) -> Plane {
    let (min_z, max_z) = heights;
    Plane {
        min_u: bounds.min_x.saturating_sub(bounds.max_y),
        max_u: bounds.max_x.saturating_sub(bounds.min_y),
        min_v: half(bounds.min_x.saturating_add(bounds.min_y)).saturating_sub(max_z),
        max_v: half(bounds.max_x.saturating_add(bounds.max_y)).saturating_sub(min_z),
    }
}

/// The map's `(lowest floor, highest ceiling)` as integers.
fn height_span(map: &Map) -> (i64, i64) {
    let mut span = (i64::MAX, i64::MIN);
    for sector in &map.sectors {
        span = (
            span.0.min(i64::from(sector.floor_height)),
            span.1.max(i64::from(sector.ceiling_height)),
        );
    }
    if span.0 <= span.1 { span } else { (0, 0) }
}

/// Floor-halving `value` (an arithmetic shift, so negatives round down).
fn half(value: i64) -> i64 {
    value.checked_shr(1).unwrap_or(0)
}

/// Moves a point by `offset` on both axes (saturating).
const fn offset_pt(point: Pt, offset: i64) -> Pt {
    (
        point.0.saturating_add(offset),
        point.1.saturating_add(offset),
    )
}

/// Parses `projection`, case-insensitively; absent means top-down.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the two projections.
fn parse_projection(text: Option<&str>) -> Result<Projection, CyberError> {
    let Some(name) = text else {
        return Ok(Projection::Topdown);
    };
    let lowered = name.to_ascii_lowercase();
    if lowered == "topdown" {
        return Ok(Projection::Topdown);
    }
    if lowered == "isometric" {
        return Ok(Projection::Isometric);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown projection {name:?}: use \"topdown\" for the flat north-up overview or \
         \"isometric\" for the 2:1 wireframe"
    )))
}

/// The rectangle to frame: the validated `requested` bounds; else the
/// union of the map's vertex bounds and the `underlay`'s map-space
/// rectangle, padded by [`BOUNDS_PAD`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an inverted or flat rectangle, and
/// for an empty map without an underlay or explicit bounds.
fn resolve_bounds(
    requested: Option<&BoundsParams>,
    map: &Map,
    underlay: Option<Rect>,
) -> Result<Bounds, CyberError> {
    if let Some(boxed) = requested {
        let bounds = Bounds {
            min_x: boxed.min_x,
            min_y: boxed.min_y,
            max_x: boxed.max_x,
            max_y: boxed.max_y,
        };
        if bounds.min_x >= bounds.max_x || bounds.min_y >= bounds.max_y {
            return Err(CyberError::InvalidArgument(format!(
                "render bounds need min_x < max_x and min_y < max_y, got x [{}, {}] and y \
                 [{}, {}]: widen the rectangle",
                bounds.min_x, bounds.max_x, bounds.min_y, bounds.max_y
            )));
        }
        return Ok(bounds);
    }
    let world = map
        .bounds()
        .map(|(x0, y0, x1, y1)| (i64::from(x0), i64::from(y0), i64::from(x1), i64::from(y1)));
    let frame = match (world, underlay) {
        (None, None) => {
            return Err(CyberError::InvalidArgument(format!(
                "{} has no vertices to frame: pass bounds {{min_x, min_y, max_x, max_y}} or an \
                 underlay to render it",
                map.name
            )));
        }
        (Some(rect), None) | (None, Some(rect)) => rect,
        (Some(one), Some(other)) => union_rect(one, other),
    };
    let (min_x, min_y, max_x, max_y) = frame;
    Ok(Bounds {
        min_x: min_x.saturating_sub(BOUNDS_PAD),
        min_y: min_y.saturating_sub(BOUNDS_PAD),
        max_x: max_x.saturating_add(BOUNDS_PAD),
        max_y: max_y.saturating_add(BOUNDS_PAD),
    })
}

/// The enclosing rectangle of two rects; an empty one (`min > max`)
/// contributes nothing.
fn union_rect(one: Rect, other: Rect) -> Rect {
    if other.0 > other.2 || other.1 > other.3 {
        return one;
    }
    if one.0 > one.2 || one.1 > one.3 {
        return other;
    }
    (
        one.0.min(other.0),
        one.1.min(other.1),
        one.2.max(other.2),
        one.3.max(other.3),
    )
}

/// Refuses the overlay parameters (`underlay`, `grid`, `labels`,
/// `annotations`, `label_scale`) for a projection that cannot draw them.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the overlays' home projection.
fn check_overlays(args: &RenderTopdownParams, projection: Projection) -> Result<(), CyberError> {
    if projection == Projection::Topdown {
        return Ok(());
    }
    let bare = args.underlay.is_none()
        && args.grid.is_none()
        && args.labels.is_none()
        && args.annotations.as_ref().is_none_or(Vec::is_empty)
        && args.label_scale.is_none();
    if bare {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(
        "overlays need the topdown projection: underlay, grid, labels, annotations and \
         label_scale only draw on the flat overview; drop projection \"isometric\" or the \
         overlay parameters"
            .to_owned(),
    ))
}

/// The validated `grid` spacing in map units, `8..=1024`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a spacing outside the range.
fn resolve_grid(requested: Option<i64>) -> Result<Option<i64>, CyberError> {
    let Some(spacing) = requested else {
        return Ok(None);
    };
    if (MIN_GRID..=MAX_GRID).contains(&spacing) {
        return Ok(Some(spacing));
    }
    Err(CyberError::InvalidArgument(format!(
        "render_topdown grid spacing must be within {MIN_GRID}..={MAX_GRID} map units, got \
         {spacing}: pass a coarser or finer grid, or drop the parameter"
    )))
}

/// The label switches, absent fields off.
fn resolve_labels(requested: Option<&LabelsParams>) -> Labels {
    let absent = LabelsParams::default();
    let wanted = requested.unwrap_or(&absent);
    Labels {
        sectors: Switch::from(wanted.sectors.unwrap_or(false)),
        linedefs: Switch::from(wanted.linedefs.unwrap_or(false)),
        things: Switch::from(wanted.things.unwrap_or(false)),
        vertices: Switch::from(wanted.vertices.unwrap_or(false)),
    }
}

/// The validated `label_scale`: `1..=4`, defaulting to [`WIDE_LABEL_SCALE`]
/// once the output is at least [`WIDE_LABEL_THRESHOLD`] pixels wide.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a scale outside the range.
fn resolve_label_scale(requested: Option<u32>, width: u32) -> Result<u32, CyberError> {
    let fallback = if width >= WIDE_LABEL_THRESHOLD {
        WIDE_LABEL_SCALE
    } else {
        DEFAULT_LABEL_SCALE
    };
    let scale = requested.unwrap_or(fallback);
    if (MIN_LABEL_SCALE..=MAX_LABEL_SCALE).contains(&scale) {
        return Ok(scale);
    }
    Err(CyberError::InvalidArgument(format!(
        "render_topdown label_scale must be within {MIN_LABEL_SCALE}..={MAX_LABEL_SCALE}, got \
         {scale}: drop it for the size-appropriate default"
    )))
}

/// The validated `annotations` rows, upper-cased and coloured.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for more than [`MAX_ANNOTATIONS`] rows,
/// a row longer than [`MAX_ANNOTATION_CHARS`] characters, or a colour
/// [`parse_hex_color`] refuses.
fn resolve_annotations(
    requested: Option<&[AnnotationParams]>,
) -> Result<Vec<Annotation>, CyberError> {
    let Some(rows) = requested else {
        return Ok(Vec::new());
    };
    if rows.len() > MAX_ANNOTATIONS {
        return Err(CyberError::InvalidArgument(format!(
            "render_topdown takes at most {MAX_ANNOTATIONS} annotations, got {}: keep the notes \
             that matter",
            rows.len()
        )));
    }
    rows.iter()
        .enumerate()
        .map(|(index, row)| row_note(index, row))
        .collect()
}

/// One annotation `row` under its list `index`.
fn row_note(index: usize, row: &AnnotationParams) -> Result<Annotation, CyberError> {
    if row.text.chars().count() > MAX_ANNOTATION_CHARS {
        return Err(CyberError::InvalidArgument(format!(
            "annotation {index} is longer than {MAX_ANNOTATION_CHARS} characters: shorten {:?}",
            row.text
        )));
    }
    let colour = match row.color.as_deref() {
        Some(text) => parse_hex_color(text)?,
        None => ANNOTATION_COLOUR,
    };
    Ok(Annotation {
        x: row.x,
        y: row.y,
        text: row.text.to_ascii_uppercase(),
        colour,
    })
}

/// The validated `opacity` percent, `0..=100`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a percent above `100`.
fn resolve_opacity(requested: Option<u8>) -> Result<u8, CyberError> {
    let opacity = requested.unwrap_or(DEFAULT_OPACITY);
    if opacity <= MAX_OPACITY {
        return Ok(opacity);
    }
    Err(CyberError::InvalidArgument(format!(
        "underlay opacity must be within 0..={MAX_OPACITY} percent, got {opacity}"
    )))
}

/// The wire `units` object as an unsnapped [`Units`]: the render's grid is
/// always `1`, snapping belongs to plan building, not to picture sampling.
fn to_overlay_units(requested: Option<UnitsParam>) -> Units {
    let wire = requested.unwrap_or_default();
    Units {
        scale_num: wire.scale_num,
        scale_den: wire.scale_den,
        origin_x: wire.origin_x,
        origin_y: wire.origin_y,
        grid: 1_i64,
        flip_y: wire.flip_y,
    }
}

/// The enclosing map-space rectangle of an `image` under `units`: its four
/// corners through [`Units::to_map`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a corner leaves the supported
/// range under the transform.
fn underlay_rect(units: &Units, image: &RgbaImage) -> Result<Rect, CyberError> {
    let wide = i64::from(image.width);
    let high = i64::from(image.height);
    let corners = [(0_i64, 0_i64), (wide, 0_i64), (wide, high), (0_i64, high)];
    let mut rect = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for (px, py) in corners {
        let (x, y) = units.to_map(px, py).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "underlay corner ({px}, {py}) leaves the map range under this scale and origin: \
                 reduce scale_num or move the origin"
            ))
        })?;
        rect = union_rect(rect, (x, y, x, y));
    }
    Ok(rect)
}

/// Fetch and decode `render_topdown`' underlay before any session lock:
/// [`AppState::fetch_source`], [`decode_image`], the wire units checked
/// and the map-space rectangle measured.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`decode_image`]'s [`CyberError::Format`] for bytes that are no
/// readable picture, and the [`Units::check`], [`underlay_rect`] and
/// [`resolve_opacity`] refusals for a bad transform or blend.
async fn load_underlay(state: &AppState, args: &UnderlayParams) -> Result<Underlay, CyberError> {
    let fetched = state.fetch_source(&args.source).await?;
    let image = decode_image(&fetched.bytes)?;
    let units = to_overlay_units(args.units);
    units.check()?;
    let rect = underlay_rect(&units, &image)?;
    Ok(Underlay {
        image,
        units,
        opacity: resolve_opacity(args.opacity)?,
        rect,
    })
}

/// The resolved highlight lists, every index checked against the map.
///
/// # Errors
///
/// [`checked_indices`]' [`CyberError::NotFound`] for an out-of-range index.
fn resolve_highlight(
    requested: Option<&HighlightParams>,
    map: &Map,
) -> Result<Highlights, CyberError> {
    let absent = HighlightParams::default();
    let wanted = requested.unwrap_or(&absent);
    Ok(Highlights {
        linedefs: checked_indices(
            "linedef",
            wanted.linedefs.as_deref(),
            map.linedefs.len(),
            map,
        )?,
        sectors: checked_indices("sector", wanted.sectors.as_deref(), map.sectors.len(), map)?,
        things: checked_indices("thing", wanted.things.as_deref(), map.things.len(), map)?,
        vertices: checked_indices(
            "vertex",
            wanted.vertices.as_deref(),
            map.vertices.len(),
            map,
        )?,
    })
}

/// `wanted` when every index names an element of `len`, else the refusal.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the index and the element count.
fn checked_indices(
    kind: &str,
    wanted: Option<&[usize]>,
    len: usize,
    map: &Map,
) -> Result<Vec<usize>, CyberError> {
    let Some(indices) = wanted else {
        return Ok(Vec::new());
    };
    for index in indices {
        if *index >= len {
            return Err(CyberError::NotFound(format!(
                "highlight {kind} index {index} is out of range: {} has {len} {kind}s",
                map.name
            )));
        }
    }
    Ok(indices.to_vec())
}

/// The call's resolved arguments: projection, resolution and style through
/// the render settings' own validators, bounds and highlights against the
/// map, and the overlay parameters checked and normalised.
///
/// # Errors
///
/// [`parse_projection`], [`RenderSettings::set_resolution`],
/// [`RenderSettings::set_topdown_style`], [`resolve_bounds`] and
/// [`resolve_highlight`]'s errors, plus the [`check_overlays`],
/// [`resolve_grid`], [`resolve_label_scale`] and [`resolve_annotations`]
/// refusals.
fn resolve_request(
    settings: &RenderSettings,
    args: &RenderTopdownParams,
    map: &Map,
    underlay: Option<Underlay>,
) -> Result<Request, CyberError> {
    let projection = parse_projection(args.projection.as_deref())?;
    check_overlays(args, projection)?;
    let mut probe = settings.clone();
    probe.set_resolution(
        args.width.unwrap_or(settings.width),
        args.height.unwrap_or(settings.height),
    )?;
    let style = match args.style.as_deref() {
        Some(text) => {
            probe.set_topdown_style(text)?;
            probe.topdown_style
        }
        None => settings.topdown_style,
    };
    Ok(Request {
        projection,
        width: probe.width,
        height: probe.height,
        style,
        bounds: resolve_bounds(args.bounds.as_ref(), map, underlay.as_ref().map(|u| u.rect))?,
        background: background_pixel(&settings.background),
        show_things: args.show_things.map_or(settings.show_things, Switch::from),
        show_vertices: args.show_vertices.unwrap_or(false).into(),
        fill_sectors: args.fill_sectors.unwrap_or(false).into(),
        highlight: resolve_highlight(args.highlight.as_ref(), map)?,
        underlay,
        grid: resolve_grid(args.grid)?,
        labels: resolve_labels(args.labels.as_ref()),
        annotations: resolve_annotations(args.annotations.as_deref())?,
        label_scale: resolve_label_scale(args.label_scale, probe.width)?,
    })
}

/// Number of colour channels in a pixel (`background_pixel` parses those).
const COLOUR_CHANNELS: usize = 3;

/// The `#RRGGBB` settings colour as an opaque pixel; text the settings
/// validator already accepted, so a bad byte just reads as `0`.
fn background_pixel(text: &str) -> Rgba {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let mut pixel = [0_u8, 0_u8, 0_u8, 255_u8];
    for (slot, pair) in pixel
        .iter_mut()
        .take(COLOUR_CHANNELS)
        .zip(digits.as_bytes().chunks_exact(2))
    {
        let Ok(pair_text) = core::str::from_utf8(pair) else {
            continue;
        };
        *slot = u8::from_str_radix(pair_text, 16).unwrap_or(0);
    }
    pixel
}

/// The editor-style colour of `line`: specials read blue, two-sided lines
/// mid grey, everything else light grey.
const fn editor_line_color(line: &Linedef) -> Rgba {
    if line.special != 0_i32 {
        return LINE_SPECIAL;
    }
    if line.back.is_some() {
        return LINE_TWO_SIDED;
    }
    LINE_ONE_SIDED
}

/// The grey a sector's fill gets: half its light level, rounded.
fn light_grey(light: i32) -> Rgba {
    let half_light = mul_div_round(i64::from(light.max(0_i32)), 1, 2).unwrap_or(0);
    let level = u8::try_from(half_light).unwrap_or(u8::MAX);
    [level, level, level, 255_u8]
}

/// The world corners of one isometrically drawn linedef.
#[derive(Clone, Copy)]
struct IsoPair {
    /// The start vertex's `(x, y)`.
    start: (i64, i64),
    /// The end vertex's `(x, y)`.
    end: (i64, i64),
}

/// One render's drawing state: the geometry, the definitions for thing
/// markers, the fitted view and the canvas being painted.
struct Scene<'render> {
    /// The map being drawn.
    map: &'render Map,
    /// The session definitions behind thing markers.
    defs: &'render Definitions,
    /// The fitted plane-to-pixel mapping.
    view: View,
    /// The pixels so far.
    canvas: Canvas,
    /// What the render skipped or adjusted, for the result's `notes`.
    notes: Vec<String>,
}

impl Scene<'_> {
    /// The sector the sidedef `side` opens into, `None` for an absent or
    /// dangling side.
    fn side_sector(&self, side: Option<usize>) -> Option<usize> {
        side_sector(self.map, side)
    }

    /// The floor height behind sidedef `side`, `None` when unresolvable.
    fn side_floor(&self, side: Option<usize>) -> Option<i64> {
        self.side_sector(side)
            .and_then(|index| self.map.sectors.get(index))
            .map(|sector| i64::from(sector.floor_height))
    }

    /// The ceiling height behind sidedef `side`, `None` when unresolvable.
    fn side_ceiling(&self, side: Option<usize>) -> Option<i64> {
        self.side_sector(side)
            .and_then(|index| self.map.sectors.get(index))
            .map(|sector| i64::from(sector.ceiling_height))
    }

    /// The pixel endpoints of `line`, `None` when a vertex dangles.
    fn line_pixels(&self, line: &Linedef) -> Option<(Pt, Pt)> {
        let start = self.map.vertices.get(line.v1)?;
        let end = self.map.vertices.get(line.v2)?;
        Some((
            self.view.point(i64::from(start.x), i64::from(start.y)),
            self.view.point(i64::from(end.x), i64::from(end.y)),
        ))
    }

    /// The boundary of `sector` as pixel-space edges, for the even-odd
    /// fill; dangling references drop out.
    fn sector_edges(&self, sector: usize) -> Vec<Edge> {
        geometry::sector_boundary(self.map, sector)
            .into_iter()
            .filter_map(|(index, _)| {
                self.map
                    .linedefs
                    .get(index)
                    .and_then(|line| self.line_pixels(line))
            })
            .collect()
    }

    /// The world `x`/`y` corners of `line`'s two vertices, `None` when a
    /// vertex dangles.
    fn iso_endpoints(&self, line: &Linedef) -> Option<IsoPair> {
        let start = self.map.vertices.get(line.v1)?;
        let end = self.map.vertices.get(line.v2)?;
        Some(IsoPair {
            start: (i64::from(start.x), i64::from(start.y)),
            end: (i64::from(end.x), i64::from(end.y)),
        })
    }

    /// The colour and width one linedef gets in either projection.
    fn line_mark(&self, line: &Linedef, index: usize, request: &Request) -> Option<(Rgba, u32)> {
        if request.highlight.linedefs.contains(&index) {
            return Some((HIGHLIGHT_RED, HIGHLIGHT_WIDTH));
        }
        match request.style {
            TopdownStyle::Editor => Some((editor_line_color(line), 1_u32)),
            TopdownStyle::Automap => self.automap_line_color(line).map(|rgba| (rgba, 1_u32)),
        }
    }

    /// The `Doom` automap colour of `line` with the RGB default palette,
    /// through the shared classification in [`crate::render::topdown`].
    fn automap_line_color(&self, line: &Linedef) -> Option<Rgba> {
        automap_color(self.map, line, &AutomapColors::DEFAULT)
    }
}

/// Paints the whole top-down view in order: background (already the
/// canvas ground), underlay, grid, sector fills, lines, vertices, things
/// with their highlights, then the labels and annotations on top.
fn draw_topdown(scene: &mut Scene<'_>, request: &Request) {
    if let Some(underlay) = request.underlay.as_ref() {
        paint_underlay(scene, underlay);
    }
    if let Some(spacing) = request.grid {
        paint_grid(scene, spacing);
    }
    if request.fill_sectors.is_on() {
        paint_sector_fills(scene, request);
    }
    paint_topdown_lines(scene, request);
    if request.show_vertices.is_on() {
        paint_vertex_marks(scene, request);
    }
    if request.show_things.is_on() {
        paint_thing_marks(scene, request);
    }
    if request.labels.any() {
        paint_labels(scene, request);
    }
    paint_annotations(scene, request);
}

/// Draws the `underlay`: every output pixel is inverted through the
/// fitted [`View`] to a world point, mapped back into image pixels by
/// [`Units::to_image`] and nearest-neighbour sampled; the texel blends
/// over the background at `opacity` percent of its own alpha. Pixels
/// outside the image keep what was painted there.
fn paint_underlay(scene: &mut Scene<'_>, underlay: &Underlay) {
    let percent = i64::from(underlay.opacity.min(MAX_OPACITY));
    for row in 0..i64::from(scene.canvas.height()) {
        for col in 0..i64::from(scene.canvas.width()) {
            paint_underlay_pixel(scene, underlay, percent, (col, row));
        }
    }
}

/// One [`paint_underlay`] output pixel, dropping everything the mapping
/// or the image cannot answer for.
fn paint_underlay_pixel(scene: &mut Scene<'_>, underlay: &Underlay, percent: i64, at: Pt) {
    let (x, y) = scene.view.unproject(at.0, at.1);
    let Some(texel) = underlay.units.to_image(x, y).and_then(|(px, py)| {
        underlay
            .image
            .get(u32::try_from(px).ok()?, u32::try_from(py).ok()?)
    }) else {
        return;
    };
    let [red, green, blue, alpha] = texel;
    let mixed = scale_alpha(alpha, percent);
    if mixed == 0_u8 {
        return;
    }
    scene.canvas.blend(at.0, at.1, [red, green, blue, mixed]);
}

/// `alpha` scaled to `percent` percent, rounded; `percent` is `0..=100`.
fn scale_alpha(alpha: u8, percent: i64) -> u8 {
    let lifted = i64::from(alpha)
        .saturating_mul(percent)
        .saturating_add(50_i64)
        .checked_div(100_i64)
        .unwrap_or(0_i64);
    u8::try_from(lifted).unwrap_or(0_u8)
}

/// Draws the map-unit grid: a line at every multiple of `spacing` across
/// the visible world, every [`GRID_MAJOR_EVERY`]th one in the major
/// colour. A spacing that measures under [`MIN_GRID_PIXELS`] pixels on
/// screen is skipped with a [`Scene::notes`] line instead of mud.
fn paint_grid(scene: &mut Scene<'_>, spacing: i64) {
    let pixels = scene.view.scale(spacing);
    if pixels < MIN_GRID_PIXELS {
        scene.notes.push(format!(
            "grid skipped: {spacing} map units measure {pixels} pixels at this zoom; use a \
             coarser spacing or a larger output"
        ));
        return;
    }
    let (left, top, right, bottom) = grid_frame(scene);
    for k in left.div_euclid(spacing)..=right.div_euclid(spacing) {
        let line = GridLine {
            at: k.saturating_mul(spacing),
            vertical: true,
            low: bottom,
            high: top,
        };
        paint_grid_line(scene, &line, k);
    }
    for k in bottom.div_euclid(spacing)..=top.div_euclid(spacing) {
        let line = GridLine {
            at: k.saturating_mul(spacing),
            vertical: false,
            low: left,
            high: right,
        };
        paint_grid_line(scene, &line, k);
    }
}

/// The world rectangle the whole canvas covers, as `(left, top, right,
/// bottom)`: the fitted view read at the canvas corners.
fn grid_frame(scene: &Scene<'_>) -> (i64, i64, i64, i64) {
    let (left, top) = scene.view.unproject(0, 0);
    let (right, bottom) = scene.view.unproject(
        i64::from(scene.canvas.width().saturating_sub(1)),
        i64::from(scene.canvas.height().saturating_sub(1)),
    );
    (left, top, right, bottom)
}

/// One grid line's placement: the world coordinate it runs along, the
/// span it covers on the other axis, and which axis is which.
struct GridLine {
    /// The fixed coordinate: `k * spacing` map units.
    at: i64,
    /// Whether [`at`](Self::at) measures along `x` (a vertical line) or
    /// along `y`.
    vertical: bool,
    /// The span's lower world coordinate on the other axis.
    low: i64,
    /// The span's higher world coordinate on the other axis.
    high: i64,
}

/// Draws one grid line, in the [`GRID_MAJOR`] colour when its multiple
/// index `k` is a full [`GRID_MAJOR_EVERY`]th step, [`GRID_MINOR`] else.
fn paint_grid_line(scene: &mut Scene<'_>, line: &GridLine, k: i64) {
    let colour = if k.rem_euclid(GRID_MAJOR_EVERY) == 0_i64 {
        GRID_MAJOR
    } else {
        GRID_MINOR
    };
    let (from, to) = if line.vertical {
        (
            scene.view.point(line.at, line.low),
            scene.view.point(line.at, line.high),
        )
    } else {
        (
            scene.view.point(line.low, line.at),
            scene.view.point(line.high, line.at),
        )
    };
    scene.canvas.line(from, to, colour);
}

/// Paints the whole isometric wireframe: height-aware lines and thing
/// markers. There is no hidden-line removal: every edge draws.
fn draw_isometric(scene: &mut Scene<'_>, request: &Request) {
    paint_iso_lines(scene, request);
    if request.show_things.is_on() {
        paint_iso_things(scene, request);
    }
}

/// Draws every linedef in its style colour and highlight width.
fn paint_topdown_lines(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        paint_one_line(scene, line, index, request);
    }
}

/// Draws one linedef unless its style skips it.
fn paint_one_line(scene: &mut Scene<'_>, line: &Linedef, index: usize, request: &Request) {
    let Some((start, end)) = scene.line_pixels(line) else {
        return;
    };
    let Some((rgba, width)) = scene.line_mark(line, index, request) else {
        return;
    };
    scene
        .canvas
        .thick_line(start, end, Stroke::new(width, rgba));
}

/// Fills every sector with a grey of half its light, then tints the
/// highlighted ones red.
fn paint_sector_fills(scene: &mut Scene<'_>, request: &Request) {
    for sector in 0..scene.map.sectors.len() {
        paint_one_sector(scene, sector, request);
    }
}

/// Fills (and maybe tints) one sector's polygon.
fn paint_one_sector(scene: &mut Scene<'_>, sector: usize, request: &Request) {
    let edges = scene.sector_edges(sector);
    if edges.is_empty() {
        return;
    }
    let light = scene.map.sectors.get(sector).map_or(0_i32, |row| row.light);
    scene.canvas.fill_polygon_evenodd(&edges, light_grey(light));
    if request.highlight.sectors.contains(&sector) {
        scene.canvas.blend_polygon_evenodd(&edges, HIGHLIGHT_TINT);
    }
}

/// Draws a `3x3` mark per vertex.
fn paint_vertex_marks(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.vertices.iter().enumerate() {
        paint_one_vertex(scene, mark, index, request);
    }
}

/// Draws one vertex mark, red for the highlighted ones.
fn paint_one_vertex(scene: &mut Scene<'_>, mark: &Vertex, index: usize, request: &Request) {
    let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
    let rgba = if request.highlight.vertices.contains(&index) {
        HIGHLIGHT_RED
    } else {
        VERTEX_MARK
    };
    scene.canvas.fill_rect(
        offset_pt(center, VERTEX_HALF.saturating_neg()),
        offset_pt(center, VERTEX_HALF),
        rgba,
    );
}

/// Marks every thing with its category circle and facing tick.
fn paint_thing_marks(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        paint_one_thing(scene, mark, index, request);
    }
}

/// Draws one thing: a scaled circle of its category colour plus a facing
/// tick; highlighted things get a white outline.
fn paint_one_thing(scene: &mut Scene<'_>, mark: &Thing, index: usize, request: &Request) {
    let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
    let (rgba, radius) = thing_visual(scene.defs, mark.kind);
    let pixels = scene.view.scale(i64::from(radius)).max(MIN_THING_RADIUS);
    if request.highlight.things.contains(&index) {
        scene
            .canvas
            .fill_circle(center, pixels.saturating_add(1), MARK_WHITE);
    }
    scene.canvas.fill_circle(center, pixels, rgba);
    let facing = rotate_point(
        pixels.saturating_add(TICK_PX),
        0_i64,
        Angle::from_degrees(i64::from(mark.angle)),
    );
    let tip = (
        center.0.saturating_add(facing.0),
        center.1.saturating_sub(facing.1),
    );
    scene.canvas.line(center, tip, rgba);
}

/// Draws every requested element's number, centred on its anchor, after
/// the map itself so the text always reads on top.
fn paint_labels(scene: &mut Scene<'_>, request: &Request) {
    if request.labels.linedefs.is_on() {
        paint_linedef_labels(scene, request);
    }
    if request.labels.sectors.is_on() {
        paint_sector_labels(scene, request);
    }
    if request.labels.vertices.is_on() {
        paint_vertex_labels(scene, request);
    }
    if request.labels.things.is_on() {
        paint_thing_labels(scene, request);
    }
}

/// Draws one centred overlay number in the label colour.
fn label_number(scene: &mut Scene<'_>, at: Pt, index: usize, request: &Request) {
    let style = TextStyle {
        colour: LABEL_COLOUR,
        outline: OUTLINE_COLOUR,
        scale: request.label_scale,
    };
    draw_text(&mut scene.canvas, at, &index.to_string(), style);
}

/// Draws every linedef's index at its midpoint; a dangling vertex skips.
fn paint_linedef_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        if let Some((from, to)) = scene.line_pixels(line) {
            label_number(
                scene,
                (midpoint(from.0, to.0), midpoint(from.1, to.1)),
                index,
                request,
            );
        }
    }
}

/// Draws every sector's index at a point guaranteed inside it.
fn paint_sector_labels(scene: &mut Scene<'_>, request: &Request) {
    for sector in 0..scene.map.sectors.len() {
        let edges = scene.sector_edges(sector);
        if let Some(at) = sector_label_point(&edges) {
            label_number(scene, at, sector, request);
        }
    }
}

/// Draws every vertex's index beside its mark.
fn paint_vertex_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.vertices.iter().enumerate() {
        let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
        label_number(
            scene,
            offset_pt(center, VERTEX_LABEL_OFFSET),
            index,
            request,
        );
    }
}

/// Draws every thing's index beside its marker, a scaled radius away.
fn paint_thing_labels(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        let center = scene.view.point(i64::from(mark.x), i64::from(mark.y));
        let (_, radius) = thing_visual(scene.defs, mark.kind);
        let away = scene
            .view
            .scale(i64::from(radius))
            .max(MIN_THING_RADIUS)
            .saturating_add(VERTEX_HALF);
        label_number(scene, offset_pt(center, away), index, request);
    }
}

/// A point guaranteed inside the sector polygon drawn as pixel `edges`:
/// the middle row of the bounding box is scanned with the even-odd rule,
/// the longest inside interval wins and its midpoint is the anchor; a
/// polygon the row misses entirely falls back to its first vertex. An
/// empty edge list has no anchor.
fn sector_label_point(edges: &[Edge]) -> Option<Pt> {
    let (min_y, max_y) = edge_rows(edges)?;
    let row = midpoint(min_y, max_y);
    let mut crossings: Vec<i64> = edges
        .iter()
        .filter(|edge| (edge.0.1 > row) != (edge.1.1 > row))
        .map(|edge| intersect_x(edge.0, edge.1, row))
        .collect();
    crossings.sort_unstable();
    longest_interval(&crossings)
        .map(|(lo, hi)| (midpoint(lo, hi), row))
        .or_else(|| edges.first().map(|edge| edge.0))
}

/// The `(lowest, highest)` endpoint rows over `edges`, `None` when empty.
fn edge_rows(edges: &[Edge]) -> Option<(i64, i64)> {
    let rows = edges.iter().flat_map(|edge| [edge.0.1, edge.1.1]);
    let min = rows.clone().min()?;
    Some((min, rows.max()?))
}

/// The widest `(lo, hi)` interval between consecutive pairs of the sorted
/// `crossings`, `None` when fewer than two of them exist.
fn longest_interval(crossings: &[i64]) -> Option<(i64, i64)> {
    let mut best: Option<(i64, i64)> = None;
    for pair in crossings.chunks_exact(2) {
        let Ok(&[lo, hi]) = <&[i64; 2]>::try_from(pair) else {
            continue;
        };
        if wider_than((lo, hi), best) {
            best = Some((lo, hi));
        }
    }
    best
}

/// Whether the interval `one` is wider than `best` (or `best` is unset).
const fn wider_than(one: (i64, i64), best: Option<(i64, i64)>) -> bool {
    match best {
        None => true,
        Some((b_lo, b_hi)) => one.1.saturating_sub(one.0) > b_hi.saturating_sub(b_lo),
    }
}

/// Draws every annotation's text centred on its world position.
fn paint_annotations(scene: &mut Scene<'_>, request: &Request) {
    for note in &request.annotations {
        let at = scene.view.point(note.x, note.y);
        let style = TextStyle {
            colour: note.colour,
            outline: OUTLINE_COLOUR,
            scale: request.label_scale,
        };
        draw_text(&mut scene.canvas, at, &note.text, style);
    }
}

/// Draws the isometric wireframe of every linedef.
fn paint_iso_lines(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, line) in map.linedefs.iter().enumerate() {
        paint_one_iso_line(scene, line, index, request);
    }
}

/// Draws one linedef's isometric edges: the front floor, the back floor
/// at a height change and, for one-sided lines, the ceiling and the two
/// vertical posts.
fn paint_one_iso_line(scene: &mut Scene<'_>, line: &Linedef, index: usize, request: &Request) {
    let Some(pair) = scene.iso_endpoints(line) else {
        return;
    };
    let stroke = iso_stroke(line, index, request);
    let floor = scene.side_floor(line.front).unwrap_or(0_i64);
    iso_span(scene, pair, floor, stroke);
    if let Some(back) = scene.side_floor(line.back).filter(|z| *z != floor) {
        iso_span(scene, pair, back, stroke);
    }
    if line.back.is_some() {
        return;
    }
    let ceiling = scene.side_ceiling(line.front).unwrap_or(0_i64);
    iso_span(scene, pair, ceiling, stroke);
    for corner in [pair.start, pair.end] {
        iso_post(scene, corner, (floor, ceiling), stroke);
    }
}

/// The stroke one isometric linedef gets: the highlight red and width
/// when selected, else the editor two-sided/one-sided greys.
fn iso_stroke(line: &Linedef, index: usize, request: &Request) -> Stroke {
    if request.highlight.linedefs.contains(&index) {
        return Stroke::new(HIGHLIGHT_WIDTH, HIGHLIGHT_RED);
    }
    if line.back.is_some() {
        return Stroke::thin(LINE_TWO_SIDED);
    }
    Stroke::thin(LINE_ONE_SIDED)
}

/// The isometric edge between the corners of `pair` at height `z`.
fn iso_span(scene: &mut Scene<'_>, pair: IsoPair, z: i64, stroke: Stroke) {
    let from = scene.view.iso(pair.start.0, pair.start.1, z);
    let to = scene.view.iso(pair.end.0, pair.end.1, z);
    scene.canvas.thick_line(from, to, stroke);
}

/// The vertical isometric post at one world corner, from the lower to
/// the higher of `heights`.
fn iso_post(scene: &mut Scene<'_>, corner: (i64, i64), heights: (i64, i64), stroke: Stroke) {
    let low = scene.view.iso(corner.0, corner.1, heights.0);
    let high = scene.view.iso(corner.0, corner.1, heights.1);
    scene.canvas.thick_line(low, high, stroke);
}

/// Marks each thing with a small circle at its sector's floor height.
fn paint_iso_things(scene: &mut Scene<'_>, request: &Request) {
    let map = scene.map;
    for (index, mark) in map.things.iter().enumerate() {
        paint_one_iso_thing(scene, mark, index, request);
    }
}

/// Marks one thing at the floor height of the sector that holds it.
fn paint_one_iso_thing(scene: &mut Scene<'_>, mark: &Thing, index: usize, request: &Request) {
    let (x, y) = (i64::from(mark.x), i64::from(mark.y));
    let floor = geometry::point_in_sector(scene.map, x, y)
        .and_then(|sector| scene.map.sectors.get(sector))
        .map_or(0_i64, |sector| i64::from(sector.floor_height));
    let center = scene.view.iso(x, y, floor);
    let (rgba, _) = thing_visual(scene.defs, mark.kind);
    if request.highlight.things.contains(&index) {
        scene
            .canvas
            .fill_circle(center, ISO_THING_RADIUS.saturating_add(1), MARK_WHITE);
    }
    scene.canvas.fill_circle(center, ISO_THING_RADIUS, rgba);
}

/// The finished pixels of one `render_topdown`: the canvas, the `transform`
/// to report (the isometric projection returns `None`) and the notes.
struct Painted {
    /// The pixels.
    canvas: Canvas,
    /// The fitted top-down mapping, `None` for isometric.
    view: Option<View>,
    /// What the render skipped or adjusted, for the result's `notes`.
    notes: Vec<String>,
}

/// Paints a whole `render_topdown` picture in the documented order.
fn paint_map(map: &Map, defs: &Definitions, request: &Request) -> Painted {
    let (plane, reported) = match request.projection {
        Projection::Topdown => (bounds_plane(&request.bounds), true),
        Projection::Isometric => (iso_plane(&request.bounds, height_span(map)), false),
    };
    let view = fit(&plane, request.width, request.height);
    let mut scene = Scene {
        map,
        defs,
        view,
        canvas: Canvas::new(request.width, request.height, request.background),
        notes: Vec::new(),
    };
    match request.projection {
        Projection::Topdown => draw_topdown(&mut scene, request),
        Projection::Isometric => draw_isometric(&mut scene, request),
    }
    let Scene { canvas, notes, .. } = scene;
    Painted {
        canvas,
        view: reported.then_some(view),
        notes,
    }
}

/// The session half of one `render_topdown` call: the map, its thing
/// definitions and the settings it defaults from, all read under the
/// session lock and cloned out so encoding and publishing never hold it.
struct Prepared {
    /// The map to render, from the merged view.
    map: Map,
    /// The session's thing definitions.
    defs: Definitions,
    /// The session's render settings.
    settings: RenderSettings,
}

/// [`Cyberdemon::render_topdown`]'s result: the fitted `transform` (null
/// for the isometric wireframe), the underlay as placed (null without
/// one), the notes and the published `PNG` asset.
#[derive(Serialize)]
struct TopdownReport {
    /// The rendered map's name.
    map: String,
    /// Which projection drew it.
    projection: &'static str,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// The top-down pixel-to-world mapping, `null` for isometric.
    transform: Option<TransformReport>,
    /// The underlay picture and where it stands, `null` without one.
    underlay: Option<UnderlayReport>,
    /// What the render skipped or adjusted, one line each.
    notes: Vec<String>,
    /// The published overview.
    asset: AssetRef,
}

/// The `underlay` object of a [`TopdownReport`].
#[derive(Serialize)]
struct UnderlayReport {
    /// The decoded picture's width in image pixels.
    width: u32,
    /// The decoded picture's height in image pixels.
    height: u32,
    /// The map-space rectangle the picture covers.
    map_bounds: BoundsReport,
}

/// A `(min_x, min_y, max_x, max_y)` world rectangle.
#[derive(Serialize)]
struct BoundsReport {
    /// World `x` of the west edge.
    min_x: i64,
    /// World `y` of the south edge.
    min_y: i64,
    /// World `x` of the east edge.
    max_x: i64,
    /// World `y` of the north edge.
    max_y: i64,
}

impl UnderlayReport {
    /// The report line for the placed `underlay`.
    const fn of(underlay: &Underlay) -> Self {
        let (min_x, min_y, max_x, max_y) = underlay.rect;
        Self {
            width: underlay.image.width,
            height: underlay.image.height,
            map_bounds: BoundsReport {
                min_x,
                min_y,
                max_x,
                max_y,
            },
        }
    }
}

/// The `transform` object: `px = offset_x + x * scale_num / scale_den`,
/// `py = offset_y - y * scale_num / scale_den` (floor-rounded).
#[derive(Serialize)]
struct TransformReport {
    /// Pixel span of the fitted axis.
    scale_num: u64,
    /// World span of the fitted axis (`>= 1`).
    scale_den: u64,
    /// Pixel column of world `x = 0`.
    offset_x: i64,
    /// Pixel row of world `y = 0`.
    offset_y: i64,
}

/// Reads the render's inputs under the session lock.
///
/// # Errors
///
/// [`common::resolve_map_name`] and [`common::load_map`]'s errors, and the
/// definition-collection errors of [`session_definitions`].
fn load_scene(state: &AppState, requested: Option<&str>) -> Result<Prepared, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let (sources, _notes) = custom_sources(session)?;
        let defs = session_definitions(session_game(session), &sources)?;
        Ok(Prepared {
            map,
            defs,
            settings: session.render_settings().clone(),
        })
    })
}

/// `render_topdown`' work: check the overlays, fetch and decode the
/// underlay before any lock, then resolve, paint, gamma, encode, publish.
///
/// # Errors
///
/// [`load_scene`] and [`resolve_request`]'s errors, [`load_underlay`]'s
/// fetch, decode and transform refusals, [`encode_png`]' and
/// [`AppState::publish`]' failures.
async fn topdown_report(
    state: &AppState,
    args: &RenderTopdownParams,
) -> Result<TopdownReport, CyberError> {
    check_overlays(args, parse_projection(args.projection.as_deref())?)?;
    let underlay = match args.underlay.as_ref() {
        Some(wanted) => Some(load_underlay(state, wanted).await?),
        None => None,
    };
    let prepared = load_scene(state, args.map.as_deref())?;
    let request = resolve_request(&prepared.settings, args, &prepared.map, underlay)?;
    let mut painted = paint_map(&prepared.map, &prepared.defs, &request);
    painted
        .canvas
        .apply_gamma(&gamma_table(prepared.settings.gamma));
    let png = encode_png(&painted.canvas.to_rgba_image(), None)?;
    let asset = state.publish(
        &format!("{}{OVERVIEW_SUFFIX}", prepared.map.name),
        PNG_MIME,
        png,
    )?;
    Ok(TopdownReport {
        map: prepared.map.name.to_string(),
        projection: request.projection.as_str(),
        width: request.width,
        height: request.height,
        transform: painted.view.map(View::report),
        underlay: request.underlay.as_ref().map(UnderlayReport::of),
        notes: painted.notes,
        asset,
    })
}

/// Which elements [`Cyberdemon::check_playability`]'s verification
/// overview draws as highlights: the report's problems, painted through the
/// shared top-down machinery with sector fills and sector labels on.
#[derive(Debug, Default)]
pub(in crate::tools) struct OverviewMarks {
    /// Sector indices to tint red (the unreachable ones).
    pub(in crate::tools) sectors: Vec<usize>,
    /// Linedef indices to draw red and thick (the blocker lines).
    pub(in crate::tools) lines: Vec<usize>,
    /// Thing indices to ring in white (the stuck things).
    pub(in crate::tools) things: Vec<usize>,
}

/// The `check_playability` overview: the top-down drawing internals behind
/// [`Cyberdemon::render_topdown`] run over `map` with `marks` highlighted,
/// thing markers and sector fills on and sector labels on, published as a
/// `PNG` asset.
///
/// # Errors
///
/// [`topdown_report`]'s errors: the same map-resolution refusals the
/// standalone render gives, plus its encode and publish failures.
pub(in crate::tools) async fn playability_overview(
    state: &AppState,
    map: Option<&str>,
    marks: OverviewMarks,
) -> Result<AssetRef, CyberError> {
    let args = RenderTopdownParams {
        map: map.map(str::to_owned),
        width: None,
        height: None,
        projection: None,
        bounds: None,
        show_things: Some(true),
        show_vertices: None,
        fill_sectors: Some(true),
        style: None,
        highlight: Some(HighlightParams {
            linedefs: Some(marks.lines),
            sectors: Some(marks.sectors),
            things: Some(marks.things),
            vertices: None,
        }),
        underlay: None,
        grid: None,
        labels: Some(LabelsParams {
            sectors: Some(true),
            ..Default::default()
        }),
        annotations: None,
        label_scale: None,
    };
    Ok(topdown_report(state, &args).await?.asset)
}

/// Arguments for `render_minimap`' `center` and `player` parameters: one
/// world point.
#[derive(Debug, Deserialize, JsonSchema)]
struct PointParams {
    /// World `x`.
    x: i64,
    /// World `y`.
    y: i64,
}

/// Arguments for `render_minimap`' `player` parameter: the automap player
/// marker's position and facing.
#[derive(Debug, Deserialize, JsonSchema)]
struct PlayerParams {
    /// World `x` of the marker.
    x: i64,
    /// World `y` of the marker.
    y: i64,
    /// Facing in degrees, counter-clockwise from east.
    angle: i64,
}

/// Arguments for [`Cyberdemon::render_minimap`]. Every field is optional;
/// the view defaults to the whole map and the colours to the vanilla
/// automap palette entries.
#[derive(Debug, Deserialize, JsonSchema)]
struct RenderMinimapParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    map: Option<String>,
    /// Square output edge in pixels, `32..=1024` (default `256`).
    #[serde(default)]
    size: Option<u32>,
    /// World point the square window centres on (default: the map centre).
    #[serde(default)]
    center: Option<PointParams>,
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
struct Minimap {
    /// Square output edge in pixels.
    size: u32,
    /// The square world window to frame.
    window: Window,
    /// The automap colours to draw with.
    colors: AutomapColors,
    /// Whether things get category-coloured dots.
    show_things: bool,
    /// The player arrow to draw, when given.
    player: Option<PlayerMark>,
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
struct MinimapReport {
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
fn resolve_size(requested: Option<u32>) -> Result<u32, CyberError> {
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
fn resolve_radius(requested: Option<u32>, map: &Map) -> Result<i64, CyberError> {
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

/// The midpoint of two world coordinates, rounded down.
const fn midpoint(low: i64, high: i64) -> i64 {
    low.saturating_add(high).div_euclid(2)
}

/// The window centre: the `requested` point, else the midpoint of the
/// map's bounds.
///
/// # Errors
///
/// [`minimap_needs_window`] for an empty map without a given centre.
fn resolve_centre(requested: Option<&PointParams>, map: &Map) -> Result<(i64, i64), CyberError> {
    if let Some(point) = requested {
        return Ok((point.x, point.y));
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
fn resolve_minimap(
    args: &RenderMinimapParams,
    map: &Map,
    palette: Option<&Palette>,
) -> Result<Minimap, CyberError> {
    let size = resolve_size(args.size)?;
    let radius = resolve_radius(args.radius, map)?;
    let (centre_x, centre_y) = resolve_centre(args.center.as_ref(), map)?;
    let player = args.player.as_ref().map(|mark| PlayerMark {
        x: mark.x,
        y: mark.y,
        angle: mark.angle,
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
fn paint_minimap(map: &Map, defs: &Definitions, request: &Minimap) -> Canvas {
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
/// definition-collection errors of [`session_definitions`].
fn load_minimap(state: &AppState, requested: Option<&str>) -> Result<PreparedMinimap, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let (sources, _notes) = custom_sources(session)?;
        let defs = session_definitions(session_game(session), &sources)?;
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
fn minimap_report(
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

/// Arguments for [`Cyberdemon::render_view`]. `x` and `y` are required;
/// every other field is optional, defaulting to the camera floor, level
/// east, or the session's render settings.
#[derive(Debug, Deserialize, JsonSchema)]
struct RenderViewParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    map: Option<String>,
    /// Camera `x` in map units.
    x: i32,
    /// Camera `y` in map units.
    y: i32,
    /// Eye height in map units (default: the camera sector's floor plus
    /// `Doom`'s 41-unit view height).
    #[serde(default)]
    z: Option<i32>,
    /// Facing in degrees, counter-clockwise from east, 0 = east, 90 =
    /// north (default `0`).
    #[serde(default)]
    angle: Option<i32>,
    /// Pitch in degrees, positive looking up, `-60..=60` (default `0`).
    #[serde(default)]
    pitch: Option<i32>,
    /// Horizontal field of view in degrees, `10..=170` (default: the
    /// render setting).
    #[serde(default)]
    fov: Option<u32>,
    /// Output width in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    width: Option<u32>,
    /// Output height in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    height: Option<u32>,
    /// Lighting model for this call only: `"doom"`, `"flat"` or
    /// `"fullbright"` (default: the render setting).
    #[serde(default)]
    lighting: Option<String>,
}

/// The render settings one `render_view` call draws with: the session's,
/// with the `lighting`, `fov` and resolution overrides validated on top.
///
/// # Errors
///
/// [`check_pitch`] and [`RenderSettings`]' setter errors for a refused
/// override; the session settings are never touched.
fn view_settings(session: &Session, args: &RenderViewParams) -> Result<RenderSettings, CyberError> {
    check_pitch(args.pitch.unwrap_or(0_i32))?;
    let mut settings = session.render_settings().clone();
    if let Some(text) = args.lighting.as_deref() {
        settings.set_lighting(text)?;
    }
    if let Some(value) = args.fov {
        settings.set_fov(value)?;
    }
    settings.set_resolution(
        args.width.unwrap_or(settings.width),
        args.height.unwrap_or(settings.height),
    )?;
    Ok(settings)
}

/// The raw 16.16 depth a sprite may not stand nearer to the camera than:
/// nearer than four map units, and its projection would tower over (or
/// miss) the frame absurdly, so the thing is skipped.
const SPRITE_MIN_TZ16: i128 = 4_i128.saturating_mul(ONE_16);

/// A thing's placement, in the units the sprite pass reads.
#[derive(Clone, Copy, Debug)]
struct SpriteSpot {
    /// Thing `x`, in map units.
    x: i64,
    /// Thing `y`, in map units.
    y: i64,
    /// Spawn height offset, in map units.
    z: i32,
    /// Facing in degrees.
    angle: i32,
}

impl SpriteSpot {
    /// The spot `mark` stands on.
    fn of(mark: &Thing) -> Self {
        Self {
            x: i64::from(mark.x),
            y: i64::from(mark.y),
            z: mark.z,
            angle: mark.angle,
        }
    }
}

/// The session half of one `render_view` sprite pass: the definitions
/// that dress the things, the camera basis they project through, the
/// per-render patch cache and everything the walk has produced. One of
/// these lives exactly one render, which is what keeps the decode-each-
/// lump-once cache local to it.
struct SpriteWiring<'defs> {
    /// The session's thing definitions.
    defs: &'defs Definitions,
    /// Camera `x`, in map units.
    cam_x: i64,
    /// Camera `y`, in map units.
    cam_y: i64,
    /// The camera forward `x` (its angle's cosine), raw 16.16.
    forward_x: i128,
    /// The camera forward `y` (its angle's sine), raw 16.16.
    forward_y: i128,
    /// Sprite lump patches decoded this render, by lump name.
    cache: HashMap<LumpName, Patch>,
    /// Sprite names a `problems` line has already reported.
    reported: HashSet<String>,
    /// Problem lines for sprite names that would not load.
    problems: Vec<String>,
    /// The prepared items, in thing order.
    items: Vec<SpriteItem>,
}

impl<'defs> SpriteWiring<'defs> {
    /// The wiring for `camera`: `defs` dress the map's things, and the
    /// camera's position and facing project and rotate them.
    fn new(defs: &'defs Definitions, camera: &ViewCamera) -> Self {
        let facing = Angle::from_degrees(i64::from(camera.angle));
        Self {
            defs,
            cam_x: i64::from(camera.x),
            cam_y: i64::from(camera.y),
            forward_x: i128::from(facing.cos().0),
            forward_y: i128::from(facing.sin().0),
            cache: HashMap::new(),
            reported: HashSet::new(),
            problems: Vec::new(),
            items: Vec::new(),
        }
    }

    /// The definition that sprites `mark`; a thing the definitions do
    /// not know, or whose def carries no sprite, reads as `None`.
    fn definition(&self, mark: &Thing) -> Option<&'defs ThingDef> {
        let defs = self.defs;
        u32::try_from(mark.kind)
            .ok()
            .and_then(|kind| defs.thing(kind))
            .filter(|def| !def.sprite.is_empty())
    }

    /// `spot`'s camera-relative offsets: `dot(rel, right)` and
    /// `dot(rel, forward)` in raw 16.16 map units, forward `(cos, sin)`
    /// and right `(sin, -cos)`, the forward one positive in front of the
    /// camera.
    fn project(&self, spot: &SpriteSpot) -> (i128, i128) {
        let rel_x = i128::from(spot.x.saturating_sub(self.cam_x)).saturating_mul(ONE_16);
        let rel_y = i128::from(spot.y.saturating_sub(self.cam_y)).saturating_mul(ONE_16);
        let right16 = rel_x
            .saturating_mul(self.forward_y)
            .saturating_sub(rel_y.saturating_mul(self.forward_x))
            .checked_shr(FRACBITS)
            .unwrap_or(0_i128);
        let depth16 = rel_x
            .saturating_mul(self.forward_x)
            .saturating_add(rel_y.saturating_mul(self.forward_y))
            .checked_shr(FRACBITS)
            .unwrap_or(0_i128);
        (right16, depth16)
    }

    /// The sprite patch at the spot's bucketed rotation, decoded once per
    /// lump for the whole render; a sprite name that will not load
    /// reports itself once and dresses nothing.
    fn patch(&mut self, session: &Session, def: &ThingDef, spot: &SpriteSpot) -> Option<Patch> {
        let rotation = rotation_for((self.cam_x, self.cam_y), (spot.x, spot.y), spot.angle);
        match load_sprite_cached(session, def, rotation, &mut self.cache) {
            Ok(sprite) => Some(sprite.patch),
            Err(error) => {
                let missing = matches!(error, CyberError::NotFound(_));
                self.report(&def.sprite, missing);
                None
            }
        }
    }

    /// One problem line per sprite name: `missing` words a lump the
    /// session serves no picture for; the alternative words a picture
    /// that is no readable image.
    fn report(&mut self, name: &str, missing: bool) {
        if !self.reported.insert(name.to_owned()) {
            return;
        }
        let word = if missing { "missing" } else { "unreadable" };
        self.problems.push(format!("{word} sprite {name}"));
    }

    /// Adds `mark` to the render when the definitions dress it, it
    /// stands at least [`SPRITE_MIN_TZ16`] ahead of the camera inside a
    /// sector, and its sprite loads; the sector's light shades the whole
    /// sprite, standing things anchor on the sector's floor plus the
    /// spawn `z` and hanging ones under the ceiling by the patch height.
    fn add(&mut self, session: &Session, map: &Map, mark: &Thing) {
        let Some(def) = self.definition(mark) else {
            return;
        };
        let spot = SpriteSpot::of(mark);
        let (right16, depth16) = self.project(&spot);
        if depth16 < SPRITE_MIN_TZ16 {
            return;
        }
        let Some(sector) = geometry::point_in_sector(map, spot.x, spot.y) else {
            return;
        };
        let Some(row) = map.sectors.get(sector) else {
            return;
        };
        let Some(patch) = self.patch(session, def, &spot) else {
            return;
        };
        let base = if def.hanging {
            i64::from(row.ceiling_height).saturating_sub(i64::from(patch.image.height))
        } else {
            i64::from(row.floor_height).saturating_add(i64::from(spot.z))
        };
        let left_offset16 = i128::from(patch.left_offset).saturating_mul(ONE_16);
        let top_offset16 = i128::from(patch.top_offset).saturating_mul(ONE_16);
        self.items.push(SpriteItem {
            image: patch.image,
            left_offset16,
            top_offset16,
            x: spot.x,
            y: spot.y,
            base,
            light: row.light,
            tx16: right16,
            tz16: depth16,
        });
    }
}

/// The session half of one `render_view` sprite pass: the map's things
/// dressed as sprite items for the transparent pass, and the problem
/// lines naming the sprite names the session could not dress with.
fn view_sprites(
    session: &Session,
    scene: &crate::render::Scene,
    defs: &Definitions,
    camera: &ViewCamera,
) -> (Vec<SpriteItem>, Vec<String>) {
    let mut wiring = SpriteWiring::new(defs, camera);
    for mark in &scene.map.things {
        wiring.add(session, &scene.map, mark);
    }
    (wiring.items, wiring.problems)
}

/// The session half of one `render_view` call: the scene, the placed
/// camera, the sprite items and their problem lines, all read under the
/// session lock.
struct PreparedView {
    /// The prepared scene (its settings carry the resolved overrides).
    scene: crate::render::Scene,
    /// The sector index holding the camera.
    sector: usize,
    /// The effective camera: resolved `z`, defaulted `angle` and
    /// `pitch`, the settings' `fov`.
    camera: ViewCamera,
    /// The sprite items for the transparent pass.
    sprites: Vec<SpriteItem>,
    /// Problem lines for sprite names the session could not load.
    sprite_problems: Vec<String>,
}

/// Reads the view's inputs under the session lock: the map, the settings
/// with overrides, the scene, the camera placed in a sector, and the
/// things dressed as sprites while the settings ask for them.
///
/// # Errors
///
/// [`common::resolve_map_name`]'s errors, [`view_settings`]' refusals,
/// [`build_scene`]'s [`CyberError::NotFound`] for a missing `PLAYPAL`,
/// and [`CyberError::InvalidArgument`] when the camera lies outside every
/// sector of the map.
fn load_view(state: &AppState, args: &RenderViewParams) -> Result<PreparedView, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, args.map.as_deref())?;
        let settings = view_settings(session, args)?;
        let scene = build_scene(session, name, &settings)?;
        let sector = camera_sector(&scene, i64::from(args.x), i64::from(args.y)).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "camera is outside the map: ({}, {}) lies in no sector of {}; move x/y inside a \
                 sector (render_topdown shows the layout)",
                args.x, args.y, scene.map.name
            ))
        })?;
        let z = args.z.unwrap_or_else(|| default_eye_z(&scene, sector));
        let camera = ViewCamera {
            x: args.x,
            y: args.y,
            z,
            angle: args.angle.unwrap_or(0_i32),
            pitch: args.pitch.unwrap_or(0_i32),
            fov: settings.fov,
        };
        let (sprites, sprite_problems) = if settings.show_sprites.is_on() {
            let (sources, _notes) = custom_sources(session)?;
            let defs = session_definitions(session_game(session), &sources)?;
            view_sprites(session, &scene, &defs, &camera)
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(PreparedView {
            scene,
            sector,
            camera,
            sprites,
            sprite_problems,
        })
    })
}

/// [`Cyberdemon::render_view`]'s result: the effective camera, the frame
/// size, the transparent pass's counts, the scene's and sprite pass's
/// problems and the published `PNG` asset.
#[derive(Serialize)]
struct ViewReport {
    /// The rendered map's name.
    map: String,
    /// The camera the frame was cast from.
    camera: CameraReport,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// Sprites the transparent pass painted at least one pixel of; `0`
    /// when it drew none, which is also what a turned-off `show_sprites`
    /// setting reports.
    sprites_drawn: u32,
    /// Screen columns the transparent pass painted at least one masked
    /// pixel in; a column carrying several masked items counts once.
    /// `0` when it drew nothing.
    masked_columns: usize,
    /// What the scene and the sprite pass could not load, one line per
    /// item.
    problems: Vec<String>,
    /// The published view.
    asset: AssetRef,
}

/// The `camera` object of a [`ViewReport`].
#[derive(Serialize)]
struct CameraReport {
    /// Camera `x` in map units.
    x: i32,
    /// Camera `y` in map units.
    y: i32,
    /// Eye height in map units.
    z: i32,
    /// Facing in degrees, counter-clockwise from east.
    angle: i32,
    /// Pitch in degrees, positive looking up.
    pitch: i32,
    /// Horizontal field of view in degrees.
    fov: u32,
    /// The index of the sector holding the camera.
    sector: usize,
}

/// `render_view`'s work: resolve, dress the things, cast, gamma, encode,
/// publish.
///
/// # Errors
///
/// [`load_view`]'s errors, [`encode_png`]' and [`AppState::publish`]'
/// failures.
fn view_report(state: &AppState, args: &RenderViewParams) -> Result<ViewReport, CyberError> {
    let prepared = load_view(state, args)?;
    let settings = prepared.scene.settings.clone();
    let camera = prepared.camera;
    let mut rendered = render_frame(
        &prepared.scene,
        &camera,
        ViewFrame {
            width: settings.width,
            height: settings.height,
            sprites: prepared.sprites,
        },
    );
    let masked_columns = rendered.masked_columns;
    let sprites_drawn = u32::try_from(rendered.sprites_drawn).unwrap_or(u32::MAX);
    rendered.canvas.apply_gamma(&gamma_table(settings.gamma));
    let png = encode_png(&rendered.canvas.to_rgba_image(), None)?;
    let asset = state.publish(
        &format!("{}{VIEW_SUFFIX}", prepared.scene.map.name),
        PNG_MIME,
        png,
    )?;
    let mut problems = prepared.scene.problems;
    problems.extend(prepared.sprite_problems);
    Ok(ViewReport {
        map: prepared.scene.map.name.to_string(),
        camera: CameraReport {
            x: camera.x,
            y: camera.y,
            z: camera.z,
            angle: camera.angle,
            pitch: camera.pitch,
            fov: camera.fov,
            sector: prepared.sector,
        },
        width: settings.width,
        height: settings.height,
        sprites_drawn,
        masked_columns,
        problems,
        asset,
    })
}

/// Pixels kept clear around the fitted view.
const MARGIN_PX: i64 = 16;
/// World units of slack added around the default map bounds.
const BOUNDS_PAD: i64 = 32;
/// Stroke width of highlighted linedefs.
const HIGHLIGHT_WIDTH: u32 = 3;
/// Half edge of a vertex mark, in pixels (a `3x3` square).
const VERTEX_HALF: i64 = 1;
/// Smallest thing-marker radius, in pixels.
const MIN_THING_RADIUS: i64 = 2;
/// Extra length of a thing marker's facing tick, beyond its radius.
const TICK_PX: i64 = 4;
/// Radius of an isometric thing marker, in pixels.
const ISO_THING_RADIUS: i64 = 2;
/// Name suffix of the published overview files.
const OVERVIEW_SUFFIX: &str = "_topdown.png";
/// Media type of the published overview.
const PNG_MIME: &str = "image/png";

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
const MINIMAP_FIT_PAD: i64 = 1;
/// Name suffix of the published minimap files.
const MINIMAP_SUFFIX: &str = "_minimap.png";
/// Name suffix of the published 3D view files.
const VIEW_SUFFIX: &str = "_view.png";
/// The minimap background: black, as the in-game automap draws it.
const MINIMAP_BACKGROUND: Rgba = [0, 0, 0, 255];

/// Editor style: a one-sided wall.
const LINE_ONE_SIDED: Rgba = [192, 192, 192, 255];
/// Editor style: a two-sided line.
const LINE_TWO_SIDED: Rgba = [112, 112, 112, 255];
/// Editor style: a line with a special.
const LINE_SPECIAL: Rgba = [106, 140, 255, 255];
/// Highlighted linedefs and vertices.
const HIGHLIGHT_RED: Rgba = [255, 64, 64, 255];
/// The translucent red tint highlighted sectors get.
const HIGHLIGHT_TINT: Rgba = [255, 64, 64, 128];
/// The white outline highlighted things get.
const MARK_WHITE: Rgba = [255, 255, 255, 255];
/// The `3x3` vertex marks.
const VERTEX_MARK: Rgba = [255, 208, 0, 255];

/// Smallest `grid` spacing, in map units.
const MIN_GRID: i64 = 8;
/// Largest `grid` spacing, in map units.
const MAX_GRID: i64 = 1_024;
/// A `grid` spacing measuring under this many pixels on screen is skipped.
const MIN_GRID_PIXELS: i64 = 4;
/// Every `GRID_MAJOR_EVERY`th grid line draws in the major colour.
const GRID_MAJOR_EVERY: i64 = 8;
/// The grid's minor lines.
const GRID_MINOR: Rgba = [42, 42, 42, 255];
/// The grid's every-[`GRID_MAJOR_EVERY`]th lines.
const GRID_MAJOR: Rgba = [60, 60, 60, 255];
/// The element-index label text.
const LABEL_COLOUR: Rgba = [255, 255, 128, 255];
/// The outline under every overlay glyph.
const OUTLINE_COLOUR: Rgba = [0, 0, 0, 255];
/// The `annotations` default colour.
const ANNOTATION_COLOUR: Rgba = [255, 255, 255, 255];
/// The `underlay.opacity` default, percent.
const DEFAULT_OPACITY: u8 = 50;
/// The largest `underlay.opacity`, percent.
const MAX_OPACITY: u8 = 100;
/// The most `annotations` rows one call may carry.
const MAX_ANNOTATIONS: usize = 64;
/// The most characters one `annotations` row may carry.
const MAX_ANNOTATION_CHARS: usize = 32;
/// The smallest `label_scale`.
const MIN_LABEL_SCALE: u32 = 1;
/// The largest `label_scale`.
const MAX_LABEL_SCALE: u32 = 4;
/// The `label_scale` default on a narrow output.
const DEFAULT_LABEL_SCALE: u32 = 1;
/// The `label_scale` default once the output is wide.
const WIDE_LABEL_SCALE: u32 = 2;
/// The output width at or above which labels default to [`WIDE_LABEL_SCALE`].
const WIDE_LABEL_THRESHOLD: u32 = 1_024;
/// How far a vertex label stands off its mark, in pixels.
const VERTEX_LABEL_OFFSET: i64 = 4;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = render_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Sets the resolution the render tools default to.
    #[tool(
        description = "Sets the pixel size render tools default to: width and height, each 16..=4096, with width*height at most 16777216 (exactly a 4096x4096 square). Returns the full render settings after the change; a refused size reports invalid_argument and nothing changes."
    )]
    fn set_render_resolution(
        &self,
        Parameters(args): Parameters<SetRenderResolutionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match resolution_report(self.state(), args.width, args.height) {
            Ok(settings) => ok(&settings),
            Err(error) => fail(&error),
        }
    }

    /// Changes the session's render options, field by field.
    #[tool(
        description = "Changes the session's render options, applying only the fields you pass and keeping the rest; at least one field is required. lighting \"doom\" (sector light plus the COLORMAP distance fade) | \"flat\" (sector light only) | \"fullbright\"; show_things (thing markers in top-down renders); show_sprites (sprites in the 3D view); sky (draw the sky texture, off means the background colour); fov (horizontal field of view, 10..=170 degrees); gamma (Doom gamma level, 0..=4); topdown_style \"editor\" | \"automap\"; background (\"#RRGGBB\"). All or nothing: a refused value reports invalid_argument listing the valid values and leaves every setting unchanged. Returns the full render settings."
    )]
    fn set_render_options(
        &self,
        Parameters(args): Parameters<SetRenderOptionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match options_report(self.state(), &args) {
            Ok(settings) => ok(&settings),
            Err(error) => fail(&error),
        }
    }

    /// Reports the session's render settings.
    #[tool(
        description = "Returns the session's render settings as one object: width, height, lighting, show_things, show_sprites, sky, fov, gamma, topdown_style, background. These are the defaults render_view, render_topdown and render_minimap use where their own arguments leave a value out. They belong to the connection: new_wad, open_wad, merge_wad and undo never change them, and they are not saved into any WAD."
    )]
    fn get_render_settings(&self) -> Result<CallToolResult, ErrorData> {
        match self
            .state()
            .with_session(|session| Ok(session.render_settings().clone()))
        {
            Ok(settings) => ok(&settings),
            Err(error) => fail(&error),
        }
    }

    /// Renders a map overview as a `PNG` asset.
    #[tool(
        description = "Renders a map overview and publishes it as a PNG asset; returns {map, projection, width, height, transform, underlay, notes, asset}. projection \"topdown\" (default) draws the flat north-up line map; projection \"isometric\" draws a 2:1 wireframe (screen = (x - y, (x + y) / 2 - z), floor edges per side, ceiling and verticals on one-sided lines, no hidden-line removal) and reports transform null. width/height default to the render settings. bounds {min_x, min_y, max_x, max_y} frames a world rectangle (default: the map bounds unioned with the underlay's map rectangle, padded by 32 units; an empty map without bounds and without an underlay fails invalid_argument, as do min >= max). style \"editor\" colours one-sided #C0C0C0, two-sided #707070, special-bearing #6A8CFF; style \"automap\" draws the in-game map: one-sided (and secret) #FC0000, floor-height change #BC7848, ceiling-height change #FCFC00, flat two-sided #808080, dontdraw lines skipped. fill_sectors first fills each sector with a grey of half its light. show_vertices adds a 3x3 #FFD000 mark per vertex; show_things (default from the settings) marks each thing with a circle of its definition radius (unknown types: 16) in its category colour and a facing tick. highlight {linedefs, sectors, things, vertices} draws those elements as highlights (red 3px lines, a red sector tint, white thing outlines); an index past the end of its table fails not_found. For topdown, transform carries {scale_num, scale_den, offset_x, offset_y} with px = offset_x + x*scale_num/scale_den and py = offset_y - y*scale_num/scale_den (both floor-rounded): convert a pixel back with x = (px - offset_x)*scale_den/scale_num and y = (offset_y - py)*scale_den/scale_num. Overlays are topdown-only: underlay {source, units, opacity} draws a reference picture (PNG/JPEG/GIF/BMP by path, file:// or http(s):// URL, or an asset URL) under the map; units {scale_num, scale_den, origin_x, origin_y, flip_y} means exactly what build_map_from_plan's units means, so image pixel (px, py) sits at x = origin_x + px*scale_num/scale_den and y = origin_y - py*scale_num/scale_den (plus when flip_y is false) and the sketch lines up with the map built from it; opacity (default 50, 0..=100 percent) blends the picture over the background scaled by each texel's alpha. grid (8..=1024 map units) draws a coordinate grid under the map, minor lines #2A2A2A and every 8th #3C3C3C; a spacing under 4 pixels on screen is skipped and reported in notes. labels {sectors, linedefs, things, vertices} draws element indices in #FFFF80 with a black outline: sectors at a point guaranteed inside them, linedefs at their midpoint, things and vertices beside their marks. annotations [{x, y, text, color}] draws free text (uppercased, characters outside the 5x7 font as ?, at most 64 rows of 32 characters, default colour #FFFFFF) centred on world coordinates. label_scale (1..=4) magnifies all overlay text (default 2 when the output is at least 1024 pixels wide, else 1). Any overlay parameter with projection \"isometric\" fails invalid_argument; an underlay source that is no readable picture fails format_error. Results report underlay {width, height, map_bounds} (null without one) and notes []. The gamma render setting is applied to the output."
    )]
    async fn render_topdown(
        &self,
        Parameters(args): Parameters<RenderTopdownParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match topdown_report(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Renders a compact automap-style view as a `PNG` asset.
    #[tool(
        description = "Renders a compact automap-style view for quick orientation and publishes it as a PNG asset; returns {map, size, center, radius, asset}. Always the automap look on a black background: one-sided (and secret) lines red, floor-height changes brown, ceiling-height changes yellow, other two-sided lines grey, dontdraw lines hidden. size is the square output edge, 32..=1024 (default 256). center {x, y} is the world point to frame (default: the map centre) and radius the map units from the centre to a window edge, 1..=65536 (default: fit the whole map, padded by one unit); lines are clipped to the square, and an empty map without center and radius fails invalid_argument. palette_colors (default true) takes the line colours from PLAYPAL palette 0 at the vanilla automap indices 176/64/231/96 and the player marker at 209 when the session has a PLAYPAL, else the RGB defaults #FC0000/#BC7848/#FCFC00/#808080/#FFFFFF. player {x, y, angle} draws the white automap arrow, 16 pixels long, facing angle degrees counter-clockwise from east. show_things (default false) marks each thing with a 2x2 pixel dot in its category colour. The gamma render setting is applied to the output; map defaults like every other map tool."
    )]
    fn render_minimap(
        &self,
        Parameters(args): Parameters<RenderMinimapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match minimap_report(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Renders a first-person 3D view as a `PNG` asset.
    #[tool(
        description = "Renders a first-person 3D view from a camera inside the map and publishes it as a PNG asset; returns {map, camera, width, height, sprites_drawn, masked_columns, problems, asset}. The camera stands at x/y map units with eye height z (default: the camera sector's floor + Doom's 41), faces angle degrees counter-clockwise from east (0 east, 90 north), looks pitch degrees up from level (positive looks up, -60..=60), through fov degrees of horizontal field of view (10..=170) onto width x height pixels. fov, width and height default to the render settings; lighting (\"doom\" | \"flat\" | \"fullbright\") overrides the settings' lighting for this call only. Walls draw the linedef's upper/middle/lower textures with their offsets and Doom's pegging flags, floors and ceilings draw their flats textured, aligned to the world grid like Doom (an F_SKY1 plane draws the sky texture instead, scrolled by the view angle and always full bright; with the sky render option off, sky surfaces show the background colour), all shaded through the COLORMAP by sector light, fake contrast and view depth. Two-sided lines whose faced side carries a middle texture show it as a see-through masked middle (transparent texels let the geometry behind show); masked_columns counts the screen columns that pass painted. While the show_sprites render setting is on, every thing whose definition carries a sprite draws its patch as a distance-scaled billboard in the same transparent pass: Doom's eight-rotation bucketing picks the frame from the viewing angle and the thing's facing (mirror-pair lumps flip automatically), it stands on its sector's floor plus its spawn z (hanging things under the ceiling instead), and it shows only where nearer than the opaque geometry; things outside every sector or closer than 4 units to the camera draw nothing, and a sprite name whose lumps the session lacks or cannot decode adds one `missing sprite NAME` or `unreadable sprite NAME` line to problems without failing the call; sprites_drawn counts the sprites the pass painted at least one pixel of. The result's camera object echoes the effective camera and names the sector holding it; problems lists pictures the scene could not load. Errors: invalid_argument when x/y lies in no sector (the refusal says so), for pitch/fov outside their range, an edge outside 16..=4096 or over the pixel cap, and an unknown lighting name; not_found when no map resolves or the session has no PLAYPAL."
    )]
    fn render_view(
        &self,
        Parameters(args): Parameters<RenderViewParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match view_report(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SetRenderOptionsParams, apply_options, options_report, resolution_report};
    use crate::render::{Lighting, RenderSettings, TopdownStyle};
    use crate::state::AppState;

    /// Options with the listed fields set and every other left out.
    fn options_with(
        lighting: Option<&str>,
        fov: Option<u32>,
        background: Option<&str>,
    ) -> SetRenderOptionsParams {
        SetRenderOptionsParams {
            lighting: lighting.map(str::to_owned),
            fov,
            background: background.map(str::to_owned),
            ..SetRenderOptionsParams::default()
        }
    }

    #[test]
    fn empty_options_are_refused_naming_the_options() {
        let state = AppState::default();
        let error = options_report(&state, &SetRenderOptionsParams::default())
            .expect_err("a call with no option does nothing");
        assert_eq!(error.code(), "invalid_argument", "empty options code");
        let message = error.to_string();
        for name in ["lighting", "show_things", "topdown_style", "background"] {
            assert!(
                message.contains(name),
                "the message lists {name}: {message}"
            );
        }
    }

    #[test]
    fn options_report_lands_on_the_session() {
        let state = AppState::default();
        let args = options_with(Some("fullbright"), Some(120), None);
        let report = options_report(&state, &args).expect("two valid options");
        assert_eq!(report.lighting, Lighting::Fullbright, "lighting changed");
        assert_eq!(report.fov, 120, "fov changed");
        let stored = state
            .with_session(|session| Ok(session.render_settings().clone()))
            .expect("a fresh lock is never poisoned");
        assert_eq!(stored, report, "the session holds what it reported");
    }

    #[test]
    fn apply_options_keeps_every_field_it_was_not_given() {
        let mut settings = RenderSettings::default();
        let before = settings.clone();
        let args = options_with(Some("FLAT"), None, Some("#102030"));
        apply_options(&mut settings, &args).expect("a valid lighting and colour");
        assert_eq!(settings.lighting, Lighting::Flat, "lighting applied");
        assert_eq!(settings.background, "#102030", "background applied");
        assert_eq!(settings.width, before.width, "resolution kept");
        assert_eq!(settings.height, before.height, "resolution kept");
        assert_eq!(settings.show_things, before.show_things, "markers kept");
        assert_eq!(settings.show_sprites, before.show_sprites, "sprites kept");
        assert_eq!(settings.sky, before.sky, "sky kept");
        assert_eq!(settings.fov, before.fov, "fov kept");
        assert_eq!(settings.gamma, before.gamma, "gamma kept");
        assert_eq!(settings.topdown_style, before.topdown_style, "style kept");
    }

    #[test]
    fn apply_options_is_all_or_nothing() {
        let mut settings = RenderSettings::default();
        let before = settings.clone();
        let args = options_with(Some("fullbright"), Some(200), Some("#ffffff"));
        let error = apply_options(&mut settings, &args)
            .expect_err("200 degrees is outside the field of view");
        assert_eq!(error.code(), "invalid_argument", "bad fov code");
        assert!(
            error.to_string().contains("fov"),
            "the message names the option that failed: {error}"
        );
        assert_eq!(
            settings, before,
            "the valid lighting and background did not land with an invalid fov"
        );
    }

    #[test]
    fn apply_options_turns_the_switches_off() {
        let mut settings = RenderSettings::default();
        let args = SetRenderOptionsParams {
            show_things: Some(false),
            show_sprites: Some(false),
            sky: Some(false),
            gamma: Some(4),
            topdown_style: Some("Automap".to_owned()),
            ..SetRenderOptionsParams::default()
        };
        apply_options(&mut settings, &args).expect("three switches and a level");
        assert!(!settings.show_things.is_on(), "thing markers off");
        assert!(!settings.show_sprites.is_on(), "sprites off");
        assert!(!settings.sky.is_on(), "sky off");
        assert_eq!(settings.gamma, 4, "the top gamma level");
        assert_eq!(
            settings.topdown_style,
            TopdownStyle::Automap,
            "the case-insensitive style"
        );
    }

    #[test]
    fn resolution_report_stores_the_size_it_returns() {
        let state = AppState::default();
        let report = resolution_report(&state, 1_280, 800).expect("a supported size");
        assert_eq!((report.width, report.height), (1_280, 800), "the new size");
        let error = resolution_report(&state, 8, 8).expect_err("8 is below the floor");
        assert_eq!(error.code(), "invalid_argument", "tiny size code");
        let stored = state
            .with_session(|session| Ok(session.render_settings().clone()))
            .expect("a fresh lock is never poisoned");
        assert_eq!(
            (stored.width, stored.height),
            (1_280, 800),
            "the refused size did not land"
        );
    }

    #[test]
    fn resolution_report_accepts_the_largest_square() {
        let state = AppState::default();
        let report = resolution_report(&state, 4_096, 4_096).expect("the cap square");
        assert_eq!(report.width, 4_096, "the widest edge");
        assert_eq!(report.height, 4_096, "the tallest edge");
    }

    use super::{
        Annotation, AnnotationParams, Bounds, BoundsParams, Edge, GRID_MAJOR, GRID_MINOR, GridLine,
        HighlightParams, Highlights, LABEL_COLOUR, Labels, LabelsParams, MINIMAP_FIT_PAD, Plane,
        PointParams, Projection, RenderMinimapParams, RenderTopdownParams, Request, Scene,
        Underlay, View, background_pixel, bounds_plane, check_overlays, editor_line_color, fit,
        grid_frame, height_span, iso_plane, light_grey, paint_grid_line, paint_map, paint_minimap,
        parse_projection, resolve_annotations, resolve_bounds, resolve_centre, resolve_grid,
        resolve_highlight, resolve_label_scale, resolve_labels, resolve_minimap, resolve_opacity,
        resolve_radius, resolve_request, resolve_size, scale_alpha, sector_label_point,
        to_overlay_units, underlay_rect,
    };
    use crate::defs::definitions_for;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::palette::Palette;
    use crate::plan::Units;
    use crate::render::canvas::intersect_x;
    use crate::render::{
        AutomapColors, Canvas, PlayerMark, Pt, Rgba, Switch, category_color, scale_down,
        thing_visual,
    };
    use crate::tools::plan::params::UnitsParam;
    use crate::wad::game::Game;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::{LumpName, MapFormat};
    use serde_json::json;

    /// An opaque test colour.
    const fn rgb(red: u8, green: u8, blue: u8) -> Rgba {
        [red, green, blue, 255_u8]
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Vertex::default()
        }
    }

    /// A blocking line from `v1` to `v2` with the given sides.
    fn edge(v1: usize, v2: usize, front: usize, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1_u32,
            front: Some(front),
            back,
            ..Linedef::default()
        }
    }

    /// A sidedef opening into `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Sidedef::default()
        }
    }

    /// A sector with the given heights and light.
    fn room(floor_height: i32, ceiling_height: i32, light: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            light,
            ..Sector::default()
        }
    }

    /// A thing of `kind` at `x`/`y` facing `angle`.
    fn spawn(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            angle,
            flags: 7_u32,
            ..Thing::default()
        }
    }

    /// The two-room fixture as the renderer sees it: a hall (sector 0,
    /// light 160), a raised back room (sector 1, light 192), the two-sided
    /// step line at `x = 256` (index 2), a player start and an imp.
    fn two_rooms() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("test names are valid"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things: vec![spawn(1, 64, 128, 0), spawn(3001, 384, 128, 180)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                edge(0, 5, 0, None),
                edge(5, 4, 1, None),
                edge(4, 1, 2, Some(3)),
                edge(1, 0, 4, None),
                edge(4, 3, 5, None),
                edge(3, 2, 6, None),
                edge(2, 1, 7, None),
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
            sectors: vec![room(0, 128, 160), room(32, 128, 192)],
            other: Vec::new(),
        }
    }

    /// The automap colour of `map`'s linedef at `index`, with `with`
    /// standing in for it when given.
    fn automap_colour(map: &Map, index: usize, with: Option<&Linedef>) -> Option<Rgba> {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let bounds = resolve_bounds(None, map, None).expect("the fixture map bounds itself");
        let view = fit(&bounds_plane(&bounds), 64, 64);
        let scene = Scene {
            map,
            defs: &defs,
            view,
            canvas: Canvas::new(8, 8, [0_u8, 0, 0, 255]),
            notes: Vec::new(),
        };
        let line = match with {
            Some(replacement) => replacement,
            None => map.linedefs.get(index)?,
        };
        scene.automap_line_color(line)
    }

    /// The default test request over `map`: top-down, 200x120, editor.
    fn request_for(map: &Map) -> Request {
        Request {
            projection: Projection::Topdown,
            width: 200,
            height: 120,
            style: TopdownStyle::Editor,
            bounds: resolve_bounds(None, map, None).expect("the fixture map bounds itself"),
            background: [0_u8, 0, 0, 255],
            show_things: Switch::new(true),
            show_vertices: Switch::new(false),
            fill_sectors: Switch::new(false),
            highlight: Highlights::default(),
            underlay: None,
            grid: None,
            labels: Labels::default(),
            annotations: Vec::new(),
            label_scale: 1_u32,
        }
    }

    /// Paint the `map` and read one pixel of the result.
    fn pixel_of(map: &Map, request: &Request, x: u32, y: u32) -> Option<Rgba> {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        paint_map(map, &defs, request).canvas.pixel(x, y)
    }

    #[test]
    fn fit_picks_the_constraining_axis_and_centres_the_view() {
        let plane = Plane {
            min_u: -32,
            max_u: 544,
            min_v: -32,
            max_v: 288,
        };
        let view = fit(&plane, 200, 120);
        assert_eq!((view.num, view.den), (88, 320), "the height fits exactly");
        assert_eq!(
            (view.offset_x, view.offset_y),
            (30, 95),
            "centred horizontally, the full height vertically"
        );
        assert_eq!(view.point(256, 128), (100, 60), "the step line midpoint");
        assert_eq!(view.point(64, 128), (47, 60), "the player start");
        assert_eq!(view.point(-32, -32), (21, 104), "the south-west corner");
    }

    #[test]
    fn fit_centres_a_square_plane_and_holds_the_margins() {
        let plane = Plane {
            min_u: -256,
            max_u: 256,
            min_v: -256,
            max_v: 256,
        };
        let view = fit(&plane, 200, 200);
        assert_eq!(view.point(0, 0), (100, 100), "the origin centres");
        let west = view.point(-256, 0).0;
        let east = view.point(256, 0).0;
        assert!(
            west >= 16_i64 && east <= 184_i64,
            "the 16-pixel margins hold: {west}..{east}"
        );
    }

    #[test]
    fn scale_down_floors_towards_negative_infinity() {
        assert_eq!(scale_down(-32, 88, 320), -9, "-8.8 floors to -9");
        assert_eq!(scale_down(31, 88, 320), 8, "8.52 floors to 8");
        assert_eq!(scale_down(0, 88, 320), 0, "zero stays zero");
    }

    #[test]
    fn projection_parses_its_two_names_case_insensitively() {
        assert_eq!(
            parse_projection(None).expect("absent is top-down"),
            Projection::Topdown
        );
        assert_eq!(
            parse_projection(Some("IsoMetric"))
                .expect("a known name")
                .as_str(),
            "isometric"
        );
        assert_eq!(Projection::Topdown.as_str(), "topdown", "the report name");
        let error = parse_projection(Some("birdseye")).expect_err("not a projection");
        assert_eq!(error.code(), "invalid_argument", "unknown projection code");
        let message = error.to_string();
        assert!(
            message.contains("\"topdown\"") && message.contains("\"isometric\""),
            "the message lists both names: {message}"
        );
    }

    #[test]
    fn bounds_default_pads_and_explicit_refuses_flattened_rectangles() {
        let map = two_rooms();
        let padded = resolve_bounds(None, &map, None).expect("the default frames the map");
        assert_eq!(
            (padded.min_x, padded.max_x, padded.min_y, padded.max_y),
            (-32_i64, 544, -32, 288),
            "32 units of slack on every side"
        );
        let flat = BoundsParams {
            min_x: 0,
            min_y: 0,
            max_x: 0,
            max_y: 100,
        };
        let error = resolve_bounds(Some(&flat), &map, None).expect_err("min_x == max_x");
        assert_eq!(error.code(), "invalid_argument", "flat bounds code");
        assert!(
            error.to_string().contains("min_x < max_x"),
            "the message states the rule: {error}"
        );
        let empty = Map::default();
        let missing = resolve_bounds(None, &empty, None).expect_err("nothing to frame");
        assert_eq!(missing.code(), "invalid_argument", "empty map code");
        assert!(
            missing.to_string().contains("bounds"),
            "the message names the fix: {missing}"
        );
    }

    #[test]
    fn highlight_indices_must_exist_and_the_refusal_names_them() {
        let map = two_rooms();
        let good = HighlightParams {
            linedefs: Some(vec![2_usize]),
            sectors: Some(vec![1_usize]),
            things: Some(vec![0_usize]),
            vertices: Some(vec![5_usize]),
        };
        assert!(
            resolve_highlight(Some(&good), &map).is_ok(),
            "in-range indices pass"
        );
        for (field, kind, value) in [
            ("linedefs", "linedef", 7_usize),
            ("sectors", "sector", 2_usize),
            ("things", "thing", 2_usize),
            ("vertices", "vertex", 6_usize),
        ] {
            let mut object = serde_json::Map::new();
            object.insert(field.to_owned(), json!([value]));
            let params: HighlightParams = serde_json::from_value(json!(object)).expect("one list");
            let error = resolve_highlight(Some(&params), &map)
                .expect_err("an index past the end of the table");
            assert_eq!(error.code(), "not_found", "{field} code");
            let message = error.to_string();
            assert!(
                message.contains(kind)
                    && message.contains(&value.to_string())
                    && message.contains("out of range"),
                "the message names the element, index and rule: {message}"
            );
        }
    }

    #[test]
    fn background_parses_the_settings_colour() {
        assert_eq!(background_pixel("#1A2b3C"), rgb(26, 43, 60), "either case");
        assert_eq!(
            background_pixel("nonsense"),
            rgb(0, 0, 0),
            "junk reads as black"
        );
    }

    #[test]
    fn categories_pick_the_documented_marker_colours() {
        assert_eq!(category_color("Monsters"), rgb(224, 64, 64));
        assert_eq!(category_color("Player Starts"), rgb(64, 224, 64));
        assert_eq!(category_color("Weapons"), rgb(224, 160, 64));
        assert_eq!(category_color("Ammunition"), rgb(224, 160, 64));
        assert_eq!(category_color("Health And Armour"), rgb(64, 160, 224));
        assert_eq!(category_color("Powerups"), rgb(64, 160, 224));
        assert_eq!(category_color("Keys"), rgb(224, 224, 64));
        assert_eq!(category_color("Decorations/Tech"), rgb(176, 176, 176));
    }

    #[test]
    fn thing_visuals_use_the_definitions_or_the_fallback() {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        assert_eq!(thing_visual(&defs, 1), (rgb(64, 224, 64), 16), "player 1");
        assert_eq!(thing_visual(&defs, 3001), (rgb(224, 64, 64), 20), "the imp");
        assert_eq!(
            thing_visual(&defs, 2012),
            (rgb(64, 160, 224), 12),
            "the medikit is Health And Armour"
        );
        assert_eq!(
            thing_visual(&defs, 13),
            (rgb(224, 224, 64), 12),
            "the keycard"
        );
        assert_eq!(
            thing_visual(&defs, 4_000_000),
            (rgb(176, 176, 176), 16),
            "an unknown type falls back to grey and radius 16"
        );
    }

    #[test]
    fn light_greys_are_half_the_level_rounded() {
        assert_eq!(light_grey(160), rgb(80, 80, 80), "the hall");
        assert_eq!(light_grey(192), rgb(96, 96, 96), "the back room");
        assert_eq!(light_grey(255), rgb(128, 128, 128), "255/2 rounds to 128");
        assert_eq!(light_grey(-10), rgb(0, 0, 0), "a negative light clamps");
    }

    #[test]
    fn editor_colours_split_special_two_sided_and_plain_lines() {
        assert_eq!(
            editor_line_color(&edge(0, 1, 0, None)),
            rgb(192, 192, 192),
            "one-sided"
        );
        assert_eq!(
            editor_line_color(&edge(0, 1, 0, Some(1))),
            rgb(112, 112, 112),
            "two-sided"
        );
        let mut special = edge(0, 1, 0, None);
        special.special = 1_i32;
        assert_eq!(editor_line_color(&special), rgb(106, 140, 255));
    }

    #[test]
    fn automap_colours_follow_the_vanilla_line_rules() {
        let map = two_rooms();
        assert_eq!(
            automap_colour(&map, 0, None),
            Some(rgb(252, 0, 0)),
            "outer wall"
        );
        assert_eq!(
            automap_colour(&map, 2, None),
            Some(rgb(188, 120, 72)),
            "the step line changes floor height"
        );
        let mut drawn = map.linedefs.first().expect("line 0 exists").clone();
        drawn.flags = 129_u32;
        assert_eq!(
            automap_colour(&map, 0, Some(&drawn)),
            None,
            "dontdraw lines are skipped"
        );
        let mut secret = map.linedefs.get(2).expect("line 2 exists").clone();
        secret.flags = 4_u32 | 32_u32;
        assert_eq!(
            automap_colour(&map, 2, Some(&secret)),
            Some(rgb(252, 0, 0)),
            "a secret line reads as one-sided"
        );
    }

    #[test]
    fn iso_planes_span_the_projected_bounds_and_heights() {
        let map = two_rooms();
        assert_eq!(height_span(&map), (0_i64, 128), "floor 0, ceiling 128");
        let bounds = resolve_bounds(None, &map, None).expect("the default bounds");
        let plane = iso_plane(&bounds, height_span(&map));
        assert_eq!(plane.min_u, -320, "west corner minus north");
        assert_eq!(plane.max_u, 576, "east corner minus south");
        assert_eq!(plane.min_v, -160, "south-west half-sum minus the top");
        assert_eq!(plane.max_v, 416, "north-east half-sum minus the floor");
        assert_eq!(
            height_span(&Map::default()),
            (0_i64, 0),
            "a sector-less map spans nothing"
        );
    }

    #[test]
    fn painting_the_fixture_lands_lines_and_things_where_the_transform_says() {
        let map = two_rooms();
        let request = request_for(&map);
        assert_eq!(
            pixel_of(&map, &request, 100, 60),
            Some(rgb(112, 112, 112)),
            "the two-sided step line at its midpoint"
        );
        assert_eq!(
            pixel_of(&map, &request, 47, 60),
            Some(rgb(64, 224, 64)),
            "the player start centre"
        );
        assert_eq!(
            pixel_of(&map, &request, 30, 60),
            Some(rgb(192, 192, 192)),
            "the one-sided west wall"
        );
    }

    #[test]
    fn painting_with_fills_and_highlights_tints_and_thickens() {
        let map = two_rooms();
        let mut filled = request_for(&map);
        filled.show_things = Switch::new(false);
        filled.fill_sectors = Switch::new(true);
        assert_eq!(
            pixel_of(&map, &filled, 47, 40),
            Some(rgb(80, 80, 80)),
            "the hall fills with half its light"
        );
        assert_eq!(
            pixel_of(&map, &filled, 140, 40),
            Some(rgb(96, 96, 96)),
            "the back room is brighter"
        );
        let mut marked = request_for(&map);
        marked.show_things = Switch::new(false);
        marked.highlight.linedefs.push(2_usize);
        assert_eq!(
            pixel_of(&map, &marked, 99, 60),
            Some(rgb(255, 64, 64)),
            "the highlight runs 3 pixels wide"
        );
        assert_eq!(
            pixel_of(&map, &marked, 100, 60),
            Some(rgb(255, 64, 64)),
            "including the line centre"
        );
    }

    #[test]
    fn the_isometric_projection_paints_a_wireframe_and_reports_no_transform() {
        let map = two_rooms();
        let mut request = request_for(&map);
        request.projection = Projection::Isometric;
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let painted = paint_map(&map, &defs, &request);
        let canvas = painted.canvas;
        let view = painted.view;
        assert!(view.is_none(), "isometric reports no transform");
        let blank = [0_u8, 0, 0, 255];
        let marks: usize = (0..canvas.height())
            .flat_map(|row| (0..canvas.width()).map(move |col| (col, row)))
            .filter(|&(col, row)| canvas.pixel(col, row) != Some(blank))
            .count();
        assert!(marks > 0, "the wireframe drew {marks} pixels");
    }

    #[test]
    fn resolve_request_falls_back_to_the_settings_and_validates_overrides() {
        let map = two_rooms();
        let settings = RenderSettings::default();
        let plain: RenderTopdownParams = serde_json::from_value(json!({})).expect("all optional");
        let request = resolve_request(&settings, &plain, &map, None).expect("defaults resolve");
        assert_eq!(
            (request.width, request.height),
            (640_u32, 400),
            "the size default"
        );
        assert!(request.show_things.is_on(), "the settings enable things");
        assert!(!request.show_vertices.is_on(), "vertices are opt-in");
        assert_eq!(request.style, TopdownStyle::Editor, "the style default");

        let over: RenderTopdownParams = serde_json::from_value(json!({
            "width": 64_u64,
            "height": 64_u64,
            "style": "Automap",
            "show_things": false,
            "show_vertices": true,
            "fill_sectors": true,
        }))
        .expect("the override object");
        let overridden = resolve_request(&settings, &over, &map, None).expect("valid overrides");
        assert_eq!(
            (overridden.width, overridden.height),
            (64_u32, 64),
            "size set"
        );
        assert_eq!(overridden.style, TopdownStyle::Automap, "style set");
        assert!(!overridden.show_things.is_on(), "things off");
        assert!(overridden.show_vertices.is_on(), "vertices on");
        assert!(overridden.fill_sectors.is_on(), "fills on");

        let bad_size: RenderTopdownParams =
            serde_json::from_value(json!({ "width": 8_u64 })).expect("one field");
        let size_error =
            resolve_request(&settings, &bad_size, &map, None).expect_err("below the floor");
        assert_eq!(size_error.code(), "invalid_argument", "tiny width code");
        let bad_style: RenderTopdownParams =
            serde_json::from_value(json!({ "style": "wireframe" })).expect("one field");
        let style_error =
            resolve_request(&settings, &bad_style, &map, None).expect_err("not a style");
        assert_eq!(style_error.code(), "invalid_argument", "bad style code");
    }

    /// An `underlay.units` wire with `scale_num` `8` and the `y` mirror.
    fn sketch_units() -> UnitsParam {
        UnitsParam {
            scale_num: 8_i64,
            flip_y: true,
            ..UnitsParam::default()
        }
    }

    /// A `50x40` opaque picture: the left half red, the right half blue.
    fn half_half_image() -> RgbaImage {
        let mut row = Vec::new();
        for _col in 0..25_u32 {
            row.extend_from_slice(&[255_u8, 0, 0, 255]);
        }
        for _col in 0..25_u32 {
            row.extend_from_slice(&[0, 0, 255, 255]);
        }
        let mut pixels = Vec::new();
        for _row in 0..40_u32 {
            pixels.extend_from_slice(&row);
        }
        RgbaImage::new(50, 40, pixels).expect("the buffer matches the shape")
    }

    /// The placed `50x40` underlay at `opacity` percent.
    fn test_underlay(opacity: u8) -> Underlay {
        Underlay {
            image: half_half_image(),
            units: to_overlay_units(Some(sketch_units())),
            opacity,
            rect: (0_i64, -320_i64, 400_i64, 0_i64),
        }
    }

    #[test]
    fn the_overlay_units_transform_is_unsnapped_and_the_rect_frames_the_image() {
        let units = to_overlay_units(Some(sketch_units()));
        assert_eq!(units.grid, 1_i64, "picture sampling never snaps");
        let image = half_half_image();
        assert_eq!(
            underlay_rect(&units, &image).expect("the corners fit"),
            (0_i64, -320, 400, 0),
            "8 units per pixel with the mirror runs south"
        );
        assert_eq!(
            underlay_rect(&Units::default(), &image).expect("identity fits"),
            (0_i64, 0, 50, 40),
            "the identity maps the raw pixel box"
        );
        let wild = Units {
            scale_num: i64::MAX,
            ..Units::default()
        };
        assert_eq!(
            underlay_rect(&wild, &image)
                .expect_err("a corner leaves the range")
                .code(),
            "invalid_argument",
            "an overflowing rectangle is refused"
        );
    }

    #[test]
    fn bounds_union_the_map_with_the_underlay_and_an_underlay_frames_an_empty_map() {
        let map = two_rooms();
        let rect = (0_i64, -320, 400, 0);
        let unioned = resolve_bounds(None, &map, Some(rect)).expect("the union frames");
        assert_eq!(
            (unioned.min_x, unioned.max_x, unioned.min_y, unioned.max_y),
            (-32_i64, 544, -352, 288),
            "the map reaches east, the picture reaches south"
        );
        let empty = Map::default();
        let picture = resolve_bounds(None, &empty, Some(rect)).expect("an underlay frames");
        assert_eq!(
            (picture.min_x, picture.min_y, picture.max_x, picture.max_y),
            (-32_i64, -352, 432, 32),
            "the padded picture rectangle alone is enough"
        );
        let chosen = BoundsParams {
            min_x: 0,
            min_y: 0,
            max_x: 100,
            max_y: 100,
        };
        let explicit = resolve_bounds(Some(&chosen), &empty, Some(rect)).expect("bounds win");
        assert_eq!(
            (explicit.min_x, explicit.max_y),
            (0_i64, 100),
            "an explicit box is not unioned"
        );
    }

    #[test]
    fn overlays_are_refused_for_the_isometric_projection_only() {
        for value in [
            json!({"grid": 64_i64}),
            json!({"underlay": {"source": "/tmp/sketch.png"}}),
            json!({"labels": {"sectors": true}}),
            json!({"annotations": [{"x": 0_i64, "y": 0_i64, "text": "A"}]}),
            json!({"label_scale": 2_u64}),
        ] {
            let args: RenderTopdownParams = serde_json::from_value(value.clone()).expect("overlay");
            check_overlays(&args, Projection::Topdown).expect("topdown takes every overlay");
            let error = check_overlays(&args, Projection::Isometric)
                .expect_err("isometric refuses the overlay");
            assert_eq!(error.code(), "invalid_argument", "overlay isometric code");
            assert!(
                error
                    .to_string()
                    .contains("overlays need the topdown projection"),
                "the message names the projection: {error}"
            );
        }
        let bare: RenderTopdownParams =
            serde_json::from_value(json!({"annotations": [], "labels": null})).expect("empties");
        check_overlays(&bare, Projection::Isometric).expect("nothing asked for nothing");
    }

    #[test]
    fn grid_spacing_and_label_scale_check_their_ranges() {
        assert_eq!(
            resolve_grid(None).expect("absent draws no grid"),
            None,
            "absent"
        );
        assert_eq!(
            resolve_grid(Some(8)).expect("the floor"),
            Some(8_i64),
            "the floor passes"
        );
        assert_eq!(
            resolve_grid(Some(1_024)).expect("the ceiling"),
            Some(1_024),
            "the ceiling passes"
        );
        for bad in [7_i64, 2_048_i64] {
            let error = resolve_grid(Some(bad)).expect_err("outside the range");
            assert_eq!(error.code(), "invalid_argument", "grid {bad} code");
            assert!(
                error.to_string().contains("8..=1024"),
                "the message shows the range: {error}"
            );
        }
        assert_eq!(
            resolve_label_scale(None, 640).expect("a narrow default"),
            1_u32,
            "narrow output labels at one"
        );
        assert_eq!(
            resolve_label_scale(None, 1_024).expect("a wide default"),
            2_u32,
            "wide output labels at two"
        );
        assert_eq!(
            resolve_label_scale(Some(4), 100).expect("an explicit scale"),
            4_u32,
            "the ceiling passes"
        );
        for bad in [0_u32, 5_u32] {
            let error = resolve_label_scale(Some(bad), 640).expect_err("outside the range");
            assert_eq!(error.code(), "invalid_argument", "scale {bad} code");
            assert!(
                error.to_string().contains("1..=4"),
                "the message shows the range: {error}"
            );
        }
        assert_eq!(
            resolve_label_scale(Some(2), 4_096).expect("an explicit wide scale"),
            2_u32,
            "an explicit scale beats the default"
        );
    }

    #[test]
    fn opacity_defaults_to_half_and_refuses_the_over_hundred() {
        assert_eq!(resolve_opacity(None).expect("the default"), 50_u8, "half");
        assert_eq!(
            resolve_opacity(Some(0)).expect("clear"),
            0_u8,
            "clear passes"
        );
        assert_eq!(
            resolve_opacity(Some(100)).expect("solid"),
            100_u8,
            "solid passes"
        );
        let error = resolve_opacity(Some(101)).expect_err("over the top");
        assert_eq!(error.code(), "invalid_argument", "opacity code");
        assert!(
            error.to_string().contains("0..=100"),
            "the message shows the range: {error}"
        );
    }

    #[test]
    fn annotations_uppercase_and_both_limits_and_bad_colours_are_refused() {
        let rows: Vec<AnnotationParams> = serde_json::from_value(json!([
            {"x": 8_i64, "y": -8_i64, "text": " armoury "},
            {"x": 0_i64, "y": 0_i64, "text": "X", "color": "#102030"},
        ]))
        .expect("two rows");
        let notes = resolve_annotations(Some(&rows)).expect("valid rows");
        let first = notes.first().expect("two annotations");
        assert_eq!(first.text, " ARMOURY ", "the text upper-cases in place");
        assert_eq!(
            first.colour,
            rgb(255, 255, 255),
            "the default colour is white"
        );
        let second = notes.get(1).expect("two annotations");
        assert_eq!(second.colour, rgb(16, 32, 48), "the given colour");
        assert!(
            resolve_annotations(None).expect("absent").is_empty(),
            "none asked"
        );

        let long_text = "X".repeat(33_usize);
        let too_long: Vec<AnnotationParams> =
            serde_json::from_value(json!([{"x": 0_i64, "y": 0_i64, "text": long_text}]))
                .expect("one row");
        let length = resolve_annotations(Some(&too_long)).expect_err("33 characters");
        assert_eq!(length.code(), "invalid_argument", "long text code");
        assert!(
            length.to_string().contains("32"),
            "the message names the limit: {length}"
        );
        let many: Vec<AnnotationParams> = (0_u32..65_u32)
            .map(|index| AnnotationParams {
                x: 0,
                y: 0,
                text: format!("N{index}"),
                color: None,
            })
            .collect();
        let count = resolve_annotations(Some(&many)).expect_err("65 rows");
        assert_eq!(count.code(), "invalid_argument", "row count code");
        assert!(
            count.to_string().contains("64"),
            "the message names the limit: {count}"
        );
        let bad_colour: Vec<AnnotationParams> = serde_json::from_value(json!(
            [{"x": 0_i64, "y": 0_i64, "text": "A", "color": "green"}]
        ))
        .expect("one row");
        assert_eq!(
            resolve_annotations(Some(&bad_colour))
                .expect_err("not a hex colour")
                .code(),
            "invalid_argument",
            "bad colour code"
        );
    }

    #[test]
    fn labels_default_off_and_switch_individually() {
        assert!(!resolve_labels(None).any(), "absent labels draw nothing");
        let wanted: LabelsParams =
            serde_json::from_value(json!({"sectors": true, "things": true})).expect("two switches");
        let some = resolve_labels(Some(&wanted));
        assert!(
            some.sectors.is_on() && some.things.is_on(),
            "the given switches land"
        );
        assert!(
            !some.linedefs.is_on() && !some.vertices.is_on(),
            "the left-out switches stay off"
        );
        assert!(some.any(), "two switches on is on");
    }

    #[test]
    fn the_underlay_alpha_scales_by_the_opacity_percent() {
        assert_eq!(scale_alpha(255, 50), 128, "half of full alpha rounds up");
        assert_eq!(scale_alpha(255, 0), 0, "nothing stays nothing");
        assert_eq!(scale_alpha(255, 100), 255, "solid stays solid");
        assert_eq!(scale_alpha(100, 25), 25, "a quarter of a hundred");
    }

    /// The closed ring through `points` as edges.
    fn ring(points: &[Pt]) -> Vec<Edge> {
        let tail = points.iter().skip(1).copied();
        let closing = core::iter::once(points.first().copied().expect("a ring has a first point"));
        tail.chain(closing).zip(points.iter().copied()).collect()
    }

    /// Whether `at` sits inside the even-odd polygon of `edges`.
    fn inside_edges(edges: &[Edge], at: Pt) -> bool {
        let crossings = edges
            .iter()
            .filter(|edge| (edge.0.1 > at.1) != (edge.1.1 > at.1))
            .filter(|edge| intersect_x(edge.0, edge.1, at.1) < at.0)
            .count();
        crossings.checked_rem(2_usize) == Some(1_usize)
    }

    /// The pixels of `canvas` within `radius` of `centre`, off-canvas
    /// coordinates dropped.
    fn sample_window(canvas: &Canvas, centre: Pt, radius: i64) -> Vec<Rgba> {
        let rows = (centre.1.saturating_sub(radius)..=centre.1.saturating_add(radius))
            .filter_map(|row| u32::try_from(row).ok());
        let columns = (centre.0.saturating_sub(radius)..=centre.0.saturating_add(radius))
            .filter_map(|col| u32::try_from(col).ok());
        rows.flat_map(move |deep| columns.clone().map(move |col| (col, deep)))
            .filter_map(|(column, deep)| canvas.pixel(column, deep))
            .collect()
    }

    #[test]
    fn sector_label_points_land_inside_concave_polygons() {
        let l_shape = ring(&[(0, 0), (60, 0), (60, 20), (20, 20), (20, 60), (0, 60)]);
        let at = sector_label_point(&l_shape).expect("an L has an anchor");
        assert_eq!(at, (10_i64, 30), "the middle row's only span, centred");
        assert!(inside_edges(&l_shape, at), "the L anchor is inside");
        let u_shape = ring(&[
            (0, 0),
            (60, 0),
            (60, 60),
            (40, 60),
            (40, 20),
            (20, 20),
            (20, 60),
            (0, 60),
        ]);
        let anchor = sector_label_point(&u_shape).expect("a U has an anchor");
        assert!(
            inside_edges(&u_shape, anchor),
            "the U anchor {anchor:?} is inside"
        );
        assert_eq!(
            sector_label_point(&[((0, 0), (0, 10))]).expect("a fallback exists"),
            (0_i64, 0),
            "no inside interval falls back to the first vertex"
        );
        assert_eq!(sector_label_point(&[]), None, "nothing to anchor on");
    }

    #[test]
    fn the_underlay_blends_under_the_map_at_its_opacity() {
        let map = two_rooms();
        let mut request = request_for(&map);
        request.width = 200;
        request.height = 160;
        request.bounds = Bounds {
            min_x: -32,
            min_y: -352,
            max_x: 432,
            max_y: 32,
        };
        request.underlay = Some(test_underlay(100));
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let canvas = paint_map(&map, &defs, &request).canvas;
        let view = fit(&bounds_plane(&request.bounds), 200, 160);
        let at = |point: Pt| {
            canvas.pixel(
                u32::try_from(point.0).expect("a fitted column"),
                u32::try_from(point.1).expect("a fitted row"),
            )
        };
        assert_eq!(
            at(view.point(100, -100)),
            Some(rgb(255, 0, 0)),
            "the red left half stands behind the map"
        );
        assert_eq!(
            at(view.point(300, -100)),
            Some(rgb(0, 0, 255)),
            "the blue right half too"
        );
        assert_eq!(
            at(view.point(-24, -100)),
            Some([0, 0, 0, 255]),
            "west of the picture the background stays"
        );
        request.underlay = Some(test_underlay(0));
        let clear = paint_map(&map, &defs, &request).canvas;
        let px = view.point(100, -100);
        assert_eq!(
            clear.pixel(
                u32::try_from(px.0).expect("a fitted column"),
                u32::try_from(px.1).expect("a fitted row"),
            ),
            Some([0, 0, 0, 255]),
            "zero opacity draws no underlay"
        );
    }

    #[test]
    fn grid_lines_pick_their_colour_across_the_frame() {
        let map = two_rooms();
        let request = request_for(&map);
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let mut scene = Scene {
            map: &map,
            defs: &defs,
            view: fit(&bounds_plane(&request.bounds), 200, 120),
            canvas: Canvas::new(200, 120, [0_u8, 0, 0, 255]),
            notes: Vec::new(),
        };
        paint_grid_line(
            &mut scene,
            &GridLine {
                at: 64,
                vertical: true,
                low: -32,
                high: 288,
            },
            1_i64,
        );
        paint_grid_line(
            &mut scene,
            &GridLine {
                at: 0,
                vertical: true,
                low: -32,
                high: 288,
            },
            8_i64,
        );
        assert_eq!(
            scene.canvas.pixel(47, 60),
            Some(GRID_MINOR),
            "the k=1 line at x=64 is minor grey"
        );
        assert_eq!(
            scene.canvas.pixel(30, 60),
            Some(GRID_MAJOR),
            "the k=8 line at x=0 is major grey"
        );
        assert_eq!(
            scene.canvas.pixel(38, 60),
            Some([0, 0, 0, 255]),
            "between the lines stays background"
        );
    }

    #[test]
    fn the_grid_frame_is_the_unprojected_canvas_rectangle() {
        let map = two_rooms();
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        assert_eq!(
            grid_frame(&Scene {
                map: &map,
                defs: &defs,
                view: View {
                    num: 128,
                    den: 384,
                    offset_x: 16,
                    offset_y: 80,
                },
                canvas: Canvas::new(200, 160, [0_u8, 0, 0, 255]),
                notes: Vec::new(),
            }),
            (-48_i64, 240, 549, -237),
            "the canvas corners unproject to the covered world rectangle"
        );
    }

    #[test]
    fn sector_labels_and_annotations_paint_their_text() {
        let map = two_rooms();
        let mut request = request_for(&map);
        request.show_things = Switch::new(false);
        request.labels = Labels {
            sectors: Switch::new(true),
            ..Labels::default()
        };
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let canvas = paint_map(&map, &defs, &request).canvas;
        let hits = (55..76_u32)
            .flat_map(|row| (58..73_u32).map(move |col| (col, row)))
            .filter(|&(col, row)| canvas.pixel(col, row) == Some(LABEL_COLOUR))
            .count();
        assert!(
            hits > 4,
            "the hall's number sits at its anchor (65, 60): {hits} label pixels"
        );
        let far = (4..14_u32)
            .flat_map(|row| (4..14_u32).map(move |col| (col, row)))
            .filter(|&(col, row)| canvas.pixel(col, row) == Some(LABEL_COLOUR))
            .count();
        assert_eq!(far, 0, "no label colour in the far margin");

        let mut noted = request_for(&map);
        noted.show_things = Switch::new(false);
        noted.background = [10, 10, 10, 255];
        noted.annotations = vec![Annotation {
            x: -64,
            y: -64,
            text: "GO".to_owned(),
            colour: rgb(255, 255, 255),
        }];
        let noted_canvas = paint_map(&map, &defs, &noted).canvas;
        let view = fit(&bounds_plane(&noted.bounds), 200, 120);
        let colours = sample_window(&noted_canvas, view.point(-64, -64), 9);
        assert!(
            colours.contains(&rgb(255, 255, 255)),
            "the annotation draws its white text centred on the point"
        );
        assert!(
            colours.contains(&[0, 0, 0, 255]),
            "and its black outline against the lifted background"
        );
    }

    #[test]
    fn resolve_request_carries_the_overlays_and_the_empty_map_gains_a_frame() {
        let map = two_rooms();
        let settings = RenderSettings::default();
        let args: RenderTopdownParams = serde_json::from_value(json!({
            "width": 200_u64,
            "height": 160_u64,
            "grid": 64_i64,
            "labels": {"sectors": true},
            "annotations": [{"x": 0_i64, "y": 0_i64, "text": "a"}],
        }))
        .expect("the overlay call");
        let request = resolve_request(&settings, &args, &map, Some(test_underlay(50)))
            .expect("overlays resolve");
        assert_eq!(request.grid, Some(64_i64), "the grid carries");
        assert!(
            request.labels.sectors.is_on() && !request.labels.vertices.is_on(),
            "only the asked-for labels"
        );
        assert_eq!(
            request.annotations.first().expect("one note").text,
            "A",
            "the text arrives upper-cased"
        );
        assert_eq!(
            request
                .underlay
                .as_ref()
                .expect("the underlay carries through")
                .opacity,
            50_u8,
            "the default opacity"
        );
        assert_eq!(request.bounds.min_y, -352, "the underlay joined the frame");
        assert_eq!(request.label_scale, 1_u32, "the narrow default");

        let iso: RenderTopdownParams =
            serde_json::from_value(json!({"projection": "isometric", "grid": 64_i64}))
                .expect("one field");
        let error =
            resolve_request(&settings, &iso, &map, None).expect_err("isometric refuses the grid");
        assert!(
            error
                .to_string()
                .contains("overlays need the topdown projection"),
            "the message names the projection: {error}"
        );
    }

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`.
    fn fixture_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, entry) in colors.iter_mut().enumerate() {
            let value = u8::try_from(index).expect("indices stay in the u8 range");
            *entry = [value, 255_u8.wrapping_sub(value), value];
        }
        Palette::from(colors)
    }

    #[test]
    fn minimap_size_defaults_and_refuses_edges_outside_the_range() {
        assert_eq!(
            resolve_size(None).expect("the default"),
            256,
            "256 by default"
        );
        assert_eq!(resolve_size(Some(32)).expect("the floor"), 32, "the floor");
        assert_eq!(
            resolve_size(Some(1_024)).expect("the ceiling"),
            1_024,
            "the ceiling"
        );
        for bad in [16_u32, 2_048_u32] {
            let error = resolve_size(Some(bad)).expect_err("outside the range");
            assert_eq!(error.code(), "invalid_argument", "size {bad} code");
            assert!(
                error.to_string().contains("32..=1024"),
                "the message shows the range: {error}"
            );
        }
    }

    #[test]
    fn minimap_radius_validates_and_fits_the_whole_map() {
        let map = two_rooms();
        assert_eq!(
            resolve_radius(Some(64), &map).expect("an explicit radius"),
            64,
            "the given radius"
        );
        assert_eq!(
            resolve_radius(None, &map).expect("the whole map fits"),
            257,
            "half the 512-span rounded up plus the fit padding"
        );
        assert_eq!(MINIMAP_FIT_PAD, 1, "the documented slack");
        for bad in [0_u32, 100_000_u32] {
            let error = resolve_radius(Some(bad), &map).expect_err("outside the range");
            assert_eq!(error.code(), "invalid_argument", "radius {bad} code");
            assert!(
                error.to_string().contains("1..=65536"),
                "the message shows the range: {error}"
            );
        }
        let empty = Map::default();
        let error = resolve_radius(None, &empty).expect_err("nothing to fit");
        assert_eq!(error.code(), "invalid_argument", "empty map code");
        let message = error.to_string();
        assert!(
            message.contains("center") && message.contains("radius"),
            "the message names both fixes: {message}"
        );
    }

    #[test]
    fn minimap_centre_defaults_to_the_map_midpoint() {
        let map = two_rooms();
        assert_eq!(
            resolve_centre(None, &map).expect("the map centre"),
            (256_i64, 128),
            "midpoint of both spans"
        );
        let point = PointParams { x: -100, y: 40 };
        assert_eq!(
            resolve_centre(Some(&point), &map).expect("an explicit centre"),
            (-100_i64, 40),
            "the given centre wins"
        );
        let empty = Map::default();
        assert_eq!(
            resolve_centre(None, &empty)
                .expect_err("nothing to centre on")
                .code(),
            "invalid_argument",
            "empty map code"
        );
    }

    #[test]
    fn the_minimap_request_resolves_every_argument() {
        let map = two_rooms();
        let plain: RenderMinimapParams = serde_json::from_value(json!({})).expect("all optional");
        let request = resolve_minimap(&plain, &map, None).expect("defaults resolve");
        assert_eq!(request.size, 256, "the size default");
        assert_eq!(request.window.centre(), (256, 128), "the map centre");
        assert_eq!(request.window.radius(), 257, "the fitting radius");
        assert_eq!(
            request.colors,
            AutomapColors::DEFAULT,
            "no palette means the defaults"
        );
        assert!(!request.show_things, "thing dots are opt-in");
        assert!(request.player.is_none(), "no arrow by default");

        let palette = fixture_palette();
        let detailed: RenderMinimapParams = serde_json::from_value(json!({
            "size": 64_u64,
            "radius": 32_u64,
            "center": { "x": 64_i64, "y": 128_i64 },
            "player": { "x": 64_i64, "y": 128_i64, "angle": 90_i64 },
            "show_things": true,
        }))
        .expect("the full object");
        let seen = resolve_minimap(&detailed, &map, Some(&palette)).expect("valid arguments");
        assert_eq!(seen.size, 64, "size set");
        assert_eq!(seen.window.centre(), (64, 128), "centre set");
        assert_eq!(seen.window.radius(), 32, "radius set");
        assert_eq!(seen.colors.wall, [176, 79, 176, 255], "palette colours");
        assert!(seen.show_things, "dots on");
        assert_eq!(
            seen.player,
            Some(PlayerMark {
                x: 64,
                y: 128,
                angle: 90
            }),
            "the arrow carries position and facing"
        );

        let declined: RenderMinimapParams =
            serde_json::from_value(json!({ "palette_colors": false })).expect("one field");
        assert_eq!(
            resolve_minimap(&declined, &map, Some(&palette))
                .expect("valid argument")
                .colors,
            AutomapColors::DEFAULT,
            "the palette is declined"
        );
        let bad: RenderMinimapParams =
            serde_json::from_value(json!({ "size": 8_u64 })).expect("one field");
        assert_eq!(
            resolve_minimap(&bad, &map, None)
                .expect_err("below the floor")
                .code(),
            "invalid_argument",
            "tiny size code"
        );
    }

    #[test]
    fn painting_the_minimap_lands_automap_colours_on_black() {
        let map = two_rooms();
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let args: RenderMinimapParams = serde_json::from_value(json!({
            "size": 256_u64,
            "palette_colors": false,
            "show_things": true,
            "player": { "x": 256_i64, "y": 128_i64, "angle": 0_i64 },
        }))
        .expect("the picture arguments");
        let request = resolve_minimap(&args, &map, None).expect("valid arguments");
        let canvas = paint_minimap(&map, &defs, &request);
        assert_eq!(
            (canvas.width(), canvas.height()),
            (256_u32, 256),
            "the square output"
        );
        assert_eq!(
            canvas.pixel(0, 128),
            Some([252, 0, 0, 255]),
            "the west wall"
        );
        assert_eq!(
            canvas.pixel(128, 120),
            Some([188, 120, 72, 255]),
            "the step line brown"
        );
        assert_eq!(
            canvas.pixel(128, 128),
            Some([255, 255, 255, 255]),
            "the arrow at its position"
        );
        assert_eq!(
            canvas.pixel(144, 128),
            Some([255, 255, 255, 255]),
            "the arrow tip sixteen pixels on"
        );
        assert_eq!(
            canvas.pixel(32, 128),
            Some([64, 224, 64, 255]),
            "the player-start dot"
        );
        assert_eq!(canvas.pixel(5, 5), Some([0, 0, 0, 255]), "black ground");
    }

    use super::{Definitions, ONE_16, SPRITE_MIN_TZ16, SpriteSpot, SpriteWiring};
    use crate::render::ViewCamera;

    /// A camera standing at `x`/`y`, facing `angle` degrees.
    fn view_camera(x: i32, y: i32, angle: i32) -> ViewCamera {
        ViewCamera {
            x,
            y,
            z: 41_i32,
            angle,
            pitch: 0_i32,
            fov: 90_u32,
        }
    }

    /// `value` map units in raw 16.16.
    fn raw16(value: i64) -> i128 {
        i128::from(value).saturating_mul(ONE_16)
    }

    /// The wiring over the embedded `Doom II` tables, due east from
    /// `(64, 128)` like the view fixture camera.
    fn east_wiring(defs: &Definitions) -> SpriteWiring<'_> {
        SpriteWiring::new(defs, &view_camera(64, 128, 0))
    }

    #[test]
    fn sprite_projection_measures_forward_and_right_from_the_camera() {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let east = east_wiring(&defs);
        let imp = SpriteSpot {
            x: 384,
            y: 128,
            z: 0,
            angle: 180,
        };
        assert_eq!(
            east.project(&imp),
            (0_i128, raw16(320)),
            "straight east of the east-facing camera"
        );
        let south_side = SpriteSpot { y: 64, ..imp };
        assert_eq!(
            east.project(&south_side),
            (raw16(64), raw16(320)),
            "south of the view axis is positive right when facing east"
        );
        let north = SpriteWiring::new(&defs, &view_camera(64, 128, 90));
        assert_eq!(
            north.project(&imp),
            (raw16(320), 0_i128),
            "due east of a north-facing camera is all right-offset and no depth"
        );
    }

    #[test]
    fn the_skip_line_is_four_map_units_of_forward_depth() {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let east = east_wiring(&defs);
        let near = SpriteSpot {
            x: 67,
            y: 128,
            z: 0,
            angle: 0,
        };
        let far = SpriteSpot { x: 68, ..near };
        assert!(
            east.project(&near).1 < SPRITE_MIN_TZ16,
            "three units ahead is under the line"
        );
        assert!(
            east.project(&far).1 >= SPRITE_MIN_TZ16,
            "four units ahead is not"
        );
    }

    #[test]
    fn only_a_known_kind_with_a_sprite_dresses_a_thing() {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let east = east_wiring(&defs);
        let imp = east
            .definition(&spawn(3001, 384, 128, 180))
            .expect("the imp carries a sprite");
        assert_eq!(
            (imp.sprite.as_str(), imp.frame, imp.hanging),
            ("TROO", 'A', false)
        );
        assert!(
            east.definition(&spawn(-1, 0, 0, 0)).is_none(),
            "a negative kind dresses nothing"
        );
        assert!(
            east.definition(&spawn(4_000_000, 0, 0, 0)).is_none(),
            "an unknown kind dresses nothing"
        );
    }

    #[test]
    fn sprite_problem_lines_dedup_per_sprite_name() {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let mut wiring = east_wiring(&defs);
        wiring.report("POSS", true);
        wiring.report("POSS", true);
        wiring.report("POSS", false);
        wiring.report("HEAD", false);
        assert_eq!(
            wiring.problems,
            [
                "missing sprite POSS".to_owned(),
                "unreadable sprite HEAD".to_owned(),
            ],
            "one line per name and the first failure's word wins"
        );
    }
}
