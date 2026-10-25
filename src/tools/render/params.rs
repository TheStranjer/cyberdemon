//! `render_topdown`' wire and resolved types: the `Projection` a call
//! picks, the parameter structs the router deserialises, and the
//! [`Request`] with its parts that [`super::resolve`] builds and
//! [`super::paint`] and [`super::marks`] draw from.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::gfx::image::RgbaImage;
use crate::plan::Units;
use crate::render::{Rgba, Switch, TopdownStyle};
use crate::tools::plan::params::UnitsParam;

/// The projection one [`Cyberdemon::render_topdown`] call draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Projection {
    /// The flat north-up overview: the default.
    Topdown,
    /// The `2:1` wireframe on the plane `(x - y, (x + y) / 2 - z)`.
    Isometric,
}

impl Projection {
    /// The `projection` result field and accepted parameter name.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Topdown => "topdown",
            Self::Isometric => "isometric",
        }
    }
}

/// Arguments for [`Cyberdemon::render_topdown`]. Every field is optional;
/// the defaults come from the session's render settings.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenderTopdownParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Output width in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    pub(super) width: Option<u32>,
    /// Output height in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    pub(super) height: Option<u32>,
    /// `"topdown"` (the default) or `"isometric"`.
    #[serde(default)]
    pub(super) projection: Option<String>,
    /// The world rectangle to frame; defaults to the map bounds padded by
    /// 32 units. Needs `min_x < max_x` and `min_y < max_y`.
    #[serde(default)]
    pub(super) bounds: Option<BoundsParams>,
    /// Draw thing markers (default: the `show_things` render setting).
    #[serde(default)]
    pub(super) show_things: Option<bool>,
    /// Draw a `3x3` mark per vertex (default `false`).
    #[serde(default)]
    pub(super) show_vertices: Option<bool>,
    /// Fill each sector with a grey of half its light level under the
    /// lines (default `false`).
    #[serde(default)]
    pub(super) fill_sectors: Option<bool>,
    /// Line colours: `"editor"` or `"automap"` (default: the
    /// `topdown_style` render setting).
    #[serde(default)]
    pub(super) style: Option<String>,
    /// Element indices to draw as highlights.
    #[serde(default)]
    pub(super) highlight: Option<HighlightParams>,
    /// Reference image drawn under the map (top-down projection only).
    #[serde(default)]
    pub(super) underlay: Option<UnderlayParams>,
    /// Grid spacing in map units, `8..=1024`, drawn under the map lines
    /// (top-down projection only).
    #[serde(default)]
    pub(super) grid: Option<i64>,
    /// Which element indices to draw as text over the map (top-down
    /// projection only).
    #[serde(default)]
    pub(super) labels: Option<LabelsParams>,
    /// Free text at map coordinates drawn over the map (top-down
    /// projection only). At most [`MAX_ANNOTATIONS`] rows of at most
    /// [`MAX_ANNOTATION_CHARS`] characters each.
    #[serde(default)]
    pub(super) annotations: Option<Vec<AnnotationParams>>,
    /// Magnification of every label and annotation glyph, `1..=4`
    /// (default `2` when the output width is at least 1024 pixels, else `1`).
    #[serde(default)]
    pub(super) label_scale: Option<u32>,
}

/// Arguments for `render_topdown`' `underlay` parameter: the reference
/// picture to draw under the map. It is sampled only within its own edges,
/// so it is drawn once and never repeated past one: output pixels clear of
/// its map rectangle keep the background.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UnderlayParams {
    /// File to draw: absolute path, `file://` URI, `http(s)://` `URL` or
    /// this server's asset `URL`.
    pub(super) source: String,
    /// How image pixels map to map units — exactly the object
    /// `build_map_from_plan` takes as `units` (no `grid`: the underlay
    /// never snaps). Default identity: `scale_num: 1`, `scale_den: 1`,
    /// origins `0`, `flip_y: false`.
    #[serde(default)]
    pub(super) units: Option<UnitsParam>,
    /// Blend strength in percent, `0..=100` (default `50`); each texel's
    /// own alpha is scaled by this.
    #[serde(default)]
    pub(super) opacity: Option<u8>,
}

/// Arguments for `render_topdown`' `labels` parameter: which element
/// indices to draw.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct LabelsParams {
    /// Draw each sector's index inside it.
    #[serde(default)]
    pub(super) sectors: Option<bool>,
    /// Draw each linedef's index at its midpoint.
    #[serde(default)]
    pub(super) linedefs: Option<bool>,
    /// Draw each thing's index beside its marker.
    #[serde(default)]
    pub(super) things: Option<bool>,
    /// Draw each vertex's index beside its mark.
    #[serde(default)]
    pub(super) vertices: Option<bool>,
}

/// Arguments for one `render_topdown`' `annotations` row.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AnnotationParams {
    /// World `x` of the text anchor.
    pub(super) x: i64,
    /// World `y` of the text anchor.
    pub(super) y: i64,
    /// The text; upper-cased, characters outside the font draw as `?`,
    /// at most [`MAX_ANNOTATION_CHARS`] characters.
    pub(super) text: String,
    /// Text colour as `"#RRGGBB"` (default `"#FFFFFF"`).
    #[serde(default)]
    pub(super) color: Option<String>,
}

