//! Session render settings: the defaults every rendering tool starts from.
//!
//! [`RenderSettings`] is plain data with validated mutators: the resolution
//! ([`RenderSettings::set_resolution`]), the lighting model, the thing and
//! sprite markers, the sky, the field of view, the gamma level, the top-down
//! style and the background colour. It lives on
//! [`crate::session::Session`], is never part of the undo history and is not
//! reset by opening or replacing archives, so a client sets it once per
//! connection and the render tools read it as their defaults.
//!
//! The names of the enum-valued options are matched case-insensitively and
//! every refusal is an [`CyberError::InvalidArgument`] naming the accepted
//! values, so a client can correct a call from the message alone.

use serde::Serialize;

use crate::error::CyberError;

/// Smallest edge, in pixels, a render resolution accepts.
pub const MIN_DIMENSION: u32 = 16;

/// Largest edge, in pixels, a render resolution accepts.
pub const MAX_DIMENSION: u32 = 4_096;

/// Largest total pixel count a render resolution accepts: the whole
/// [`MAX_DIMENSION`] square.
pub const MAX_PIXELS: u64 = 4_096 * 4_096;

/// Narrowest horizontal field of view, in degrees.
pub const MIN_FOV: u32 = 10;

/// Widest horizontal field of view, in degrees.
pub const MAX_FOV: u32 = 170;

/// Highest gamma level: Doom's own gamma slider runs `0..=4`.
pub const MAX_GAMMA: u8 = 4;

/// Default render width, in pixels: the vanilla 640.
const DEFAULT_WIDTH: u32 = 640;

/// Default render height, in pixels: the vanilla 400.
const DEFAULT_HEIGHT: u32 = 400;

/// Default horizontal field of view, in degrees: Doom's own view angle.
const DEFAULT_FOV: u32 = 90;

/// Default background colour: black, as an unlit void reads on screen.
const DEFAULT_BACKGROUND: &str = "#000000";

/// Number of hex digits after the `#` in a background colour.
const HEX_DIGITS: usize = 6;

/// How a 3D view is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lighting {
    /// Sector light plus the `COLORMAP` distance fade: Doom's own look.
    #[default]
    Doom,
    /// Sector light only: no distance fade, so depth reads flat.
    Flat,
    /// Every surface at full brightness, whatever its sector light.
    Fullbright,
}

/// How a top-down render draws the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TopdownStyle {
    /// Editor look: lines and fills by lump kind.
    #[default]
    Editor,
    /// Automap look: the in-game red-on-black line map.
    Automap,
}

/// A render switch: on draws what it controls, off does not.
///
/// A wrapper over `bool` with a name, so the settings struct says what each
/// switch means wherever it is passed and a setter cannot take two switches
/// by accident. It serializes transparently as a `JSON` boolean and converts
/// from a plain `bool`, so neither a client nor a caller has to build one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Switch(bool);

impl Switch {
    /// The switch in position `value`.
    #[must_use]
    #[inline]
    pub const fn new(value: bool) -> Self {
        Self(value)
    }

    /// Whether what this switch controls is drawn.
    #[must_use]
    #[inline]
    pub const fn is_on(self) -> bool {
        self.0
    }
}

impl From<bool> for Switch {
    #[inline]
    fn from(value: bool) -> Self {
        Self(value)
    }
}

