//! The two texture listings: `list_textures` over the merged view's
//! composite definitions and `list_pnames` over its patch-name index.
//!
//! [`ListTexturesParams`] and [`ListPnamesParams`] are the wire arguments;
//! [`list_textures_run`] filters and pages [`TextureSet::texture_rows`] into
//! [`TextureRow`]s, [`list_pnames_run`] the merged names into [`PnameRow`]s
//! whose [`PnameRow::lump_found`] comes from [`present_lump_names`]. Both
//! reports count every match before paging.

use alloc::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::{Scope, Session, Source};
use crate::tools::lumps::rows::build_rows;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

use super::{Page, TextureSet, page_rows, passes};

/// Arguments for [`Cyberdemon::list_textures`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListTexturesParams {
    /// Case-insensitive texture-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pub(super) pattern: Option<String>,
    /// Matching textures to skip before the page (default 0).
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::list_pnames`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListPnamesParams {
    /// Case-insensitive patch-name glob: `*` matches any run of characters,
    /// `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pub(super) pattern: Option<String>,
    /// Matching entries to skip before the page (default 0).
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// One `list_textures` row.
#[derive(Debug, Serialize)]
pub(super) struct TextureRow {
    /// The texture name.
    pub(super) name: String,
    /// Width in pixels.
    pub(super) width: u16,
    /// Height in pixels.
    pub(super) height: u16,
    /// Patch count.
    pub(super) patches: usize,
    /// Which texture lump defines it: `"TEXTURE1"` or `"TEXTURE2"`.
    pub(super) lump: &'static str,
    /// The archive that lump was read from.
    pub(super) source: Source,
}

/// Structured result of [`Cyberdemon::list_textures`].
#[derive(Debug, Serialize)]
pub(super) struct TexturesReport {
    /// Textures matching `pattern` (before paging).
    pub(super) total: usize,
    /// The matching textures, in lump order, on this page.
    pub(super) textures: Vec<TextureRow>,
}

/// One `list_pnames` row.
#[derive(Debug, Serialize)]
pub(super) struct PnameRow {
    /// Index in `PNAMES` (the number textures refer to).
    pub(super) index: usize,
    /// The patch name.
    pub(super) name: String,
    /// Whether a lump with this name exists in the merged view outside the
    /// flats and sprites regions.
    pub(super) lump_found: bool,
}

/// Structured result of [`Cyberdemon::list_pnames`].
#[derive(Debug, Serialize)]
pub(super) struct PnamesReport {
    /// Entries matching `pattern` (before paging).
    pub(super) total: usize,
    /// The matching entries, in `PNAMES` order, on this page.
    pub(super) pnames: Vec<PnameRow>,
}

/// `list_textures`' work over the session.
///
/// # Errors
///
/// [`TextureSet::load`]'s [`CyberError::Format`] for a malformed lump.
pub(super) fn list_textures_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
) -> Result<TexturesReport, CyberError> {
    let set = TextureSet::load(session)?;
    let matched: Vec<TextureRow> = set
        .texture_rows()
        .into_iter()
        .filter(|row| passes(&row.name, pattern))
        .collect();
    let (total, textures) = page_rows(matched, page);
    Ok(TexturesReport { total, textures })
}

/// `list_pnames`' work over the session.
///
/// # Errors
///
/// [`TextureSet::load`]'s errors and [`build_rows`]' for the merged scope.
pub(super) fn list_pnames_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
) -> Result<PnamesReport, CyberError> {
    let set = TextureSet::load(session)?;
    let present = present_lump_names(session)?;
    let matched: Vec<PnameRow> = set
        .pnames
        .iter()
        .enumerate()
        .filter(|&(_index, entry)| passes(entry, pattern))
        .map(|(index, entry)| PnameRow {
            lump_found: LumpName::parse(entry).is_ok_and(|parsed| present.contains(&parsed)),
            index,
            name: entry.clone(),
        })
        .collect();
    let (total, pnames) = page_rows(matched, page);
    Ok(PnamesReport { total, pnames })
}

/// The merged-view names that exist as lumps outside the flats and sprites
/// regions: the candidates a `PNAMES` entry can point at.
pub(super) fn present_lump_names(session: &Session) -> Result<BTreeSet<LumpName>, CyberError> {
    Ok(build_rows(session, Scope::Merged)?
        .into_iter()
        .filter(|row| !matches!(row.namespace, Some(Namespace::Flats | Namespace::Sprites)))
        .map(|row| row.name)
        .collect())
}
