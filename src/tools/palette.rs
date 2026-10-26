//! Palette tools: reading the session's `PLAYPAL` and writing it back,
//! with optional regeneration of the vanilla tints and the `COLORMAP`.
//!
//! The shared `load_palette` reader lives beside the session it reads in
//! [`crate::session::palette`]; this group re-exports it so every graphics
//! tool keeps reaching it at `crate::tools::palette::load_palette`.
//! `get_palette` returns a palette as hex plus a published `PNG` swatch;
//! `set_palette` replaces palettes from inline colours, a fetched image or
//! raw palette bytes - copy-on-write into the working archive as one
//! undoable edit - and can rebuild the 13 vanilla tints from palette 0 and
//! regenerate `COLORMAP` by nearest-colour matching. The tint and `COLORMAP`
//! recipes approximate the vanilla tables; they are formulaic, integer-only
//! and documented in the tool descriptions.
//!
//! The `set_palette` side lives in the private `set` submodule: its
//! parameter, payload and request types beside the steps that resolve one
//! source, apply it to a copy of the merged `PLAYPAL` and commit the edit.
//! This root keeps the re-exported reader, the swatch builder, the constants
//! both halves share and the two router methods.

mod set;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::gfx::palette::Palette;
use crate::server::Cyberdemon;
pub(crate) use crate::session::palette::load_palette;
use crate::session::palette::require_playpal;
use crate::state::AppState;
use crate::tools::palette::set::{SetPaletteParams, set_palette_run};
use crate::tools::reply;
use crate::wad::lumps;

/// Bytes one palette occupies inside `PLAYPAL` (256 RGB triples).
const PALETTE_BYTES: usize = 768;
/// Colours in one palette.
const PALETTE_COLORS: usize = 256;
/// Vanilla `PLAYPAL` palette count: normal, red damage, gold pickup,
/// green suit.
const VANILLA_PALETTES: usize = 14;
/// First tint index (index 0 is the untinted normal palette).
const FIRST_TINT: usize = 1;
/// Last red (damage) tint index.
const RED_LAST: i64 = 8;
/// Last gold (pickup) tint index.
const GOLD_LAST: i64 = 12;
/// Offset from a gold tint index to its weight numerator: `(i - 8) / 8`.
const GOLD_OFFSET: i64 = 8;
/// Pixels on one swatch cell edge.
const SWATCH_CELL: u32 = 16;
/// Swatch cells per edge: the 256 colours in a 16x16 grid.
const SWATCH_CELLS: u32 = 16;
/// Swatch pixels per edge (`SWATCH_CELL * SWATCH_CELLS`).
const SWATCH_SIDE: u32 = SWATCH_CELL.saturating_mul(SWATCH_CELLS);
/// Alpha byte of every swatch pixel.
const OPAQUE: u8 = 255;
/// Channel value `0` as an `i64`, for checked arithmetic.
const ZERO: i64 = 0;
/// Weight numerator/denominator one, for checked arithmetic.
const ONE: i64 = 1;

/// Arguments for [`Cyberdemon::get_palette`].
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct GetPaletteParams {
    /// Which `PLAYPAL` entry to read (default 0: the normal palette;
    /// Doom's 1..=8 are the red damage tints, 9..=12 the gold pickup
    /// tint and 13 the green radiation-suit tint).
    #[serde(default)]
    index: Option<usize>,
}

/// Structured result of [`Cyberdemon::get_palette`].
#[derive(Serialize)]
struct GetPaletteReport {
    /// Palettes the `PLAYPAL` lump holds.
    palettes: usize,
    /// The palette this call read.
    index: usize,
    /// The 256 colours as `#RRGGBB` strings, uppercase hex.
    colors: Vec<String>,
    /// The published 256x256 `PNG` swatch: a 16x16 grid of 16-pixel cells.
    swatch: AssetRef,
    /// Whether the session has a `COLORMAP` lump.
    has_colormap: bool,
}

