//! The load tools' side of the session group: the parameters, the reports
//! and the three runs behind `open_iwad`, `open_wad` and `new_wad`.
//!
//! Each run reads the file *outside* the session lock, folds what it found
//! into an [`ArchiveSummary`] and takes the lock only to change the session,
//! so a bad path or an unreadable archive never leaves the session
//! half-loaded. The shared rules live in the root: [`super::read_archive`]
//! validates the path and opens the file, [`super::ensure_clean`] refuses to
//! drop unsaved work and [`super::path_text`] renders a path into report
//! text.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::{Layer, LayerRole};
use crate::state::AppState;
use crate::wad::WadKind;
use crate::wad::archive::Wad;
use crate::wad::classify::analyze;
use crate::wad::game::Game;

use super::{ensure_clean, path_text, read_archive};

/// Arguments for [`Cyberdemon::open_iwad`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct OpenIwadParams {
    /// Absolute filesystem path of the archive to load as the base game
    /// layer; either `IWAD` or `PWAD` magic is accepted.
    pub(super) path: String,
}

/// Arguments for [`Cyberdemon::open_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct OpenWadParams {
    /// Absolute filesystem path of the archive to open as the working
    /// archive.
    pub(super) path: String,
    /// Replace the working archive even when it holds unsaved changes;
    /// without this a dirty working archive refuses the open.
    #[serde(default)]
    pub(super) discard_changes: bool,
}

/// Arguments for [`Cyberdemon::new_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct NewWadParams {
    /// Replace the working archive even when it holds unsaved changes.
    #[serde(default)]
    pub(super) discard_changes: bool,
}

/// A whole archive as the load tools report it.
struct ArchiveSummary {
    /// The path it was read from, as text.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Number of directory entries.
    lumps: usize,
    /// Number of map blocks it declares.
    maps: usize,
}

impl ArchiveSummary {
    /// Classify `wad` at `path` into the fields the load reports carry.
    fn of(path: &Path, wad: &Wad) -> Self {
        Self {
            path: path_text(path),
            kind: wad.kind,
            lumps: wad.len(),
            maps: analyze(&wad.lumps).maps.len(),
        }
    }
}

/// Structured result of [`Cyberdemon::open_iwad`].
#[derive(Serialize)]
pub(super) struct IwadReport {
    /// The file that became the base layer.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
    /// Map blocks in the archive.
    maps: usize,
    /// The game the session reports after the load.
    game: Game,
}

/// Structured result of [`Cyberdemon::open_wad`].
#[derive(Serialize)]
pub(super) struct OpenWadReport {
    /// The file that became the working archive.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
    /// Map blocks in the archive.
    maps: usize,
    /// The game the session reports after the open.
    game: Game,
    /// Whether the working archive now holds unsaved changes (never does
    /// right after an open).
    dirty: bool,
}

/// Structured result of [`Cyberdemon::new_wad`].
#[derive(Serialize)]
pub(super) struct NewWadReport {
    /// The kind of the fresh archive (always `PWAD`).
    kind: WadKind,
    /// Directory entries it holds (always `0`).
    lumps: usize,
}

/// `open_iwad`'s work: load the file and make it the session's base layer.
pub(super) fn open_iwad_report(state: &AppState, text: &str) -> Result<IwadReport, CyberError> {
    let (path, wad) = read_archive(text)?;
    let summary = ArchiveSummary::of(&path, &wad);
    state.with_session(|session| {
        session.set_iwad(Layer::new(path, LayerRole::Iwad, wad))?;
        Ok(IwadReport {
            path: summary.path,
            kind: summary.kind,
            lumps: summary.lumps,
            maps: summary.maps,
            game: session.game(),
        })
    })
}

/// `open_wad`'s work: load the file and adopt it as the working archive.
pub(super) fn open_wad_report(
    state: &AppState,
    text: &str,
    discard_changes: bool,
) -> Result<OpenWadReport, CyberError> {
    let (path, wad) = read_archive(text)?;
    let summary = ArchiveSummary::of(&path, &wad);
    state.with_session(|session| {
        ensure_clean(session, discard_changes)?;
        session.replace_working(wad, Some(path))?;
        Ok(OpenWadReport {
            path: summary.path,
            kind: summary.kind,
            lumps: summary.lumps,
            maps: summary.maps,
            game: session.game(),
            dirty: session.is_dirty(),
        })
    })
}

/// `new_wad`'s work: adopt a fresh empty `PWAD` as the working archive.
pub(super) fn new_wad_report(
    state: &AppState,
    discard_changes: bool,
) -> Result<NewWadReport, CyberError> {
    state.with_session(|session| {
        ensure_clean(session, discard_changes)?;
        session.replace_working(Wad::default(), None)?;
        Ok(NewWadReport {
            kind: session.working().kind,
            lumps: session.working().len(),
        })
    })
}
