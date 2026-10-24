//! Where the archives stand: the origin of a lump of the merged view, that
//! lump as the view holds it, and the scope names a read tool asks for.
//!
//! [`Source`] tags every [`MergedLump`] with the archive it was folded in
//! from, and [`Scope`] is how a client picks the archive it wants to read:
//! the merged view, the working archive alone, or one read-only layer by
//! index. Both name the same stack [`crate::session::Session::layers`]
//! returns, so a layer index means the same thing on both sides;
//! [`crate::session::Session::lumps_in`] turns a [`Scope`] into a directory.

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::wad::archive::Lump;

/// Which archive a lump of the merged view was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Layer `n` of [`crate::session::Session::layers`]: 0 is the `IWAD`,
    /// then the resources.
    Layer(usize),
    /// The editable working archive.
    Working,
}

/// A lump as the merged view sees it: the bytes plus their origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedLump {
    /// The lump itself.
    pub lump: Lump,
    /// The archive it was taken from.
    pub source: Source,
    /// Its index inside that archive's directory.
    pub source_index: usize,
}

/// The archive a read tool looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The merged view: `IWAD`, resource layers, then the working archive.
    Merged,
    /// The editable working archive alone.
    Working,
    /// Read-only layer `n` of [`crate::session::Session::layers`] alone.
    Layer(usize),
}

impl Scope {
    /// Parses a scope name: `merged`, `working` or `layer:N`, where `N` is a
    /// zero-based index into [`crate::session::Session::layers`]. Every name
    /// is matched case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the accepted forms when `text`
    /// is neither keyword and `layer:` is not followed by a decimal index.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        let lowered = text.to_ascii_lowercase();
        if lowered == "merged" {
            return Ok(Self::Merged);
        }
        if lowered == "working" {
            return Ok(Self::Working);
        }
        let Some(digits) = lowered.strip_prefix("layer:") else {
            return Err(unknown_scope(text));
        };
        digits.parse::<usize>().map(Self::Layer).map_err(|error| {
            CyberError::InvalidArgument(format!(
                "scope {text:?} names layer {digits:?}, which is not a decimal index: {error}"
            ))
        })
    }
}

/// The refusal for text that is not one of the accepted scope names.
fn unknown_scope(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown scope {text:?}: expected \"merged\", \"working\" or \"layer:N\", where N is a \
         zero-based index into the loaded layers"
    ))
}