/// A palette and its lump's facts, read under the session lock.
struct LoadedPalette {
    /// Palettes the `PLAYPAL` lump holds.
    palettes: usize,
    /// The palette that was asked for.
    palette: Palette,
    /// Whether the session has a `COLORMAP` lump.
    has_colormap: bool,
}

/// The 256x256 preview image: a 16x16 grid of 16-pixel cells in palette
/// order, every pixel opaque.
pub(in crate::tools) fn swatch_image(palette: &Palette) -> Result<RgbaImage, CyberError> {
    let side = usize::try_from(SWATCH_SIDE).unwrap_or(PALETTE_COLORS);
    let mut pixels: Vec<u8> = Vec::with_capacity(side.saturating_mul(side).saturating_mul(4));
    for row in 0..SWATCH_SIDE {
        let cell_row = row.checked_div(SWATCH_CELL).unwrap_or(0);
        for column in 0..SWATCH_SIDE {
            let cell = cell_row
                .checked_mul(SWATCH_CELLS)
                .and_then(|line| line.checked_add(column.checked_div(SWATCH_CELL).unwrap_or(0)));
            let color = cell
                .and_then(|number| usize::try_from(number).ok())
                .and_then(|number| palette.color(number))
                .unwrap_or([0_u8; 3]);
            let [red, green, blue] = color;
            pixels.extend_from_slice(&[red, green, blue, OPAQUE]);
        }
    }
    RgbaImage::new(SWATCH_SIDE, SWATCH_SIDE, pixels)
}

/// `get_palette`'s work: read under the session lock, then publish the
/// swatch.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a missing `PLAYPAL` or an out-of-range
/// `index`; the encoder's and publisher's errors for the swatch.
fn get_palette_run(state: &AppState, index: usize) -> Result<GetPaletteReport, CyberError> {
    let loaded = state.with_session(|session| {
        let palette = load_palette(session, index)?;
        Ok(LoadedPalette {
            palettes: require_playpal(session)?
                .chunks_exact(PALETTE_BYTES)
                .count(),
            palette,
            has_colormap: session.find_global(lumps::COLORMAP).is_some(),
        })
    })?;
    let png = encode_png(&swatch_image(&loaded.palette)?, None)?;
    let swatch = state.publish(&format!("PLAYPAL_{index}.png"), "image/png", png)?;
    Ok(GetPaletteReport {
        palettes: loaded.palettes,
        index,
        colors: loaded.palette.to_hex_list(),
        swatch,
        has_colormap: loaded.has_colormap,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = palette_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reads one `PLAYPAL` palette of the session.
    #[tool(
        description = "Reads one PLAYPAL palette of the merged view as 256 #RRGGBB strings with a published 256x256 swatch, and reports whether a COLORMAP exists. index picks the entry (0 the normal palette, 1..=8 damage tints, 9..=12 pickup, 13 the radiation suit). No PLAYPAL loaded is not_found - open_iwad first. set_palette writes new palettes."
    )]
    fn get_palette(
        &self,
        Parameters(args): Parameters<GetPaletteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(get_palette_run(self.state(), args.index.unwrap_or(0_usize)))
    }

    /// Replaces `PLAYPAL` palettes, optionally rebuilding tints and `COLORMAP`.
    #[tool(
        description = "Writes palettes into the working archive's PLAYPAL as ONE undoable edit (copy-on-write from the merged lump): colors is exactly 256 #RRGGBB strings replacing the entry index (default 0, passing the count appends), or source is a 256-pixel image or raw palette bytes; regenerate_tints rebuilds the vanilla 1..=13 and regenerate_colormap rebuilds COLORMAP. Returns a swatch preview."
    )]
    async fn set_palette(
        &self,
        Parameters(args): Parameters<SetPaletteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(set_palette_run(self.state(), &args).await)
    }
}

#[cfg(test)]
mod tests;