/// Arguments for `render_topdown`'s `bounds` parameter: the world
/// rectangle to frame.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BoundsParams {
    /// World `x` of the west edge.
    pub(super) min_x: i64,
    /// World `y` of the south edge.
    pub(super) min_y: i64,
    /// World `x` of the east edge.
    pub(super) max_x: i64,
    /// World `y` of the north edge.
    pub(super) max_y: i64,
}

/// Arguments for `render_topdown`'s `highlight` parameter.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct HighlightParams {
    /// Linedef indices to draw red and thick.
    #[serde(default)]
    pub(super) linedefs: Option<Vec<usize>>,
    /// Sector indices to tint red.
    #[serde(default)]
    pub(super) sectors: Option<Vec<usize>>,
    /// Thing indices to ring in white.
    #[serde(default)]
    pub(super) things: Option<Vec<usize>>,
    /// Vertex indices to draw red.
    #[serde(default)]
    pub(super) vertices: Option<Vec<usize>>,
}

/// The decoded [`UnderlayParams`]: the picture, its map placement and the
/// blend strength it draws at.
#[derive(Debug)]
pub(super) struct Underlay {
    /// The decoded reference picture.
    pub(super) image: RgbaImage,
    /// Image-pixel to map-unit transform, unsnapped
    /// ([`grid`](Units::grid) `1`).
    pub(super) units: Units,
    /// Blend strength in percent, `0..=100`.
    pub(super) opacity: u8,
    /// The underlay's map-space rectangle: the four image corners
    /// through [`Units::to_map`].
    pub(super) rect: Rect,
}

/// Which element indices [`render_topdown`](Cyberdemon::render_topdown)
/// draws as text.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Labels {
    /// Sector indices.
    pub(super) sectors: Switch,
    /// Linedef indices.
    pub(super) linedefs: Switch,
    /// Thing indices.
    pub(super) things: Switch,
    /// Vertex indices.
    pub(super) vertices: Switch,
}

impl Labels {
    /// Whether any label pass has to run.
    pub(super) const fn any(self) -> bool {
        self.sectors.is_on()
            || self.linedefs.is_on()
            || self.things.is_on()
            || self.vertices.is_on()
    }
}

/// One resolved `annotations` row.
#[derive(Debug)]
pub(super) struct Annotation {
    /// World `x` of the text anchor.
    pub(super) x: i64,
    /// World `y` of the text anchor.
    pub(super) y: i64,
    /// The upper-cased text.
    pub(super) text: String,
    /// The text colour.
    pub(super) colour: Rgba,
}

/// A world rectangle as `(min_x, min_y, max_x, max_y)`; [`union_rect`]
/// folds these.
pub(super) type Rect = (i64, i64, i64, i64);

/// The world rectangle a render frames; validation guarantees strictly
/// positive spans.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    /// West edge.
    pub(super) min_x: i64,
    /// South edge.
    pub(super) min_y: i64,
    /// East edge.
    pub(super) max_x: i64,
    /// North edge.
    pub(super) max_y: i64,
}

/// Resolved highlight lists, already checked against the map.
#[derive(Debug, Default)]
pub(super) struct Highlights {
    /// Linedef indices to draw red and thick.
    pub(super) linedefs: Vec<usize>,
    /// Sector indices to tint red.
    pub(super) sectors: Vec<usize>,
    /// Thing indices to ring in white.
    pub(super) things: Vec<usize>,
    /// Vertex indices to draw red.
    pub(super) vertices: Vec<usize>,
}

/// One [`Cyberdemon::render_topdown`] call's fully resolved arguments.
#[derive(Debug)]
pub(super) struct Request {
    /// Which projection to draw.
    pub(super) projection: Projection,
    /// Output width in pixels.
    pub(super) width: u32,
    /// Output height in pixels.
    pub(super) height: u32,
    /// Line-colour style (top-down renders).
    pub(super) style: TopdownStyle,
    /// The world rectangle to frame.
    pub(super) bounds: Bounds,
    /// Background pixel colour from the render settings.
    pub(super) background: Rgba,
    /// Thing-marker switch.
    pub(super) show_things: Switch,
    /// Vertex-mark switch.
    pub(super) show_vertices: Switch,
    /// Sector-fill switch.
    pub(super) fill_sectors: Switch,
    /// Highlighted elements.
    pub(super) highlight: Highlights,
    /// The reference picture drawn under the map, when given.
    pub(super) underlay: Option<Underlay>,
    /// Grid spacing in map units, when given.
    pub(super) grid: Option<i64>,
    /// Which element indices draw their numbers.
    pub(super) labels: Labels,
    /// Free texts standing at map coordinates.
    pub(super) annotations: Vec<Annotation>,
    /// Magnification of every label and annotation glyph.
    pub(super) label_scale: u32,
}
