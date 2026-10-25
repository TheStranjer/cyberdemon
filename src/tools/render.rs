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
//! [`crate::plan::Units`] under the map
//! ([`resolve::load_underlay`] runs before any session lock), a
//! coordinate grid, element-index labels and free
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
//!
//! The work is split by stage: [`params`] carries the `render_topdown`
//! wire and resolved types, [`frame`] the plane-to-pixel mapping,
//! [`resolve`] the argument resolution, [`paint`] and [`marks`] the
//! drawing, [`topdown`] the overview run, [`minimap`] and [`view`] the
//! other two render tools. This root keeps the settings tools, the
//! constants the submodules share and the router; the group's unit tests
//! live in `render/tests.rs`.

mod frame;
mod marks;
mod minimap;
mod paint;
mod params;
mod resolve;
mod topdown;
mod view;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::render::{RenderSettings, Rgba, Switch};
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::render::minimap::{RenderMinimapParams, minimap_report};
use crate::tools::render::params::RenderTopdownParams;
use crate::tools::render::topdown::topdown_report;
use crate::tools::render::view::{RenderViewParams, view_report};
use crate::tools::{fail, ok};

// The marks and the overview run the `playability` group drives, kept at
// the paths it already imports; both live in `topdown`.
pub(in crate::tools) use crate::tools::render::topdown::{OverviewMarks, playability_overview};

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

/// Stroke width of highlighted linedefs.
pub(super) const HIGHLIGHT_WIDTH: u32 = 3;
/// Media type of the published overview.
pub(super) const PNG_MIME: &str = "image/png";
/// Editor style: a one-sided wall.
pub(super) const LINE_ONE_SIDED: Rgba = [192, 192, 192, 255];
/// Editor style: a two-sided line.
pub(super) const LINE_TWO_SIDED: Rgba = [112, 112, 112, 255];
/// Highlighted linedefs and vertices.
pub(super) const HIGHLIGHT_RED: Rgba = [255, 64, 64, 255];
/// The largest `underlay.opacity`, percent.
pub(super) const MAX_OPACITY: u8 = 100;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = render_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Sets the resolution the render tools default to.
    #[tool(
        description = "Sets the pixel size the render tools default to: width and height, each 16..=4096, with the product at most 16777216. Returns both values. Connection-level like every render setting: it belongs to no WAD and survives archive changes and undo."
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
        description = "Changes the session's render options, applying only the fields you pass (at least one is required): lighting (doom or flat), show_things, show_sprites, sky, fov, gamma, topdown_style and background, each validated against its range. These are the defaults render_view, render_topdown and render_minimap use when a call leaves them out."
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
        description = "Returns the session's render settings as one object: width, height, lighting, show_things, show_sprites, sky, fov, gamma, topdown_style and background. They belong to the connection: new_wad, open_wad, merge_wad and undo never change them and no archive holds them."
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
        description = "Renders a map overview and publishes it as a PNG asset URL: projection doom draws automap-coloured linedefs and thing markers, isometric a tilted take; centre on at/radius or the whole map, highlights draw problem elements, show_things toggles markers, and underlay: {url, units, opacity} puts a sketch picture behind the lines for calibration. Use it to look after every meaningful change."
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
        description = "Renders a compact automap-style square around a centre point (default the player 1 start) with a radius, and publishes it as a PNG asset: {map, size, center, asset}. Use it for quick orientation between edits; render_topdown is the full overview worth judging in detail."
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
        description = "Renders a first-person 3D view from a camera inside the map and publishes it as a PNG: an integer column caster with walls, textured planes, pegged faces, sky and depth-tested sprites. The camera defaults to the player 1 start; pass x, y, angle, height and a fov when you need another. Use it to see what you built the way a player will."
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
mod tests;
