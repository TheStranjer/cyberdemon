//! Render tools: the session-level defaults the rendering tools use.
//!
//! Three tools, no rendering yet: [`Cyberdemon::set_render_resolution`] and
//! [`Cyberdemon::set_render_options`] change
//! [`crate::render::RenderSettings`] on the session,
//! [`Cyberdemon::get_render_settings`] reports them. Every change is
//! validated before it lands and every result is the full settings, so a
//! client never has to remember what it set earlier.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::render::{RenderSettings, Switch};
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::{fail, ok};

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
}