/// The session-level defaults the rendering tools use.
///
/// Build one with [`RenderSettings::default`], change it through the
/// validating setters (each refuses values outside the documented ranges and
/// leaves the settings untouched when it does) and read the fields directly.
/// The three [`Switch`] fields are the render's on/off overlays and
/// serialize as plain JSON booleans.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RenderSettings {
    /// Output width in pixels, `MIN_DIMENSION..=MAX_DIMENSION`.
    pub width: u32,
    /// Output height in pixels, `MIN_DIMENSION..=MAX_DIMENSION`, with
    /// `width * height` at most [`MAX_PIXELS`].
    pub height: u32,
    /// How a 3D view is lit; see [`Lighting`].
    pub lighting: Lighting,
    /// Whether top-down renders and minimaps mark thing lumps.
    pub show_things: Switch,
    /// Whether 3D views draw sprites for things.
    pub show_sprites: Switch,
    /// Whether 3D views draw the sky texture; off means the plain
    /// [`RenderSettings::background`] colour instead.
    pub sky: Switch,
    /// Horizontal field of view in degrees, `MIN_FOV..=MAX_FOV`.
    pub fov: u32,
    /// Gamma level applied to every output channel, `0..=MAX_GAMMA`
    /// (see [`crate::render::gamma_table`]).
    pub gamma: u8,
    /// How top-down renders draw the map; see [`TopdownStyle`].
    pub topdown_style: TopdownStyle,
    /// Background colour as `#RRGGBB` hex: the void outside the map, and the
    /// sky colour when [`RenderSettings::sky`] is off.
    pub background: String,
}

impl Default for RenderSettings {
    /// The vanilla-shaped starting point: a 640x400 `Doom`-lit view with a
    /// 90 degree field of view, markers and sky on, no gamma and a black
    /// background.
    #[inline]
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            lighting: Lighting::Doom,
            show_things: Switch::new(true),
            show_sprites: Switch::new(true),
            sky: Switch::new(true),
            fov: DEFAULT_FOV,
            gamma: 0_u8,
            topdown_style: TopdownStyle::Editor,
            background: DEFAULT_BACKGROUND.to_owned(),
        }
    }
}

impl RenderSettings {
    /// Sets the output resolution, refusing a total over [`MAX_PIXELS`] or an
    /// edge outside `MIN_DIMENSION..=MAX_DIMENSION`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the range or the cap when the
    /// resolution is refused; the settings keep the resolution they had.
    #[inline]
    pub fn set_resolution(&mut self, width: u32, height: u32) -> Result<(), CyberError> {
        check_pixel_budget(width, height)?;
        check_dimension("width", width)?;
        check_dimension("height", height)?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Sets the lighting model from its name, matched case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] listing the accepted names.
    #[inline]
    pub fn set_lighting(&mut self, name: &str) -> Result<(), CyberError> {
        self.lighting = parse_lighting(name)?;
        Ok(())
    }

    /// Sets the top-down style from its name, matched case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] listing the accepted names.
    #[inline]
    pub fn set_topdown_style(&mut self, name: &str) -> Result<(), CyberError> {
        self.topdown_style = parse_topdown_style(name)?;
        Ok(())
    }

    /// Sets the horizontal field of view in degrees.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the range.
    #[inline]
    pub fn set_fov(&mut self, degrees: u32) -> Result<(), CyberError> {
        check_fov(degrees)?;
        self.fov = degrees;
        Ok(())
    }

    /// Sets the gamma level.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the range.
    #[inline]
    pub fn set_gamma(&mut self, level: u8) -> Result<(), CyberError> {
        check_gamma(level)?;
        self.gamma = level;
        Ok(())
    }

    /// Sets the background colour from a `#RRGGBB` hex string, either case.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the required shape.
    #[inline]
    pub fn set_background(&mut self, color: &str) -> Result<(), CyberError> {
        check_background(color)?;
        color.clone_into(&mut self.background);
        Ok(())
    }
}

/// Refuses an image edge outside `MIN_DIMENSION..=MAX_DIMENSION`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `name` and the range.
fn check_dimension(name: &str, value: u32) -> Result<(), CyberError> {
    if (MIN_DIMENSION..=MAX_DIMENSION).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render {name} must be within {MIN_DIMENSION}..={MAX_DIMENSION} pixels, got {value}"
    )))
}

/// Refuses a resolution whose total pixel count exceeds [`MAX_PIXELS`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the cap and what to lower.
fn check_pixel_budget(width: u32, height: u32) -> Result<(), CyberError> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels <= MAX_PIXELS {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render resolution {width}x{height} is {pixels} pixels, over the {MAX_PIXELS} cap: lower \
         width or height"
    )))
}

