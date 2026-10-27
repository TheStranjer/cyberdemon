//! The `render_topdown` run: [`load_scene`] reads the map, definitions and
//! settings under the session lock, [`paint_map`] fits and paints the
//! picture ([`super::paint`] and [`super::marks`] do the strokes), and
//! [`topdown_report`] gamma-corrects, encodes and publishes the `PNG`
//! with the fitted `transform`. [`playability_overview`] runs the same
//! machinery for [`Cyberdemon::check_playability`]'s verification frame.

use serde::Serialize;

use crate::assets::server::AssetRef;
use crate::defs::Definitions;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::render::{Canvas, RenderSettings, gamma_table};
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::maps::common;
use crate::wad::map::Map;

use super::PNG_MIME;
use super::frame::{View, bounds_plane, fit, height_span, iso_plane};
use super::paint::{Scene, draw_isometric, draw_topdown};
use super::params::{
    Call, HighlightParams, LabelsParams, Projection, RenderTopdownParams, Request, Underlay,
};
use super::resolve::{check_overlays, load_underlay, parse_projection, resolve_request};

/// Name suffix of the published overview files.
const OVERVIEW_SUFFIX: &str = "_topdown.png";

/// The finished pixels of one `render_topdown`: the canvas, the `transform`
/// to report (the isometric projection returns `None`) and the notes.
pub(super) struct Painted {
    /// The pixels.
    pub(super) canvas: Canvas,
    /// The fitted top-down mapping, `None` for isometric.
    pub(super) view: Option<View>,
    /// What the render skipped or adjusted, for the result's `notes`.
    pub(super) notes: Vec<String>,
}

/// Paints a whole `render_topdown` picture in the documented order.
pub(super) fn paint_map(map: &Map, defs: &Definitions, request: &Request) -> Painted {
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
///
/// The `playability` group reads its own through `playability`'s `load`
/// and hands it to [`playability_overview`], so an overview is painted
/// from the very inputs the analysis walked — one lock, one revision.
#[derive(Debug)]
pub(in crate::tools) struct Prepared {
    /// The map to render, from the merged view.
    pub(in crate::tools) map: Map,
    /// The session's thing definitions.
    pub(in crate::tools) defs: Definitions,
    /// The session's render settings.
    pub(in crate::tools) settings: RenderSettings,
}

/// [`Cyberdemon::render_topdown`]'s result: the fitted `transform` (null
/// for the isometric wireframe), the underlay as placed (null without
/// one), the notes and the published `PNG` asset.
#[derive(Serialize)]
pub(super) struct TopdownReport {
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
pub(super) struct UnderlayReport {
    /// The decoded picture's width in image pixels.
    width: u32,
    /// The decoded picture's height in image pixels.
    height: u32,
    /// The map-space rectangle the picture covers.
    map_bounds: BoundsReport,
}

/// A `(min_x, min_y, max_x, max_y)` world rectangle.
#[derive(Serialize)]
pub(super) struct BoundsReport {
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
pub(super) struct TransformReport {
    /// Pixel span of the fitted axis.
    pub(super) scale_num: u64,
    /// World span of the fitted axis (`>= 1`).
    pub(super) scale_den: u64,
    /// Pixel column of world `x = 0`.
    pub(super) offset_x: i64,
    /// Pixel row of world `y = 0`.
    pub(super) offset_y: i64,
}

/// Reads the render's inputs under the session lock.
///
/// # Errors
///
/// [`common::resolve_map_name`] and [`common::load_map`]'s errors, and the
/// definition-collection errors of [`crate::session::Session::definitions`].
fn load_scene(state: &AppState, requested: Option<&str>) -> Result<Prepared, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, requested)?;
        let map = common::load_map(session, name, Scope::Merged)?;
        let defs = session.definitions()?;
        Ok(Prepared {
            map,
            defs,
            settings: session.render_settings().clone(),
        })
    })
}

/// `render_topdown`' work: parse the projection and check the overlays
/// once up front, fetch and decode the underlay before any lock, then
/// resolve, paint, gamma, encode, publish.
///
/// # Errors
///
/// [`load_scene`] and [`resolve_request`]'s errors, the
/// [`parse_projection`] and [`check_overlays`] refusals, [`load_underlay`]'s
/// fetch, decode and transform refusals, [`encode_png`]' and
/// [`AppState::publish`]' failures.
pub(super) async fn topdown_report(
    state: &AppState,
    args: &RenderTopdownParams,
) -> Result<TopdownReport, CyberError> {
    let projection = parse_projection(args.projection.as_deref())?;
    check_overlays(args, projection)?;
    let underlay = match args.underlay.as_ref() {
        Some(wanted) => Some(load_underlay(state, wanted).await?),
        None => None,
    };
    let prepared = load_scene(state, args.map.as_deref())?;
    let call = Call { args, projection };
    published_report(state, &prepared, &call, underlay)
}

/// The lock-free tail of one `render_topdown`: resolve the request against
/// the already-loaded `prepared` inputs, paint, gamma-correct, encode and
/// publish. Shared by [`topdown_report`] and [`playability_overview`] so
/// neither paints a different revision than it resolved. The `call`'s
/// projection is parsed once by the caller, never re-read here.
fn published_report(
    state: &AppState,
    prepared: &Prepared,
    call: &Call<'_>,
    underlay: Option<Underlay>,
) -> Result<TopdownReport, CyberError> {
    let request = resolve_request(&prepared.settings, call, &prepared.map, underlay)?;
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
/// [`Cyberdemon::render_topdown`] run over the map, definitions and
/// settings already held in `prepared`, with `marks` highlighted, thing
/// markers and sector fills on and sector labels on, published as a
/// `PNG` asset. No session lock is taken again: the picture is the very
/// revision the caller analysed.
///
/// # Errors
///
/// [`resolve_request`]'s errors plus [`encode_png`]' and
/// [`AppState::publish`]' failures — the standalone render's, minus its
/// map resolution and its projection parsing, since this call is always
/// the top-down one.
pub(in crate::tools) async fn playability_overview(
    state: &AppState,
    prepared: &Prepared,
    marks: OverviewMarks,
) -> Result<AssetRef, CyberError> {
    let args = RenderTopdownParams {
        map: None,
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
    let call = Call {
        args: &args,
        projection: Projection::Topdown,
    };
    Ok(published_report(state, prepared, &call, None)?.asset)
}
