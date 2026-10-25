//! The archive-moving side of the session group: `merge_wad` brings a file
//! into the session and `save_wad` writes one back out.
//!
//! [`merge_wad_report`] parses the mode, reads the file and stacks it as a
//! read-only layer ([`merge_layer_report`]) or folds it into the working
//! archive as one undoable edit ([`merge_working_report`]).
//! [`save_wad_report`] resolves the target path, the mode and the header
//! magic, refuses to overwrite a loaded layer's own file, serialises either
//! the working archive ([`working_copy`]) or the whole merged view
//! ([`merged_export`]) and writes it atomically.
//!
//! [`super::read_archive`] and [`super::path_text`] come from the shared
//! root; everything here that is not one of the two runs is a step of one
//! of them.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::merge::{MergeCounts, merge_counts, merge_into};
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::wad::WadKind;
use crate::wad::archive::{Wad, write_file_atomic};

use super::{path_text, read_archive};

/// Arguments for [`Cyberdemon::merge_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct MergeWadParams {
    /// Absolute filesystem path of the archive to merge.
    pub(super) path: String,
    /// `"layer"` stacks the file as a read-only resource layer (the
    /// default); `"working"` folds its lumps into the working archive as
    /// one undoable edit.
    #[serde(default)]
    pub(super) mode: Option<String>,
}

/// How [`Cyberdemon::merge_wad`] combines the file with the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum MergeMode {
    /// Stack the archive as a read-only resource layer.
    #[default]
    Layer,
    /// Fold the archive's lumps into the working archive.
    Working,
}

/// The mode names the `merge_wad` `mode` parameter accepts, `None` for the
/// default. Anything else is refused with a hint naming both forms.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `layer` or
/// `working`, in any case.
fn parse_mode(text: Option<&str>) -> Result<MergeMode, CyberError> {
    let Some(raw) = text else {
        return Ok(MergeMode::Layer);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "layer" {
        return Ok(MergeMode::Layer);
    }
    if lowered == "working" {
        return Ok(MergeMode::Working);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown merge mode {raw:?}: use \"layer\" to stack the file read-only or \"working\" \
         to merge its lumps into the working archive"
    )))
}

/// Arguments for [`Cyberdemon::save_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SaveWadParams {
    /// Absolute filesystem path to write. Defaults to the working archive's
    /// path (set by `open_wad` or a previous `save_wad`); without either the
    /// call fails with `invalid_argument`.
    #[serde(default)]
    pub(super) path: Option<String>,
    /// `"pwad"` (the default) writes the working archive and marks it saved;
    /// `"full"` exports the entire merged view as one standalone WAD without
    /// touching the working path or the dirty flag.
    #[serde(default)]
    pub(super) mode: Option<String>,
    /// Header magic to write, `"PWAD"` or `"IWAD"`. Defaults to the working
    /// archive's kind for mode `pwad`; for `full`, `IWAD` when an IWAD layer
    /// is loaded, else `PWAD`.
    #[serde(default)]
    pub(super) kind: Option<String>,
    /// Write each distinct non-empty lump payload once, with the duplicate
    /// directory entries sharing the first copy's offset (default `false`).
    /// Engines read shared offsets fine; the result then reports how many
    /// bytes the sharing saved.
    #[serde(default)]
    pub(super) dedupe: bool,
}

/// How [`Cyberdemon::save_wad`] decides what to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SaveMode {
    /// Write the working archive and record it as saved.
    Pwad,
    /// Export the merged view; the session keeps its working path and dirty
    /// state untouched.
    Full,
}

/// The mode names the `save_wad` `mode` parameter accepts, `None` for the
/// default. Anything else is refused with a hint naming both forms.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `pwad` or
/// `full`, in any case.
fn parse_save_mode(text: Option<&str>) -> Result<SaveMode, CyberError> {
    let Some(raw) = text else {
        return Ok(SaveMode::Pwad);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "pwad" {
        return Ok(SaveMode::Pwad);
    }
    if lowered == "full" {
        return Ok(SaveMode::Full);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown save mode {raw:?}: use \"pwad\" to save the working archive or \"full\" to \
         export the merged view as one standalone WAD"
    )))
}

