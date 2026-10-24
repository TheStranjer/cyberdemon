//! `decode_patch`: read one `Doom` patch from a lump or a file and publish
//! both encodings.
//!
//! [`patch_payload`] is the either-or of [`DecodePatchParams`], reading a
//! merged-view lump or fetching an external file and naming the result
//! [`display_name`]; [`decode_patch_run`] decodes those bytes, colours them
//! with the session's `PLAYPAL` entry and publishes the `PNG` - drawing
//! offsets in a `grAb` chunk - beside the raw `RGBA8` bytes, answering with
//! the [`DecodedPatchReport`] that carries both asset references.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::patch::decode_patch;
use crate::state::AppState;
use crate::tools::graphics::resolve::lump_bytes;
use crate::tools::graphics::{DEFAULT_PALETTE, FALLBACK_NAME, RGBA_MIME};
use crate::tools::palette::load_palette;
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::decode_patch`]: exactly one of `name` and
/// `source` says where the patch bytes come from.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct DecodePatchParams {
    /// The lump to decode, `Doom` patch format only: 1 to 8 printable
    /// `ASCII` characters, case-insensitive, resolved anywhere in the
    /// merged view. Give this or `source`, never both.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Absolute path, `file://` URI, `http(s)://` `URL` or this server's
    /// asset `URL` of the patch file to decode. Give this or `name`, never
    /// both.
    #[serde(default)]
    pub(super) source: Option<String>,
    /// Which `PLAYPAL` entry colours the picture (default 0).
    #[serde(default)]
    pub(super) palette: Option<usize>,
}

/// The name and bytes of the patch one `decode_patch` call reads, from the
/// merged view or from a fetched file: exactly one of the two.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both `name` and `source` or for
/// neither; [`lump_bytes`]' and [`AppState::fetch_source`]' errors for the
/// chosen one.
async fn patch_payload(
    state: &AppState,
    args: &DecodePatchParams,
) -> Result<(String, Vec<u8>), CyberError> {
    match (args.name.as_deref(), args.source.as_deref()) {
        (Some(text), None) => {
            let name = LumpName::parse(text)?;
            let data = state.with_session(|session| lump_bytes(session, name))?;
            Ok((name.to_string(), data))
        }
        (None, Some(uri)) => {
            let fetched = state.fetch_source(uri).await?;
            Ok((display_name(&fetched.filename), fetched.bytes))
        }
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "decode_patch takes either name or source, not both: name one session lump or one \
             external file"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "decode_patch needs a patch to read: pass the name of a session lump or the source \
             of a file"
                .to_owned(),
        )),
    }
}

/// The name a fetched file is reported and published under: the lump name
/// its stem allows, else its filename, else [`FALLBACK_NAME`].
pub(super) fn display_name(filename: &str) -> String {
    derive_lump_name(filename)
        .map(|name| name.to_string())
        .or_else(|| (!filename.is_empty()).then(|| filename.to_owned()))
        .unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// Structured result of [`Cyberdemon::decode_patch`]: one patch as both
/// encodings.
#[derive(Serialize)]
pub(super) struct DecodedPatchReport {
    /// The patch's name: the lump, or the fetched file's name.
    name: String,
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// Horizontal drawing offset in pixels.
    left_offset: i32,
    /// Vertical drawing offset in pixels.
    top_offset: i32,
    /// The `PLAYPAL` entry both published pictures are coloured with.
    palette: usize,
    /// The published `PNG`, drawing offsets in a `grAb` chunk.
    png: AssetRef,
    /// The same picture as raw `RGBA8` bytes.
    rgba: AssetRef,
}

/// `decode_patch`'s work: fetch or read the bytes, decode the `Doom`
/// format and publish both encodings.
///
/// # Errors
///
/// [`patch_payload`]' errors for the arguments, [`decode_patch`]'
/// [`CyberError::Format`] for bytes that are no patch,
/// [`load_palette`]' [`CyberError::NotFound`] for the palette and
/// [`AppState::publish`]' errors for the two assets.
pub(super) async fn decode_patch_run(
    state: &AppState,
    args: &DecodePatchParams,
) -> Result<DecodedPatchReport, CyberError> {
    let index = args.palette.unwrap_or(DEFAULT_PALETTE);
    let (name, data) = patch_payload(state, args).await?;
    let patch = decode_patch(&data)?;
    let palette = state.with_session(|session| load_palette(session, index))?;
    let image = patch.image.to_rgba(&palette);
    let left_offset = i32::from(patch.left_offset);
    let top_offset = i32::from(patch.top_offset);
    let rgba = image.pixels.clone();
    let (width, height) = (image.width, image.height);
    let png_asset = state.publish(
        &format!("{name}.png"),
        "image/png",
        encode_png(&image, Some((left_offset, top_offset)))?,
    )?;
    let rgba_asset = state.publish(&format!("{name}.rgba"), RGBA_MIME, rgba)?;
    Ok(DecodedPatchReport {
        name,
        width,
        height,
        left_offset,
        top_offset,
        palette: index,
        png: png_asset,
        rgba: rgba_asset,
    })
}
