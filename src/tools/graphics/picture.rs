//! The shared half of the image tools: one picture's options, render and
//! publication.
//!
//! [`parse_options`] flattens the [`ImageParams`] every image tool embeds
//! into the [`ImageOptions`] the rest of the module reads, with
//! [`AssetFormat`] naming the encoding [`parse_scale`] bounds.
//! [`render_indexed`] and [`render_patch`] colour an indexed picture with
//! the chosen `PLAYPAL` entry and upscale it into a [`Rendered`], whose
//! drawing offsets [`scale_offset`] scales and [`grab_of`] collects for
//! [`encode_rendered`]; [`publish_image`] puts those bytes in the asset
//! store and [`finish`] answers with the [`ImageReport`] the tools return.
//! [`Prepared`] carries a render beside the `get_sprite` extras a caller has
//! resolved, and [`AssetFormat::extension`] is the one place a published
//! file name's suffix is decided.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::grab::GrabOffsets;
use crate::gfx::image::{IndexedImage, RgbaImage, scale_integer};
use crate::gfx::patch::Patch;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::graphics::{DEFAULT_PALETTE, DEFAULT_SCALE, MAX_SCALE, RGBA_MIME};
use crate::tools::palette::load_palette;
use crate::wad::name::LumpName;

/// The shared picture options of the image tools, flattened into each
/// tool's arguments.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct ImageParams {
    /// Which `PLAYPAL` entry colours the picture (default 0: the normal
    /// palette). Past the entries the lump holds is `not_found`, as is a
    /// session with no `PLAYPAL`.
    #[serde(default)]
    pub(super) palette: Option<usize>,
    /// How the asset is encoded: `"png"` (default) or `"rgba"` for raw
    /// row-major `RGBA8` bytes, `width*height*4` of them.
    #[serde(default)]
    pub(super) format: Option<String>,
    /// Whole-pixel nearest-neighbour upscale applied before publishing,
    /// `1..=8` (default 1); it multiplies the reported dimensions and
    /// drawing offsets too.
    #[serde(default)]
    pub(super) scale: Option<u32>,
}

/// How an image tool encodes the asset it publishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AssetFormat {
    /// A `PNG`, with a `grAb` chunk carrying the drawing offsets when the
    /// source has any.
    Png,
    /// Raw row-major `RGBA8` bytes.
    Rgba,
}

impl AssetFormat {
    /// The client's `format` word, defaulting to `png`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] for any other name.
    pub(super) fn parse(text: Option<&str>) -> Result<Self, CyberError> {
        match text {
            None | Some("png") => Ok(Self::Png),
            Some("rgba") => Ok(Self::Rgba),
            Some(other) => Err(CyberError::InvalidArgument(format!(
                "format {other:?} is unknown: pass \"png\" for a PNG asset or \"rgba\" for raw \
                 RGBA8 bytes"
            ))),
        }
    }

    /// This encoding's filename extension and media type.
    pub(super) const fn extension(self) -> (&'static str, &'static str) {
        match self {
            Self::Png => ("png", "image/png"),
            Self::Rgba => ("rgba", RGBA_MIME),
        }
    }
}

/// The validated `palette`, `format` and `scale` of one image call.
#[derive(Debug)]
pub(super) struct ImageOptions {
    /// `PLAYPAL` entry to colour with.
    pub(super) palette: usize,
    /// How to encode the asset.
    pub(super) format: AssetFormat,
    /// Whole-pixel upscale factor.
    pub(super) scale: u32,
}

/// The validated options of one image call.
///
/// # Errors
///
/// [`AssetFormat::parse`] and [`parse_scale`]' [`CyberError::InvalidArgument`].
pub(super) fn parse_options(common: &ImageParams) -> Result<ImageOptions, CyberError> {
    Ok(ImageOptions {
        palette: common.palette.unwrap_or(DEFAULT_PALETTE),
        format: AssetFormat::parse(common.format.as_deref())?,
        scale: parse_scale(common.scale)?,
    })
}

/// The `scale` factor: a whole number in `1..=8`, defaulting to 1.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected factor.
fn parse_scale(value: Option<u32>) -> Result<u32, CyberError> {
    match value {
        None => Ok(DEFAULT_SCALE),
        Some(factor @ (DEFAULT_SCALE..=MAX_SCALE)) => Ok(factor),
        Some(factor) => Err(CyberError::InvalidArgument(format!(
            "scale {factor} is out of range: pass a whole factor between {DEFAULT_SCALE} and \
             {MAX_SCALE}"
        ))),
    }
}

/// A picture's pixels and drawing offsets, rendered and ready to publish.
pub(super) struct Rendered {
    /// The scaled pixels.
    pub(super) image: RgbaImage,
    /// Horizontal drawing offset in published pixels; `None` for a flat.
    pub(super) left_offset: Option<i32>,
    /// Vertical drawing offset in published pixels; `None` for a flat.
    pub(super) top_offset: Option<i32>,
}

/// What one image call answers with: the picture plus the sprite extras
/// the plain image tools leave out.
pub(super) struct Prepared {
    /// The name the report answers with.
    pub(super) name: LumpName,
    /// The lump that actually served the image (`get_sprite` only).
    pub(super) lump: Option<LumpName>,
    /// Whether a stored sprite image was flipped (`get_sprite` only).
    pub(super) mirrored: Option<bool>,
    /// The pixels and offsets to publish.
    pub(super) rendered: Rendered,
}