/// Refuses a field of view outside `MIN_FOV..=MAX_FOV` degrees.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the range.
fn check_fov(degrees: u32) -> Result<(), CyberError> {
    if (MIN_FOV..=MAX_FOV).contains(&degrees) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render fov must be within {MIN_FOV}..={MAX_FOV} degrees, got {degrees}"
    )))
}

/// Refuses a gamma level outside `0..=MAX_GAMMA`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the range.
fn check_gamma(level: u8) -> Result<(), CyberError> {
    if level <= MAX_GAMMA {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render gamma must be within 0..={MAX_GAMMA}, got {level}"
    )))
}

/// Refuses a background colour that is not `#` plus [`HEX_DIGITS`] hex
/// digits.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] showing the required shape.
fn check_background(color: &str) -> Result<(), CyberError> {
    let digits = color.strip_prefix('#').unwrap_or_default();
    if digits.len() == HEX_DIGITS && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "render background must be a \"#RRGGBB\" hex colour such as \"#000000\", got {color:?}"
    )))
}

/// The lighting model `name` says, case-insensitively.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the three models.
fn parse_lighting(name: &str) -> Result<Lighting, CyberError> {
    let lowered = name.to_ascii_lowercase();
    if lowered == "doom" {
        return Ok(Lighting::Doom);
    }
    if lowered == "flat" {
        return Ok(Lighting::Flat);
    }
    if lowered == "fullbright" {
        return Ok(Lighting::Fullbright);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown lighting {name:?}: use \"doom\" for sector light plus the COLORMAP distance \
         fade, \"flat\" for sector light only, or \"fullbright\" to light everything fully"
    )))
}

