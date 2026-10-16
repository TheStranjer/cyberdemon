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
//! coordinates.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::fixed::{Angle, mul_div_round, rotate_point};
use crate::gfx::codec::encode_png;
use crate::render::{
    Canvas, Edge, Pt, RenderSettings, Rgba, Stroke, Switch, TopdownStyle, gamma_table,
};
use crate::server::Cyberdemon;
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::common;
use crate::tools::{fail, ok};
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
    /// Whether top-down renders and minimaps mark thing lumps.
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

/// `value * num / den` rounded down, exact in `i128` and saturating back
/// to `i64`; a non-positive `den` behaves like `1`.
fn scale_down(value: i64, num: i64, den: i64) -> i64 {
    let product = i128::from(value).saturating_mul(i128::from(num));
    narrow(product.div_euclid(i128::from(den).max(1)))
}

/// The `i64` nearest an `i128` value (saturating at the bounds).
fn narrow(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
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

/// The rectangle to frame: the validated `requested` bounds, else the
/// map's vertex bounds padded by [`BOUNDS_PAD`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an inverted or flat rectangle, and
/// for an empty map without explicit bounds.
fn resolve_bounds(requested: Option<&BoundsParams>, map: &Map) -> Result<Bounds, CyberError> {
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
    let (min_x, min_y, max_x, max_y) = map.bounds().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} has no vertices to frame: pass bounds {{min_x, min_y, max_x, max_y}} to render \
             it",
            map.name
        ))
    })?;
    Ok(Bounds {
        min_x: i64::from(min_x).saturating_sub(BOUNDS_PAD),
        min_y: i64::from(min_y).saturating_sub(BOUNDS_PAD),
        max_x: i64::from(max_x).saturating_add(BOUNDS_PAD),
        max_y: i64::from(max_y).saturating_add(BOUNDS_PAD),
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
/// map.
///
/// # Errors
///
/// [`parse_projection`], [`RenderSettings::set_resolution`],
/// [`RenderSettings::set_topdown_style`], [`resolve_bounds`] and
/// [`resolve_highlight`]'s errors.
fn resolve_request(
    settings: &RenderSettings,
    args: &RenderTopdownParams,
    map: &Map,
) -> Result<Request, CyberError> {
    let projection = parse_projection(args.projection.as_deref())?;
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
        bounds: resolve_bounds(args.bounds.as_ref(), map)?,
        background: background_pixel(&settings.background),
        show_things: args.show_things.map_or(settings.show_things, Switch::from),
        show_vertices: args.show_vertices.unwrap_or(false).into(),
        fill_sectors: args.fill_sectors.unwrap_or(false).into(),
        highlight: resolve_highlight(args.highlight.as_ref(), map)?,
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

/// The marker colour and world radius of thing type `kind` according to
/// `defs`; an unknown type reads as `#B0B0B0` with radius 16.
fn thing_visual(defs: &Definitions, kind: i32) -> (Rgba, u32) {
    let found = u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number));
    let Some(row) = found else {
        return (THING_OTHER, UNKNOWN_THING_RADIUS);
    };
    (category_color(&row.category), row.radius)
}

/// The marker colour an editor category prefix selects.
fn category_color(category: &str) -> Rgba {
    if category.starts_with("Monsters") {
        return THING_MONSTER;
    }
    if category.starts_with("Player Starts") {
        return THING_PLAYER;
    }
    if category.starts_with("Weapons") || category.starts_with("Ammunition") {
        return THING_WEAPON;
    }
    if category.starts_with("Health") || category.starts_with("Powerups") {
        return THING_SUPPORT;
    }
    if category.starts_with("Keys") {
        return THING_KEY;
    }
    THING_OTHER
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
}

impl Scene<'_> {
    /// The sector the sidedef `side` opens into, `None` for an absent or
    /// dangling side.
    fn side_sector(&self, side: Option<usize>) -> Option<usize> {
        let sidedef = side.and_then(|index| self.map.sidedefs.get(index))?;
        self.map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
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

    /// The `Doom` automap colour of `line`: skipped for `dontdraw`, red
    /// for one-sided and secret lines, brown and yellow for height
    /// changes across two-sided ones, grey for everything else.
    fn automap_line_color(&self, line: &Linedef) -> Option<Rgba> {
        if line.flags & FLAG_DONT_DRAW != 0 {
            return None;
        }
        let across = if line.flags & FLAG_SECRET == 0 {
            line.back
        } else {
            None
        };
        let (Some(front), Some(back)) = (self.side_sector(line.front), self.side_sector(across))
        else {
            return Some(AUTOMAP_WALL);
        };
        let (Some(fs), Some(bs)) = (self.map.sectors.get(front), self.map.sectors.get(back)) else {
            return Some(AUTOMAP_WALL);
        };
        if fs.floor_height != bs.floor_height {
            return Some(AUTOMAP_FLOOR_CHANGE);
        }
        if fs.ceiling_height != bs.ceiling_height {
            return Some(AUTOMAP_CEILING_CHANGE);
        }
        Some(AUTOMAP_OTHER)
    }
}

/// Paints the whole top-down view: fills, lines, vertices, things.
fn draw_topdown(scene: &mut Scene<'_>, request: &Request) {
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

/// `render_topdown`' picture: the painted canvas and the `transform` to
/// report (the isometric projection returns `None`).
fn paint_map(map: &Map, defs: &Definitions, request: &Request) -> (Canvas, Option<View>) {
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
    };
    match request.projection {
        Projection::Topdown => draw_topdown(&mut scene, request),
        Projection::Isometric => draw_isometric(&mut scene, request),
    }
    (scene.canvas, reported.then_some(view))
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
/// for the isometric wireframe) and the published `PNG` asset.
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
    /// The published overview.
    asset: AssetRef,
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

/// `render_topdown`' work: resolve, paint, gamma, encode, publish.
///
/// # Errors
///
/// [`load_scene`] and [`resolve_request`]'s errors,
/// [`encode_png`]' and [`AppState::publish`]' failures.
fn topdown_report(
    state: &AppState,
    args: &RenderTopdownParams,
) -> Result<TopdownReport, CyberError> {
    let prepared = load_scene(state, args.map.as_deref())?;
    let request = resolve_request(&prepared.settings, args, &prepared.map)?;
    let (mut canvas, view) = paint_map(&prepared.map, &prepared.defs, &request);
    canvas.apply_gamma(&gamma_table(prepared.settings.gamma));
    let png = encode_png(&canvas.to_rgba_image(), None)?;
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
        transform: view.map(View::report),
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
/// World radius assumed for thing types the definitions do not know.
const UNKNOWN_THING_RADIUS: u32 = 16;
/// Smallest thing-marker radius, in pixels.
const MIN_THING_RADIUS: i64 = 2;
/// Extra length of a thing marker's facing tick, beyond its radius.
const TICK_PX: i64 = 4;
/// Radius of an isometric thing marker, in pixels.
const ISO_THING_RADIUS: i64 = 2;
/// `Doom`-family linedef flag bit: the automap must not draw the line.
const FLAG_DONT_DRAW: u32 = 128;
/// `Doom`-family linedef flag bit: the line marks a secret; the automap
/// draws it as a plain wall.
const FLAG_SECRET: u32 = 32;
/// Name suffix of the published overview files.
const OVERVIEW_SUFFIX: &str = "_topdown.png";
/// Media type of the published overview.
const PNG_MIME: &str = "image/png";

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
/// Automap: one-sided walls (and secret lines).
const AUTOMAP_WALL: Rgba = [252, 0, 0, 255];
/// Automap: a floor-height change across the line.
const AUTOMAP_FLOOR_CHANGE: Rgba = [188, 120, 72, 255];
/// Automap: a ceiling-height change across the line.
const AUTOMAP_CEILING_CHANGE: Rgba = [252, 252, 0, 255];
/// Automap: a flat two-sided line.
const AUTOMAP_OTHER: Rgba = [128, 128, 128, 255];
/// The `3x3` vertex marks.
const VERTEX_MARK: Rgba = [255, 208, 0, 255];
/// Thing markers: Monsters.
const THING_MONSTER: Rgba = [224, 64, 64, 255];
/// Thing markers: Player Starts.
const THING_PLAYER: Rgba = [64, 224, 64, 255];
/// Thing markers: Weapons and Ammunition.
const THING_WEAPON: Rgba = [224, 160, 64, 255];
/// Thing markers: Health And Armour and Powerups.
const THING_SUPPORT: Rgba = [64, 160, 224, 255];
/// Thing markers: Keys.
const THING_KEY: Rgba = [224, 224, 64, 255];
/// Thing markers: every other category, and unknown types.
const THING_OTHER: Rgba = [176, 176, 176, 255];

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
        description = "Changes the session's render options, applying only the fields you pass and keeping the rest; at least one field is required. lighting \"doom\" (sector light plus the COLORMAP distance fade) | \"flat\" (sector light only) | \"fullbright\"; show_things (thing markers in top-down renders and minimaps); show_sprites (sprites in the 3D view); sky (draw the sky texture, off means the background colour); fov (horizontal field of view, 10..=170 degrees); gamma (Doom gamma level, 0..=4); topdown_style \"editor\" | \"automap\"; background (\"#RRGGBB\"). All or nothing: a refused value reports invalid_argument listing the valid values and leaves every setting unchanged. Returns the full render settings."
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
        description = "Renders a map overview and publishes it as a PNG asset; returns {map, projection, width, height, transform, asset}. projection \"topdown\" (default) draws the flat north-up line map; projection \"isometric\" draws a 2:1 wireframe (screen = (x - y, (x + y) / 2 - z), floor edges per side, ceiling and verticals on one-sided lines, no hidden-line removal) and reports transform null. width/height default to the render settings. bounds {min_x, min_y, max_x, max_y} frames a world rectangle (default: the map bounds padded by 32 units; an empty map without bounds fails invalid_argument, as do min >= max). style \"editor\" colours one-sided #C0C0C0, two-sided #707070, special-bearing #6A8CFF; style \"automap\" draws the in-game map: one-sided (and secret) #FC0000, floor-height change #BC7848, ceiling-height change #FCFC00, flat two-sided #808080, dontdraw lines skipped. fill_sectors first fills each sector with a grey of half its light. show_vertices adds a 3x3 #FFD000 mark per vertex; show_things (default from the settings) marks each thing with a circle of its definition radius (unknown types: 16) in its category colour and a facing tick. highlight {linedefs, sectors, things, vertices} draws those elements as highlights (red 3px lines, a red sector tint, white thing outlines); an index past the end of its table fails not_found. For topdown, transform carries {scale_num, scale_den, offset_x, offset_y} with px = offset_x + x*scale_num/scale_den and py = offset_y - y*scale_num/scale_den (both floor-rounded): convert a pixel back with x = (px - offset_x)*scale_den/scale_num and y = (offset_y - py)*scale_den/scale_num. The gamma render setting is applied to the output."
    )]
    fn render_topdown(
        &self,
        Parameters(args): Parameters<RenderTopdownParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match topdown_report(self.state(), &args) {
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
        BoundsParams, HighlightParams, Highlights, Plane, Projection, RenderTopdownParams, Request,
        Scene, background_pixel, bounds_plane, category_color, editor_line_color, fit, height_span,
        iso_plane, light_grey, paint_map, parse_projection, resolve_bounds, resolve_highlight,
        resolve_request, scale_down, thing_visual,
    };
    use crate::defs::definitions_for;
    use crate::render::{Canvas, Rgba, Switch};
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
        let bounds = resolve_bounds(None, map).expect("the fixture map bounds itself");
        let view = fit(&bounds_plane(&bounds), 64, 64);
        let scene = Scene {
            map,
            defs: &defs,
            view,
            canvas: Canvas::new(8, 8, [0_u8, 0, 0, 255]),
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
            bounds: resolve_bounds(None, map).expect("the fixture map bounds itself"),
            background: [0_u8, 0, 0, 255],
            show_things: Switch::new(true),
            show_vertices: Switch::new(false),
            fill_sectors: Switch::new(false),
            highlight: Highlights::default(),
        }
    }

    /// Paint the `map` and read one pixel of the result.
    fn pixel_of(map: &Map, request: &Request, x: u32, y: u32) -> Option<Rgba> {
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let (canvas, _) = paint_map(map, &defs, request);
        canvas.pixel(x, y)
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
        let padded = resolve_bounds(None, &map).expect("the default frames the map");
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
        let error = resolve_bounds(Some(&flat), &map).expect_err("min_x == max_x");
        assert_eq!(error.code(), "invalid_argument", "flat bounds code");
        assert!(
            error.to_string().contains("min_x < max_x"),
            "the message states the rule: {error}"
        );
        let empty = Map::default();
        let missing = resolve_bounds(None, &empty).expect_err("nothing to frame");
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
        let bounds = resolve_bounds(None, &map).expect("the default bounds");
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
        let (canvas, view) = paint_map(&map, &defs, &request);
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
        let request = resolve_request(&settings, &plain, &map).expect("defaults resolve");
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
        let overridden = resolve_request(&settings, &over, &map).expect("valid overrides");
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
        let size_error = resolve_request(&settings, &bad_size, &map).expect_err("below the floor");
        assert_eq!(size_error.code(), "invalid_argument", "tiny width code");
        let bad_style: RenderTopdownParams =
            serde_json::from_value(json!({ "style": "wireframe" })).expect("one field");
        let style_error = resolve_request(&settings, &bad_style, &map).expect_err("not a style");
        assert_eq!(style_error.code(), "invalid_argument", "bad style code");
    }
}