impl Prepared {
    /// The answer for a flat or a patch: the requested name, no extras.
    pub(super) const fn plain(name: LumpName, rendered: Rendered) -> Self {
        Self {
            name,
            lump: None,
            mirrored: None,
            rendered,
        }
    }

    /// The answer for a sprite call, which also names the lump that served
    /// the rotation and whether its stored image was flipped.
    pub(super) const fn served(
        name: LumpName,
        lump: LumpName,
        mirrored: bool,
        rendered: Rendered,
    ) -> Self {
        Self {
            name,
            lump: Some(lump),
            mirrored: Some(mirrored),
            rendered,
        }
    }
}

/// Structured result of `get_flat`, `get_patch` and `get_sprite`.
#[derive(Serialize)]
pub(super) struct ImageReport {
    /// The picture's name: the flat or patch lump, or the sprite name the
    /// call asked for (`TROOA8`).
    name: LumpName,
    /// Width in published pixels: `scale` times the stored image.
    width: u32,
    /// Height in published pixels.
    height: u32,
    /// Horizontal drawing offset in published pixels; absent for flats,
    /// which have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    left_offset: Option<i32>,
    /// Vertical drawing offset in published pixels, like `left_offset`.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_offset: Option<i32>,
    /// The lump whose pixels served the call; `get_sprite` only, because a
    /// mirror-pair or all-angles lump can stand in for the requested name.
    #[serde(skip_serializing_if = "Option::is_none")]
    lump: Option<LumpName>,
    /// Whether the stored sprite image was flipped horizontally to serve
    /// the rotation; `get_sprite` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    mirrored: Option<bool>,
    /// The published picture.
    asset: AssetRef,
}

/// Colours `image` with the session's requested palette, upscales it and
/// keeps the drawing offsets scaled along.
///
/// # Errors
///
/// [`load_palette`]' [`CyberError::NotFound`] for a palette index the
/// session's `PLAYPAL` does not hold.
pub(super) fn render_indexed(
    session: &Session,
    image: &IndexedImage,
    offsets: Option<(i16, i16)>,
    options: &ImageOptions,
) -> Result<Rendered, CyberError> {
    let palette = load_palette(session, options.palette)?;
    Ok(Rendered {
        image: scale_integer(&image.to_rgba(&palette), options.scale),
        left_offset: offsets.map(|(left, _)| scale_offset(left, options.scale)),
        top_offset: offsets.map(|(_, top)| scale_offset(top, options.scale)),
    })
}

/// The render of a patch or sprite: its pixels plus both drawing offsets.
///
/// # Errors
///
/// [`render_indexed`]' errors.
pub(super) fn render_patch(
    session: &Session,
    patch: &Patch,
    options: &ImageOptions,
) -> Result<Rendered, CyberError> {
    let offsets = (patch.left_offset, patch.top_offset);
    render_indexed(session, &patch.image, Some(offsets), options)
}

/// One drawing offset in published pixels: `scale` times the stored
/// offset, keeping it as is when the product would not fit.
pub(super) fn scale_offset(value: i16, scale: u32) -> i32 {
    let factor = i32::try_from(scale).unwrap_or(1_i32);
    let scaled = i32::from(value);
    scaled.checked_mul(factor).unwrap_or(scaled)
}

/// The `grAb` offsets a published `PNG` carries: a render's drawing
/// offsets, and none for a flat.
pub(super) fn grab_of(rendered: &Rendered) -> Option<GrabOffsets> {
    let (left, top) = (rendered.left_offset?, rendered.top_offset?);
    Some((left, top))
}

/// The asset bytes of one render in one encoding.
///
/// # Errors
///
/// [`encode_png`]' errors for a `PNG`; raw `RGBA8` bytes cannot fail.
fn encode_rendered(rendered: &Rendered, format: AssetFormat) -> Result<Vec<u8>, CyberError> {
    match format {
        AssetFormat::Png => encode_png(&rendered.image, grab_of(rendered)),
        AssetFormat::Rgba => Ok(rendered.image.pixels.clone()),
    }
}

/// Publishes one render as `NAME.png` or `NAME.rgba`.
///
/// # Errors
///
/// [`encode_rendered`]' errors and [`AppState::publish`]' for the store.
pub(super) fn publish_image(
    state: &AppState,
    name: &str,
    rendered: &Rendered,
    options: &ImageOptions,
) -> Result<AssetRef, CyberError> {
    let (extension, mime) = options.format.extension();
    let data = encode_rendered(rendered, options.format)?;
    state.publish(&format!("{name}.{extension}"), mime, data)
}

/// Publishes a prepared picture and assembles the shared report.
///
/// # Errors
///
/// [`publish_image`]' errors.
pub(super) fn finish(
    state: &AppState,
    prepared: Prepared,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let Prepared {
        name,
        lump,
        mirrored,
        rendered,
    } = prepared;
    let width = rendered.image.width;
    let height = rendered.image.height;
    let left_offset = rendered.left_offset;
    let top_offset = rendered.top_offset;
    let asset = publish_image(state, &name.to_string(), &rendered, options)?;
    Ok(ImageReport {
        name,
        width,
        height,
        left_offset,
        top_offset,
        lump,
        mirrored,
        asset,
    })
}