/// The header magic the `save_wad` `kind` parameter selects, `None` for the
/// mode's default.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `pwad` or
/// `iwad`, in any case.
fn parse_kind(text: Option<&str>) -> Result<Option<WadKind>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "pwad" {
        return Ok(Some(WadKind::Pwad));
    }
    if lowered == "iwad" {
        return Ok(Some(WadKind::Iwad));
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown kind {raw:?}: use \"PWAD\" for a patch archive or \"IWAD\" for a full game \
         archive, or leave the parameter out for the default magic"
    )))
}

/// Structured result of [`Cyberdemon::merge_wad`], one shape per mode.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum MergeReport {
    /// The file went onto the read-only layer stack.
    Layer {
        /// Which mode ran.
        mode: MergeMode,
        /// Index the new layer took in the session's layer numbering.
        layer_index: usize,
        /// Directory entries the layered archive holds.
        lumps: usize,
    },
    /// The file was folded into the working archive.
    Working {
        /// Which mode ran.
        mode: MergeMode,
        /// Base entries the overlay replaced (a whole map block counts
        /// as one).
        replaced: usize,
        /// Entries the overlay brought that the working archive lacked
        /// (a whole new map block counts as one).
        added: usize,
    },
}

/// Structured result of [`Cyberdemon::save_wad`].
#[derive(Serialize)]
pub(super) struct SaveReport {
    /// The file that was written.
    pub(super) path: String,
    /// Which mode ran.
    pub(super) mode: SaveMode,
    /// The header magic written into the file.
    pub(super) kind: WadKind,
    /// Directory entries in the file.
    pub(super) lumps: usize,
    /// Bytes in the file.
    pub(super) bytes: usize,
    /// Payload bytes saved by writing duplicate lump data once, `0` unless
    /// the call passed `dedupe`.
    pub(super) bytes_saved_by_dedupe: usize,
    /// Whether the working archive still holds unsaved changes (`false`
    /// after a `pwad` save, unchanged after a `full` export).
    pub(super) dirty: bool,
}

/// `merge_wad` mode `layer`: stack the file above the existing layers.
fn merge_layer_report(
    state: &AppState,
    path: PathBuf,
    wad: &Wad,
) -> Result<MergeReport, CyberError> {
    let lumps = wad.len();
    state.with_session(|session| {
        let layer_index = session.layers().len();
        session.add_resource(Layer::new(path, LayerRole::Resource, wad.clone()));
        Ok(MergeReport::Layer {
            mode: MergeMode::Layer,
            layer_index,
            lumps,
        })
    })
}

/// `merge_wad` mode `working`: fold the file into the working archive as
/// one undoable edit.
fn merge_working_report(
    state: &AppState,
    path: &Path,
    overlay: &Wad,
) -> Result<MergeReport, CyberError> {
    let file_name = path.file_name().map_or_else(
        || path_text(path),
        |name| name.to_string_lossy().into_owned(),
    );
    let label = format!("merge_wad {file_name}");
    state.with_session(|session| {
        let base = session.working();
        let MergeCounts { replaced, added } = merge_counts(base, overlay);
        let merged = merge_into(base, overlay);
        session.edit(&label, |working| {
            *working = merged;
            Ok(())
        })?;
        Ok(MergeReport::Working {
            mode: MergeMode::Working,
            replaced,
            added,
        })
    })
}

/// `merge_wad`'s work: parse the mode, read the file, then run one mode.
pub(super) fn merge_wad_report(
    state: &AppState,
    text: &str,
    mode: Option<&str>,
) -> Result<MergeReport, CyberError> {
    let wanted = parse_mode(mode)?;
    let (path, wad) = read_archive(text)?;
    match wanted {
        MergeMode::Layer => merge_layer_report(state, path, &wad),
        MergeMode::Working => merge_working_report(state, &path, &wad),
    }
}

/// The file `save_wad` will write: the `path` argument when given (it must
/// be absolute), else the working archive's remembered path.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a relative `path`, and for no `path`
/// on a session that never opened or saved to a file.
fn resolve_target(session: &Session, text: Option<&str>) -> Result<PathBuf, CyberError> {
    let Some(raw) = text else {
        return session
            .working_path()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                CyberError::InvalidArgument(
                    "no path: pass path, the working archive was never opened from or saved to \
                     a file"
                        .to_owned(),
                )
            });
    };
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(CyberError::InvalidArgument(format!(
            "path {raw:?} is not absolute; pass the full filesystem path to save to"
        )));
    }
    Ok(path.to_path_buf())
}