/// The top-down style `name` says, case-insensitively.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the two styles.
fn parse_topdown_style(name: &str) -> Result<TopdownStyle, CyberError> {
    let lowered = name.to_ascii_lowercase();
    if lowered == "editor" {
        return Ok(TopdownStyle::Editor);
    }
    if lowered == "automap" {
        return Ok(TopdownStyle::Automap);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown topdown_style {name:?}: use \"editor\" for the editor look or \"automap\" for \
         the in-game red-on-black line map"
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Lighting, MAX_DIMENSION, MAX_FOV, MAX_GAMMA, MAX_PIXELS, MIN_DIMENSION, MIN_FOV,
        RenderSettings, Switch, TopdownStyle,
    };

    /// Serialises a settings value to JSON for shape assertions.
    fn to_json(settings: &RenderSettings) -> serde_json::Value {
        serde_json::to_value(settings).expect("settings are plain data")
    }

    #[test]
    fn defaults_are_the_vanilla_shaped_settings() {
        let settings = RenderSettings::default();
        assert_eq!(settings.width, 640, "default width");
        assert_eq!(settings.height, 400, "default height");
        assert_eq!(settings.lighting, Lighting::Doom, "default lighting");
        assert!(settings.show_things.is_on(), "things show by default");
        assert!(settings.show_sprites.is_on(), "sprites show by default");
        assert!(settings.sky.is_on(), "the sky draws by default");
        assert_eq!(settings.fov, 90, "default field of view");
        assert_eq!(settings.gamma, 0, "default gamma is the identity");
        assert_eq!(
            settings.topdown_style,
            TopdownStyle::Editor,
            "default top-down style"
        );
        assert_eq!(settings.background, "#000000", "default background");
    }

    #[test]
    fn serializes_with_the_documented_field_names() {
        let value = to_json(&RenderSettings::default());
        assert_eq!(
            value["lighting"],
            json!("doom"),
            "lighting is its lowercase name"
        );
        assert_eq!(value["topdown_style"], json!("editor"), "style name");
        assert_eq!(value["sky"], json!(true), "switches are JSON booleans");
        assert_eq!(value["show_things"], json!(true), "markers are a boolean");
        assert_eq!(value["show_sprites"], json!(true), "sprites are a boolean");
        assert_eq!(value["background"], json!("#000000"), "colour is a string");
        for key in [
            "width",
            "height",
            "lighting",
            "show_things",
            "show_sprites",
            "sky",
            "fov",
            "gamma",
            "topdown_style",
            "background",
        ] {
            assert!(value.get(key).is_some(), "settings report {key}");
        }
    }

    #[test]
    fn resolution_accepts_the_range_edges_and_the_pixel_cap_square() {
        let mut settings = RenderSettings::default();
        settings
            .set_resolution(MIN_DIMENSION, MIN_DIMENSION)
            .expect("the smallest resolution");
        assert_eq!(settings.width, MIN_DIMENSION, "smallest width");
        settings
            .set_resolution(MAX_DIMENSION, MAX_DIMENSION)
            .expect("the largest square is exactly the cap");
        assert_eq!(settings.height, MAX_DIMENSION, "largest height");
        settings
            .set_resolution(4_096_u32, 4_095_u32)
            .expect("just under the cap");
    }

    #[test]
    fn resolution_refuses_edges_outside_the_range() {
        // One-sided cases: the other edge stays mid-range so the total never
        // reaches the pixel cap and the refusal is about the edge itself.
        for (name, width, height, bad) in [
            ("width", 0_u32, 400_u32, 0_u32),
            ("width", 15_u32, 400_u32, 15_u32),
            ("height", 640_u32, 15_u32, 15_u32),
            ("height", 640_u32, 4_097_u32, 4_097_u32),
        ] {
            let mut settings = RenderSettings::default();
            let before = settings.clone();
            let error = settings
                .set_resolution(width, height)
                .expect_err("an edge outside the range is refused");
            assert_eq!(error.code(), "invalid_argument", "{name} code");
            let message = error.to_string();
            assert!(
                message.contains(name)
                    && message.contains(&bad.to_string())
                    && message.contains("16..=4096"),
                "the message names the field, the value and the range: {message}"
            );
            assert_eq!(settings, before, "a refused resolution changes nothing");
        }
    }

    #[test]
    fn resolution_refuses_a_total_over_the_pixel_cap() {
        let mut settings = RenderSettings::default();
        let error = settings
            .set_resolution(4_096_u32, 4_097_u32)
            .expect_err("one row past the cap square");
        assert_eq!(error.code(), "invalid_argument", "cap code");
        let message = error.to_string();
        assert!(
            message.contains(&MAX_PIXELS.to_string()) && message.contains("lower"),
            "the message names the cap and the fix: {message}"
        );
        let untouched = RenderSettings::default();
        assert_eq!(settings, untouched, "the cap refusal changes nothing");
    }

    #[test]
    fn lighting_accepts_its_three_names_any_case() {
        let mut settings = RenderSettings::default();
        for (text, wanted) in [
            ("doom", Lighting::Doom),
            ("FLAT", Lighting::Flat),
            ("FullBright", Lighting::Fullbright),
        ] {
            settings.set_lighting(text).expect(text);
            assert_eq!(settings.lighting, wanted, "{text} maps to its model");
        }
    }

    #[test]
    fn lighting_refuses_anything_else_listing_the_names() {
        let mut settings = RenderSettings::default();
        let before = settings.clone();
        let error = settings
            .set_lighting("realistic")
            .expect_err("realistic is not a model");
        assert_eq!(error.code(), "invalid_argument", "unknown lighting code");
        let message = error.to_string();
        for name in ["\"doom\"", "\"flat\"", "\"fullbright\""] {
            assert!(
                message.contains(name),
                "the message lists {name}: {message}"
            );
        }
        assert_eq!(settings, before, "a refused lighting changes nothing");
    }

    #[test]
    fn topdown_style_accepts_its_two_names_any_case() {
        let mut settings = RenderSettings::default();
        settings.set_topdown_style("AutoMap").expect("automap");
        assert_eq!(
            settings.topdown_style,
            TopdownStyle::Automap,
            "case ignored"
        );
        settings.set_topdown_style("editor").expect("editor");
        assert_eq!(settings.topdown_style, TopdownStyle::Editor, "back again");
    }

    #[test]
    fn topdown_style_refuses_anything_else_listing_the_names() {
        let mut settings = RenderSettings::default();
        let error = settings
            .set_topdown_style("wireframe")
            .expect_err("wireframe is not a style");
        assert_eq!(error.code(), "invalid_argument", "unknown style code");
        let message = error.to_string();
        assert!(
            message.contains("\"editor\"") && message.contains("\"automap\""),
            "the message lists both styles: {message}"
        );
    }

    #[test]
    fn fov_accepts_the_range_edges() {
        let mut settings = RenderSettings::default();
        settings.set_fov(MIN_FOV).expect("the narrowest view");
        assert_eq!(settings.fov, MIN_FOV, "narrow field");
        settings.set_fov(MAX_FOV).expect("the widest view");
        assert_eq!(settings.fov, MAX_FOV, "wide field");
    }

    #[test]
    fn fov_refuses_values_outside_the_range() {
        let mut settings = RenderSettings::default();
        for value in [9_u32, 0_u32, 171_u32, 360_u32] {
            let error = settings
                .set_fov(value)
                .expect_err("degrees outside the range are refused");
            assert_eq!(error.code(), "invalid_argument", "fov {value} code");
            assert!(
                error.to_string().contains(&value.to_string()),
                "the message shows the rejected value: {error}"
            );
        }
        assert_eq!(settings.fov, 90, "the refused values changed nothing");
    }

    #[test]
    fn gamma_accepts_the_four_levels_and_refuses_the_rest() {
        let mut settings = RenderSettings::default();
        for level in 0_u8..=MAX_GAMMA {
            settings.set_gamma(level).expect("a supported level");
            assert_eq!(settings.gamma, level, "level {level}");
        }
        let error = settings
            .set_gamma(5_u8)
            .expect_err("one past the top level");
        assert_eq!(error.code(), "invalid_argument", "gamma above the top code");
        assert_eq!(settings.gamma, MAX_GAMMA, "the refusal changed nothing");
    }

    #[test]
    fn background_accepts_six_hex_digits_either_case() {
        let mut settings = RenderSettings::default();
        settings.set_background("#ff8800").expect("lowercase hex");
        assert_eq!(settings.background, "#ff8800", "stored verbatim");
        settings.set_background("#A1B2C3").expect("uppercase hex");
        assert_eq!(settings.background, "#A1B2C3", "stored verbatim");
    }

    #[test]
    fn background_refuses_anything_that_is_not_a_colour() {
        let mut settings = RenderSettings::default();
        for value in ["", "red", "#fff", "#gggggg", "0xffffff", "#0000000"] {
            let error = settings
                .set_background(value)
                .expect_err("text that is not a colour is refused");
            assert_eq!(error.code(), "invalid_argument", "background code");
            let message = error.to_string();
            assert!(
                message.contains("#RRGGBB"),
                "the message shows the shape to use: {message}"
            );
        }
        assert_eq!(settings.background, "#000000", "a refusal changes nothing");
    }

    #[test]
    fn switches_are_plain_json_booleans_either_way() {
        let settings = RenderSettings {
            show_things: Switch::new(false),
            sky: false.into(),
            ..RenderSettings::default()
        };
        let value = to_json(&settings);
        assert_eq!(value["show_things"], json!(false), "off is JSON false");
        assert_eq!(value["sky"], json!(false), "the wrapper never shows up");
        assert!(!settings.show_things.is_on(), "off reads as off");
        assert!(!settings.sky.is_on(), "a converted bool reads as off");
    }

    #[test]
    fn lighting_and_style_use_the_documented_wire_names() {
        assert_eq!(
            serde_json::to_value(Lighting::Fullbright).expect("serializes"),
            json!("fullbright")
        );
        assert_eq!(
            serde_json::to_value(TopdownStyle::Automap).expect("serializes"),
            json!("automap")
        );
    }
}
