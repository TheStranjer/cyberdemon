//! `render_topdown`' argument resolution: [`parse_projection`] reads the
//! call's projection once and [`check_overlays`] judges it before anything
//! is fetched, [`resolve_request`] folds the bounds, highlight and overlay
//! parsing into a [`Request`] over that already-settled [`Call`], and
//! [`load_underlay`] is the fetch-and-decode that runs before any session
//! lock, beside the range checks behind them.

use crate::error::CyberError;
use crate::gfx::codec::decode_image;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::parse_hex_colour;
use crate::plan::Units;
use crate::render::{RenderSettings, Rgba, Switch};
use crate::state::AppState;
use crate::tools::maps::params::{ElementKind, check_index};
use crate::tools::plan::params::UnitsParam;
use crate::wad::map::Map;

use super::MAX_OPACITY;
use super::params::{
    Annotation, AnnotationParams, Bounds, BoundsParams, Call, HighlightParams, Highlights, Labels,
    LabelsParams, Projection, Rect, RenderTopdownParams, Request, Underlay, UnderlayParams,
};

/// World units of slack added around the default map bounds.
const BOUNDS_PAD: i64 = 32;
/// Smallest `grid` spacing, in map units.
const MIN_GRID: i64 = 8;
/// Largest `grid` spacing, in map units.
const MAX_GRID: i64 = 1_024;
/// The `annotations` default colour.
const ANNOTATION_COLOUR: Rgba = [255, 255, 255, 255];
/// The `underlay.opacity` default, percent.
const DEFAULT_OPACITY: u8 = 50;
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

/// Parses `projection`, case-insensitively; absent means top-down.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the two projections.
pub(super) fn parse_projection(text: Option<&str>) -> Result<Projection, CyberError> {
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
pub(super) fn resolve_bounds(
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
pub(super) fn check_overlays(
    args: &RenderTopdownParams,
    projection: Projection,
) -> Result<(), CyberError> {
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
pub(super) fn resolve_grid(requested: Option<i64>) -> Result<Option<i64>, CyberError> {
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
pub(super) fn resolve_labels(requested: Option<&LabelsParams>) -> Labels {
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
pub(super) fn resolve_label_scale(requested: Option<u32>, width: u32) -> Result<u32, CyberError> {
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
/// [`parse_hex_colour`] refuses.
pub(super) fn resolve_annotations(
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
        Some(text) => parse_hex_colour(text, true)?,
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
pub(super) fn resolve_opacity(requested: Option<u8>) -> Result<u8, CyberError> {
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
pub(super) fn to_overlay_units(requested: Option<UnitsParam>) -> Units {
    requested.unwrap_or_default().to_units(1_i64)
}

/// The enclosing map-space rectangle of an `image` under `units`: its four
/// corners through [`Units::to_map`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a corner leaves the supported
/// range under the transform.
pub(super) fn underlay_rect(units: &Units, image: &RgbaImage) -> Result<Rect, CyberError> {
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
pub(super) async fn load_underlay(
    state: &AppState,
    args: &UnderlayParams,
) -> Result<Underlay, CyberError> {
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
pub(super) fn resolve_highlight(
    requested: Option<&HighlightParams>,
    map: &Map,
) -> Result<Highlights, CyberError> {
    let absent = HighlightParams::default();
    let wanted = requested.unwrap_or(&absent);
    Ok(Highlights {
        linedefs: checked_indices(
            ElementKind::Linedefs,
            wanted.linedefs.as_deref(),
            map.linedefs.len(),
            map,
        )?,
        sectors: checked_indices(
            ElementKind::Sectors,
            wanted.sectors.as_deref(),
            map.sectors.len(),
            map,
        )?,
        things: checked_indices(
            ElementKind::Things,
            wanted.things.as_deref(),
            map.things.len(),
            map,
        )?,
        vertices: checked_indices(
            ElementKind::Vertices,
            wanted.vertices.as_deref(),
            map.vertices.len(),
            map,
        )?,
    })
}

/// `wanted` when every index names an element of `len`, else the refusal:
/// the shared [`check_index`] message with the map and the parameter as
/// context.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the index and the element count.
///
/// [`check_index`]: crate::tools::maps::params::check_index
fn checked_indices(
    kind: ElementKind,
    wanted: Option<&[usize]>,
    len: usize,
    map: &Map,
) -> Result<Vec<usize>, CyberError> {
    let Some(indices) = wanted else {
        return Ok(Vec::new());
    };
    for index in indices {
        check_index(kind, *index, len).map_err(|error| {
            error.map_message(|message| format!("highlight on map {}: {message}", map.name))
        })?;
    }
    Ok(indices.to_vec())
}

/// The call's resolved arguments: the already-parsed projection from the
/// [`Call`], resolution and style through the render settings' own
/// validators, bounds and highlights against the map, and the overlay
/// parameters checked and normalised. The caller parses `projection` and
/// runs [`check_overlays`] on it once before fetching or loading anything;
/// this folds the rest.
///
/// # Errors
///
/// [`RenderSettings::set_resolution`], [`RenderSettings::set_topdown_style`],
/// [`resolve_bounds`] and [`resolve_highlight`]'s errors, plus the
/// [`resolve_grid`], [`resolve_label_scale`] and [`resolve_annotations`]
/// refusals.
pub(super) fn resolve_request(
    settings: &RenderSettings,
    call: &Call<'_>,
    map: &Map,
    underlay: Option<Underlay>,
) -> Result<Request, CyberError> {
    let args = call.args;
    let projection = call.projection;
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
        background: settings.background,
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