/// The canonical form of `path` when a file stands there, for same-file
/// comparisons; `None` when it is absent or cannot be resolved.
fn canonical_existing(path: &Path) -> Option<PathBuf> {
    if !path.try_exists().is_ok_and(|exists| exists) {
        return None;
    }
    path.canonicalize().ok()
}

/// Refuse to overwrite the file one of the session's read-only layers reads
/// from, compared by canonical path whenever the target exists.
///
/// # Errors
///
/// [`CyberError::Conflict`] naming the layer and the escape hatch when
/// `target` is a loaded layer's file.
fn ensure_not_layer_file(session: &Session, target: &Path) -> Result<(), CyberError> {
    let Some(resolved) = canonical_existing(target) else {
        return Ok(());
    };
    for layer in session.layers() {
        let same_file = canonical_existing(&layer.path).is_some_and(|file| file == resolved);
        if same_file {
            return Err(CyberError::Conflict(format!(
                "{} is the session's {} layer at {}; pass a different path",
                target.display(),
                layer.role.label(),
                layer.path.display(),
            )));
        }
    }
    Ok(())
}

/// The archive `save_wad` mode `pwad` writes: the working archive, its magic
/// overridden when `kind` says so.
fn working_copy(session: &Session, kind: Option<WadKind>) -> Wad {
    let mut wad = session.working().clone();
    if let Some(wanted) = kind {
        wad.kind = wanted;
    }
    wad
}

/// The archive `save_wad` mode `full` writes: the whole merged view as one
/// standalone `Wad`, `IWAD`-magic by default while a base layer is loaded.
fn merged_export(session: &Session, kind: Option<WadKind>) -> Wad {
    let view = session.merged();
    let default = session.iwad().map_or(WadKind::Pwad, |_| WadKind::Iwad);
    Wad {
        kind: kind.unwrap_or(default),
        lumps: view.into_iter().map(|entry| entry.lump).collect(),
    }
}

/// `save_wad`'s work: resolve path, mode and kind and snapshot the archive
/// under the session lock, then serialize and write outside it, so no
/// filesystem work runs with the session held. In mode `pwad` the finished
/// write is recorded as the working archive's saved state under its own
/// short lock. With `dedupe` the write shares duplicate payloads and the
/// report carries the bytes saved.
pub(super) fn save_wad_report(
    state: &AppState,
    args: &SaveWadParams,
) -> Result<SaveReport, CyberError> {
    let wanted_mode = parse_save_mode(args.mode.as_deref())?;
    let wanted_kind = parse_kind(args.kind.as_deref())?;
    let (target, wad) = state.with_session(|session| {
        let target = resolve_target(session, args.path.as_deref())?;
        ensure_not_layer_file(session, &target)?;
        let wad = match wanted_mode {
            SaveMode::Pwad => working_copy(session, wanted_kind),
            SaveMode::Full => merged_export(session, wanted_kind),
        };
        Ok((target, wad))
    })?;
    let (bytes, bytes_saved_by_dedupe) = if args.dedupe {
        wad.to_bytes_dedup()?
    } else {
        (wad.to_bytes()?, 0_usize)
    };
    write_file_atomic(&target, &bytes, true)?;
    let dirty = record_saved(state, wanted_mode, &target)?;
    Ok(SaveReport {
        path: path_text(&target),
        mode: wanted_mode,
        kind: wad.kind,
        lumps: wad.len(),
        bytes: bytes.len(),
        bytes_saved_by_dedupe,
        dirty,
    })
}

/// The session's dirty flag once a save has landed: a `pwad` write records
/// `target` as the working archive's saved state, a `full` export leaves
/// the session untouched. Takes its own short lock, after the filesystem
/// work is done.
///
/// # Errors
///
/// [`AppState::with_session`]'s poisoned-lock error.
fn record_saved(state: &AppState, mode: SaveMode, target: &Path) -> Result<bool, CyberError> {
    state.with_session(|session| {
        if mode == SaveMode::Pwad {
            session.mark_saved(target.to_path_buf());
        }
        Ok(session.is_dirty())
    })
}
