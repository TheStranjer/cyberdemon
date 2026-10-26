//! The session's `PLAYPAL` reader, shared by the graphics resolvers, the
//! renderers and the palette tools.
//!
//! `load_palette` is the one entry point every caller reads current
//! colours through: it takes the last (winning) `PLAYPAL` of the merged view
//! (`require_playpal`) and answers with one 768-byte entry. It lives here,
//! beside the session it reads, so neither [`crate::render`] nor
//! [`crate::session::resolve`] has to reach into the tool layer for it; the
//! `palette` tools re-export it and add the `PNG` swatch and the writing
//! half on top.

use crate::error::CyberError;
use crate::gfx::palette::Palette;
use crate::session::Session;
use crate::wad::lumps;

/// The `index`th palette of the session's `PLAYPAL`.
///
/// Reads the last (winning) `PLAYPAL` of the merged view. This is the one
/// entry point every graphics tool and the renderers load current colours
/// through.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds a `PLAYPAL` or
/// `index` names a palette the lump does not contain.
#[inline]
pub(crate) fn load_palette(session: &Session, index: usize) -> Result<Palette, CyberError> {
    Palette::from_playpal(&require_playpal(session)?, index)
}

/// The merged view's winning `PLAYPAL` bytes.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming `open_iwad` when no loaded archive
/// holds a `PLAYPAL` lump.
pub(crate) fn require_playpal(session: &Session) -> Result<Vec<u8>, CyberError> {
    let entry = session.find_global(lumps::PLAYPAL).ok_or_else(|| {
        CyberError::NotFound("no PLAYPAL in the session; open an IWAD with open_iwad".to_owned())
            .with_hint("call open_iwad (game data) first, then new_wad or open_wad")
    })?;
    Ok(entry.lump.data.to_vec())
}
